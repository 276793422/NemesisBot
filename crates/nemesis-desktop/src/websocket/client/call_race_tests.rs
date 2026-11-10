//! call() 挂起点与请求出线的次序回归（BUG-20261110-052）。
//!
//! 修复前：call() 先 try_send 再注册 pending——响应若在 insert 前到达
//! 读循环会被静默丢弃（handle_message 只留 debug 日志），调用方空等
//! 30s 超时。修复后注册先于发送（与 server 侧 call_child 同序）+ 发送
//! 失败回滚。
//!
//! 两个测试分别钉住：
//! 1. 次序不变量：请求到达对端时 pending 必已注册（修复后是同线程
//!    程序序的确定性保证，不会 flake；旧代码下该断言在多线程运行态
//!    可偶发红）；
//! 2. 回滚臂：发送失败后 pending 不残留 oneshot tx。

use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use super::*;
use std::sync::Arc;

fn client_for(port: u16) -> WebSocketClient {
    WebSocketClient::new(&WebSocketKey {
        key: "race-key".to_string(),
        port,
        path: "/race".to_string(),
    })
}

/// fake parent：消费 auth 后等第一条文本帧（= call() 的请求出线），经
/// oneshot 通知测试「已收到请求」，然后 stall 保持连接（永不回包）。
async fn spawn_signaling_parent(signal_tx: oneshot::Sender<()>) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    // 单连接：测试只 connect 一次，oneshot Sender 不可 clone，直接交出
    // 所有权。
    tokio::spawn(async move {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
            return;
        };
        let _ = ws.next().await; // auth 帧
        if let Some(Ok(WsMessage::Text(_))) = ws.next().await {
            let _ = signal_tx.send(());
        }
        tokio::time::sleep(Duration::from_secs(60)).await;
    });
    port
}

/// stall parent：消费 auth 后什么都不做，永不读后续帧（写侧可灌满）。
async fn spawn_stall_parent() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                if let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await {
                    let _ = ws.next().await; // auth 帧
                    tokio::time::sleep(Duration::from_secs(60)).await;
                }
            });
        }
    });
    port
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn race_pending_registered_before_request_hits_the_wire() {
    let (signal_tx, signal_rx) = oneshot::channel();
    let port = spawn_signaling_parent(signal_tx).await;
    let client = Arc::new(client_for(port));
    client.connect().await.unwrap();

    // call 的同步前缀（insert → try_send）在 spawn 任务首次 poll 即执行；
    // 请求出线由写任务接力。多线程运行态下写任务与调用任务并发——
    // 旧代码（send 后才 insert）下信号到达时 pending 可能还没注册；
    // 新代码下 insert 是 try_send 之前的同线程程序序，信号到达时
    // pending 必已在场。
    let caller = {
        let c = client.clone();
        tokio::spawn(async move { c.call("race.probe", serde_json::json!({})).await })
    };

    let arrived = tokio::time::timeout(Duration::from_secs(5), signal_rx)
        .await
        .is_ok();
    assert!(arrived, "request never reached the fake parent");
    assert!(
        !client.pending.lock().is_empty(),
        "pending entry must exist by the time the request reaches the wire (register-before-send)"
    );

    caller.abort();
    client.close();
}

#[tokio::test]
async fn race_send_failure_rolls_back_pending_entry() {
    let port = spawn_stall_parent().await;
    let client = client_for(port);
    client.connect().await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    // 用大通知灌满 64 槽 mpsc + 内核缓冲（parent 永不读）。
    let big = "x".repeat(200_000);
    let mut saturated = false;
    for _ in 0..300 {
        if client
            .notify("bulk", serde_json::json!({ "blob": big }))
            .is_err()
        {
            saturated = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(saturated, "send channel should eventually report full");

    // 通道满时 call() 在发送一步快速失败，且 pending 必须回滚干净
    // （残留 oneshot tx = 泄漏的 map 条目）。
    let err = client
        .call("blocked.request", serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(err.contains("send request"), "unexpected error: {}", err);
    assert!(
        client.pending.lock().is_empty(),
        "pending entry must be rolled back when the send fails"
    );
    client.close();
}

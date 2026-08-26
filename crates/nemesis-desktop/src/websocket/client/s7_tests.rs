//! S7 冲刺覆盖测试：websocket/client.rs 的读/写循环错误分支、pending
//! 管线、以及 call() 对真实本地服务器的完整往返。
//!
//! 与既有测试的区别：既有 round-trip 测试把 `client.call()` 包在 3 秒
//! timeout 里且 `if let` 不做断言（超时也算过），本文件用严格断言钉死
//! 行为——这直接暴露了 pending 双 map 分叉 bug（见 S7 报告 BUG 条目）。

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use super::*;
use crate::websocket::protocol::{Message, ERR_METHOD_NOT_FOUND};
use crate::websocket::server::{KeyGenerator, WebSocketServer};

/// fake parent 在读完 child 的 auth 消息之后的行为。
#[derive(Clone, Copy)]
enum ParentBehavior {
    /// 握手后什么都不做，保持连接（请求永不回复）。
    Stall,
    /// 发一个正常的 WebSocket close 帧后保持连接。
    CloseFrame,
    /// 发一个 binary 帧后保持连接。
    BinaryFrame,
    /// SO_LINGER=0 后丢弃套接字，产生 TCP RST。
    Reset,
    /// 不发 close 帧直接干净关闭 TCP（纯 FIN）。
    FinNoClose,
}

/// 起一个本地 fake parent：接受任意数量的连接，每条连接按 `behavior`
/// 行事。返回监听端口。
async fn spawn_fake_parent(behavior: ParentBehavior) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { break };
            let behavior = behavior;
            tokio::spawn(async move {
                let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
                    return;
                };
                // 消费 auth 消息
                let _ = ws.next().await;
                match behavior {
                    ParentBehavior::Stall => {
                        tokio::time::sleep(Duration::from_secs(60)).await;
                    }
                    ParentBehavior::CloseFrame => {
                        let _ = ws.send(WsMessage::Close(None)).await;
                        tokio::time::sleep(Duration::from_secs(60)).await;
                    }
                    ParentBehavior::BinaryFrame => {
                        let _ = ws.send(WsMessage::Binary(b"s7-binary".to_vec().into())).await;
                        tokio::time::sleep(Duration::from_secs(60)).await;
                    }
                    ParentBehavior::Reset => {
                        // accept_async 直接包 TcpStream，get_ref() 即 &TcpStream；
                        // deprecated 警告是 tokio 对生产代码的提醒，测试里 RST 正是我们要的。
                        #[allow(deprecated)]
                        let _ = ws.get_ref().set_linger(Some(Duration::ZERO));
                        drop(ws);
                    }
                    ParentBehavior::FinNoClose => {
                        drop(ws);
                    }
                }
            });
        }
    });
    port
}

fn client_for(port: u16) -> WebSocketClient {
    WebSocketClient::new(&WebSocketKey {
        key: "s7-key".to_string(),
        port,
        path: "/s7".to_string(),
    })
}

/// 反复 notify 直到写循环死亡（mpsc 接收端被 drop 后 try_send 报错）。
/// 返回 true 表示在轮询预算内观察到通道死亡。
async fn poke_until_write_loop_dead(client: &WebSocketClient) -> bool {
    for _ in 0..100 {
        if client.notify("poke", serde_json::json!({})).is_err() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

#[tokio::test]
async fn s7_close_frame_breaks_read_loop_but_keeps_state() {
    let port = spawn_fake_parent(ParentBehavior::CloseFrame).await;
    let client = client_for(port);
    client.connect().await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    // close 帧只结束读循环；状态位由 close() 复位，写循环仍在跑。
    assert!(client.is_connected());
    assert!(
        client.notify("after.close", serde_json::json!({})).is_ok(),
        "write loop should still be alive after a close frame"
    );
    client.close();
    assert!(!client.is_connected());
}

#[tokio::test]
async fn s7_binary_frame_is_ignored_in_read_loop() {
    let port = spawn_fake_parent(ParentBehavior::BinaryFrame).await;
    let client = client_for(port);
    client.connect().await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    // binary 帧命中忽略分支，两个循环都还活着。
    assert!(client.is_connected());
    assert!(client.notify("after.binary", serde_json::json!({})).is_ok());
    client.close();
}

#[tokio::test]
async fn s7_reset_connection_surfaces_read_and_write_errors() {
    let port = spawn_fake_parent(ParentBehavior::Reset).await;
    let client = client_for(port);
    client.connect().await.unwrap();
    // 等 RST 到达并让读循环退出。
    tokio::time::sleep(Duration::from_millis(200)).await;

    // 写循环在第一次 send 失败后退出，之后的 notify 会因为通道关闭而报错。
    let dead = poke_until_write_loop_dead(&client).await;
    assert!(dead, "write loop should die after the socket reset");
    client.close();
}

#[tokio::test]
async fn s7_fin_without_close_frame_ends_stream() {
    let port = spawn_fake_parent(ParentBehavior::FinNoClose).await;
    let client = client_for(port);
    client.connect().await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    // 对端 FIN 后继续写入：第一次写进内核缓冲可能成功，但 RST 回来后
    // 后续 send 失败 -> 写循环退出 -> 通道关闭。
    let dead = poke_until_write_loop_dead(&client).await;
    assert!(
        dead,
        "write loop should die after the peer closed the socket"
    );
    client.close();
}

#[tokio::test]
async fn s7_reconnect_with_inflight_pending_request() {
    let port = spawn_fake_parent(ParentBehavior::Stall).await;
    let client = client_for(port);
    client.connect().await.unwrap();

    // 发起一个永远不会被应答的 call，150ms 后放弃 future —— pending
    // 请求残留在客户端里（call 的清理块不会运行）。
    let early = tokio::select! {
        r = client.call("slow.request", serde_json::json!({})) => r,
        _ = tokio::time::sleep(Duration::from_millis(150)) => Err(String::new()),
    };
    assert!(early.is_err(), "stalling parent must not answer the call");

    // 旧连接仍被 stalling parent 持有；直接二次 connect。
    client.connect().await.unwrap();
    assert!(client.is_connected());
    client.close();
}

#[tokio::test]
async fn s7_call_fails_when_pending_channel_dropped_by_close() {
    let port = spawn_fake_parent(ParentBehavior::Stall).await;
    let client = std::sync::Arc::new(client_for(port));
    client.connect().await.unwrap();

    let c2 = client.clone();
    let handle =
        tokio::spawn(
            async move { c2.call("never.answered", serde_json::json!({})).await },
        );
    // 等 call 发出请求并注册 pending。
    tokio::time::sleep(Duration::from_millis(200)).await;

    // close() 清掉 pending -> oneshot tx 被 drop -> call 返回 Err。
    client.close();
    let result = tokio::time::timeout(Duration::from_secs(3), handle)
        .await
        .expect("call should finish promptly after close()")
        .unwrap();
    assert_eq!(result.unwrap_err(), "response channel dropped");
}

#[tokio::test(start_paused = true)]
async fn s7_call_times_out_after_30s() {
    let port = spawn_fake_parent(ParentBehavior::Stall).await;
    let client = client_for(port);
    client.connect().await.unwrap();

    // paused 时钟下 30 秒立即快进。
    let result = client.call("never.answered", serde_json::json!({})).await;
    assert_eq!(result.unwrap_err(), "call timeout (30s)");
    client.close();
}

#[tokio::test]
async fn s7_call_errors_when_send_channel_is_saturated() {
    let port = spawn_fake_parent(ParentBehavior::Stall).await;
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

    // 通道满时 call() 在发送一步就快速失败。
    let err = client
        .call("blocked.request", serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(
        err.contains("send request"),
        "unexpected error: {}",
        err
    );
    client.close();
}

/// 关键回归测试：client <-> 本地真实 WebSocketServer 双向往返。
/// (a) server -> client 请求由 client 注册的 handler 应答；
/// (b) client -> server 请求：连接 dispatcher 上没注册 handler，服务端
///     回 method-not-found 错误响应，错误响应也是响应，必须路由回
///     挂起的 call()。
/// 修复前 (b) 会超时（pending 双 map 分叉，响应查不到挂起请求）。
#[tokio::test]
async fn s7_round_trip_with_real_server_both_directions() {
    let key_gen = std::sync::Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen.clone());
    let port = server.start().await.unwrap();
    let key = key_gen.generate("s7-child", 4242);
    let client = WebSocketClient::new(&WebSocketKey {
        key: key.clone(),
        port,
        path: format!("/{}", key),
    });
    client.connect().await.unwrap();
    for _ in 0..150 {
        if server.get_connection("s7-child").is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        server.get_connection("s7-child").is_some(),
        "child connection never registered"
    );

    // (a) server -> client
    client.register_handler("s7.echo", |msg| {
        Ok(Message::new_response(
            msg.id.as_deref().unwrap_or(""),
            serde_json::json!({ "ok": true }),
        ))
    });
    let resp = tokio::time::timeout(
        Duration::from_secs(5),
        server.call_child("s7-child", "s7.echo", serde_json::json!({})),
    )
    .await
    .expect("server->client call timed out");
    assert!(
        resp.is_ok(),
        "server->client call failed: {:?}",
        resp.err().map(|e| e.to_string())
    );
    assert_eq!(resp.unwrap().result.unwrap()["ok"], true);

    // (b) client -> server
    let resp2 = tokio::time::timeout(
        Duration::from_secs(5),
        client.call("no.such.method", serde_json::json!({})),
    )
    .await
    .expect("client->server call timed out: response was never routed back to the pending call");
    let resp2 = resp2.expect("client->server call returned error");
    assert!(
        resp2.is_error_response(),
        "expected method-not-found error response, got: {:?}",
        resp2
    );
    assert_eq!(resp2.error.unwrap().code, ERR_METHOD_NOT_FOUND);

    client.close();
    server.stop();
}

//! S7 冲刺覆盖测试：process/manager.rs 的 spawn_wait_for_result 路径 —
//! approval.submit 通知处理器在「无结果通道」时的警告分支、有通道时的
//! 投递、以及 shutdown 信号分支。
//!
//! rogue client 充当子进程连上 manager 的 WS 服务器（127.0.0.1 随机端口），
//! 不 spawn 任何真实窗口进程。

use std::time::Duration;

use futures_util::SinkExt;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use super::*;
use crate::websocket::protocol::Message;

#[tokio::test]
async fn s7_spawn_wait_for_result_without_and_with_result_channel() {
    let mgr = ProcessManager::new();
    mgr.start().await.unwrap();
    let port = mgr.ws_port();
    let key = mgr.ws_server().key_generator().generate("s7-wait", 77);

    // rogue 子进程：连上并 auth。
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{}/k", port))
        .await
        .unwrap();
    ws.send(WsMessage::Text(
        serde_json::json!({"type": "auth", "key": key})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    for _ in 0..150 {
        if mgr.ws_server().get_connection("s7-wait").is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        mgr.ws_server().get_connection("s7-wait").is_some(),
        "rogue child never registered"
    );

    // 连接已存在，spawn_wait_for_result 第一次轮询即命中。
    mgr.spawn_wait_for_result("s7-wait".to_string());
    tokio::time::sleep(Duration::from_millis(200)).await; // 等处理器注册

    // (a) 无结果通道：处理器触发后走 "no result channel" 警告分支。
    let note = Message::new_notification(
        "approval.submit",
        serde_json::json!({"action": "approved", "request_id": "r1"}),
    );
    ws.send(WsMessage::Text(
        serde_json::to_string(&note).unwrap().into(),
    ))
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    // (b) 有结果通道：结果必须送达 oneshot 接收端。
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    {
        let mut s = mgr.state.lock();
        s.result_channels.insert("s7-wait".to_string(), tx);
    }
    let note2 = Message::new_notification(
        "approval.submit",
        serde_json::json!({"action": "rejected", "request_id": "r2"}),
    );
    ws.send(WsMessage::Text(
        serde_json::to_string(&note2).unwrap().into(),
    ))
    .await
    .unwrap();
    let got = tokio::time::timeout(Duration::from_secs(3), &mut rx)
        .await
        .expect("result was never delivered to the result channel")
        .unwrap();
    assert_eq!(got["action"], "rejected");
    assert_eq!(got["request_id"], "r2");

    // (c) shutdown 信号分支：stop() 唤醒等待任务并清理通道。
    mgr.stop().unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
}

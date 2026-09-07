//! M1a（2026-09-05）：`pump_agent_events` 单元测试。
//!
//! 路由语义：`web:` 前缀 chat_id → 该 session 的 SendQueue 收到
//! `{type:"push", cmd:"tool_event"}` 帧；所有事件（含非 web）都进 EventHub。

use std::sync::Arc;

use tokio::sync::{mpsc, watch};

use crate::events::EventHub;
use crate::server::pump_agent_events;
use crate::session::SessionManager;
use crate::websocket_handler::SendQueue;
use nemesis_types::agent::AgentEvent;

/// 挂一个测试 SendQueue 到 session（真实 WS sink 的测试替身）。
fn attach_fake_queue(manager: &SessionManager, session_id: &str) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel::<Vec<u8>>(16);
    let (_done_tx, done_rx) = watch::channel(false);
    let queue = Arc::new(SendQueue::from_channels(tx, done_rx));
    manager.set_send_queue(session_id, queue);
    rx
}

#[tokio::test]
async fn web_chat_id_routed_to_session_and_event_hub() {
    let manager = SessionManager::with_default_timeout();
    let session = manager.create_session();
    let mut queue_rx = attach_fake_queue(&manager, &session.id);

    let event_hub = Arc::new(EventHub::new());
    let mut hub_rx = event_hub.subscribe();

    let (tx, rx) = tokio::sync::broadcast::channel::<AgentEvent>(16);
    let pump = tokio::spawn(pump_agent_events(rx, Arc::new(manager), event_hub.clone()));

    tx.send(AgentEvent::ToolStarted {
        session_key: format!("web:{}:main", session.id),
        chat_id: format!("web:{}", session.id),
        call_id: "c1".into(),
        tool: "exec".into(),
        args_preview: "{}".into(),
    })
    .unwrap();

    // Session 侧：收到 push 帧且 cmd/tool_event 正确。
    let frame = tokio::time::timeout(std::time::Duration::from_secs(5), queue_rx.recv())
        .await
        .expect("timed out waiting for session frame")
        .expect("queue closed");
    let text = String::from_utf8(frame).unwrap();
    assert!(text.contains("\"type\":\"push\""), "frame: {text}");
    assert!(text.contains("\"cmd\":\"tool_event\""), "frame: {text}");
    assert!(text.contains("\"tool\":\"exec\""), "frame: {text}");

    // EventHub 侧：同名事件发布（订阅先于 send，broadcast 不丢）。
    let ev = tokio::time::timeout(std::time::Duration::from_secs(5), hub_rx.recv())
        .await
        .expect("timed out waiting for hub event")
        .expect("hub closed");
    assert_eq!(ev.event_type, "tool_event");

    pump.abort();
}

#[tokio::test]
async fn non_web_chat_id_only_goes_to_event_hub() {
    let manager = SessionManager::with_default_timeout();
    // 没有 session、没有 queue——非 web chat_id 不得触碰任何 session。
    let event_hub = Arc::new(EventHub::new());
    let mut hub_rx = event_hub.subscribe();

    let (tx, rx) = tokio::sync::broadcast::channel::<AgentEvent>(16);
    let pump = tokio::spawn(pump_agent_events(rx, Arc::new(manager), event_hub.clone()));

    tx.send(AgentEvent::ToolFinished {
        session_key: "telegram:42:main".into(),
        chat_id: "telegram:42".into(),
        call_id: "c2".into(),
        tool: "exec".into(),
        duration_ms: 7,
        ok: true,
        result_preview: "done".into(),
    })
    .unwrap();

    let ev = tokio::time::timeout(std::time::Duration::from_secs(5), hub_rx.recv())
        .await
        .expect("timed out waiting for hub event")
        .expect("hub closed");
    assert_eq!(ev.event_type, "tool_event");
    assert_eq!(ev.data["kind"], "ToolFinished");
    assert_eq!(ev.data["data"]["tool"], "exec");

    pump.abort();
}

#[tokio::test]
async fn session_without_queue_errors_but_pump_survives() {
    // web: 前缀但 session 不存在 → broadcast 返回 Err，pump 必须吞掉并继续。
    let manager = SessionManager::with_default_timeout();
    let event_hub = Arc::new(EventHub::new());

    let (tx, rx) = tokio::sync::broadcast::channel::<AgentEvent>(16);
    let pump = tokio::spawn(pump_agent_events(rx, Arc::new(manager), event_hub.clone()));

    tx.send(AgentEvent::TodoUpdated {
        session_key: "web:ghost:main".into(),
        chat_id: "web:ghost".into(),
        todos: Vec::new(),
    })
    .unwrap();
    // 事件 2：pump 没被前一个错误杀死则正常进 hub。
    tx.send(AgentEvent::TodoUpdated {
        session_key: "web:ghost2:main".into(),
        chat_id: "web:ghost2".into(),
        todos: Vec::new(),
    })
    .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(!pump.is_finished(), "pump died on session broadcast error");

    pump.abort();
}

#[tokio::test]
async fn closed_channel_exits_pump() {
    let manager = SessionManager::with_default_timeout();
    let event_hub = Arc::new(EventHub::new());

    let (tx, rx) = tokio::sync::broadcast::channel::<AgentEvent>(16);
    let pump = tokio::spawn(pump_agent_events(rx, Arc::new(manager), event_hub));

    drop(tx); // 关闭通道 → pump 退出（gateway 停机路径）。
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), pump).await;
    assert!(result.is_ok(), "pump should exit after channel close");
}

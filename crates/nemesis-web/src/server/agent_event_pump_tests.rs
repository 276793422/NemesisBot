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

// -------------------------------------------------------------------------
// BUG-B（2026-09-20）：四个审批/提问事件的 SSE 载荷必须展平为内层 data
// （前端 useApprovals/useQuestions 直接读平铺字段，曾收到 adjacently
// tagged 整体 `{kind,data}` 静默丢弃 → 弹窗「刷新才见」）。
// -------------------------------------------------------------------------

#[tokio::test]
async fn approval_requested_sse_payload_flattened() {
    let event_hub = Arc::new(EventHub::new());
    let mut hub_rx = event_hub.subscribe();

    let (tx, rx) = tokio::sync::broadcast::channel::<AgentEvent>(16);
    let pump = tokio::spawn(pump_agent_events(
        rx,
        Arc::new(SessionManager::with_default_timeout()),
        event_hub.clone(),
    ));

    tx.send(AgentEvent::ApprovalRequested {
        session_key: "agent:main:session:s1".into(),
        chat_id: "web:conn-9".into(),
        request_id: "req-77".into(),
        operation: "process_exec".into(),
        target: "python --version".into(),
        risk_level: "MEDIUM".into(),
        reason: "exec rule matched".into(),
        timeout_secs: 300,
        pattern: "python".into(),
    })
    .unwrap();

    let ev = tokio::time::timeout(std::time::Duration::from_secs(5), hub_rx.recv())
        .await
        .expect("timed out waiting for hub event")
        .expect("hub closed");
    assert_eq!(ev.event_type, "approval-requested");
    // 展平断言：request_id 等平铺字段直接在顶层（前端类型卫以此判活），
    // 不得再包 `kind`/`data` 一层。
    assert_eq!(ev.data["request_id"], "req-77", "payload: {}", ev.data);
    assert_eq!(ev.data["operation"], "process_exec");
    assert_eq!(ev.data["target"], "python --version");
    assert_eq!(ev.data["timeout_secs"], 300);
    assert!(
        ev.data.get("kind").is_none(),
        "payload must be flattened: {}",
        ev.data
    );

    pump.abort();
}

#[tokio::test]
async fn approval_resolved_sse_payload_flattened() {
    let event_hub = Arc::new(EventHub::new());
    let mut hub_rx = event_hub.subscribe();

    let (tx, rx) = tokio::sync::broadcast::channel::<AgentEvent>(16);
    let pump = tokio::spawn(pump_agent_events(
        rx,
        Arc::new(SessionManager::with_default_timeout()),
        event_hub.clone(),
    ));

    tx.send(AgentEvent::ApprovalResolved {
        request_id: "req-77".into(),
        decision: "approved".into(),
    })
    .unwrap();

    let ev = tokio::time::timeout(std::time::Duration::from_secs(5), hub_rx.recv())
        .await
        .expect("timed out waiting for hub event")
        .expect("hub closed");
    assert_eq!(ev.event_type, "approval-resolved");
    assert_eq!(ev.data["request_id"], "req-77", "payload: {}", ev.data);
    assert_eq!(ev.data["decision"], "approved");
    assert!(
        ev.data.get("kind").is_none(),
        "payload must be flattened: {}",
        ev.data
    );

    pump.abort();
}

#[tokio::test]
async fn question_asked_sse_payload_flattened() {
    let event_hub = Arc::new(EventHub::new());
    let mut hub_rx = event_hub.subscribe();

    let (tx, rx) = tokio::sync::broadcast::channel::<AgentEvent>(16);
    let pump = tokio::spawn(pump_agent_events(
        rx,
        Arc::new(SessionManager::with_default_timeout()),
        event_hub.clone(),
    ));

    tx.send(AgentEvent::QuestionAsked {
        session_key: "agent:main:session:s1".into(),
        chat_id: "web:conn-9".into(),
        question_id: "q-1".into(),
        question: "选哪种方案?".into(),
        options: vec!["A".into(), "B".into()],
        multi: false,
        timeout_secs: 120,
    })
    .unwrap();

    let ev = tokio::time::timeout(std::time::Duration::from_secs(5), hub_rx.recv())
        .await
        .expect("timed out waiting for hub event")
        .expect("hub closed");
    assert_eq!(ev.event_type, "question-asked");
    assert_eq!(ev.data["question_id"], "q-1", "payload: {}", ev.data);
    assert_eq!(ev.data["options"][0], "A");
    assert_eq!(ev.data["multi"], false);
    assert!(
        ev.data.get("kind").is_none(),
        "payload must be flattened: {}",
        ev.data
    );

    pump.abort();
}

// -------------------------------------------------------------------------
// BUG-A（2026-09-20）：tool_event push 帧内层注入 `session_id`（会话 id，
// session_key 末段）——前端按 currentId 精确过滤（chat_id 是连接级 id，
// 恒不等曾致全部实时帧被丢弃）。
// -------------------------------------------------------------------------

#[tokio::test]
async fn tool_event_frame_injects_session_id_from_session_key() {
    let manager = SessionManager::with_default_timeout();
    let session = manager.create_session();
    let mut queue_rx = attach_fake_queue(&manager, &session.id);

    let event_hub = Arc::new(EventHub::new());
    // 订阅先于发事件（broadcast 无订阅者即丢，publish 只落 replay_buf）。
    let mut hub_rx = event_hub.subscribe();
    let (tx, rx) = tokio::sync::broadcast::channel::<AgentEvent>(16);
    let pump = tokio::spawn(pump_agent_events(rx, Arc::new(manager), event_hub.clone()));

    // 生产形态：session_key = `agent:main:session:{前端会话id}`，chat_id =
    // `web:{连接id}`（两域不同——正是 BUG-A 的错配源头）。
    tx.send(AgentEvent::TodoUpdated {
        session_key: format!("agent:main:session:{}", session.id),
        chat_id: format!("web:{}", session.id),
        todos: vec![nemesis_types::agent::TodoItem {
            content: "step".into(),
            status: nemesis_types::agent::TodoStatus::InProgress,
        }],
    })
    .unwrap();

    let frame = tokio::time::timeout(std::time::Duration::from_secs(5), queue_rx.recv())
        .await
        .expect("timed out waiting for session frame")
        .expect("queue closed");
    let text = String::from_utf8(frame).unwrap();
    // 会话 id 注入到帧 data 内层（AgentEvent 序列化的 data 对象）。
    // WS 帧形状：{type,module,cmd,data:{kind,data:{载荷,session_id}}}。
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    let inner = &parsed["data"]["data"];
    assert_eq!(
        inner["session_id"],
        session.id.as_str(),
        "session_id must be injected from session_key tail: {text}"
    );
    assert_eq!(parsed["data"]["kind"], "TodoUpdated");

    // SSE 侧同帧同样带注入（EventHub 与 WS push 共用同一 data）。
    let ev = tokio::time::timeout(std::time::Duration::from_secs(5), hub_rx.recv())
        .await
        .expect("timed out waiting for hub event")
        .expect("hub closed");
    assert_eq!(ev.event_type, "tool_event");
    assert_eq!(ev.data["data"]["session_id"], session.id.as_str());

    pump.abort();
}

#[tokio::test]
async fn question_resolved_sse_payload_flattened() {
    // 四个 SSE 分支事件全覆盖：QuestionResolved 同族展平（前端 removeLocal
    // 读平铺 question_id），且不落 tool_event 通道。
    let event_hub = Arc::new(EventHub::new());
    let mut hub_rx = event_hub.subscribe();

    let (tx, rx) = tokio::sync::broadcast::channel::<AgentEvent>(16);
    let pump = tokio::spawn(pump_agent_events(
        rx,
        Arc::new(SessionManager::with_default_timeout()),
        event_hub.clone(),
    ));

    tx.send(AgentEvent::QuestionResolved {
        question_id: "q-2".into(),
        decision: "timeout".into(),
    })
    .unwrap();

    let ev = tokio::time::timeout(std::time::Duration::from_secs(5), hub_rx.recv())
        .await
        .expect("timed out waiting for hub event")
        .expect("hub closed");
    assert_eq!(ev.event_type, "question-resolved");
    assert_eq!(ev.data["question_id"], "q-2", "payload: {}", ev.data);
    assert_eq!(ev.data["decision"], "timeout");
    assert!(
        ev.data.get("kind").is_none(),
        "payload must be flattened: {}",
        ev.data
    );

    pump.abort();
}

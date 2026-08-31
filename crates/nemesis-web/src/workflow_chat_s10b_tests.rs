//! S10b (quality-hardening goal 冲刺, web 批次 2): workflow_chat history
//! arms + send unknown-index arm not reached by `workflow_chat_extra_tests`
//! (which covers the invalid-data / no-engine / human-review cases):
//!
//! - `history_request` with an index no registered workflow owns → the
//!   "未找到工作流" send_error path (queue-less session → log_send_error warn)
//! - `history_request` happy path: chat_log page read + response encode; the
//!   queue-less broadcast surfaces as the handler's Err
//! - `send` with unknown index → the send-side "未找到工作流" arm
//!
//! chat_log rows use nanos-unique workflow names + delete_chat_log cleanup
//! (house pattern — the log root is the process-global path manager).

use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::protocol::ProtocolMessage;
use crate::session::SessionManager;
use crate::workflow_chat::handle_workflow_chat_message;
use nemesis_workflow::engine::WorkflowEngine;
use nemesis_workflow::types::{NodeDef, Workflow};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn make_state(engine: Option<Arc<WorkflowEngine>>) -> Arc<AppState> {
    Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: None,
        home: None,
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("m".to_string())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
        session_manager: Arc::new(SessionManager::with_default_timeout()),
        inbound_tx: None,
        streaming_provider: None,
        ws_router: None,
        agent_service: None,
        data_store: None,
        memory_manager: None,
        forge: None,
        agent_loop: Arc::new(parking_lot::RwLock::new(None)),
        cluster: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: engine,
        #[cfg(feature = "workflow")]
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: std::sync::Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
        board: None,
    })
}

fn unique_wf() -> String {
    format!(
        "wfs10b{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn registered_engine(name: &str) -> Arc<WorkflowEngine> {
    let engine = WorkflowEngine::new();
    engine
        .register_workflow(Workflow {
            name: name.to_string(),
            description: String::new(),
            version: "1.0.0".to_string(),
            triggers: Vec::new(),
            nodes: vec![NodeDef {
                id: "n1".to_string(),
                node_type: "transform".to_string(),
                config: HashMap::new(),
                depends_on: Vec::new(),
                retry_count: 0,
                timeout: None,
                is_terminal: true,
            }],
            edges: Vec::new(),
            variables: HashMap::new(),
            metadata: HashMap::new(),
        })
        .expect("register workflow");
    Arc::new(engine)
}

fn pm(cmd: &str, data: serde_json::Value) -> ProtocolMessage {
    ProtocolMessage::new("message", "workflow_chat", cmd, Some(data))
}

#[tokio::test]
async fn history_unknown_index_reports_not_found_without_failing() {
    let state = make_state(Some(Arc::new(WorkflowEngine::new())));
    // Queue-less session → send_error's broadcast fails into the
    // log_send_error warn arm; the handler still returns Ok.
    let res = handle_workflow_chat_message(
        state.clone(),
        "sess-no-queue".to_string(),
        "chat-1".to_string(),
        pm(
            "history_request",
            serde_json::json!({ "index": "deadbeef", "request_id": "r1" }),
        ),
    )
    .await;
    assert!(res.is_ok(), "unknown index is reported to the client, not the caller");
}

#[tokio::test]
async fn history_happy_path_reads_chat_log_and_surfaces_broadcast_failure() {
    let name = unique_wf();
    let engine = registered_engine(&name);
    let index = WorkflowEngine::chat_index(&name);
    let key = format!("wf_chat:{}", name);
    for i in 0..3 {
        nemesis_agent::chat_log::append_chat_log(&key, if i % 2 == 0 { "user" } else { "assistant" }, &format!("消息{}", i));
    }

    let state = make_state(Some(engine));
    let res = handle_workflow_chat_message(
        state.clone(),
        "sess-no-queue".to_string(),
        "chat-1".to_string(),
        pm(
            "history_request",
            serde_json::json!({ "index": index, "request_id": "r2", "limit": 2 }),
        ),
    )
    .await;
    // The read + encode succeeded; the queue-less broadcast is the only
    // failure, surfaced as the handler's Err.
    let err = res.expect_err("broadcast must fail on a queue-less session");
    assert!(
        err.contains("failed to broadcast history response"),
        "got: {}",
        err
    );

    nemesis_agent::chat_log::delete_chat_log(&key);
}

#[tokio::test]
async fn send_unknown_index_reports_not_found_without_failing() {
    let state = make_state(Some(Arc::new(WorkflowEngine::new())));
    let res = handle_workflow_chat_message(
        state.clone(),
        "sess-no-queue".to_string(),
        "chat-1".to_string(),
        pm(
            "send",
            serde_json::json!({ "index": "deadbeef", "content": "你好" }),
        ),
    )
    .await;
    assert!(res.is_ok(), "unknown index is reported to the client, not the caller");
}

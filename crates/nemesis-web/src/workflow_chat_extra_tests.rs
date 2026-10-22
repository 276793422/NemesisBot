//! Coverage for `workflow_chat` — exercises the protocol/parameter-parse and
//! engine-missing branches (the engine-present path needs a full running
//! workflow and is covered by integration tests elsewhere).

use crate::api_handlers::AppState;
use crate::protocol::ProtocolMessage;
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
        model_name: Arc::new(parking_lot::Mutex::new(String::new())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(crate::events::EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
        session_manager: Arc::new(crate::session::SessionManager::with_default_timeout()),
        inbound_tx: None,
        streaming_provider: None,
        ws_router: None,
        agent_service: None,
        data_store: None,
        memory_manager: None,
        forge: None,
        agent_loop: Arc::new(parking_lot::RwLock::new(None)),
        cluster: None,
        quic_relay: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: engine,
        chat_secret_store: Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: None,
        estop: None,
        signature_verify: None,
        skills_install_gate: None,
        cron: None,
        board: None,
    })
}

fn pm(cmd: &str, data: Option<serde_json::Value>) -> ProtocolMessage {
    ProtocolMessage::new("request", "workflow_chat", cmd, data)
}

#[tokio::test]
async fn unknown_cmd_returns_error() {
    let state = make_state(None);
    let res =
        handle_workflow_chat_message(state, "s".into(), "c".into(), pm("frobnicate", None), None)
            .await;
    assert!(res.is_err());
    assert!(res.unwrap_err().contains("unknown workflow_chat cmd"));
}

#[tokio::test]
async fn send_invalid_data_returns_error() {
    let state = make_state(None);
    // data is a string, not an object → decode fails.
    let res = handle_workflow_chat_message(
        state,
        "s".into(),
        "c".into(),
        pm("send", Some(serde_json::json!("not an object"))),
        None,
    )
    .await;
    assert!(res.is_err());
    assert!(res.unwrap_err().contains("invalid workflow_chat.send data"));
}

#[tokio::test]
async fn send_empty_content_returns_error() {
    let state = make_state(None);
    let res = handle_workflow_chat_message(
        state,
        "s".into(),
        "c".into(),
        pm(
            "send",
            Some(serde_json::json!({"index": "x", "content": "   "})),
        ),
        None,
    )
    .await;
    assert!(res.is_err());
    assert!(res.unwrap_err().contains("cannot be empty"));
}

#[tokio::test]
async fn send_no_engine_returns_ok_with_error_message() {
    // workflow_engine is None → send_error to client, return Ok (not Err).
    let state = make_state(None);
    let res = handle_workflow_chat_message(
        state,
        "s".into(),
        "c".into(),
        pm(
            "send",
            Some(serde_json::json!({"index": "x", "content": "hi"})),
        ),
        None,
    )
    .await;
    assert!(res.is_ok());
}

#[tokio::test]
async fn history_request_invalid_data_returns_error() {
    let state = make_state(None);
    let res = handle_workflow_chat_message(
        state,
        "s".into(),
        "c".into(),
        pm("history_request", Some(serde_json::json!(42))),
        None,
    )
    .await;
    assert!(res.is_err());
    assert!(
        res.unwrap_err()
            .contains("invalid workflow_chat.history_request data")
    );
}

#[tokio::test]
async fn history_request_no_engine_returns_ok() {
    let state = make_state(None);
    let res = handle_workflow_chat_message(
        state,
        "s".into(),
        "c".into(),
        pm(
            "history_request",
            Some(serde_json::json!({"index": "x", "request_id": "r1"})),
        ),
        None,
    )
    .await;
    assert!(res.is_ok());
}

// ---- engine-present branches ----

fn make_node(id: &str, node_type: &str, deps: Vec<&str>) -> NodeDef {
    NodeDef {
        id: id.to_string(),
        node_type: node_type.to_string(),
        config: HashMap::new(),
        depends_on: deps.into_iter().map(|s| s.to_string()).collect(),
        retry_count: 0,
        timeout: None,
        is_terminal: false,
    }
}

fn make_workflow(name: &str, nodes: Vec<NodeDef>) -> Workflow {
    Workflow {
        name: name.to_string(),
        description: String::new(),
        version: "1.0.0".to_string(),
        triggers: vec![],
        nodes,
        edges: vec![],
        variables: HashMap::new(),
        metadata: HashMap::new(),
    }
}

#[tokio::test]
async fn send_engine_index_not_found_logs_error() {
    // Engine present and has a registered workflow, but the client sends
    // an index that resolves to nothing → log_send_error + Ok(()).
    let engine = Arc::new(WorkflowEngine::new());
    engine
        .register_workflow(make_workflow(
            "other_wf",
            vec![make_node("n1", "delay", vec![])],
        ))
        .unwrap();
    let state = make_state(Some(engine));

    let res = handle_workflow_chat_message(
        state,
        "sess".into(),
        "web:sess".into(),
        pm(
            "send",
            Some(serde_json::json!({
                "index": "deadbeef",
                "content": "hi",
            })),
        ),
        None,
    )
    .await;
    // workflow_by_chat_index returns None → log_send_error → Ok(())
    assert!(res.is_ok());
}

#[tokio::test]
async fn send_engine_human_review_workflow_rejected() {
    // A workflow that contains a human_review node must be rejected up
    // front — chatting it would hang the UI waiting on a review.
    let engine = Arc::new(WorkflowEngine::new());
    let name = "review_wf";
    engine
        .register_workflow(make_workflow(
            name,
            vec![make_node("n1", "human_review", vec![])],
        ))
        .unwrap();
    let index = WorkflowEngine::chat_index(name);
    let state = make_state(Some(engine));

    let res = handle_workflow_chat_message(
        state,
        "sess".into(),
        "web:sess".into(),
        pm(
            "send",
            Some(serde_json::json!({ "index": index, "content": "go" })),
        ),
        None,
    )
    .await;
    // Rejected before start_async → Ok(()), no execution spawned.
    assert!(res.is_ok());
}

#[tokio::test]
async fn send_engine_starts_workflow_returns_ok() {
    // Happy path: index resolves, no human_review, start_async succeeds.
    // handle_send writes the user turn to chat_log, stores the per-workflow
    // mutex guard, and returns Ok(()). The background execution is
    // detached; its outcome does not affect this return value.
    let engine = Arc::new(WorkflowEngine::new());
    let name = "chat_ok_wf";
    engine
        .register_workflow(make_workflow(name, vec![make_node("n1", "delay", vec![])]))
        .unwrap();
    let index = WorkflowEngine::chat_index(name);
    let state = make_state(Some(engine));

    let res = handle_workflow_chat_message(
        state,
        "sess".into(),
        "web:sess".into(),
        pm(
            "send",
            Some(serde_json::json!({ "index": index, "content": "hello" })),
        ),
        None,
    )
    .await;
    assert!(res.is_ok());
}

#[tokio::test]
async fn history_request_engine_present_broadcasts_response() {
    // Engine present, index resolves → reads (empty) chat_log and builds a
    // history_response. No send queue is registered for this session in a
    // unit test, so the final broadcast returns Err — but reaching the
    // broadcast proves workflow_by_chat_index resolved, chat_log was read,
    // and the response was encoded. Assert we got that far (an earlier
    // "引擎未启用"/"未找到工作流" branch would surface a different error).
    let engine = Arc::new(WorkflowEngine::new());
    let name = "hist_wf";
    engine
        .register_workflow(make_workflow(name, vec![make_node("n1", "delay", vec![])]))
        .unwrap();
    let index = WorkflowEngine::chat_index(name);
    let state = make_state(Some(engine));

    let res = handle_workflow_chat_message(
        state,
        "sess".into(),
        "web:sess".into(),
        pm(
            "history_request",
            Some(serde_json::json!({ "index": index, "request_id": "r1" })),
        ),
        None,
    )
    .await;
    match res {
        Ok(()) => {}
        Err(e) => assert!(
            e.contains("broadcast"),
            "expected to reach broadcast step, got: {}",
            e
        ),
    }
}

// ---------------------------------------------------------------------------
// 2026-10-16 hardening: history limit clamp + bound-workflow precedence
// ---------------------------------------------------------------------------

#[test]
fn history_limit_clamp_table() {
    // clamp_history_limit is the single source of truth for the page-size cap:
    // missing/0 → default 50; 1..=200 pass through; >200 clamp to 200.
    use crate::workflow_chat::clamp_history_limit as clamp;
    assert_eq!(clamp(None), 50);
    assert_eq!(clamp(Some(0)), 50);
    assert_eq!(clamp(Some(1)), 1);
    assert_eq!(clamp(Some(49)), 49);
    assert_eq!(clamp(Some(50)), 50);
    assert_eq!(clamp(Some(200)), 200);
    assert_eq!(clamp(Some(201)), 200);
    assert_eq!(clamp(Some(100_000)), 200, "whole-file threshold must clamp");
    assert_eq!(clamp(Some(usize::MAX)), 200);
}

/// Register a capture send queue for `session_id` so broadcasts can be
/// received in-test (test_channels queue without a real socket).
fn capture_queue(state: &AppState, session_id: &str) -> tokio::sync::mpsc::Receiver<Vec<u8>> {
    let (queue, rx, _lo_rx, _done_tx) = crate::websocket_handler::SendQueue::test_channels(64);
    state
        .session_manager
        .set_send_queue(session_id, std::sync::Arc::new(queue));
    rx
}

#[tokio::test]
async fn bound_send_pins_workflow_ignoring_client_index() {
    // Both workflows are registered, so the client-supplied index WOULD
    // resolve on its own — the bound session must still land on its own
    // workflow only.
    let engine = Arc::new(WorkflowEngine::new());
    engine
        .register_workflow(make_workflow(
            "bound_pin_wf",
            vec![make_node("n1", "delay", vec![])],
        ))
        .unwrap();
    engine
        .register_workflow(make_workflow(
            "bound_other_wf",
            vec![make_node("n1", "delay", vec![])],
        ))
        .unwrap();
    let state = make_state(Some(engine));

    let other_index = WorkflowEngine::chat_index("bound_other_wf");
    let res = handle_workflow_chat_message(
        state.clone(),
        "sess-pin".into(),
        "web:sess-pin".into(),
        pm(
            "send",
            Some(serde_json::json!({
                "index": other_index,
                "content": "hello pinned",
            })),
        ),
        Some("bound_pin_wf".to_string()),
    )
    .await;
    assert!(res.is_ok());

    // The user turn must be recorded under the BOUND workflow's chat log...
    let (page, total, _, _) =
        nemesis_agent::chat_log::read_chat_log_async("wf_chat:bound_pin_wf", 10, None).await;
    assert!(total >= 1, "bound workflow log must have the user turn");
    assert!(
        page.iter()
            .any(|m| m["content"].as_str().unwrap_or_default().contains("pinned")),
        "user content missing from bound workflow log: {:?}",
        page
    );
    // ...and the client-indexed workflow must have received nothing.
    let (_, other_total, _, _) =
        nemesis_agent::chat_log::read_chat_log_async("wf_chat:bound_other_wf", 10, None).await;
    assert_eq!(other_total, 0, "client index must be ignored when bound");

    // chat_log persists to the default home across runs — clean up.
    nemesis_agent::chat_log::delete_chat_log("wf_chat:bound_pin_wf");
}

#[tokio::test]
async fn bound_history_pins_workflow_ignoring_client_index() {
    // With a bound workflow, even a bogus client index must not block the
    // history read — resolution goes through the bound workflow, so the
    // history_response broadcast fires (a dashboard-style session with a
    // bogus index would have answered "未找到工作流" instead).
    let engine = Arc::new(WorkflowEngine::new());
    engine
        .register_workflow(make_workflow(
            "bound_hist_wf",
            vec![make_node("n1", "delay", vec![])],
        ))
        .unwrap();
    let state = make_state(Some(engine));
    let mut rx = capture_queue(&state, "sess-hist");

    let res = handle_workflow_chat_message(
        state,
        "sess-hist".into(),
        "web:sess-hist".into(),
        pm(
            "history_request",
            Some(serde_json::json!({
                "index": "ffffffffffffffff",
                "request_id": "bh1",
            })),
        ),
        Some("bound_hist_wf".to_string()),
    )
    .await;
    assert!(res.is_ok(), "bound history must not fail: {:?}", res.err());

    let bytes = rx.recv().await.expect("history_response expected");
    let resp: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(resp["cmd"], "history_response");
    assert_eq!(resp["data"]["request_id"], "bh1");
}

#[tokio::test]
async fn history_limit_clamped_end_to_end() {
    // 210 entries in the log; a client asking for limit=100_000 (the
    // whole-file threshold) must get a 200-entry page, not the full log;
    // a missing limit must get the 50-entry default. The workflow name
    // carries a per-run suffix: chat_log persists across cargo-test runs,
    // so a fixed key would accumulate and break the exact total asserts.
    let name = format!(
        "bound_clamp_wf_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );
    let engine = Arc::new(WorkflowEngine::new());
    engine
        .register_workflow(make_workflow(&name, vec![make_node("n1", "delay", vec![])]))
        .unwrap();
    let state = make_state(Some(engine));
    let mut rx = capture_queue(&state, "sess-clamp");

    let session_key = format!("wf_chat:{name}");
    for i in 0..210 {
        nemesis_agent::chat_log::append_chat_log(&session_key, "user", &format!("msg-{i}"));
    }

    for (request_id, limit, expected_page) in [
        ("clamp-huge", serde_json::json!(100_000u64), 200),
        ("clamp-default", serde_json::Value::Null, 50),
        ("clamp-small", serde_json::json!(7u64), 7),
    ] {
        let mut data = serde_json::json!({
            "index": "ffffffffffffffff",
            "request_id": request_id,
        });
        if !limit.is_null() {
            data["limit"] = limit;
        }
        let res = handle_workflow_chat_message(
            state.clone(),
            "sess-clamp".into(),
            "web:sess-clamp".into(),
            pm("history_request", Some(data)),
            Some(name.clone()),
        )
        .await;
        assert!(res.is_ok(), "{request_id}: {:?}", res.err());

        let bytes = rx.recv().await.expect("history_response expected");
        let resp: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(resp["data"]["request_id"], request_id);
        assert_eq!(
            resp["data"]["total_count"], 210,
            "{request_id}: total must be the full log size"
        );
        let page = resp["data"]["messages"].as_array().unwrap();
        assert_eq!(
            page.len(),
            expected_page,
            "{request_id}: page size must be clamped"
        );
    }

    // chat_log persists to the default home across runs — clean up.
    nemesis_agent::chat_log::delete_chat_log(&session_key);
}

//! Coverage for `workflow_chat` — exercises the protocol/parameter-parse and
//! engine-missing branches (the engine-present path needs a full running
//! workflow and is covered by integration tests elsewhere).

#[cfg(test)]
mod workflow_chat_extra_tests {
    use crate::api_handlers::AppState;
    use crate::protocol::ProtocolMessage;
    use crate::workflow_chat::handle_workflow_chat_message;
    use nemesis_workflow::engine::WorkflowEngine;
    use nemesis_workflow::types::{NodeDef, Workflow};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::sync::Arc;
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
            cluster_service: None,
            cluster_log_dir: None,
            workflow_engine: engine,
            chat_secret_store: Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
            webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
            internal_cmd_tx: None,
            estop: None,
            cron: None,
        })
    }

    fn pm(cmd: &str, data: Option<serde_json::Value>) -> ProtocolMessage {
        ProtocolMessage::new("request", "workflow_chat", cmd, data)
    }

    #[tokio::test]
    async fn unknown_cmd_returns_error() {
        let state = make_state(None);
        let res = handle_workflow_chat_message(state, "s".into(), "c".into(), pm("frobnicate", None)).await;
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
            pm("send", Some(serde_json::json!({"index": "x", "content": "   "}))),
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
            pm("send", Some(serde_json::json!({"index": "x", "content": "hi"}))),
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
        )
        .await;
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("invalid workflow_chat.history_request data"));
    }

    #[tokio::test]
    async fn history_request_no_engine_returns_ok() {
        let state = make_state(None);
        let res = handle_workflow_chat_message(
            state,
            "s".into(),
            "c".into(),
            pm("history_request", Some(serde_json::json!({"index": "x", "request_id": "r1"}))),
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
            .register_workflow(make_workflow(
                name,
                vec![make_node("n1", "delay", vec![])],
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
                Some(serde_json::json!({ "index": index, "content": "hello" })),
            ),
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
            .register_workflow(make_workflow(
                name,
                vec![make_node("n1", "delay", vec![])],
            ))
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
}

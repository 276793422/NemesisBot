//! S10b (quality-hardening goal 冲刺, web 批次 2): AgentHandler arms with a
//! REAL AgentLoop attached (existing tests only cover the None/stub paths) —
//! cancel Some-loop, checkpoints Some-loop, rewind with a live loop but no
//! checkpoint store, and the missing/mistyped `turn` guards.

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_agent::r#loop::{AgentLoop, LlmProvider, LlmResponse};
use nemesis_agent::types::AgentConfig;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

struct NoopProvider;

#[async_trait::async_trait]
impl LlmProvider for NoopProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<nemesis_agent::r#loop::LlmMessage>,
        _options: Option<nemesis_agent::types::ChatOptions>,
        _tools: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Ok(LlmResponse {
            content: "ok".to_string(),
            tool_calls: vec![],
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

fn make_ctx_with_loop() -> RequestContext {
    make_ctx_with_optional_loop(Some(Arc::new(AgentLoop::new(
        Box::new(NoopProvider),
        AgentConfig::default(),
    ))))
}

fn make_ctx_with_optional_loop(al: Option<Arc<AgentLoop>>) -> RequestContext {
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: None,
        home: None,
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new(String::new())),
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
        agent_loop: Arc::new(parking_lot::RwLock::new(al)),
        cluster: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        #[cfg(feature = "workflow")]
        chat_secret_store: Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
    });
    RequestContext {
        session_id: "s10b".to_string(),
        chat_id: "chat".to_string(),
        workspace: None,
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

#[tokio::test]
async fn cancel_with_live_loop_reports_zero_cancelled() {
    let ctx = make_ctx_with_loop();
    let resp = AgentHandler
        .handle_cmd("cancel", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resp["cancelled"], 0, "no active sessions to cancel");
}

#[tokio::test]
async fn checkpoints_with_live_loop_returns_empty_list() {
    let ctx = make_ctx_with_loop();
    let resp = AgentHandler
        .handle_cmd("checkpoints", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(resp["checkpoints"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn rewind_with_live_loop_but_no_checkpoint_store_errors() {
    let ctx = make_ctx_with_loop();
    let handler = AgentHandler;

    // turn field missing / not a u64 → guard errors.
    let err = handler
        .handle_cmd("rewind", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "turn is required");
    let err = handler
        .handle_cmd("rewind", Some(serde_json::json!({ "turn": "x" })), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "turn is required");

    // Live loop without a checkpoint store → rewind propagates the error.
    let err = handler
        .handle_cmd("rewind", Some(serde_json::json!({ "turn": 3 })), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("checkpoint store not attached"), "{err}");
}

// ============================================================
// U7 dashboard visibility (G1): agent.inbox_status handler。
// ============================================================

#[tokio::test]
async fn inbox_status_without_loop_reports_unavailable() {
    let ctx = make_ctx_with_optional_loop(None);
    let resp = AgentHandler
        .handle_cmd(
            "inbox_status",
            Some(serde_json::json!({ "session_id": "x" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resp["available"], false);
    assert_eq!(resp["next_turn"], 0);
    assert_eq!(resp["next_step"], 0);
    assert_eq!(resp["busy"], false);
}

#[tokio::test]
async fn inbox_status_empty_session_reports_idle() {
    let ctx = make_ctx_with_loop();
    let resp = AgentHandler
        .handle_cmd(
            "inbox_status",
            Some(serde_json::json!({ "session_id": "inbox-t" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    // session_key 复刻 server.rs 入站规则：sanitize 过的 id 进 key。
    assert_eq!(resp["session_key"], "agent:main:session:inbox-t");
    assert_eq!(resp["available"], true);
    assert_eq!(resp["next_turn"], 0);
    assert_eq!(resp["next_step"], 0);
    assert_eq!(resp["capacity"], 8, "AgentLoop::new 用 DEFAULT_QUEUE_SIZE");
    assert_eq!(resp["busy"], false);
    // AgentConfig::default() 是 Reject 模式。
    assert_eq!(resp["mode"], "reject");
}

#[tokio::test]
async fn inbox_status_missing_data_maps_to_legacy_key() {
    let ctx = make_ctx_with_loop();
    let resp = AgentHandler
        .handle_cmd("inbox_status", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resp["session_key"], "agent:main:session:legacy");
    assert_eq!(resp["available"], true);
}

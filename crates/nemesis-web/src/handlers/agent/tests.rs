use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::RequestContext;
use nemesis_services::bot_service::{AgentLoopService, LifecycleService};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

/// Mock AgentLoopService — covers the trait so handler commands that depend
/// on `agent_service` (status/start/stop) hit their real Some-branch logic
/// instead of the None stub.
struct MockAgentService {
    running: AtomicBool,
    start_fails: bool,
}
impl MockAgentService {
    fn new(running: bool) -> Self {
        Self {
            running: AtomicBool::new(running),
            start_fails: false,
        }
    }
    fn new_start_fails() -> Self {
        Self {
            running: AtomicBool::new(false),
            start_fails: true,
        }
    }
}
impl LifecycleService for MockAgentService {
    fn start(&self) -> Result<(), String> {
        if self.start_fails {
            return Err("boom".to_string());
        }
        self.running.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn stop(&self) -> Result<(), String> {
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }
    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}
impl AgentLoopService for MockAgentService {}

fn make_ctx_with_agent(svc: Arc<dyn AgentLoopService>) -> RequestContext {
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: None,
        home: None,
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("test-model".to_string())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
        session_manager: Arc::new(SessionManager::with_default_timeout()),
        inbound_tx: None,
        streaming_provider: None,
        ws_router: None,
        agent_service: Some(svc),
        data_store: None,
        memory_manager: None,
        forge: None,
        agent_loop: Arc::new(parking_lot::RwLock::new(None)),
        cluster: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        chat_secret_store: Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
    });
    RequestContext {
        session_id: "s".to_string(),
        chat_id: "c".to_string(),
        workspace: None,
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

#[tokio::test]
async fn status_running_with_service() {
    let handler = AgentHandler;
    let ctx = make_ctx_with_agent(Arc::new(MockAgentService::new(true)));
    let r = handler
        .handle_cmd("status", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["running"], true);
    assert_eq!(r["model_name"], "test-model");
}

#[tokio::test]
async fn status_stopped_with_service() {
    let handler = AgentHandler;
    let ctx = make_ctx_with_agent(Arc::new(MockAgentService::new(false)));
    let r = handler
        .handle_cmd("status", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["running"], false);
}

#[tokio::test]
async fn start_success_sets_started() {
    let handler = AgentHandler;
    let ctx = make_ctx_with_agent(Arc::new(MockAgentService::new(false)));
    let r = handler
        .handle_cmd("start", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["started"], true);
}

#[tokio::test]
async fn start_failure_propagates_error() {
    let handler = AgentHandler;
    let ctx = make_ctx_with_agent(Arc::new(MockAgentService::new_start_fails()));
    let err = handler.handle_cmd("start", None, &ctx).await.unwrap_err();
    assert_eq!(err, "boom");
}

#[tokio::test]
async fn stop_success_sets_stopped() {
    let handler = AgentHandler;
    let ctx = make_ctx_with_agent(Arc::new(MockAgentService::new(true)));
    let r = handler
        .handle_cmd("stop", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["stopped"], true);
}

#[tokio::test]
async fn cancel_without_running_loop_returns_error() {
    let handler = AgentHandler;
    let ctx = make_ctx_with_agent(Arc::new(MockAgentService::new(true)));
    let err = handler.handle_cmd("cancel", None, &ctx).await.unwrap_err();
    assert_eq!(err, "Agent not running");
}

#[tokio::test]
async fn rewind_without_running_loop_returns_error() {
    let handler = AgentHandler;
    let ctx = make_ctx_with_agent(Arc::new(MockAgentService::new(true)));
    let err = handler
        .handle_cmd("rewind", Some(serde_json::json!({"turn": 1})), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "Agent not running");
}

#[tokio::test]
async fn rewind_missing_data_returns_error() {
    let handler = AgentHandler;
    let ctx = make_ctx_with_agent(Arc::new(MockAgentService::new(true)));
    let err = handler.handle_cmd("rewind", None, &ctx).await.unwrap_err();
    assert_eq!(err, "missing data");
}

#[tokio::test]
async fn checkpoints_without_running_loop_returns_empty() {
    let handler = AgentHandler;
    let ctx = make_ctx_with_agent(Arc::new(MockAgentService::new(true)));
    let r = handler
        .handle_cmd("checkpoints", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["checkpoints"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn unknown_command_returns_error() {
    let handler = AgentHandler;
    let ctx = make_ctx_with_agent(Arc::new(MockAgentService::new(true)));
    let err = handler.handle_cmd("bogus", None, &ctx).await.unwrap_err();
    assert!(err.contains("unknown command"));
}

// --- agent_loop Some-branch (cancel / checkpoints / rewind real paths) ---

use nemesis_agent::checkpoint::CheckpointStore;
use nemesis_agent::r#loop::{AgentLoop, LlmMessage, LlmProvider, LlmResponse};
use nemesis_agent::types::AgentConfig;

struct WebMockProvider;
#[async_trait::async_trait]
impl LlmProvider for WebMockProvider {
    async fn chat(
        &self,
        _: &str,
        _: Vec<LlmMessage>,
        _: Option<nemesis_agent::types::ChatOptions>,
        _: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Ok(LlmResponse {
            content: String::new(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

fn make_ctx_with_loop(workspace: Option<String>) -> RequestContext {
    let al = AgentLoop::new(Box::new(WebMockProvider), AgentConfig::default());
    if let Some(ws) = workspace.clone() {
        al.set_checkpoint_store(Arc::new(CheckpointStore::new(
            None,
            std::path::PathBuf::from(ws),
        )));
    }
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: workspace.clone(),
        home: workspace,
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("test-model".to_string())),
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
        agent_loop: Arc::new(parking_lot::RwLock::new(Some(Arc::new(al)))),
        cluster: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        chat_secret_store: Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
    });
    RequestContext {
        session_id: "s".to_string(),
        chat_id: "c".to_string(),
        workspace: None,
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

#[tokio::test]
async fn cancel_with_loop_returns_count() {
    let handler = AgentHandler;
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_loop(Some(dir.path().to_string_lossy().to_string()));
    let r = handler
        .handle_cmd("cancel", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["cancelled"], 0);
}

#[tokio::test]
async fn checkpoints_with_loop_returns_list() {
    let handler = AgentHandler;
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_loop(Some(dir.path().to_string_lossy().to_string()));
    let r = handler
        .handle_cmd("checkpoints", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    // No checkpoint turn begun → empty list.
    assert_eq!(r["checkpoints"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn rewind_nonexistent_turn_returns_empty() {
    let handler = AgentHandler;
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_loop(Some(dir.path().to_string_lossy().to_string()));
    // turn 999 was never begun → rewind returns Ok with empty writes/deletes.
    let r = handler
        .handle_cmd("rewind", Some(serde_json::json!({"turn": 999})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["turn"], 999);
    assert!(r["written"].as_array().unwrap().is_empty());
    assert!(r["deleted"].as_array().unwrap().is_empty());
}

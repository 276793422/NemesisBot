//! Agent handler — status/start/stop.
//!
//! start() triggers the factory to build a fresh AgentLoop from disk config.
//! stop() drops the old AgentLoop entirely.

use crate::handlers::require_home;
use crate::ws_router::{ModuleHandler, RequestContext};

pub struct AgentHandler;

#[async_trait::async_trait]
impl ModuleHandler for AgentHandler {
    fn module_name(&self) -> &str {
        "agent"
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        match cmd {
            "status" => self.status(ctx),
            "start" => self.start(ctx),
            "stop" => self.stop(ctx),
            "cancel" => self.cancel(data, ctx),
            "rewind" => self.rewind(data, ctx).await,
            "checkpoints" => self.checkpoints(ctx).await,
            _ => Err(format!("unknown command: agent.{}", cmd)),
        }
    }
}

impl AgentHandler {
    fn status(&self, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        let running = ctx
            .state
            .agent_service
            .as_ref()
            .map(|s| nemesis_services::bot_service::LifecycleService::is_running(s.as_ref()))
            .unwrap_or(false);
        let model_name = ctx.state.model_name.lock().clone();
        let model_base = ctx.state.model_base.lock().clone();
        let model_has_key = ctx.state.model_has_key.load(std::sync::atomic::Ordering::SeqCst);
        let session_count = ctx.state.session_count.load(std::sync::atomic::Ordering::SeqCst);

        Ok(Some(serde_json::json!({
            "running": running,
            "model_name": model_name,
            "model_base": model_base,
            "model_has_key": model_has_key,
            "active_sessions": session_count,
        })))
    }

    fn start(&self, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        match ctx.state.agent_service {
            Some(ref svc) => {
                svc.start()?; // Factory rebuilds AgentLoop from disk config
                tracing::info!("[Agent] Started with fresh config");
                update_model_info(ctx);
                Ok(Some(serde_json::json!({ "started": true })))
            }
            None => Err("Agent not available".to_string()),
        }
    }

    fn stop(&self, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        match ctx.state.agent_service {
            Some(ref svc) => {
                svc.stop()?; // Drops the old AgentLoop entirely
                tracing::info!("[Agent] Stopped (AgentLoop destroyed)");
                Ok(Some(serde_json::json!({ "stopped": true })))
            }
            None => Err("Agent not available".to_string()),
        }
    }

    fn cancel(&self, _data: Option<serde_json::Value>, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        let agent_loop = ctx.state.agent_loop.read().clone();
        match agent_loop {
            Some(al) => {
                let cancelled = al.cancel_all_sessions();
                tracing::info!("[Agent] Cancel request: {} session(s) cancelled", cancelled);
                Ok(Some(serde_json::json!({ "cancelled": cancelled })))
            }
            None => Err("Agent not running".to_string()),
        }
    }

    /// `agent.rewind {turn}` — restore the workspace to the start of the given
    /// turn (the edit safety net). Returns the paths written back and deleted.
    async fn rewind(
        &self,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let data = data.ok_or("missing data")?;
        let turn = data
            .get("turn")
            .and_then(|v| v.as_u64())
            .ok_or("turn is required")? as usize;
        let agent_loop = ctx.state.agent_loop.read().clone();
        match agent_loop {
            Some(al) => match al.rewind(turn).await {
                Ok((written, deleted)) => Ok(Some(serde_json::json!({
                    "turn": turn,
                    "written": written,
                    "deleted": deleted,
                }))),
                Err(e) => Err(e),
            },
            None => Err("Agent not running".to_string()),
        }
    }

    /// `agent.checkpoints` — list checkpoint turns (for a rewind picker UI).
    async fn checkpoints(&self, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        let agent_loop = ctx.state.agent_loop.read().clone();
        let list: Vec<serde_json::Value> = match agent_loop {
            Some(al) => al
                .checkpoint_list()
                .into_iter()
                .map(|c| {
                    serde_json::json!({
                        "turn": c.turn,
                        "time": c.time,
                        "prompt": c.prompt,
                        "paths": c.paths,
                    })
                })
                .collect(),
            None => Vec::new(),
        };
        Ok(Some(serde_json::json!({ "checkpoints": list })))
    }
}

/// Re-read model info from config and update AppState tracking fields.
/// Called after start() so the UI reflects the current model.
fn update_model_info(ctx: &RequestContext) {
    let home = match require_home(ctx) {
        Ok(h) => h,
        Err(_) => return,
    };
    let config_path = std::path::Path::new(home).join("config.json");
    if let Ok(cfg) = nemesis_config::load_config(&config_path) {
        let llm_ref = nemesis_config::get_effective_llm(Some(&cfg));
        if let Ok(resolution) = nemesis_config::resolve_model_config(&cfg, &llm_ref) {
            *ctx.state.model_name.lock() = resolution.model_name;
            *ctx.state.model_base.lock() = resolution.api_base;
            ctx.state.model_has_key.store(
                !resolution.api_key.is_empty(),
                std::sync::atomic::Ordering::Release,
            );
        }
    }
}

#[cfg(test)]
mod agent_handler_tests {
    use super::*;
    use crate::api_handlers::AppState;
    use crate::events::EventHub;
    use crate::session::SessionManager;
    use crate::ws_router::RequestContext;
    use nemesis_services::bot_service::{AgentLoopService, LifecycleService};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
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
            Self { running: AtomicBool::new(running), start_fails: false }
        }
        fn new_start_fails() -> Self {
            Self { running: AtomicBool::new(false), start_fails: true }
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
        let r = handler.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
        assert_eq!(r["running"], true);
        assert_eq!(r["model_name"], "test-model");
    }

    #[tokio::test]
    async fn status_stopped_with_service() {
        let handler = AgentHandler;
        let ctx = make_ctx_with_agent(Arc::new(MockAgentService::new(false)));
        let r = handler.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
        assert_eq!(r["running"], false);
    }

    #[tokio::test]
    async fn start_success_sets_started() {
        let handler = AgentHandler;
        let ctx = make_ctx_with_agent(Arc::new(MockAgentService::new(false)));
        let r = handler.handle_cmd("start", None, &ctx).await.unwrap().unwrap();
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
        let r = handler.handle_cmd("stop", None, &ctx).await.unwrap().unwrap();
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
        let r = handler.handle_cmd("checkpoints", None, &ctx).await.unwrap().unwrap();
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
            al.set_checkpoint_store(Arc::new(CheckpointStore::new(None, std::path::PathBuf::from(ws))));
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
        let r = handler.handle_cmd("cancel", None, &ctx).await.unwrap().unwrap();
        assert_eq!(r["cancelled"], 0);
    }

    #[tokio::test]
    async fn checkpoints_with_loop_returns_list() {
        let handler = AgentHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx_with_loop(Some(dir.path().to_string_lossy().to_string()));
        let r = handler.handle_cmd("checkpoints", None, &ctx).await.unwrap().unwrap();
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
}

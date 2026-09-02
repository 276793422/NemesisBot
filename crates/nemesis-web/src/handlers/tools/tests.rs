//! Tools handler 补测（Phase 3 覆盖率，2026-08-25）。
//!
//! `tools.list` 的「agent not running」bail 臂已有覆盖；缺的是真 AgentLoop
//! 下的 map 体（name/description/parameters 三字段组装）。用一个假 Tool
//! 注册进 loop，钉 WSAPI 契约：条目字段名 + count。

use super::ToolsHandler;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_agent::r#loop::{AgentLoop, LlmMessage, LlmProvider, LlmResponse, Tool};
use nemesis_agent::types::AgentConfig;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

struct LoopProvider;
#[async_trait::async_trait]
impl LlmProvider for LoopProvider {
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

struct FakeTool;
#[async_trait::async_trait]
impl Tool for FakeTool {
    async fn execute(
        &self,
        _args: &str,
        _context: &nemesis_agent::context::RequestContext,
    ) -> Result<String, String> {
        Ok("ok".to_string())
    }
    fn description(&self) -> String {
        "fake tool for coverage".to_string()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": { "path": { "type": "string" } }
        })
    }
}

fn make_ctx(al: AgentLoop, dir: &tempfile::TempDir) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.clone()),
        home: Some(ws.clone()),
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
    });
    RequestContext {
        session_id: "test-session".to_string(),
        chat_id: "test-chat".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

#[tokio::test]
async fn list_reports_registered_tools_with_schema() {
    let dir = tempfile::tempdir().unwrap();
    let mut al = AgentLoop::new(Box::new(LoopProvider), AgentConfig::default());
    al.register_tool("fake".to_string(), Box::new(FakeTool));
    let ctx = make_ctx(al, &dir);

    let h = ToolsHandler;
    let out = h.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
    assert_eq!(out["count"], 1);
    let tools = out["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], "fake");
    assert_eq!(tools[0]["description"], "fake tool for coverage");
    assert_eq!(tools[0]["parameters"]["type"], "object");
    assert_eq!(
        tools[0]["parameters"]["properties"]["path"]["type"],
        "string"
    );
}

#[tokio::test]
async fn list_bails_when_agent_loop_missing() {
    let dir = tempfile::tempdir().unwrap();
    let al = AgentLoop::new(Box::new(LoopProvider), AgentConfig::default());
    let ctx = make_ctx(al, &dir);
    // 清空 loop → bail 臂。
    *ctx.state.agent_loop.write() = None;

    let h = ToolsHandler;
    let err = h.handle_cmd("list", None, &ctx).await.unwrap_err();
    assert_eq!(err, "agent not running");
}

//! Agent handler — status/start/stop.

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
        _data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        match cmd {
            "status" => self.status(ctx),
            "start" => self.start(),
            "stop" => self.stop(),
            _ => Err(format!("unknown command: agent.{}", cmd)),
        }
    }
}

impl AgentHandler {
    fn status(&self, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        let running = ctx.state.running.load(std::sync::atomic::Ordering::SeqCst);
        let model_name = ctx.state.model_name.lock().clone();
        let session_count = ctx.state.session_count.load(std::sync::atomic::Ordering::SeqCst);

        Ok(Some(serde_json::json!({
            "running": running,
            "model_name": model_name,
            "active_sessions": session_count,
        })))
    }

    fn start(&self) -> Result<Option<serde_json::Value>, String> {
        // Stub — requires AgentLoop integration
        Ok(Some(serde_json::json!({
            "started": false,
            "message": "Agent start requires runtime integration"
        })))
    }

    fn stop(&self) -> Result<Option<serde_json::Value>, String> {
        // Stub — requires AgentLoop integration
        Ok(Some(serde_json::json!({
            "stopped": false,
            "message": "Agent stop requires runtime integration"
        })))
    }
}

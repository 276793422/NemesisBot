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
mod tests;

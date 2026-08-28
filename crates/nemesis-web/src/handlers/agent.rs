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
            "inbox_status" => Self::inbox_status(data, ctx),
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
        let model_has_key = ctx
            .state
            .model_has_key
            .load(std::sync::atomic::Ordering::SeqCst);
        let session_count = ctx
            .state
            .session_count
            .load(std::sync::atomic::Ordering::SeqCst);

        Ok(Some(serde_json::json!({
            "running": running,
            "model_name": model_name,
            "model_base": model_base,
            "model_has_key": model_has_key,
            "active_sessions": session_count,
        })))
    }

    /// U7 dashboard visibility (G1): per-session inbox snapshot for the chat
    /// UI's queued/steer chip. `data.session_id` is the raw conversation id
    /// the client sends with chat messages — the SAME session_key build rule
    /// as the web inbound chokepoint (server.rs process_messages: empty →
    /// "legacy", else sanitized) so the queried key always matches the key
    /// the agent actually queues under.
    fn inbox_status(
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let sid = data
            .as_ref()
            .and_then(|d| d.get("session_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let session_key = if sid.is_empty() {
            "agent:main:session:legacy".to_string()
        } else {
            format!(
                "agent:main:session:{}",
                nemesis_agent::session::SessionStore::sanitize_session_id(&sid)
            )
        };
        let loop_guard = ctx.state.agent_loop.read();
        match loop_guard.as_ref() {
            Some(al) => {
                let s = al.inbox_status(&session_key);
                Ok(Some(serde_json::json!({
                    "available": true,
                    "session_key": session_key,
                    "next_turn": s.next_turn,
                    "next_step": s.next_step,
                    "capacity": s.capacity,
                    "busy": s.busy,
                    "mode": s.mode,
                })))
            }
            None => Ok(Some(serde_json::json!({
                "available": false,
                "session_key": session_key,
                "next_turn": 0,
                "next_step": 0,
                "capacity": 0,
                "busy": false,
                "mode": "reject",
            }))),
        }
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

    fn cancel(
        &self,
        _data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
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

#[cfg(all(test, feature = "workflow"))]
mod tests;

// S10b (2026-08-26, quality-hardening goal 冲刺 web 批次 2): real-AgentLoop
// arms (cancel/checkpoints/rewind Some-loop + turn guards).
#[cfg(test)]
mod s10b_tests;

//! System handler — version and status commands.

use crate::ws_router::{ModuleHandler, RequestContext};

pub struct SystemHandler;

#[async_trait::async_trait]
impl ModuleHandler for SystemHandler {
    fn module_name(&self) -> &str {
        "system"
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        _data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        match cmd {
            "version" => self.version(ctx),
            "status" => self.status(ctx),
            _ => Err(format!("unknown command: system.{}", cmd)),
        }
    }
}

impl SystemHandler {
    fn version(&self, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        let uptime = ctx.state.start_time.elapsed().as_secs();
        Ok(Some(serde_json::json!({
            "version": ctx.state.version,
            "uptime_seconds": uptime,
        })))
    }

    fn status(&self, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        let uptime = ctx.state.start_time.elapsed().as_secs();
        let session_count = ctx.state.session_count.load(std::sync::atomic::Ordering::SeqCst);
        let running = ctx.state.running.load(std::sync::atomic::Ordering::SeqCst);
        let model_name = ctx.state.model_name.lock().clone();

        let mut status = serde_json::json!({
            "version": ctx.state.version,
            "uptime_seconds": uptime,
            "running": running,
            "session_count": session_count,
            "model_name": model_name,
        });

        if let Some(ref ws) = ctx.workspace {
            status.as_object_mut().unwrap().insert(
                "workspace".to_string(),
                serde_json::json!(ws),
            );
        }

        Ok(Some(status))
    }
}

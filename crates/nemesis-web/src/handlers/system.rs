//! System handler — version and status commands.

use crate::ws_router::{ModuleHandler, RequestContext};

pub struct SystemHandler;

#[async_trait::async_trait]
impl ModuleHandler for SystemHandler {
    fn module_name(&self) -> &str {
        "system"
    }

    fn commands(&self) -> &'static [&'static str] {
        &["version", "status", "commands"]
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
            // L1（devtool-upgrade 阶段 6）：全量 WSAPI 命令注册表——
            // register_all 发布的 OnceLock 快照（module → 静态清单）。
            "commands" => {
                let modules: Vec<serde_json::Value> = crate::handlers::commands_registry()
                    .iter()
                    .map(|(module, cmds)| serde_json::json!({ "module": module, "commands": cmds }))
                    .collect();
                let total_cmds: usize = crate::handlers::commands_registry()
                    .iter()
                    .map(|(_, cmds)| cmds.len())
                    .sum();
                Ok(Some(serde_json::json!({
                    "modules": modules,
                    "total_modules": modules.len(),
                    "total_cmds": total_cmds,
                })))
            }
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
        let session_count = ctx
            .state
            .session_count
            .load(std::sync::atomic::Ordering::SeqCst);
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
            status
                .as_object_mut()
                .unwrap()
                .insert("workspace".to_string(), serde_json::json!(ws));
        }

        Ok(Some(status))
    }
}

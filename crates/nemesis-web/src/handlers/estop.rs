//! E-stop（急停 / Kill Switch）handler — `estop.trigger` / `estop.release` /
//! `estop.status`。
//!
//! 读 `ctx.state.estop`（gateway 启动时经 `web_server.set_estop` 注入的同一个
//! `Arc<EstopState>`，agent loop 也读它）。WSAPI 是 Dashboard 按钮的入口；
//! CLI 走 `/api/internal` 是另一条入口，两者操作同一个 EstopState。

use crate::ws_router::{ModuleHandler, RequestContext};

pub struct EstopHandler;

#[async_trait::async_trait]
impl ModuleHandler for EstopHandler {
    fn module_name(&self) -> &str {
        "estop"
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        _data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        match cmd {
            "trigger" => {
                let estop = ctx
                    .state
                    .estop
                    .as_ref()
                    .ok_or("e-stop not available")?;
                estop.trigger();
                tracing::info!("[Web/WSAPI] E-stop engaged");
                Ok(Some(serde_json::json!({ "engaged": true })))
            }
            "release" => {
                let estop = ctx
                    .state
                    .estop
                    .as_ref()
                    .ok_or("e-stop not available")?;
                estop.release();
                tracing::info!("[Web/WSAPI] E-stop released");
                Ok(Some(serde_json::json!({ "engaged": false })))
            }
            "status" => {
                let engaged = ctx
                    .state
                    .estop
                    .as_ref()
                    .map(|e| e.is_engaged())
                    .unwrap_or(false);
                Ok(Some(serde_json::json!({ "engaged": engaged })))
            }
            _ => Err(format!("unknown command: estop.{}", cmd)),
        }
    }
}

#[cfg(test)]
mod tests;

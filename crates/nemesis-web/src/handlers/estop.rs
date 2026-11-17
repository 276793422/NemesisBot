//! E-stop（急停 / Kill Switch）handler — `estop.trigger` / `estop.release` /
//! `estop.status`。
//!
//! 读 `ctx.state.estop`（gateway 启动时经 `web_server.set_estop` 注入的同一个
//! `Arc<EstopState>`，agent loop 也读它）。WSAPI 是 Dashboard 按钮的入口；
//! CLI 走 `/api/internal` 是另一条入口，两者操作同一个 EstopState。

use crate::ws_router::{ModuleHandler, RequestContext};
use std::sync::Arc;

/// 急停审计钩子（BUG-20261118-096）：gateway 注入（set_estop_audit），持
/// SecurityPlugin 真身走 `append_direct_audit_event` 落 Merkle 审计链。
/// 参数 (operation, source)：operation ∈ {"engage","release"}；source ∈
/// {"cli","dashboard","tray"}。None = 审计链未启用/未注入 → 诚实降级
/// （tracing 兜底，急停动作本身不受影响——审计是记录面不是执行闸）。
pub type EstopAuditHook = Arc<dyn Fn(&str, &str) + Send + Sync>;

pub struct EstopHandler;

#[async_trait::async_trait]
impl ModuleHandler for EstopHandler {
    fn module_name(&self) -> &str {
        "estop"
    }

    fn commands(&self) -> &'static [&'static str] {
        &["trigger", "release", "status"]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        _data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        match cmd {
            "trigger" => {
                let estop = ctx.state.estop.as_ref().ok_or("e-stop not available")?;
                estop.trigger();
                if let Some(hook) = ctx.state.estop_audit.as_ref() {
                    hook("engage", "dashboard");
                }
                tracing::info!("[Web/WSAPI] E-stop engaged");
                Ok(Some(serde_json::json!({ "engaged": true })))
            }
            "release" => {
                let estop = ctx.state.estop.as_ref().ok_or("e-stop not available")?;
                estop.release();
                if let Some(hook) = ctx.state.estop_audit.as_ref() {
                    hook("release", "dashboard");
                }
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

#[cfg(all(test, feature = "workflow"))]
mod tests;

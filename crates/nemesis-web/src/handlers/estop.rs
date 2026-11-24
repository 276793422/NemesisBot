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

/// 用户行为学习采集（2026-11-24 批 C ⑤）：estop 触发/释放落
/// signals.jsonl 交互强信号行（`{"ts_ms","kind":"estop","action"}`）。
/// **只加观测行，急停语义零改动**；estop 是比审批更强的负向交互信号，
/// wsapi/cli/tray 三入口各落一行（复查 2026-11-24：托盘入口此前漏落，
/// 由 gateway runtime 托盘闭包同款调用本函数补齐）。forge feature 关闭 /
/// workspace 未配置 = 不落（旁路面诚实降级）。失败静默（append_signal
/// 侧同纪律）。pub = nemesisbot 侧托盘闭包复用同一实现（三入口单点）。
#[cfg(feature = "forge")]
pub fn record_estop_signal(workspace: Option<&str>, action: &str) {
    let Some(ws) = workspace else {
        return;
    };
    nemesis_forge::userprofile::collection::append_signal(
        std::path::Path::new(ws),
        &serde_json::json!({
            "ts_ms": chrono::Utc::now().timestamp_millis(),
            "kind": "estop",
            "action": action,
        }),
    );
}

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
                #[cfg(feature = "forge")]
                record_estop_signal(ctx.state.workspace.as_deref(), "engage");
                tracing::info!("[Web/WSAPI] E-stop engaged");
                Ok(Some(serde_json::json!({ "engaged": true })))
            }
            "release" => {
                let estop = ctx.state.estop.as_ref().ok_or("e-stop not available")?;
                estop.release();
                if let Some(hook) = ctx.state.estop_audit.as_ref() {
                    hook("release", "dashboard");
                }
                #[cfg(feature = "forge")]
                record_estop_signal(ctx.state.workspace.as_deref(), "release");
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

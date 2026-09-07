//! M7（devtool-upgrade 阶段 5）— `approval.respond` / `approval.pending`。
//!
//! Dashboard 审批卡的 WSAPI 入口：把用户裁决送回等待中的
//! `WebApprovalManager::request_approval_sync`（auditor `require_approval`
//! 阻塞点）。Responder 经 `ctx.state.agent_loop` 的 approval_responder 槽触达
//! （gateway 装配时 `set_approval_responder` 注入）——不经 nemesis-security
//! 依赖，未装配（headless / 旧装配）时诚实报「未装配」。

use crate::ws_router::{ModuleHandler, RequestContext};

pub struct ApprovalHandler;

#[async_trait::async_trait]
impl ModuleHandler for ApprovalHandler {
    fn module_name(&self) -> &str {
        "approval"
    }

    fn commands(&self) -> &'static [&'static str] {
        &["respond", "pending"]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        // 槽读取窗口尽量窄：clone 出 responder Arc 后立即放锁，respond 是
        // 无 await 的同步调用（mpsc send），不持 parking_lot 锁跨 await。
        let responder = {
            let guard = ctx.state.agent_loop.read();
            match guard.as_ref() {
                None => return Err("agent loop not running".to_string()),
                Some(loop_ref) => loop_ref.approval_responder(),
            }
        };
        let responder = responder.ok_or_else(|| {
            "approval manager not wired (no interactive approval on this instance)".to_string()
        })?;
        match cmd {
            "respond" => {
                let data = data.ok_or("missing data")?;
                let request_id = data
                    .get("request_id")
                    .and_then(|v| v.as_str())
                    .ok_or("missing request_id")?;
                let approved = data
                    .get("approved")
                    .and_then(|v| v.as_bool())
                    .ok_or("missing approved (expected bool)")?;
                // F3:「总是允许」——缺键按 false（旧前端兼容）。
                let always = data
                    .get("always")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                // F6: 拒绝备注——缺键/非串按 None（approved=true 时被 manager 忽略）。
                let note = data.get("note").and_then(|v| v.as_str()).map(String::from);
                let result = responder.respond(request_id, approved, always, note)?;
                tracing::info!(
                    "[Web/WSAPI] approval.respond id={} approved={} always={} -> delivered={}",
                    request_id,
                    approved,
                    always,
                    result
                );
                Ok(Some(serde_json::json!({
                    "request_id": request_id,
                    "approved": result,
                    "delivered": true,
                })))
            }
            "pending" => Ok(Some(serde_json::json!({
                "pending": responder.pending(),
            }))),
            _ => Err(format!("unknown command: approval.{}", cmd)),
        }
    }
}

#[cfg(test)]
mod tests;

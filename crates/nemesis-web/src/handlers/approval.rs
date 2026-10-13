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
        &[
            "respond",
            "pending",
            #[cfg(feature = "cluster")]
            "remote_list",
            #[cfg(feature = "cluster")]
            "remote_respond",
        ]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        // 槽读取窗口尽量窄：clone 出 responder Arc 后立即放锁，respond 是
        // 无 await 的同步调用（mpsc send），不持 parking_lot 锁跨 await。
        // L6++ G4（2026-09-08）：可选 session_id——项目会话先路由项目 loop；
        // G6（2026-09-08）修正：审批卡 responder 是全局 Dashboard 单例
        // （WebApprovalManager 挂共享 plugin 的 auditor，per-loop 槽不承载
        // 会话状态），项目 loop 的 responder 槽从未装配 → 槽空时回退主槽，
        // 否则项目会话审批永远 "not wired"。
        // M5：槽解析惰性化（闭包）——remote_list/remote_respond 臂不消费
        // 本节点 responder 槽（裁决走 RPC 下发 worker，本节点无 loop 也该
        // 能列出/批复远程卡）。
        let resolve_responder = || {
            let resolved_loop = if let Some(sid) = data
                .as_ref()
                .and_then(|d| d.get("session_id"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                let session_key = format!(
                    "agent:main:session:{}",
                    nemesis_agent::session::SessionStore::sanitize_session_id(sid)
                );
                Some(crate::handlers::projects::resolve_session_loop(
                    ctx,
                    &session_key,
                )?)
            } else {
                None
            };
            let responder = match resolved_loop {
                // 项目路由命中：槽空（项目 loop 无独立审批卡）→ 回退主槽；
                // 主槽也空 = 全实例无审批卡（not wired 诚实报错）。
                Some(loop_ref) => loop_ref.approval_responder().or_else(|| {
                    let guard = ctx.state.agent_loop.read();
                    guard.as_ref().and_then(|l| l.approval_responder())
                }),
                // 无 session_id：保持原语义——loop 未装配 = not running。
                None => {
                    let guard = ctx.state.agent_loop.read();
                    match guard.as_ref() {
                        None => return Err("agent loop not running".to_string()),
                        Some(loop_ref) => loop_ref.approval_responder(),
                    }
                }
            };
            responder.ok_or_else(|| {
                "approval manager not wired (no interactive approval on this instance)".to_string()
            })
        };
        match cmd {
            "respond" => {
                let responder = resolve_responder()?;
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
            "pending" => {
                let responder = resolve_responder()?;
                Ok(Some(serde_json::json!({
                    "pending": responder.pending(),
                })))
            }
            // M5（2026-10-10 集群无人值守硬化）：远程审批两臂（cluster 形态
            // only）。裁决真相源仍留 worker 本节点 pending（300s 超时不变）；
            // master 侧只做转发 + 留痕。
            #[cfg(feature = "cluster")]
            "remote_list" => Ok(Some(serde_json::json!({
                "pending": crate::remote_approval::registry().snapshot(),
            }))),
            #[cfg(feature = "cluster")]
            "remote_respond" => {
                let data = data.ok_or("missing data")?;
                let id = data
                    .get("request_id")
                    .or_else(|| data.get("short_id"))
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .ok_or("missing request_id (or short_id)")?;
                let approved = data
                    .get("approved")
                    .and_then(|v| v.as_bool())
                    .ok_or("missing approved (expected bool)")?;
                let note = data.get("note").and_then(|v| v.as_str()).map(String::from);
                let cluster = ctx
                    .state
                    .cluster
                    .clone()
                    .ok_or("cluster not available on this instance")?;
                // 离线预检（友好报错；真值仍以 RPC 结果为准）。
                let (short_id, entry) = crate::remote_approval::registry()
                    .resolve_with_key(id)
                    .ok_or_else(|| {
                        format!("unknown remote approval: {id}（可能已超时或已处理）")
                    })?;
                let worker = entry.worker_node.clone();
                if let Some(node) = cluster.get_node_info(&worker)
                    && !node.is_online()
                {
                    return Err(format!("worker 节点 {worker} 当前离线，无法转发批复"));
                }
                crate::remote_approval::send_verdict(
                    &cluster,
                    &worker,
                    &entry.request_id,
                    approved,
                    note.as_deref(),
                )
                .await?;
                crate::remote_approval::registry().remove(&short_id);
                crate::remote_approval::publish_resolved(serde_json::json!({
                    "request_id": entry.request_id,
                    "short_id": short_id,
                    "decision": if approved { "approved" } else { "denied" },
                    "source": "dashboard",
                }));
                tracing::info!(
                    "[Web/WSAPI] approval.remote_respond id={} worker={} approved={}",
                    entry.request_id,
                    worker,
                    approved
                );
                Ok(Some(serde_json::json!({
                    "request_id": entry.request_id,
                    "worker": worker,
                    "approved": approved,
                    "delivered": true,
                })))
            }
            _ => Err(format!("unknown command: approval.{}", cmd)),
        }
    }
}

#[cfg(test)]
mod tests;

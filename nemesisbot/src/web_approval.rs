//! M7（devtool-upgrade 阶段 5）：Dashboard 审批管理器。
//!
//! 实现 `nemesis_security::auditor::ApprovalManager`（auditor → manager 方向：
//! auditor 命中 `require_approval` 时同步调用 `request_approval_sync` 阻塞等
//! 用户裁决），内部把请求广播为 `AgentEvent::ApprovalRequested`（web pump 转
//! SSE `approval-requested`），并实现 `nemesis_types::agent::ApprovalResponder`
//! （WSAPI → manager 方向：`approval.respond` / `approval.pending` 把用户裁决
//! 送回等待中的调用）。
//!
//! 与已停用的 `ApprovalPopupAdapter`（desktop 原生弹窗子进程）同构替换：审批
//! 统一走 dashboard 审批卡——plugin-ui WebView 窗口承载的就是 dashboard 前端，
//! 桌面场景天然兼容。等待侧与 PopupAdapter 一样是 sync trait 方法内阻塞：
//! tokio 上下文用 `block_in_place` 让出 worker，同步上下文（CLI/测试）直接
//! recv——两者都不新建 tokio runtime。
//!
//! 恢复方法（如需回到原生弹窗形态）：gateway.rs 审批装配块里换回
//! `ApprovalPopupAdapter::new(process_manager)` 即可（结构体保留未删）。

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Duration;

use nemesis_types::agent::{AgentEvent, ApprovalResponder};

/// 一条等待裁决的审批请求：元数据（`approval.pending` 列表用）+ 裁决回传
/// 通道（`approval.respond` 取出后 send；mpsc 在 async handler 侧 send 无需
/// runtime）。通道载荷是 `ApprovalVerdict`（F6：拒绝可携带用户备注）。
struct PendingEntry {
    operation: String,
    target: String,
    /// F3:「总是允许」对应的规则 pattern（exec 类=B5 归约前缀），随事件
    /// 与 pending 列表下发，前端按钮回显确认串。
    pattern: String,
    risk_level: String,
    reason: String,
    timeout_secs: u64,
    chat_id: String,
    session_key: String,
    created_at: std::time::Instant,
    tx: mpsc::Sender<nemesis_security::auditor::ApprovalVerdict>,
}

pub struct WebApprovalManager {
    agent_event_tx: Option<tokio::sync::broadcast::Sender<AgentEvent>>,
    pending: Mutex<HashMap<String, PendingEntry>>,
    /// F3:「总是允许」规则表路径（`<workspace>/config/approval_rules.json`）。
    /// None = 未配置（respond always 诚实报错，不静默吞）。
    rules_path: Option<std::path::PathBuf>,
    /// 规则文件读-改-写串行化锁（同刻多卡连点不互踩）。
    rules_write: Mutex<()>,
}

impl WebApprovalManager {
    pub fn new(
        agent_event_tx: Option<tokio::sync::broadcast::Sender<AgentEvent>>,
        rules_path: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            agent_event_tx,
            pending: Mutex::new(HashMap::new()),
            rules_path,
            rules_write: Mutex::new(()),
        }
    }

    /// F3:「总是允许」规则落盘。pattern 由 B5 归约生成（单一真相源
    /// `approval_rules::pattern_for`，与 auditor 匹配侧同源）；空 pattern
    /// 诚实报错（如 run_script 无 target 提取，这类操作保持每次人工）。
    fn save_always_rule(&self, operation: &str, target: &str) -> Result<(), String> {
        use nemesis_security::approval_rules as rules;
        let Some(path) = self.rules_path.as_ref() else {
            return Err("approval rules not configured (no workspace)".to_string());
        };
        let pattern = rules::pattern_for(operation, target);
        if pattern.is_empty() {
            return Err(format!(
                "cannot derive approval pattern from {} target",
                operation
            ));
        }
        let _w = self
            .rules_write
            .lock()
            .map_err(|e| format!("rules lock poisoned: {}", e))?;
        let mut stored = rules::load_rules(path);
        rules::upsert_rule(&mut stored, operation, &pattern);
        rules::save_rules(path, &stored)?;
        tracing::info!(
            "[WebApproval] always-allow rule saved: {} {}",
            operation,
            pattern
        );
        Ok(())
    }

    /// 广播审批请求事件（无订阅者/通道关闭都是良性——审批等待不依赖广播）。
    fn broadcast_requested(&self, entry: &PendingEntry, request_id: &str) {
        if let Some(tx) = self.agent_event_tx.as_ref() {
            let event = AgentEvent::ApprovalRequested {
                session_key: entry.session_key.clone(),
                chat_id: entry.chat_id.clone(),
                request_id: request_id.to_string(),
                operation: entry.operation.clone(),
                target: entry.target.clone(),
                risk_level: entry.risk_level.clone(),
                reason: entry.reason.clone(),
                timeout_secs: entry.timeout_secs,
                pattern: entry.pattern.clone(),
            };
            let _ = tx.send(event);
        }
    }

    /// F6: 广播裁决事件（`approval-resolved`）——所有前端窗口（桌面
    /// WebView + 外部浏览器）据此摘除本地审批卡，竞速败方不再挂到倒计时
    /// 结束；`decision` ∈ approved/denied/timeout。
    fn broadcast_resolved(&self, request_id: &str, decision: &str) {
        if let Some(tx) = self.agent_event_tx.as_ref() {
            let _ = tx.send(AgentEvent::ApprovalResolved {
                request_id: request_id.to_string(),
                decision: decision.to_string(),
            });
        }
    }

    fn entry_json(request_id: &str, entry: &PendingEntry) -> serde_json::Value {
        serde_json::json!({
            "request_id": request_id,
            "operation": entry.operation,
            "target": entry.target,
            "risk_level": entry.risk_level,
            "reason": entry.reason,
            "timeout_secs": entry.timeout_secs,
            "pattern": entry.pattern,
            "chat_id": entry.chat_id,
            "session_key": entry.session_key,
            "age_secs": entry.created_at.elapsed().as_secs(),
        })
    }
}

impl nemesis_security::auditor::ApprovalManager for WebApprovalManager {
    fn is_running(&self) -> bool {
        true
    }

    fn request_approval_sync(
        &self,
        request_id: &str,
        operation: &str,
        target: &str,
        risk_level: &str,
        reason: &str,
        timeout_secs: u64,
    ) -> Result<nemesis_security::auditor::ApprovalVerdict, String> {
        let (tx, rx) = mpsc::channel::<nemesis_security::auditor::ApprovalVerdict>();
        let entry = PendingEntry {
            operation: operation.to_string(),
            target: target.to_string(),
            pattern: nemesis_security::approval_rules::pattern_for(operation, target),
            risk_level: risk_level.to_string(),
            reason: reason.to_string(),
            timeout_secs,
            chat_id: String::new(),
            session_key: String::new(),
            created_at: std::time::Instant::now(),
            tx,
        };
        self.broadcast_requested(&entry, request_id);
        self.pending
            .lock()
            .map_err(|e| format!("pending map poisoned: {}", e))?
            .insert(request_id.to_string(), entry);

        // 阻塞等裁决。tokio 上下文（agent loop 工具调度链）用 block_in_place
        // 让出当前 worker——否则多会话并发审批可能占满 worker 池，WSAPI 无法
        // 处理 respond 导致死锁；同步上下文（CLI / 单测）直接 recv。
        let wait = || rx.recv_timeout(Duration::from_secs(timeout_secs));
        let result = if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::block_in_place(wait)
        } else {
            wait()
        };

        // 无论结果如何都清 pending（超时/断开后迟到的 respond 诚实报 unknown）。
        if let Ok(mut map) = self.pending.lock() {
            map.remove(request_id);
        }

        match result {
            Ok(verdict) => {
                tracing::info!(
                    "[WebApproval] request {} {}: {}",
                    request_id,
                    operation,
                    if verdict.approved {
                        "approved"
                    } else {
                        "denied"
                    }
                );
                Ok(verdict)
            }
            Err(_) => {
                tracing::warn!(
                    "[WebApproval] request {} ({}) timed out after {}s — denying",
                    request_id,
                    operation,
                    timeout_secs
                );
                // F6: 超时自动拒绝也是一次裁决——广播出去让所有前端摘卡。
                self.broadcast_resolved(request_id, "timeout");
                Ok(nemesis_security::auditor::ApprovalVerdict::denied())
            }
        }
    }
}

impl ApprovalResponder for WebApprovalManager {
    fn respond(
        &self,
        request_id: &str,
        approved: bool,
        always: bool,
        note: Option<String>,
    ) -> Result<bool, String> {
        let entry = {
            let mut map = self
                .pending
                .lock()
                .map_err(|e| format!("pending map poisoned: {}", e))?;
            map.remove(request_id)
                .ok_or_else(|| format!("unknown approval request: {}", request_id))?
        };
        // 先到先得：remove 已把请求移出 pending，第二个 respond 天然 unknown。
        // F6: 拒绝备注只在 denied 时携带（空串/纯空白视同无备注）。
        let verdict = if approved {
            nemesis_security::auditor::ApprovalVerdict::approved()
        } else {
            nemesis_security::auditor::ApprovalVerdict {
                approved: false,
                note: note.filter(|n| !n.trim().is_empty()),
            }
        };
        entry
            .tx
            .send(verdict)
            .map_err(|_| "approval waiter already gone".to_string())?;
        // F6: 裁决已送达 → 广播 resolved，竞速败方窗口摘卡。在 always 规则
        // 写盘之前发（写失败不影响「卡已处理」事实）。
        self.broadcast_resolved(request_id, if approved { "approved" } else { "denied" });

        // F3: 批准 + 总是允许 → 写规则。裁决已送达不回滚；层级安全门
        // （CRITICAL 仅 process_exec 豁免）不过则忽略 always 并 warn；
        // 规则写失败诚实回 Err（前端 toast 提示，本次批准仍生效）。
        if approved && always {
            if !nemesis_security::approval_rules::rule_permitted_for(
                &entry.operation,
                &entry.risk_level,
            ) {
                tracing::warn!(
                    "[WebApproval] always-allow ignored for {} (risk {}): CRITICAL ops stay manual",
                    entry.operation,
                    entry.risk_level
                );
            } else {
                self.save_always_rule(&entry.operation, &entry.target)?;
            }
        }
        Ok(approved)
    }

    fn pending(&self) -> Vec<serde_json::Value> {
        match self.pending.lock() {
            Ok(map) => map.iter().map(|(id, e)| Self::entry_json(id, e)).collect(),
            Err(_) => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests;

//! P4-d（2026-10-17）— 评审结论 → 交付账本闭环（持久 Agent 实体 P4-d）。
//!
//! PLAN P4「全自动链推广」的账本半边：board 链每条评审决策经
//! [`crate::board_review::record_auto_decide`] 单一漏斗同步落 deliveries
//! 表 `review` 行（证据链一处可查：`deliveries.list` / `deliveries.by_run`）；
//! 子单决策同时驱动分支交付闭环——
//! - `auto_accept` → confirm：交付被接受；
//! - `redispatch` → reject（带评审理由）：该次尝试未被接受（每次派发
//!   task_id 唯一 = 每次尝试独立 branch 行，重派产生新行不互扰）；
//! - 其余（escalate_human / suggest_manual / conflict*）→ 留 pending，
//!   人工经 `deliveries.confirm` / `deliveries.reject` WSAPI 裁决。
//! 父单（`parent_*`）/项目（`project_*`）决策只记证据行，不挂分支动作。
//!
//! 诚实边界：账本与审计全是增值证据面——任何失败（`Cluster::new` 测试
//! 路径无 task_db / SQLite 错误 / 审计链未启用）= warn/debug 兜底，绝不
//! 炸评审主链路。项目档案自动导出（auto_on_complete）同记 `dossier` 行；
//! WSAPI/CLI 手动导出走 nemesis-board 同源核心、不入账（目录路径直接回
//! 给调用方，账本只收自动链证据）。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::board_review::BoardReviewDeps;

/// 事件键进程内单调序列号。毫秒时间戳单独做幂等键在亚毫秒连发下会
/// 碰撞——`INSERT OR IGNORE` 静默吞行=审计记录丢失（合并缝实测）；
/// tasks.db 单写者权威（评审漏斗只在本进程写），序列号拼接即全局唯一。
static EVENT_KEY_SEQ: AtomicU64 = AtomicU64::new(0);

fn next_event_key_seq() -> u64 {
    EVENT_KEY_SEQ.fetch_add(1, Ordering::Relaxed)
}

/// 决策 → 账本 scope。分支闭环动作只对 `issue` scope 生效。
fn scope_of(decision: &str) -> &'static str {
    if decision.starts_with("parent_") {
        "parent"
    } else if decision.starts_with("project_") {
        "project"
    } else {
        "issue"
    }
}

fn unix_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// redispatch 拒绝理由：verdict + 评审差距原文（截 300 字符留痕）。
fn reject_reason(verdict: &str, details: &serde_json::Value) -> String {
    let reasons = details
        .get("reasons")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let mut reason = format!("评审未通过（{verdict}），自动重派");
    if !reasons.is_empty() {
        let head: String = reasons.chars().take(300).collect();
        reason.push('：');
        reason.push_str(&head);
    }
    reason
}

/// 分支闭环审计留痕（cfg security）：与 WSAPI 裁决审计对称——自动链的
/// confirm/reject 同样过 Merkle 审计链。槽缺席/审计链未启用 = tracing
/// debug 兜底可见。
#[cfg(feature = "security")]
fn audit_closure(deps: &BoardReviewDeps, operation: &str, target: &str, reason: &str) {
    let plugin = deps
        .security_slot
        .as_ref()
        .and_then(|slot| slot.get())
        .and_then(|p| p.as_ref());
    let Some(plugin) = plugin else {
        tracing::debug!(
            target = %target,
            "[ReviewLedger] 审计链未启用，分支闭环仅 tracing 可见"
        );
        return;
    };
    if plugin
        .append_direct_audit_event(
            operation,
            "review_ledger",
            "system",
            "auto_review",
            target,
            "allow",
            reason,
        )
        .is_none()
    {
        tracing::debug!(
            target = %target,
            "[ReviewLedger] 审计 writer 缺席，分支闭环仅 tracing 可见"
        );
    }
}

/// 评审结论 → 交付账本（review 行 + issue scope 两臂分支闭环）。任何失败
/// 只 warn，绝不炸评审主链路。`task_db` 由调用方从 `deps.cluster.task_db()`
/// 提取（None = 无账本路径，静默跳过）。
pub(crate) fn review_outcome_delivery(
    deps: &BoardReviewDeps,
    task_db: Option<&Arc<nemesis_taskstore::TaskDb>>,
    issue_id: i64,
    decision: &str,
    verdict: &str,
    details: &serde_json::Value,
) {
    let Some(db) = task_db else {
        return; // Cluster::new 测试路径无账本（与交付分支记账同语义）
    };
    let scope = scope_of(decision);
    let issue_number = deps
        .store
        .get_issue(issue_id)
        .map(|i| i.number.clone())
        .unwrap_or_else(|_| "?".to_string());
    // 最新派发 = 本次决策针对的尝试（run_id + 分支闭环锚；rowid 升序取
    // 末位——同秒多轮重派打平的决胜语义与派发回调路由同源）。
    let task_id = deps
        .store
        .list_dispatches(issue_id)
        .ok()
        .and_then(|ds| ds.last().map(|d| d.task_id.clone()));
    let payload = serde_json::json!({
        "scope": scope,
        "issue_id": issue_id,
        "issue_number": issue_number,
        "decision": decision,
        "verdict": verdict,
        "task_id": task_id,
        "details": details,
    });
    let key = format!(
        "issue-{issue_number}:review:{}:{}",
        unix_millis(),
        next_event_key_seq()
    );
    if let Err(e) = db.record_delivery("review", task_id.as_deref(), &key, &payload, false) {
        tracing::warn!(
            issue_id = issue_id,
            decision = decision,
            "[ReviewLedger] 评审结论入账失败: {e}"
        );
        return;
    }
    // 分支闭环（仅 issue scope 两臂）。无分支交付（JSON checkpoint 形态 /
    // 无影子库）= Ok(false) 静默——没有可闭环的行，不是错误。
    if scope == "issue"
        && let Some(task_id) = task_id.as_deref()
    {
        let branch_key = format!("{task_id}:branch");
        match decision {
            "auto_accept" => match db.confirm_delivery(&branch_key) {
                Ok(true) => {
                    tracing::info!(
                        issue_id = issue_id,
                        task_id = task_id,
                        "[ReviewLedger] auto_accept → 分支交付已确认: {branch_key}"
                    );
                    #[cfg(feature = "security")]
                    audit_closure(
                        deps,
                        "delivery_confirm",
                        &branch_key,
                        "auto_accept review settled the delivery",
                    );
                }
                Ok(false) => {}
                Err(e) => tracing::warn!(
                    issue_id = issue_id,
                    task_id = task_id,
                    "[ReviewLedger] 分支交付确认失败: {e}"
                ),
            },
            "redispatch" => {
                let reason = reject_reason(verdict, details);
                match db.reject_delivery(&branch_key, &reason) {
                    Ok(true) => {
                        tracing::info!(
                            issue_id = issue_id,
                            task_id = task_id,
                            "[ReviewLedger] redispatch → 分支交付已拒绝: {branch_key}"
                        );
                        #[cfg(feature = "security")]
                        audit_closure(deps, "delivery_reject", &branch_key, &reason);
                    }
                    Ok(false) => {}
                    Err(e) => tracing::warn!(
                        issue_id = issue_id,
                        task_id = task_id,
                        "[ReviewLedger] 分支交付拒绝失败: {e}"
                    ),
                }
            }
            _ => {}
        }
    }
}

/// 项目档案自动导出入账（dossier 行；run_id=None——项目级证据包不挂单一
/// 任务）。key 带毫秒：多次导出各自成行不互斥。
pub(crate) fn dossier_delivery(
    task_db: Option<&Arc<nemesis_taskstore::TaskDb>>,
    project_id: i64,
    outcome: &nemesis_board::dossier::DossierOutcome,
) {
    let Some(db) = task_db else {
        return;
    };
    let payload = serde_json::json!({
        "scope": "project",
        "project_id": project_id,
        "root": outcome.root.display().to_string(),
        "issue_numbers": outcome.issue_numbers.clone(),
        "notes": outcome.notes.clone(),
    });
    let key = format!(
        "project-{project_id}:dossier:{}:{}",
        unix_millis(),
        next_event_key_seq()
    );
    if let Err(e) = db.record_delivery("dossier", None, &key, &payload, false) {
        tracing::warn!(
            project_id = project_id,
            "[ReviewLedger] 项目档案入账失败: {e}"
        );
    }
}

#[cfg(test)]
mod tests;

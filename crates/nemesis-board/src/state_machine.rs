//! Issue 状态机（开发计划 §1.1）。
//!
//! 状态集：`backlog / todo / in_progress / in_review / done / blocked / cancelled`。
//! `done` 是终态；`cancelled` 唯一终态出口 = reopen 回 `backlog`（发现④
//! 修复：cancel 级联+重派闸叠加曾导致中断恢复只能 SQL 手术）。非法转移由
//! [`validate_transition`] 拒绝（handler 层转 422/错误返回）。

use crate::models::IssueStatus;

/// 判断 `from → to` 是否为合法转移。
pub fn can_transition(from: IssueStatus, to: IssueStatus) -> bool {
    use IssueStatus::*;
    if from == to {
        return false;
    }
    match (from, to) {
        // backlog：万物的起点，可进任意非终态，也可直接 done/cancelled。
        (Backlog, Todo | InProgress | Done | Blocked | Cancelled) => true,
        // todo：开工 / 完成 / 阻塞 / 取消。
        (Todo, InProgress | Done | Blocked | Cancelled) => true,
        // in_progress：送审 / 完成 / 阻塞 / 取消。
        (InProgress, InReview | Done | Blocked | Cancelled) => true,
        // in_review：返工回 in_progress / 完成 / 阻塞 / 取消。
        (InReview, InProgress | Done | Blocked | Cancelled) => true,
        // blocked：解除阻塞回 todo / 直接开工 / 放弃。
        (Blocked, Todo | InProgress | Cancelled) => true,
        // cancelled 唯一终态出口：reopen 回 backlog（发现④；WSAPI/CLI
        // issue.reopen 专用路径，自动追加审计评论）。done 的重开是另一
        // 语义（重开已完成工作），本轮不做——留口待后续单独评估。
        (Cancelled, Backlog) => true,
        // done 及其余终态转移不可达。
        _ => false,
    }
}

/// 校验转移，非法时返回人读原因（含完整合法目标集，方便前端提示）。
pub fn validate_transition(from: IssueStatus, to: IssueStatus) -> Result<(), String> {
    if from == to {
        return Err(format!("issue 已处于 {from} 状态"));
    }
    if can_transition(from, to) {
        return Ok(());
    }
    Err(format!(
        "非法状态转移 {from} → {to}（{from} 可转移到 {}）",
        from.allowed_targets()
    ))
}

#[cfg(test)]
mod tests;

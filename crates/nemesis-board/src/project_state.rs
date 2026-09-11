//! 项目状态机（全自动流转 P3/F2）。
//!
//! 状态集：`active / in_progress / completed / archived`。主链
//! `active → in_progress → completed`；`archived` 是各活跃态共用的归档
//! 旁路（`active/in_progress → archived`），`archived → active` 为重开。
//! `completed → in_progress` 为 P4 F3 项目级验收 FAIL 自动回退预留。
//! 非法转移由 [`validate_transition`] 拒绝——写入口统一收在
//! [`crate::store::BoardStore::update_project`]。

use crate::models::ProjectStatus;

/// 判断 `from → to` 是否为合法转移。
pub fn can_transition(from: ProjectStatus, to: ProjectStatus) -> bool {
    use ProjectStatus::*;
    if from == to {
        return false;
    }
    match (from, to) {
        // active：开工（首派联动进 in_progress）或直接归档。
        (Active, InProgress | Archived) => true,
        // in_progress：收口完成或归档。
        (InProgress, Completed | Archived) => true,
        // completed：归档，或 F3 验收 FAIL 自动回退重开（P4 接线）。
        (Completed, Archived | InProgress) => true,
        // archived：重开回 active。
        (Archived, Active) => true,
        _ => false,
    }
}

/// 校验转移，非法时返回人读原因（含完整合法目标集，方便前端提示）。
pub fn validate_transition(from: ProjectStatus, to: ProjectStatus) -> Result<(), String> {
    if from == to {
        return Err(format!("project 已处于 {from} 状态"));
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

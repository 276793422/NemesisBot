//! 多态指派 + 操作者类型（开发计划 §1.3）。
//!
//! MVP 两级简化：`assignee_type ∈ {manager_self, worker}`。`manager_self`
//! 指 coordinator 自己执行（assignee_id = coordinator 节点 id）；`worker`
//! 指集群 worker 节点。单节点部署只有 manager_self。

use serde::{Deserialize, Serialize};

/// 指派对象类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssignmentType {
    /// coordinator 自己（hybrid：既协调也执行）。
    ManagerSelf,
    /// 集群 worker 节点（纯执行器）。
    Worker,
}

impl AssignmentType {
    pub fn as_str(&self) -> &'static str {
        match self {
            AssignmentType::ManagerSelf => "manager_self",
            AssignmentType::Worker => "worker",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "manager_self" => Some(AssignmentType::ManagerSelf),
            "worker" => Some(AssignmentType::Worker),
            _ => None,
        }
    }
}

impl std::fmt::Display for AssignmentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 操作者（activity_log / comment / subscriber 的 author）。
///
/// `kind` 约定值：`admin`（人）、`agent`（manager/worker agent 自主行为）、
/// `system`（autopilot / 回流机械写入）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor {
    pub kind: String,
    pub id: String,
}

impl Actor {
    pub fn new(kind: &str, id: &str) -> Self {
        Self {
            kind: kind.to_string(),
            id: id.to_string(),
        }
    }

    pub fn admin(id: &str) -> Self {
        Self::new("admin", id)
    }

    pub fn agent(id: &str) -> Self {
        Self::new("agent", id)
    }

    pub fn system(id: &str) -> Self {
        Self::new("system", id)
    }
}

#[cfg(test)]
mod tests;

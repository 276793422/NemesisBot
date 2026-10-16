//! 审批台账（统一任务层 approvals 表的内存投影与契约）。
//!
//! P1 审批持久化：pending 落库 + 裁决条件更新幂等（先到先得，与
//! WebApprovalManager 的「respond 先移除后裁决」单次生效语义一致）。
//! trait 放 nemesis-types（全仓叶子依赖，无新增依赖边）：web/IM/ACP
//! 三个审批 broker（`#[cfg(feature = "security")]` 门内）引用它不牵连
//! crate 拓扑；`nemesis-taskstore` 的 SQLite 实现方按本契约交付。
//!
//! 语义契约（实现方承诺）：
//! - `record_pending` 幂等（同 id 重复落库不报错不覆盖）；
//! - `record_decision` 只在 status='pending' 的行上生效且**原子**：
//!   并发双裁决恰有一方拿到 `Ok(true)`（`UPDATE ... WHERE status='pending'`）；
//! - `find_pending` 按 full id 或 short_id 命中 pending 行（IM 回执
//!   与重启补裁共用）。
//!
//! 重启补裁语义：原操作进程已死，裁决为**记账性质**（原任务不会继续）；
//! 跨重启续执行归 P3 在途调用幂等域，本契约只承诺「可裁决且只生效一次」。

use serde::{Deserialize, Serialize};

/// 审批台账状态（approvals.status）。
pub mod status {
    pub const PENDING: &str = "pending";
    pub const APPROVED: &str = "approved";
    pub const DENIED: &str = "denied";
    pub const EXPIRED: &str = "expired";
}

/// 一条审批记录（pending 或已裁决）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalJournalEntry {
    /// 全局唯一请求 id（broker 侧 request_id 原样）。
    pub id: String,
    /// 发起 broker：`web` / `channel` / `acp`（统一表内区分来源）。
    pub broker: String,
    /// IM 卡片短编号（web/ACP 无短编号 = None）。
    pub short_id: Option<String>,
    pub operation: String,
    pub target: String,
    pub risk_level: String,
    pub reason: String,
    /// broker 特有上下文（channel/chat_id、session_key 等），自由形态。
    pub payload: serde_json::Value,
    pub status: String,
    pub created_at: String,
    pub decided_at: Option<String>,
    pub decision_note: Option<String>,
    pub decided_by: Option<String>,
}

/// 裁决种类（approval 记账面；web/IM 超时归 Expired，编辑器拒绝归 Denied）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Approved,
    Denied,
    Expired,
}

impl ApprovalDecision {
    pub fn as_str(&self) -> &'static str {
        match self {
            ApprovalDecision::Approved => status::APPROVED,
            ApprovalDecision::Denied => status::DENIED,
            ApprovalDecision::Expired => status::EXPIRED,
        }
    }
}

/// 审批台账契约。实现方：`nemesis-taskstore::SqliteApprovalJournal`
/// （SQLite，生产）；测试用内存实现自备。
pub trait ApprovalJournal: Send + Sync {
    /// pending 落库。幂等：同 id 已存在时不覆盖、不报错。
    fn record_pending(&self, entry: &ApprovalJournalEntry) -> Result<(), String>;

    /// 裁决。条件更新幂等：只在 pending 行上生效，先到先得。
    /// 返回 `Ok(true)` = 本次裁决生效；`Ok(false)` = 已被裁决（竞态败方）。
    fn record_decision(
        &self,
        id: &str,
        decision: ApprovalDecision,
        note: Option<&str>,
        decided_by: &str,
    ) -> Result<bool, String>;

    /// 按 id 取记录（不存在 = Err）。
    fn entry(&self, id: &str) -> Result<ApprovalJournalEntry, String>;

    /// 全部 pending（含重启前遗留），created_at 升序。
    fn pending_entries(&self) -> Vec<ApprovalJournalEntry>;

    /// 按 full id 或 short_id 找 pending（重启补裁入口）。
    fn find_pending(&self, id_or_short: &str) -> Option<ApprovalJournalEntry>;
}

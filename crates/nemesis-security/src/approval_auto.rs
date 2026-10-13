//! §2 免打扰运行时态（2026-10-11 无人值守计划 Part 2）。
//!
//! `security.approvals.auto_approve` 开关的**运行期载体**：同一
//! [`ApprovalAutoApproveState`] Arc 双注入（editor_access 单一真相源同款）：
//! - auditor（`set_approval_auto_approve`）：RequireApproval 臂读 `enabled`
//!   决定是否自动放行，放行即 `bump()` 计数；
//! - web security handler（`install_auto_approve`）：WSAPI
//!   `security.approvals.status` 透传 enabled + 计数、`config.save` 热切。
//!
//! config.json 是**持久真相源**（重启回 config 值，security_setup 构造期
//! 初始化）；本态是运行期投影——count「本次已自动批准 N 条」即进程生命周期
//! 内计数，重启清零（前端文案「本次」语义）。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// 免打扰运行时态：开关 + 本次运行自动批准计数。
pub struct ApprovalAutoApproveState {
    enabled: AtomicBool,
    count: AtomicU64,
}

impl ApprovalAutoApproveState {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled: AtomicBool::new(enabled),
            count: AtomicU64::new(0),
        }
    }

    /// 免打扰是否生效（auditor RequireApproval 臂判定）。
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    /// 热切（config.save 链调用；config 是持久真相源，这里只翻投影）。
    pub fn set_enabled(&self, v: bool) {
        self.enabled.store(v, Ordering::SeqCst);
    }

    /// 自动放行计数 +1（auditor auto_by_config 臂调用）。
    pub fn bump(&self) {
        self.count.fetch_add(1, Ordering::SeqCst);
    }

    /// 本次运行已自动批准条数（前端「免打扰中：本次已自动批准 N 条」）。
    pub fn count(&self) -> u64 {
        self.count.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests;

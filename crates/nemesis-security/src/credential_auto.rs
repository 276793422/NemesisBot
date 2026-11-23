//! 凭据免打扰运行时态 + 使用统计面（2026-11-23 ABCD 批 A/B）。
//!
//! `security.approvals.credential_auto_approve` 开关的**运行期载体**，镜像
//! [`crate::approval_auto::ApprovalAutoApproveState`]（免打扰 Part 2）：
//! 同一 Arc 双注入——
//! - AgentLoop（`set_credential_auto_approve`）：dispatch 凭据闸 v1 卡臂
//!   读 `enabled` 决定是否免卡直放（闸 auto 臂；`|| auditor.approval_auto_enabled()`
//!   ——全局免打扰同罩凭据卡，原始投诉「免打扰开了凭据卡照弹」的根治）；
//! - web vault handler（`install_credential_auto`）：WSAPI
//!   `vault.credential_status` / `credential_set` / `credential_stats` 读写。
//!
//! config.security.json 是**持久真相源**（重启回 config 值，security_setup
//! 构造期初始化）；本态是运行期投影——计数与记录均为进程生命周期内，
//! 重启清零（前端「本次运行」语义；持久回查面走既有审计链 JSONL）。
//!
//! 统计面（批 B）：`CredentialUseRecord` 只含**别名与槽位元数据**
//! （tool/fields/aliases/target_summary ≤120 字符/outcome/source）——凭据
//! 真值零参与（vault 纪律：值不落日志/统计/审计）。闸全路径打点
//! （auto/preauth/approved/denied/cooldown/out_of_domain/channel_error），
//! 计数器只记三主臂（auto/preauth/denied），长尾只进记录环。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// 记录环容量（超过即挤掉最旧——前端统计表按最近优先展示）。
pub const CREDENTIAL_RECORDS_CAP: usize = 200;

/// target_summary 截断上限（args 紧凑 JSON 头部；exec 命令/URL host 等方向感）。
const TARGET_SUMMARY_MAX: usize = 120;

/// 单次凭据引用使用记录（闸全路径打点；纯元数据，真值零参与）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CredentialUseRecord {
    /// 事件时间（Unix 毫秒）。
    pub ts_ms: u64,
    /// 工具名（exec / web_fetch / ...）。
    pub tool: String,
    /// 声明槽位 CSV（`env.PW104, env.PW117` 形态）。
    pub fields: String,
    /// vault 别名 CSV（别名本身可含域绑定信息，如 `vault:browser/<域>/<名>`）。
    pub aliases: String,
    /// 目标摘要：tool args 紧凑 JSON 头部 ≤120 字符（方向感——exec 命令
    /// 头 / URL 头；只作展示，不参与任何判定）。
    pub target_summary: String,
    /// 结果臂：`auto`（开关免卡）/ `preauth`（域绑定预授权）/
    /// `approved`（卡批准）/ `denied`（卡拒绝）/ `cooldown`（冷却静默拒）/
    /// `out_of_domain`（出域硬拒）/ `channel_error`（审批通道故障）。
    pub outcome: &'static str,
    /// 来源：`credential_switch`（本开关）/ `approvals_dnd`（全局免打扰
    /// 兜底）/ `user`（人工裁决）/ `domain`（域绑定）/ `cooldown`（冷却
    /// 静默拒）/ `channel`（审批通道故障）。
    pub source: &'static str,
}

/// 凭据免打扰运行时态：开关 + 三主臂计数 + 记录环。
pub struct CredentialAutoApproveState {
    enabled: AtomicBool,
    auto_count: AtomicU64,
    preauth_count: AtomicU64,
    denied_count: AtomicU64,
    records: parking_lot::Mutex<VecDeque<CredentialUseRecord>>,
}

impl CredentialAutoApproveState {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled: AtomicBool::new(enabled),
            auto_count: AtomicU64::new(0),
            preauth_count: AtomicU64::new(0),
            denied_count: AtomicU64::new(0),
            records: parking_lot::Mutex::new(VecDeque::new()),
        }
    }

    /// 开关是否生效（闸 auto 臂判定之一）。
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    /// 热切（`vault.credential_set` 链调用；config 是持久真相源，这里只翻投影）。
    pub fn set_enabled(&self, v: bool) {
        self.enabled.store(v, Ordering::SeqCst);
    }

    /// auto 臂放行计数 +1。
    pub fn bump_auto(&self) {
        self.auto_count.fetch_add(1, Ordering::SeqCst);
    }

    /// 域绑定预授权计数 +1。
    pub fn bump_preauth(&self) {
        self.preauth_count.fetch_add(1, Ordering::SeqCst);
    }

    /// 拒绝计数 +1（卡拒绝臂调用；冷却/出域不进计数，只进记录环）。
    pub fn bump_denied(&self) {
        self.denied_count.fetch_add(1, Ordering::SeqCst);
    }

    /// 记录一次凭据使用事件（环满挤最旧；锁内短临界区，纯内存）。
    pub fn record(&self, rec: CredentialUseRecord) {
        let mut ring = self.records.lock();
        if ring.len() >= CREDENTIAL_RECORDS_CAP {
            ring.pop_front();
        }
        ring.push_back(rec);
    }

    /// 三主臂计数快照（WSAPI `vault.credential_status`/`credential_stats`）。
    pub fn counts(&self) -> (u64, u64, u64) {
        (
            self.auto_count.load(Ordering::SeqCst),
            self.preauth_count.load(Ordering::SeqCst),
            self.denied_count.load(Ordering::SeqCst),
        )
    }

    /// 记录环快照（最新在后；克隆出锁，前端统计表数据源）。
    pub fn records_snapshot(&self) -> Vec<CredentialUseRecord> {
        self.records.lock().iter().cloned().collect()
    }
}

/// args 紧凑 JSON 头部截断（target_summary 素材；字符级截断避免切开
/// UTF-8 多字节序列——中文命令头乱码面）。
pub fn truncate_target_summary(args_raw: &str) -> String {
    let mut out: String = args_raw.chars().take(TARGET_SUMMARY_MAX).collect();
    if args_raw.chars().count() > TARGET_SUMMARY_MAX {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests;

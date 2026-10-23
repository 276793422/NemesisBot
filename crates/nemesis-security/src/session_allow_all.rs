//! 会话级全部放行（2026-10-24 长任务无人值守计划）。
//!
//! 用户裁决（原话语义，见 `docs/REPORT/2026-10-24_会话级全部放行与循环干预阶梯.md`
//! §0）：长任务无人值守场景下，进程级免打扰（§2 `config.approvals.auto_approve`）
//! 隔离粒度不够——多会话并行时，A 会话的任务内容无权放行 B 会话的操作。
//! 本模块是**会话级**开关的运行时载体：开启后该会话的**所有**安全审批卡
//! 自动放行（含 CRITICAL、含 ziwei `run_policy=confirm` 的 force 卡——裁决
//! 语义）；**不在射程**：硬 deny（根本到不了 RequireApproval）、estop 冻结
//! （上游 dispatch 已拦）、无审批通道 fail-closed。
//!
//! **无 TTL、无自动关闭**（用户裁决：自动关闭=用户某天忘了还得回来找产品
//! 麻烦）——纯手动开关；持久真相源 = 会话 sidecar
//! `{safe_key}.meta.json` 的 `allow_all` 字段（写路径在 nemesis-agent
//! `chat_log`，WSAPI handler 双写），本态是运行期投影，gateway 启动 hydrate。
//!
//! 消费方与注入模式复刻 [`crate::approval_auto::ApprovalAutoApproveState`]：
//! 同一实例 Arc 双注入——auditor（`set_session_allow_all`，RequireApproval 臂
//! 读）+ web security handler（WSAPI 读写）——单一真相源。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::RwLock;

/// 单会话放行条目：开关 + 开启时刻 + 本次放行期已自动批准计数。
pub struct AllowAllEntry {
    enabled: bool,
    /// 开启时刻（RFC3339 本地时区，前端「自 HH:MM 放行中」展示）。
    since: String,
    /// 本放行期已自动批准条数（关闭后保留供回看，重开清零——「本次」语义
    /// 与 [`crate::approval_auto::ApprovalAutoApproveState`] 的 count 对齐）。
    count: AtomicU64,
}

impl AllowAllEntry {
    fn new(since: String) -> Self {
        Self {
            enabled: true,
            since,
            count: AtomicU64::new(0),
        }
    }
}

/// 会话级全部放行运行时状态：`session_key → AllowAllEntry`。
pub struct SessionAllowAllState {
    map: RwLock<HashMap<String, AllowAllEntry>>,
}

impl SessionAllowAllState {
    pub fn new() -> Self {
        Self {
            map: RwLock::new(HashMap::new()),
        }
    }

    /// 该会话是否处于全部放行中（auditor RequireApproval 臂判定；热路径
    /// 纯内存）。
    pub fn check(&self, session_key: &str) -> bool {
        self.map.read().get(session_key).is_some_and(|e| e.enabled)
    }

    /// 翻转会话开关（WSAPI set；**只翻内存态**——sidecar 持久化由调用方
    /// 双写，见模块注释的真相源分工）。
    ///
    /// 开 = 插入/更新条目（`since`=现在 + 计数清零）；关 = `enabled=false`
    ///（条目保留——`since`/count 供状态回看，重开时被覆盖）。
    /// 返回 `(since, count)` 供 WSAPI 响应（关 = `("", 保留的 count)`）。
    pub fn set(&self, session_key: &str, enabled: bool) -> (String, u64) {
        let mut map = self.map.write();
        if enabled {
            let entry = AllowAllEntry::new(chrono::Local::now().to_rfc3339());
            let since = entry.since.clone();
            map.insert(session_key.to_string(), entry);
            tracing::info!(
                "[SessionAllowAll] 会话全部放行开启: {session_key}（含 CRITICAL 与 force 卡）"
            );
            (since, 0)
        } else {
            match map.get_mut(session_key) {
                Some(e) => {
                    e.enabled = false;
                    let count = e.count.load(Ordering::SeqCst);
                    tracing::info!(
                        "[SessionAllowAll] 会话全部放行关闭: {session_key}（本期共自动批准 {count} 条）"
                    );
                    (String::new(), count)
                }
                None => (String::new(), 0),
            }
        }
    }

    /// 放行计数 +1（auditor 臂放行时调用；未开启/未知会话 no-op）。
    pub fn bump(&self, session_key: &str) {
        if let Some(e) = self.map.read().get(session_key) {
            e.count.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// 状态快照 `(enabled, since, count)`；无条目 = `None`。
    pub fn status(&self, session_key: &str) -> Option<(bool, String, u64)> {
        self.map
            .read()
            .get(session_key)
            .map(|e| (e.enabled, e.since.clone(), e.count.load(Ordering::SeqCst)))
    }

    /// 启动 hydrate：gateway 启动扫 `logs/session_logs/*.meta.json` 后把
    /// `allow_all=true` 的会话键种进内存态（`since` 取 sidecar 现值；sidecar
    /// 未记 since 的旧形态填空串）。此后 auditor 判定零磁盘 IO。
    pub fn hydrate(&self, entries: Vec<(String, String)>) {
        let mut map = self.map.write();
        for (key, since) in entries {
            map.entry(key).or_insert_with(|| AllowAllEntry::new(since));
        }
        let n = map.values().filter(|e| e.enabled).count();
        if n > 0 {
            tracing::info!("[SessionAllowAll] 启动回载 {n} 个放行中的会话");
        }
    }

    /// 全部启用中的会话键（sessions.list 投影 / 状态盘点用）。
    pub fn enabled_keys(&self) -> Vec<String> {
        self.map
            .read()
            .iter()
            .filter(|(_, e)| e.enabled)
            .map(|(k, _)| k.clone())
            .collect()
    }
}

impl Default for SessionAllowAllState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;

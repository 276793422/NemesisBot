//! 降级写信号注册表（M2，2026-10-27 整改批次一）。
//!
//! 关键持久化写失败（chat_log / checkpoint 影子 tree / turn WAL）不再只进
//! 日志文件：失败源在此登记，gateway 侧水泵轮询 [`take_pending`] 后经
//! EventHub 推 SSE（`system-alert`），`/ready` 健康面同步降级。
//!
//! 放在 nemesis-utils 的原因：生产者是 nemesis-agent（不依赖 nemesis-health，
//! 也不该依赖——health 带 axum），消费者是 gateway；utils 是两侧共同的下沉
//! 依赖。纯内存、进程级；重启即清（与事件环形缓冲同语义——重启后写恢复，
//! 不再有历史包袱）。
//!
//! 去抖：同一 kind 的 [`report`] 在 [`ALERT_DEBOUNCE`] 窗口内只置一次
//! pending（磁盘满时每条 chat 写都会失败，不去抖会刷爆 SSE）。count 与
//! detail 始终更新——`/ready` 快照永远反映最新现场。

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 同一失败源的告警去抖窗口（计划 §1.2：10 分钟）。
const ALERT_DEBOUNCE: Duration = Duration::from_secs(600);

/// 一条降级写记录（`/ready` 快照与 SSE 载荷共用形态）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct DegradedWrite {
    /// 失败源类别（`chat_log` / `checkpoint_tree` / `turn_wal`）。
    pub kind: String,
    /// 最近一次失败的现场（路径 + io error）。
    pub detail: String,
    /// 进程生命周期内累计失败次数。
    pub count: u64,
    /// 首次失败时间（epoch 毫秒）。
    pub first_at_ms: u64,
    /// 最近失败时间（epoch 毫秒）。
    pub last_at_ms: u64,
}

#[derive(Default)]
struct Entry {
    detail: String,
    count: u64,
    first_at_ms: u64,
    last_at_ms: u64,
    last_alert: Option<Instant>,
    pending: bool,
}

// parking_lot：无中毒语义——report() 在写失败路径上被调用，锁绝不能把
// 降级路径变成 panic 源（std Mutex 的 PoisonError unwrap 会）。
static REGISTRY: LazyLock<Mutex<HashMap<String, Entry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 登记一次降级写失败。去抖窗口内的重复失败只累计计数，不重复置告警。
pub fn report(kind: &str, detail: String) {
    let mut reg = REGISTRY.lock();
    let entry = reg.entry(kind.to_string()).or_default();
    entry.detail = detail;
    entry.count += 1;
    let ms = now_ms();
    if entry.first_at_ms == 0 {
        entry.first_at_ms = ms;
    }
    entry.last_at_ms = ms;
    let due = match entry.last_alert {
        None => true,
        Some(t) => t.elapsed() >= ALERT_DEBOUNCE,
    };
    if due {
        entry.last_alert = Some(Instant::now());
        entry.pending = true;
    }
}

/// 取走待告警条目（清 pending，保留条目供快照）。gateway 水泵轮询消费。
pub fn take_pending() -> Vec<DegradedWrite> {
    let mut reg = REGISTRY.lock();
    let mut out = Vec::new();
    for (kind, entry) in reg.iter_mut() {
        if entry.pending {
            entry.pending = false;
            out.push(DegradedWrite {
                kind: kind.clone(),
                detail: entry.detail.clone(),
                count: entry.count,
                first_at_ms: entry.first_at_ms,
                last_at_ms: entry.last_at_ms,
            });
        }
    }
    out.sort_by(|a, b| a.kind.cmp(&b.kind));
    out
}

/// 全量快照（`/ready` 健康面用；含去抖窗口内未告警的条目）。
pub fn snapshot() -> Vec<DegradedWrite> {
    let reg = REGISTRY.lock();
    let mut out: Vec<DegradedWrite> = reg
        .iter()
        .map(|(kind, e)| DegradedWrite {
            kind: kind.clone(),
            detail: e.detail.clone(),
            count: e.count,
            first_at_ms: e.first_at_ms,
            last_at_ms: e.last_at_ms,
        })
        .collect();
    out.sort_by(|a, b| a.kind.cmp(&b.kind));
    out
}

/// 清空注册表（仅测试用）。
pub fn reset_for_tests() {
    REGISTRY.lock().clear();
}

#[cfg(test)]
mod tests;

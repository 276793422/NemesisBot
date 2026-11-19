//! 防风暴三闸的纯逻辑（毫秒时间轴，全函数可测无 IO）。
//!
//! 三闸分工（项1 计划）：
//! 1. [`CooldownGate`]——per-rule 冷却：上次投递后 `cooldown_secs` 内不再投
//!    （0=关）；冷却期内的爆发直接丢弃，不排队不合并；
//! 2. [`HourWindow`]——滑动 1 小时窗口（per-rule `max_per_hour` 与全局上限
//!    共用同一逻辑；0=不限）；
//! 3. [`MergeBuffer`]——burst 合并：`merge_window_secs` 静默期攒事件合成一条
//!    带计数消息（0=下一个泵 tick 立即投）；同路径去重窗（debounce）内重复
//!    折叠为 1 次计数，列名封顶 8 条防消息膨胀。

use std::collections::HashMap;

const HOUR_MS: u64 = 3_600_000;
/// 消息内列出的路径上限（超出只计不列，由 {count} 承载诚实总量）。
pub const MAX_PATHS_LISTED: usize = 8;

/// per-rule 冷却闸（0 = 关闭）。
#[derive(Debug, Default, Clone)]
pub struct CooldownGate {
    last_ms: Option<u64>,
}

impl CooldownGate {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn allow(&self, now_ms: u64, cooldown_secs: u64) -> bool {
        if cooldown_secs == 0 {
            return true;
        }
        match self.last_ms {
            None => true,
            Some(t) => now_ms.saturating_sub(t) >= cooldown_secs.saturating_mul(1000),
        }
    }

    pub fn record(&mut self, now_ms: u64) {
        self.last_ms = Some(now_ms);
    }
}

/// 滑动 1 小时窗口（per-rule 与全局共用同一逻辑；0 = 不限）。
#[derive(Debug, Default, Clone)]
pub struct HourWindow {
    hits: std::collections::VecDeque<u64>,
}

impl HourWindow {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn allow(&mut self, now_ms: u64, max_per_hour: u32) -> bool {
        if max_per_hour == 0 {
            return true;
        }
        self.prune(now_ms);
        (self.hits.len() as u64) < max_per_hour as u64
    }

    pub fn record(&mut self, now_ms: u64) {
        self.prune(now_ms);
        self.hits.push_back(now_ms);
    }

    /// 窗口内当前计数（观测面用）。
    pub fn len(&self) -> usize {
        self.hits.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hits.is_empty()
    }

    fn prune(&mut self, now_ms: u64) {
        // 逐条差值判定（不用 cutoff 减法：小时间戳下 saturating_sub 会把
        // 「还差 1ms 满 1h」的条目误剪掉）。
        while self
            .hits
            .front()
            .is_some_and(|&t| now_ms.saturating_sub(t) >= HOUR_MS)
        {
            self.hits.pop_front();
        }
    }
}

/// burst 合并缓冲：攒一个静默窗的事件，合成一条带计数消息。
#[derive(Debug)]
pub struct MergeBuffer {
    first_ms: u64,
    last_ms: u64,
    count: u32,
    paths: Vec<String>,
    /// 同路径去重窗（debounce）：path → 最近一次事件时刻。
    last_seen: HashMap<String, u64>,
    /// 防抖表膨胀上限（超限整体清空——长尾路径的去重损失可接受，计数照常）。
    last_seen_cap: usize,
}

impl MergeBuffer {
    pub fn new(at_ms: u64) -> Self {
        Self {
            first_ms: at_ms,
            last_ms: at_ms,
            count: 0,
            paths: Vec::new(),
            last_seen: HashMap::new(),
            last_seen_cap: 256,
        }
    }

    /// 同路径去重判定：`debounce_ms` 窗口内同路径重复 = true（滑动窗，
    /// 持续写盘的路径一直折叠为 1 次）。命中时刷新时间戳。
    pub fn duplicate(&mut self, path: &str, at_ms: u64, debounce_ms: u64) -> bool {
        if debounce_ms == 0 {
            return false;
        }
        let dup = self
            .last_seen
            .get(path)
            .is_some_and(|&t| at_ms.saturating_sub(t) < debounce_ms);
        if self.last_seen.len() >= self.last_seen_cap {
            self.last_seen.clear();
        }
        self.last_seen.insert(path.to_string(), at_ms);
        dup
    }

    /// 记入一个事件（去重判定通过后调用）。
    pub fn push(&mut self, path: &str, at_ms: u64) {
        self.count += 1;
        self.last_ms = at_ms;
        if self.paths.len() < MAX_PATHS_LISTED && !self.paths.iter().any(|p| p == path) {
            self.paths.push(path.to_string());
        }
    }

    /// 静默期已过 = 可投递（`merge_window_secs` 0 = 有 buffer 即投）。
    pub fn should_flush(&self, now_ms: u64, merge_window_secs: u64) -> bool {
        if merge_window_secs == 0 {
            return true;
        }
        now_ms.saturating_sub(self.last_ms) >= merge_window_secs.saturating_mul(1000)
    }

    pub fn count(&self) -> u32 {
        self.count
    }

    pub fn paths(&self) -> &[String] {
        &self.paths
    }

    pub fn first_ms(&self) -> u64 {
        self.first_ms
    }

    /// 消费（投递/丢弃后取走计数）。
    pub fn into_count(self) -> u32 {
        self.count
    }
}

#[cfg(test)]
mod tests;

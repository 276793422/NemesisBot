//! 声明式量化风险限制（P0 vault 计划 D2，2026-09-22 计划 §4）。
//!
//! 通用机制原则：本模块不认识任何业务名词——工具经
//! [`Tool::limit_categories()`] 声明自己属于哪些类别（如 exec、
//! mass_message），规则由配置 `security.limits`（类别 → {max, window_secs}）
//! 提供，装配层（nemesisbot security_setup）启动时注入 [`set_rules`]。
//! 未注入或类别无规则 = 不限（缺省全关）。
//!
//! 计数是**进程内滑动窗口**（`VecDeque<Instant>` 淘汰出窗项）：重启清零
//! 是诚实的 v1 边界（计划 §4 明示），跨进程聚合留给后续批次。
//!
//! 检查点：estop 之后、security 管线之前（限额不是内容安全——不付
//! judge/scanner 成本）。超限不静默拒绝：走 auditor 审批直通车升级
//! （D3，`request_limit_approval`），批准 = 计数 + 放行；拒绝/超时/
//! 无审批通道 = fail-closed。

use parking_lot::{Mutex, RwLock};
use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

/// 单类别规则（与 nemesis_config::RateLimitRule 同构——机制层不反向依赖
/// 配置层的 serde 形态，装配层负责转换）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LimitRule {
    pub max: u32,
    pub window_secs: u64,
}

static RULES: RwLock<BTreeMap<String, LimitRule>> = RwLock::new(BTreeMap::new());
static COUNTERS: Mutex<BTreeMap<String, VecDeque<Instant>>> = Mutex::new(BTreeMap::new());

/// 装配层启动时注入规则（后装覆盖先装）。空表 = 全关（与配置缺省一致）。
pub fn set_rules(rules: BTreeMap<String, LimitRule>) {
    *RULES.write() = rules;
}

/// 测试 hygiene。
pub fn clear_all() {
    RULES.write().clear();
    COUNTERS.lock().clear();
}

/// 超限信息（拒绝消息与审批理由共用）。
#[derive(Debug, Clone)]
pub struct OverLimit {
    pub category: String,
    pub count: u32,
    pub max: u32,
    pub window_secs: u64,
}

impl OverLimit {
    /// 模型可读的拒绝串（fail-closed 分支）。
    pub fn denial_message(&self) -> String {
        format!(
            "⛔ RATE LIMIT EXCEEDED [layer:limits|category:{}] 窗口 {}s 内已 {} 次（上限 {}）——\
             工具未执行。不要立即重试；告知用户可等待窗口过后重试，或在 security.limits 调整/批准该类别。",
            self.category, self.window_secs, self.count, self.max
        )
    }
}

/// 纯检查（不计数）：任一类别超限返回 `Some(OverLimit)`；全在限内或无
/// 规则返回 `None`。
pub fn check_limit(categories: &[String]) -> Option<OverLimit> {
    let rules = RULES.read();
    let mut counters = COUNTERS.lock();
    let now = Instant::now();
    for category in categories {
        let Some(rule) = rules.get(category) else {
            continue;
        };
        let window = Duration::from_secs(rule.window_secs);
        let q = counters.entry(category.clone()).or_default();
        while let Some(front) = q.front() {
            if now.duration_since(*front) > window {
                q.pop_front();
            } else {
                break;
            }
        }
        if q.len() >= rule.max as usize {
            return Some(OverLimit {
                category: category.clone(),
                count: q.len() as u32,
                max: rule.max,
                window_secs: rule.window_secs,
            });
        }
    }
    None
}

/// 记一次执行（放行路径调用：under-limit 直接放行前、升级获批后各一次）。
/// 无规则的类别不记——不限额无需计数，也避免无规则类别的死条目在常驻
/// 进程里无界增长（检查侧从不淘汰无规则类别的队列）。
pub fn record(categories: &[String]) {
    let rules = RULES.read();
    let mut counters = COUNTERS.lock();
    let now = Instant::now();
    for category in categories {
        if rules.contains_key(category) {
            counters.entry(category.clone()).or_default().push_back(now);
        }
    }
}

/// 组合：在限内则记录并放行（`None`）；超限返回 `Some(OverLimit)`（**不
/// 计数**——是否放行由调用方的升级结果决定，获批后由调用方显式 `record`）。
///
/// 检查与记录在**同一临界区**完成（RULES.read + COUNTERS.lock 一次持有到
/// 结束，锁序恒为 RULES → COUNTERS）：旧实现分离两次加锁，check 与 record
/// 之间存在 TOCTOU——两个并发调用都在对方 record 前看到 count = max-1，
/// 双双放行，窗口计数超出 max（安全特性不允许，2026-09-21 复审修复）。
pub fn check_and_record(categories: &[String]) -> Option<OverLimit> {
    if categories.is_empty() {
        return None;
    }
    let rules = RULES.read();
    let mut counters = COUNTERS.lock();
    let now = Instant::now();
    // 先全量检查（淘汰出窗项 + 比对 max）；任一超限则整体不记——失败
    // 不产生半笔账。
    let mut over: Option<OverLimit> = None;
    for category in categories {
        let Some(rule) = rules.get(category) else {
            continue;
        };
        let window = Duration::from_secs(rule.window_secs);
        let q = counters.entry(category.clone()).or_default();
        while let Some(front) = q.front() {
            if now.duration_since(*front) > window {
                q.pop_front();
            } else {
                break;
            }
        }
        if q.len() >= rule.max as usize {
            over = Some(OverLimit {
                category: category.clone(),
                count: q.len() as u32,
                max: rule.max,
                window_secs: rule.window_secs,
            });
            break;
        }
    }
    if over.is_none() {
        for category in categories {
            if rules.contains_key(category) {
                counters.entry(category.clone()).or_default().push_back(now);
            }
        }
    }
    over
}

#[cfg(test)]
mod tests;

//! Swarm M3：讨论额度记账（master 侧，内存台账）。
//!
//! 三闸（impl-plan §9.5 + 风险表「agent 讨论死循环」四闸之额度闸）：
//! - **线程额度** `max_agent_turns_per_thread`：每线程 agent 发言上限
//!   （默认 8，0=不限）；剩余额度随 wake.post 下发（`max_turns_left`），
//!   扣完裁决器不再投递只推人——护栏握在 master 手里。
//! - **节点小时额度** `hourly_budget_per_node`：每节点每小时发言上限
//!   （默认 20，0=不限），滑动 1h 窗口。
//! - **上行限速** `rate_limit_per_min`：每节点每分钟 board.comment.post
//!   上限（0=不限），滑动 60s 窗口。
//!
//! 台账是**进程内存态**（重启清零 = 诚实边界：额度是防抖护栏不是计费
//! 账本，重启重置可接受，不为它加盘上状态）。拒绝映射信封错误码：
//! 线程/小时额度 → `quota_exhausted`，限速 → `rate_limited`。

use std::collections::HashMap;
use std::sync::Mutex;

/// 额度配置（config `board.discussion` 段 + 限速；0 = 该闸关闭）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaConfig {
    pub max_agent_turns_per_thread: u32,
    pub hourly_budget_per_node: u32,
    pub rate_limit_per_min: u32,
}

impl Default for QuotaConfig {
    fn default() -> Self {
        Self {
            max_agent_turns_per_thread: 8,
            hourly_budget_per_node: 20,
            rate_limit_per_min: 12,
        }
    }
}

/// 拒绝原因（上行 handler 据此映射信封 error code 与人读文案）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaDenied {
    /// 线程 agent 发言额度耗尽。
    ThreadQuotaExhausted,
    /// 节点小时额度耗尽。
    HourlyBudgetExhausted,
    /// 触发上行限速。
    RateLimited,
}

impl QuotaDenied {
    pub fn message(&self) -> &'static str {
        match self {
            QuotaDenied::ThreadQuotaExhausted => {
                "thread agent-turn quota exhausted (discuss on the board, not in this thread)"
            }
            QuotaDenied::HourlyBudgetExhausted => "hourly discussion budget exhausted for this node",
            QuotaDenied::RateLimited => "rate limited: too many messages per minute",
        }
    }

    /// 信封 error code（`nemesis_cluster::envelope::error_code` 词表子集，
    /// 字符串复制避免 board→cluster 依赖）。
    pub fn error_code(&self) -> &'static str {
        match self {
            QuotaDenied::ThreadQuotaExhausted | QuotaDenied::HourlyBudgetExhausted => {
                "quota_exhausted"
            }
            QuotaDenied::RateLimited => "rate_limited",
        }
    }
}

/// 单节点发言时间戳环形账（滑动窗口 pruning）。
#[derive(Default)]
struct NodeWindow {
    /// 60s 窗口（限速）。
    minute: Vec<i64>,
    /// 3600s 窗口（小时额度）。
    hour: Vec<i64>,
}

/// 每线程记账。
#[derive(Default)]
struct ThreadLedger {
    /// agent 发言已用次数。
    used: u32,
}

/// 额度配置供给（每次判定取一次快照）：live 接线见 gateway（读
/// ConfigStore live 句柄，config.json 热改即时生效）；`new` 提供固定快照。
type ConfigProvider = Box<dyn Fn() -> QuotaConfig + Send + Sync>;

/// 额度台账（`Arc<QuotaLedger>` 注入 master 的 nb_bus handler）。
pub struct QuotaLedger {
    config_provider: ConfigProvider,
    inner: Mutex<LedgerInner>,
}

#[derive(Default)]
struct LedgerInner {
    nodes: HashMap<String, NodeWindow>,
    threads: HashMap<String, ThreadLedger>,
}

impl QuotaLedger {
    /// 固定快照构造（测试 / 无 live 配置场景）：额度值恒为传入快照。
    pub fn new(config: QuotaConfig) -> Self {
        Self::with_provider(move || config)
    }

    /// live 构造：每次判定调用 provider 取最新额度（与 tier/hidden_tools
    /// 的 config.json 热改语义对齐，改额度不重启生效）。
    pub fn with_provider(provider: impl Fn() -> QuotaConfig + Send + Sync + 'static) -> Self {
        Self {
            config_provider: Box::new(provider),
            inner: Mutex::new(LedgerInner::default()),
        }
    }

    /// 当前生效额度（快照；wake.post 下发 `max_turns_left` 等读这里）。
    pub fn current_config(&self) -> QuotaConfig {
        (self.config_provider)()
    }

    /// 线程剩余 agent 发言额度（不扣账；wake.post 下发 `max_turns_left` 用）。
    pub fn turns_left(&self, thread_key: &str) -> u32 {
        let cfg = (self.config_provider)();
        if cfg.max_agent_turns_per_thread == 0 {
            return u32::MAX; // 不限。
        }
        let inner = self.lock();
        let used = inner
            .threads
            .get(thread_key)
            .map(|t| t.used)
            .unwrap_or(0);
        cfg.max_agent_turns_per_thread
            .saturating_sub(used)
    }

    /// 记账一次 agent 上行发言。Ok(剩余线程额度)；Err(拒绝原因)。
    /// 三闸全过才扣账（任一拒绝不产生副作用）。
    pub fn try_consume_post(
        &self,
        thread_key: &str,
        node_id: &str,
        now_secs: i64,
    ) -> Result<u32, QuotaDenied> {
        let mut inner = self.lock();
        let cfg = (self.config_provider)();

        // 闸 1：上行限速（60s 滑窗）。
        if cfg.rate_limit_per_min > 0 {
            let node = inner.nodes.entry(node_id.to_string()).or_default();
            node.minute.retain(|t| now_secs - *t < 60);
            if node.minute.len() as u32 >= cfg.rate_limit_per_min {
                return Err(QuotaDenied::RateLimited);
            }
        }
        // 闸 2：节点小时额度（1h 滑窗）。
        if cfg.hourly_budget_per_node > 0 {
            let node = inner.nodes.entry(node_id.to_string()).or_default();
            node.hour.retain(|t| now_secs - *t < 3600);
            if node.hour.len() as u32 >= cfg.hourly_budget_per_node {
                return Err(QuotaDenied::HourlyBudgetExhausted);
            }
        }
        // 闸 3：线程 agent 发言额度。
        if cfg.max_agent_turns_per_thread > 0 {
            let thread = inner.threads.entry(thread_key.to_string()).or_default();
            if thread.used >= cfg.max_agent_turns_per_thread {
                return Err(QuotaDenied::ThreadQuotaExhausted);
            }
        }

        // 全过 → 三账齐记。
        let node = inner.nodes.entry(node_id.to_string()).or_default();
        node.minute.push(now_secs);
        node.hour.push(now_secs);
        if cfg.max_agent_turns_per_thread > 0 {
            inner
                .threads
                .entry(thread_key.to_string())
                .or_default()
                .used += 1;
        }
        Ok(self.turns_left_locked(&inner, thread_key, cfg))
    }

    /// 判断（不扣账）：该节点现在能否发一条（wake 投递决策预检用——
    /// 额度耗尽的节点不投递只推人）。只查限速与小时额度，线程额度由
    /// 裁决器按目标线程单独看。
    pub fn can_post(&self, node_id: &str, now_secs: i64) -> Result<(), QuotaDenied> {
        let inner = self.lock();
        let cfg = (self.config_provider)();
        if let Some(node) = inner.nodes.get(node_id) {
            if cfg.rate_limit_per_min > 0 {
                let recent = node.minute.iter().filter(|t| now_secs - *t < 60).count();
                if recent as u32 >= cfg.rate_limit_per_min {
                    return Err(QuotaDenied::RateLimited);
                }
            }
            if cfg.hourly_budget_per_node > 0 {
                let recent = node.hour.iter().filter(|t| now_secs - *t < 3600).count();
                if recent as u32 >= cfg.hourly_budget_per_node {
                    return Err(QuotaDenied::HourlyBudgetExhausted);
                }
            }
        }
        Ok(())
    }

    fn turns_left_locked(&self, inner: &LedgerInner, thread_key: &str, cfg: QuotaConfig) -> u32 {
        let cap = cfg.max_agent_turns_per_thread;
        if cap == 0 {
            return u32::MAX;
        }
        let used = inner
            .threads
            .get(thread_key)
            .map(|t| t.used)
            .unwrap_or(0);
        cap.saturating_sub(used)
    }

    /// poison 恢复（台账是护栏不是账本，Mutex 中毒后重建空账继续——
    /// 宁可额度短暂失忆不可卡死讨论链路）。
    fn lock(&self) -> std::sync::MutexGuard<'_, LedgerInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests;

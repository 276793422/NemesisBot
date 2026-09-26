//! P32（能力扩展 Wave3）：prompt cache warmer（cache-warmer）。
//!
//! 语义：会话 idle 且 prompt cache TTL 将过期（idle 时长 ≥ 模型 cache TTL
//! 的 [`WARM_TRIGGER_RATIO`]）→ 用上一轮请求的**字节级快照**重放一次
//! （`max_tokens` 钉 1），刷新服务端 prompt cache；经济闸按价目表保守
//! 估算，超限 / 未命中跳过（默认上限 $0.05，`cost_limit_usd` 可配）。
//!
//! # TTL 真相源（诚实降级，锚点漂移记录）
//!
//! 计划原文设想「TTL 两档（anthropic 5min/1h）从 provider 元数据读」。
//! 实查结论：`nemesis-providers` 现网请求**不发** anthropic
//! `cache_control`（`anthropic.rs::build_request_body` 无该键；OpenAI 兼容
//! lane 的自动缓存 TTL 无文档），全仓库也不存在任何 prompt cache TTL 元
//! 数据（`nemesis-config` 的 `cache_ttl_seconds` 属 Skills 搜索缓存，无
//! 关）。「无源可读」→ 按计划自身的诚实降级条款执行：TTL 唯一可诚实来源
//! 是用户在 `model_list` 条目上**显式声明**的 `cache_ttl_secs`（anthropic
//! 5min=300 / 1h=3600 两档都由此表达；未声明 / 非法 / 未知模型 → 不
//! warm）。后续若给 anthropic lane 落 `cache_control` 线上表达（涉及
//! 计费行为变更，需单独决策），本模块解析链无需改动。
//!
//! # 开关语义（D-4：成本敏感 opt-in）
//!
//! `agents.cache_warmer.enabled` 默认 **false**——gateway 启动时
//! [`AgentLoop::spawn_cache_warmer_if_enabled`] 一次新鲜读后直接返回：
//! 无定时器、无任务对象（零副作用）。运行中改 false → 下一 tick 退出
//! （热关生效）；启动时关、运行中改 true **不热开**（无任务可感知，重启
//! gateway 生效——刻意的诚实不对称，见 spawn 点注释）。
//!
//! # 锁纪律
//!
//! 绝不 `try_acquire_session`（不与 turn / compact / steer 抢任何锁）；
//! warm 前 `is_session_busy` 检查，活跃会话绝不 warm；estop 期间整轮
//! 跳过（warm 也是 agent LLM 活动，急停覆盖它）。重放失败只 debug 级
//! 静默记录，绝不打扰用户、绝不重试风暴（失败同样推进锚点 = 每个 TTL
//! 窗口至多一次尝试）。

use super::prelude::*;
use super::*;

/// warm 触发阈值：idle 时长 ≥ 模型 cache TTL × 0.9 即触发重放。
pub(crate) const WARM_TRIGGER_RATIO: f64 = 0.9;

/// warm 扫描周期（秒）。轻量 tokio interval，不挂 cron（P31 域）。
pub(crate) const WARM_SCAN_INTERVAL_SECS: u64 = 60;

/// 单次 warm 重放的调用超时（秒）。`max_tokens=1` 的重放只有 prefill
/// 开销，120s 对超大上下文也宽裕；超时按失败处理（静默 debug）。
pub(crate) const WARM_CALL_TIMEOUT_SECS: u64 = 120;

/// warm 候选快照的会话数上限（每份快照 ≈ 一整个上下文的克隆，必须封顶；
/// 超限淘汰 anchor 最旧的会话）。
pub(crate) const MAX_WARM_CANDIDATES: usize = 8;

/// P32：warm 重放候选——某会话最近一次成功 LLM 请求的字节级快照。
///
/// 由 `run_loop` 在每次成功 LLM 轮后写入（后写覆盖 → 天然收敛到「最后一
/// 轮请求」），warm 重放时原样重发（`max_tokens` 除外）。
pub(crate) struct WarmCandidate {
    /// 请求消息体（字节级副本——warm 重放的前缀真相源）。
    pub(crate) messages: Vec<LlmMessage>,
    /// 工具 defs（与消息同属缓存前缀，必须一并原样）。
    pub(crate) tool_defs: Vec<crate::types::ToolDefinition>,
    /// 捕获时的模型别名（模型切换后旧快照作废——新模型下无对应缓存条目）。
    pub(crate) model: String,
    /// idle 锚点：最后一次真实请求完成 / 上次 warm 重放（含失败）完成时刻。
    pub(crate) anchor: std::time::Instant,
}

// ---------------------------------------------------------------------------
// 纯函数（单测在 cache_warmer_tests.rs）
// ---------------------------------------------------------------------------

/// 纯函数：warm 重放是否到期（idle 时长 ≥ TTL × [`WARM_TRIGGER_RATIO`]）。
/// `ttl_secs == 0` 恒不触发（防御）。
pub(crate) fn warm_due(anchor: std::time::Instant, ttl_secs: u64, now: std::time::Instant) -> bool {
    if ttl_secs == 0 {
        return false;
    }
    let threshold = ttl_secs as f64 * WARM_TRIGGER_RATIO;
    now.duration_since(anchor).as_secs_f64() >= threshold
}

/// 纯函数：解析某模型条目的 prompt cache TTL（秒）。只认条目上显式声明的
/// `cache_ttl_secs`（正整数）；条目缺失 / 键缺失 / 非法（0、负数、字符串
/// 数字等）→ `None`——**未知 TTL 不 warm**（诚实降级，见模块注释）。
///
/// 条目匹配语义与 `resolve_context_window_tiered` 同源：`model_list` 里
/// `model_name == alias` 或 `model == alias`。
pub(crate) fn resolve_cache_ttl_secs(cfg: Option<&serde_json::Value>, alias: &str) -> Option<u64> {
    let cfg = cfg?;
    let entry = cfg.get("model_list")?.as_array()?.iter().find(|m| {
        let name = m.get("model_name").and_then(|v| v.as_str()).unwrap_or("");
        let full = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
        name == alias || full == alias
    })?;
    let ttl = entry.get("cache_ttl_secs")?.as_u64()?;
    (ttl > 0).then_some(ttl)
}

/// 纯函数：经济闸判定。估算值 `None`（价目表未命中）= 保守跳过 → false；
/// 估算 ≤ 上限 → true。免费模型（估算 0.0）在 0 上限下也放行。
pub(crate) fn warm_cost_allowed(estimated_usd: Option<f64>, cost_limit_usd: f64) -> bool {
    match estimated_usd {
        Some(cost) => cost <= cost_limit_usd,
        None => false,
    }
}

/// 解析 `agents.cache_warmer` 配置节（宽松：段缺失 / 形态非法 → 类型默认
/// 值 `CacheWarmerConfig::default()` = 关 + $0.05）。类型与默认值单一真相
/// 源在 `nemesis_config::CacheWarmerConfig`，此处只做容错解包。
pub(crate) fn parse_cache_warmer_config(
    agents: Option<&serde_json::Value>,
) -> nemesis_config::CacheWarmerConfig {
    agents
        .and_then(|v| serde_json::from_value::<nemesis_config::AgentsConfig>(v.clone()).ok())
        .map(|a| a.cache_warmer)
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// AgentLoop 接线（快照存取 + spawn + 扫描 + 重放）
// ---------------------------------------------------------------------------

impl AgentLoop {
    /// 新鲜读 config.json（同 `config_watch` 各 current_* 模式；standalone
    /// 无 config_path → `None`）。
    fn read_config_json(&self) -> Option<serde_json::Value> {
        let path = self.config_path.read().clone()?;
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
    }

    /// `agents.cache_warmer` 新鲜读（config.json 唯一真相源；无 config /
    /// 段缺失 → 默认关——零副作用）。
    pub(crate) fn current_cache_warmer_config(&self) -> nemesis_config::CacheWarmerConfig {
        self.read_config_json()
            .map(|v| parse_cache_warmer_config(v.get("agents")))
            .unwrap_or_default()
    }

    /// 每轮 LLM 请求前的快照开关（run_loop 调用）。默认关 = 一次新鲜读、
    /// 无克隆（与 `current_max_tokens` 等每轮新鲜读同量级的既有先例）。
    pub(crate) fn cache_warmer_capture_enabled(&self) -> bool {
        self.current_cache_warmer_config().enabled
    }

    /// 存 warm 候选（run_loop 每次成功 LLM 轮调用；同会话后写覆盖 → 收敛
    /// 到最后一轮请求的快照）。超过 [`MAX_WARM_CANDIDATES`] 淘汰 anchor 最
    /// 旧的会话。
    pub(crate) fn store_warm_candidate(
        &self,
        session_key: &str,
        messages: Vec<LlmMessage>,
        tool_defs: Vec<crate::types::ToolDefinition>,
        model: String,
    ) {
        let mut map = self.warm_candidates.lock();
        if map.len() >= MAX_WARM_CANDIDATES && !map.contains_key(session_key) {
            if let Some(oldest) = map
                .iter()
                .min_by_key(|(_, c)| c.anchor)
                .map(|(k, _)| k.clone())
            {
                map.remove(&oldest);
            }
        }
        map.insert(
            session_key.to_string(),
            WarmCandidate {
                messages,
                tool_defs,
                model,
                anchor: std::time::Instant::now(),
            },
        );
    }

    /// spawn 点（`run_bus_impl` 启动时调用一次）：开关关 = 什么都不做
    /// （无定时器、无任务对象——零副作用）；开 = spawn 周期扫描任务。
    ///
    /// 热语义（诚实不对称）：启动时开、运行中改关 → 下一 tick 退出（热关
    /// 生效）；启动时关、运行中改开 → **不热开**（本函数只在 gateway 启动
    /// 跑一次，关闭态没有存活任务可感知 config 变化——重启生效）。
    pub(crate) fn spawn_cache_warmer_if_enabled(self: &Arc<Self>) {
        if !self.current_cache_warmer_config().enabled {
            return;
        }
        let this = self.clone();
        tokio::spawn(async move {
            this.run_cache_warmer_loop().await;
        });
    }

    /// warm 周期扫描主循环。每 tick：停机 / 热关 → 退出；estop → 跳过；
    /// 模型未声明 TTL → 跳过（诚实降级）；否则扫描到期候选（见
    /// [`Self::warm_scan_once`]）。
    pub(crate) async fn run_cache_warmer_loop(self: Arc<Self>) {
        info!(
            "[cache-warmer] active (scan every {}s; cost_limit from agents.cache_warmer)",
            WARM_SCAN_INTERVAL_SECS
        );
        let mut ticker =
            tokio::time::interval(std::time::Duration::from_secs(WARM_SCAN_INTERVAL_SECS));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            if !self.running.load(std::sync::atomic::Ordering::Acquire) {
                debug!("[cache-warmer] loop stopped, exiting");
                return;
            }
            if !self.current_cache_warmer_config().enabled {
                debug!("[cache-warmer] disabled by config, exiting (hot-off)");
                return;
            }
            self.warm_scan_once().await;
        }
    }

    /// 单次扫描：estop 跳过 → 解析活动模型 TTL（未声明 → 跳过）→ 清理
    /// 模型已切换的旧快照 → 对「到期且空闲」的会话逐个 warm。锁内只做
    /// 判定与清理，重放一律放锁外。
    pub(crate) async fn warm_scan_once(&self) {
        if self.security.is_engaged() {
            debug!("[cache-warmer] estop engaged, skip scan");
            return;
        }
        let model = self.active_model.read().clone();
        let ttl_secs = self
            .read_config_json()
            .and_then(|v| resolve_cache_ttl_secs(Some(&v), &model));
        let Some(ttl_secs) = ttl_secs else {
            debug!(
                "[cache-warmer] model {} has no declared cache_ttl_secs, skip (honest degradation)",
                model
            );
            return;
        };
        let cost_limit = self.current_cache_warmer_config().cost_limit_usd;
        let now = std::time::Instant::now();
        let due: Vec<String> = {
            let mut map = self.warm_candidates.lock();
            let mut due = Vec::new();
            // 模型已切换 → 旧模型快照全部作废（新模型下没有对应缓存条目，
            // 重放只会白花钱）。
            map.retain(|k, c| {
                if c.model != model {
                    return false;
                }
                if warm_due(c.anchor, ttl_secs, now) {
                    due.push(k.clone());
                }
                true
            });
            due
        };
        for key in due {
            // 活跃会话绝不 warm（turn / compact / steer 都可能占着会话；
            // 这里只读忙表，绝不 acquire——不与任何人抢锁）。
            if self.is_session_busy(&key) {
                debug!("[cache-warmer] {} busy, skip", key);
                continue;
            }
            self.warm_session_once(&key, &model, ttl_secs, cost_limit)
                .await;
        }
    }

    /// 单会话 warm 重放：快照取出（字节级原样）→ 经济闸 → `max_tokens=1`
    /// 重放 → 推进锚点。失败只 debug 静默记录；失败同样推进锚点 = 每个
    /// TTL 窗口至多一次尝试（防重试风暴）。
    pub(crate) async fn warm_session_once(
        &self,
        session_key: &str,
        active_model: &str,
        ttl_secs: u64,
        cost_limit_usd: f64,
    ) {
        let (messages, tool_defs, model) = {
            let map = self.warm_candidates.lock();
            let Some(c) = map.get(session_key) else {
                return;
            };
            if c.model != active_model {
                return;
            }
            (c.messages.clone(), c.tool_defs.clone(), c.model.clone())
        };
        let provider = self.provider.read().clone();

        // 经济闸（保守口径）：价目表未命中 → 跳过；估算按「全价 input +
        // 1 output token」，不假设任何 cache read 折扣（宁可高估不低估）。
        let estimated = self.estimate_warm_cost_usd(&messages, &tool_defs, &model);
        if !warm_cost_allowed(estimated, cost_limit_usd) {
            debug!(
                "[cache-warmer] {} warm skipped by cost gate (estimated {:?} vs limit {})",
                session_key, estimated, cost_limit_usd
            );
            return;
        }

        let options = crate::types::ChatOptions {
            max_tokens: Some(1),
            ..Default::default()
        };
        let result = Self::chat_call_bounded(
            WARM_CALL_TIMEOUT_SECS,
            &provider,
            &model,
            messages,
            Some(options),
            tool_defs,
        )
        .await;

        // 锚点推进。窗口内若来了真实请求，store_warm_candidate 会写入更新
        // 的候选；此处最坏把新候选锚点推后一个周期（少 warm 一次，无正确
        // 性影响——缓存过期只是下一次真实请求冷读）。
        {
            let mut map = self.warm_candidates.lock();
            if let Some(c) = map.get_mut(session_key) {
                c.anchor = std::time::Instant::now();
            }
        }
        match result {
            Ok(_) => debug!(
                "[cache-warmer] {} cache refreshed (model={}, ttl={}s)",
                session_key, model, ttl_secs
            ),
            Err(e) => debug!(
                "[cache-warmer] {} warm replay failed (silent): {}",
                session_key, e
            ),
        }
    }

    /// warm 重放成本保守估算（USD）。`None` = 价目表未命中（调用方保守
    /// 跳过）。口径：messages 文本 / tool_calls 参数 / 工具 defs schema 全
    /// 按**全价 input** 计（token 估算同 `session::estimate_tokens` 的
    /// chars×2/5），外加 1 个 output token——不假设任何 cache read 折扣。
    fn estimate_warm_cost_usd(
        &self,
        messages: &[LlmMessage],
        tool_defs: &[crate::types::ToolDefinition],
        model: &str,
    ) -> Option<f64> {
        let store = self.pricing_store.read().clone()?;
        let mut chars = 0usize;
        for m in messages {
            chars += m.content.chars().count();
            if let Some(tcs) = &m.tool_calls {
                for tc in tcs {
                    chars += tc.arguments.chars().count();
                }
            }
        }
        for t in tool_defs {
            chars += t.function.name.chars().count();
            chars += t.function.description.chars().count();
            chars += t.function.parameters.to_string().chars().count();
        }
        let input_tokens = (chars * 2 / 5).max(1) as i64;
        let breakdown = store.compute_cost_breakdown(model, input_tokens, 1, 0, 0)?;
        Some(breakdown.total_cost_usd)
    }
}

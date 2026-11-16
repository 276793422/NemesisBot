//! 召回质量闸（项2 P2）：top-k 召回 → 一次批量小模型判相关（保留/丢弃）
//! → 过滤后注入。
//!
//! 语义契约（`docs/PLAN/2026-11-16_质变级能力开发计划.md` 五）：
//! - gate 缺席/关 = 直通现状（`MemoryManager::filter_recall_hits` 原样返回）；
//! - 成本闸：候选数 < `min_candidates` = 不花 token 直通；
//! - 同 query+候选集短窗缓存复用判定（TTL `cache_secs`，默认 300s）；
//! - 判定失败 / 解析失败 / 返回长度不匹配 = **fail-open 全保留**（不确定时
//!   判保留，绝不因闸失败丢注入）；
//! - 统计（recalled/kept/dropped/judge_calls/cache_hits/…）供 `memory status`
//!   与 Dashboard 展示。
//!
//! 判定通道用 trait 抽象（[`RecallJudgeLlm`]）——gateway 侧接真实 provider，
//! 测试侧接 mock；本 crate 不依赖 nemesis-providers。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// 判定 LLM 抽象（gateway 侧 provider 适配 / 测试侧 mock）。
///
/// 返回 `Vec<bool>` 与 `candidates` 一一对应（true=保留）。长度不匹配由
/// gate 侧按 fail-open 处理（实现方不保证对齐）。
#[async_trait::async_trait]
pub trait RecallJudgeLlm: Send + Sync {
    async fn judge(&self, query: &str, candidates: &[String]) -> Result<Vec<bool>, String>;
}

/// 闸统计快照（`memory.status` / Dashboard 消费）。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct RecallGateStats {
    /// 进过闸的候选总数（含最终被保留与丢弃的；不含成本闸直通）。
    pub recalled: u64,
    /// 判定保留数。
    pub kept: u64,
    /// 判定丢弃数。
    pub dropped: u64,
    /// 实际调用判定 LLM 的次数（缓存命中不计）。
    pub judge_calls: u64,
    /// 缓存命中次数。
    pub cache_hits: u64,
    /// 成本闸直通次数（候选数不足阈值）。
    pub skipped_cost_gate: u64,
    /// fail-open 次数（判定/解析失败全保留）。
    pub errors: u64,
}

/// 判定缓存条目：verdict 与候选集绑定（同 query 不同候选 = 不同 key）。
struct CacheEntry {
    verdicts: Vec<bool>,
    at: Instant,
}

/// 召回质量闸。挂在 `MemoryManager` 上（`set_recall_gate`）；loop 侧经
/// `filter_recall_hits` 消费，gate 缺席 = 原样返回，loop 零新分支。
pub struct RecallGate {
    judge: Arc<dyn RecallJudgeLlm>,
    /// 成本闸阈值：候选 < 该数直通不判定。
    min_candidates: usize,
    /// 闸后注入条数上限（0 = 不再截断，沿用 loop 侧 top_k）。
    max_items: usize,
    cache_ttl: Duration,
    cache: Mutex<HashMap<u64, CacheEntry>>,
    stats: RecallGateCounters,
}

#[derive(Default)]
struct RecallGateCounters {
    recalled: AtomicU64,
    kept: AtomicU64,
    dropped: AtomicU64,
    judge_calls: AtomicU64,
    cache_hits: AtomicU64,
    skipped_cost_gate: AtomicU64,
    errors: AtomicU64,
}

impl RecallGate {
    pub fn new(
        judge: Arc<dyn RecallJudgeLlm>,
        min_candidates: usize,
        max_items: usize,
        cache_secs: u64,
    ) -> Self {
        Self {
            judge,
            min_candidates: min_candidates.max(1),
            max_items,
            cache_ttl: Duration::from_secs(if cache_secs == 0 { 300 } else { cache_secs }),
            cache: Mutex::new(HashMap::new()),
            stats: RecallGateCounters::default(),
        }
    }

    /// 缓存 key：query + 候选集内容（同一 query 候选不同 = 判定不可复用，
    /// verdict 与候选一一对应是契约）。
    fn cache_key(query: &str, candidates: &[String]) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        query.hash(&mut h);
        for c in candidates {
            c.hash(&mut h);
        }
        h.finish()
    }

    /// 过滤一批召回命中。永远返回非 Err——失败路径全部 fail-open 原样返回。
    pub async fn filter(&self, query: &str, candidates: Vec<String>) -> Vec<String> {
        if candidates.len() < self.min_candidates {
            self.stats.skipped_cost_gate.fetch_add(1, Ordering::Relaxed);
            return candidates;
        }

        let key = Self::cache_key(query, &candidates);
        if let Some(verdicts) = self.cache_lookup(key, candidates.len()) {
            self.stats.cache_hits.fetch_add(1, Ordering::Relaxed);
            return self.apply_verdicts(query, candidates, verdicts);
        }

        self.stats.judge_calls.fetch_add(1, Ordering::Relaxed);
        let verdicts = match self.judge.judge(query, &candidates).await {
            Ok(v) if v.len() == candidates.len() => v,
            Ok(v) => {
                tracing::warn!(
                    got = v.len(),
                    want = candidates.len(),
                    "[RecallGate] 判定返回长度不匹配——fail-open 全保留"
                );
                self.stats.errors.fetch_add(1, Ordering::Relaxed);
                return candidates;
            }
            Err(e) => {
                tracing::warn!(error = %e, "[RecallGate] 判定失败——fail-open 全保留");
                self.stats.errors.fetch_add(1, Ordering::Relaxed);
                return candidates;
            }
        };
        self.cache_insert(key, verdicts.clone());
        self.apply_verdicts(query, candidates, verdicts)
    }

    /// 命中检查 + 过期清扫（懒删除：读时跳过过期项，插入时全表清扫）。
    fn cache_lookup(&self, key: u64, want_len: usize) -> Option<Vec<bool>> {
        let mut cache = self.cache.lock();
        let now = Instant::now();
        cache.retain(|_, e| now.duration_since(e.at) < self.cache_ttl);
        let entry = cache.get(&key)?;
        // 候选集长度变化（理论不可能——key 绑定内容）防御性失配 = 不复用。
        if entry.verdicts.len() != want_len {
            return None;
        }
        Some(entry.verdicts.clone())
    }

    fn cache_insert(&self, key: u64, verdicts: Vec<bool>) {
        let mut cache = self.cache.lock();
        // 容量兜底：查询/清扫后仍超 256 条 = 极端噪音，整表清空重来。
        if cache.len() >= 256 {
            cache.clear();
        }
        cache.insert(
            key,
            CacheEntry {
                verdicts,
                at: Instant::now(),
            },
        );
    }

    /// 按 verdict 过滤 + 统计 + max_items 截断（保留序不变）。
    fn apply_verdicts(
        &self,
        query: &str,
        candidates: Vec<String>,
        verdicts: Vec<bool>,
    ) -> Vec<String> {
        self.stats
            .recalled
            .fetch_add(candidates.len() as u64, Ordering::Relaxed);
        let mut kept: Vec<String> = Vec::new();
        let mut dropped = 0usize;
        for (c, v) in candidates.into_iter().zip(verdicts) {
            if v {
                kept.push(c);
            } else {
                dropped += 1;
            }
        }
        self.stats
            .kept
            .fetch_add(kept.len() as u64, Ordering::Relaxed);
        self.stats
            .dropped
            .fetch_add(dropped as u64, Ordering::Relaxed);
        if self.max_items > 0 && kept.len() > self.max_items {
            kept.truncate(self.max_items);
        }
        if dropped > 0 {
            tracing::debug!(
                query = %truncate_for_log(query),
                kept = kept.len() + dropped,
                dropped,
                "[RecallGate] 召回过滤完成"
            );
        }
        kept
    }

    /// 统计快照（`memory.status` / Dashboard）。
    pub fn stats(&self) -> RecallGateStats {
        let s = &self.stats;
        RecallGateStats {
            recalled: s.recalled.load(Ordering::Relaxed),
            kept: s.kept.load(Ordering::Relaxed),
            dropped: s.dropped.load(Ordering::Relaxed),
            judge_calls: s.judge_calls.load(Ordering::Relaxed),
            cache_hits: s.cache_hits.load(Ordering::Relaxed),
            skipped_cost_gate: s.skipped_cost_gate.load(Ordering::Relaxed),
            errors: s.errors.load(Ordering::Relaxed),
        }
    }
}

fn truncate_for_log(s: &str) -> String {
    let cut = s.char_indices().nth(60).map(|(i, _)| i).unwrap_or(s.len());
    let head = &s[..cut];
    if cut < s.len() {
        format!("{head}…")
    } else {
        head.to_string()
    }
}

/// 通用判定 prompt + 响应解析（gateway 侧 provider 适配复用同一实现，
/// 保证 mock 测试与生产行为语义一致：解析失败 = Err → gate fail-open）。
pub fn judge_prompt(query: &str, candidates: &[String]) -> (String, String) {
    let system = "你是记忆召回相关性判定器。给定用户消息和若干候选记忆条目，\
判断每条候选是否包含有助于回应用户消息的信息。只输出一个 JSON 布尔数组，\
如 [true,false,true]，顺序与候选编号一致，不输出任何其他文字。\
不确定时输出 true（保留）。"
        .to_string();
    let mut user = format!("# 用户消息\n{query}\n\n# 候选记忆\n");
    for (i, c) in candidates.iter().enumerate() {
        user.push_str(&format!("{}. {}\n", i + 1, c));
    }
    user.push_str("\n输出 JSON 布尔数组：");
    (system, user)
}

/// 解析判定响应：提取首个 `[` 到末个 `]` 之间的 JSON 数组。解析失败 /
/// 非布尔元素 = Err（调用方 fail-open）。
pub fn parse_judge_response(raw: &str) -> Result<Vec<bool>, String> {
    let start = raw.find('[').ok_or("响应中无 JSON 数组")?;
    let end = raw.rfind(']').ok_or("响应中 JSON 数组未闭合")?;
    if end < start {
        return Err("响应中 JSON 数组括号次序异常".to_string());
    }
    serde_json::from_str::<Vec<bool>>(&raw[start..=end])
        .map_err(|e| format!("判定数组解析失败: {e}"))
}

// （纪律）：测试独立文件——声明在 lib.rs（dreaming_tests 同款惯例）。

//! P31 记忆 dreaming —— 三段式记忆巩固（召回记账 / cron sweep / LLM 决策 + 宿主产出）。
//!
//! 三段职责：
//! ① **召回记账**：检索命中时 touch 条目（`recall_count`/`last_recall`），
//!    标记式幂等（实现见 [`crate::manager::MemoryManager::record_recall`]）。
//! ② **sweep 候选**：对全量条目做 6 信号加权打分（召回次数 / 时间衰减 /
//!    条目冲突 / 冗余度 / 年龄 / 来源可靠性），取 topK 候选。
//! ③ **LLM 决策 + 宿主产出**：候选交 LLM 只做**决策**（merge/promote/expire/
//!    keep + 指出源条目 id），宿主从源证据构建新正文——**模型不产正文**
//!    （Forge Reflect 先例哲学）；候选处理后打 processed 标记防重复处理。
//!
//! 诚实边界：冲突/冗余信号是词面启发式（token Jaccard 分档），语义级冲突
//! 检测靠 LLM 决策兜底；真实 LLM 的决策质量不在单测覆盖范围（挂账）。

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use chrono::{DateTime, Local};
use serde::Serialize;

use crate::manager::MemoryManager;
use crate::types::{Entry, MemoryType};

// ---------------------------------------------------------------------------
// metadata 保留键（Entry.metadata 内，dreaming 专属；召回记账键在 manager.rs）
// ---------------------------------------------------------------------------

/// 已被某次 sweep 决策处理过的时间戳（RFC3339）。带此标记的条目不再进入
/// 后续 sweep 候选（幂等防重复处理）。
pub const META_DREAM_PROCESSED_AT: &str = "dream_processed_at";
/// 过期标记（"true"）。归档而非物理删除——条目仍可查证，只是不再参与 sweep。
pub const META_DREAM_ARCHIVED: &str = "dream_archived";
/// merge 产物的源条目 id 列表（逗号分隔）。
pub const META_DREAM_MERGED_FROM: &str = "dream_merged_from";
/// 来源可靠性自报值（[0,1]；1=用户明示，0.5=默认，0=模型自行生成）。
/// 生产方可在写入时盖章；缺省按 0.5 参与"来源可靠性"信号。
pub const META_DREAM_SOURCE_RELIABILITY: &str = "dream_source_reliability";

// ---------------------------------------------------------------------------
// 6 信号权重与打分纯函数
// ---------------------------------------------------------------------------

/// 6 信号权重（P31 ②）。打分时按权重和归一——调用方给未归一权重也安全。
#[derive(Debug, Clone, PartialEq)]
pub struct DreamingWeights {
    /// 召回次数（越常被召回越值得巩固/晋升）。
    pub recall: f64,
    /// 时间衰减（久未召回——过期/合并候选）。
    pub decay: f64,
    /// 条目冲突（与他条目疑似同主题不同内容——合并/裁决候选）。
    pub conflict: f64,
    /// 冗余度（与他条目近似重复——合并候选）。
    pub redundancy: f64,
    /// 年龄（越老越需要审视）。
    pub age: f64,
    /// 来源可靠性（来源越不可靠越需要审查）。
    pub source: f64,
}

impl Default for DreamingWeights {
    fn default() -> Self {
        Self {
            recall: 0.25,
            decay: 0.20,
            conflict: 0.20,
            redundancy: 0.15,
            age: 0.10,
            source: 0.10,
        }
    }
}

/// 一条条目的 6 信号取值（打分输入，全部由宿主预计算——[`score_entry`]
/// 是它的纯函数）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DreamSignals {
    /// 累计召回次数（来自 metadata `recall_count` 记账）。
    pub recall_count: u64,
    /// 久未召回的天数（从未召回 = 条目年龄）。
    pub days_since_last_recall: f64,
    /// 与其他条目近似重复的数量（词面 Jaccard >= 0.9）。
    pub duplicate_count: usize,
    /// 与其他条目疑似冲突的数量（词面 Jaccard 落在 [0.4, 0.9)）。
    pub conflict_count: usize,
    /// 条目年龄（天）。
    pub age_days: f64,
    /// 来源可靠性 [0,1]（缺省 0.5）。
    pub source_reliability: f64,
}

/// 饱和归一：`min(v / cap, 1.0)`。
fn saturate(v: f64, cap: f64) -> f64 {
    if cap <= 0.0 {
        return 0.0;
    }
    (v / cap).min(1.0)
}

/// 信号①召回次数：`1 - 1/(1+n)`（0 次=0，1 次=0.5，9 次=0.9，饱和趋近 1）。
pub fn signal_recall(recall_count: u64) -> f64 {
    1.0 - 1.0 / (1.0 + recall_count as f64)
}

/// 信号②时间衰减：距上次召回天数 / 90 饱和（从未召回按年龄计）。
pub fn signal_decay(days_since_last_recall: f64) -> f64 {
    saturate(days_since_last_recall.max(0.0), 90.0)
}

/// 信号③条目冲突：冲突数 / 3 饱和。
pub fn signal_conflict(conflict_count: usize) -> f64 {
    saturate(conflict_count as f64, 3.0)
}

/// 信号④冗余度：重复数 / 3 饱和。
pub fn signal_redundancy(duplicate_count: usize) -> f64 {
    saturate(duplicate_count as f64, 3.0)
}

/// 信号⑤年龄：天数 / 180 饱和。
pub fn signal_age(age_days: f64) -> f64 {
    saturate(age_days.max(0.0), 180.0)
}

/// 信号⑥来源可靠性：`1 - reliability`（可靠性越低信号越强）。
pub fn signal_source(source_reliability: f64) -> f64 {
    1.0 - source_reliability.clamp(0.0, 1.0)
}

/// 读取条目的来源可靠性自报值（缺省 0.5；越界钳到 [0,1]）。
pub fn source_reliability_of(entry: &Entry) -> f64 {
    entry
        .metadata
        .get(META_DREAM_SOURCE_RELIABILITY)
        .and_then(|s| s.parse::<f64>().ok())
        .map(|v| v.clamp(0.0, 1.0))
        .unwrap_or(0.5)
}

/// 读取条目的累计召回次数（缺省 0）。
pub fn recall_count_of(entry: &Entry) -> u64 {
    entry
        .metadata
        .get(crate::manager::META_RECALL_COUNT)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0)
}

/// 6 信号加权打分（P31 ②纯函数）。输入信号快照 + 权重，输出 [0,1] 分数
///（权重和为 0 时诚实返回 0）。打分不读存储、不修改条目——信号怎么来的
///（metadata 记账 / 两两词面比对）由调用方负责。
pub fn score_entry(signals: &DreamSignals, weights: &DreamingWeights) -> f64 {
    let parts = [
        (weights.recall, signal_recall(signals.recall_count)),
        (weights.decay, signal_decay(signals.days_since_last_recall)),
        (weights.conflict, signal_conflict(signals.conflict_count)),
        (
            weights.redundancy,
            signal_redundancy(signals.duplicate_count),
        ),
        (weights.age, signal_age(signals.age_days)),
        (weights.source, signal_source(signals.source_reliability)),
    ];
    let total_w: f64 = parts.iter().map(|(w, _)| *w).sum();
    if total_w <= 0.0 {
        return 0.0;
    }
    parts.iter().map(|(w, s)| w * s).sum::<f64>() / total_w
}

/// 由条目 + 关系信号构建信号快照（sweep 编排用；`last_recall` 缺省回落到
/// 条目年龄——从未被召回的条目按"久未召回"审视）。
pub fn build_signals(entry: &Entry, rel: &RelSignals, now: DateTime<Local>) -> DreamSignals {
    let age_days = (now - entry.created_at).num_hours() as f64 / 24.0;
    let days_since_recall = entry
        .metadata
        .get(crate::manager::META_LAST_RECALL)
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|t| {
            (now.naive_local() - t.with_timezone(&Local).naive_local()).num_hours() as f64 / 24.0
        })
        .unwrap_or(age_days);
    DreamSignals {
        recall_count: recall_count_of(entry),
        days_since_last_recall: days_since_recall,
        duplicate_count: rel.duplicate_count,
        conflict_count: rel.conflict_count,
        age_days,
        source_reliability: source_reliability_of(entry),
    }
}

// ---------------------------------------------------------------------------
// 词面关系启发式（宿主预计算 冲突/冗余 信号）
// ---------------------------------------------------------------------------

/// 词面 token 集（复用 BM25 分词——CJK 单字切分，中文条目可比）。
fn token_set(text: &str) -> HashSet<String> {
    crate::retrieval::tokens(text).into_iter().collect()
}

/// Jaccard 相似度：|A∩B| / |A∪B|（任一为空 → 0）。
pub fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count() as f64;
    let union = a.len() as f64 + b.len() as f64 - inter;
    if union <= 0.0 { 0.0 } else { inter / union }
}

/// 词面重复阈值：>= 视为近似重复。
pub const DUP_JACCARD: f64 = 0.9;
/// 词面疑似冲突下阈：>= 且 < DUP_JACCARD 视为同主题不同内容（疑似冲突）。
pub const CONFLICT_JACCARD: f64 = 0.4;

/// 每条目的关系信号（宿主预计算结果）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RelSignals {
    /// 近似重复的其他条目数。
    pub duplicate_count: usize,
    /// 疑似冲突的其他条目数。
    pub conflict_count: usize,
}

/// 两两词面比对，产出每条目的 (重复数, 冲突数)。O(n²)——sweep 每日一次，
/// 记忆条目量级下可接受。
pub fn compute_relationship_signals(entries: &[Entry]) -> HashMap<String, RelSignals> {
    let sets: Vec<(&Entry, HashSet<String>)> =
        entries.iter().map(|e| (e, token_set(&e.content))).collect();
    let mut out: HashMap<String, RelSignals> = HashMap::new();
    for i in 0..sets.len() {
        for j in (i + 1)..sets.len() {
            let sim = jaccard(&sets[i].1, &sets[j].1);
            if sim < CONFLICT_JACCARD {
                continue;
            }
            let dup = sim >= DUP_JACCARD;
            for a in [i, j] {
                let slot = out
                    .entry(sets[a].0.id.clone())
                    .or_insert_with(RelSignals::default);
                if dup {
                    slot.duplicate_count += 1;
                } else {
                    slot.conflict_count += 1;
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 候选选择
// ---------------------------------------------------------------------------

/// sweep 候选（打分后的条目 + 其信号快照，供 prompt 呈现证据）。
#[derive(Debug, Clone)]
pub struct DreamCandidate {
    pub entry: Entry,
    pub score: f64,
    pub signals: DreamSignals,
}

/// 条目是否已被 dreaming 处理过（processed 或 archived）——不再进候选。
pub fn is_dream_settled(entry: &Entry) -> bool {
    entry.metadata.contains_key(META_DREAM_PROCESSED_AT)
        || entry
            .metadata
            .get(META_DREAM_ARCHIVED)
            .map(|v| v == "true")
            .unwrap_or(false)
}

/// 由全量条目选出 topK 候选（P31 ②纯函数）。排除已处理/已归档条目；按
/// 分数降序（同分按创建时间升序——老条目优先），截取 top_k。
pub fn select_candidates(
    entries: Vec<Entry>,
    weights: &DreamingWeights,
    top_k: usize,
    now: DateTime<Local>,
) -> Vec<DreamCandidate> {
    let rel = compute_relationship_signals(&entries);
    let mut candidates: Vec<DreamCandidate> = entries
        .into_iter()
        .filter(|e| !is_dream_settled(e))
        .map(|e| {
            let rel = rel.get(&e.id).copied().unwrap_or_default();
            let signals = build_signals(&e, &rel, now);
            let score = score_entry(&signals, weights);
            DreamCandidate {
                entry: e,
                score,
                signals,
            }
        })
        .collect();
    candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.entry.created_at.cmp(&b.entry.created_at))
    });
    candidates.truncate(top_k.max(1));
    candidates
}

// ---------------------------------------------------------------------------
// LLM 决策协议（LLM 只决策，宿主产出——模型不产正文）
// ---------------------------------------------------------------------------

/// sweep 的 LLM 通道（Forge `LLMCaller` 同型先例：nemesis-memory 不依赖
/// provider，gateway 集成层提供具体实现——小模型通道 `agents.small_model`）。
#[async_trait]
pub trait DreamingLlm: Send + Sync {
    /// 发一次决策请求，返回模型原始输出（期望为 JSON 决策集）。
    async fn decide(&self, system_prompt: &str, user_prompt: &str) -> Result<String, String>;
}

/// 决策动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DreamAction {
    /// 合并：>=2 条说同一件事 → 宿主拼接去重正文新建条目。
    Merge,
    /// 晋升：正文原样提升 tier（→ long_term）。
    Promote,
    /// 过期：标记 archived（不物理删除）。
    Expire,
    /// 保留：仅打 processed 标记（不再进后续 sweep）。
    Keep,
}

impl DreamAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Promote => "promote",
            Self::Expire => "expire",
            Self::Keep => "keep",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "merge" => Some(Self::Merge),
            "promote" => Some(Self::Promote),
            "expire" => Some(Self::Expire),
            "keep" => Some(Self::Keep),
            _ => None,
        }
    }
}

/// 校验通过的决策。
#[derive(Debug, Clone, PartialEq)]
pub struct DreamDecision {
    pub action: DreamAction,
    pub source_ids: Vec<String>,
    pub reason: String,
}

/// 剥掉模型常见的 ```json 围栏，取出 JSON 文本。
fn strip_code_fence(raw: &str) -> &str {
    let t = raw.trim();
    if let Some(rest) = t.strip_prefix("```") {
        // 吃掉语言行（json / JSON / 空）
        let rest = rest.trim_start_matches(|c: char| c.is_ascii_alphanumeric());
        let rest = rest.strip_prefix('\n').unwrap_or(rest);
        if let Some(body) = rest.strip_suffix("```") {
            return body.trim();
        }
    }
    t
}

/// 解析 LLM 输出为原始决策数组（只做 JSON 结构解析，不做语义校验）。
/// 诚实失败：找不到合法 JSON → Err（sweep 记为失败而非静默吞掉）。
pub fn parse_decisions(raw: &str) -> Result<Vec<serde_json::Value>, String> {
    let text = strip_code_fence(raw);
    let v: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("LLM 决策输出不是合法 JSON: {e}"))?;
    let arr = v
        .get("decisions")
        .and_then(|d| d.as_array())
        .ok_or_else(|| "LLM 决策输出缺少 decisions 数组".to_string())?;
    Ok(arr.clone())
}

/// 单条决策校验（P31 ③边界闸）：
/// - 字段白名单 {action, source_ids, reason}——出现 `content`/`new_content`/
///   `text` 等正文字段 = 越界（模型不得产正文）→ 整条拒绝；
/// - action 必须是四动作之一；
/// - source_ids 必须非空且全部存在于候选集（引用不存在条目 → 拒绝）；
/// - merge 需 >=2 条源；promote/expire/keep 恰好 1 条源。
pub fn validate_decision(
    raw: &serde_json::Value,
    known_ids: &HashSet<String>,
) -> Result<DreamDecision, String> {
    let obj = raw
        .as_object()
        .ok_or_else(|| "决策项不是 JSON 对象".to_string())?;
    for key in obj.keys() {
        let k = key.to_ascii_lowercase();
        if k.contains("content") || k == "text" || k == "body" || k == "正文" {
            return Err(format!(
                "拒绝越界决策：模型试图产出正文字段 `{key}`（模型只决策，宿主产出）"
            ));
        }
    }
    let allowed = ["action", "source_ids", "reason"];
    for key in obj.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!(
                "拒绝决策：未知字段 `{key}`（协议只允许 action/source_ids/reason）"
            ));
        }
    }
    let action_str = obj
        .get("action")
        .and_then(|a| a.as_str())
        .ok_or_else(|| "决策缺少 action 字符串".to_string())?;
    let action = DreamAction::parse(action_str)
        .ok_or_else(|| format!("拒绝决策：未知 action `{action_str}`"))?;
    let ids: Vec<String> = obj
        .get("source_ids")
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .ok_or_else(|| "决策缺少 source_ids 数组".to_string())?;
    if ids.is_empty() {
        return Err("拒绝决策：source_ids 为空".to_string());
    }
    for id in &ids {
        if !known_ids.contains(id) {
            return Err(format!("拒绝决策：引用不存在的条目 id `{id}`"));
        }
    }
    let need = match action {
        DreamAction::Merge => 2,
        _ => 1,
    };
    if ids.len() < need {
        return Err(format!(
            "拒绝决策：action `{}` 需要至少 {need} 条源，实际 {}",
            action.as_str(),
            ids.len()
        ));
    }
    if action != DreamAction::Merge && ids.len() > 1 {
        return Err(format!(
            "拒绝决策：action `{}` 只接受 1 条源，实际 {}",
            action.as_str(),
            ids.len()
        ));
    }
    let reason = obj
        .get("reason")
        .and_then(|r| r.as_str())
        .unwrap_or("")
        .to_string();
    Ok(DreamDecision {
        action,
        source_ids: ids,
        reason,
    })
}

// ---------------------------------------------------------------------------
// 宿主产出（从源证据构建，模型不产正文）
// ---------------------------------------------------------------------------

/// merge 正文构建：源条目正文逐行拼接去重（宿主做）。行级归一（小写 +
/// 空白折叠）精确去重 + token Jaccard >= 0.9 的近似去重。
pub fn build_merged_content(sources: &[Entry]) -> String {
    let mut kept: Vec<String> = Vec::new();
    let mut kept_norm: Vec<String> = Vec::new();
    let mut kept_tokens: Vec<HashSet<String>> = Vec::new();
    for src in sources {
        for line in src.content.split('\n') {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let norm = line.to_lowercase().split_whitespace().collect::<String>();
            let toks = token_set(line);
            let dup = kept_norm.iter().any(|k| *k == norm)
                || kept_tokens.iter().any(|k| jaccard(k, &toks) >= DUP_JACCARD);
            if dup {
                continue;
            }
            kept.push(line.to_string());
            kept_norm.push(norm);
            kept_tokens.push(toks);
        }
    }
    kept.join("\n")
}

/// 打 processed 标记（幂等防重复处理）：写 `dream_processed_at` 并刷新
/// updated_at，走 update 通路持久化。
async fn mark_processed(
    manager: &MemoryManager,
    mut entry: Entry,
    now: DateTime<Local>,
) -> Result<(), String> {
    entry
        .metadata
        .insert(META_DREAM_PROCESSED_AT.to_string(), now.to_rfc3339());
    entry.updated_at = Local::now();
    manager.update_entry(entry).await
}

/// 应用一条已校验决策（P31 ③宿主产出）。
/// 返回实际执行的动作名（供报告记账）。
/// - merge：宿主拼接去重源正文 → 新建 long_term 条目（metadata 记
///   `dream_merged_from`，tags 取并集）；源条目仅打 processed 标记（是否
///   归档由 LLM 对源条目的独立 expire 决策决定——动作语义正交）。
/// - promote：源条目 tier 提升 → long_term，正文不变。
/// - expire：源条目打 archived 标记（不物理删除）。
/// - keep：仅打 processed 标记。
pub async fn apply_decision(
    manager: &MemoryManager,
    decision: &DreamDecision,
    now: DateTime<Local>,
) -> Result<&'static str, String> {
    // TOCTOU 复核：校验后源条目可能已被并发处理/删除——逐条现取。
    let mut sources: Vec<Entry> = Vec::with_capacity(decision.source_ids.len());
    for id in &decision.source_ids {
        let entry = manager
            .get(id)
            .await?
            .ok_or_else(|| format!("源条目 `{id}` 已不存在（决策无法应用）"))?;
        if is_dream_settled(&entry) {
            return Err(format!("源条目 `{id}` 已被处理过（幂等保护，决策拒绝）"));
        }
        sources.push(entry);
    }

    match decision.action {
        DreamAction::Merge => {
            let merged_content = build_merged_content(&sources);
            if merged_content.trim().is_empty() {
                return Err("merge 产出的正文为空（源条目正文全被去重？），决策拒绝".to_string());
            }
            let mut tags: Vec<String> = Vec::new();
            for s in &sources {
                for t in &s.tags {
                    if !tags.contains(t) {
                        tags.push(t.clone());
                    }
                }
            }
            let mut merged = Entry::new(MemoryType::LongTerm, merged_content).with_tags(tags);
            merged.metadata.insert(
                META_DREAM_MERGED_FROM.to_string(),
                decision.source_ids.join(","),
            );
            manager.store_entry(merged).await?;
            for src in sources {
                mark_processed(manager, src, now).await?;
            }
            Ok("merge")
        }
        DreamAction::Promote => {
            let mut e = sources.into_iter().next().expect("promote 恰 1 条源");
            e.typ = MemoryType::LongTerm;
            mark_processed(manager, e, now).await?;
            Ok("promote")
        }
        DreamAction::Expire => {
            let mut e = sources.into_iter().next().expect("expire 恰 1 条源");
            e.metadata
                .insert(META_DREAM_ARCHIVED.to_string(), "true".to_string());
            mark_processed(manager, e, now).await?;
            Ok("expire")
        }
        DreamAction::Keep => {
            let e = sources.into_iter().next().expect("keep 恰 1 条源");
            mark_processed(manager, e, now).await?;
            Ok("keep")
        }
    }
}

// ---------------------------------------------------------------------------
// sweep 编排（P31 ②+③ 闭环）
// ---------------------------------------------------------------------------

/// 一次 sweep 的结果报告（Serialize 供 gateway 落盘 JSON）。
#[derive(Debug, Clone, Default, Serialize)]
pub struct SweepReport {
    /// 全量条目数（sweep 输入）。
    pub total_entries: usize,
    /// 送 LLM 的候选数。
    pub candidates: usize,
    /// LLM 返回的决策数。
    pub decisions_returned: usize,
    /// 实际应用的决策数。
    pub decisions_applied: usize,
    /// 被拒绝/应用失败的决策数。
    pub decisions_rejected: usize,
    pub merged: usize,
    pub promoted: usize,
    pub expired: usize,
    pub kept: usize,
    /// 拒绝/失败原因（诚实留痕）。
    pub notes: Vec<String>,
}

/// 决策系统提示：只决策、不产正文、只输出 JSON。
pub fn decision_system_prompt() -> String {
    "你是记忆系统的巩固决策器（dreaming）。你唯一的职责是对候选记忆条目做处置决策。\
硬性规则：\n\
1. 你绝对不能输出任何记忆正文内容——不允许 content/new_content/text 之类的字段；新正文由系统从源条目自动构建。\n\
2. 只输出一个 JSON 对象，形如：\n\
   {\"decisions\": [{\"action\": \"merge|promote|expire|keep\", \"source_ids\": [\"条目id\"], \"reason\": \"一句话理由\"}]}\n\
3. action 语义：merge=多条说同一件事（至少 2 条源）；promote=值得长期保留（1 条源）；expire=过时/无价值（1 条源，系统会归档不删除）；keep=暂时不动（1 条源）。\n\
4. source_ids 必须从候选清单的 id 中选取，不得编造。\n\
5. 没有值得处理的条目就返回 {\"decisions\": []}。"
        .to_string()
}

/// 截断文本到 max chars（rune 安全）。
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    format!("{cut}…")
}

/// 决策用户提示：候选清单 + 证据（信号快照）。
pub fn build_decision_user_prompt(candidates: &[DreamCandidate]) -> String {
    let mut sb = String::new();
    sb.push_str(&format!(
        "以下是 {} 条记忆候选（已按巩固紧迫度排序）。请给出处置决策。\n\n",
        candidates.len()
    ));
    for (i, c) in candidates.iter().enumerate() {
        let e = &c.entry;
        sb.push_str(&format!(
            "### 候选 {} id=`{}`\n- type: {}\n- 综合分: {:.3}\n- 召回次数: {}\n- 疑似重复条目数: {}\n- 疑似冲突条目数: {}\n- 条目年龄: {:.0} 天\n- 来源可靠性: {:.2}\n- 正文: {}\n\n",
            i + 1,
            e.id,
            e.typ,
            c.score,
            recall_count_of(e),
            c.signals.duplicate_count,
            c.signals.conflict_count,
            c.signals.age_days,
            c.signals.source_reliability,
            truncate_chars(&e.content, 300),
        ));
    }
    sb.push_str(
        "提示：重复条目数 >= 1 的候选通常适合 merge（把重复的候选 id 放进同一个 merge 决策）；\
         久未召回且无重复的候选考虑 expire 或 keep；仍在使用中的短期条目考虑 promote。\n",
    );
    sb
}

/// 跑一轮 sweep 闭环（P31 ②+③）：全量条目 → 6 信号打分 topK → LLM 决策 →
/// 宿主产出 → 幂等标记。LLM 调用/JSON 解析失败 = Err（调用方如实记账）；
/// 单条决策非法 = 记入报告 notes 后继续（不因个别越界决策丢掉整轮）。
pub async fn run_sweep(
    manager: &MemoryManager,
    llm: &dyn DreamingLlm,
    weights: &DreamingWeights,
    top_k: usize,
    now: DateTime<Local>,
) -> Result<SweepReport, String> {
    let entries = manager.list_all_entries().await?;
    let mut report = SweepReport {
        total_entries: entries.len(),
        ..Default::default()
    };
    let candidates = select_candidates(entries, weights, top_k, now);
    report.candidates = candidates.len();
    if candidates.is_empty() {
        return Ok(report);
    }

    let system = decision_system_prompt();
    let user = build_decision_user_prompt(&candidates);
    let raw = llm.decide(&system, &user).await?;
    let parsed = parse_decisions(&raw)?;

    // 已被本轮其他决策占用的源条目（同源二次决策 = 幂等拒绝）。
    let mut taken: HashSet<String> = HashSet::new();
    let known: HashSet<String> = candidates.iter().map(|c| c.entry.id.clone()).collect();

    for raw_decision in parsed {
        report.decisions_returned += 1;
        let decision = match validate_decision(&raw_decision, &known) {
            Ok(d) => d,
            Err(reason) => {
                tracing::warn!("[Dreaming] 决策被拒绝: {reason}");
                report.decisions_rejected += 1;
                report.notes.push(reason);
                continue;
            }
        };
        if decision.source_ids.iter().any(|id| taken.contains(id)) {
            let reason = format!(
                "拒绝决策：源条目已被本轮其他决策处理（{:?}）",
                decision.source_ids
            );
            tracing::warn!("[Dreaming] {reason}");
            report.decisions_rejected += 1;
            report.notes.push(reason);
            continue;
        }
        match apply_decision(manager, &decision, now).await {
            Ok(action) => {
                report.decisions_applied += 1;
                taken.extend(decision.source_ids.iter().cloned());
                match action {
                    "merge" => report.merged += 1,
                    "promote" => report.promoted += 1,
                    "expire" => report.expired += 1,
                    _ => report.kept += 1,
                }
                tracing::info!(
                    "[Dreaming] 决策已应用: action={action} sources={:?} reason={}",
                    decision.source_ids,
                    decision.reason
                );
            }
            Err(e) => {
                tracing::warn!("[Dreaming] 决策应用失败: {e}");
                report.decisions_rejected += 1;
                report.notes.push(e);
            }
        }
    }
    Ok(report)
}

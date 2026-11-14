//! Pattern extraction - detects reusable patterns from experience data.
//!
//! B1（缺陷整改批次 2026-11-14）：检测逻辑单一来源 =
//! [`extract_conversation_patterns`]（会话级 ConversationPattern，镜像 Go）。
//! 旧「经验级」工具计数检测器族（`Pattern`/`PatternType` +
//! 四个 detect_*）已整体删除——它们把「单工具出现次数」当链、失败率当
//! 恢复，产出的伪模式污染下游决策表。
//!
//! `ConversationPattern` 提供会话级富元数据（SHA256 指纹去重、
//! first_seen/last_seen、参数键聚合、类型专属字段 tool_chain/avg_rounds/
//! error_tool/recovery_tool/efficiency_score）。

use std::collections::HashMap;

use sha2::{Digest, Sha256};

use nemesis_types::utils;

use crate::types::CollectedExperience;

// ---------------------------------------------------------------------------
// ConversationPattern — rich pattern type (mirrors Go's ConversationPattern)
// ---------------------------------------------------------------------------

/// Pattern type identifier (mirrors Go's PatternType string enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConversationPatternType {
    ToolChain,
    ErrorRecovery,
    EfficiencyIssue,
    SuccessTemplate,
}

impl ConversationPatternType {
    /// Convert to the string used in Go.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ToolChain => "tool_chain",
            Self::ErrorRecovery => "error_recovery",
            Self::EfficiencyIssue => "efficiency_issue",
            Self::SuccessTemplate => "success_template",
        }
    }
}

/// A recurring pattern detected in conversation traces.
///
/// Mirrors Go's `ConversationPattern` struct with rich metadata including
/// SHA256 fingerprint, first_seen/last_seen tracking, and type-specific fields.
#[derive(Debug, Clone)]
pub struct ConversationPattern {
    /// Unique pattern ID (e.g. "tc-abc123def456").
    pub id: String,
    /// Pattern type.
    pub pattern_type: ConversationPatternType,
    /// SHA256 fingerprint for deduplication.
    pub fingerprint: String,
    /// How many times this pattern was seen.
    pub frequency: u32,
    /// Confidence score [0, 1].
    pub confidence: f64,
    /// First occurrence timestamp (ISO 8601).
    pub first_seen: String,
    /// Last occurrence timestamp (ISO 8601).
    pub last_seen: String,

    // Type-specific fields
    /// Tool chain description (for tool_chain, efficiency_issue, success_template).
    pub tool_chain: Option<String>,
    /// Average rounds (for tool_chain, success_template).
    pub avg_rounds: Option<f64>,
    /// Average duration in ms (for tool_chain, efficiency_issue, success_template).
    pub avg_duration_ms: Option<i64>,
    /// Success rate [0, 1] (for tool_chain, error_recovery, success_template).
    pub success_rate: Option<f64>,
    /// Tool that failed (for error_recovery).
    pub error_tool: Option<String>,
    /// Error code (for error_recovery).
    pub error_code: Option<String>,
    /// Tool that recovered (for error_recovery).
    pub recovery_tool: Option<String>,
    /// Efficiency score (for efficiency_issue).
    pub efficiency_score: Option<f64>,
    /// Common argument keys (for tool_chain, success_template).
    pub common_arg_keys: Vec<String>,
    /// Human-readable description.
    pub description: String,
}

impl ConversationPattern {
    /// Create a new conversation pattern with the given type and fingerprint.
    pub fn new(pattern_type: ConversationPatternType, fingerprint: &str) -> Self {
        let prefix = match pattern_type {
            ConversationPatternType::ToolChain => "tc",
            ConversationPatternType::ErrorRecovery => "er",
            ConversationPatternType::EfficiencyIssue => "ef",
            ConversationPatternType::SuccessTemplate => "st",
        };
        let id_prefix = if fingerprint.len() >= 12 {
            let end = utils::floor_char_boundary(fingerprint, 12);
            &fingerprint[..end]
        } else {
            fingerprint
        };
        Self {
            id: format!("{}-{}", prefix, id_prefix),
            pattern_type,
            fingerprint: fingerprint.to_string(),
            frequency: 0,
            confidence: 0.0,
            first_seen: String::new(),
            last_seen: String::new(),
            tool_chain: None,
            avg_rounds: None,
            avg_duration_ms: None,
            success_rate: None,
            error_tool: None,
            error_code: None,
            recovery_tool: None,
            efficiency_score: None,
            common_arg_keys: Vec::new(),
            description: String::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Fingerprint helpers
// ---------------------------------------------------------------------------

/// Generate a SHA256 fingerprint for pattern deduplication.
///
/// Mirrors Go's `patternFingerprint(prefix, data)`.
pub fn pattern_fingerprint(prefix: &str, data: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{}:{}", prefix, data).as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Normalize a tool chain for fingerprinting.
/// The chain is order-sensitive (read->edit->exec != edit->read->exec).
///
/// Mirrors Go's `deduplicateChainString`.
pub fn dedup_chain_string(tools: &[&str]) -> String {
    tools.join("\u{2192}") // → arrow
}

/// Extract tool names from a chain string like "read->edit->exec".
///
/// Mirrors Go's `extractToolChain`.
pub fn extract_tool_chain_from_experiences(experiences: &[CollectedExperience]) -> String {
    experiences
        .iter()
        .map(|e| e.experience.tool_name.as_str())
        .collect::<Vec<_>>()
        .join("\u{2192}") // → arrow
}

// ---------------------------------------------------------------------------
// Conversation-level pattern extraction
// ---------------------------------------------------------------------------

/// Aggregation data for tool chain pattern detection.
#[allow(dead_code)]
struct ChainAgg {
    chain: String,
    count: u32,
    successes: u32,
    total_rounds: f64,
    total_dur: i64,
    first_seen: String,
    last_seen: String,
    #[allow(dead_code)]
    arg_keys: HashMap<String, u32>,
}

/// Aggregation data for error recovery pattern detection.
#[allow(dead_code)]
struct RecoveryAgg {
    error_tool: String,
    recovery_tool: String,
    count: u32,
    #[allow(dead_code)]
    error_codes: HashMap<String, u32>,
    first_seen: String,
    last_seen: String,
}

/// Aggregation data for efficiency issue detection.
struct EffAgg {
    chain: String,
    count: u32,
    total_rounds: f64,
    total_dur: i64,
    first_seen: String,
    last_seen: String,
}

/// Aggregation data for success template detection.
#[allow(dead_code)]
struct SuccAgg {
    chain: String,
    count: u32,
    successes: u32,
    total_rounds: f64,
    total_dur: i64,
    first_seen: String,
    last_seen: String,
    #[allow(dead_code)]
    arg_keys: HashMap<String, u32>,
}

/// Detect tool chain patterns at the conversation level (mirrors Go's detectToolChainPatterns).
pub fn detect_conversation_tool_chains(
    experiences: &[CollectedExperience],
    min_freq: u32,
) -> Vec<ConversationPattern> {
    let mut agg: HashMap<String, ChainAgg> = HashMap::new();

    // Group experiences by session, then extract chains per session
    let mut sessions: HashMap<&str, Vec<&CollectedExperience>> = HashMap::new();
    for exp in experiences {
        sessions
            .entry(&exp.experience.session_key)
            .or_default()
            .push(exp);
    }

    for exps in sessions.values() {
        if exps.len() < 2 {
            continue;
        }
        let chain: Vec<&str> = exps
            .iter()
            .map(|e| e.experience.tool_name.as_str())
            .collect();
        let chain_str = chain.join("\u{2192}");
        let fp = pattern_fingerprint("tool_chain", &chain_str);

        let all_success = exps.iter().all(|e| e.experience.success);
        let total_rounds = exps.len() as f64;
        let total_dur: i64 = exps.iter().map(|e| e.experience.duration_ms as i64).sum();
        let timestamp = exps.first().unwrap().experience.timestamp.clone();

        if let Some(a) = agg.get_mut(&fp) {
            a.count += 1;
            a.total_rounds += total_rounds;
            a.total_dur += total_dur;
            if all_success {
                a.successes += 1;
            }
            if timestamp < a.first_seen {
                a.first_seen = timestamp.clone();
            }
            if timestamp > a.last_seen {
                a.last_seen = timestamp.clone();
            }
        } else {
            agg.insert(
                fp,
                ChainAgg {
                    chain: chain_str,
                    count: 1,
                    successes: if all_success { 1 } else { 0 },
                    total_rounds,
                    total_dur,
                    first_seen: timestamp.clone(),
                    last_seen: timestamp,
                    arg_keys: HashMap::new(),
                },
            );
        }
    }

    agg.into_iter()
        .filter(|(_, a)| a.count >= min_freq)
        .map(|(fp, a)| {
            let success_rate = a.successes as f64 / a.count as f64;
            let confidence = (a.count as f64 / min_freq as f64).min(1.0) * success_rate;
            let mut p = ConversationPattern::new(ConversationPatternType::ToolChain, &fp);
            p.frequency = a.count;
            p.confidence = confidence;
            p.first_seen = a.first_seen;
            p.last_seen = a.last_seen;
            p.tool_chain = Some(a.chain.clone());
            p.avg_rounds = Some(a.total_rounds / a.count as f64);
            p.avg_duration_ms = Some(a.total_dur / a.count as i64);
            p.success_rate = Some(success_rate);
            p.description = format!(
                "Tool chain: {} (seen {} times, {:.0}% success)",
                a.chain,
                a.count,
                success_rate * 100.0
            );
            p
        })
        .collect()
}

/// Detect error recovery patterns at the conversation level (mirrors Go's detectErrorRecoveryPatterns).
///
/// BUG-20261114-017/018 两处语义修复：
/// - **会话内配对**：旧实现全局按时间排序后 `windows(2)`——跨会话事件穿插在
///   失败与恢复之间时，同会话 (失败,恢复) 对永不相邻 → 系统性漏检（多会话
///   并发是常态）。与其余三个检测器同构：先按 session 分组，会话内排序后
///   再配对。单调增益：旧实现能检出的对（全局相邻且同会话）在新结构下
///   依旧相邻，只增不漏。
/// - **真置信度**：旧实现 `successes` 与 `count` 在同一分支同步自增（检测
///   条件本身即「失败后恢复成功」），recovery_rate 恒 ≡ 1.0 → 决策表
///   conf ≥ high_confidence 恒真，`error_recovery_suggest_min` 臂死代码
///   （同提交已对 success_template 的同构恒真式裁断「置信度闸恒真无区分
///   度」）。现分母 = 失败工具在本批经验中的失败总数（无论后续跟什么：
///   同工具重试 / 其他工具失败 / 会话结束，失败都计入），置信度 = 恢复
///   覆盖率 hits / failures(error_tool)，(0,1] 区间真方差，三臂表恢复
///   区分度。
pub fn detect_conversation_error_recovery(
    experiences: &[CollectedExperience],
    min_freq: u32,
) -> Vec<ConversationPattern> {
    let mut agg: HashMap<String, RecoveryAgg> = HashMap::new();
    // 失败工具 → 失败总次数（置信度分母，全会话累计；map-out 阶段已完整）。
    let mut failures: HashMap<String, u32> = HashMap::new();

    // Group by session first, then pair within each session.
    let mut sessions: HashMap<&str, Vec<&CollectedExperience>> = HashMap::new();
    for exp in experiences {
        sessions
            .entry(&exp.experience.session_key)
            .or_default()
            .push(exp);
    }

    for exps in sessions.values() {
        let mut sorted: Vec<&CollectedExperience> = exps.to_vec();
        sorted.sort_by(|a, b| a.experience.timestamp.cmp(&b.experience.timestamp));

        for (i, prev) in sorted.iter().enumerate() {
            // 失败计数与配对解耦：凡是失败都计入分母（哪怕后面没跟任何
            // 事件——会话尾部的失败也是未被恢复的失败）。
            if prev.experience.success {
                continue;
            }
            *failures
                .entry(prev.experience.tool_name.clone())
                .or_insert(0) += 1;

            let Some(next) = sorted.get(i + 1) else {
                continue;
            };
            // 恢复命中：同会话相邻、失败工具 ≠ 恢复工具、下一事件成功。
            // （同工具成功 = 重试自愈，不属于跨工具恢复链。）
            if next.experience.success && next.experience.tool_name != prev.experience.tool_name {
                let key = format!(
                    "{}:{}",
                    prev.experience.tool_name, next.experience.tool_name
                );
                let fp = pattern_fingerprint("error_recovery", &key);

                let timestamp = prev.experience.timestamp.clone();

                if let Some(a) = agg.get_mut(&fp) {
                    a.count += 1;
                    if timestamp < a.first_seen {
                        a.first_seen = timestamp.clone();
                    }
                    if timestamp > a.last_seen {
                        a.last_seen = timestamp.clone();
                    }
                } else {
                    agg.insert(
                        fp,
                        RecoveryAgg {
                            error_tool: prev.experience.tool_name.clone(),
                            recovery_tool: next.experience.tool_name.clone(),
                            count: 1,
                            error_codes: HashMap::new(),
                            first_seen: timestamp.clone(),
                            last_seen: timestamp,
                        },
                    );
                }
            }
        }
    }

    agg.into_iter()
        .filter(|(_, a)| a.count >= min_freq)
        .map(|(fp, a)| {
            // 覆盖率 = 命中数 / 失败工具失败总数；失败数 ≥ 命中数（每次
            // 命中都对应一次失败），分母恒 ≥ 1。max 守卫防御性兜底。
            let denom = failures
                .get(&a.error_tool)
                .copied()
                .unwrap_or(0)
                .max(a.count) as f64;
            let recovery_rate = a.count as f64 / denom;
            let confidence = recovery_rate;
            let mut p = ConversationPattern::new(ConversationPatternType::ErrorRecovery, &fp);
            p.frequency = a.count;
            p.confidence = confidence;
            p.first_seen = a.first_seen;
            p.last_seen = a.last_seen;
            p.error_tool = Some(a.error_tool.clone());
            p.recovery_tool = Some(a.recovery_tool.clone());
            p.success_rate = Some(recovery_rate);
            p.description = format!(
                "Error recovery: {} \u{2192} {} (recovered {}/{} times)",
                a.error_tool, a.recovery_tool, a.count, denom as u32
            );
            p
        })
        .collect()
}

/// Detect efficiency issues at the conversation level (mirrors Go's detectEfficiencyIssues).
pub fn detect_conversation_efficiency_issues(
    experiences: &[CollectedExperience],
    min_freq: u32,
) -> Vec<ConversationPattern> {
    if experiences.is_empty() {
        return Vec::new();
    }

    // Calculate global average duration
    let global_avg_dur: f64 = experiences
        .iter()
        .map(|e| e.experience.duration_ms as f64)
        .sum::<f64>()
        / experiences.len() as f64;

    if global_avg_dur == 0.0 {
        return Vec::new();
    }

    let mut agg: HashMap<String, EffAgg> = HashMap::new();

    // Group by session and check for slow sessions
    let mut sessions: HashMap<&str, Vec<&CollectedExperience>> = HashMap::new();
    for exp in experiences {
        sessions
            .entry(&exp.experience.session_key)
            .or_default()
            .push(exp);
    }

    // 全局平均轮数（每会话经验条数均值）。 修复：效率分公式
    // 分母原为 `2.0 * global_avg_dur / global_avg_dur`（Go 移植丢变量，恒
    // 等于常数 2.0）——实际语义是「2 × 全局平均轮数」；丢变量后轮数被拿
    // 去除以常数，confidence 恒 ≤0.5（轮数 >2 的会话直接 0 分），且描述
    // 文案把时长打进了轮数槽位。
    let global_avg_rounds = experiences.len() as f64 / sessions.len() as f64;

    for exps in sessions.values() {
        let total_dur: i64 = exps.iter().map(|e| e.experience.duration_ms as i64).sum();
        let avg_dur = total_dur as f64 / exps.len() as f64;

        // Only consider sessions with avg duration > 2x global average
        if avg_dur <= 2.0 * global_avg_dur {
            continue;
        }

        let chain: Vec<&str> = exps
            .iter()
            .map(|e| e.experience.tool_name.as_str())
            .collect();
        if chain.len() > 3 {
            continue; // Only simple chains
        }
        let chain_str = chain.join("\u{2192}");
        let fp = pattern_fingerprint("efficiency", &chain_str);
        let timestamp = exps.first().unwrap().experience.timestamp.clone();

        if let Some(a) = agg.get_mut(&fp) {
            a.count += 1;
            a.total_rounds += exps.len() as f64;
            a.total_dur += total_dur;
            if timestamp < a.first_seen {
                a.first_seen = timestamp.clone();
            }
            if timestamp > a.last_seen {
                a.last_seen = timestamp.clone();
            }
        } else {
            agg.insert(
                fp,
                EffAgg {
                    chain: chain_str,
                    count: 1,
                    total_rounds: exps.len() as f64,
                    total_dur,
                    first_seen: timestamp.clone(),
                    last_seen: timestamp,
                },
            );
        }
    }

    agg.into_iter()
        .filter(|(_, a)| a.count >= min_freq)
        .map(|(fp, a)| {
            let actual_rounds = a.total_rounds / a.count as f64;
            // 效率分：相对全局平均轮数的比值（≤2× 全局平均轮数 → 分数线性
            // 衰减，全局平均处 0.5、半程 1.0、2× 处 0）。修复语义见上方
            // global_avg_rounds 注记。
            let eff_score = (1.0 - actual_rounds / (2.0 * global_avg_rounds)).max(0.0);
            let mut p = ConversationPattern::new(ConversationPatternType::EfficiencyIssue, &fp);
            p.frequency = a.count;
            p.confidence = eff_score;
            p.first_seen = a.first_seen;
            p.last_seen = a.last_seen;
            p.tool_chain = Some(a.chain.clone());
            p.avg_rounds = Some(actual_rounds);
            p.avg_duration_ms = Some(a.total_dur / a.count as i64);
            p.efficiency_score = Some(eff_score);
            p.description = format!(
                "Efficiency issue: {} ({:.1} avg rounds vs {:.1} global avg)",
                a.chain, actual_rounds, global_avg_rounds
            );
            p
        })
        .collect()
}

/// Detect success templates at the conversation level (mirrors Go's detectSuccessTemplates).
pub fn detect_conversation_success_templates(
    experiences: &[CollectedExperience],
    min_freq: u32,
) -> Vec<ConversationPattern> {
    if experiences.is_empty() {
        return Vec::new();
    }

    let mut agg: HashMap<String, SuccAgg> = HashMap::new();

    // Group by session, only look at all-success sessions
    let mut sessions: HashMap<&str, Vec<&CollectedExperience>> = HashMap::new();
    for exp in experiences {
        sessions
            .entry(&exp.experience.session_key)
            .or_default()
            .push(exp);
    }

    for exps in sessions.values() {
        let all_success = exps.iter().all(|e| e.experience.success);
        if !all_success {
            continue;
        }

        let chain: Vec<&str> = exps
            .iter()
            .map(|e| e.experience.tool_name.as_str())
            .collect();
        let chain_str = chain.join("\u{2192}");
        let fp = pattern_fingerprint("success", &chain_str);
        let total_dur: i64 = exps.iter().map(|e| e.experience.duration_ms as i64).sum();
        let timestamp = exps.first().unwrap().experience.timestamp.clone();

        if let Some(a) = agg.get_mut(&fp) {
            a.count += 1;
            a.successes += 1;
            a.total_rounds += exps.len() as f64;
            a.total_dur += total_dur;
            if timestamp < a.first_seen {
                a.first_seen = timestamp.clone();
            }
            if timestamp > a.last_seen {
                a.last_seen = timestamp.clone();
            }
        } else {
            agg.insert(
                fp,
                SuccAgg {
                    chain: chain_str,
                    count: 1,
                    successes: 1,
                    total_rounds: exps.len() as f64,
                    total_dur,
                    first_seen: timestamp.clone(),
                    last_seen: timestamp,
                    arg_keys: HashMap::new(),
                },
            );
        }
    }

    agg.into_iter()
        .filter(|(_, a)| a.count >= min_freq)
        .map(|(fp, a)| {
            let success_rate = a.successes as f64 / a.count as f64;
            let actual_rounds = a.total_rounds / a.count as f64;
            let confidence = success_rate.min(1.0);
            let mut p = ConversationPattern::new(ConversationPatternType::SuccessTemplate, &fp);
            p.frequency = a.count;
            p.confidence = confidence;
            p.first_seen = a.first_seen;
            p.last_seen = a.last_seen;
            p.tool_chain = Some(a.chain.clone());
            p.avg_rounds = Some(actual_rounds);
            p.avg_duration_ms = Some(a.total_dur / a.count as i64);
            p.success_rate = Some(success_rate);
            p.description = format!(
                "Success template: {} ({:.1} avg rounds, {:.0}% success)",
                a.chain,
                actual_rounds,
                success_rate * 100.0
            );
            p
        })
        .collect()
}

/// Extract all conversation-level patterns from a set of experiences.
///
/// Mirrors Go's `extractPatterns`. Returns patterns sorted by confidence descending.
pub fn extract_conversation_patterns(
    experiences: &[CollectedExperience],
    min_freq: u32,
) -> Vec<ConversationPattern> {
    if experiences.is_empty() || min_freq == 0 {
        return Vec::new();
    }

    let mut patterns = Vec::new();
    patterns.extend(detect_conversation_tool_chains(experiences, min_freq));
    patterns.extend(detect_conversation_error_recovery(experiences, min_freq));
    patterns.extend(detect_conversation_efficiency_issues(experiences, min_freq));
    patterns.extend(detect_conversation_success_templates(experiences, min_freq));

    // Sort by confidence descending
    patterns.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    patterns
}

#[cfg(test)]
mod tests;

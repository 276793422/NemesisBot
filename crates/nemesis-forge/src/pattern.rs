//! Pattern extraction - detects reusable patterns from experience data.
//!
//! Implements four pattern detectors:
//! - tool_chain: High-frequency tool sequences
//! - error_recovery: Tools that fail then succeed
//! - efficiency_issue: Slow or wasteful tool usage
//! - success_template: Consistently successful tool patterns
//!
//! Also provides `ConversationPattern` with rich metadata for conversation-level
//! pattern detection (mirrors Go's `ConversationPattern` struct), including
//! SHA256 fingerprinting, first_seen/last_seen tracking, arg_keys deduplication,
//! and type-specific fields (tool_chain, avg_rounds, error_tool, recovery_tool,
//! efficiency_score).

use std::collections::HashMap;

use sha2::{Digest, Sha256};

use nemesis_types::utils;

use crate::types::{CollectedExperience, ExperienceStats};

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
// Existing Pattern type (experience-level detection)
// ---------------------------------------------------------------------------

/// A detected pattern with metadata.
#[derive(Debug, Clone)]
pub struct Pattern {
    /// Pattern type identifier.
    pub pattern_type: PatternType,
    /// Frequency of occurrence.
    pub frequency: u32,
    /// Confidence score [0, 1].
    pub confidence: f64,
    /// Human-readable description.
    pub description: String,
    /// Associated tool names.
    pub tools: Vec<String>,
    /// Supporting data (JSON).
    pub data: serde_json::Value,
}

/// Types of detectable patterns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatternType {
    ToolChain,
    ErrorRecovery,
    EfficiencyIssue,
    SuccessTemplate,
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
    successes: u32,
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

    for (_session, exps) in &sessions {
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
pub fn detect_conversation_error_recovery(
    experiences: &[CollectedExperience],
    min_freq: u32,
) -> Vec<ConversationPattern> {
    let mut agg: HashMap<String, RecoveryAgg> = HashMap::new();

    // Look for consecutive experiences: failed tool -> different tool -> success
    let mut sorted: Vec<&CollectedExperience> = experiences.iter().collect();
    sorted.sort_by(|a, b| a.experience.timestamp.cmp(&b.experience.timestamp));

    for window in sorted.windows(2) {
        let prev = &window[0];
        let next = &window[1];

        // Condition: prev failed, next succeeded, different tools, same session
        if prev.experience.success || !next.experience.success {
            continue;
        }
        if prev.experience.tool_name == next.experience.tool_name {
            continue;
        }
        if prev.experience.session_key != next.experience.session_key {
            continue;
        }

        let key = format!(
            "{}:{}",
            prev.experience.tool_name, next.experience.tool_name
        );
        let fp = pattern_fingerprint("error_recovery", &key);

        let timestamp = prev.experience.timestamp.clone();

        if let Some(a) = agg.get_mut(&fp) {
            a.count += 1;
            a.successes += 1;
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
                    successes: 1,
                    error_codes: HashMap::new(),
                    first_seen: timestamp.clone(),
                    last_seen: timestamp,
                },
            );
        }
    }

    agg.into_iter()
        .filter(|(_, a)| a.count >= min_freq)
        .map(|(fp, a)| {
            let recovery_rate = a.successes as f64 / a.count as f64;
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
                a.error_tool, a.recovery_tool, a.successes, a.count
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

    for (_session, exps) in &sessions {
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
            let eff_score =
                (1.0 - actual_rounds / (2.0 * global_avg_dur / global_avg_dur)).max(0.0);
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
                a.chain, actual_rounds, global_avg_dur
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

    for (_session, exps) in &sessions {
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

/// Extract patterns from a set of collected experiences.
pub fn extract_patterns(
    experiences: &[CollectedExperience],
    stats: &ExperienceStats,
) -> Vec<Pattern> {
    let mut patterns = Vec::new();

    patterns.extend(detect_tool_chains(experiences, stats));
    patterns.extend(detect_error_recovery(experiences));
    patterns.extend(detect_efficiency_issues(experiences, stats));
    patterns.extend(detect_success_templates(experiences));

    patterns
}

/// Detect high-frequency tool chains.
fn detect_tool_chains(
    experiences: &[CollectedExperience],
    _stats: &ExperienceStats,
) -> Vec<Pattern> {
    let mut tool_counts: std::collections::HashMap<String, u32> = std::collections::HashMap::new();

    for exp in experiences {
        *tool_counts
            .entry(exp.experience.tool_name.clone())
            .or_insert(0) += 1;
    }

    let total = experiences.len() as f64;
    tool_counts
        .into_iter()
        .filter(|(_, count)| *count >= 3)
        .map(|(tool, count)| {
            let frequency_ratio = count as f64 / total;
            Pattern {
                pattern_type: PatternType::ToolChain,
                frequency: count,
                confidence: frequency_ratio.min(1.0),
                description: format!(
                    "High-frequency tool: {} used {} times ({:.0}%)",
                    tool,
                    count,
                    frequency_ratio * 100.0
                ),
                tools: vec![tool],
                data: serde_json::json!({"frequency_ratio": frequency_ratio}),
            }
        })
        .collect()
}

/// Detect error recovery patterns (tool fails, then succeeds).
fn detect_error_recovery(experiences: &[CollectedExperience]) -> Vec<Pattern> {
    let mut tool_errors: std::collections::HashMap<String, (u32, u32)> =
        std::collections::HashMap::new();

    for exp in experiences {
        let entry = tool_errors
            .entry(exp.experience.tool_name.clone())
            .or_insert((0, 0));
        entry.1 += 1;
        if !exp.experience.success {
            entry.0 += 1;
        }
    }

    tool_errors
        .into_iter()
        .filter(|(_, (errors, total))| *errors > 0 && *total >= 3)
        .map(|(tool, (errors, total))| {
            let error_rate = errors as f64 / total as f64;
            Pattern {
                pattern_type: PatternType::ErrorRecovery,
                frequency: errors,
                confidence: error_rate,
                description: format!(
                    "Error recovery: {} has {:.0}% failure rate ({}/{})",
                    tool,
                    error_rate * 100.0,
                    errors,
                    total
                ),
                tools: vec![tool],
                data: serde_json::json!({"error_rate": error_rate}),
            }
        })
        .collect()
}

/// Detect efficiency issues (slow tools).
fn detect_efficiency_issues(
    experiences: &[CollectedExperience],
    stats: &ExperienceStats,
) -> Vec<Pattern> {
    let avg_duration = stats.avg_duration_ms;

    experiences
        .iter()
        .filter(|e| e.experience.duration_ms as f64 > avg_duration * 2.0)
        .map(|e| {
            let ratio = e.experience.duration_ms as f64 / avg_duration.max(1.0);
            Pattern {
                pattern_type: PatternType::EfficiencyIssue,
                frequency: 1,
                confidence: (ratio - 1.0).min(1.0),
                description: format!(
                    "Slow operation: {} took {}ms (avg: {:.0}ms, {:.1}x slower)",
                    e.experience.tool_name, e.experience.duration_ms, avg_duration, ratio
                ),
                tools: vec![e.experience.tool_name.clone()],
                data: serde_json::json!({
                    "duration_ms": e.experience.duration_ms,
                    "avg_ms": avg_duration,
                    "ratio": ratio,
                }),
            }
        })
        .collect()
}

/// Detect success templates (consistently successful patterns).
fn detect_success_templates(experiences: &[CollectedExperience]) -> Vec<Pattern> {
    let mut tool_success: std::collections::HashMap<String, (u32, u32)> =
        std::collections::HashMap::new();

    for exp in experiences {
        if exp.experience.success {
            let entry = tool_success
                .entry(exp.experience.tool_name.clone())
                .or_insert((0, 0));
            entry.0 += 1;
            entry.1 += 1;
        } else {
            let entry = tool_success
                .entry(exp.experience.tool_name.clone())
                .or_insert((0, 0));
            entry.1 += 1;
        }
    }

    tool_success
        .into_iter()
        .filter(|(_, (successes, total))| *total >= 3 && *successes == *total)
        .map(|(tool, (successes, total))| Pattern {
            pattern_type: PatternType::SuccessTemplate,
            frequency: successes,
            confidence: 1.0,
            description: format!(
                "Perfect success: {} succeeded {}/{} times",
                tool, successes, total
            ),
            tools: vec![tool],
            data: serde_json::json!({"success_rate": 1.0}),
        })
        .collect()
}

#[cfg(test)]
mod tests;

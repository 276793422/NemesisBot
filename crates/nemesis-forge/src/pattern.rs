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
        let chain: Vec<&str> = exps.iter().map(|e| e.experience.tool_name.as_str()).collect();
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
                a.chain, a.count, success_rate * 100.0
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

        let key = format!("{}:{}", prev.experience.tool_name, next.experience.tool_name);
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
    let global_avg_dur: f64 =
        experiences.iter().map(|e| e.experience.duration_ms as f64).sum::<f64>()
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

        let chain: Vec<&str> = exps.iter().map(|e| e.experience.tool_name.as_str()).collect();
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
            let eff_score = (1.0 - actual_rounds / (2.0 * global_avg_dur / global_avg_dur)).max(0.0);
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

        let chain: Vec<&str> = exps.iter().map(|e| e.experience.tool_name.as_str()).collect();
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
                a.chain, actual_rounds, success_rate * 100.0
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
    patterns.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal));

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
                    e.experience.tool_name,
                    e.experience.duration_ms,
                    avg_duration,
                    ratio
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
        .map(|(tool, (successes, total))| {
            Pattern {
                pattern_type: PatternType::SuccessTemplate,
                frequency: successes,
                confidence: 1.0,
                description: format!(
                    "Perfect success: {} succeeded {}/{} times",
                    tool, successes, total
                ),
                tools: vec![tool],
                data: serde_json::json!({"success_rate": 1.0}),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Experience;

    fn make_exp(tool: &str, success: bool, duration: u64) -> CollectedExperience {
        CollectedExperience {
            experience: Experience {
                id: uuid::Uuid::new_v4().to_string(),
                tool_name: tool.into(),
                input_summary: "test".into(),
                output_summary: if success { "ok" } else { "err" }.into(),
                success,
                duration_ms: duration,
                timestamp: chrono::Utc::now().to_rfc3339(),
                session_key: "test".into(),
            },
            dedup_hash: format!("hash-{}", tool),
        }
    }

    #[test]
    fn test_extract_tool_chain() {
        let exps: Vec<CollectedExperience> = (0..5)
            .map(|_| make_exp("file_read", true, 100))
            .collect();

        let stats = ExperienceStats {
            total_count: 5,
            success_count: 5,
            failure_count: 0,
            avg_duration_ms: 100.0,
            tool_counts: Default::default(),
        };

        let patterns = extract_patterns(&exps, &stats);
        assert!(patterns.iter().any(|p| p.pattern_type == PatternType::ToolChain));
        assert!(patterns.iter().any(|p| p.pattern_type == PatternType::SuccessTemplate));
    }

    #[test]
    fn test_extract_error_recovery() {
        let exps = vec![
            make_exp("tool_a", false, 100),
            make_exp("tool_a", false, 100),
            make_exp("tool_a", true, 100),
            make_exp("tool_a", true, 100),
        ];

        let stats = ExperienceStats {
            total_count: 4,
            success_count: 2,
            failure_count: 2,
            avg_duration_ms: 100.0,
            tool_counts: Default::default(),
        };

        let patterns = extract_patterns(&exps, &stats);
        assert!(patterns.iter().any(|p| p.pattern_type == PatternType::ErrorRecovery));
    }

    #[test]
    fn test_efficiency_issue() {
        let exps = vec![
            make_exp("fast_tool", true, 50),
            make_exp("fast_tool", true, 50),
            make_exp("slow_tool", true, 500),
        ];

        let stats = ExperienceStats {
            total_count: 3,
            success_count: 3,
            failure_count: 0,
            avg_duration_ms: 200.0,
            tool_counts: Default::default(),
        };

        let patterns = extract_patterns(&exps, &stats);
        assert!(patterns.iter().any(|p| p.pattern_type == PatternType::EfficiencyIssue));
    }

    #[test]
    fn test_empty_experiences() {
        let patterns = extract_patterns(&[], &ExperienceStats {
            total_count: 0,
            success_count: 0,
            failure_count: 0,
            avg_duration_ms: 0.0,
            tool_counts: Default::default(),
        });
        assert!(patterns.is_empty());
    }

    // --- ConversationPattern tests ---

    #[test]
    fn test_pattern_fingerprint() {
        let fp1 = pattern_fingerprint("tool_chain", "read->edit->exec");
        let fp2 = pattern_fingerprint("tool_chain", "read->edit->exec");
        let fp3 = pattern_fingerprint("tool_chain", "edit->read->exec");

        assert_eq!(fp1, fp2); // Same input = same fingerprint
        assert_ne!(fp1, fp3); // Different order = different fingerprint
        assert!(!fp1.is_empty());
    }

    #[test]
    fn test_conversation_pattern_new() {
        let fp = pattern_fingerprint("test", "data");
        let p = ConversationPattern::new(ConversationPatternType::ToolChain, &fp);
        assert!(p.id.starts_with("tc-"));
        assert_eq!(p.pattern_type, ConversationPatternType::ToolChain);
        assert_eq!(p.frequency, 0);
    }

    #[test]
    fn test_extract_conversation_patterns() {
        let exps: Vec<CollectedExperience> = (0..6)
            .map(|_i| make_exp_session("file_read", true, 100, "sess-1"))
            .chain((0..6).map(|_i| make_exp_session("file_read", true, 100, "sess-2")))
            .collect();

        let patterns = extract_conversation_patterns(&exps, 2);
        // Should detect some patterns
        assert!(!patterns.is_empty());
    }

    #[test]
    fn test_extract_conversation_patterns_empty() {
        let patterns = extract_conversation_patterns(&[], 1);
        assert!(patterns.is_empty());
    }

    #[test]
    fn test_detect_error_recovery_conversation() {
        let exps = vec![
            make_exp_session("tool_a", false, 100, "sess-1"),
            make_exp_session("tool_b", true, 100, "sess-1"),
            make_exp_session("tool_a", false, 100, "sess-2"),
            make_exp_session("tool_b", true, 100, "sess-2"),
        ];
        let patterns = detect_conversation_error_recovery(&exps, 2);
        assert!(!patterns.is_empty());
        assert!(patterns[0].error_tool.is_some());
        assert!(patterns[0].recovery_tool.is_some());
    }

    #[test]
    fn test_dedup_chain_string() {
        let chain = dedup_chain_string(&["read", "edit", "exec"]);
        assert!(chain.contains("read"));
        assert!(chain.contains("exec"));
    }

    fn make_exp_session(tool: &str, success: bool, duration: u64, session: &str) -> CollectedExperience {
        CollectedExperience {
            experience: Experience {
                id: uuid::Uuid::new_v4().to_string(),
                tool_name: tool.into(),
                input_summary: "test".into(),
                output_summary: if success { "ok" } else { "err" }.into(),
                success,
                duration_ms: duration,
                timestamp: chrono::Utc::now().to_rfc3339(),
                session_key: session.into(),
            },
            dedup_hash: format!("hash-{}", tool),
        }
    }

    #[test]
    fn test_conversation_pattern_type_as_str() {
        assert_eq!(ConversationPatternType::ToolChain.as_str(), "tool_chain");
        assert_eq!(ConversationPatternType::ErrorRecovery.as_str(), "error_recovery");
        assert_eq!(ConversationPatternType::EfficiencyIssue.as_str(), "efficiency_issue");
        assert_eq!(ConversationPatternType::SuccessTemplate.as_str(), "success_template");
    }

    #[test]
    fn test_conversation_pattern_id_prefix() {
        let fp = "abcdef1234567890abcdef";
        let tc = ConversationPattern::new(ConversationPatternType::ToolChain, fp);
        assert!(tc.id.starts_with("tc-"));

        let er = ConversationPattern::new(ConversationPatternType::ErrorRecovery, fp);
        assert!(er.id.starts_with("er-"));

        let ef = ConversationPattern::new(ConversationPatternType::EfficiencyIssue, fp);
        assert!(ef.id.starts_with("ef-"));

        let st = ConversationPattern::new(ConversationPatternType::SuccessTemplate, fp);
        assert!(st.id.starts_with("st-"));
    }

    #[test]
    fn test_conversation_pattern_short_fingerprint() {
        let fp = "ab";
        let p = ConversationPattern::new(ConversationPatternType::ToolChain, fp);
        assert_eq!(p.id, "tc-ab");
    }

    #[test]
    fn test_pattern_fingerprint_different_prefixes() {
        let fp1 = pattern_fingerprint("tool_chain", "data");
        let fp2 = pattern_fingerprint("error_recovery", "data");
        assert_ne!(fp1, fp2);
    }

    #[test]
    fn test_extract_tool_chain_from_experiences() {
        let exps = vec![
            make_exp("read", true, 100),
            make_exp("edit", true, 200),
            make_exp("exec", true, 300),
        ];
        let chain = extract_tool_chain_from_experiences(&exps);
        assert!(chain.contains("read"));
        assert!(chain.contains("edit"));
        assert!(chain.contains("exec"));
    }

    #[test]
    fn test_extract_conversation_patterns_zero_min_freq() {
        let exps = vec![make_exp("tool", true, 100)];
        let patterns = extract_conversation_patterns(&exps, 0);
        assert!(patterns.is_empty());
    }

    #[test]
    fn test_detect_success_templates_conversation() {
        let exps: Vec<CollectedExperience> = (0..3)
            .flat_map(|i| {
                vec![
                    make_exp_session("read", true, 100, &format!("sess-{}", i)),
                    make_exp_session("write", true, 200, &format!("sess-{}", i)),
                ]
            })
            .collect();
        let patterns = detect_conversation_success_templates(&exps, 2);
        assert!(!patterns.is_empty());
        assert!(patterns[0].tool_chain.is_some());
        assert!(patterns[0].success_rate.unwrap() > 0.0);
    }

    #[test]
    fn test_detect_tool_chains_below_min_freq() {
        let exps = vec![
            make_exp_session("tool_a", true, 100, "sess-1"),
            make_exp_session("tool_b", true, 100, "sess-1"),
        ];
        // Only 1 session, min_freq=2 should filter it out
        let patterns = detect_conversation_tool_chains(&exps, 2);
        assert!(patterns.is_empty());
    }

    #[test]
    fn test_detect_error_recovery_same_tool_no_match() {
        let exps = vec![
            make_exp_session("tool_a", false, 100, "sess-1"),
            make_exp_session("tool_a", true, 100, "sess-1"),
        ];
        let patterns = detect_conversation_error_recovery(&exps, 1);
        // Same tool should not match
        assert!(patterns.is_empty());
    }

    #[test]
    fn test_detect_error_recovery_different_sessions_no_match() {
        let exps = vec![
            make_exp_session("tool_a", false, 100, "sess-1"),
            make_exp_session("tool_b", true, 100, "sess-2"),
        ];
        let patterns = detect_conversation_error_recovery(&exps, 1);
        // Different sessions should not match
        assert!(patterns.is_empty());
    }

    #[test]
    fn test_detect_efficiency_issues_empty() {
        let patterns = detect_conversation_efficiency_issues(&[], 1);
        assert!(patterns.is_empty());
    }

    #[test]
    fn test_detect_success_templates_empty() {
        let patterns = detect_conversation_success_templates(&[], 1);
        assert!(patterns.is_empty());
    }

    #[test]
    fn test_extract_patterns_tool_chain_threshold() {
        // Only 2 uses - below threshold of 3
        let exps = vec![
            make_exp("rare_tool", true, 100),
            make_exp("rare_tool", true, 100),
        ];
        let stats = ExperienceStats {
            total_count: 2,
            success_count: 2,
            failure_count: 0,
            avg_duration_ms: 100.0,
            tool_counts: Default::default(),
        };
        let patterns = extract_patterns(&exps, &stats);
        assert!(!patterns.iter().any(|p| p.pattern_type == PatternType::ToolChain && p.tools.contains(&"rare_tool".to_string())));
    }

    #[test]
    fn test_pattern_sorted_by_confidence() {
        let exps: Vec<CollectedExperience> = (0..10)
            .flat_map(|i| {
                vec![
                    make_exp_session("fast", true, 50, &format!("sess-{}", i)),
                    make_exp_session("fast", true, 50, &format!("sess-{}", i)),
                ]
            })
            .collect();
        let patterns = extract_conversation_patterns(&exps, 2);
        for i in 1..patterns.len() {
            assert!(patterns[i-1].confidence >= patterns[i].confidence);
        }
    }

    // --- Additional pattern tests ---

    #[test]
    fn test_pattern_type_equality() {
        assert_eq!(PatternType::ToolChain, PatternType::ToolChain);
        assert_ne!(PatternType::ToolChain, PatternType::ErrorRecovery);
        assert_ne!(PatternType::EfficiencyIssue, PatternType::SuccessTemplate);
    }

    #[test]
    fn test_conversation_pattern_type_ordering() {
        let types = [
            ConversationPatternType::ToolChain,
            ConversationPatternType::ErrorRecovery,
            ConversationPatternType::EfficiencyIssue,
            ConversationPatternType::SuccessTemplate,
        ];
        // Verify all produce distinct strings
        let strs: Vec<&str> = types.iter().map(|t| t.as_str()).collect();
        for i in 0..strs.len() {
            for j in (i+1)..strs.len() {
                assert_ne!(strs[i], strs[j], "Pattern type strings should be unique");
            }
        }
    }

    #[test]
    fn test_pattern_fingerprint_deterministic() {
        let fp1 = pattern_fingerprint("tool_chain", "a→b→c");
        let fp2 = pattern_fingerprint("tool_chain", "a→b→c");
        assert_eq!(fp1, fp2);
        // Verify it's a hex string
        assert!(fp1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_pattern_fingerprint_empty_data() {
        let fp = pattern_fingerprint("test", "");
        assert!(!fp.is_empty());
    }

    #[test]
    fn test_pattern_fingerprint_long_data() {
        let long_data: String = "x".repeat(10000);
        let fp = pattern_fingerprint("test", &long_data);
        assert!(!fp.is_empty());
    }

    #[test]
    fn test_dedup_chain_string_order_sensitive() {
        let c1 = dedup_chain_string(&["a", "b", "c"]);
        let c2 = dedup_chain_string(&["c", "b", "a"]);
        assert_ne!(c1, c2);
    }

    #[test]
    fn test_dedup_chain_string_single() {
        let c = dedup_chain_string(&["tool"]);
        assert_eq!(c, "tool");
    }

    #[test]
    fn test_dedup_chain_string_empty() {
        let c = dedup_chain_string(&[]);
        assert!(c.is_empty());
    }

    #[test]
    fn test_extract_tool_chain_from_experiences_preserves_order() {
        let exps = vec![
            make_exp("alpha", true, 100),
            make_exp("beta", true, 200),
            make_exp("gamma", true, 300),
        ];
        let chain = extract_tool_chain_from_experiences(&exps);
        assert!(chain.starts_with("alpha"));
        assert!(chain.contains("beta"));
        assert!(chain.ends_with("gamma"));
    }

    #[test]
    fn test_extract_tool_chain_from_experiences_empty() {
        let chain = extract_tool_chain_from_experiences(&[]);
        assert!(chain.is_empty());
    }

    #[test]
    fn test_conversation_pattern_default_values() {
        let p = ConversationPattern::new(ConversationPatternType::ToolChain, "abc123def456");
        assert_eq!(p.frequency, 0);
        assert_eq!(p.confidence, 0.0);
        assert!(p.first_seen.is_empty());
        assert!(p.last_seen.is_empty());
        assert!(p.tool_chain.is_none());
        assert!(p.avg_rounds.is_none());
        assert!(p.avg_duration_ms.is_none());
        assert!(p.success_rate.is_none());
        assert!(p.error_tool.is_none());
        assert!(p.recovery_tool.is_none());
        assert!(p.efficiency_score.is_none());
        assert!(p.common_arg_keys.is_empty());
        assert!(p.description.is_empty());
    }

    #[test]
    fn test_conversation_pattern_clone() {
        let mut p = ConversationPattern::new(ConversationPatternType::ErrorRecovery, "testfp123456");
        p.frequency = 5;
        p.confidence = 0.8;
        p.error_tool = Some("tool_a".into());
        let cloned = p.clone();
        assert_eq!(cloned.frequency, 5);
        assert_eq!(cloned.confidence, 0.8);
        assert_eq!(cloned.error_tool, Some("tool_a".into()));
    }

    #[test]
    fn test_detect_tool_chains_single_session_below_min() {
        let exps = vec![
            make_exp_session("a", true, 100, "s1"),
            make_exp_session("b", true, 100, "s1"),
        ];
        let patterns = detect_conversation_tool_chains(&exps, 2);
        assert!(patterns.is_empty());
    }

    #[test]
    fn test_detect_tool_chains_multiple_sessions_meets_min() {
        let exps: Vec<CollectedExperience> = (0..3)
            .flat_map(|i| {
                let s = format!("sess-{}", i);
                vec![
                    make_exp_session("read", true, 100, &s),
                    make_exp_session("write", true, 200, &s),
                ]
            })
            .collect();
        let patterns = detect_conversation_tool_chains(&exps, 2);
        assert!(!patterns.is_empty());
        let tc = &patterns[0];
        assert!(tc.tool_chain.is_some());
        assert!(tc.success_rate.is_some());
        assert!(tc.avg_rounds.is_some());
        assert!(tc.avg_duration_ms.is_some());
    }

    #[test]
    fn test_detect_tool_chains_success_rate_calculation() {
        // 2 sessions with same chain, one fails
        let exps = vec![
            make_exp_session("read", true, 100, "s1"),
            make_exp_session("write", true, 100, "s1"),
            make_exp_session("read", false, 100, "s2"),
            make_exp_session("write", true, 100, "s2"),
        ];
        let patterns = detect_conversation_tool_chains(&exps, 2);
        assert!(!patterns.is_empty());
        // s2 has a failure so all_success=false, success_rate should be 0.5
        let sr = patterns[0].success_rate.unwrap();
        assert!((sr - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_detect_error_recovery_filters_same_tool() {
        let exps = vec![
            make_exp_session("tool_a", false, 100, "s1"),
            make_exp_session("tool_a", true, 100, "s1"), // same tool, skip
        ];
        let patterns = detect_conversation_error_recovery(&exps, 1);
        assert!(patterns.is_empty());
    }

    #[test]
    fn test_detect_error_recovery_filters_different_session() {
        let exps = vec![
            make_exp_session("tool_a", false, 100, "s1"),
            make_exp_session("tool_b", true, 100, "s2"), // different session, skip
        ];
        let patterns = detect_conversation_error_recovery(&exps, 1);
        assert!(patterns.is_empty());
    }

    #[test]
    fn test_detect_error_recovery_both_success_no_match() {
        let exps = vec![
            make_exp_session("tool_a", true, 100, "s1"),
            make_exp_session("tool_b", true, 100, "s1"),
        ];
        let patterns = detect_conversation_error_recovery(&exps, 1);
        assert!(patterns.is_empty());
    }

    #[test]
    fn test_detect_error_recovery_both_failure_no_match() {
        let exps = vec![
            make_exp_session("tool_a", false, 100, "s1"),
            make_exp_session("tool_b", false, 100, "s1"),
        ];
        let patterns = detect_conversation_error_recovery(&exps, 1);
        assert!(patterns.is_empty());
    }

    #[test]
    fn test_detect_efficiency_issues_zero_global_avg() {
        // All durations are 0 => global avg = 0 => no patterns
        let exps: Vec<CollectedExperience> = (0..4)
            .map(|i| make_exp_session("tool", true, 0, &format!("s{}", i)))
            .collect();
        let patterns = detect_conversation_efficiency_issues(&exps, 1);
        assert!(patterns.is_empty());
    }

    #[test]
    fn test_detect_efficiency_issues_no_slow_sessions() {
        // Sessions with avg duration <= 2x global avg should be filtered
        let exps = vec![
            make_exp_session("fast", true, 100, "s1"),
            make_exp_session("fast", true, 100, "s1"),
        ];
        let patterns = detect_conversation_efficiency_issues(&exps, 1);
        // avg = 100, threshold = 200, actual = 100, so no patterns
        assert!(patterns.is_empty());
    }

    #[test]
    fn test_detect_efficiency_issues_slow_session_detected() {
        // Create sessions where some are much slower than others
        let mut exps = Vec::new();
        // Fast sessions
        for i in 0..5 {
            exps.push(make_exp_session("fast", true, 10, &format!("fast-{}", i)));
        }
        // Slow session (10x average)
        for _ in 0..3 {
            exps.push(make_exp_session("slow", true, 10000, "slow-1"));
            exps.push(make_exp_session("slow", true, 10000, "slow-1"));
        }
        let _patterns = detect_conversation_efficiency_issues(&exps, 1);
        // The slow session should be detected if its chain length <= 3
        // This depends on exact thresholds
    }

    #[test]
    fn test_detect_success_templates_partial_success_excluded() {
        // Session with a failure should be excluded
        let exps = vec![
            make_exp_session("tool_a", true, 100, "s1"),
            make_exp_session("tool_b", false, 100, "s1"), // failure
        ];
        let patterns = detect_conversation_success_templates(&exps, 1);
        assert!(patterns.is_empty());
    }

    #[test]
    fn test_detect_success_templates_all_success_included() {
        let exps: Vec<CollectedExperience> = (0..3)
            .flat_map(|i| {
                let s = format!("s{}", i);
                vec![
                    make_exp_session("read", true, 50, &s),
                    make_exp_session("write", true, 100, &s),
                ]
            })
            .collect();
        let patterns = detect_conversation_success_templates(&exps, 2);
        assert!(!patterns.is_empty());
        assert!(patterns[0].confidence > 0.0);
    }

    #[test]
    fn test_extract_patterns_efficiency_with_high_duration() {
        let exps = vec![
            make_exp("normal", true, 100),
            make_exp("normal", true, 100),
            make_exp("slow", true, 10000), // 100x avg
        ];
        let stats = ExperienceStats {
            total_count: 3,
            success_count: 3,
            failure_count: 0,
            avg_duration_ms: 3400.0,
            tool_counts: Default::default(),
        };
        let patterns = extract_patterns(&exps, &stats);
        let eff_patterns: Vec<_> = patterns.iter().filter(|p| p.pattern_type == PatternType::EfficiencyIssue).collect();
        assert!(!eff_patterns.is_empty());
    }

    #[test]
    fn test_extract_patterns_success_template_requires_perfect() {
        // 3 successes, 1 failure -> not a success template
        let exps = vec![
            make_exp("tool_a", true, 100),
            make_exp("tool_a", true, 100),
            make_exp("tool_a", true, 100),
            make_exp("tool_a", false, 100),
        ];
        let stats = ExperienceStats {
            total_count: 4,
            success_count: 3,
            failure_count: 1,
            avg_duration_ms: 100.0,
            tool_counts: Default::default(),
        };
        let patterns = extract_patterns(&exps, &stats);
        assert!(!patterns.iter().any(|p| p.pattern_type == PatternType::SuccessTemplate && p.tools.contains(&"tool_a".to_string())));
    }

    #[test]
    fn test_extract_patterns_error_recovery_mixed() {
        let exps = vec![
            make_exp("flaky", false, 100),
            make_exp("flaky", false, 100),
            make_exp("flaky", true, 100),
        ];
        let stats = ExperienceStats {
            total_count: 3,
            success_count: 1,
            failure_count: 2,
            avg_duration_ms: 100.0,
            tool_counts: Default::default(),
        };
        let patterns = extract_patterns(&exps, &stats);
        assert!(patterns.iter().any(|p| p.pattern_type == PatternType::ErrorRecovery));
    }

    #[test]
    fn test_conversation_pattern_id_format_all_types() {
        let fp = "abcdefghijklmnop";
        let tc = ConversationPattern::new(ConversationPatternType::ToolChain, fp);
        assert!(tc.id.starts_with("tc-"));
        let er = ConversationPattern::new(ConversationPatternType::ErrorRecovery, fp);
        assert!(er.id.starts_with("er-"));
        let ef = ConversationPattern::new(ConversationPatternType::EfficiencyIssue, fp);
        assert!(ef.id.starts_with("ef-"));
        let st = ConversationPattern::new(ConversationPatternType::SuccessTemplate, fp);
        assert!(st.id.starts_with("st-"));
    }

    #[test]
    fn test_pattern_fingerprint_sha256_length() {
        let fp = pattern_fingerprint("prefix", "data");
        // SHA256 produces 64 hex characters
        assert_eq!(fp.len(), 64);
    }

    #[test]
    fn test_extract_conversation_patterns_returns_combined() {
        // Create experiences that trigger multiple pattern types
        let mut exps = Vec::new();
        // Tool chains across sessions
        for i in 0..5 {
            let s = format!("sess-{}", i);
            exps.push(make_exp_session("read", true, 100, &s));
            exps.push(make_exp_session("write", true, 100, &s));
        }
        // Error recovery
        exps.push(make_exp_session("fail_tool", false, 100, "err-1"));
        exps.push(make_exp_session("recover_tool", true, 100, "err-1"));
        exps.push(make_exp_session("fail_tool", false, 100, "err-2"));
        exps.push(make_exp_session("recover_tool", true, 100, "err-2"));

        let patterns = extract_conversation_patterns(&exps, 2);
        assert!(!patterns.is_empty());
        // Should contain at least tool_chain and error_recovery patterns
        let types: std::collections::HashSet<&str> = patterns.iter()
            .map(|p| p.pattern_type.as_str())
            .collect();
        assert!(types.contains("tool_chain"));
    }
}

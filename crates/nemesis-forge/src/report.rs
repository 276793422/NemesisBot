//! Report formatter - generates markdown reflection reports.
//!
//! Formats experience statistics, insights, and learning cycle data
//! into human-readable markdown reports. Supports:
//! - Rich reflection reports with top/low-success pattern tables
//! - Phase 5 trace insights (tool chains, retries, signals)
//! - Phase 6 learning cycle insights (patterns, actions, deployment feedback)

use crate::reflector::{ReflectionReport, TraceStats};
#[cfg(test)]
use crate::reflector::{PatternInsight, ReflectionStats, RetryPattern, ToolChainPattern};
use crate::types::ExperienceStats;

use nemesis_types::forge::{Artifact, LearningCycle};

// ---------------------------------------------------------------------------
// Markdown table helper
// ---------------------------------------------------------------------------

/// Escape pipe characters for markdown tables and truncate.
fn truncate(s: &str, max_len: usize) -> String {
    let escaped = s.replace('|', "\\|");
    if escaped.len() <= max_len {
        escaped
    } else {
        format!("{}...", &escaped[..max_len.saturating_sub(3)])
    }
}

// ---------------------------------------------------------------------------
// format_report (rich ReflectionReport-based)
// ---------------------------------------------------------------------------

/// Generate a markdown reflection report from a rich `ReflectionReport`.
pub fn format_report_from_report(report: &ReflectionReport) -> String {
    let mut sb = String::new();

    // Header
    sb.push_str(&format!("# Forge Reflection Report\n\n"));
    sb.push_str(&format!("**Date**: {}\n", report.date));
    sb.push_str(&format!("**Period**: {}\n", report.period));
    sb.push_str(&format!("**Focus**: {}\n\n", report.focus));

    // Statistics section
    let stats = &report.stats;
    sb.push_str("## Statistics\n\n");
    sb.push_str(&format!("- Total records: {}\n", stats.total_records));
    sb.push_str(&format!("- Unique patterns: {}\n", stats.unique_patterns));
    sb.push_str(&format!(
        "- Average success rate: {:.1}%\n\n",
        stats.avg_success_rate * 100.0
    ));

    // Tool frequency
    if !stats.tool_frequency.is_empty() {
        sb.push_str("## Tool Usage Frequency\n\n");
        sb.push_str("| Tool | Count |\n");
        sb.push_str("|------|-------|\n");
        let mut freq: Vec<_> = stats.tool_frequency.iter().collect();
        freq.sort_by(|a, b| b.1.cmp(a.1));
        for (tool, count) in &freq {
            sb.push_str(&format!("| {} | {} |\n", tool, count));
        }
        sb.push('\n');
    }

    // Top patterns (high frequency)
    if !stats.top_patterns.is_empty() {
        sb.push_str("## High Frequency Patterns\n\n");
        sb.push_str("| Tool | Count | Success Rate | Avg Duration | Suggestion |\n");
        sb.push_str("|------|-------|-------------|-------------|------------|\n");
        for p in &stats.top_patterns {
            sb.push_str(&format!(
                "| {} | {} | {:.1}% | {}ms | {} |\n",
                truncate(&p.tool_name, 20),
                p.count,
                p.success_rate * 100.0,
                p.avg_duration_ms,
                truncate(&p.suggestion, 30)
            ));
        }
        sb.push('\n');
    }

    // Low success patterns
    if !stats.low_success.is_empty() {
        sb.push_str("## Low Success Patterns\n\n");
        sb.push_str("| Tool | Count | Success Rate | Suggestion |\n");
        sb.push_str("|------|-------|-------------|------------|\n");
        for p in &stats.low_success {
            sb.push_str(&format!(
                "| {} | {} | {:.1}% | {} |\n",
                truncate(&p.tool_name, 20),
                p.count,
                p.success_rate * 100.0,
                truncate(&p.suggestion, 40)
            ));
        }
        sb.push('\n');
    }

    // LLM deep analysis
    if let Some(ref llm) = report.llm_insights {
        if !llm.is_empty() {
            sb.push_str("## LLM Deep Analysis\n\n");
            sb.push_str(llm);
            sb.push_str("\n\n");
        }
    }

    // Phase 5: Trace insights
    if let Some(ref trace_stats) = report.trace_stats {
        let trace_section = format_trace_insights(trace_stats);
        if !trace_section.is_empty() {
            sb.push_str(&trace_section);
        }
    }

    // Phase 6: Learning cycle
    if let Some(ref cycle) = report.learning_cycle {
        let learning_section = format_learning_insights_full(
            cycle,
            &[], // actions
            &[], // outcomes
        );
        sb.push_str(&learning_section);
    }

    sb
}

// ---------------------------------------------------------------------------
// format_report (simplified ExperienceStats-based, kept for backward compat)
// ---------------------------------------------------------------------------

/// Generate a markdown reflection report from experience stats.
pub fn format_report(
    period_start: &str,
    period_end: &str,
    stats: &ExperienceStats,
    insights: &[String],
    recommendations: &[String],
) -> String {
    let mut report = String::new();

    report.push_str("# Forge Reflection Report\n\n");
    report.push_str(&format!("**Period**: {} to {}\n\n", period_start, period_end));

    // Statistics section
    report.push_str("## Statistics\n\n");
    report.push_str(&format!("- Total tool invocations: {}\n", stats.total_count));
    report.push_str(&format!("- Successful: {}\n", stats.success_count));
    report.push_str(&format!("- Failed: {}\n", stats.failure_count));
    report.push_str(&format!(
        "- Success rate: {:.1}%\n",
        if stats.total_count > 0 {
            stats.success_count as f64 / stats.total_count as f64 * 100.0
        } else {
            0.0
        }
    ));
    report.push_str(&format!("- Average duration: {:.0}ms\n\n", stats.avg_duration_ms));

    // Tool breakdown
    if !stats.tool_counts.is_empty() {
        report.push_str("## Tool Breakdown\n\n");
        report.push_str("| Tool | Count | Success | Avg Duration |\n");
        report.push_str("|------|-------|---------|-------------|\n");
        let mut tools: Vec<_> = stats.tool_counts.iter().collect();
        tools.sort_by(|a, b| b.1.count.cmp(&a.1.count));
        for (tool, ts) in &tools {
            report.push_str(&format!(
                "| {} | {} | {} | {:.0}ms |\n",
                tool, ts.count, ts.success_count, ts.avg_duration_ms
            ));
        }
        report.push('\n');
    }

    // Insights
    if !insights.is_empty() {
        report.push_str("## Insights\n\n");
        for insight in insights {
            report.push_str(&format!("- {}\n", insight));
        }
        report.push('\n');
    }

    // Recommendations
    if !recommendations.is_empty() {
        report.push_str("## Recommendations\n\n");
        for rec in recommendations {
            report.push_str(&format!("- {}\n", rec));
        }
        report.push('\n');
    }

    report
}

// ---------------------------------------------------------------------------
// format_existing_artifacts
// ---------------------------------------------------------------------------

/// Format an existing artifacts table for inclusion in a report.
pub fn format_existing_artifacts(artifacts: &[Artifact]) -> String {
    if artifacts.is_empty() {
        return String::new();
    }

    let mut sb = String::new();
    sb.push_str("## Existing Artifacts\n\n");
    sb.push_str("| Type | Name | Version | Status | Usage Count | Success Rate |\n");
    sb.push_str("|------|------|---------|--------|-------------|-------------|\n");
    for a in artifacts {
        let sr = if a.usage_count > 0 {
            let total = a.usage_count + a.consecutive_observing_rounds as u64;
            format!("{:.0}%", (a.usage_count as f64 / total.max(1) as f64) * 100.0)
        } else {
            "N/A".to_string()
        };
        sb.push_str(&format!(
            "| {:?} | {} | {} | {:?} | {} | {} |\n",
            a.kind, a.name, a.version, a.status, a.usage_count, sr
        ));
    }
    sb.push('\n');
    sb
}

// ---------------------------------------------------------------------------
// format_trace_insights (Phase 5)
// ---------------------------------------------------------------------------

/// Format conversation-level trace insights for inclusion in a report.
pub fn format_trace_insights(stats: &TraceStats) -> String {
    if stats.total_traces == 0
        && stats.tool_chain_patterns.is_empty()
        && stats.retry_patterns.is_empty()
        && stats.signal_summary.is_empty()
    {
        return String::new();
    }

    let mut sb = String::new();
    sb.push_str("## Trace Insights\n\n");
    sb.push_str(&format!("- Total traces: {}\n", stats.total_traces));
    sb.push_str(&format!(
        "- Average rounds: {:.1}\n",
        stats.avg_rounds
    ));
    sb.push_str(&format!(
        "- Average duration: {}ms\n",
        stats.avg_duration_ms
    ));
    sb.push_str(&format!(
        "- Efficiency score: {:.2}\n\n",
        stats.efficiency_score
    ));

    // High-frequency tool chains
    if !stats.tool_chain_patterns.is_empty() {
        sb.push_str("### High-Frequency Tool Chains\n\n");
        sb.push_str("| Chain | Count | Avg Rounds | Success Rate |\n");
        sb.push_str("|-------|-------|-----------|-------------|\n");
        for chain in &stats.tool_chain_patterns {
            sb.push_str(&format!(
                "| {} | {} | {:.1} | {:.1}% |\n",
                truncate(&chain.chain, 30),
                chain.count,
                chain.avg_rounds,
                chain.success_rate * 100.0
            ));
        }
        sb.push('\n');
    }

    // Retry patterns
    if !stats.retry_patterns.is_empty() {
        sb.push_str("### Retry Patterns\n\n");
        sb.push_str("| Tool | Retry Count | Success Rate |\n");
        sb.push_str("|------|------------|-------------|\n");
        for retry in &stats.retry_patterns {
            sb.push_str(&format!(
                "| {} | {} | {:.1}% |\n",
                truncate(&retry.tool_name, 20),
                retry.retry_count,
                retry.success_rate * 100.0
            ));
        }
        sb.push('\n');
    }

    // Session signals
    if !stats.signal_summary.is_empty() {
        sb.push_str("### Session Signals\n\n");
        sb.push_str("| Signal Type | Count |\n");
        sb.push_str("|------------|-------|\n");
        let mut signals: Vec<_> = stats.signal_summary.iter().collect();
        signals.sort_by(|a, b| b.1.cmp(a.1));
        for (signal, count) in &signals {
            sb.push_str(&format!("| {} | {} |\n", signal, count));
        }
        sb.push('\n');
    }

    sb
}

// ---------------------------------------------------------------------------
// format_learning_insights (simple version, backward compat)
// ---------------------------------------------------------------------------

/// Format learning cycle insights for inclusion in a report.
pub fn format_learning_insights(
    patterns_found: u32,
    actions_created: u32,
    actions_executed: u32,
) -> String {
    let mut sb = String::new();
    sb.push_str("## Learning Cycle\n\n");
    sb.push_str(&format!("- Patterns detected: {}\n", patterns_found));
    sb.push_str(&format!("- Actions created: {}\n", actions_created));
    sb.push_str(&format!("- Actions executed: {}\n", actions_executed));
    sb
}

// ---------------------------------------------------------------------------
// format_learning_insights_full (Phase 6)
// ---------------------------------------------------------------------------

/// A learning action for report formatting.
#[derive(Debug, Clone)]
pub struct LearningActionReport {
    pub action_type: String,
    pub priority: String,
    pub status: String,
    pub artifact_id: Option<String>,
}

/// A deployment outcome for report formatting.
#[derive(Debug, Clone)]
pub struct DeploymentOutcomeReport {
    pub artifact_id: String,
    pub verdict: String,
    pub improvement_score: f64,
    pub sample_size: usize,
}

/// Format full learning cycle insights with detailed tables (Phase 6).
pub fn format_learning_insights_full(
    cycle: &LearningCycle,
    actions: &[LearningActionReport],
    _outcomes: &[DeploymentOutcomeReport],
    // Note: patterns info is derived from cycle.patterns_found
) -> String {
    let mut sb = String::new();
    sb.push_str("## Learning Cycle\n\n");

    // Cycle info
    sb.push_str(&format!("- Cycle ID: {}\n", cycle.id));
    sb.push_str(&format!("- Started: {}\n", cycle.started_at));
    if let Some(ref completed) = cycle.completed_at {
        sb.push_str(&format!("- Completed: {}\n", completed));
    }
    sb.push_str(&format!(
        "- Status: {:?}\n",
        cycle.status
    ));
    sb.push_str(&format!(
        "- Patterns found: {}\n",
        cycle.patterns_found
    ));
    sb.push_str(&format!(
        "- Actions taken: {}\n\n",
        cycle.actions_taken
    ));

    // Learning actions table
    if !actions.is_empty() {
        sb.push_str("### Learning Actions\n\n");
        sb.push_str("| Type | Priority | Status | Artifact ID |\n");
        sb.push_str("|------|----------|--------|------------|\n");
        for action in actions {
            let artifact_id = action
                .artifact_id
                .as_deref()
                .unwrap_or("-");
            sb.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                truncate(&action.action_type, 15),
                action.priority,
                action.status,
                truncate(artifact_id, 20)
            ));
        }
        sb.push('\n');
    }

    sb
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_stats() -> ExperienceStats {
        let mut tool_counts = HashMap::new();
        tool_counts.insert(
            "file_read".into(),
            crate::types::ToolStats {
                count: 10,
                success_count: 9,
                avg_duration_ms: 50.0,
            },
        );

        ExperienceStats {
            total_count: 10,
            success_count: 9,
            failure_count: 1,
            avg_duration_ms: 50.0,
            tool_counts,
        }
    }

    #[test]
    fn test_format_report() {
        let stats = make_stats();
        let report = format_report(
            "2026-04-28",
            "2026-04-29",
            &stats,
            &["Tool usage is consistent".into()],
            &["Consider caching file reads".into()],
        );

        assert!(report.contains("# Forge Reflection Report"));
        assert!(report.contains("2026-04-28"));
        assert!(report.contains("file_read"));
        assert!(report.contains("Tool usage is consistent"));
        assert!(report.contains("Consider caching file reads"));
    }

    #[test]
    fn test_format_report_empty_insights() {
        let stats = make_stats();
        let report = format_report("2026-01-01", "2026-01-02", &stats, &[], &[]);
        assert!(!report.contains("## Insights"));
        assert!(!report.contains("## Recommendations"));
    }

    #[test]
    fn test_format_learning_insights() {
        let output = format_learning_insights(5, 3, 2);
        assert!(output.contains("Patterns detected: 5"));
        assert!(output.contains("Actions created: 3"));
        assert!(output.contains("Actions executed: 2"));
    }

    #[test]
    fn test_truncate_short() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_long() {
        let result = truncate("a very long string that needs truncation", 15);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 15);
    }

    #[test]
    fn test_truncate_pipe_escape() {
        assert_eq!(truncate("a|b", 10), "a\\|b");
    }

    #[test]
    fn test_format_report_from_report() {
        let mut tool_freq = HashMap::new();
        tool_freq.insert("file_read".to_string(), 42);
        tool_freq.insert("file_write".to_string(), 15);

        let report = ReflectionReport {
            date: "2026-04-29".to_string(),
            period: "today".to_string(),
            focus: "all".to_string(),
            stats: ReflectionStats {
                total_records: 100,
                unique_patterns: 12,
                avg_success_rate: 0.85,
                top_patterns: vec![PatternInsight {
                    tool_name: "file_read".to_string(),
                    count: 42,
                    avg_duration_ms: 50,
                    success_rate: 0.95,
                    suggestion: "High frequency tool".to_string(),
                }],
                low_success: vec![PatternInsight {
                    tool_name: "exec".to_string(),
                    count: 5,
                    avg_duration_ms: 200,
                    success_rate: 0.2,
                    suggestion: "Consider safer alternative".to_string(),
                }],
                tool_frequency: tool_freq,
            },
            llm_insights: Some("LLM analysis suggests optimizing file reads".to_string()),
            trace_stats: None,
            learning_cycle: None,
        };

        let output = format_report_from_report(&report);
        assert!(output.contains("# Forge Reflection Report"));
        assert!(output.contains("2026-04-29"));
        assert!(output.contains("100"));
        assert!(output.contains("12"));
        assert!(output.contains("85.0%"));
        assert!(output.contains("High Frequency Patterns"));
        assert!(output.contains("file_read"));
        assert!(output.contains("Low Success Patterns"));
        assert!(output.contains("exec"));
        assert!(output.contains("LLM Deep Analysis"));
        assert!(output.contains("Tool Usage Frequency"));
    }

    #[test]
    fn test_format_trace_insights() {
        let stats = TraceStats {
            total_traces: 50,
            avg_rounds: 3.5,
            avg_duration_ms: 1200,
            efficiency_score: 0.78,
            tool_chain_patterns: vec![ToolChainPattern {
                chain: "read_file->edit_file".to_string(),
                count: 15,
                avg_rounds: 2.5,
                success_rate: 0.9,
            }],
            retry_patterns: vec![RetryPattern {
                tool_name: "exec".to_string(),
                retry_count: 3,
                success_rate: 0.67,
            }],
            signal_summary: {
                let mut m = HashMap::new();
                m.insert("retry".to_string(), 5);
                m.insert("backtrack".to_string(), 2);
                m
            },
        };

        let output = format_trace_insights(&stats);
        assert!(output.contains("## Trace Insights"));
        assert!(output.contains("50"));
        assert!(output.contains("3.5"));
        assert!(output.contains("High-Frequency Tool Chains"));
        assert!(output.contains("read_file->edit_file"));
        assert!(output.contains("Retry Patterns"));
        assert!(output.contains("exec"));
        assert!(output.contains("Session Signals"));
        assert!(output.contains("retry"));
    }

    #[test]
    fn test_format_trace_insights_empty() {
        let stats = TraceStats::default();
        let output = format_trace_insights(&stats);
        assert!(output.is_empty());
    }

    #[test]
    fn test_format_learning_insights_full() {
        let cycle = LearningCycle {
            id: "cycle-001".to_string(),
            started_at: "2026-04-29T10:00:00Z".to_string(),
            completed_at: Some("2026-04-29T10:05:00Z".to_string()),
            patterns_found: 3,
            actions_taken: 2,
            status: nemesis_types::forge::CycleStatus::Completed,
        };

        let actions = vec![LearningActionReport {
            action_type: "create_skill".to_string(),
            priority: "high".to_string(),
            status: "executed".to_string(),
            artifact_id: Some("skill-auto-1".to_string()),
        }];

        let output = format_learning_insights_full(&cycle, &actions, &[]);
        assert!(output.contains("## Learning Cycle"));
        assert!(output.contains("cycle-001"));
        assert!(output.contains("Patterns found: 3"));
        assert!(output.contains("### Learning Actions"));
        assert!(output.contains("create_skill"));
        assert!(output.contains("skill-auto-1"));
    }

    #[test]
    fn test_format_existing_artifacts() {
        use nemesis_types::forge::{ArtifactKind, ArtifactStatus};

        let artifacts = vec![Artifact {
            id: "skill-1".into(),
            name: "my-skill".into(),
            kind: ArtifactKind::Skill,
            version: "1.0".into(),
            status: ArtifactStatus::Active,
            content: String::new(),
            tool_signature: vec![],
            created_at: "2026-04-29T00:00:00Z".into(),
            updated_at: "2026-04-29T00:00:00Z".into(),
            usage_count: 10,
            last_degraded_at: None,
            success_rate: 0.0,
            consecutive_observing_rounds: 0,
        }];

        let output = format_existing_artifacts(&artifacts);
        assert!(output.contains("## Existing Artifacts"));
        assert!(output.contains("my-skill"));
        assert!(output.contains("Skill"));
    }

    #[test]
    fn test_format_existing_artifacts_empty() {
        let output = format_existing_artifacts(&[]);
        assert!(output.is_empty());
    }

    // --- Additional report tests ---

    #[test]
    fn test_format_report_zero_stats() {
        let stats = ExperienceStats {
            total_count: 0,
            success_count: 0,
            failure_count: 0,
            avg_duration_ms: 0.0,
            tool_counts: HashMap::new(),
        };
        let report = format_report("2026-01-01", "2026-01-02", &stats, &[], &[]);
        assert!(report.contains("0.0%"));
        assert!(report.contains("0"));
    }

    #[test]
    fn test_format_report_with_multiple_insights() {
        let stats = make_stats();
        let insights = vec!["Insight 1".into(), "Insight 2".into(), "Insight 3".into()];
        let recs = vec!["Rec 1".into(), "Rec 2".into()];
        let report = format_report("2026-01-01", "2026-01-02", &stats, &insights, &recs);
        assert!(report.contains("Insight 1"));
        assert!(report.contains("Insight 2"));
        assert!(report.contains("Insight 3"));
        assert!(report.contains("Rec 1"));
        assert!(report.contains("Rec 2"));
    }

    #[test]
    fn test_truncate_exact_length() {
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_zero_length() {
        let result = truncate("hello", 0);
        // saturating_sub(3) = 0, so empty string + "..."
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_format_report_from_report_no_llm() {
        let report = ReflectionReport {
            date: "2026-05-01".into(),
            period: "today".into(),
            focus: "all".into(),
            stats: ReflectionStats::default(),
            llm_insights: None,
            trace_stats: None,
            learning_cycle: None,
        };
        let output = format_report_from_report(&report);
        assert!(output.contains("# Forge Reflection Report"));
        assert!(!output.contains("LLM Deep Analysis"));
    }

    #[test]
    fn test_format_report_from_report_empty_llm() {
        let report = ReflectionReport {
            date: "2026-05-01".into(),
            period: "today".into(),
            focus: "all".into(),
            stats: ReflectionStats::default(),
            llm_insights: Some(String::new()),
            trace_stats: None,
            learning_cycle: None,
        };
        let output = format_report_from_report(&report);
        assert!(!output.contains("LLM Deep Analysis"));
    }

    #[test]
    fn test_format_trace_insights_only_signals() {
        let mut signals = HashMap::new();
        signals.insert("retry".to_string(), 3);
        let stats = TraceStats {
            total_traces: 0,
            avg_rounds: 0.0,
            avg_duration_ms: 0,
            efficiency_score: 0.0,
            tool_chain_patterns: vec![],
            retry_patterns: vec![],
            signal_summary: signals,
        };
        let output = format_trace_insights(&stats);
        assert!(output.contains("## Trace Insights"));
        assert!(output.contains("retry"));
    }

    #[test]
    fn test_format_learning_insights_full_no_completed() {
        let cycle = LearningCycle {
            id: "lc-nc".into(),
            started_at: "2026-05-01T00:00:00Z".into(),
            completed_at: None,
            patterns_found: 1,
            actions_taken: 0,
            status: nemesis_types::forge::CycleStatus::Running,
        };
        let output = format_learning_insights_full(&cycle, &[], &[]);
        assert!(output.contains("lc-nc"));
        assert!(!output.contains("Completed:"));
    }

    #[test]
    fn test_format_learning_insights_full_no_artifact_id() {
        let cycle = LearningCycle {
            id: "lc-na".into(),
            started_at: "2026-05-01T00:00:00Z".into(),
            completed_at: None,
            patterns_found: 0,
            actions_taken: 0,
            status: nemesis_types::forge::CycleStatus::Running,
        };
        let actions = vec![LearningActionReport {
            action_type: "suggest".into(),
            priority: "low".into(),
            status: "pending".into(),
            artifact_id: None,
        }];
        let output = format_learning_insights_full(&cycle, &actions, &[]);
        assert!(output.contains("-"));
        assert!(output.contains("suggest"));
    }

    #[test]
    fn test_format_existing_artifacts_multiple() {
        use nemesis_types::forge::{ArtifactKind, ArtifactStatus};
        let artifacts = vec![
            Artifact {
                id: "a1".into(), name: "skill-a".into(), kind: ArtifactKind::Skill,
                version: "1.0".into(), status: ArtifactStatus::Active,
                content: String::new(), tool_signature: vec![],
                created_at: "2026-05-01T00:00:00Z".into(), updated_at: "2026-05-01T00:00:00Z".into(),
                usage_count: 5, last_degraded_at: None, success_rate: 0.0, consecutive_observing_rounds: 0,
            },
            Artifact {
                id: "a2".into(), name: "script-b".into(), kind: ArtifactKind::Script,
                version: "2.0".into(), status: ArtifactStatus::Draft,
                content: String::new(), tool_signature: vec![],
                created_at: "2026-05-01T00:00:00Z".into(), updated_at: "2026-05-01T00:00:00Z".into(),
                usage_count: 0, last_degraded_at: None, success_rate: 0.0, consecutive_observing_rounds: 0,
            },
        ];
        let output = format_existing_artifacts(&artifacts);
        assert!(output.contains("skill-a"));
        assert!(output.contains("script-b"));
        assert!(output.contains("N/A")); // zero usage_count
    }

    #[test]
    fn test_format_report_success_rate_calculation() {
        let stats = ExperienceStats {
            total_count: 3,
            success_count: 1,
            failure_count: 2,
            avg_duration_ms: 100.0,
            tool_counts: HashMap::new(),
        };
        let report = format_report("2026-01-01", "2026-01-02", &stats, &[], &[]);
        assert!(report.contains("33.3%"));
    }

    #[test]
    fn test_format_report_from_report_with_trace() {
        let report = ReflectionReport {
            date: "2026-05-01".into(),
            period: "today".into(),
            focus: "all".into(),
            stats: ReflectionStats::default(),
            llm_insights: None,
            trace_stats: Some(TraceStats {
                total_traces: 5,
                avg_rounds: 2.0,
                avg_duration_ms: 300,
                efficiency_score: 0.5,
                tool_chain_patterns: vec![],
                retry_patterns: vec![],
                signal_summary: HashMap::new(),
            }),
            learning_cycle: None,
        };
        let output = format_report_from_report(&report);
        assert!(output.contains("Trace Insights"));
        assert!(output.contains("5"));
    }

    #[test]
    fn test_format_report_from_report_with_learning_cycle() {
        let report = ReflectionReport {
            date: "2026-05-01".into(),
            period: "today".into(),
            focus: "all".into(),
            stats: ReflectionStats::default(),
            llm_insights: None,
            trace_stats: None,
            learning_cycle: Some(LearningCycle {
                id: "lc-test".into(),
                started_at: "2026-05-01T00:00:00Z".into(),
                completed_at: Some("2026-05-01T01:00:00Z".into()),
                patterns_found: 7,
                actions_taken: 3,
                status: nemesis_types::forge::CycleStatus::Completed,
            }),
        };
        let output = format_report_from_report(&report);
        assert!(output.contains("Learning Cycle"));
        assert!(output.contains("lc-test"));
        assert!(output.contains("Patterns found: 7"));
    }
}

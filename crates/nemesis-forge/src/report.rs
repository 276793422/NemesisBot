//! Report formatter - generates markdown reflection reports.
//!
//! Formats experience statistics, insights, and learning cycle data
//! into human-readable markdown reports. Supports:
//! - Rich reflection reports with top/low-success pattern tables
//! - Phase 5 trace insights (tool chains, retries, signals)
//! - Phase 6 learning cycle insights (patterns, actions, deployment feedback)

#[cfg(test)]
use crate::reflector::{PatternInsight, ReflectionStats, RetryPattern, ToolChainPattern};
use crate::reflector::{ReflectionReport, TraceStats};
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
    report.push_str(&format!(
        "**Period**: {} to {}\n\n",
        period_start, period_end
    ));

    // Statistics section
    report.push_str("## Statistics\n\n");
    report.push_str(&format!(
        "- Total tool invocations: {}\n",
        stats.total_count
    ));
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
    report.push_str(&format!(
        "- Average duration: {:.0}ms\n\n",
        stats.avg_duration_ms
    ));

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
            format!(
                "{:.0}%",
                (a.usage_count as f64 / total.max(1) as f64) * 100.0
            )
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
    sb.push_str(&format!("- Average rounds: {:.1}\n", stats.avg_rounds));
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
    sb.push_str(&format!("- Status: {:?}\n", cycle.status));
    sb.push_str(&format!("- Patterns found: {}\n", cycle.patterns_found));
    sb.push_str(&format!("- Actions taken: {}\n\n", cycle.actions_taken));

    // Learning actions table
    if !actions.is_empty() {
        sb.push_str("### Learning Actions\n\n");
        sb.push_str("| Type | Priority | Status | Artifact ID |\n");
        sb.push_str("|------|----------|--------|------------|\n");
        for action in actions {
            let artifact_id = action.artifact_id.as_deref().unwrap_or("-");
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
mod tests;

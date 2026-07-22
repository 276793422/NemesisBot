//! Reflection LLM integration - semantic analysis via LLM.
//!
//! Provides LLM-powered deep analysis of statistical data, generating
//! insights, suggestions, and extracting structured data from responses.

use async_trait::async_trait;

use crate::reflector::{ReflectionStats, TraceStats};
use crate::types::{Artifact, ExperienceStats};

/// Trait for making LLM calls within the forge module.
///
/// This avoids a direct dependency on nemesis-providers. The integration
/// layer (bot_service) provides the concrete implementation.
#[async_trait]
pub trait LLMCaller: Send + Sync {
    /// Send a chat message to the LLM and return the text response.
    async fn chat(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        max_tokens: Option<i64>,
    ) -> Result<String, String>;
}

/// Build an LLM prompt from reflection statistics for semantic analysis.
pub fn build_analysis_prompt(stats: &ExperienceStats, total_tools: usize) -> String {
    let mut sb = String::new();
    sb.push_str("Analyze the following tool usage data from an AI agent system:\n\n");

    sb.push_str(&format!(
        "- Total tool invocations: {}\n",
        stats.total_count
    ));
    sb.push_str(&format!("- Unique patterns: {}\n", total_tools));
    sb.push_str(&format!(
        "- Average success rate: {:.1}%\n\n",
        if stats.total_count > 0 {
            stats.success_count as f64 / stats.total_count as f64 * 100.0
        } else {
            0.0
        }
    ));

    sb.push_str("## Tool Frequency\n");
    for (tool, ts) in &stats.tool_counts {
        sb.push_str(&format!("- {}: {} uses\n", tool, ts.count));
    }

    sb.push_str("\nPlease provide:\n");
    sb.push_str("1. Key patterns that could become reusable Skills\n");
    sb.push_str("2. Areas for improvement\n");
    sb.push_str("3. Optimization suggestions\n");

    sb
}

/// Build the full analysis prompt including all stages (matches Go semanticAnalysis).
///
/// This is the comprehensive prompt builder that includes:
/// - Statistical summary (tool frequency, success rates)
/// - High-frequency and low-success patterns
/// - Existing artifacts
/// - Phase 5: conversation-level trace insights
/// - Phase 6: closed-loop learning state
pub fn build_full_analysis_prompt(
    stats: &ReflectionStats,
    artifacts: &[Artifact],
    trace_stats: Option<&TraceStats>,
    cycle: Option<&nemesis_types::forge::LearningCycle>,
) -> String {
    let mut sb = String::new();
    sb.push_str(
        "Analyze the following tool usage data from an AI agent system and provide insights:\n\n",
    );

    // Statistical Summary
    sb.push_str("## Statistical Summary\n");
    sb.push_str(&format!(
        "- Total tool invocations: {}\n",
        stats.total_records
    ));
    sb.push_str(&format!("- Unique patterns: {}\n", stats.unique_patterns));
    sb.push_str(&format!(
        "- Average success rate: {:.1}%\n\n",
        stats.avg_success_rate * 100.0
    ));

    // Tool Frequency
    sb.push_str("## Tool Frequency\n");
    for (tool, count) in &stats.tool_frequency {
        sb.push_str(&format!("- {}: {} uses\n", tool, count));
    }

    // High-Frequency Patterns
    sb.push_str("\n## High-Frequency Patterns\n");
    for (i, p) in stats.top_patterns.iter().enumerate() {
        if i >= 5 {
            break;
        }
        sb.push_str(&format!(
            "- {}: {} uses, {:.0}% success, avg {}ms\n",
            p.tool_name,
            p.count,
            p.success_rate * 100.0,
            p.avg_duration_ms
        ));
    }

    // Low Success Patterns
    if !stats.low_success.is_empty() {
        sb.push_str("\n## Low Success Patterns\n");
        for p in &stats.low_success {
            sb.push_str(&format!(
                "- {}: {} uses, {:.0}% success\n",
                p.tool_name,
                p.count,
                p.success_rate * 100.0
            ));
        }
    }

    // Existing Artifacts
    sb.push_str("\n## Existing Forge Artifacts\n");
    for a in artifacts {
        sb.push_str(&format!(
            "- [{:?}] {} v{} ({:?}, {} uses)\n",
            a.kind, a.name, a.version, a.status, a.usage_count
        ));
    }

    // Phase 5: Conversation-level trace insights
    if let Some(ts) = trace_stats {
        sb.push_str("\n## Conversation-Level Trace Insights\n");
        sb.push_str(&format!("- Total conversations: {}\n", ts.total_traces));
        sb.push_str(&format!(
            "- Average LLM rounds per conversation: {:.1}\n",
            ts.avg_rounds
        ));
        sb.push_str(&format!(
            "- Efficiency score: {:.2} (tool steps per round)\n",
            ts.efficiency_score
        ));

        if !ts.tool_chain_patterns.is_empty() {
            sb.push_str("\n### Top Tool Chains\n");
            for p in &ts.tool_chain_patterns {
                sb.push_str(&format!(
                    "- {}: {} uses, {:.1} avg rounds, {:.0}% success\n",
                    p.chain,
                    p.count,
                    p.avg_rounds,
                    p.success_rate * 100.0
                ));
            }
        }

        if !ts.retry_patterns.is_empty() {
            sb.push_str("\n### Retry Patterns\n");
            for p in &ts.retry_patterns {
                sb.push_str(&format!(
                    "- {}: {} calls, {:.0}% success rate\n",
                    p.tool_name,
                    p.retry_count,
                    p.success_rate * 100.0
                ));
            }
        }

        if !ts.signal_summary.is_empty() {
            sb.push_str("\n### Session Signals\n");
            for (sig_type, count) in &ts.signal_summary {
                sb.push_str(&format!("- {}: {} occurrences\n", sig_type, count));
            }
        }
    }

    // Phase 6: Closed-loop learning state
    if let Some(cycle) = cycle {
        sb.push_str("\n## Closed-Loop Learning State (Phase 6)\n");
        sb.push_str(&format!("- Patterns detected: {}\n", cycle.patterns_found));
        sb.push_str(&format!("- Actions taken: {}\n", cycle.actions_taken));
    }

    sb.push_str("\nPlease provide:\n");
    sb.push_str("1. Key patterns that could become reusable Skills or scripts\n");
    sb.push_str("2. Areas for improvement in the agent's tool usage\n");
    sb.push_str("3. Suggestions for optimizing high-frequency operations\n");

    sb
}

/// Perform semantic analysis by calling the LLM.
///
/// This is the core function that was missing in Rust. It builds the full
/// analysis prompt (including all phases) and calls the LLM provider.
pub async fn semantic_analysis(
    caller: &dyn LLMCaller,
    stats: &ReflectionStats,
    artifacts: &[Artifact],
    trace_stats: Option<&TraceStats>,
    cycle: Option<&nemesis_types::forge::LearningCycle>,
    max_tokens: Option<i64>,
) -> Result<String, String> {
    let user_prompt = build_full_analysis_prompt(stats, artifacts, trace_stats, cycle);

    let system_prompt = "You are an AI system analyst. Analyze tool usage data and provide concise, actionable insights. \
        Focus on identifying patterns that could be automated, improved, or turned into reusable components. \
        Keep your response under 500 words.";

    caller.chat(system_prompt, &user_prompt, max_tokens).await
}

/// Parse bullet-point insights from an LLM response.
pub fn parse_insights(response: &str) -> Vec<String> {
    let mut insights = Vec::new();
    for line in response.lines() {
        let line = line.trim();
        if line.starts_with("- ") || line.starts_with("* ") || line.starts_with("• ") {
            let insight = line
                .trim_start_matches("- ")
                .trim_start_matches("* ")
                .trim_start_matches("• ")
                .to_string();
            if !insight.is_empty() {
                insights.push(insight);
            }
        }
    }
    insights
}

/// Attempt to extract JSON from an LLM response.
pub fn extract_json(response: &str) -> Option<serde_json::Value> {
    let start = response.find('{')?;
    let end = response.rfind('}')?;
    if end > start {
        serde_json::from_str(&response[start..=end]).ok()
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// ProviderBridge: wraps LlmProvider into the LLMCaller interface
// ---------------------------------------------------------------------------

/// Provider bridge note:
/// The concrete ProviderBridge adapter is defined in `nemesisbot/src/commands/gateway.rs`
/// because it needs access to `nemesis_providers::router::LLMProvider` which is only
/// available at the binary crate level. The adapter wraps the provider and implements
/// this `LLMCaller` trait.
///
/// Mirrors Go's `forgeInstance.SetProvider(s.provider)`.

#[cfg(test)]
mod tests;

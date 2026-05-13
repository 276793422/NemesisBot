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
pub fn build_analysis_prompt(
    stats: &ExperienceStats,
    total_tools: usize,
) -> String {
    let mut sb = String::new();
    sb.push_str("Analyze the following tool usage data from an AI agent system:\n\n");

    sb.push_str(&format!("- Total tool invocations: {}\n", stats.total_count));
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
    sb.push_str(&format!(
        "- Unique patterns: {}\n",
        stats.unique_patterns
    ));
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
            p.tool_name, p.count, p.success_rate * 100.0, p.avg_duration_ms
        ));
    }

    // Low Success Patterns
    if !stats.low_success.is_empty() {
        sb.push_str("\n## Low Success Patterns\n");
        for p in &stats.low_success {
            sb.push_str(&format!(
                "- {}: {} uses, {:.0}% success\n",
                p.tool_name, p.count, p.success_rate * 100.0
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
                    p.chain, p.count, p.avg_rounds, p.success_rate * 100.0
                ));
            }
        }

        if !ts.retry_patterns.is_empty() {
            sb.push_str("\n### Retry Patterns\n");
            for p in &ts.retry_patterns {
                sb.push_str(&format!(
                    "- {}: {} calls, {:.0}% success rate\n",
                    p.tool_name, p.retry_count, p.success_rate * 100.0
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
        sb.push_str(&format!(
            "- Actions taken: {}\n",
            cycle.actions_taken
        ));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reflector::{PatternInsight, RetryPattern, ToolChainPattern};
    use std::collections::HashMap;

    /// A mock LLM caller that echoes back the user prompt.
    struct MockLLMCaller;

    #[async_trait]
    impl LLMCaller for MockLLMCaller {
        async fn chat(
            &self,
            _system_prompt: &str,
            user_prompt: &str,
            _max_tokens: Option<i64>,
        ) -> Result<String, String> {
            Ok(format!("Mock LLM response for: {}", user_prompt.len()))
        }
    }

    /// A mock LLM caller that returns bullet-point insights.
    struct InsightMockLLMCaller;

    #[async_trait]
    impl LLMCaller for InsightMockLLMCaller {
        async fn chat(
            &self,
            _system_prompt: &str,
            _user_prompt: &str,
            _max_tokens: Option<i64>,
        ) -> Result<String, String> {
            Ok("- First insight\n- Second insight\n- Third insight".to_string())
        }
    }

    /// A mock LLM caller that returns an error.
    struct ErrorMockLLMCaller;

    #[async_trait]
    impl LLMCaller for ErrorMockLLMCaller {
        async fn chat(
            &self,
            _system_prompt: &str,
            _user_prompt: &str,
            _max_tokens: Option<i64>,
        ) -> Result<String, String> {
            Err("LLM service unavailable".to_string())
        }
    }

    fn make_stats() -> ExperienceStats {
        ExperienceStats {
            total_count: 100,
            success_count: 85,
            failure_count: 15,
            avg_duration_ms: 250.0,
            tool_counts: {
                let mut m = HashMap::new();
                m.insert(
                    "file_read".into(),
                    crate::types::ToolStats {
                        count: 50,
                        success_count: 48,
                        avg_duration_ms: 100.0,
                    },
                );
                m
            },
        }
    }

    fn make_reflection_stats() -> ReflectionStats {
        ReflectionStats {
            total_records: 200,
            unique_patterns: 15,
            avg_success_rate: 0.85,
            top_patterns: vec![PatternInsight {
                tool_name: "file_read".to_string(),
                count: 100,
                avg_duration_ms: 120,
                success_rate: 0.95,
                suggestion: "High frequency pattern".to_string(),
            }],
            low_success: vec![PatternInsight {
                tool_name: "flaky_tool".to_string(),
                count: 20,
                avg_duration_ms: 500,
                success_rate: 0.3,
                suggestion: "Investigate failures".to_string(),
            }],
            tool_frequency: {
                let mut m = HashMap::new();
                m.insert("file_read".to_string(), 100);
                m.insert("file_write".to_string(), 50);
                m
            },
        }
    }

    fn make_trace_stats() -> TraceStats {
        TraceStats {
            total_traces: 50,
            avg_rounds: 3.5,
            avg_duration_ms: 2500,
            efficiency_score: 0.75,
            tool_chain_patterns: vec![ToolChainPattern {
                chain: "read->edit->exec".to_string(),
                count: 15,
                avg_rounds: 3.0,
                success_rate: 0.8,
            }],
            retry_patterns: vec![RetryPattern {
                tool_name: "exec".to_string(),
                retry_count: 5,
                success_rate: 0.6,
            }],
            signal_summary: {
                let mut m = HashMap::new();
                m.insert("retry".to_string(), 10);
                m.insert("backtrack".to_string(), 3);
                m
            },
        }
    }

    #[test]
    fn test_build_analysis_prompt() {
        let stats = make_stats();
        let prompt = build_analysis_prompt(&stats, 5);
        assert!(prompt.contains("100"));
        assert!(prompt.contains("file_read"));
        assert!(prompt.contains("85.0%"));
    }

    #[test]
    fn test_build_full_analysis_prompt_basic() {
        let stats = make_reflection_stats();
        let prompt = build_full_analysis_prompt(&stats, &[], None, None);
        assert!(prompt.contains("200"));
        assert!(prompt.contains("15"));
        assert!(prompt.contains("85.0%"));
        assert!(prompt.contains("file_read"));
        assert!(prompt.contains("flaky_tool"));
        assert!(prompt.contains("Low Success Patterns"));
    }

    #[test]
    fn test_build_full_analysis_prompt_with_artifacts() {
        let stats = make_reflection_stats();
        let artifacts = vec![Artifact {
            id: "art-1".to_string(),
            name: "test-skill".to_string(),
            kind: nemesis_types::forge::ArtifactKind::Skill,
            version: "1.0".to_string(),
            status: nemesis_types::forge::ArtifactStatus::Active,
            content: "skill content".to_string(),
            tool_signature: vec!["file_read".to_string()],
            usage_count: 25,
            last_degraded_at: None,
            success_rate: 0.0,
            consecutive_observing_rounds: 0,
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        }];
        let prompt = build_full_analysis_prompt(&stats, &artifacts, None, None);
        assert!(prompt.contains("test-skill"));
        assert!(prompt.contains("Skill"));
        assert!(prompt.contains("1.0"));
    }

    #[test]
    fn test_build_full_analysis_prompt_with_trace_stats() {
        let stats = make_reflection_stats();
        let trace = make_trace_stats();
        let prompt = build_full_analysis_prompt(&stats, &[], Some(&trace), None);
        assert!(prompt.contains("Conversation-Level Trace Insights"));
        assert!(prompt.contains("50"));
        assert!(prompt.contains("3.5"));
        assert!(prompt.contains("read->edit->exec"));
        assert!(prompt.contains("Retry Patterns"));
        assert!(prompt.contains("Session Signals"));
        assert!(prompt.contains("retry"));
    }

    #[test]
    fn test_build_full_analysis_prompt_with_learning_cycle() {
        let stats = make_reflection_stats();
        let cycle = nemesis_types::forge::LearningCycle {
            id: "lc-1".to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
            completed_at: None,
            patterns_found: 5,
            actions_taken: 3,
            status: nemesis_types::forge::CycleStatus::Completed,
        };
        let prompt = build_full_analysis_prompt(&stats, &[], None, Some(&cycle));
        assert!(prompt.contains("Closed-Loop Learning State"));
        assert!(prompt.contains("5"));
        assert!(prompt.contains("3"));
    }

    #[tokio::test]
    async fn test_semantic_analysis_success() {
        let caller = MockLLMCaller;
        let stats = make_reflection_stats();
        let result = semantic_analysis(&caller, &stats, &[], None, None, Some(500)).await;
        assert!(result.is_ok());
        let response = result.unwrap();
        assert!(response.contains("Mock LLM response"));
    }

    #[tokio::test]
    async fn test_semantic_analysis_with_insights() {
        let caller = InsightMockLLMCaller;
        let stats = make_reflection_stats();
        let result = semantic_analysis(&caller, &stats, &[], None, None, None).await;
        let response = result.unwrap();
        let insights = parse_insights(&response);
        assert_eq!(insights.len(), 3);
        assert_eq!(insights[0], "First insight");
    }

    #[tokio::test]
    async fn test_semantic_analysis_error() {
        let caller = ErrorMockLLMCaller;
        let stats = make_reflection_stats();
        let result = semantic_analysis(&caller, &stats, &[], None, None, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unavailable"));
    }

    #[tokio::test]
    async fn test_semantic_analysis_with_all_contexts() {
        let caller = MockLLMCaller;
        let stats = make_reflection_stats();
        let trace = make_trace_stats();
        let cycle = nemesis_types::forge::LearningCycle {
            id: "lc-2".to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
            completed_at: None,
            patterns_found: 2,
            actions_taken: 1,
            status: nemesis_types::forge::CycleStatus::Running,
        };
        let result = semantic_analysis(
            &caller,
            &stats,
            &[],
            Some(&trace),
            Some(&cycle),
            Some(1000),
        )
        .await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_insights() {
        let response = "Here are some insights:\n- First insight\n- Second insight\n* Third insight\n• Fourth insight\nSome other text";
        let insights = parse_insights(response);
        assert_eq!(insights.len(), 4);
        assert_eq!(insights[0], "First insight");
        assert_eq!(insights[3], "Fourth insight");
    }

    #[test]
    fn test_parse_insights_empty() {
        let response = "No insights here.\nJust regular text.";
        let insights = parse_insights(response);
        assert!(insights.is_empty());
    }

    #[test]
    fn test_extract_json_found() {
        let response = "Here is the JSON: {\"key\": \"value\"} and some more text";
        let json = extract_json(response).unwrap();
        assert_eq!(json["key"], "value");
    }

    #[test]
    fn test_extract_json_not_found() {
        let response = "No JSON here";
        assert!(extract_json(response).is_none());
    }

    #[test]
    fn test_extract_json_nested() {
        let response = r#"Result: {"a": 1, "b": {"c": 2}} end"#;
        let json = extract_json(response).unwrap();
        assert_eq!(json["a"], 1);
        assert_eq!(json["b"]["c"], 2);
    }

    #[test]
    fn test_build_analysis_prompt_empty_stats() {
        let stats = ExperienceStats {
            total_count: 0,
            success_count: 0,
            failure_count: 0,
            avg_duration_ms: 0.0,
            tool_counts: HashMap::new(),
        };
        let prompt = build_analysis_prompt(&stats, 0);
        assert!(prompt.contains("0"));
        assert!(prompt.contains("0.0%"));
    }

    #[test]
    fn test_parse_insights_skip_empty() {
        let response = "- \n- valid insight\n- ";
        let insights = parse_insights(response);
        assert_eq!(insights.len(), 1);
        assert_eq!(insights[0], "valid insight");
    }

    #[test]
    fn test_parse_insights_mixed_prefixes() {
        let response = "- dash insight\n* star insight\n- bullet insight";
        let insights = parse_insights(response);
        assert_eq!(insights.len(), 3);
    }

    #[test]
    fn test_extract_json_invalid_json() {
        let response = "{not valid json}";
        assert!(extract_json(response).is_none());
    }

    #[test]
    fn test_extract_json_array_not_supported() {
        let response = "[1, 2, 3]";
        assert!(extract_json(response).is_none());
    }

    #[test]
    fn test_build_analysis_prompt_multiple_tools() {
        let mut tool_counts = HashMap::new();
        tool_counts.insert("file_read".into(), crate::types::ToolStats { count: 30, success_count: 28, avg_duration_ms: 50.0 });
        tool_counts.insert("file_write".into(), crate::types::ToolStats { count: 20, success_count: 18, avg_duration_ms: 80.0 });
        tool_counts.insert("exec".into(), crate::types::ToolStats { count: 10, success_count: 5, avg_duration_ms: 200.0 });
        let stats = ExperienceStats {
            total_count: 60,
            success_count: 51,
            failure_count: 9,
            avg_duration_ms: 100.0,
            tool_counts,
        };
        let prompt = build_analysis_prompt(&stats, 3);
        assert!(prompt.contains("file_read"));
        assert!(prompt.contains("file_write"));
        assert!(prompt.contains("exec"));
        assert!(prompt.contains("60"));
    }

    #[test]
    fn test_build_full_analysis_prompt_empty_artifacts() {
        let stats = make_reflection_stats();
        let prompt = build_full_analysis_prompt(&stats, &[], None, None);
        assert!(prompt.contains("Existing Forge Artifacts"));
    }

    #[test]
    fn test_build_full_analysis_prompt_no_low_success() {
        let stats = ReflectionStats {
            total_records: 100,
            unique_patterns: 5,
            avg_success_rate: 0.99,
            top_patterns: vec![],
            low_success: vec![],
            tool_frequency: HashMap::new(),
        };
        let prompt = build_full_analysis_prompt(&stats, &[], None, None);
        // Should still contain the section header but no entries
        assert!(prompt.contains("100"));
    }

    #[test]
    fn test_parse_insights_single_dash() {
        let insights = parse_insights("- only one insight");
        assert_eq!(insights.len(), 1);
        assert_eq!(insights[0], "only one insight");
    }

    #[test]
    fn test_parse_insights_unicode_content() {
        let insights = parse_insights("- Unicode test content");
        assert_eq!(insights.len(), 1);
    }

    #[test]
    fn test_extract_json_multiple_braces() {
        // extract_json extracts from first { to last } and tries to parse.
        // With two separate JSON objects, the combined string is invalid,
        // so extract_json returns None.
        let response = r#"first {"a":1} second {"b":2}"#;
        assert!(extract_json(response).is_none());

        // A single valid JSON object should still work
        let response2 = r#"text before {"a":1} text after"#;
        let json = extract_json(response2).unwrap();
        assert_eq!(json["a"], 1);
    }

    #[test]
    fn test_build_full_analysis_prompt_trace_signals() {
        let stats = make_reflection_stats();
        let mut trace = TraceStats::default();
        trace.total_traces = 10;
        trace.signal_summary.insert("retry".to_string(), 5);
        trace.signal_summary.insert("backtrack".to_string(), 2);
        let prompt = build_full_analysis_prompt(&stats, &[], Some(&trace), None);
        assert!(prompt.contains("retry"));
        assert!(prompt.contains("backtrack"));
    }
}

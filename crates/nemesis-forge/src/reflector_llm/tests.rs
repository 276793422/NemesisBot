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
        created_at: chrono::Local::now().to_rfc3339(),
        updated_at: chrono::Local::now().to_rfc3339(),
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
        started_at: chrono::Local::now().to_rfc3339(),
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
        started_at: chrono::Local::now().to_rfc3339(),
        completed_at: None,
        patterns_found: 2,
        actions_taken: 1,
        status: nemesis_types::forge::CycleStatus::Running,
    };
    let result =
        semantic_analysis(&caller, &stats, &[], Some(&trace), Some(&cycle), Some(1000)).await;
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
    tool_counts.insert(
        "file_read".into(),
        crate::types::ToolStats {
            count: 30,
            success_count: 28,
            avg_duration_ms: 50.0,
        },
    );
    tool_counts.insert(
        "file_write".into(),
        crate::types::ToolStats {
            count: 20,
            success_count: 18,
            avg_duration_ms: 80.0,
        },
    );
    tool_counts.insert(
        "exec".into(),
        crate::types::ToolStats {
            count: 10,
            success_count: 5,
            avg_duration_ms: 200.0,
        },
    );
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

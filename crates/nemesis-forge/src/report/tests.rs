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
            id: "a1".into(),
            name: "skill-a".into(),
            kind: ArtifactKind::Skill,
            version: "1.0".into(),
            status: ArtifactStatus::Active,
            content: String::new(),
            tool_signature: vec![],
            created_at: "2026-05-01T00:00:00Z".into(),
            updated_at: "2026-05-01T00:00:00Z".into(),
            usage_count: 5,
            last_degraded_at: None,
            success_rate: 0.0,
            consecutive_observing_rounds: 0,
        },
        Artifact {
            id: "a2".into(),
            name: "script-b".into(),
            kind: ArtifactKind::Script,
            version: "2.0".into(),
            status: ArtifactStatus::Draft,
            content: String::new(),
            tool_signature: vec![],
            created_at: "2026-05-01T00:00:00Z".into(),
            updated_at: "2026-05-01T00:00:00Z".into(),
            usage_count: 0,
            last_degraded_at: None,
            success_rate: 0.0,
            consecutive_observing_rounds: 0,
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

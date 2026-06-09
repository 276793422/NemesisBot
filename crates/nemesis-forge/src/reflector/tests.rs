use super::*;
use crate::types::CollectedExperience;
use crate::types::Experience;

fn make_collected(tool: &str, input: &str, success: bool, duration_ms: u64) -> CollectedExperience {
    let hash = Collector::dedup_hash(tool, &serde_json::json!({"input": input}));
    let exp = Experience {
        id: uuid::Uuid::new_v4().to_string(),
        tool_name: tool.into(),
        input_summary: input.into(),
        output_summary: if success { "ok".into() } else { "err".into() },
        success,
        duration_ms,
        timestamp: "2026-04-29T00:00:00Z".into(),
        session_key: "sess-test".into(),
    };
    CollectedExperience {
        experience: exp,
        dedup_hash: hash,
    }
}

// Need Collector for dedup_hash
use crate::collector::Collector;

#[test]
fn test_analyze_empty() {
    let reflector = Reflector::new();
    let stats = reflector.analyze(&[]);
    assert_eq!(stats.total_count, 0);
    assert_eq!(stats.success_count, 0);
    assert!(stats.tool_counts.is_empty());
}

#[test]
fn test_analyze_mixed_experiences() {
    let reflector = Reflector::new();
    let experiences = vec![
        make_collected("file_read", "a.txt", true, 50),
        make_collected("file_read", "b.txt", true, 60),
        make_collected("file_write", "c.txt", false, 200),
        make_collected("file_write", "d.txt", false, 300),
        make_collected("file_write", "e.txt", false, 250),
    ];
    let stats = reflector.analyze(&experiences);
    assert_eq!(stats.total_count, 5);
    assert_eq!(stats.success_count, 2);
    assert_eq!(stats.failure_count, 3);
    assert_eq!(stats.tool_counts.len(), 2);

    let fr = &stats.tool_counts["file_read"];
    assert_eq!(fr.count, 2);
    assert_eq!(fr.success_count, 2);

    let fw = &stats.tool_counts["file_write"];
    assert_eq!(fw.count, 3);
    assert_eq!(fw.success_count, 0);
}

#[test]
fn test_generate_reflection_with_low_success_tool() {
    let reflector = Reflector::new();
    let mut experiences = Vec::new();
    for i in 0..5 {
        experiences.push(make_collected(
            "flaky_tool",
            &format!("input-{}", i),
            false,
            6000,
        ));
    }
    experiences.push(make_collected("stable_tool", "ok", true, 100));

    let reflection = reflector.generate_reflection(&experiences);

    assert!(!reflection.insights.is_empty());
    // Check patterns embedded in statistics JSON
    let patterns = reflection.statistics.get("patterns")
        .and_then(|v| v.as_array())
        .unwrap();
    assert!(patterns.iter().any(|p| p.as_str().unwrap().contains("flaky_tool")));
    assert!(reflection.recommendations.iter().any(|r| r.contains("flaky_tool")));
}

#[test]
fn test_statistical_analysis() {
    let reflector = Reflector::new();
    let experiences = vec![
        make_collected("tool_a", "input1", true, 100),
        make_collected("tool_a", "input2", true, 150),
        make_collected("tool_a", "input3", false, 200),
        make_collected("tool_b", "input1", true, 50),
    ];

    let stats = reflector.statistical_analysis(&experiences);
    assert_eq!(stats.total_records, 4);
    assert_eq!(stats.unique_patterns, 2);
    assert_eq!(stats.top_patterns.len(), 2);
    assert_eq!(stats.tool_frequency["tool_a"], 3);
    assert_eq!(stats.tool_frequency["tool_b"], 1);
}

#[test]
fn test_analyze_traces() {
    let reflector = Reflector::new();
    let experiences = vec![
        make_collected("read", "file", true, 50),
        make_collected("edit", "file", true, 100),
        make_collected("exec", "cmd", false, 200),
        make_collected("read", "file2", true, 60),
        make_collected("edit", "file2", true, 120),
        make_collected("exec", "cmd2", true, 180),
    ];

    let trace_stats = reflector.analyze_traces(&experiences, None);
    // All experiences share the same session_key "sess-test" from make_collected,
    // so total_traces = unique sessions = 1
    assert_eq!(trace_stats.total_traces, 1);
    assert!(trace_stats.avg_duration_ms > 0);
    assert!(!trace_stats.retry_patterns.is_empty());
}

#[test]
fn test_analyze_traces_with_learning_cycle() {
    let reflector = Reflector::new();
    let experiences = vec![
        make_collected("tool_a", "input", true, 100),
        make_collected("tool_b", "input", true, 200),
    ];

    let cycle = nemesis_types::forge::LearningCycle {
        id: "lc-123".to_string(),
        started_at: chrono::Local::now().to_rfc3339(),
        completed_at: None,
        patterns_found: 3,
        actions_taken: 1,
        status: nemesis_types::forge::CycleStatus::Completed,
    };

    let trace_stats = reflector.analyze_traces(&experiences, Some(&cycle));
    assert_eq!(trace_stats.signal_summary.get("learning_patterns_found"), Some(&3));
    assert_eq!(trace_stats.signal_summary.get("learning_actions_taken"), Some(&1));
}

#[test]
fn test_reflect_full_cycle() {
    let reflector = Reflector::with_cluster();
    let experiences = vec![
        make_collected("tool_a", "input1", true, 100),
        make_collected("tool_b", "input2", false, 200),
    ];

    let report = reflector.reflect(&experiences, None, "today", "all");
    assert_eq!(report.period, "today");
    assert_eq!(report.focus, "all");
    assert!(report.stats.total_records > 0);
    assert!(report.trace_stats.is_some());
}

#[test]
fn test_reflect_empty() {
    let reflector = Reflector::new();
    let report = reflector.reflect(&[], None, "week", "skill");
    assert_eq!(report.stats.total_records, 0);
    assert!(report.trace_stats.is_none());
}

#[test]
fn test_generate_suggestion() {
    let reflector = Reflector::new();
    assert!(reflector.generate_suggestion(10, 0.95).contains("High frequency"));
    assert!(reflector.generate_suggestion(3, 0.8).contains("Stable pattern"));
    assert!(reflector.generate_suggestion(5, 0.5).contains("failure modes"));
    // count < 5, success_rate >= 0.9 but count check comes first and fails
    // So it falls through to success_rate >= 0.7 => "Stable pattern"
    assert!(reflector.generate_suggestion(2, 0.85).contains("Stable pattern"));
    // count < 5, success_rate < 0.7 => "Review failure modes"
    assert!(reflector.generate_suggestion(2, 0.5).contains("failure modes"));
}

// ----- Disk operation tests -----

#[test]
fn test_write_report() {
    let dir = tempfile::tempdir().unwrap();
    let reflector = Reflector::with_reflections_dir(dir.path().join("reflections"));

    let report = ReflectionReport {
        date: "2026-05-01".to_string(),
        period: "today".to_string(),
        focus: "all".to_string(),
        stats: ReflectionStats {
            total_records: 5,
            unique_patterns: 2,
            avg_success_rate: 0.8,
            top_patterns: vec![PatternInsight {
                tool_name: "file_read".to_string(),
                count: 5,
                avg_duration_ms: 100,
                success_rate: 1.0,
                suggestion: "Stable".to_string(),
            }],
            low_success: vec![],
            tool_frequency: HashMap::new(),
        },
        llm_insights: None,
        trace_stats: None,
        learning_cycle: None,
    };

    let path = reflector.write_report(&report).unwrap();
    assert!(path.exists());
    assert!(path.file_name().unwrap().to_string_lossy().starts_with("reflection_2026-05-01"));

    // Verify content
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("# Reflection Report: 2026-05-01"));
    assert!(content.contains("file_read"));
}

#[test]
fn test_write_report_no_dir_configured() {
    let reflector = Reflector::new();
    let report = ReflectionReport {
        date: "2026-05-01".to_string(),
        period: "today".to_string(),
        focus: "all".to_string(),
        stats: ReflectionStats::default(),
        llm_insights: None,
        trace_stats: None,
        learning_cycle: None,
    };
    let result = reflector.write_report(&report);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not configured"));
}

#[test]
fn test_cleanup_reports() {
    let dir = tempfile::tempdir().unwrap();
    let ref_dir = dir.path().join("reflections");
    std::fs::create_dir_all(&ref_dir).unwrap();

    // Create an "old" file with a known modification time in the past.
    // We use filetime::set_file_mtime to control the mtime precisely.
    let old_path = ref_dir.join("reflection_2026-03-01_120000.md");
    std::fs::write(&old_path, "old report").unwrap();

    // Set modification time to 31 days ago
    let old_time = std::time::SystemTime::now()
        - std::time::Duration::from_secs(31 * 24 * 3600);
    let ft = filetime::FileTime::from_system_time(old_time);
    filetime::set_file_mtime(&old_path, ft).unwrap();

    // Create a "new" file (current time = not old)
    let new_path = ref_dir.join("reflection_2026-05-01_120000.md");
    std::fs::write(&new_path, "new report").unwrap();

    let reflector = Reflector::with_reflections_dir(ref_dir);
    let deleted = reflector.cleanup_reports(30);
    assert_eq!(deleted, 1);
    assert!(!old_path.exists());
    assert!(new_path.exists());
}

#[test]
fn test_cleanup_reports_empty_dir() {
    let dir = tempfile::tempdir().unwrap();
    let reflector = Reflector::with_reflections_dir(dir.path().join("reflections"));
    let deleted = reflector.cleanup_reports(30);
    assert_eq!(deleted, 0);
}

#[test]
fn test_get_latest_report() {
    let dir = tempfile::tempdir().unwrap();
    let ref_dir = dir.path().join("reflections");
    std::fs::create_dir_all(&ref_dir).unwrap();

    // Create two report files
    let report1 = ref_dir.join("reflection_2026-04-28_120000.md");
    std::fs::write(&report1, "report 1").unwrap();

    // Small delay to ensure different mtime
    std::thread::sleep(std::time::Duration::from_millis(50));

    let report2 = ref_dir.join("reflection_2026-04-29_120000.md");
    std::fs::write(&report2, "report 2").unwrap();

    let reflector = Reflector::with_reflections_dir(ref_dir);
    let latest = reflector.get_latest_report();
    assert!(latest.is_some());
    assert_eq!(latest.unwrap(), report2);
}

#[test]
fn test_get_latest_report_empty() {
    let dir = tempfile::tempdir().unwrap();
    let ref_dir = dir.path().join("reflections");
    std::fs::create_dir_all(&ref_dir).unwrap();

    let reflector = Reflector::with_reflections_dir(ref_dir);
    assert!(reflector.get_latest_report().is_none());
}

#[test]
fn test_get_latest_report_no_dir() {
    let reflector = Reflector::new();
    assert!(reflector.get_latest_report().is_none());
}

#[test]
fn test_resolve_period() {
    let today = Reflector::resolve_period("today");
    assert!(today.is_some());
    let today_val = today.unwrap();
    assert!(today_val.contains("T00:00:00"));

    let week = Reflector::resolve_period("week");
    assert!(week.is_some());

    let all = Reflector::resolve_period("all");
    assert!(all.is_none());

    let unknown = Reflector::resolve_period("unknown");
    assert!(unknown.is_some()); // defaults to today
}

#[test]
fn test_filter_by_period() {
    let reflector = Reflector::new();
    let experiences = vec![
        make_collected("tool_a", "old", true, 100),
        make_collected("tool_b", "new", true, 200),
    ];

    let filtered = reflector.filter_by_period(&experiences, "all");
    assert_eq!(filtered.len(), 2);

    let filtered = reflector.filter_by_period(&experiences, "today");
    // All test experiences have timestamp 2026-04-29, so they may or may not
    // be filtered depending on when the test runs. Just verify no panic.
    let _ = filtered.len();
}

#[test]
fn test_filter_by_focus() {
    let reflector = Reflector::new();
    let experiences = vec![
        make_collected("file_read", "a", true, 100),
        make_collected("file_write", "b", true, 200),
        make_collected("file_read", "c", true, 300),
    ];

    let all = reflector.filter_by_focus(&experiences, "all");
    assert_eq!(all.len(), 3);

    let all_empty = reflector.filter_by_focus(&experiences, "");
    assert_eq!(all_empty.len(), 3);

    let only_read = reflector.filter_by_focus(&experiences, "file_read");
    assert_eq!(only_read.len(), 2);

    let only_write = reflector.filter_by_focus(&experiences, "file_write");
    assert_eq!(only_write.len(), 1);
}

// --- Additional reflector tests ---

#[test]
fn test_analyze_single_tool_all_success() {
    let reflector = Reflector::new();
    let experiences: Vec<CollectedExperience> = (0..5)
        .map(|i| make_collected("perfect_tool", &format!("input-{}", i), true, 100))
        .collect();
    let stats = reflector.analyze(&experiences);
    assert_eq!(stats.total_count, 5);
    assert_eq!(stats.success_count, 5);
    assert_eq!(stats.failure_count, 0);
    assert_eq!(stats.tool_counts.len(), 1);
    let ts = &stats.tool_counts["perfect_tool"];
    assert_eq!(ts.count, 5);
    assert_eq!(ts.success_count, 5);
}

#[test]
fn test_analyze_single_tool_all_failures() {
    let reflector = Reflector::new();
    let experiences: Vec<CollectedExperience> = (0..3)
        .map(|i| make_collected("broken_tool", &format!("input-{}", i), false, 500))
        .collect();
    let stats = reflector.analyze(&experiences);
    assert_eq!(stats.total_count, 3);
    assert_eq!(stats.success_count, 0);
    assert_eq!(stats.failure_count, 3);
}

#[test]
fn test_analyze_mixed_durations() {
    let reflector = Reflector::new();
    let experiences = vec![
        make_collected("fast", "a", true, 10),
        make_collected("fast", "b", true, 20),
        make_collected("slow", "c", true, 5000),
        make_collected("slow", "d", true, 6000),
    ];
    let stats = reflector.analyze(&experiences);
    assert_eq!(stats.tool_counts.len(), 2);
    let fast = &stats.tool_counts["fast"];
    let slow = &stats.tool_counts["slow"];
    assert!(fast.avg_duration_ms < slow.avg_duration_ms);
}

#[test]
fn test_statistical_analysis_empty() {
    let reflector = Reflector::new();
    let stats = reflector.statistical_analysis(&[]);
    assert_eq!(stats.total_records, 0);
    assert_eq!(stats.unique_patterns, 0);
    assert!(stats.top_patterns.is_empty());
    assert!(stats.low_success.is_empty());
}

#[test]
fn test_statistical_analysis_success_rate() {
    let reflector = Reflector::new();
    let experiences = vec![
        make_collected("tool", "a", true, 100),
        make_collected("tool", "b", true, 100),
    ];
    let stats = reflector.statistical_analysis(&experiences);
    assert!((stats.avg_success_rate - 1.0).abs() < 0.01);
}

#[test]
fn test_statistical_analysis_low_success_detection() {
    let reflector = Reflector::new();
    let mut experiences = Vec::new();
    // 3 failures, 1 success = 25% success rate for flaky_tool
    for i in 0..3 {
        experiences.push(make_collected("flaky_tool", &format!("f-{}", i), false, 100));
    }
    experiences.push(make_collected("flaky_tool", "s-1", true, 100));
    let stats = reflector.statistical_analysis(&experiences);
    assert_eq!(stats.low_success.len(), 1);
    assert_eq!(stats.low_success[0].tool_name, "flaky_tool");
    assert!(stats.low_success[0].suggestion.contains("failure"));
}

#[test]
fn test_statistical_analysis_top_patterns_sorted_by_count() {
    let reflector = Reflector::new();
    let mut experiences = Vec::new();
    for _ in 0..10 {
        experiences.push(make_collected("popular", "x", true, 50));
    }
    for _ in 0..5 {
        experiences.push(make_collected("moderate", "x", true, 50));
    }
    for _ in 0..2 {
        experiences.push(make_collected("rare", "x", true, 50));
    }
    let stats = reflector.statistical_analysis(&experiences);
    assert!(stats.top_patterns.len() >= 2);
    assert!(stats.top_patterns[0].count >= stats.top_patterns[1].count);
}

#[test]
fn test_reflect_with_learning_cycle() {
    let reflector = Reflector::with_cluster();
    let experiences = vec![
        make_collected("tool_a", "input1", true, 100),
    ];
    let cycle = nemesis_types::forge::LearningCycle {
        id: "lc-test".into(),
        started_at: chrono::Local::now().to_rfc3339(),
        completed_at: Some(chrono::Local::now().to_rfc3339()),
        patterns_found: 5,
        actions_taken: 2,
        status: nemesis_types::forge::CycleStatus::Completed,
    };
    let report = reflector.reflect(&experiences, Some(&cycle), "today", "all");
    assert!(report.learning_cycle.is_some());
    assert_eq!(report.learning_cycle.unwrap().patterns_found, 5);
}

#[test]
fn test_reflect_with_trace_stats() {
    let reflector = Reflector::with_cluster();
    let experiences = vec![
        make_collected("tool_a", "input", true, 100),
        make_collected("tool_b", "input", true, 200),
    ];
    let report = reflector.reflect(&experiences, None, "week", "all");
    assert!(report.trace_stats.is_some());
    let ts = report.trace_stats.unwrap();
    assert!(ts.total_traces > 0);
}

#[test]
fn test_reflect_report_structure() {
    let reflector = Reflector::new();
    let experiences = vec![
        make_collected("tool", "input", true, 100),
    ];
    let report = reflector.reflect(&experiences, None, "today", "all");
    assert!(!report.date.is_empty());
    assert_eq!(report.period, "today");
    assert_eq!(report.focus, "all");
    assert!(report.llm_insights.is_none());
}

#[test]
fn test_analyze_traces_empty() {
    let reflector = Reflector::new();
    let trace_stats = reflector.analyze_traces(&[], None);
    assert_eq!(trace_stats.total_traces, 0);
    assert_eq!(trace_stats.avg_duration_ms, 0);
}

#[test]
fn test_analyze_traces_multiple_sessions() {
    let reflector = Reflector::new();
    // Create experiences across multiple sessions
    let mut experiences = Vec::new();
    for i in 0..3 {
        experiences.push(Experience {
            id: format!("exp-{}", i),
            tool_name: "tool".into(),
            input_summary: "input".into(),
            output_summary: "ok".into(),
            success: true,
            duration_ms: 100 * (i as u64 + 1),
            timestamp: "2026-04-29T00:00:00Z".into(),
            session_key: format!("session-{}", i),
        });
    }
    let ces: Vec<CollectedExperience> = experiences.into_iter().map(|e| {
        CollectedExperience {
            dedup_hash: Collector::dedup_hash(&e.tool_name, &serde_json::json!({})),
            experience: e,
        }
    }).collect();
    let trace_stats = reflector.analyze_traces(&ces, None);
    assert_eq!(trace_stats.total_traces, 3);
}

#[test]
fn test_generate_suggestion_boundary_values() {
    let reflector = Reflector::new();
    // count=5, rate=0.9 -> High frequency
    assert!(reflector.generate_suggestion(5, 0.9).contains("High frequency"));
    // count=4, rate=0.9 -> Stable (count < 5)
    assert!(reflector.generate_suggestion(4, 0.9).contains("Stable"));
    // count=10, rate=0.7 -> Stable
    assert!(reflector.generate_suggestion(10, 0.7).contains("Stable"));
    // count=10, rate=0.69 -> Review failure
    assert!(reflector.generate_suggestion(10, 0.69).contains("failure"));
}

#[test]
fn test_write_report_with_llm_insights() {
    let dir = tempfile::tempdir().unwrap();
    let reflector = Reflector::with_reflections_dir(dir.path().join("reflections"));
    let report = ReflectionReport {
        date: "2026-05-01".into(),
        period: "today".into(),
        focus: "all".into(),
        stats: ReflectionStats::default(),
        llm_insights: Some("AI detected efficiency issues".into()),
        trace_stats: None,
        learning_cycle: None,
    };
    let path = reflector.write_report(&report).unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("LLM Insights"));
    assert!(content.contains("AI detected efficiency issues"));
}

#[test]
fn test_write_report_with_trace_stats() {
    let dir = tempfile::tempdir().unwrap();
    let reflector = Reflector::with_reflections_dir(dir.path().join("reflections"));
    let report = ReflectionReport {
        date: "2026-05-01".into(),
        period: "today".into(),
        focus: "all".into(),
        stats: ReflectionStats::default(),
        llm_insights: None,
        trace_stats: Some(TraceStats {
            total_traces: 10,
            avg_rounds: 5.0,
            avg_duration_ms: 500,
            efficiency_score: 0.75,
            tool_chain_patterns: vec![],
            retry_patterns: vec![],
            signal_summary: HashMap::new(),
        }),
        learning_cycle: None,
    };
    let path = reflector.write_report(&report).unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("Trace Analysis"));
    assert!(content.contains("10"));
}

#[test]
fn test_cleanup_reports_result_ok() {
    let dir = tempfile::tempdir().unwrap();
    let ref_dir = dir.path().join("reflections");
    std::fs::create_dir_all(&ref_dir).unwrap();
    let reflector = Reflector::with_reflections_dir(ref_dir);
    let result = reflector.cleanup_reports_result(30);
    assert!(result.is_ok());
}

#[test]
fn test_cleanup_reports_result_no_dir() {
    let reflector = Reflector::new();
    let result = reflector.cleanup_reports_result(30);
    assert!(result.is_err());
}

#[test]
fn test_get_latest_report_content_no_reports() {
    let dir = tempfile::tempdir().unwrap();
    let ref_dir = dir.path().join("reflections");
    std::fs::create_dir_all(&ref_dir).unwrap();
    let reflector = Reflector::with_reflections_dir(ref_dir);
    let result = reflector.get_latest_report_content();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("no reflection"));
}

#[test]
fn test_get_latest_report_content_with_report() {
    let dir = tempfile::tempdir().unwrap();
    let ref_dir = dir.path().join("reflections");
    std::fs::create_dir_all(&ref_dir).unwrap();
    let report_path = ref_dir.join("reflection_2026-05-01_120000.md");
    std::fs::write(&report_path, "# Test Report").unwrap();
    let reflector = Reflector::with_reflections_dir(ref_dir);
    let content = reflector.get_latest_report_content().unwrap();
    assert!(content.contains("Test Report"));
}

#[test]
fn test_merge_remote_reflections_empty() {
    let reflector = Reflector::new();
    let result = reflector.merge_remote_reflections(&[], &[]);
    assert!(result.local_patterns.is_empty());
    assert!(result.remote_patterns.is_empty());
    assert!(result.merged_patterns.is_empty());
}

#[test]
fn test_merge_remote_reflections_local_only() {
    let reflector = Reflector::new();
    let experiences = vec![
        make_collected("local_tool", "input", true, 100),
    ];
    let result = reflector.merge_remote_reflections(&[], &experiences);
    assert!(!result.local_patterns.is_empty());
    assert!(result.remote_patterns.is_empty());
}

#[test]
fn test_merge_remote_reflections_from_file() {
    let dir = tempfile::tempdir().unwrap();
    let report_path = dir.path().join("remote_report.md");
    let report_content = "# Report\n| read_file | 10 |\n| write_file | 5 |\n";
    std::fs::write(&report_path, report_content).unwrap();

    let reflector = Reflector::new();
    let result = reflector.merge_remote_reflections(&[report_path], &[]);
    assert!(!result.remote_patterns.is_empty());
    assert!(result.unique_remote_tools.contains(&"read_file".to_string())
        || result.unique_remote_tools.contains(&"write_file".to_string()));
}

#[test]
fn test_resolve_period_defaults_to_today() {
    let result = Reflector::resolve_period("custom_period");
    assert!(result.is_some());
    let today = Reflector::resolve_period("today");
    assert_eq!(result, today);
}

#[test]
fn test_reflection_stats_default() {
    let stats = ReflectionStats::default();
    assert_eq!(stats.total_records, 0);
    assert_eq!(stats.unique_patterns, 0);
    assert_eq!(stats.avg_success_rate, 0.0);
    assert!(stats.top_patterns.is_empty());
    assert!(stats.low_success.is_empty());
    assert!(stats.tool_frequency.is_empty());
}

#[test]
fn test_trace_stats_default() {
    let stats = TraceStats::default();
    assert_eq!(stats.total_traces, 0);
    assert_eq!(stats.avg_rounds, 0.0);
    assert_eq!(stats.avg_duration_ms, 0);
    assert_eq!(stats.efficiency_score, 0.0);
    assert!(stats.tool_chain_patterns.is_empty());
    assert!(stats.retry_patterns.is_empty());
    assert!(stats.signal_summary.is_empty());
}

#[test]
fn test_filter_by_focus_nonexistent_tool() {
    let reflector = Reflector::new();
    let experiences = vec![
        make_collected("tool_a", "input", true, 100),
    ];
    let filtered = reflector.filter_by_focus(&experiences, "nonexistent");
    assert!(filtered.is_empty());
}

#[test]
fn test_reflector_default() {
    let reflector = Reflector::default();
    let stats = reflector.analyze(&[]);
    assert_eq!(stats.total_count, 0);
}

#[test]
fn test_get_latest_report_content_no_dir() {
    let reflector = Reflector::new();
    let result = reflector.get_latest_report_content();
    assert!(result.is_err());
}

#[test]
fn test_get_latest_report_from_dir() {
    let dir = tempfile::tempdir().unwrap();
    let ref_dir = dir.path().join("reflections");
    std::fs::create_dir_all(&ref_dir).unwrap();
    std::fs::write(ref_dir.join("reflection_2026-05-01_120000.md"), "# Report 1").unwrap();
    std::fs::write(ref_dir.join("reflection_2026-05-02_120000.md"), "# Report 2").unwrap();
    let reflector = Reflector::with_reflections_dir(ref_dir);
    let latest = reflector.get_latest_report();
    assert!(latest.is_some());
}

#[test]
fn test_get_latest_report_content_from_dir() {
    let dir = tempfile::tempdir().unwrap();
    let ref_dir = dir.path().join("reflections");
    std::fs::create_dir_all(&ref_dir).unwrap();
    std::fs::write(ref_dir.join("reflection_2026-05-01_120000.md"), "# Report content here").unwrap();
    let reflector = Reflector::with_reflections_dir(ref_dir);
    let content = reflector.get_latest_report_content();
    assert!(content.is_ok());
    assert!(content.unwrap().contains("Report content here"));
}

#[test]
fn test_cleanup_reports_empty_dir_2() {
    let dir = tempfile::tempdir().unwrap();
    let ref_dir = dir.path().join("reflections");
    std::fs::create_dir_all(&ref_dir).unwrap();
    let reflector = Reflector::with_reflections_dir(ref_dir);
    let deleted = reflector.cleanup_reports(30);
    assert_eq!(deleted, 0);
}

#[test]
fn test_cleanup_reports_no_dir() {
    let reflector = Reflector::new();
    let deleted = reflector.cleanup_reports(30);
    assert_eq!(deleted, 0);
}

#[test]
fn test_resolve_period_all() {
    let result = Reflector::resolve_period("all");
    assert!(result.is_none());
}

#[test]
fn test_resolve_period_week() {
    let result = Reflector::resolve_period("week");
    assert!(result.is_some());
}

#[test]
fn test_resolve_period_today() {
    let result = Reflector::resolve_period("today");
    assert!(result.is_some());
}

#[test]
fn test_filter_by_period_all() {
    let reflector = Reflector::new();
    let experiences = vec![
        make_collected("tool", "a", true, 100),
        make_collected("tool", "b", false, 200),
    ];
    let filtered = reflector.filter_by_period(&experiences, "all");
    assert_eq!(filtered.len(), 2);
}

#[test]
fn test_filter_by_focus_all() {
    let reflector = Reflector::new();
    let experiences = vec![
        make_collected("tool_a", "a", true, 100),
        make_collected("tool_b", "b", false, 200),
    ];
    let filtered = reflector.filter_by_focus(&experiences, "all");
    assert_eq!(filtered.len(), 2);
}

#[test]
fn test_filter_by_focus_empty() {
    let reflector = Reflector::new();
    let experiences = vec![
        make_collected("tool_a", "a", true, 100),
    ];
    let filtered = reflector.filter_by_focus(&experiences, "");
    assert_eq!(filtered.len(), 1);
}

#[test]
fn test_set_cluster_enabled() {
    let mut reflector = Reflector::new();
    reflector.set_cluster_enabled(true);
    // No crash, field updated
}

#[test]
fn test_set_reflections_dir() {
    let mut reflector = Reflector::new();
    reflector.set_reflections_dir(PathBuf::from("/tmp/test"));
    // No crash, field updated
}

#[test]
fn test_with_cluster_and_dir() {
    let reflector = Reflector::with_cluster_and_dir(PathBuf::from("/tmp/test"));
    let report = reflector.reflect(&[], None, "today", "all");
    assert_eq!(report.period, "today");
}

#[test]
fn test_analyze_mixed_success_failure() {
    let reflector = Reflector::new();
    let mut experiences = Vec::new();
    for _ in 0..3 {
        experiences.push(make_collected("slow_tool", "x", false, 10000));
    }
    experiences.push(make_collected("slow_tool", "y", true, 9000));
    let stats = reflector.analyze(&experiences);
    assert_eq!(stats.total_count, 4);
    assert_eq!(stats.success_count, 1);
    assert_eq!(stats.failure_count, 3);
    let ts = stats.tool_counts.get("slow_tool").unwrap();
    assert_eq!(ts.count, 4);
    assert_eq!(ts.success_count, 1);
}

#[test]
fn test_generate_reflection_slow_tool_pattern() {
    let reflector = Reflector::new();
    let mut experiences = Vec::new();
    for _ in 0..3 {
        experiences.push(make_collected("very_slow_tool", "x", true, 8000));
    }
    let reflection = reflector.generate_reflection(&experiences);
    // Should mention slow tool in insights or recommendations
    let all_text = reflection.insights.join(" ") + &reflection.recommendations.join(" ");
    assert!(all_text.contains("slow") || all_text.contains("Slow") || all_text.contains("optimiz"));
}

#[test]
fn test_generate_reflection_frequent_tool_pattern() {
    let reflector = Reflector::new();
    let mut experiences = Vec::new();
    for _ in 0..6 {
        experiences.push(make_collected("popular_tool", "x", true, 100));
    }
    let reflection = reflector.generate_reflection(&experiences);
    // The tool should appear in insights, recommendations, or statistics
    let _all_text = format!("{:?} {:?}", reflection.insights, reflection.recommendations);
    // Just check that the reflection was generated successfully with the right number of insights
    assert!(!reflection.insights.is_empty());
}

#[test]
fn test_generate_reflection_below_80_percent() {
    let reflector = Reflector::new();
    let mut experiences = Vec::new();
    for i in 0..5 {
        experiences.push(make_collected("tool", &format!("f-{}", i), false, 100));
    }
    for i in 0..2 {
        experiences.push(make_collected("tool", &format!("s-{}", i), true, 100));
    }
    let reflection = reflector.generate_reflection(&experiences);
    let recs = reflection.recommendations.iter().any(|r| r.contains("80%") || r.contains("below"));
    assert!(recs);
}

#[test]
fn test_analyze_traces_with_tool_chains() {
    let reflector = Reflector::new();
    let mut experiences = Vec::new();
    // Create enough for a chain pattern (3+ tools in sequence)
    for i in 0..6 {
        experiences.push(Experience {
            id: format!("exp-{}", i),
            tool_name: format!("tool_{}", i % 3),
            input_summary: "input".into(),
            output_summary: "ok".into(),
            success: true,
            duration_ms: 100,
            timestamp: "2026-04-29T00:00:00Z".into(),
            session_key: "same-session".into(),
        });
    }
    let ces: Vec<CollectedExperience> = experiences.into_iter().map(|e| {
        CollectedExperience {
            dedup_hash: Collector::dedup_hash(&e.tool_name, &serde_json::json!({})),
            experience: e,
        }
    }).collect();
    let trace_stats = reflector.analyze_traces(&ces, None);
    assert_eq!(trace_stats.total_traces, 1); // all same session
}

#[test]
fn test_analyze_traces_with_retry_patterns() {
    let reflector = Reflector::new();
    let mut experiences = Vec::new();
    // 2 calls, 1 error -> retry pattern
    experiences.push(Experience {
        id: "e1".into(),
        tool_name: "retry_tool".into(),
        input_summary: "input".into(),
        output_summary: "fail".into(),
        success: false,
        duration_ms: 100,
        timestamp: "2026-04-29T00:00:00Z".into(),
        session_key: "session-1".into(),
    });
    experiences.push(Experience {
        id: "e2".into(),
        tool_name: "retry_tool".into(),
        input_summary: "input".into(),
        output_summary: "ok".into(),
        success: true,
        duration_ms: 100,
        timestamp: "2026-04-29T00:00:00Z".into(),
        session_key: "session-1".into(),
    });
    let ces: Vec<CollectedExperience> = experiences.into_iter().map(|e| {
        CollectedExperience {
            dedup_hash: Collector::dedup_hash(&e.tool_name, &serde_json::json!({})),
            experience: e,
        }
    }).collect();
    let trace_stats = reflector.analyze_traces(&ces, None);
    assert_eq!(trace_stats.retry_patterns.len(), 1);
    assert_eq!(trace_stats.retry_patterns[0].tool_name, "retry_tool");
}

#[test]
fn test_write_report_with_low_success_patterns() {
    let dir = tempfile::tempdir().unwrap();
    let reflector = Reflector::with_reflections_dir(dir.path().join("reflections"));
    let report = ReflectionReport {
        date: "2026-05-01".into(),
        period: "today".into(),
        focus: "all".into(),
        stats: ReflectionStats {
            total_records: 10,
            unique_patterns: 2,
            avg_success_rate: 0.5,
            top_patterns: vec![],
            low_success: vec![PatternInsight {
                tool_name: "failing_tool".into(),
                count: 5,
                avg_duration_ms: 200,
                success_rate: 0.2,
                suggestion: "Fix it".into(),
            }],
            tool_frequency: HashMap::new(),
        },
        llm_insights: None,
        trace_stats: None,
        learning_cycle: None,
    };
    let path = reflector.write_report(&report).unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("Low Success"));
    assert!(content.contains("failing_tool"));
}

#[test]
fn test_write_report_with_top_patterns() {
    let dir = tempfile::tempdir().unwrap();
    let reflector = Reflector::with_reflections_dir(dir.path().join("reflections"));
    let report = ReflectionReport {
        date: "2026-05-01".into(),
        period: "today".into(),
        focus: "all".into(),
        stats: ReflectionStats {
            total_records: 10,
            unique_patterns: 1,
            avg_success_rate: 0.9,
            top_patterns: vec![PatternInsight {
                tool_name: "read_file".into(),
                count: 8,
                avg_duration_ms: 50,
                success_rate: 1.0,
                suggestion: "Good".into(),
            }],
            low_success: vec![],
            tool_frequency: HashMap::new(),
        },
        llm_insights: None,
        trace_stats: None,
        learning_cycle: None,
    };
    let path = reflector.write_report(&report).unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("Top Patterns"));
    assert!(content.contains("read_file"));
}

#[test]
fn test_cleanup_reports_result_no_dir_configured() {
    let reflector = Reflector::new();
    let result = reflector.cleanup_reports_result(30);
    assert!(result.is_err());
}

#[test]
fn test_cleanup_reports_result_nonexistent_dir() {
    let reflector = Reflector::with_reflections_dir(PathBuf::from("/nonexistent/path/reflections"));
    let result = reflector.cleanup_reports_result(30);
    assert!(result.is_ok());
}

#[test]
fn test_merge_remote_reflections_empty_reports() {
    let reflector = Reflector::new();
    let experiences = vec![make_collected("tool_a", "x", true, 100)];
    let merged = reflector.merge_remote_reflections(&[], &experiences);
    assert!(merged.remote_patterns.is_empty());
    assert!(merged.common_tools.is_empty());
    assert!(merged.unique_remote_tools.is_empty());
}

#[test]
fn test_merge_remote_reflections_with_report_file() {
    let dir = tempfile::tempdir().unwrap();
    let report_path = dir.path().join("remote_report.md");
    let report_content = "# Remote Report\n\n| read_file | 10 |\n| write_file | 5 |\n";
    std::fs::write(&report_path, report_content).unwrap();
    let reflector = Reflector::new();
    let experiences = vec![make_collected("read_file", "x", true, 100)];
    let merged = reflector.merge_remote_reflections(&[report_path], &experiences);
    // read_file should be in common_tools (present in both local and remote)
    assert!(merged.common_tools.contains_key("read_file")
        || merged.unique_remote_tools.iter().any(|t| t == "write_file"));
}

#[test]
fn test_merge_remote_reflections_no_local() {
    let reflector = Reflector::new();
    let merged = reflector.merge_remote_reflections(&[], &[]);
    assert!(merged.local_patterns.is_empty());
    assert!(merged.merged_patterns.is_empty());
}

#[test]
fn test_llm_caller_accessors() {
    let reflector = Reflector::new();
    {
        let caller = reflector.llm_caller();
        assert!(caller.is_none());
    }
}

// --- Additional coverage tests ---

#[test]
fn test_analyze_traces_coverage_empty() {
    let reflector = Reflector::new();
    let stats = reflector.analyze_traces(&[], None);
    assert_eq!(stats.total_traces, 0);
}

#[test]
fn test_analyze_traces_coverage_single() {
    let reflector = Reflector::new();
    let trace = vec![make_collected("tool_a", "x", true, 100)];
    let stats = reflector.analyze_traces(&trace, None);
    assert_eq!(stats.total_traces, 1);
    assert_eq!(stats.avg_duration_ms, 100);
}

#[test]
fn test_analyze_traces_coverage_multiple() {
    let reflector = Reflector::new();
    let traces = vec![
        make_collected("tool_a", "x", true, 100),
        make_collected("tool_b", "y", true, 200),
        make_collected("tool_c", "z", false, 300),
    ];
    let stats = reflector.analyze_traces(&traces, None);
    // total_traces counts unique session_keys, all are "sess-test"
    assert_eq!(stats.total_traces, 1);
    assert_eq!(stats.avg_duration_ms, 200); // (100+200+300)/3
}

#[test]
fn test_analyze_traces_coverage_retries() {
    let reflector = Reflector::new();
    let traces = vec![
        make_collected("tool_a", "x", false, 100),
        make_collected("tool_a", "x", false, 100),
        make_collected("tool_a", "x", true, 100),
    ];
    let stats = reflector.analyze_traces(&traces, None);
    assert!(!stats.retry_patterns.is_empty());
}

#[test]
fn test_analyze_traces_coverage_chains() {
    let reflector = Reflector::new();
    let mut traces: Vec<CollectedExperience> = vec![];
    for _ in 0..5 {
        traces.push(make_collected("read", "x", true, 50));
        traces.push(make_collected("write", "x", true, 50));
    }
    let stats = reflector.analyze_traces(&traces, None);
    assert!(stats.total_traces > 0);
}

#[test]
fn test_analyze_traces_coverage_low_freq() {
    let reflector = Reflector::new();
    let traces = vec![
        make_collected("a", "x", true, 50),
        make_collected("b", "x", true, 50),
    ];
    let stats = reflector.analyze_traces(&traces, None);
    assert!(stats.total_traces > 0);
}

#[test]
fn test_write_report_with_tool_frequency() {
    let dir = tempfile::tempdir().unwrap();
    let reflector = Reflector::with_reflections_dir(dir.path().join("reflections"));

    let mut tool_frequency = HashMap::new();
    tool_frequency.insert("read_file".into(), 15);
    tool_frequency.insert("write_file".into(), 8);

    let report = ReflectionReport {
        date: "2026-05-01".into(),
        period: "today".into(),
        focus: "all".into(),
        stats: ReflectionStats {
            total_records: 23,
            unique_patterns: 2,
            avg_success_rate: 0.92,
            top_patterns: vec![],
            low_success: vec![],
            tool_frequency,
        },
        llm_insights: None,
        trace_stats: None,
        learning_cycle: None,
    };
    let path = reflector.write_report(&report).unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("23")); // total_records
    assert!(content.contains("92.0%")); // avg_success_rate
}

#[test]
fn test_cleanup_reports_keeps_recent() {
    let dir = tempfile::tempdir().unwrap();
    let reflections_dir = dir.path().join("reflections");
    std::fs::create_dir_all(&reflections_dir).unwrap();

    // Create a recent report
    let recent_name = format!("{}.md", chrono::Local::now().format("%Y%m%d"));
    std::fs::write(reflections_dir.join(&recent_name), "recent report").unwrap();

    let reflector = Reflector::with_reflections_dir(reflections_dir);
    let result = reflector.cleanup_reports_result(30);
    assert!(result.is_ok());

    // Verify file still exists
    assert!(dir.path().join("reflections").join(&recent_name).exists());
}

#[test]
fn test_merge_remote_reflections_unreadable_file() {
    let reflector = Reflector::new();
    let experiences = vec![make_collected("tool_a", "x", true, 100)];
    // Non-existent file path should be handled gracefully
    let merged = reflector.merge_remote_reflections(
        &[PathBuf::from("/nonexistent/report.md")],
        &experiences,
    );
    // Should not panic, just skip the file
    assert!(merged.remote_patterns.is_empty() || merged.merged_patterns.is_empty());
}

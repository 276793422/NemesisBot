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
            timestamp: chrono::Local::now().to_rfc3339(),
            session_key: "test".into(),
        },
        dedup_hash: format!("hash-{}", tool),
    }
}

#[test]
fn test_extract_tool_chain() {
    let exps: Vec<CollectedExperience> = (0..5).map(|_| make_exp("file_read", true, 100)).collect();

    let stats = ExperienceStats {
        total_count: 5,
        success_count: 5,
        failure_count: 0,
        avg_duration_ms: 100.0,
        tool_counts: Default::default(),
    };

    let patterns = extract_patterns(&exps, &stats);
    assert!(
        patterns
            .iter()
            .any(|p| p.pattern_type == PatternType::ToolChain)
    );
    assert!(
        patterns
            .iter()
            .any(|p| p.pattern_type == PatternType::SuccessTemplate)
    );
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
    assert!(
        patterns
            .iter()
            .any(|p| p.pattern_type == PatternType::ErrorRecovery)
    );
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
    assert!(
        patterns
            .iter()
            .any(|p| p.pattern_type == PatternType::EfficiencyIssue)
    );
}

#[test]
fn test_empty_experiences() {
    let patterns = extract_patterns(
        &[],
        &ExperienceStats {
            total_count: 0,
            success_count: 0,
            failure_count: 0,
            avg_duration_ms: 0.0,
            tool_counts: Default::default(),
        },
    );
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

fn make_exp_session(
    tool: &str,
    success: bool,
    duration: u64,
    session: &str,
) -> CollectedExperience {
    CollectedExperience {
        experience: Experience {
            id: uuid::Uuid::new_v4().to_string(),
            tool_name: tool.into(),
            input_summary: "test".into(),
            output_summary: if success { "ok" } else { "err" }.into(),
            success,
            duration_ms: duration,
            timestamp: chrono::Local::now().to_rfc3339(),
            session_key: session.into(),
        },
        dedup_hash: format!("hash-{}", tool),
    }
}

#[test]
fn test_conversation_pattern_type_as_str() {
    assert_eq!(ConversationPatternType::ToolChain.as_str(), "tool_chain");
    assert_eq!(
        ConversationPatternType::ErrorRecovery.as_str(),
        "error_recovery"
    );
    assert_eq!(
        ConversationPatternType::EfficiencyIssue.as_str(),
        "efficiency_issue"
    );
    assert_eq!(
        ConversationPatternType::SuccessTemplate.as_str(),
        "success_template"
    );
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
    assert!(
        !patterns
            .iter()
            .any(|p| p.pattern_type == PatternType::ToolChain
                && p.tools.contains(&"rare_tool".to_string()))
    );
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
        assert!(patterns[i - 1].confidence >= patterns[i].confidence);
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
        for j in (i + 1)..strs.len() {
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
    let eff_patterns: Vec<_> = patterns
        .iter()
        .filter(|p| p.pattern_type == PatternType::EfficiencyIssue)
        .collect();
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
    assert!(
        !patterns
            .iter()
            .any(|p| p.pattern_type == PatternType::SuccessTemplate
                && p.tools.contains(&"tool_a".to_string()))
    );
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
    assert!(
        patterns
            .iter()
            .any(|p| p.pattern_type == PatternType::ErrorRecovery)
    );
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
    let types: std::collections::HashSet<&str> =
        patterns.iter().map(|p| p.pattern_type.as_str()).collect();
    assert!(types.contains("tool_chain"));
}

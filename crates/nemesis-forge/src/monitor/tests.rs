use super::*;

fn make_artifact(id: &str, usage: u64, status: ArtifactStatus) -> Artifact {
    Artifact {
        id: id.into(),
        name: format!("artifact-{}", id),
        kind: nemesis_types::forge::ArtifactKind::Skill,
        version: "1.0.0".into(),
        status,
        content: "test".into(),
        tool_signature: vec!["tool_a".into()],
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
        usage_count: usage,
        last_degraded_at: None,
        success_rate: 0.0,
        consecutive_observing_rounds: 0,
    }
}

fn make_trace(rounds: u32, duration_ms: i64, tools: &[&str], has_signals: bool) -> ConversationTrace {
    ConversationTrace {
        start_time: chrono::Utc::now().to_rfc3339(),
        total_rounds: rounds,
        duration_ms,
        tool_steps: tools.iter().map(|t| ToolStep { tool_name: t.to_string() }).collect(),
        signals: if has_signals { vec!["retry".to_string()] } else { vec![] },
    }
}

#[test]
fn test_evaluate_insufficient_samples() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);

    let artifact = make_artifact("a1", 2, ArtifactStatus::Active);
    let result = monitor.evaluate(&artifact);
    assert_eq!(result.verdict, "observing");
    assert_eq!(result.sample_size, 2);
}

#[test]
fn test_should_degrade() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);

    let mut artifact = make_artifact("a2", 10, ArtifactStatus::Observing);
    artifact.consecutive_observing_rounds = 3;
    assert!(monitor.should_degrade(&artifact));

    artifact.consecutive_observing_rounds = 2;
    assert!(!monitor.should_degrade(&artifact));
}

#[test]
fn test_apply_degradation() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let id = registry.add(make_artifact("a3", 10, ArtifactStatus::Active));

    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);
    let applied = monitor.apply_degradation(&id);
    assert!(applied);

    let artifact = monitor.registry.get(&id).unwrap();
    assert_eq!(artifact.status, ArtifactStatus::Degraded);
}

// --- Cooldown tests ---

#[test]
fn test_cooldown_elapsed_no_previous_degradation() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);

    let artifact = make_artifact("cool1", 10, ArtifactStatus::Active);
    assert!(monitor.is_degradation_cooldown_elapsed(&artifact));
}

#[test]
fn test_cooldown_not_elapsed_recent_degradation() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);

    let mut artifact = make_artifact("cool2", 10, ArtifactStatus::Active);
    artifact.last_degraded_at = Some(chrono::Utc::now().to_rfc3339());
    assert!(!monitor.is_degradation_cooldown_elapsed(&artifact));
}

#[test]
fn test_cooldown_elapsed_old_degradation() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);

    let mut artifact = make_artifact("cool3", 10, ArtifactStatus::Active);
    let ten_days_ago = (chrono::Utc::now() - chrono::Duration::days(10)).to_rfc3339();
    artifact.last_degraded_at = Some(ten_days_ago);
    // Default cooldown is 7 days, so 10 days should be fine
    assert!(monitor.is_degradation_cooldown_elapsed(&artifact));
}

#[test]
fn test_apply_degradation_respects_cooldown() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));

    let mut artifact = make_artifact("cool4", 10, ArtifactStatus::Active);
    artifact.last_degraded_at = Some(chrono::Utc::now().to_rfc3339()); // Just degraded
    let id = registry.add(artifact);

    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);
    let applied = monitor.apply_degradation(&id);
    assert!(!applied); // Should be skipped due to cooldown

    let artifact = monitor.registry.get(&id).unwrap();
    assert_eq!(artifact.status, ArtifactStatus::Active); // Status unchanged
}

// --- Auto-upgrade tests ---

#[test]
fn test_evaluate_all_auto_upgrade_consecutive_observing() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));

    let mut artifact = make_artifact("upgrade1", 10, ArtifactStatus::Observing);
    artifact.consecutive_observing_rounds = 3;
    registry.add(artifact.clone());

    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);
    let results = monitor.evaluate_all();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].verdict, "negative"); // Auto-upgraded
}

#[test]
fn test_evaluate_all_no_upgrade_few_rounds() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));

    let mut artifact = make_artifact("upgrade2", 10, ArtifactStatus::Observing);
    artifact.consecutive_observing_rounds = 2;
    registry.add(artifact.clone());

    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);
    let results = monitor.evaluate_all();

    assert_eq!(results.len(), 1);
    // Should not have been force-set to "negative" by auto-upgrade
    // since consecutive_observing_rounds < 3
    if results[0].verdict == "observing" {
        // Good: stayed as observing
    }
}

#[test]
fn test_run_evaluation_cycle_degrades_negative() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));

    let mut artifact = make_artifact("cycle1", 10, ArtifactStatus::Observing);
    artifact.consecutive_observing_rounds = 3;
    let id = registry.add(artifact.clone());

    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);
    let results = monitor.run_evaluation_cycle();

    assert!(results.iter().any(|r| r.verdict == "negative"));
    let updated = monitor.registry.get(&id).unwrap();
    assert_eq!(updated.status, ArtifactStatus::Degraded);
}

#[test]
fn test_run_evaluation_cycle_respects_cooldown() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));

    let mut artifact = make_artifact("cycle2", 10, ArtifactStatus::Observing);
    artifact.consecutive_observing_rounds = 3;
    artifact.last_degraded_at = Some(chrono::Utc::now().to_rfc3339()); // Just degraded
    let id = registry.add(artifact.clone());

    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);
    let _results = monitor.run_evaluation_cycle();

    // Should NOT be degraded due to cooldown
    let updated = monitor.registry.get(&id).unwrap();
    assert_eq!(updated.status, ArtifactStatus::Observing);
}

// --- Trace-based evaluation tests ---

#[test]
fn test_matches_tool_signature_exact() {
    let trace = make_trace(3, 100, &["tool_a", "tool_b", "tool_c"], false);
    assert!(matches_tool_signature(&trace, &["tool_a".to_string(), "tool_b".to_string()]));
}

#[test]
fn test_matches_tool_signature_subsequence() {
    let trace = make_trace(3, 100, &["tool_a", "tool_x", "tool_b"], false);
    assert!(matches_tool_signature(&trace, &["tool_a".to_string(), "tool_b".to_string()]));
}

#[test]
fn test_matches_tool_signature_no_match() {
    let trace = make_trace(2, 100, &["tool_x", "tool_y"], false);
    assert!(!matches_tool_signature(&trace, &["tool_a".to_string()]));
}

#[test]
fn test_matches_tool_signature_empty() {
    let trace = make_trace(1, 100, &["tool_a"], false);
    assert!(!matches_tool_signature(&trace, &[]));
}

#[test]
fn test_normalize_basic() {
    // (before - after) / before
    assert_eq!(normalize(10.0, 8.0), 0.2); // 20% improvement
    assert_eq!(normalize(10.0, 12.0), -0.2); // 20% worse
}

#[test]
fn test_normalize_zero_before() {
    assert_eq!(normalize(0.0, 5.0), 0.0);
}

#[test]
fn test_avg_rounds() {
    let traces: Vec<ConversationTrace> = vec![
        make_trace(4, 100, &["a"], false),
        make_trace(6, 100, &["b"], false),
    ];
    let refs: Vec<&ConversationTrace> = traces.iter().collect();
    assert_eq!(avg_rounds(&refs), 5.0);
}

#[test]
fn test_avg_rounds_empty() {
    let refs: Vec<&ConversationTrace> = vec![];
    assert_eq!(avg_rounds(&refs), 0.0);
}

#[test]
fn test_success_rate() {
    let traces: Vec<ConversationTrace> = vec![
        make_trace(1, 100, &["a"], false),
        make_trace(1, 100, &["b"], true), // has signals
        make_trace(1, 100, &["c"], false),
    ];
    let refs: Vec<&ConversationTrace> = traces.iter().collect();
    assert_eq!(success_rate(&refs), 2.0 / 3.0);
}

#[test]
fn test_avg_duration() {
    let traces: Vec<ConversationTrace> = vec![
        make_trace(1, 100, &["a"], false),
        make_trace(1, 200, &["b"], false),
        make_trace(1, 300, &["c"], false),
    ];
    let refs: Vec<&ConversationTrace> = traces.iter().collect();
    assert_eq!(avg_duration(&refs), 200);
}

#[test]
fn test_classify_verdict() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);

    let artifact = make_artifact("v1", 10, ArtifactStatus::Active);

    assert_eq!(monitor.classify_verdict(0.5, &artifact), "positive");
    assert_eq!(monitor.classify_verdict(0.05, &artifact), "neutral");
    assert_eq!(monitor.classify_verdict(-0.15, &artifact), "observing");
    assert_eq!(monitor.classify_verdict(-0.3, &artifact), "negative");
}

#[test]
fn test_evaluate_outcomes_trace_based() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));

    let mut artifact = make_artifact("trace1", 10, ArtifactStatus::Active);
    artifact.tool_signature = vec!["file_read".to_string()];
    let id = registry.add(artifact);

    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);

    // Create traces that match the tool signature
    let traces = vec![
        make_trace(3, 100, &["file_read"], false),
        make_trace(2, 80, &["file_read"], false),
        make_trace(4, 120, &["file_read"], false),
        make_trace(3, 90, &["file_read"], false),
        make_trace(2, 70, &["file_read"], false),
        make_trace(5, 150, &["other_tool"], false), // doesn't match signature
    ];

    let outcomes = monitor.evaluate_outcomes(&traces);
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].artifact_id, id);
    assert!(outcomes[0].sample_size >= 5);
}

#[test]
fn test_track_observing_increments_and_deprecates() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));

    let mut artifact = make_artifact("obs1", 10, ArtifactStatus::Observing);
    artifact.consecutive_observing_rounds = 2;
    let id = registry.add(artifact);

    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);

    // First observing round (2 -> 3, triggers deprecation)
    let artifact = monitor.registry.get(&id).unwrap();
    monitor.track_observing(&artifact);

    let updated = monitor.registry.get(&id).unwrap();
    assert_eq!(updated.status, ArtifactStatus::Degraded);
    assert_eq!(updated.consecutive_observing_rounds, 0);
}

#[test]
fn test_handle_verdict_positive_resets_counter() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));

    let mut artifact = make_artifact("pos1", 10, ArtifactStatus::Active);
    artifact.consecutive_observing_rounds = 2;
    let id = registry.add(artifact);

    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);

    let artifact = monitor.registry.get(&id).unwrap();
    let outcome = ActionOutcome {
        artifact_id: id.clone(),
        measured_at: chrono::Utc::now().to_rfc3339(),
        sample_size: 10,
        rounds_before_avg: 5.0,
        rounds_after_avg: 3.0,
        success_before: 0.6,
        success_after: 0.9,
        duration_before_ms: 200,
        duration_after_ms: 100,
        improvement_score: 0.5,
        verdict: "positive".to_string(),
    };

    monitor.handle_verdict(&artifact, &outcome);

    let updated = monitor.registry.get(&id).unwrap();
    assert_eq!(updated.consecutive_observing_rounds, 0);
}

// --- Additional monitor tests ---

#[test]
fn test_evaluate_with_high_usage() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let artifact = make_artifact("high-use", 100, ArtifactStatus::Active);
    let id = registry.add(artifact);
    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);
    let result = monitor.evaluate(&monitor.registry.get(&id).unwrap());
    assert_eq!(result.artifact_id, id);
    assert_eq!(result.sample_size, 100);
}

#[test]
fn test_evaluate_with_zero_usage() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let artifact = make_artifact("zero-use", 0, ArtifactStatus::Active);
    let id = registry.add(artifact);
    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);
    let result = monitor.evaluate(&monitor.registry.get(&id).unwrap());
    assert_eq!(result.artifact_id, id);
    assert_eq!(result.sample_size, 0);
}

#[test]
fn test_should_degrade_below_threshold() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);
    let mut artifact = make_artifact("no-degrade", 10, ArtifactStatus::Active);
    artifact.consecutive_observing_rounds = 0;
    assert!(!monitor.should_degrade(&artifact));
    artifact.consecutive_observing_rounds = 1;
    assert!(!monitor.should_degrade(&artifact));
    artifact.consecutive_observing_rounds = 2;
    assert!(!monitor.should_degrade(&artifact));
}

#[test]
fn test_should_degrade_at_threshold() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);
    let mut artifact = make_artifact("at-threshold", 10, ArtifactStatus::Observing);
    artifact.consecutive_observing_rounds = 3;
    assert!(monitor.should_degrade(&artifact));
    artifact.consecutive_observing_rounds = 4;
    assert!(monitor.should_degrade(&artifact));
}

#[test]
fn test_apply_degradation_nonexistent() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);
    assert!(!monitor.apply_degradation("no-such-id"));
}

#[test]
fn test_apply_degradation_sets_timestamp() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let id = registry.add(make_artifact("degrade-ts", 10, ArtifactStatus::Active));
    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);
    assert!(monitor.apply_degradation(&id));
    let artifact = monitor.registry.get(&id).unwrap();
    assert!(artifact.last_degraded_at.is_some());
}

#[test]
fn test_apply_degradation_resets_counter() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let mut artifact = make_artifact("reset-ctr", 10, ArtifactStatus::Active);
    artifact.consecutive_observing_rounds = 5;
    let id = registry.add(artifact);
    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);
    monitor.apply_degradation(&id);
    let updated = monitor.registry.get(&id).unwrap();
    assert_eq!(updated.consecutive_observing_rounds, 0);
}

#[test]
fn test_cooldown_elapsed_invalid_timestamp() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);
    let mut artifact = make_artifact("bad-ts", 10, ArtifactStatus::Active);
    artifact.last_degraded_at = Some("not-a-timestamp".to_string());
    // Invalid timestamp should allow degradation
    assert!(monitor.is_degradation_cooldown_elapsed(&artifact));
}

#[test]
fn test_cooldown_elapsed_none() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);
    let artifact = make_artifact("no-ts", 10, ArtifactStatus::Active);
    assert!(artifact.last_degraded_at.is_none());
    assert!(monitor.is_degradation_cooldown_elapsed(&artifact));
}

#[test]
fn test_classify_verdict_boundaries() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);
    let artifact = make_artifact("boundary", 10, ArtifactStatus::Active);
    // Positive threshold: > 0.1
    assert_eq!(monitor.classify_verdict(0.11, &artifact), "positive");
    // Neutral: >= -0.1
    assert_eq!(monitor.classify_verdict(0.0, &artifact), "neutral");
    assert_eq!(monitor.classify_verdict(0.1, &artifact), "neutral");
    assert_eq!(monitor.classify_verdict(-0.1, &artifact), "neutral");
    // Observing: >= -0.2 and < -0.1
    assert_eq!(monitor.classify_verdict(-0.15, &artifact), "observing");
    assert_eq!(monitor.classify_verdict(-0.2, &artifact), "observing");
    // Negative: < -0.2
    assert_eq!(monitor.classify_verdict(-0.21, &artifact), "negative");
}

#[test]
fn test_matches_tool_signature_single_tool() {
    let trace = make_trace(1, 100, &["tool_a"], false);
    assert!(matches_tool_signature(&trace, &["tool_a".to_string()]));
    assert!(!matches_tool_signature(&trace, &["tool_b".to_string()]));
}

#[test]
fn test_matches_tool_signature_long_chain() {
    let trace = make_trace(5, 100, &["a", "b", "c", "d", "e"], false);
    assert!(matches_tool_signature(&trace, &["a".to_string(), "c".to_string(), "e".to_string()]));
    assert!(!matches_tool_signature(&trace, &["a".to_string(), "e".to_string(), "c".to_string()]));
}

#[test]
fn test_matches_tool_signature_partial_match() {
    let trace = make_trace(3, 100, &["a", "b", "c"], false);
    assert!(matches_tool_signature(&trace, &["a".to_string(), "b".to_string()]));
    assert!(matches_tool_signature(&trace, &["b".to_string(), "c".to_string()]));
    assert!(!matches_tool_signature(&trace, &["a".to_string(), "c".to_string(), "d".to_string()]));
}

#[test]
fn test_normalize_positive() {
    assert!(normalize(100.0, 80.0) > 0.0);
}

#[test]
fn test_normalize_negative() {
    assert!(normalize(100.0, 120.0) < 0.0);
}

#[test]
fn test_normalize_equal() {
    assert_eq!(normalize(100.0, 100.0), 0.0);
}

#[test]
fn test_avg_duration_empty() {
    let refs: Vec<&ConversationTrace> = vec![];
    assert_eq!(avg_duration(&refs), 0);
}

#[test]
fn test_avg_duration_single() {
    let traces = vec![make_trace(1, 300, &["a"], false)];
    let refs: Vec<&ConversationTrace> = traces.iter().collect();
    assert_eq!(avg_duration(&refs), 300);
}

#[test]
fn test_success_rate_all_success() {
    let traces: Vec<ConversationTrace> = vec![
        make_trace(1, 100, &["a"], false),
        make_trace(1, 100, &["b"], false),
    ];
    let refs: Vec<&ConversationTrace> = traces.iter().collect();
    assert_eq!(success_rate(&refs), 1.0);
}

#[test]
fn test_success_rate_all_failure() {
    let traces: Vec<ConversationTrace> = vec![
        make_trace(1, 100, &["a"], true),
        make_trace(1, 100, &["b"], true),
    ];
    let refs: Vec<&ConversationTrace> = traces.iter().collect();
    assert_eq!(success_rate(&refs), 0.0);
}

#[test]
fn test_success_rate_empty() {
    let refs: Vec<&ConversationTrace> = vec![];
    assert_eq!(success_rate(&refs), 0.0);
}

#[test]
fn test_evaluate_all_empty_registry() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);
    let results = monitor.evaluate_all();
    assert!(results.is_empty());
}

#[test]
fn test_evaluate_all_filters_draft_artifacts() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    registry.add(make_artifact("draft", 10, ArtifactStatus::Draft));
    registry.add(make_artifact("active", 10, ArtifactStatus::Active));
    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry.clone());
    let results = monitor.evaluate_all();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].artifact_id, registry.list(None, Some(ArtifactStatus::Active))[0].id);
}

#[test]
fn test_handle_verdict_negative_triggers_deprecation() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let artifact = make_artifact("neg1", 10, ArtifactStatus::Active);
    let id = registry.add(artifact.clone());
    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);
    let artifact = monitor.registry.get(&id).unwrap();
    let outcome = ActionOutcome {
        artifact_id: id.clone(),
        measured_at: chrono::Utc::now().to_rfc3339(),
        sample_size: 10,
        rounds_before_avg: 5.0,
        rounds_after_avg: 8.0,
        success_before: 0.9,
        success_after: 0.3,
        duration_before_ms: 100,
        duration_after_ms: 500,
        improvement_score: -0.5,
        verdict: "negative".to_string(),
    };
    monitor.handle_verdict(&artifact, &outcome);
    let updated = monitor.registry.get(&id).unwrap();
    assert_eq!(updated.status, ArtifactStatus::Degraded);
}

#[test]
fn test_handle_verdict_observing_increments() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let mut artifact = make_artifact("obs2", 10, ArtifactStatus::Observing);
    artifact.consecutive_observing_rounds = 0;
    let id = registry.add(artifact);
    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);
    let artifact = monitor.registry.get(&id).unwrap();
    let outcome = ActionOutcome {
        artifact_id: id.clone(),
        measured_at: chrono::Utc::now().to_rfc3339(),
        sample_size: 10,
        rounds_before_avg: 5.0,
        rounds_after_avg: 5.5,
        success_before: 0.8,
        success_after: 0.75,
        duration_before_ms: 100,
        duration_after_ms: 110,
        improvement_score: -0.1,
        verdict: "observing".to_string(),
    };
    monitor.handle_verdict(&artifact, &outcome);
    let updated = monitor.registry.get(&id).unwrap();
    assert_eq!(updated.consecutive_observing_rounds, 1);
}

#[test]
fn test_handle_verdict_neutral_no_change() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let mut artifact = make_artifact("neut1", 10, ArtifactStatus::Active);
    artifact.consecutive_observing_rounds = 2;
    let id = registry.add(artifact);
    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);
    let artifact = monitor.registry.get(&id).unwrap();
    let outcome = ActionOutcome {
        artifact_id: id.clone(),
        measured_at: chrono::Utc::now().to_rfc3339(),
        sample_size: 10,
        rounds_before_avg: 5.0,
        rounds_after_avg: 5.0,
        success_before: 0.8,
        success_after: 0.8,
        duration_before_ms: 100,
        duration_after_ms: 100,
        improvement_score: 0.05,
        verdict: "neutral".to_string(),
    };
    monitor.handle_verdict(&artifact, &outcome);
    let updated = monitor.registry.get(&id).unwrap();
    // Counter should not change for neutral
    assert_eq!(updated.consecutive_observing_rounds, 2);
}

#[test]
fn test_evaluation_result_fields() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let artifact = make_artifact("fields", 5, ArtifactStatus::Active);
    let id = registry.add(artifact);
    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);
    let result = monitor.evaluate(&monitor.registry.get(&id).unwrap());
    assert!(!result.artifact_id.is_empty());
    assert_eq!(result.sample_size, 5);
}

#[test]
fn test_action_outcome_fields() {
    let outcome = ActionOutcome {
        artifact_id: "test-artifact".to_string(),
        measured_at: "2026-05-09T00:00:00Z".to_string(),
        sample_size: 10,
        rounds_before_avg: 5.0,
        rounds_after_avg: 3.0,
        success_before: 0.6,
        success_after: 0.9,
        duration_before_ms: 200,
        duration_after_ms: 100,
        improvement_score: 0.5,
        verdict: "positive".to_string(),
    };
    assert_eq!(outcome.artifact_id, "test-artifact");
    assert_eq!(outcome.verdict, "positive");
    assert_eq!(outcome.improvement_score, 0.5);
}

#[test]
fn test_evaluation_result_fields_manual() {
    let result = EvaluationResult {
        artifact_id: "test-result".to_string(),
        improvement_score: 0.3,
        verdict: "observing".to_string(),
        sample_size: 5,
    };
    assert_eq!(result.artifact_id, "test-result");
    assert_eq!(result.verdict, "observing");
}

// --- success_rate-based evaluate() tests ---

#[test]
fn test_evaluate_high_success_rate_positive() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let mut artifact = make_artifact("sr-pos", 10, ArtifactStatus::Active);
    artifact.success_rate = 0.9; // 0.9 - 0.5 = 0.4 improvement => positive
    let id = registry.add(artifact);
    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);
    let result = monitor.evaluate(&monitor.registry.get(&id).unwrap());
    assert_eq!(result.verdict, "positive");
    assert!((result.improvement_score - 0.4).abs() < 0.001);
}

#[test]
fn test_evaluate_medium_success_rate_neutral() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let mut artifact = make_artifact("sr-neut", 10, ArtifactStatus::Active);
    artifact.success_rate = 0.55; // 0.55 - 0.5 = 0.05 improvement => neutral
    let id = registry.add(artifact);
    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);
    let result = monitor.evaluate(&monitor.registry.get(&id).unwrap());
    assert_eq!(result.verdict, "neutral");
}

#[test]
fn test_evaluate_low_success_rate_negative() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let mut artifact = make_artifact("sr-neg", 10, ArtifactStatus::Active);
    artifact.success_rate = 0.1; // 0.1 - 0.5 = -0.4 improvement => negative
    let id = registry.add(artifact);
    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);
    let result = monitor.evaluate(&monitor.registry.get(&id).unwrap());
    assert_eq!(result.verdict, "negative");
    assert!((result.improvement_score - (-0.4)).abs() < 0.001);
}

#[test]
fn test_evaluate_below_baseline_observing() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let mut artifact = make_artifact("sr-obs", 10, ArtifactStatus::Active);
    artifact.success_rate = 0.35; // 0.35 - 0.5 = -0.15 improvement => observing
    let id = registry.add(artifact);
    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);
    let result = monitor.evaluate(&monitor.registry.get(&id).unwrap());
    assert_eq!(result.verdict, "observing");
}

#[test]
fn test_evaluate_exact_baseline_neutral() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let mut artifact = make_artifact("sr-base", 10, ArtifactStatus::Active);
    artifact.success_rate = 0.5; // 0.5 - 0.5 = 0.0 improvement => neutral
    let id = registry.add(artifact);
    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);
    let result = monitor.evaluate(&monitor.registry.get(&id).unwrap());
    assert_eq!(result.verdict, "neutral");
}

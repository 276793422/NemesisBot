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
        created_at: chrono::Local::now().to_rfc3339(),
        updated_at: chrono::Local::now().to_rfc3339(),
        usage_count: usage,
        last_degraded_at: None,
        success_rate: 0.0,
        consecutive_observing_rounds: 0,
        deployed_at: None,
        last_efficacy: None,
    }
}

fn make_trace(
    rounds: u32,
    duration_ms: i64,
    tools: &[&str],
    has_signals: bool,
) -> ConversationTrace {
    ConversationTrace {
        start_time: chrono::Local::now().to_rfc3339(),
        total_rounds: rounds,
        duration_ms,
        tool_steps: tools
            .iter()
            .map(|t| ToolStep {
                tool_name: t.to_string(),
            })
            .collect(),
        signals: if has_signals {
            vec!["retry".to_string()]
        } else {
            vec![]
        },
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
    artifact.last_degraded_at = Some(chrono::Local::now().to_rfc3339());
    assert!(!monitor.is_degradation_cooldown_elapsed(&artifact));
}

#[test]
fn test_cooldown_elapsed_old_degradation() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);

    let mut artifact = make_artifact("cool3", 10, ArtifactStatus::Active);
    let ten_days_ago = (chrono::Local::now() - chrono::Duration::days(10)).to_rfc3339();
    artifact.last_degraded_at = Some(ten_days_ago);
    // Default cooldown is 7 days, so 10 days should be fine
    assert!(monitor.is_degradation_cooldown_elapsed(&artifact));
}

#[test]
fn test_apply_degradation_respects_cooldown() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));

    let mut artifact = make_artifact("cool4", 10, ArtifactStatus::Active);
    artifact.last_degraded_at = Some(chrono::Local::now().to_rfc3339()); // Just degraded
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
    // 用健康工件（success_rate 0.8 → 普通分类 = positive）让 auto-upgrade
    // 路径可判别：旧版 success_rate 0.0 时普通分类本来就出 negative，
    // 删掉 auto-upgrade 代码测试照样绿（恒绿死测试）。断言升级签名值
    // （verdict=negative + improvement=-0.3）双重钉死。
    artifact.success_rate = 0.8;
    registry.add(artifact.clone());

    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);
    let results = monitor.evaluate_all();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].verdict, "negative"); // Auto-upgraded
    assert!(
        (results[0].improvement_score + 0.3).abs() < f64::EPSILON,
        "auto-upgrade must stamp the -0.3 signature score, got {}",
        results[0].improvement_score
    );
}

#[test]
fn test_evaluate_all_no_upgrade_few_rounds() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));

    let mut artifact = make_artifact("upgrade2", 10, ArtifactStatus::Observing);
    artifact.consecutive_observing_rounds = 2;
    // 健康工件：普通分类 = positive（improvement 0.3）。rounds<3 时
    // auto-upgrade 不得触发——若错误触发，verdict 会被强改为 negative。
    artifact.success_rate = 0.8;
    registry.add(artifact.clone());

    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);
    let results = monitor.evaluate_all();

    assert_eq!(results.len(), 1);
    // 旧版此处是空 if 体（零断言死测试），且旧样张 success_rate=0.0 时
    // verdict 本来就是 negative，测不出 auto-upgrade 是否误触发。
    assert_eq!(results[0].verdict, "positive");
    assert!(
        (results[0].improvement_score - 0.3).abs() < f64::EPSILON,
        "improvement must be the un-tampered 0.3, got {}",
        results[0].improvement_score
    );
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
    artifact.last_degraded_at = Some(chrono::Local::now().to_rfc3339()); // Just degraded
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
    assert!(matches_tool_signature(
        &trace,
        &["tool_a".to_string(), "tool_b".to_string()]
    ));
}

#[test]
fn test_matches_tool_signature_subsequence() {
    let trace = make_trace(3, 100, &["tool_a", "tool_x", "tool_b"], false);
    assert!(matches_tool_signature(
        &trace,
        &["tool_a".to_string(), "tool_b".to_string()]
    ));
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
        measured_at: chrono::Local::now().to_rfc3339(),
        sample_size: 10,
        sample_size_before: 8,
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
    assert!(matches_tool_signature(
        &trace,
        &["a".to_string(), "c".to_string(), "e".to_string()]
    ));
    assert!(!matches_tool_signature(
        &trace,
        &["a".to_string(), "e".to_string(), "c".to_string()]
    ));
}

#[test]
fn test_matches_tool_signature_partial_match() {
    let trace = make_trace(3, 100, &["a", "b", "c"], false);
    assert!(matches_tool_signature(
        &trace,
        &["a".to_string(), "b".to_string()]
    ));
    assert!(matches_tool_signature(
        &trace,
        &["b".to_string(), "c".to_string()]
    ));
    assert!(!matches_tool_signature(
        &trace,
        &["a".to_string(), "c".to_string(), "d".to_string()]
    ));
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
    let traces = [make_trace(1, 300, &["a"], false)];
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
    assert_eq!(
        results[0].artifact_id,
        registry.list(None, Some(ArtifactStatus::Active))[0].id
    );
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
        measured_at: chrono::Local::now().to_rfc3339(),
        sample_size: 10,
        sample_size_before: 8,
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
        measured_at: chrono::Local::now().to_rfc3339(),
        sample_size: 10,
        sample_size_before: 8,
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
        measured_at: chrono::Local::now().to_rfc3339(),
        sample_size: 10,
        sample_size_before: 8,
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
        sample_size_before: 8,
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

// ---- S8 coverage batch ---- (quality-hardening goal 冲刺 S8)

#[test]
fn test_s8_evaluate_outcomes_all_traces_outside_window_returns_empty() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let mut artifact = make_artifact("old1", 10, ArtifactStatus::Active);
    artifact.tool_signature = vec!["file_read".to_string()];
    registry.add(artifact);

    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);

    // Trace with a parseable start_time far outside the 7-day window →
    // recent_traces empty → early return with no outcomes.
    let old_trace = ConversationTrace {
        start_time: "2020-01-01T00:00:00+08:00".to_string(),
        total_rounds: 3,
        duration_ms: 100,
        tool_steps: vec![ToolStep {
            tool_name: "file_read".to_string(),
        }],
        signals: vec![],
    };
    let outcomes = monitor.evaluate_outcomes(&[old_trace]);
    assert!(outcomes.is_empty());
}

#[test]
fn test_s8_evaluate_outcomes_window_respects_timestamp_offset() {
    // 2026-10-22 验证轮二回归锁：窗口过滤必须全量偏移感知解析 start_time。
    // 旧实现 `[..19]` 截断后硬贴 "Z"——把 -05:00 挂钟读成 UTC，时刻被平移
    // -5h。本例真值 = now-166h（7 天=168h 窗内），旧解析得 now-171h（窗
    // 外）→ 旧代码空 outcomes；新代码解析回真值 → 产出 1 条 outcome。
    // 时间锚全部取自绝对时刻，断言与机器本地时区无关。
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let mut artifact = make_artifact("offs1", 10, ArtifactStatus::Active);
    artifact.tool_signature = vec!["file_read".to_string()];
    registry.add(artifact);

    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);

    let real = chrono::Utc::now() - chrono::Duration::hours(166);
    let offset = chrono::FixedOffset::west_opt(5 * 3600).unwrap();
    let trace = ConversationTrace {
        start_time: real.with_timezone(&offset).to_rfc3339(),
        total_rounds: 3,
        duration_ms: 100,
        tool_steps: vec![ToolStep {
            tool_name: "file_read".to_string(),
        }],
        signals: vec![],
    };

    let outcomes = monitor.evaluate_outcomes(&[trace]);
    // 窗内轨迹进入评估（样本数 < min_outcome_samples=5 → insufficient_data，
    // 但 outcome 本身必须产出——空 outcomes 意味着窗口过滤把轨迹丢了）。
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].verdict, "insufficient_data");
}

#[test]
fn test_s8_evaluate_outcomes_min_traces_guard() {
    // 2026-10-23 验证轮四回归锁：`min_traces_for_analysis` 此前零消费者
    // （死键）。接线语义 = 窗口内轨迹少于 N 条不做实效分析（样本太少，
    // 结论即噪声），0 = 不设下限（护栏默认关）。双臂对照：min=5 时 2 条
    // 窗内轨迹被闸掉；min=0 时同样轨迹照常产出 outcome——证明空结果
    // 来自本护栏而非其他过滤。
    let make = |min_traces: usize| {
        let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
        let mut artifact = make_artifact("guard1", 10, ArtifactStatus::Active);
        artifact.tool_signature = vec!["file_read".to_string()];
        registry.add(artifact);

        let mut config = ForgeConfig::default();
        config.trace.min_traces_for_analysis = min_traces;
        DeploymentMonitor::new(config, registry)
    };

    let traces = vec![
        make_trace(3, 100, &["file_read"], false),
        make_trace(3, 100, &["file_read"], false),
    ];

    // min=5：2 条 < 5 → 护栏拦下，空 outcomes。
    let guarded = make(5);
    assert!(guarded.evaluate_outcomes(&traces).is_empty());

    // min=0（默认）：不设下限 → 照常评估（verdict 打分另受
    // min_outcome_samples 约束，但 outcome 本身必须产出）。
    let open = make(0);
    let outcomes = open.evaluate_outcomes(&traces);
    assert_eq!(outcomes.len(), 1);
}

#[test]
fn test_s8_evaluate_outcomes_skips_non_active_artifact() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    // Observing (not Active) → skipped by the status filter.
    let mut artifact = make_artifact("skip1", 10, ArtifactStatus::Observing);
    artifact.tool_signature = vec!["file_read".to_string()];
    registry.add(artifact);

    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);
    let traces = vec![make_trace(3, 100, &["file_read"], false)];
    let outcomes = monitor.evaluate_outcomes(&traces);
    assert!(outcomes.is_empty());
}

#[test]
fn test_s8_evaluate_artifact_before_traces_insufficient_after() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    // min_outcome_samples = 0 → the default minimum of 5 applies; zero
    // after-traces → "insufficient_data" early return.
    let mut cfg = ForgeConfig::default();
    cfg.learning.min_outcome_samples = 0;
    let monitor = DeploymentMonitor::new(cfg, registry.clone());

    let mut artifact = make_artifact("ins1", 10, ArtifactStatus::Active);
    artifact.tool_signature = vec!["file_read".to_string()];
    // Deploy time = now; traces below are 1h old → "before" bucket.
    let id = registry.add(artifact);

    let old = chrono::Local::now() - chrono::Duration::hours(1);
    let before_trace = ConversationTrace {
        start_time: old.to_rfc3339(),
        total_rounds: 4,
        duration_ms: 100,
        tool_steps: vec![ToolStep {
            tool_name: "file_read".to_string(),
        }],
        signals: vec![],
    };

    let outcomes = monitor.evaluate_outcomes(&[before_trace]);
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].artifact_id, id);
    assert_eq!(outcomes[0].verdict, "insufficient_data");
    assert_eq!(outcomes[0].sample_size, 0);
}

#[test]
fn test_s8_try_deprecate_default_cooldown_config_branch() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    // degradation_cooldown_days = 0 → default 7-day fallback branch;
    // no prior degradation → the degrade path runs.
    let mut cfg = ForgeConfig::default();
    cfg.learning.degradation_cooldown_days = 0;
    let monitor = DeploymentMonitor::new(cfg, registry.clone());

    let artifact = make_artifact("dep1", 10, ArtifactStatus::Active);
    let id = registry.add(artifact);

    let fetched = monitor.registry.get(&id).unwrap();
    monitor.try_deprecate(&fetched);
    let updated = monitor.registry.get(&id).unwrap();
    assert_eq!(updated.status, ArtifactStatus::Degraded);
    assert!(updated.last_degraded_at.is_some());
}

#[test]
fn test_s8_try_deprecate_within_cooldown_returns_early() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let mut cfg = ForgeConfig::default();
    cfg.learning.degradation_cooldown_days = 7;
    let monitor = DeploymentMonitor::new(cfg, registry.clone());

    let mut artifact = make_artifact("cool1", 10, ArtifactStatus::Active);
    artifact.last_degraded_at = Some(chrono::Local::now().to_rfc3339()); // just degraded
    let id = registry.add(artifact);

    let fetched = monitor.registry.get(&id).unwrap();
    monitor.try_deprecate(&fetched);
    let updated = monitor.registry.get(&id).unwrap();
    // Still in cooldown → status must remain Active.
    assert_eq!(updated.status, ArtifactStatus::Active);
}

#[test]
fn test_s8_track_observing_unknown_artifact_is_noop() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);

    // Artifact id not present in the registry → update is a no-op and the
    // re-read get() returns None without panicking.
    let orphan = make_artifact("ghost", 1, ArtifactStatus::Observing);
    monitor.track_observing(&orphan);
    assert!(monitor.registry.get("ghost").is_none());
}

#[test]
fn test_s8_run_evaluation_cycle_non_negative_verdict_no_degradation() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    // Low-usage observing artifact → non-negative verdict in the cycle;
    // no degradation attempted.
    registry.add(make_artifact("ok1", 2, ArtifactStatus::Observing));
    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);

    let results = monitor.run_evaluation_cycle();
    assert_eq!(results.len(), 1);
    assert_ne!(results[0].verdict, "negative");
    let a = monitor.registry.get(&results[0].artifact_id).unwrap();
    assert_ne!(a.status, ArtifactStatus::Degraded);
}

#[test]
fn test_s8_try_deprecate_unparseable_degraded_timestamp_still_degrades() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry.clone());

    // last_degraded_at present but unparseable → the parse-Err fall-through
    // must NOT block degradation.
    let mut artifact = make_artifact("badts", 10, ArtifactStatus::Active);
    artifact.last_degraded_at = Some("not-a-timestamp".to_string());
    let id = registry.add(artifact);

    let fetched = monitor.registry.get(&id).unwrap();
    monitor.try_deprecate(&fetched);
    let updated = monitor.registry.get(&id).unwrap();
    assert_eq!(updated.status, ArtifactStatus::Degraded);
}

// ---- S8 Wave 2 ----

/// try_deprecate with a last_degraded_at OLDER than the cooldown window: the
/// cooldown no longer blocks and the artifact is degraded again.
#[test]
fn test_s8_try_deprecate_expired_cooldown_degrades() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry.clone());

    let old = chrono::Local::now() - chrono::Duration::days(30);
    let mut artifact = make_artifact("cooldown-expired", 0, ArtifactStatus::Degraded);
    artifact.last_degraded_at = Some(old.to_rfc3339());
    artifact.consecutive_observing_rounds = 0;
    registry.add(artifact);

    monitor.try_deprecate(&registry.get("cooldown-expired").unwrap());

    let after = registry.get("cooldown-expired").unwrap();
    assert_eq!(after.status, ArtifactStatus::Degraded);
    // last_degraded_at was refreshed to now (within the last minute).
    let refreshed =
        chrono::DateTime::parse_from_rfc3339(after.last_degraded_at.as_deref().unwrap())
            .unwrap()
            .with_timezone(&chrono::Local);
    let delta = chrono::Local::now() - refreshed;
    assert!(delta.num_minutes() < 1, "delta: {:?}", delta);
}

// ---- build_conversation_traces（审计 P0-F2，2026-10-22）----

fn make_trace_event(
    event_type: &str,
    session_key: &str,
    timestamp: &str,
    data: serde_json::Value,
) -> crate::trace::TraceEvent {
    crate::trace::TraceEvent {
        id: uuid::Uuid::new_v4().to_string(),
        event_type: event_type.into(),
        session_key: session_key.into(),
        timestamp: timestamp.into(),
        data,
    }
}

#[test]
fn test_build_conversation_traces_empty_input() {
    let traces = build_conversation_traces(&[]);
    assert!(traces.is_empty());
}

#[test]
fn test_build_conversation_traces_single_session_multi_tools() {
    let events = vec![
        make_trace_event(
            "tool_call",
            "web:1",
            "2026-10-22T10:00:00+08:00",
            serde_json::json!({"tool": "file_read", "ok": true}),
        ),
        make_trace_event(
            "tool_call",
            "web:1",
            "2026-10-22T10:00:01+08:00",
            serde_json::json!({"tool": "exec", "ok": true}),
        ),
        make_trace_event(
            "tool_call",
            "web:1",
            "2026-10-22T10:00:02+08:00",
            serde_json::json!({"tool": "write_file", "ok": true}),
        ),
    ];
    let traces = build_conversation_traces(&events);
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].start_time, "2026-10-22T10:00:00+08:00");
    assert_eq!(traces[0].total_rounds, 3);
    assert_eq!(traces[0].duration_ms, 2000);
    let tools: Vec<&str> = traces[0]
        .tool_steps
        .iter()
        .map(|s| s.tool_name.as_str())
        .collect();
    assert_eq!(tools, vec!["file_read", "exec", "write_file"]);
    assert!(
        traces[0].signals.is_empty(),
        "all-ok session has no signals"
    );
}

#[test]
fn test_build_conversation_traces_failure_emits_tool_error_signal() {
    let events = vec![
        make_trace_event(
            "tool_call",
            "rpc:2",
            "2026-10-22T10:00:00+08:00",
            serde_json::json!({"tool": "exec", "ok": true}),
        ),
        make_trace_event(
            "tool_call",
            "rpc:2",
            "2026-10-22T10:00:01+08:00",
            serde_json::json!({"tool": "write_file", "ok": false}),
        ),
    ];
    let traces = build_conversation_traces(&events);
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].signals, vec!["tool_error".to_string()]);
    // The failed step is still a tool step.
    assert_eq!(traces[0].tool_steps.len(), 2);
}

#[test]
fn test_build_conversation_traces_groups_by_session_in_order() {
    let events = vec![
        make_trace_event(
            "tool_call",
            "web:a",
            "2026-10-22T10:00:00+08:00",
            serde_json::json!({"tool": "t1", "ok": true}),
        ),
        make_trace_event(
            "tool_call",
            "web:b",
            "2026-10-22T10:00:01+08:00",
            serde_json::json!({"tool": "t2", "ok": true}),
        ),
        make_trace_event(
            "tool_call",
            "web:a",
            "2026-10-22T10:00:02+08:00",
            serde_json::json!({"tool": "t3", "ok": true}),
        ),
    ];
    let traces = build_conversation_traces(&events);
    assert_eq!(traces.len(), 2);
    // Insertion order preserved: session "a" (first seen) before "b".
    assert_eq!(traces[0].start_time, "2026-10-22T10:00:00+08:00");
    assert_eq!(traces[0].total_rounds, 2);
    assert_eq!(traces[1].start_time, "2026-10-22T10:00:01+08:00");
    assert_eq!(traces[1].total_rounds, 1);
    assert_eq!(traces[1].tool_steps.len(), 1);
}

#[test]
fn test_build_conversation_traces_unparseable_timestamps_zero_duration() {
    let events = vec![
        make_trace_event(
            "tool_call",
            "s",
            "not-a-ts",
            serde_json::json!({"tool": "t", "ok": true}),
        ),
        make_trace_event(
            "tool_call",
            "s",
            "also-not-a-ts",
            serde_json::json!({"tool": "t", "ok": true}),
        ),
    ];
    let traces = build_conversation_traces(&events);
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].duration_ms, 0);
}

#[test]
fn test_build_conversation_traces_missing_tool_field_skips_step() {
    // tool_call event without a "tool" field: no tool step, but the event
    // still counts as a round.
    let events = vec![make_trace_event(
        "tool_call",
        "s",
        "2026-10-22T10:00:00+08:00",
        serde_json::json!({"ok": true}),
    )];
    let traces = build_conversation_traces(&events);
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].total_rounds, 1);
    assert!(traces[0].tool_steps.is_empty());
}

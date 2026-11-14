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
    let now = chrono::Local::now().to_rfc3339();
    ConversationTrace {
        start_time: now.clone(),
        total_rounds: rounds,
        duration_ms,
        tool_steps: tools
            .iter()
            .map(|t| ToolStep {
                tool_name: t.to_string(),
                ok: true,
                duration_ms: 0,
                ts: now.clone(),
            })
            .collect(),
        signals: if has_signals {
            vec!["retry".to_string()]
        } else {
            vec![]
        },
        round_ts: Vec::new(),
    }
}

/// 步级夹具：每步 ok/duration 可控——D2 步级池化语义（success_rate /
/// avg_duration 按步聚合）的最小夹具单元。
fn make_trace_with_steps(steps: &[(bool, i64)]) -> ConversationTrace {
    let now = chrono::Local::now().to_rfc3339();
    ConversationTrace {
        start_time: now.clone(),
        total_rounds: 1,
        duration_ms: steps.iter().map(|(_, d)| *d).sum(),
        tool_steps: steps
            .iter()
            .map(|(ok, d)| ToolStep {
                tool_name: "t".to_string(),
                ok: *ok,
                duration_ms: *d,
                ts: now.clone(),
            })
            .collect(),
        signals: vec![],
        round_ts: Vec::new(),
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
    // 步级池化（D2）：sum(ok steps) / sum(all steps)。轨迹 1 = 1 失败步，
    // 轨迹 2/3 = 各 1 成功步 → 2/3。旧轨迹级「无 signals = 成功」语义
    // （单步失败拖垮整轨 20 步成功）已废。
    let traces = [
        make_trace_with_steps(&[(false, 100)]),
        make_trace_with_steps(&[(true, 100)]),
        make_trace_with_steps(&[(true, 100)]),
    ];
    let refs: Vec<&ConversationTrace> = traces.iter().collect();
    assert_eq!(success_rate(&refs), 2.0 / 3.0);
}

#[test]
fn test_avg_duration() {
    // 步级池化（D2）：跨轨迹步级 duration 均值 = (100+200+300)/3 = 200。
    // 旧轨迹级 first→last span 均值已废（长会话单次工具失败拉爆整轨）。
    let traces = [
        make_trace_with_steps(&[(true, 100)]),
        make_trace_with_steps(&[(true, 200)]),
        make_trace_with_steps(&[(true, 300)]),
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
    let traces = [make_trace_with_steps(&[(true, 300)])];
    let refs: Vec<&ConversationTrace> = traces.iter().collect();
    assert_eq!(avg_duration(&refs), 300);
}

#[test]
fn test_success_rate_all_success() {
    let traces = [
        make_trace_with_steps(&[(true, 100)]),
        make_trace_with_steps(&[(true, 100)]),
    ];
    let refs: Vec<&ConversationTrace> = traces.iter().collect();
    assert_eq!(success_rate(&refs), 1.0);
}

#[test]
fn test_success_rate_all_failure() {
    let traces = [
        make_trace_with_steps(&[(false, 100)]),
        make_trace_with_steps(&[(false, 100)]),
    ];
    let refs: Vec<&ConversationTrace> = traces.iter().collect();
    assert_eq!(success_rate(&refs), 0.0);
}

#[test]
fn test_success_rate_empty() {
    // 零步（含空输入）= 无失败证据 → 1.0：零步轨迹（纯 LLM 对话）不进
    // 分母，也不把「没有工具数据」判成全败。
    let refs: Vec<&ConversationTrace> = vec![];
    assert_eq!(success_rate(&refs), 1.0);
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

// ---- coverage batch ---- (quality-hardening goal 冲刺)

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
            ok: true,
            duration_ms: 0,
            ts: "2020-01-01T00:00:00+08:00".to_string(),
        }],
        signals: vec![],
        round_ts: Vec::new(),
    };
    let outcomes = monitor.evaluate_outcomes(&[old_trace]);
    assert!(outcomes.is_empty());
}

#[test]
fn test_s8_evaluate_outcomes_window_respects_timestamp_offset() {
    // 验证轮二回归锁：窗口过滤必须全量偏移感知解析 start_time。
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
            ok: true,
            duration_ms: 0,
            ts: real.to_rfc3339(),
        }],
        signals: vec![],
        round_ts: Vec::new(),
    };

    let outcomes = monitor.evaluate_outcomes(&[trace]);
    // 窗内轨迹进入评估（样本数 < min_outcome_samples=5 → insufficient_data，
    // 但 outcome 本身必须产出——空 outcomes 意味着窗口过滤把轨迹丢了）。
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].verdict, "insufficient_data");
}

#[test]
fn test_s8_evaluate_outcomes_min_traces_guard() {
    // 验证轮四回归锁：`min_traces_for_analysis` 此前零消费者
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
            ok: true,
            duration_ms: 0,
            ts: old.to_rfc3339(),
        }],
        signals: vec![],
        round_ts: Vec::new(),
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
    // degradation_cooldown_days = 0（显式 = 关闭冷却；BUG-20261114-029：旧
    // 0→7 回落臂已删，serde default 本就是 7、0 唯一入口是显式配置）；
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

/// BUG-20261114-029 回归锁：显式 cooldown_days=0 + 刚降级过（last_degraded_at
/// 在旧 0→7 回落臂的冷却窗内）→ try_deprecate 必须**立即执行降级**（0 =
/// 用户关闭冷却），且与 is_degradation_cooldown_elapsed 判定一致（同键单
/// 一语义）。旧代码 fallback 把 0 静默改 7 → early return 不降级 = 本测试红。
#[test]
fn test_try_deprecate_explicit_zero_cooldown_disables_cooldown_window() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let mut cfg = ForgeConfig::default();
    cfg.learning.degradation_cooldown_days = 0;
    let monitor = DeploymentMonitor::new(cfg, registry.clone());

    let mut artifact = make_artifact("zerocd", 10, ArtifactStatus::Active);
    artifact.last_degraded_at = Some(chrono::Local::now().to_rfc3339()); // 刚降级
    let id = registry.add(artifact);

    let fetched = monitor.registry.get(&id).unwrap();
    // 两个消费面同键同语义：cooldown 闸都放行。
    assert!(
        monitor.is_degradation_cooldown_elapsed(&fetched),
        "explicit 0 must mean 'no cooldown' in is_degradation_cooldown_elapsed"
    );
    monitor.try_deprecate(&fetched);
    let updated = monitor.registry.get(&id).unwrap();
    assert_eq!(
        updated.status,
        ArtifactStatus::Degraded,
        "explicit cooldown_days=0 must disable the cooldown window (old 0→7 fallback silently overrode user config)"
    );
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

// ---- Wave 2 ----

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

// ---- build_conversation_traces（审计）----

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
    // D2 事件粒度：total_rounds 只数 llm_turn 事件（tool_call 不计轮——
    // 旧「事件总数代理」把工具密集会话的轮数灌水）；duration_ms = 会话
    // last−first 事件时间戳；ToolStep.duration_ms 从事件 data 提取。
    let events = vec![
        make_trace_event(
            "llm_turn",
            "web:1",
            "2026-10-22T10:00:00+08:00",
            serde_json::json!({}),
        ),
        make_trace_event(
            "tool_call",
            "web:1",
            "2026-10-22T10:00:01+08:00",
            serde_json::json!({"tool": "file_read", "ok": true, "duration_ms": 50}),
        ),
        make_trace_event(
            "llm_turn",
            "web:1",
            "2026-10-22T10:00:02+08:00",
            serde_json::json!({}),
        ),
        make_trace_event(
            "tool_call",
            "web:1",
            "2026-10-22T10:00:03+08:00",
            serde_json::json!({"tool": "exec", "ok": true}),
        ),
        make_trace_event(
            "llm_turn",
            "web:1",
            "2026-10-22T10:00:04+08:00",
            serde_json::json!({}),
        ),
        make_trace_event(
            "tool_call",
            "web:1",
            "2026-10-22T10:00:05+08:00",
            serde_json::json!({"tool": "write_file", "ok": true}),
        ),
    ];
    let traces = build_conversation_traces(&events);
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].start_time, "2026-10-22T10:00:00+08:00");
    assert_eq!(traces[0].total_rounds, 3, "rounds = llm_turn 事件数");
    assert_eq!(traces[0].round_ts.len(), 3);
    assert_eq!(traces[0].duration_ms, 5000);
    let tools: Vec<&str> = traces[0]
        .tool_steps
        .iter()
        .map(|s| s.tool_name.as_str())
        .collect();
    assert_eq!(tools, vec!["file_read", "exec", "write_file"]);
    assert_eq!(
        traces[0].tool_steps[0].duration_ms, 50,
        "步级 duration 来自事件 data"
    );
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
    // D2：各会话 total_rounds = 各自 llm_turn 事件数；插入序保持
    // （首次出现序）。
    let events = vec![
        make_trace_event(
            "llm_turn",
            "web:a",
            "2026-10-22T10:00:00+08:00",
            serde_json::json!({}),
        ),
        make_trace_event(
            "tool_call",
            "web:a",
            "2026-10-22T10:00:01+08:00",
            serde_json::json!({"tool": "t1", "ok": true}),
        ),
        make_trace_event(
            "llm_turn",
            "web:b",
            "2026-10-22T10:00:02+08:00",
            serde_json::json!({}),
        ),
        make_trace_event(
            "tool_call",
            "web:b",
            "2026-10-22T10:00:03+08:00",
            serde_json::json!({"tool": "t2", "ok": true}),
        ),
        make_trace_event(
            "llm_turn",
            "web:a",
            "2026-10-22T10:00:04+08:00",
            serde_json::json!({}),
        ),
        make_trace_event(
            "tool_call",
            "web:a",
            "2026-10-22T10:00:05+08:00",
            serde_json::json!({"tool": "t3", "ok": true}),
        ),
    ];
    let traces = build_conversation_traces(&events);
    assert_eq!(traces.len(), 2);
    // Insertion order preserved: session "a" (first seen) before "b".
    assert_eq!(traces[0].start_time, "2026-10-22T10:00:00+08:00");
    assert_eq!(traces[0].total_rounds, 2, "session a 收到 2 个 llm_turn");
    assert_eq!(traces[1].start_time, "2026-10-22T10:00:02+08:00");
    assert_eq!(traces[1].total_rounds, 1, "session b 收到 1 个 llm_turn");
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
    // D2 新语义三面覆盖：缺 tool 字段的 tool_call 不产步也不计轮
    // （total_rounds 只数 llm_turn——旧「事件总数代理」已废）；
    // 正常 tool_call 只产步不计轮；llm_turn 只计轮不产步。
    let events = vec![
        make_trace_event(
            "tool_call",
            "s",
            "2026-10-22T10:00:00+08:00",
            serde_json::json!({"ok": true}),
        ),
        make_trace_event(
            "tool_call",
            "s",
            "2026-10-22T10:00:01+08:00",
            serde_json::json!({"tool": "t", "ok": true}),
        ),
        make_trace_event(
            "llm_turn",
            "s",
            "2026-10-22T10:00:02+08:00",
            serde_json::json!({}),
        ),
    ];
    let traces = build_conversation_traces(&events);
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].total_rounds, 1, "只有 llm_turn 计轮");
    assert_eq!(traces[0].tool_steps.len(), 1);
    assert_eq!(traces[0].tool_steps[0].tool_name, "t");
}

#[test]
fn test_evaluate_outcomes_insufficient_does_not_overwrite_efficacy() {
    // 回归锁：evaluate_outcomes 此前对 insufficient_data outcome 无条件
    // 写回 last_efficacy——滚动窗口把 after 样本滚出窗后（样本 <
    // min_samples），无结论快照（verdict=insufficient_data + success 全 0
    // 假零）会周期性覆盖上次有效 verdict 快照，Dashboard 实效列与裁决数据
    // 源丢历史证据。修复 = 写回前加 verdict 闸，与 handle_verdict 对同一
    // outcome 的 `_ =>` 忽略语义对齐；outcome 本身仍进返回值（可观测）。
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let mut cfg = ForgeConfig::default();
    cfg.learning.min_outcome_samples = 0; // → 默认下限 5 生效
    let monitor = DeploymentMonitor::new(cfg, registry.clone());

    let mut artifact = make_artifact("eff1", 10, ArtifactStatus::Active);
    artifact.tool_signature = vec!["file_read".to_string()];
    let id = registry.add(artifact);

    // 5 条足样本（全部成功、无信号）。D3 事件级切分后全部样本落
    // after 桶（步 ts = now > created_at 部署点），before 桶空 →
    // 空输入约定 success_before = 1.0 → norm_success = 0，
    // norm_rounds/norm_duration 在 before ≤ 0 时归零 → improvement = 0
    // → neutral。neutral 仍是有效结论 → 快照写回（只有
    // insufficient_data 不写回）。旧「空 before → improvement 0.4 →
    // positive」语义已随 D1/D3 废除。
    let good_traces: Vec<_> = (0..5)
        .map(|_| make_trace(2, 100, &["file_read"], false))
        .collect();
    let outcomes = monitor.evaluate_outcomes(&good_traces);
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].verdict, "neutral");
    let first = monitor
        .registry
        .get(&id)
        .unwrap()
        .last_efficacy
        .clone()
        .expect("有效评估必须写回快照");
    assert_eq!(first.verdict, "neutral");

    // 一条不匹配签名的轨迹（recent_traces 非空——零轨迹会在窗口
    // 过滤处提前返回，走不到 artifact 循环）→ after_traces 空 →
    // insufficient_data outcome 照常产出，但快照必须保持的值。
    let mismatched = vec![make_trace(2, 100, &["other_tool"], false)];
    let outcomes = monitor.evaluate_outcomes(&mismatched);
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].verdict, "insufficient_data");
    let after = monitor
        .registry
        .get(&id)
        .unwrap()
        .last_efficacy
        .clone()
        .expect("快照不应被 insufficient_data 清除");
    assert_eq!(after.verdict, first.verdict, "无结论测量不得覆盖有效快照");
    assert_eq!(after.measured_at, first.measured_at);
    assert_eq!(after.samples_after, first.samples_after);
}

// ---- 缺陷整改批次新增单测（D1 跨偏移 / D3 长会话跨部署点）----

#[test]
fn test_ts_before_cross_offset_moment_order() {
    // D1：双侧可解析 = 时刻序（跨时区偏移书写正确分桶）。
    // 2026-10-22T10:00:00+08:00 与 2026-10-22T02:00:00Z 是同一时刻 →
    // 不严格早于（分桶落在 after 侧）。
    assert!(!ts_before(
        "2026-10-22T10:00:00+08:00",
        "2026-10-22T02:00:00Z"
    ));
    // Z 书写的更早时刻 < +08:00 书写的部署点 → 时刻序判早（字符串序会
    // 误判 "2026-10-22T01:59:59Z" > "2026-10-22T10:00:00+08:00"，因为
    // 字符 '0' < '1'——这正是 D1 要消灭的误分桶）。
    assert!(ts_before(
        "2026-10-22T01:59:59Z",
        "2026-10-22T10:00:00+08:00"
    ));
    // 反向：+08:00 书写的早时刻 < Z 书写的部署点。
    assert!(ts_before(
        "2026-10-22T09:00:00+08:00",
        "2026-10-22T02:00:00Z"
    ));
    // 任一侧不可解析 → 回落字符串序（同偏移写入下字符串序 == 时刻序，
    // 正确性不劣于旧实现）。
    assert_eq!(
        ts_before("2026-10-22T09:00:00+08:00", "not-a-ts"),
        "2026-10-22T09:00:00+08:00" < "not-a-ts"
    );
    assert!(!ts_before("not-a-ts", "2026-10-22T09:00:00+08:00"));
}

#[test]
fn test_evaluate_artifact_splits_long_session_across_deploy_point() {
    // D3 事件级切分：长命会话跨部署点 → 单条 matched trace 的
    // tool_steps / round_ts 按 deploy_time 拆 ≤2 条子轨迹，两侧非空各自
    // 成样本（旧行为整条轨迹挂 before 名下，稀释 after 指标）。
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let mut cfg = ForgeConfig::default();
    cfg.learning.min_outcome_samples = 1; // after 1 条子轨迹 ≥ 1 → 足样本
    let monitor = DeploymentMonitor::new(cfg, registry.clone());

    let deploy = chrono::Local::now() - chrono::Duration::hours(1);
    let mut artifact = make_artifact("d3", 10, ArtifactStatus::Active);
    artifact.tool_signature = vec!["file_read".to_string()];
    // 部署点必须走 deployed_at（F 线优先锚）：Registry::add 无条件重盖
    // created_at = now，回落锚在测试夹具里不可用。
    artifact.deployed_at = Some(deploy.to_rfc3339());
    let id = registry.add(artifact);

    let early = (deploy - chrono::Duration::minutes(10)).to_rfc3339();
    let late = (deploy + chrono::Duration::minutes(10)).to_rfc3339();
    let long_session = ConversationTrace {
        start_time: early.clone(),
        total_rounds: 3, // 母轨迹值仅描述性；分桶后 total_rounds = round_ts 计数
        duration_ms: 0,
        tool_steps: vec![
            ToolStep {
                tool_name: "file_read".into(),
                ok: true,
                duration_ms: 100,
                ts: early.clone(),
            },
            ToolStep {
                tool_name: "file_read".into(),
                ok: false,
                duration_ms: 200,
                ts: early.clone(),
            },
            ToolStep {
                tool_name: "file_read".into(),
                ok: true,
                duration_ms: 50,
                ts: late.clone(),
            },
            ToolStep {
                tool_name: "file_read".into(),
                ok: true,
                duration_ms: 60,
                ts: late.clone(),
            },
            ToolStep {
                tool_name: "file_read".into(),
                ok: true,
                duration_ms: 70,
                ts: late.clone(),
            },
        ],
        round_ts: vec![early.clone(), late.clone(), late.clone()],
        signals: vec![],
    };
    let traces = vec![long_session];
    let outcomes = monitor.evaluate_outcomes(&traces);
    assert_eq!(outcomes.len(), 1);
    let o = &outcomes[0];
    // min_samples / sample_size 的计数单位是「子轨迹条数」（每条 matched
    // 母轨迹每侧 ≤1 条子轨迹）——与旧轨迹级语义同单位；步/轮聚合体现在
    // 下方指标值断言（success 按步级池化 / rounds avg 按 round_ts 计数）。
    assert_eq!(o.sample_size, 1, "after 桶 = 部署点后事件切出的 1 条子轨迹");
    assert_eq!(
        o.sample_size_before, 1,
        "before 桶 = 部署点前事件切出的 1 条子轨迹"
    );
    // 步级池化（D2）：before 1/2 成功；after 3/3 成功。
    assert!((o.success_before - 0.5).abs() < 1e-9);
    assert!((o.success_after - 1.0).abs() < 1e-9);
    // 拆桶后轮数 = 各侧 round_ts 计数（before 1 轮 / after 2 轮），
    // 不再继承母轨迹 total_rounds。
    assert!((o.rounds_before_avg - 1.0).abs() < 1e-9);
    assert!((o.rounds_after_avg - 2.0).abs() < 1e-9);
    // 有效结论 → 快照写回（非 insufficient_data）。
    let snap = registry
        .get(&id)
        .unwrap()
        .last_efficacy
        .clone()
        .expect("D3 拆桶评估应写回快照");
    assert_eq!(snap.samples_after, 1);
    assert_eq!(snap.samples_before, 1);
}

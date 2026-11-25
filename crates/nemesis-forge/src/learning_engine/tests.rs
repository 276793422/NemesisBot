use super::*;
use crate::types::{Experience, RegistryConfig};

fn make_experience(tool: &str, success: bool) -> Experience {
    Experience {
        id: uuid::Uuid::new_v4().to_string(),
        tool_name: tool.into(),
        input_summary: "test".into(),
        output_summary: if success { "ok" } else { "err" }.into(),
        success,
        duration_ms: 100,
        timestamp: chrono::Local::now().to_rfc3339(),
        session_key: "test".into(),
    }
}

fn make_collected(tool: &str, success: bool) -> CollectedExperience {
    CollectedExperience {
        experience: make_experience(tool, success),
        dedup_hash: format!("hash-{}-{}", tool, success),
    }
}

fn make_collected_with_duration(tool: &str, success: bool, duration: u64) -> CollectedExperience {
    let mut exp = make_experience(tool, success);
    exp.duration_ms = duration;
    CollectedExperience {
        experience: exp,
        dedup_hash: format!("hash-{}-{}-{}", tool, success, duration),
    }
}

/// 跨会话构造：session_key 可指定——会话级检测器的最小夹具单元。
/// dedup_hash 复用经验 uuid，恒唯一（同参数多条经验不互撞）。
fn make_session_experience(
    session: &str,
    tool: &str,
    success: bool,
    duration_ms: u64,
) -> CollectedExperience {
    let mut exp = make_experience(tool, success);
    exp.session_key = session.into();
    exp.duration_ms = duration_ms;
    let dedup_hash = format!("hash-{}", exp.id);
    CollectedExperience {
        experience: exp,
        dedup_hash,
    }
}

#[test]
fn test_ff1_adjust_confidence_uses_success_rate_not_usage_count() {
    // F-: feedback must adjust the dedicated success_rate field, not corrupt
    // usage_count (the old code treated the integer count as a rate).
    use crate::monitor::EvaluationResult;
    use nemesis_types::forge::{Artifact, ArtifactKind, ArtifactStatus};
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    registry.add(Artifact {
        id: "skill-x".into(),
        name: "x".into(),
        kind: ArtifactKind::Skill,
        version: "1.0".into(),
        status: ArtifactStatus::Active,
        content: "...".into(),
        tool_signature: vec![],
        created_at: "2026-01-01T00:00:00+08:00".into(),
        updated_at: "2026-01-01T00:00:00+08:00".into(),
        usage_count: 42,
        last_degraded_at: None,
        success_rate: 0.5,
        consecutive_observing_rounds: 0,
        deployed_at: None,
        last_efficacy: None,
    });
    let cycle_store = CycleStore::new(dir.path());
    let engine = LearningEngine::new(ForgeConfig::default(), registry.clone(), cycle_store);
    engine.adjust_confidence_for_test(&[EvaluationResult {
        artifact_id: "skill-x".into(),
        verdict: "positive".into(),
        improvement_score: 0.5,
        sample_size: 10,
    }]);
    let a = registry.get("skill-x").expect("artifact present");
    assert!(
        (a.success_rate - 0.6).abs() < 1e-9,
        "success_rate should be 0.6, got {}",
        a.success_rate
    );
    assert_eq!(a.usage_count, 42, "usage_count must be untouched (F-F1)");
}

#[test]
fn test_ff2_disable_degraded_skill_hides_deployed_file() {
    // F-: a Degraded skill's deployed SKILL.md must be renamed to.disabled
    // so the skills loader (workspace/skills/*/SKILL.md) stops loading it.
    use nemesis_types::forge::{Artifact, ArtifactKind, ArtifactStatus};
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path();
    let forge_dir = workspace.join("forge");
    std::fs::create_dir_all(&forge_dir).unwrap();
    let skill_dir = workspace.join("skills").join("mybad-forge");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "---\nname: mybad\n---\nbody").unwrap();

    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    registry.add(Artifact {
        id: "skill-mybad".into(),
        name: "mybad".into(),
        kind: ArtifactKind::Skill,
        version: "1.0".into(),
        status: ArtifactStatus::Degraded,
        content: "...".into(),
        tool_signature: vec![],
        created_at: "2026-01-01T00:00:00+08:00".into(),
        updated_at: "2026-01-01T00:00:00+08:00".into(),
        usage_count: 0,
        last_degraded_at: None,
        success_rate: 0.0,
        consecutive_observing_rounds: 0,
        deployed_at: None,
        last_efficacy: None,
    });
    let cycle_store = CycleStore::new(&forge_dir);
    let engine =
        LearningEngine::with_forge_dir(ForgeConfig::default(), forge_dir, registry, cycle_store);
    engine.disable_degraded_skills_impl();

    assert!(
        !skill_dir.join("SKILL.md").exists(),
        "degraded skill must be disabled"
    );
    assert!(
        skill_dir.join("SKILL.md.disabled").exists(),
        "should be renamed to .disabled"
    );
}

#[test]
fn test_disable_degraded_poison_name_contained() {
    // N 回归钉：disable 面的 a.name 来自磁盘态注册表，历史遗留中毒名直拼
    // join（`../../evil` → `<tmp>/evil-forge/`）会把 workspace 外同名目录的
    // SKILL.md 禁用掉（rename = 破坏面）。修复后过统一 slug+占位规则——毒
    // 名塌缩为安全段，目录不存在诚实跳过；workspace 外的陷阱文件原样保留。
    use nemesis_types::forge::{Artifact, ArtifactKind, ArtifactStatus};
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path();
    let forge_dir = workspace.join("forge");
    std::fs::create_dir_all(&forge_dir).unwrap();

    // 穿越目标陷阱：`workspace/skills/../../evil-forge/SKILL.md` = `<tmp>/evil-forge/SKILL.md`
    let trap = dir.path().join("evil-forge");
    std::fs::create_dir_all(&trap).unwrap();
    std::fs::write(trap.join("SKILL.md"), "trap").unwrap();

    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    registry.add(Artifact {
        id: "skill-poison".into(),
        name: "../../evil".into(),
        kind: ArtifactKind::Skill,
        version: "1.0".into(),
        status: ArtifactStatus::Degraded,
        content: "...".into(),
        tool_signature: vec![],
        created_at: "2026-01-01T00:00:00+08:00".into(),
        updated_at: "2026-01-01T00:00:00+08:00".into(),
        usage_count: 0,
        last_degraded_at: None,
        success_rate: 0.0,
        consecutive_observing_rounds: 0,
        deployed_at: None,
        last_efficacy: None,
    });
    let cycle_store = CycleStore::new(&forge_dir);
    let engine =
        LearningEngine::with_forge_dir(ForgeConfig::default(), forge_dir, registry, cycle_store);
    engine.disable_degraded_skills_impl();

    assert!(
        trap.join("SKILL.md").exists() && !trap.join("SKILL.md.disabled").exists(),
        "outside-workspace trap file must stay untouched"
    );
    // 塌缩后的安全段目录（`skills/evil-forge`）也不存在——该毒名条目从未
    // 真实部署过，disable 面诚实跳过，不留幻影目录
    assert!(
        !workspace.join("skills").join("evil-forge").exists(),
        "collapsed segment dir must not be created (honest skip)"
    );
}
#[test]
fn test_fm2a_classify_verdict_respects_configured_threshold() {
    // F-: when degrade_threshold is configured (e.g. 0.5), classify_verdict
    // must respect it. Old code forced -0.2 whenever threshold >= 0.0.
    use crate::monitor::DeploymentMonitor;
    use nemesis_types::forge::{Artifact, ArtifactKind, ArtifactStatus};
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let mut config = ForgeConfig::default();
    config.learning.degrade_threshold = 0.5;
    let monitor = DeploymentMonitor::new(config, registry);
    let artifact = Artifact {
        id: "x".into(),
        name: "x".into(),
        kind: ArtifactKind::Skill,
        version: "1.0".into(),
        status: ArtifactStatus::Active,
        content: "".into(),
        tool_signature: vec![],
        created_at: "".into(),
        updated_at: "".into(),
        usage_count: 0,
        last_degraded_at: None,
        success_rate: 0.0,
        consecutive_observing_rounds: 0,
        deployed_at: None,
        last_efficacy: None,
    };
    // improvement=-0.15: with threshold=0.5 → "negative" (new, respects config);
    // old code forced -0.2 → "observing" (ignored user config).
    assert_eq!(
        monitor.classify_verdict(-0.15, &artifact),
        "negative",
        "threshold=0.5 should make -0.15 'negative', not 'observing'"
    );
}

#[test]
fn test_bug20261113_161_explicit_zero_threshold_respected() {
    // BUG-20261113-161：显式 degrade_threshold=0.0 是合法最严档（observing
    // 缓冲带 [-0.1, 0) 为空），旧守卫把它当「未配置默认值」静默回落 -0.2
    // （observing 带 [-0.2, -0.1) 复活）——serde default 自 INIT 起就是
    // -0.2，`== 0.0` 唯一入口是显式配置。improvement=-0.15：修复后应
    // "negative"（尊重 0.0），旧代码 → "observing"（被 -0.2 覆盖）。
    use crate::monitor::DeploymentMonitor;
    use nemesis_types::forge::{Artifact, ArtifactKind, ArtifactStatus};
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let mut config = ForgeConfig::default();
    config.learning.degrade_threshold = 0.0;
    let monitor = DeploymentMonitor::new(config, registry);
    let artifact = Artifact {
        id: "x".into(),
        name: "x".into(),
        kind: ArtifactKind::Skill,
        version: "1.0".into(),
        status: ArtifactStatus::Active,
        content: "".into(),
        tool_signature: vec![],
        created_at: "".into(),
        updated_at: "".into(),
        usage_count: 0,
        last_degraded_at: None,
        success_rate: 0.0,
        consecutive_observing_rounds: 0,
        deployed_at: None,
        last_efficacy: None,
    };
    assert_eq!(
        monitor.classify_verdict(-0.15, &artifact),
        "negative",
        "explicit threshold=0.0 is the strictest setting; -0.15 must be 'negative'"
    );
}

#[tokio::test]
async fn test_run_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    // 会话级语义：5 个独立会话各跑 (file_read→file_write) 全成功链
    // → tool_chain freq 5 ≥ min_freq 5。
    let exps: Vec<CollectedExperience> = (0..5)
        .flat_map(|i| {
            vec![
                make_session_experience(&format!("s{i}"), "file_read", true, 100),
                make_session_experience(&format!("s{i}"), "file_write", true, 100),
            ]
        })
        .collect();

    let cycle = engine.run_cycle(&exps).await;
    assert!(cycle.patterns_found > 0);
    assert_eq!(cycle.status, nemesis_types::forge::CycleStatus::Completed);
}

#[test]
fn test_extract_patterns() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    // 会话级语义：5 个会话各一次 (失败 → 成功恢复，不同工具) →
    // error_recovery freq 5 ≥ min_freq 5。
    let exps: Vec<CollectedExperience> = (0..5)
        .flat_map(|i| {
            vec![
                make_session_experience(&format!("s{i}"), "tool_a", false, 100),
                make_session_experience(&format!("s{i}"), "tool_b", true, 100),
            ]
        })
        .collect();

    let patterns = engine.extract_patterns(&exps);
    assert!(!patterns.is_empty());
}
#[test]
fn test_detect_efficiency_issue() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let mut config = ForgeConfig::default();
    config.learning.min_pattern_frequency = 3; // Lower threshold for test
    let engine = LearningEngine::new(config, registry, cycle_store);

    // 会话级语义：9 个快会话（1 条 @10ms）+ 5 个慢会话（1 条 @1000ms）。
    // 全局 avg = (9×10 + 5×1000) / 14 ≈ 363.6；慢会话 1000 > 2×363.6 ✓。
    let mut exps = Vec::new();
    for i in 0..9 {
        exps.push(make_session_experience(
            &format!("fast-s{i}"),
            "fast_tool",
            true,
            10,
        ));
    }
    for i in 0..5 {
        exps.push(make_session_experience(
            &format!("slow-s{i}"),
            "slow_tool",
            true,
            1000,
        ));
    }

    let patterns = engine.extract_patterns(&exps);
    let efficiency: Vec<_> = patterns
        .iter()
        .filter(|p| p.pattern_type == "efficiency_issue")
        .collect();
    assert!(
        !efficiency.is_empty(),
        "Expected efficiency issue patterns, got: {:?}",
        patterns
    );
    assert!(efficiency[0].description.contains("slow_tool"));
    // 置信度公式钉：轮数基 eff_score = 1 - actual_rounds/(2×global_avg_rounds)。
    // 本夹具每会话 1 条经验 → actual_rounds=1、global_avg_rounds=14/14=1
    // → eff_score = 1 - 1/2 = 0.5。旧时长基公式已废。
    assert!(
        efficiency[0].confidence < 1.0,
        "confidence must not saturate at 1.0, got {}",
        efficiency[0].confidence
    );
    assert!(
        (efficiency[0].confidence - 0.5).abs() < 1e-9,
        "expected exactly 0.5 (= 1 - 1/(2×1.0)), got {}",
        efficiency[0].confidence
    );
}

#[test]
fn test_detect_success_template() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    // 会话级语义：5 个独立会话各 1 条完美成功经验 → 同形链
    // "perfect_tool" freq 5 ≥ 5 → success_template，conf ≡ 1.0。
    let exps: Vec<CollectedExperience> = (0..5)
        .map(|i| make_session_experience(&format!("p{i}"), "perfect_tool", true, 100))
        .collect();

    let patterns = engine.extract_patterns(&exps);
    let success: Vec<_> = patterns
        .iter()
        .filter(|p| p.pattern_type == "success_template")
        .collect();
    assert!(!success.is_empty());
    assert_eq!(success[0].confidence, 1.0);
}

#[test]
fn test_detect_all_four_pattern_types() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let mut config = ForgeConfig::default();
    config.learning.min_pattern_frequency = 3; // Lower threshold for test
    let engine = LearningEngine::new(config, registry, cycle_store);

    let mut exps = Vec::new();

    // tool_chain: 10 个会话各跑同形两步全成功链
    for i in 0..10 {
        exps.push(make_session_experience(
            &format!("c{i}"),
            "chain_tool",
            true,
            100,
        ));
        exps.push(make_session_experience(
            &format!("c{i}"),
            "chain_helper",
            true,
            100,
        ));
    }
    // error_recovery: 5 个会话各一次 (失败 → 成功恢复，不同工具)
    for i in 0..5 {
        exps.push(make_session_experience(
            &format!("e{i}"),
            "flaky_tool",
            false,
            100,
        ));
        exps.push(make_session_experience(
            &format!("e{i}"),
            "fixer_tool",
            true,
            100,
        ));
    }
    // efficiency_issue: 9 个快会话（1 条 @10ms）+ 5 个慢会话（1 条 @1000ms）
    //   全局 avg = (9×10 + 5×1000) / 14 ≈ 363.6；1000 > 2×363.6 ✓
    for i in 0..9 {
        exps.push(make_session_experience(&format!("f{i}"), "fast", true, 10));
    }
    for i in 0..5 {
        exps.push(make_session_experience(
            &format!("slow{i}"),
            "slow_tool",
            true,
            1000,
        ));
    }
    // success_template: 5 个会话各 1 条完美成功
    for i in 0..5 {
        exps.push(make_session_experience(
            &format!("p{i}"),
            "perfect",
            true,
            100,
        ));
    }

    let patterns = engine.extract_patterns(&exps);
    let types: std::collections::HashSet<&str> =
        patterns.iter().map(|p| p.pattern_type.as_str()).collect();

    assert!(
        types.contains("tool_chain"),
        "Should detect tool_chain, found: {:?}",
        types
    );
    assert!(
        types.contains("error_recovery"),
        "Should detect error_recovery, found: {:?}",
        types
    );
    assert!(
        types.contains("efficiency_issue"),
        "Should detect efficiency_issue, found: {:?}",
        types
    );
    assert!(
        types.contains("success_template"),
        "Should detect success_template, found: {:?}",
        types
    );
}
#[test]
fn test_evaluate_outcomes() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));

    use nemesis_types::forge::{Artifact, ArtifactKind, ArtifactStatus};
    let artifact = Artifact {
        id: "test-artifact".to_string(),
        name: "test".to_string(),
        kind: ArtifactKind::Skill,
        version: "1.0".to_string(),
        status: ArtifactStatus::Active,
        content: "test".to_string(),
        tool_signature: vec!["tool_a".to_string()],
        created_at: chrono::Local::now().to_rfc3339(),
        updated_at: chrono::Local::now().to_rfc3339(),
        usage_count: 10,
        last_degraded_at: None,
        success_rate: 0.0,
        consecutive_observing_rounds: 0,
        deployed_at: None,
        last_efficacy: None,
    };
    registry.add(artifact);

    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry.clone(), cycle_store);

    let outcomes = vec![DeploymentOutcome {
        artifact_id: "test-artifact".to_string(),
        verdict: "positive".to_string(),
        improvement_score: 0.5,
        sample_size: 10,
    }];

    engine.evaluate_outcomes(&outcomes);
    // Should not panic, artifact should still exist
    assert!(registry.get("test-artifact").is_some());
    // 回归钉：positive 评测必须真实调整 success_rate
    // （0.0 + 0.1 = 0.1）——不再是无声 no-op（旧实现 `let _ = new_rate`）。
    let art = registry.get("test-artifact").unwrap();
    assert!(
        (art.success_rate - 0.1).abs() < 1e-9,
        "positive 评测后 success_rate 应为 0.1，实际 {}",
        art.success_rate
    );
}

#[test]
fn test_empty_experiences() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    let patterns = engine.extract_patterns(&[]);
    assert!(patterns.is_empty());
}

// --- Additional learning_engine tests ---

#[tokio::test]
async fn test_run_cycle_empty_experiences() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    let cycle = engine.run_cycle(&[]).await;
    assert_eq!(cycle.patterns_found, 0);
    assert_eq!(cycle.status, nemesis_types::forge::CycleStatus::Completed);
}

#[tokio::test]
async fn test_run_cycle_persists() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path());
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    let exps: Vec<CollectedExperience> = (0..3).map(|_| make_collected("tool", true)).collect();
    let cycle = engine.run_cycle(&exps).await;
    assert!(!cycle.id.is_empty());
    assert!(!cycle.started_at.is_empty());
    assert!(cycle.completed_at.is_some());
}

#[tokio::test]
async fn test_get_latest_cycle_initially_none() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);
    assert!(engine.get_latest_cycle().is_none());
}

#[tokio::test]
async fn test_get_latest_cycle_after_run() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path());
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);
    engine.run_cycle(&[]).await;
    assert!(engine.get_latest_cycle().is_some());
}

#[test]
fn test_extract_patterns_tool_chain_detection() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    // 会话级语义：10 个会话各跑同形两步全成功链 → freq 10 ≥ 5。
    let exps: Vec<CollectedExperience> = (0..10)
        .flat_map(|i| {
            vec![
                make_session_experience(&format!("s{i}"), "frequent_tool", true, 100),
                make_session_experience(&format!("s{i}"), "helper_tool", true, 100),
            ]
        })
        .collect();

    let patterns = engine.extract_patterns(&exps);
    assert!(patterns.iter().any(|p| p.pattern_type == "tool_chain"));
    let tc = patterns
        .iter()
        .find(|p| p.pattern_type == "tool_chain")
        .unwrap();
    assert!(tc.frequency >= 3);
    assert!(tc.confidence > 0.0);
}

#[test]
fn test_extract_patterns_error_recovery_detection() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    // 会话级语义：5 个会话各一次 (失败 → 成功恢复，不同工具)。
    let exps: Vec<CollectedExperience> = (0..5)
        .flat_map(|i| {
            vec![
                make_session_experience(&format!("s{i}"), "flaky", false, 100),
                make_session_experience(&format!("s{i}"), "retry_helper", true, 100),
            ]
        })
        .collect();

    let patterns = engine.extract_patterns(&exps);
    assert!(patterns.iter().any(|p| p.pattern_type == "error_recovery"));
}

#[test]
fn test_extract_patterns_sorted_by_confidence() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    // 形状 A：5 个会话 × (tool_a→tool_b) 全成功 → conf 1.0；
    // 形状 B：5 个会话 × (tool_c 失败 → tool_d 成功) → tool_chain
    // success_rate 0.5 → conf 0.5、error_recovery conf 1.0。
    let exps: Vec<CollectedExperience> = (0..5)
        .flat_map(|i| {
            vec![
                make_session_experience(&format!("a{i}"), "tool_a", true, 100),
                make_session_experience(&format!("a{i}"), "tool_b", true, 100),
            ]
        })
        .chain((0..5).flat_map(|i| {
            vec![
                make_session_experience(&format!("b{i}"), "tool_c", false, 100),
                make_session_experience(&format!("b{i}"), "tool_d", true, 100),
            ]
        }))
        .collect();

    let patterns = engine.extract_patterns(&exps);
    assert!(patterns.len() >= 2, "expected multiple patterns");
    for i in 1..patterns.len() {
        assert!(patterns[i - 1].confidence >= patterns[i].confidence);
    }
}
#[test]
fn test_detect_tool_chains_min_frequency() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    // 会话级语义：单会话 2 条同工具经验 → 整会话一条链 freq 1
    // < min_pattern_frequency (5) → 零模式。
    let exps = vec![
        make_collected("rare_tool", true),
        make_collected("rare_tool", true),
    ];
    let patterns = engine.extract_patterns(&exps);
    assert!(patterns.is_empty());
}

#[test]
fn test_detect_error_recovery_no_errors() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    let exps: Vec<CollectedExperience> = (0..5).map(|_| make_collected("perfect", true)).collect();
    let patterns = engine.extract_patterns(&exps);
    assert!(!patterns.iter().any(|p| p.pattern_type == "error_recovery"));
}

#[test]
fn test_detect_success_template_with_failure() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    let mut exps: Vec<CollectedExperience> = (0..5)
        .map(|_| make_collected("almost_perfect", true))
        .collect();
    exps.push(make_collected("almost_perfect", false));

    let patterns = engine.extract_patterns(&exps);
    let success: Vec<_> = patterns
        .iter()
        .filter(|p| {
            p.pattern_type == "success_template" && p.tools.contains(&"almost_perfect".to_string())
        })
        .collect();
    assert!(
        success.is_empty(),
        "Should not be success template if any failure"
    );
}

#[test]
fn test_deployment_outcome_fields() {
    let outcome = DeploymentOutcome {
        artifact_id: "art-123".into(),
        verdict: "positive".into(),
        improvement_score: 0.75,
        sample_size: 10,
    };
    assert_eq!(outcome.artifact_id, "art-123");
    assert_eq!(outcome.verdict, "positive");
    assert_eq!(outcome.improvement_score, 0.75);
    assert_eq!(outcome.sample_size, 10);
}

#[test]
fn test_pattern_summary_fields() {
    let summary = PatternSummary {
        id: "p-abc".into(),
        pattern_type: "tool_chain".into(),
        fingerprint: "sha256:abc".into(),
        frequency: 15,
        confidence: 0.9,
    };
    assert_eq!(summary.id, "p-abc");
    assert_eq!(summary.frequency, 15);
}
#[tokio::test]
async fn test_run_cycle_with_forge_dir() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::with_forge_dir(
        ForgeConfig::default(),
        dir.path().to_path_buf(),
        registry,
        cycle_store,
    );
    let exps: Vec<CollectedExperience> = (0..3).map(|_| make_collected("tool", true)).collect();
    let cycle = engine.run_cycle(&exps).await;
    assert_eq!(cycle.status, nemesis_types::forge::CycleStatus::Completed);
}

#[test]
fn test_adjust_confidence_for_test_positive() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let artifact = nemesis_types::forge::Artifact {
        id: "adj-test".into(),
        name: "test".into(),
        kind: nemesis_types::forge::ArtifactKind::Skill,
        version: "1.0".into(),
        status: nemesis_types::forge::ArtifactStatus::Active,
        content: "test".into(),
        tool_signature: vec![],
        created_at: chrono::Local::now().to_rfc3339(),
        updated_at: chrono::Local::now().to_rfc3339(),
        usage_count: 50,
        last_degraded_at: None,
        success_rate: 0.0,
        consecutive_observing_rounds: 0,
        deployed_at: None,
        last_efficacy: None,
    };
    registry.add(artifact);
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry.clone(), cycle_store);

    let outcomes = vec![crate::monitor::EvaluationResult {
        artifact_id: "adj-test".into(),
        improvement_score: 0.5,
        verdict: "positive".into(),
        sample_size: 10,
    }];
    engine.adjust_confidence_for_test(&outcomes);
    // Should not panic, artifact should still exist
    assert!(registry.get("adj-test").is_some());
}

#[test]
fn test_evaluate_outcomes_empty_artifact_id() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    let outcomes = vec![DeploymentOutcome {
        artifact_id: String::new(),
        verdict: "positive".into(),
        improvement_score: 0.5,
        sample_size: 10,
    }];
    // Should not panic
    engine.evaluate_outcomes(&outcomes);
}

#[test]
fn test_evaluate_outcomes_unknown_verdict() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let artifact = nemesis_types::forge::Artifact {
        id: "unk-verdict".into(),
        name: "test".into(),
        kind: nemesis_types::forge::ArtifactKind::Skill,
        version: "1.0".into(),
        status: nemesis_types::forge::ArtifactStatus::Active,
        content: "test".into(),
        tool_signature: vec![],
        created_at: chrono::Local::now().to_rfc3339(),
        updated_at: chrono::Local::now().to_rfc3339(),
        usage_count: 50,
        last_degraded_at: None,
        success_rate: 0.0,
        consecutive_observing_rounds: 0,
        deployed_at: None,
        last_efficacy: None,
    };
    registry.add(artifact);
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry.clone(), cycle_store);

    let outcomes = vec![DeploymentOutcome {
        artifact_id: "unk-verdict".into(),
        verdict: "unknown_verdict".into(),
        improvement_score: 0.0,
        sample_size: 5,
    }];
    engine.evaluate_outcomes(&outcomes);
    // Unknown verdict should not change usage_count
    let art = registry.get("unk-verdict").unwrap();
    assert_eq!(art.usage_count, 50);
}

#[tokio::test]
async fn test_get_latest_cycle_from_store() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path());
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    // Run two cycles
    engine.run_cycle(&[]).await;
    engine.run_cycle(&[]).await;

    let latest = engine.get_latest_cycle();
    assert!(latest.is_some());
}

// BUG-20261118-167 横向排查回归锁：get_latest_cycle_from_store 曾直接
// read_cycles(...).last()，把「月目录/文件名序 + JSONL 行 append 序」当
// started_at 时刻序——时钟回拨 / NTP 校时 / 并发写交错下行序≠时刻序，
// .last() 取到非最新 cycle（doc 承诺 "most recent"）。修复后显式
// rfc3339_order 排序再取尾（对齐 load_latest_cycle 的 BUG-140 形态）。
// 构造：同一文件先写时刻较晚的 B、再写时刻较早的 A（模拟时钟回拨），
// 正确答案必须是 B（时刻最新）而非文件尾行的 A。
#[tokio::test]
async fn test_get_latest_cycle_from_store_out_of_order_appends() {
    use nemesis_types::forge::{CycleStatus, LearningCycle};

    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let engine = LearningEngine::new(
        ForgeConfig::default(),
        registry,
        CycleStore::new(dir.path()),
    );
    // 播种用独立 store 实例（CycleStore 无 Clone；同 base_dir 读写同盘）。
    let seed_store = CycleStore::new(dir.path());

    // 播种在 30 天窗口内（since 按文件名钟面日期过滤）：昨天同一天的两个
    // 时刻，落同一 JSONL 文件，行序与时刻序刻意相反。
    let tz = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
    let base_day = (chrono::Local::now() - chrono::Duration::days(1)).date_naive();
    let late_dt = base_day.and_hms_opt(12, 0, 0).unwrap(); // 时刻较晚
    let early_dt = base_day.and_hms_opt(8, 0, 0).unwrap(); // 时刻较早
    let cycle_late = LearningCycle {
        id: "cycle-late".into(),
        started_at: late_dt.and_local_timezone(tz).unwrap().to_rfc3339(),
        completed_at: None,
        patterns_found: 2,
        actions_taken: 2,
        status: CycleStatus::Completed,
    };
    let cycle_early = LearningCycle {
        id: "cycle-early".into(),
        started_at: early_dt.and_local_timezone(tz).unwrap().to_rfc3339(),
        completed_at: None,
        patterns_found: 1,
        actions_taken: 1,
        status: CycleStatus::Completed,
    };
    // 先写晚的、再写早的（append 序≠时刻序）
    seed_store.append(&cycle_late).await.unwrap();
    seed_store.append(&cycle_early).await.unwrap();

    let latest = engine
        .get_latest_cycle_from_store()
        .await
        .expect("30 天窗口内的播种数据必须可见");
    assert_eq!(
        latest.id, "cycle-late",
        "时刻最新的 cycle 必须胜过文件尾行——显式排序必须压过 append 序"
    );
}

// ============================================================
// Additional tests for set_* methods, detect patterns, and
// evaluate outcome edge cases
// ============================================================
#[test]
fn test_set_monitor() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let registry2 = registry.clone();
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    let monitor = Arc::new(crate::monitor::DeploymentMonitor::new(
        ForgeConfig::default(),
        registry2,
    ));
    engine.set_monitor(monitor);
}

#[test]
fn test_detect_tool_chain_patterns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    let patterns = engine.extract_patterns(&[]);
    assert!(patterns.is_empty());
}
#[test]
fn test_detect_tool_chain_patterns_few_experiences() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    let exps: Vec<CollectedExperience> = (0..2).map(|_| make_collected("tool_a", true)).collect();
    let patterns = engine.extract_patterns(&exps);
    assert!(patterns.is_empty());
}

#[test]
fn test_detect_error_recovery_patterns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    let patterns = engine.extract_patterns(&[]);
    assert!(patterns.is_empty());
}

#[test]
fn test_detect_efficiency_issue_patterns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    let patterns = engine.extract_patterns(&[]);
    assert!(patterns.is_empty());
}

#[test]
fn test_detect_success_template_patterns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    let patterns = engine.extract_patterns(&[]);
    assert!(patterns.is_empty());
}

#[test]
fn test_evaluate_result_positive_verdict() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let artifact = nemesis_types::forge::Artifact {
        id: "eval-pos".into(),
        name: "test".into(),
        kind: nemesis_types::forge::ArtifactKind::Skill,
        version: "1.0".into(),
        status: nemesis_types::forge::ArtifactStatus::Active,
        content: "test".into(),
        tool_signature: vec![],
        created_at: chrono::Local::now().to_rfc3339(),
        updated_at: chrono::Local::now().to_rfc3339(),
        usage_count: 10,
        last_degraded_at: None,
        success_rate: 0.5,
        consecutive_observing_rounds: 0,
        deployed_at: None,
        last_efficacy: None,
    };
    registry.add(artifact);
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry.clone(), cycle_store);

    let outcomes = vec![DeploymentOutcome {
        artifact_id: "eval-pos".into(),
        verdict: "positive".into(),
        improvement_score: 0.8,
        sample_size: 15,
    }];
    // Should not panic
    engine.evaluate_outcomes(&outcomes);
    assert!(registry.get("eval-pos").is_some());
}

#[test]
fn test_evaluate_result_negative_verdict() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let artifact = nemesis_types::forge::Artifact {
        id: "eval-neg".into(),
        name: "test".into(),
        kind: nemesis_types::forge::ArtifactKind::Skill,
        version: "1.0".into(),
        status: nemesis_types::forge::ArtifactStatus::Active,
        content: "test".into(),
        tool_signature: vec![],
        created_at: chrono::Local::now().to_rfc3339(),
        updated_at: chrono::Local::now().to_rfc3339(),
        usage_count: 10,
        last_degraded_at: None,
        success_rate: 0.5,
        consecutive_observing_rounds: 0,
        deployed_at: None,
        last_efficacy: None,
    };
    registry.add(artifact);
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry.clone(), cycle_store);

    let outcomes = vec![DeploymentOutcome {
        artifact_id: "eval-neg".into(),
        verdict: "negative".into(),
        improvement_score: -0.5,
        sample_size: 10,
    }];
    // Should not panic
    engine.evaluate_outcomes(&outcomes);
    assert!(registry.get("eval-neg").is_some());
    // 回归钉：negative 评测真实下调（0.5 - 0.2 = 0.3）
    let art = registry.get("eval-neg").unwrap();
    assert!(
        (art.success_rate - 0.3).abs() < 1e-9,
        "negative 评测后 success_rate 应为 0.3，实际 {}",
        art.success_rate
    );
}

#[tokio::test]
async fn test_get_latest_cycle_empty() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    let latest = engine.get_latest_cycle();
    assert!(
        latest.is_none(),
        "Should be None when no cycles have been run"
    );
}

// ============================================================
// Additional coverage tests for execute paths
// ============================================================
#[test]
fn test_detect_efficiency_issue_single_tool() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    // Only 1 usage - should not generate efficiency pattern (needs >= 2)
    let exps = vec![make_collected_with_duration("solo_tool", true, 10000)];
    let patterns = engine.extract_patterns(&exps);
    assert!(patterns.is_empty());
}

#[test]
fn test_detect_error_recovery_only_errors() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    // 会话级语义：恢复窗口 = (失败, 成功) 且工具不同。全失败
    // （fail→fail）不构成恢复对——旧「失败也算模式」语义已废。
    let exps: Vec<CollectedExperience> = (0..5)
        .map(|_| make_collected("always_fails", false))
        .collect();
    let patterns = engine.extract_patterns(&exps);
    assert!(
        !patterns.iter().any(|p| p.pattern_type == "error_recovery"),
        "全失败会话不得产出 error_recovery（fail→fail 不是恢复对）"
    );
}

// ---- C1：证据透传 + prompt 证据块 ----
#[test]
fn test_detected_pattern_fields() {
    let pattern = DetectedPattern {
        pattern_type: "tool_chain".into(),
        frequency: 10,
        confidence: 0.85,
        description: "Test pattern".into(),
        tools: vec!["tool_a".into(), "tool_b".into()],
        evidence: None,
    };
    assert_eq!(pattern.pattern_type, "tool_chain");
    assert_eq!(pattern.frequency, 10);
    assert!((pattern.confidence - 0.85).abs() < 0.01);
    assert_eq!(pattern.tools.len(), 2);
}
// --- Additional coverage tests ---

#[test]
fn test_detect_tool_chains_empty() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);
    let patterns = engine.extract_patterns(&[]);
    assert!(patterns.is_empty());
}

#[test]
fn test_detect_efficiency_issue_empty() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);
    let patterns = engine.extract_patterns(&[]);
    assert!(patterns.is_empty());
}

#[test]
fn test_detect_efficiency_issue_single_tool_no_pattern() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);
    // Only one experience for a tool => can't be 2x slower than itself
    let exps = vec![make_collected_with_duration("solo_tool", true, 500)];
    let patterns = engine.extract_patterns(&exps);
    let efficiency: Vec<_> = patterns
        .iter()
        .filter(|p| p.pattern_type == "efficiency_issue")
        .collect();
    assert!(efficiency.is_empty());
}
// =========================================================================
// coverage batch (quality-hardening goal 冲刺)
// =========================================================================

/// Build an engine rooted at `<dir>/forge` with a separate cycle store.
fn s8_engine(dir: &std::path::Path, config: ForgeConfig) -> (LearningEngine, Arc<Registry>) {
    std::fs::create_dir_all(dir.join("forge")).unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let engine = LearningEngine::with_forge_dir(
        config,
        dir.join("forge"),
        registry.clone(),
        CycleStore::from_base(dir.join("cycles")),
    );
    (engine, registry)
}

/// Build a registry artifact with the given kind/status.
fn s8_artifact(
    id: &str,
    kind: nemesis_types::forge::ArtifactKind,
    status: nemesis_types::forge::ArtifactStatus,
) -> nemesis_types::forge::Artifact {
    nemesis_types::forge::Artifact {
        id: id.to_string(),
        name: id.to_string(),
        kind,
        version: "1.0".to_string(),
        status,
        content: String::new(),
        tool_signature: vec![],
        created_at: chrono::Local::now().to_rfc3339(),
        updated_at: chrono::Local::now().to_rfc3339(),
        usage_count: 0,
        last_degraded_at: None,
        success_rate: 0.0,
        consecutive_observing_rounds: 0,
        deployed_at: None,
        last_efficacy: None,
    }
}

/// with_forge_dir tracing field expression (forge_dir display).
#[test]
fn test_s8_with_forge_dir_tracing() {
    let _g = crate::test_support::quiet_trace_guard();
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("forge")).unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let engine = LearningEngine::with_forge_dir(
        ForgeConfig::default(),
        dir.path().join("forge"),
        registry,
        CycleStore::from_base(dir.path()),
    );
    let _ = engine.get_latest_cycle();
}
/// run_cycle with a cycle store whose base path is a regular file — the
/// persist step errs (warn branch).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_s8_run_cycle_persist_err() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("forge")).unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    // Cycle-store base is a FILE -> append fails.
    let file_base = dir.path().join("cycles-as-file");
    std::fs::write(&file_base, "not a dir").unwrap();
    let engine = LearningEngine::with_forge_dir(
        ForgeConfig::default(),
        dir.path().join("forge"),
        registry,
        CycleStore::from_base(file_base),
    );

    let _g = crate::test_support::quiet_trace_guard();
    let cycle = engine.run_cycle(&[]).await;
    assert_eq!(cycle.status, nemesis_types::forge::CycleStatus::Completed);
}
/// adjust_confidence_from_outcomes verdict handling: empty artifact id,
/// negative delta, and unknown verdict (no adjustment).
#[test]
fn test_s8_adjust_confidence_verdicts() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, registry) = s8_engine(dir.path(), ForgeConfig::default());
    registry.add(s8_artifact(
        "s8-conf",
        nemesis_types::forge::ArtifactKind::Skill,
        nemesis_types::forge::ArtifactStatus::Active,
    ));

    let outcomes = vec![
        crate::monitor::EvaluationResult {
            artifact_id: String::new(), // skipped (no artifact)
            verdict: "positive".into(),
            improvement_score: 1.0,
            sample_size: 1,
        },
        crate::monitor::EvaluationResult {
            artifact_id: "s8-conf".into(),
            verdict: "negative".into(), // -0.2
            improvement_score: -1.0,
            sample_size: 2,
        },
        crate::monitor::EvaluationResult {
            artifact_id: "s8-conf".into(),
            verdict: "neutral".into(), // no delta
            improvement_score: 0.0,
            sample_size: 3,
        },
    ];
    engine.adjust_confidence_for_test(&outcomes);

    let art = registry
        .list(None, None)
        .into_iter()
        .find(|a| a.id == "s8-conf")
        .expect("artifact present");
    // Initial 0.0, negative delta -0.2 clamps at the 0.0 floor; the neutral
    // verdict and the empty artifact id must not adjust anything.
    assert!(
        (art.success_rate - 0.0).abs() < 1e-9,
        "negative delta from 0.0 clamps to 0.0: got {}",
        art.success_rate
    );
}

/// adjust_confidence negative delta from a non-zero base: 0.5 - 0.2 = 0.3.
#[test]
fn test_s8_adjust_confidence_negative_from_base() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, registry) = s8_engine(dir.path(), ForgeConfig::default());
    registry.add(s8_artifact(
        "s8-conf2",
        nemesis_types::forge::ArtifactKind::Skill,
        nemesis_types::forge::ArtifactStatus::Active,
    ));
    registry.update("s8-conf2", |a| a.success_rate = 0.5);

    engine.adjust_confidence_for_test(&[crate::monitor::EvaluationResult {
        artifact_id: "s8-conf2".into(),
        verdict: "negative".into(),
        improvement_score: -1.0,
        sample_size: 2,
    }]);

    let art = registry
        .list(None, None)
        .into_iter()
        .find(|a| a.id == "s8-conf2")
        .unwrap();
    assert!(
        (art.success_rate - 0.3).abs() < 1e-9,
        "0.5 - 0.2 = 0.3: got {}",
        art.success_rate
    );
}

/// disable_degraded_skills_impl branch matrix: non-skill artifact, non-
/// degraded skill, degraded skill without file, rename failure.
#[test]
fn test_s8_disable_degraded_variants() {
    use nemesis_types::forge::{ArtifactKind, ArtifactStatus};
    let dir = tempfile::tempdir().unwrap();
    let (engine, registry) = s8_engine(dir.path(), ForgeConfig::default());

    // Non-Skill degraded artifact -> skipped by kind.
    registry.add(s8_artifact(
        "s8-script",
        ArtifactKind::Script,
        ArtifactStatus::Degraded,
    ));
    // Skill but not degraded -> skipped by status.
    registry.add(s8_artifact(
        "s8-active-skill",
        ArtifactKind::Skill,
        ArtifactStatus::Active,
    ));
    // Degraded skill with no workspace file -> exists() false branch.
    registry.add(s8_artifact(
        "s8-no-file",
        ArtifactKind::Skill,
        ArtifactStatus::Degraded,
    ));
    // Degraded skill whose SKILL.md exists but rename fails (target is a dir).
    registry.add(s8_artifact(
        "s8-blocked",
        ArtifactKind::Skill,
        ArtifactStatus::Degraded,
    ));
    let blocked_dir = dir.path().join("skills").join("s8-blocked-forge");
    std::fs::create_dir_all(blocked_dir.join("SKILL.md.disabled")).unwrap();
    std::fs::write(blocked_dir.join("SKILL.md"), "content").unwrap();

    let _g = crate::test_support::quiet_trace_guard();
    engine.disable_degraded_skills_impl(); // no panic

    // The blocked skill file is still in place (rename failed).
    assert!(blocked_dir.join("SKILL.md").exists());
}

/// get_latest_cycle_from_store: Ok path (empty store -> None) and Err path
/// (store base is a file -> fallback to in-memory cache).
#[tokio::test]
async fn test_s8_get_latest_cycle_from_store() {
    // Ok path: valid empty store.
    let dir1 = tempfile::tempdir().unwrap();
    {
        let registry = Arc::new(Registry::new(RegistryConfig::default()));
        let engine = LearningEngine::new(
            ForgeConfig::default(),
            registry,
            CycleStore::from_base(dir1.path().join("cycles")),
        );
        assert!(engine.get_latest_cycle_from_store().await.is_none());
    }

    // Err path: store base is a regular file -> fallback (None here).
    let dir2 = tempfile::tempdir().unwrap();
    {
        let file_base = dir2.path().join("cycles-as-file");
        std::fs::write(&file_base, "not a dir").unwrap();
        let registry = Arc::new(Registry::new(RegistryConfig::default()));
        let engine = LearningEngine::new(
            ForgeConfig::default(),
            registry,
            CycleStore::from_base(file_base),
        );
        let _g = crate::test_support::quiet_trace_guard();
        assert!(engine.get_latest_cycle_from_store().await.is_none());
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_p0f2_run_cycle_trace_arm_evaluates_outcomes() {
    // 审计 -：run_cycle Step1 轨迹臂接线。TraceStore
    // （forge_dir/"traces"，与 gateway 装配点同形态）里有签名匹配的轨迹时，
    // run_cycle 必须走 evaluate_outcomes 并把 last_efficacy 写回 registry——
    // 此前轨迹零读者，实效对比链在这里断头。证据面：断言 last_efficacy
    // 从 None 变 Some 且带 verdict（TraceStore 直读的原始事件是输入，
    // registry 的实效快照是输出，中间整条链被钉住）。
    use crate::trace_store::TraceStore;
    use nemesis_types::forge::{Artifact, ArtifactKind, ArtifactStatus};
    let dir = tempfile::tempdir().unwrap();
    let forge_dir = dir.path().join("forge");
    std::fs::create_dir_all(&forge_dir).unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));

    // Active skill with a tool signature matched by the seeded traces.
    // created_at 久远 → 轨迹（now）全落 after 桶；6 条 ≥ 默认 min_samples 5。
    registry.add(Artifact {
        id: "skill-traced".into(),
        name: "traced".into(),
        kind: ArtifactKind::Skill,
        version: "1.0".into(),
        status: ArtifactStatus::Active,
        content: "...".into(),
        tool_signature: vec!["file_read".into()],
        created_at: "2026-01-01T00:00:00+08:00".into(),
        updated_at: "2026-01-01T00:00:00+08:00".into(),
        usage_count: 10,
        last_degraded_at: None,
        success_rate: 0.0,
        consecutive_observing_rounds: 0,
        deployed_at: None,
        last_efficacy: None,
    });

    let monitor = Arc::new(DeploymentMonitor::new(
        ForgeConfig::default(),
        registry.clone(),
    ));
    let cycle_store = CycleStore::new(&forge_dir);
    let engine = LearningEngine::with_forge_dir(
        ForgeConfig::default(),
        forge_dir.clone(),
        registry.clone(),
        cycle_store,
    );
    engine.set_monitor(monitor);

    // Seed the trace store the same way the tool-dispatch side does:
    // tool_call events keyed by session_key, data {tool, ok}.
    let store = TraceStore::new(forge_dir.join("traces"));
    for i in 0..6 {
        store
            .append(&crate::trace::TraceEvent {
                id: uuid::Uuid::new_v4().to_string(),
                event_type: "tool_call".into(),
                session_key: format!("web:{i}"),
                timestamp: chrono::Local::now().to_rfc3339(),
                data: serde_json::json!({"tool": "file_read", "ok": true}),
            })
            .await
            .unwrap();
    }

    let cycle = engine.run_cycle(&[]).await;
    assert_eq!(cycle.status, nemesis_types::forge::CycleStatus::Completed);

    let updated = registry.get("skill-traced").unwrap();
    let efficacy = updated
        .last_efficacy
        .expect("trace arm must evaluate matched traces and write last_efficacy back");
    assert!(
        efficacy.verdict == "positive"
            || efficacy.verdict == "neutral"
            || efficacy.verdict == "observing",
        "unexpected verdict from all-success traces: {}",
        efficacy.verdict
    );
    assert_eq!(efficacy.samples_after, 6);
}

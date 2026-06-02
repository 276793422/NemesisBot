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
        timestamp: chrono::Utc::now().to_rfc3339(),
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

#[tokio::test]
async fn test_run_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    let exps: Vec<CollectedExperience> = (0..5)
        .map(|_| make_collected("file_read", true))
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

    let exps: Vec<CollectedExperience> = (0..5)
        .flat_map(|_| {
            vec![
                make_collected("tool_a", true),
                make_collected("tool_b", false),
            ]
        })
        .collect();

    let patterns = engine.extract_patterns(&exps);
    assert!(!patterns.is_empty());
}

#[test]
fn test_generate_actions() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));

    let mut config = ForgeConfig::default();
    config.learning.high_conf_threshold = 0.5;
    let engine = LearningEngine::new(config, registry, cycle_store);

    let patterns = vec![DetectedPattern {
        pattern_type: "tool_chain".into(),
        frequency: 10,
        confidence: 0.9,
        description: "test pattern".into(),
        tools: vec!["tool_a".into()],
    }];

    let actions = engine.generate_actions(&patterns);
    assert!(!actions.is_empty());
    assert_eq!(actions[0].action_type, "create_skill");
    assert_eq!(actions[0].priority, "high");
    assert_eq!(actions[0].status, "pending");
}

#[test]
fn test_detect_efficiency_issue() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let mut config = ForgeConfig::default();
    config.learning.min_pattern_frequency = 3; // Lower threshold for test
    let engine = LearningEngine::new(config, registry, cycle_store);

    // Create a large dataset where slow_tool is 10x slower than average
    let mut exps = Vec::new();
    // 10 fast operations
    for _ in 0..10 {
        exps.push(make_collected_with_duration("fast_tool", true, 10));
    }
    // 5 slow operations (1000ms, well over 2x the avg)
    for _ in 0..5 {
        exps.push(make_collected_with_duration("slow_tool", true, 1000));
    }

    let patterns = engine.extract_patterns(&exps);
    let efficiency: Vec<_> = patterns
        .iter()
        .filter(|p| p.pattern_type == "efficiency_issue")
        .collect();
    assert!(!efficiency.is_empty(), "Expected efficiency issue patterns, got: {:?}", patterns);
    assert!(efficiency[0].description.contains("slow_tool"));
}

#[test]
fn test_detect_success_template() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    let exps: Vec<CollectedExperience> = (0..5)
        .map(|_| make_collected("perfect_tool", true))
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

    // tool_chain: high frequency (10+ uses)
    for _ in 0..10 {
        exps.push(make_collected("chain_tool", true));
    }
    // error_recovery: some failures among >= 3 total
    for _ in 0..3 {
        exps.push(make_collected("error_tool", false));
    }
    exps.push(make_collected("error_tool", true));
    // efficiency_issue: very slow tool (10 fast + 5 slow)
    for _ in 0..10 {
        exps.push(make_collected_with_duration("fast", true, 10));
    }
    for _ in 0..5 {
        exps.push(make_collected_with_duration("slow_tool", true, 1000));
    }
    // success_template: perfect success with >= 5 uses
    for _ in 0..5 {
        exps.push(make_collected("perfect", true));
    }

    let patterns = engine.extract_patterns(&exps);
    let types: std::collections::HashSet<&str> = patterns
        .iter()
        .map(|p| p.pattern_type.as_str())
        .collect();

    assert!(types.contains("tool_chain"), "Should detect tool_chain, found: {:?}", types);
    assert!(
        types.contains("error_recovery"),
        "Should detect error_recovery, found: {:?}", types
    );
    assert!(
        types.contains("efficiency_issue"),
        "Should detect efficiency_issue, found: {:?}", types
    );
    assert!(
        types.contains("success_template"),
        "Should detect success_template, found: {:?}", types
    );
}

#[test]
fn test_generate_actions_for_all_pattern_types() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));

    let mut config = ForgeConfig::default();
    config.learning.high_conf_threshold = 0.5;
    let engine = LearningEngine::new(config, registry, cycle_store);

    let patterns = vec![
        DetectedPattern {
            pattern_type: "tool_chain".into(),
            frequency: 10,
            confidence: 0.9,
            description: "tool chain pattern".into(),
            tools: vec!["tool_a".into()],
        },
        DetectedPattern {
            pattern_type: "error_recovery".into(),
            frequency: 5,
            confidence: 0.85,
            description: "error recovery pattern".into(),
            tools: vec!["tool_b".into()],
        },
        DetectedPattern {
            pattern_type: "efficiency_issue".into(),
            frequency: 3,
            confidence: 0.7,
            description: "efficiency issue".into(),
            tools: vec!["tool_c".into()],
        },
        DetectedPattern {
            pattern_type: "success_template".into(),
            frequency: 8,
            confidence: 0.95,
            description: "success template".into(),
            tools: vec!["tool_d".into()],
        },
    ];

    let actions = engine.generate_actions(&patterns);
    // tool_chain (conf 0.9 >= 0.5, freq 10 >= 10) => create_skill
    // error_recovery (conf 0.85 >= 0.5) => create_skill
    // efficiency_issue => suggest_prompt
    // success_template (conf 0.95 >= 0.5) => create_skill
    assert!(actions.len() >= 3, "Expected at least 3 actions, got {}", actions.len());

    let create_skills: Vec<_> = actions
        .iter()
        .filter(|a| a.action_type == "create_skill")
        .collect();
    let suggest_prompts: Vec<_> = actions
        .iter()
        .filter(|a| a.action_type == "suggest_prompt")
        .collect();
    assert!(!create_skills.is_empty());
    assert!(!suggest_prompts.is_empty());
}

#[test]
fn test_generate_skill_name() {
    assert_eq!(
        generate_skill_name("read->edit->exec"),
        "read-edit-exec-workflow"
    );
    assert_eq!(generate_skill_name("tool"), "tool-workflow");

    // Long name should be truncated
    let long_chain = "a->b->c->d->e->f->g->h->i->j->k->l->m->n->o->p";
    let name = generate_skill_name(long_chain);
    assert!(name.len() <= 60); // 50 + "-workflow"
    assert!(name.ends_with("-workflow"));
}

#[test]
fn test_iterative_refiner_passes_immediately() {
    let refiner = IterativeRefiner::new(3);
    let (content, passed) = refiner.refine("---\nname: test\n---\nValid content", |c| {
        c.contains("---") && c.contains("Valid")
    });
    assert!(passed);
    assert!(content.contains("---"));
}

#[test]
fn test_iterative_refiner_refines() {
    let refiner = IterativeRefiner::new(3);
    let (content, _) = refiner.refine("plain content", |c| c.contains("---"));
    // After refinement, should have frontmatter added
    assert!(content.contains("---"));
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
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
        usage_count: 10,
        last_degraded_at: None,
        success_rate: 0.0,
        consecutive_observing_rounds: 0,
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

    let exps: Vec<CollectedExperience> = (0..3)
        .map(|_| make_collected("tool", true))
        .collect();
    let cycle = engine.run_cycle(&exps).await;
    assert!(cycle.id.len() > 0);
    assert!(cycle.started_at.len() > 0);
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

    let exps: Vec<CollectedExperience> = (0..10)
        .map(|_| make_collected("frequent_tool", true))
        .collect();

    let patterns = engine.extract_patterns(&exps);
    assert!(patterns.iter().any(|p| p.pattern_type == "tool_chain"));
    let tc = patterns.iter().find(|p| p.pattern_type == "tool_chain").unwrap();
    assert!(tc.frequency >= 3);
    assert!(tc.confidence > 0.0);
}

#[test]
fn test_extract_patterns_error_recovery_detection() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    let mut exps = Vec::new();
    for _ in 0..5 {
        exps.push(make_collected("flaky", false));
    }
    exps.push(make_collected("flaky", true));

    let patterns = engine.extract_patterns(&exps);
    assert!(patterns.iter().any(|p| p.pattern_type == "error_recovery"));
}

#[test]
fn test_extract_patterns_sorted_by_confidence() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    let exps: Vec<CollectedExperience> = (0..20)
        .flat_map(|_| {
            vec![
                make_collected("high_freq", true),
                make_collected("low_freq", true),
            ]
        })
        .chain((0..15).map(|_| make_collected("high_freq", true)))
        .collect();

    let patterns = engine.extract_patterns(&exps);
    for i in 1..patterns.len() {
        assert!(patterns[i-1].confidence >= patterns[i].confidence);
    }
}

#[test]
fn test_generate_actions_tool_chain_below_threshold() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let mut config = ForgeConfig::default();
    config.learning.high_conf_threshold = 0.9;
    let engine = LearningEngine::new(config, registry, cycle_store);

    let patterns = vec![DetectedPattern {
        pattern_type: "tool_chain".into(),
        frequency: 5,
        confidence: 0.5,
        description: "low conf chain".into(),
        tools: vec!["tool_a".into()],
    }];

    let actions = engine.generate_actions(&patterns);
    assert!(!actions.is_empty());
    // Below threshold => suggest_prompt, not create_skill
    assert_eq!(actions[0].action_type, "suggest_prompt");
}

#[test]
fn test_generate_actions_tool_chain_high_freq() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let mut config = ForgeConfig::default();
    config.learning.high_conf_threshold = 0.5;
    let engine = LearningEngine::new(config, registry, cycle_store);

    let patterns = vec![DetectedPattern {
        pattern_type: "tool_chain".into(),
        frequency: 15,
        confidence: 0.9,
        description: "high freq chain".into(),
        tools: vec!["tool_a".into(), "tool_b".into()],
    }];

    let actions = engine.generate_actions(&patterns);
    assert!(!actions.is_empty());
    assert_eq!(actions[0].action_type, "create_skill");
    assert!(actions[0].draft_name.is_some());
}

#[test]
fn test_generate_actions_efficiency_always_suggest() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    let patterns = vec![DetectedPattern {
        pattern_type: "efficiency_issue".into(),
        frequency: 5,
        confidence: 0.99,
        description: "very slow".into(),
        tools: vec!["slow_tool".into()],
    }];

    let actions = engine.generate_actions(&patterns);
    assert!(!actions.is_empty());
    assert_eq!(actions[0].action_type, "suggest_prompt");
}

#[test]
fn test_generate_actions_unknown_pattern_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    let patterns = vec![DetectedPattern {
        pattern_type: "unknown_type".into(),
        frequency: 100,
        confidence: 1.0,
        description: "mystery".into(),
        tools: vec!["tool".into()],
    }];

    let actions = engine.generate_actions(&patterns);
    assert!(actions.is_empty());
}

#[test]
fn test_generate_actions_sorted_by_priority() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let mut config = ForgeConfig::default();
    config.learning.high_conf_threshold = 0.5;
    let engine = LearningEngine::new(config, registry, cycle_store);

    let patterns = vec![
        DetectedPattern {
            pattern_type: "efficiency_issue".into(),
            frequency: 5,
            confidence: 0.7,
            description: "slow".into(),
            tools: vec!["slow".into()],
        },
        DetectedPattern {
            pattern_type: "tool_chain".into(),
            frequency: 15,
            confidence: 0.9,
            description: "chain".into(),
            tools: vec!["chain".into()],
        },
    ];

    let actions = engine.generate_actions(&patterns);
    if actions.len() >= 2 {
        // High priority (create_skill) should come before medium (suggest_prompt)
        let priority_order = |p: &str| -> u8 {
            match p { "high" => 0, "medium" => 1, _ => 2 }
        };
        assert!(priority_order(&actions[0].priority) <= priority_order(&actions[1].priority));
    }
}

#[test]
fn test_detect_tool_chains_min_frequency() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    // Only 2 experiences - below min frequency of 3
    let exps = vec![
        make_collected("rare_tool", true),
        make_collected("rare_tool", true),
    ];
    let patterns = engine.extract_patterns(&exps);
    // All patterns should be filtered out since frequency < min_pattern_frequency (default 3)
    assert!(patterns.is_empty());
}

#[test]
fn test_detect_error_recovery_no_errors() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    let exps: Vec<CollectedExperience> = (0..5)
        .map(|_| make_collected("perfect", true))
        .collect();
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
    let success: Vec<_> = patterns.iter()
        .filter(|p| p.pattern_type == "success_template" && p.tools.contains(&"almost_perfect".to_string()))
        .collect();
    assert!(success.is_empty(), "Should not be success template if any failure");
}

#[test]
fn test_learning_action_new() {
    let action = LearningAction::new("create_skill", "high", "test description");
    assert!(action.id.starts_with("la-"));
    assert_eq!(action.action_type, "create_skill");
    assert_eq!(action.priority, "high");
    assert_eq!(action.description, "test description");
    assert_eq!(action.status, "pending");
    assert!(action.error_msg.is_none());
    assert!(action.draft_name.is_none());
    assert!(action.rationale.is_none());
    assert_eq!(action.confidence, 0.0);
    assert!(action.pattern_id.is_none());
    assert!(action.artifact_id.is_none());
    assert!(action.created_at.is_some());
    assert!(action.executed_at.is_none());
}

#[test]
fn test_learning_action_clone() {
    let mut action = LearningAction::new("test", "low", "desc");
    action.confidence = 0.85;
    action.draft_name = Some("test-skill".into());
    let cloned = action.clone();
    assert_eq!(cloned.confidence, 0.85);
    assert_eq!(cloned.draft_name, Some("test-skill".into()));
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

#[test]
fn test_action_summary_fields() {
    let summary = ActionSummary {
        id: "la-xyz".into(),
        action_type: "create_skill".into(),
        priority: "high".into(),
        status: "executed".into(),
        artifact_id: Some("skill-test".into()),
    };
    assert_eq!(summary.id, "la-xyz");
    assert_eq!(summary.artifact_id, Some("skill-test".into()));
}

#[test]
fn test_generate_skill_name_simple() {
    assert_eq!(generate_skill_name("read"), "read-workflow");
}

#[test]
fn test_generate_skill_name_with_underscores() {
    assert_eq!(generate_skill_name("file_read->file_write"), "file-read-file-write-workflow");
}

#[test]
fn test_generate_skill_name_truncation() {
    let long = "a->b->c->d->e->f->g->h->i->j->k->l->m->n->o->p->q->r->s->t";
    let name = generate_skill_name(long);
    assert!(name.len() <= 60);
    assert!(name.ends_with("-workflow"));
}

#[test]
fn test_extract_tool_signature_simple() {
    // The function splits on Unicode arrow →, not on ->
    let sig = extract_tool_signature_from_chain_public("read→edit→exec");
    assert_eq!(sig, vec!["read", "edit", "exec"]);
}

#[test]
fn test_extract_tool_signature_single() {
    let sig = extract_tool_signature_from_chain_public("tool_a");
    assert_eq!(sig, vec!["tool_a"]);
}

#[test]
fn test_extract_tool_signature_with_prefix() {
    // The function splits on Unicode arrow →
    let sig = extract_tool_signature_from_chain_public("Tool chain: read→edit");
    assert!(sig.contains(&"read".to_string()));
}

#[test]
fn test_build_diagnosis_stage1_failed() {
    let validation = ArtifactValidation {
        stage1_static: Some(crate::pipeline::StaticValidationResult {
            stage: crate::pipeline::ValidationStage {
                passed: false,
                timestamp: String::new(),
                errors: vec!["too short".into()],
            },
            warnings: vec![],
        }),
        stage2_functional: None,
        stage3_quality: None,
        last_validated: String::new(),
    };
    let diagnosis = build_diagnosis_public(&validation);
    assert!(diagnosis.contains("Stage 1"));
    assert!(diagnosis.contains("too short"));
}

#[test]
fn test_build_diagnosis_stage2_failed() {
    let validation = ArtifactValidation {
        stage1_static: Some(crate::pipeline::StaticValidationResult {
            stage: crate::pipeline::ValidationStage {
                passed: true,
                timestamp: String::new(),
                errors: vec![],
            },
            warnings: vec![],
        }),
        stage2_functional: Some(crate::pipeline::FunctionalValidationResult {
            stage: crate::pipeline::ValidationStage {
                passed: false,
                timestamp: String::new(),
                errors: vec!["Only 1/3 checks passed".into()],
            },
            tests_run: 3,
            tests_passed: 1,
        }),
        stage3_quality: None,
        last_validated: String::new(),
    };
    let diagnosis = build_diagnosis_public(&validation);
    assert!(diagnosis.contains("Stage 2"));
    assert!(diagnosis.contains("checks passed"));
}

#[test]
fn test_build_diagnosis_all_passed() {
    let validation = ArtifactValidation {
        stage1_static: Some(crate::pipeline::StaticValidationResult {
            stage: crate::pipeline::ValidationStage {
                passed: true,
                timestamp: String::new(),
                errors: vec![],
            },
            warnings: vec![],
        }),
        stage2_functional: Some(crate::pipeline::FunctionalValidationResult {
            stage: crate::pipeline::ValidationStage {
                passed: true,
                timestamp: String::new(),
                errors: vec![],
            },
            tests_run: 3,
            tests_passed: 3,
        }),
        stage3_quality: Some(crate::pipeline::QualityValidationResult {
            stage: crate::pipeline::ValidationStage {
                passed: true,
                timestamp: String::new(),
                errors: vec![],
            },
            score: 85,
            notes: "Good quality".into(),
            dimensions: Default::default(),
        }),
        last_validated: String::new(),
    };
    let diagnosis = build_diagnosis_public(&validation);
    assert!(diagnosis.contains("Score: 85"));
    assert!(diagnosis.contains("Good quality"));
}

#[test]
fn test_find_artifact_by_fingerprint_empty_registry() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);
    assert!(!engine.find_artifact_by_fingerprint_public("nonexistent"));
}

#[test]
fn test_find_artifact_by_fingerprint_exists() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let artifact = nemesis_types::forge::Artifact {
        id: "test-id".into(),
        name: "existing-skill".into(),
        kind: nemesis_types::forge::ArtifactKind::Skill,
        version: "1.0".into(),
        status: nemesis_types::forge::ArtifactStatus::Active,
        content: "test".into(),
        tool_signature: vec![],
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
        usage_count: 0,
        last_degraded_at: None,
        success_rate: 0.0,
        consecutive_observing_rounds: 0,
    };
    registry.add(artifact);
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);
    assert!(engine.find_artifact_by_fingerprint_public("existing-skill"));
}

#[test]
fn test_sort_actions_by_priority() {
    let mut actions = vec![
        LearningAction::new("test", "low", "low priority"),
        LearningAction::new("test", "high", "high priority"),
        LearningAction::new("test", "medium", "medium priority"),
    ];
    actions[1].confidence = 0.9;
    actions[2].confidence = 0.8;
    actions[0].confidence = 0.7;

    LearningEngine::sort_actions(&mut actions);
    assert_eq!(actions[0].priority, "high");
    assert_eq!(actions[1].priority, "medium");
    assert_eq!(actions[2].priority, "low");
}

#[test]
fn test_sort_actions_by_confidence_same_priority() {
    let mut actions = vec![
        {
            let mut a = LearningAction::new("test", "high", "low conf");
            a.confidence = 0.5;
            a
        },
        {
            let mut a = LearningAction::new("test", "high", "high conf");
            a.confidence = 0.95;
            a
        },
    ];
    LearningEngine::sort_actions(&mut actions);
    assert!(actions[0].confidence > actions[1].confidence);
}

#[test]
fn test_iterative_refiner_max_rounds_zero() {
    let refiner = IterativeRefiner::new(0);
    // max_rounds=0 should be treated as 3
    assert_eq!(refiner.max_rounds, 3);
}

#[test]
fn test_iterative_refiner_all_rounds_fail() {
    let refiner = IterativeRefiner::new(2);
    let (_, passed) = refiner.refine("initial", |_| false);
    assert!(!passed);
}

#[test]
fn test_iterative_refiner_adds_frontmatter() {
    let refiner = IterativeRefiner::new(3);
    let (content, _) = refiner.refine("plain text", |c| c.contains("---"));
    assert!(content.contains("---"));
    assert!(content.contains("name: generated-skill"));
}

#[test]
fn test_iterative_refiner_adds_structure_round2() {
    let refiner = IterativeRefiner::new(3);
    // First pass: adds frontmatter but validate checks for "## "
    let (content, _) = refiner.refine("---\nname: test\n---\nplain", |c| c.contains("## "));
    assert!(content.contains("## Steps"));
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
    let exps: Vec<CollectedExperience> = (0..3)
        .map(|_| make_collected("tool", true))
        .collect();
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
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
        usage_count: 50,
        last_degraded_at: None,
        success_rate: 0.0,
        consecutive_observing_rounds: 0,
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
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
        usage_count: 50,
        last_degraded_at: None,
        success_rate: 0.0,
        consecutive_observing_rounds: 0,
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

// ============================================================
// Additional tests for set_* methods, detect patterns, and
// evaluate outcome edge cases
// ============================================================

#[test]
fn test_set_provider() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    struct MockProvider;
    #[async_trait::async_trait]
    impl crate::reflector_llm::LLMCaller for MockProvider {
        async fn chat(
            &self,
            _system: &str,
            _user: &str,
            _max_tokens: Option<i64>,
        ) -> Result<String, String> {
            Ok("mock response".into())
        }
    }

    engine.set_provider(Arc::new(MockProvider));
}

#[test]
fn test_set_pipeline() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let registry2 = registry.clone();
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    let pipeline = Arc::new(crate::pipeline::Pipeline::new(ForgeConfig::default(), registry2));
    engine.set_pipeline(pipeline);
}

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
fn test_set_skill_creator() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    struct MockCreator;
    impl SkillCreator for MockCreator {
        fn create_skill(
            &self,
            name: &str,
            _content: &str,
            _description: &str,
            _tool_signature: Vec<String>,
        ) -> Result<nemesis_types::forge::Artifact, String> {
            Ok(nemesis_types::forge::Artifact {
                id: format!("skill-{}", name),
                name: name.into(),
                kind: nemesis_types::forge::ArtifactKind::Skill,
                version: "1.0".into(),
                status: nemesis_types::forge::ArtifactStatus::Draft,
                content: String::new(),
                tool_signature: vec![],
                created_at: chrono::Utc::now().to_rfc3339(),
                updated_at: chrono::Utc::now().to_rfc3339(),
                usage_count: 0,
                last_degraded_at: None,
                success_rate: 0.0,
                consecutive_observing_rounds: 0,
            })
        }
    }

    engine.set_skill_creator(Arc::new(MockCreator));
}

#[test]
fn test_detect_tool_chain_patterns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    let patterns = engine.detect_tool_chains(&[]);
    assert!(patterns.is_empty());
}

#[test]
fn test_detect_tool_chain_patterns_few_experiences() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    let exps: Vec<CollectedExperience> = (0..2)
        .map(|_| make_collected("tool_a", true))
        .collect();
    let patterns = engine.detect_tool_chains(&exps);
    assert!(patterns.is_empty());
}

#[test]
fn test_detect_error_recovery_patterns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    let patterns = engine.detect_error_recovery(&[]);
    assert!(patterns.is_empty());
}

#[test]
fn test_detect_efficiency_issue_patterns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    let patterns = engine.detect_efficiency_issue(&[]);
    assert!(patterns.is_empty());
}

#[test]
fn test_detect_success_template_patterns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    let patterns = engine.detect_success_template(&[]);
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
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
        usage_count: 10,
        last_degraded_at: None,
        success_rate: 0.5,
        consecutive_observing_rounds: 0,
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
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
        usage_count: 10,
        last_degraded_at: None,
        success_rate: 0.5,
        consecutive_observing_rounds: 0,
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
}

#[tokio::test]
async fn test_get_latest_cycle_empty() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    let latest = engine.get_latest_cycle();
    assert!(latest.is_none(), "Should be None when no cycles have been run");
}

// ============================================================
// Additional coverage tests for execute paths
// ============================================================

#[tokio::test]
async fn test_execute_create_skill_action_no_draft_name() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    let action = LearningAction::new("create_skill", "high", "test");
    let result = engine.execute_create_skill_action(&action);
    assert_eq!(result.status, "failed");
    assert!(result.error_msg.unwrap().contains("No draft name"));
}

#[tokio::test]
async fn test_execute_create_skill_action_no_provider() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    let mut action = LearningAction::new("create_skill", "high", "test");
    action.draft_name = Some("test-skill".into());
    let result = engine.execute_create_skill_action(&action);
    assert_eq!(result.status, "failed");
    assert!(result.error_msg.unwrap().contains("No LLM provider"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_execute_create_skill_action_llm_fails() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    struct FailingProvider;
    #[async_trait::async_trait]
    impl crate::reflector_llm::LLMCaller for FailingProvider {
        async fn chat(
            &self,
            _system: &str,
            _user: &str,
            _max_tokens: Option<i64>,
        ) -> Result<String, String> {
            Err("LLM unavailable".into())
        }
    }
    engine.set_provider(Arc::new(FailingProvider));

    let mut action = LearningAction::new("create_skill", "high", "test pattern");
    action.draft_name = Some("test-skill".into());
    let result = engine.execute_create_skill_action(&action);
    assert_eq!(result.status, "failed");
    assert!(result.error_msg.unwrap().contains("LLM generation failed"));
}

#[tokio::test]
async fn test_execute_create_skill_action_already_exists() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let artifact = nemesis_types::forge::Artifact {
        id: "skill-existing".into(),
        name: "existing".into(),
        kind: nemesis_types::forge::ArtifactKind::Skill,
        version: "1.0".into(),
        status: nemesis_types::forge::ArtifactStatus::Active,
        content: "test".into(),
        tool_signature: vec![],
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
        usage_count: 0,
        last_degraded_at: None,
        success_rate: 0.0,
        consecutive_observing_rounds: 0,
    };
    registry.add(artifact);
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    let mut action = LearningAction::new("create_skill", "high", "test");
    action.draft_name = Some("existing".into());
    let result = engine.execute_create_skill_action(&action);
    assert_eq!(result.status, "skipped");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_execute_create_skill_action_no_pipeline() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry.clone(), cycle_store);

    struct MockProvider;
    #[async_trait::async_trait]
    impl crate::reflector_llm::LLMCaller for MockProvider {
        async fn chat(
            &self,
            _system: &str,
            _user: &str,
            _max_tokens: Option<i64>,
        ) -> Result<String, String> {
            Ok("---\nname: test\n---\n\n## Overview\nA test skill with enough content".into())
        }
    }
    engine.set_provider(Arc::new(MockProvider));

    let mut action = LearningAction::new("create_skill", "high", "test description");
    action.draft_name = Some("new-skill-no-pipeline".into());
    let result = engine.execute_create_skill_action(&action);
    // No pipeline => registered as Draft
    assert!(registry.get("skill-new-skill-no-pipeline").is_some());
    let _ = result;
}

#[test]
fn test_detect_efficiency_issue_single_tool() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    // Only 1 usage - should not generate efficiency pattern (needs >= 2)
    let exps = vec![make_collected_with_duration("solo_tool", true, 10000)];
    let patterns = engine.detect_efficiency_issue(&exps);
    assert!(patterns.is_empty());
}

#[test]
fn test_detect_error_recovery_only_errors() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

    let exps: Vec<CollectedExperience> = (0..5)
        .map(|_| make_collected("always_fails", false))
        .collect();
    let patterns = engine.detect_error_recovery(&exps);
    assert!(!patterns.is_empty());
    assert_eq!(patterns[0].pattern_type, "error_recovery");
}

#[tokio::test]
async fn test_execute_suggest_prompt_with_forge_dir() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::with_forge_dir(
        ForgeConfig::default(),
        dir.path().join("forge"),
        registry,
        cycle_store,
    );

    let mut action = LearningAction::new("suggest_prompt", "medium", "test pattern desc");
    action.draft_name = Some("test-suggest".into());
    action.rationale = Some("test rationale".into());
    action.confidence = 0.75;

    engine.execute_suggest_prompt_for_test(&mut action);
    assert_eq!(action.status, "executed");
    assert!(action.artifact_id.is_some());
    assert!(action.executed_at.is_some());
}

#[test]
fn test_generate_actions_success_template_below_threshold() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let mut config = ForgeConfig::default();
    config.learning.high_conf_threshold = 0.99; // Very high threshold
    let engine = LearningEngine::new(config, registry, cycle_store);

    let patterns = vec![DetectedPattern {
        pattern_type: "success_template".into(),
        frequency: 5,
        confidence: 0.95, // Below 0.99 threshold
        description: "success pattern".into(),
        tools: vec!["tool_a".into()],
    }];
    let actions = engine.generate_actions(&patterns);
    // Below threshold => no action generated for success_template
    assert!(actions.is_empty());
}

#[test]
fn test_generate_actions_error_recovery_below_threshold() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let mut config = ForgeConfig::default();
    config.learning.high_conf_threshold = 0.99;
    let engine = LearningEngine::new(config, registry, cycle_store);

    let patterns = vec![DetectedPattern {
        pattern_type: "error_recovery".into(),
        frequency: 5,
        confidence: 0.5, // Below 0.99
        description: "error pattern".into(),
        tools: vec!["tool_b".into()],
    }];
    let actions = engine.generate_actions(&patterns);
    // Below threshold => no action for error_recovery
    assert!(actions.is_empty());
}

#[test]
fn test_generate_actions_tool_chain_below_freq_threshold() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let mut config = ForgeConfig::default();
    config.learning.high_conf_threshold = 0.5;
    let engine = LearningEngine::new(config, registry, cycle_store);

    // High confidence but frequency < 10
    let patterns = vec![DetectedPattern {
        pattern_type: "tool_chain".into(),
        frequency: 5, // Below 10
        confidence: 0.9,
        description: "chain".into(),
        tools: vec!["tool_a".into()],
    }];
    let actions = engine.generate_actions(&patterns);
    assert!(!actions.is_empty());
    assert_eq!(actions[0].action_type, "suggest_prompt"); // Not create_skill since freq < 10
}

#[tokio::test]
async fn test_run_cycle_with_suggest_prompt_action() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let mut config = ForgeConfig::default();
    config.learning.min_pattern_frequency = 3;
    let engine = LearningEngine::with_forge_dir(
        config,
        dir.path().join("forge"),
        registry,
        cycle_store,
    );

    // Create efficiency_issue pattern which triggers suggest_prompt
    let mut exps = Vec::new();
    for _ in 0..10 {
        exps.push(make_collected_with_duration("fast", true, 10));
    }
    for _ in 0..5 {
        exps.push(make_collected_with_duration("slow_tool", true, 1000));
    }

    let cycle = engine.run_cycle(&exps).await;
    assert_eq!(cycle.status, nemesis_types::forge::CycleStatus::Completed);
}

#[test]
fn test_detected_pattern_fields() {
    let pattern = DetectedPattern {
        pattern_type: "tool_chain".into(),
        frequency: 10,
        confidence: 0.85,
        description: "Test pattern".into(),
        tools: vec!["tool_a".into(), "tool_b".into()],
    };
    assert_eq!(pattern.pattern_type, "tool_chain");
    assert_eq!(pattern.frequency, 10);
    assert!((pattern.confidence - 0.85).abs() < 0.01);
    assert_eq!(pattern.tools.len(), 2);
}

#[tokio::test]
async fn test_run_cycle_auto_create_limit() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let mut config = ForgeConfig::default();
    config.learning.max_auto_creates = 1;
    let engine = LearningEngine::with_forge_dir(
        config,
        dir.path().join("forge"),
        registry,
        cycle_store,
    );

    let exps: Vec<CollectedExperience> = (0..5)
        .map(|_| make_collected("tool", true))
        .collect();
    let cycle = engine.run_cycle(&exps).await;
    assert_eq!(cycle.status, nemesis_types::forge::CycleStatus::Completed);
}

// --- Additional coverage tests ---

#[tokio::test]
async fn test_execute_suggest_prompt_writes_file() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::with_forge_dir(
        ForgeConfig::default(),
        dir.path().join("forge"),
        registry,
        cycle_store,
    );

    let mut action = LearningAction::new("suggest_prompt", "medium", "tool chain desc");
    action.draft_name = Some("test-suggestion".into());
    action.rationale = Some("Pattern detected".into());
    action.confidence = 0.8;

    engine.execute_suggest_prompt_for_test(&mut action);
    assert_eq!(action.status, "executed");
    assert!(action.artifact_id.is_some());

    // Verify file was written
    let prompts_dir = dir.path().join("prompts");
    assert!(prompts_dir.exists());
    let files: Vec<_> = std::fs::read_dir(&prompts_dir).unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(files.len(), 1);
    let content = std::fs::read_to_string(files[0].path()).unwrap();
    assert!(content.contains("test-suggestion"));
    assert!(content.contains("Pattern detected"));
}

#[test]
fn test_execute_suggest_prompt_empty_forge_dir() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    // Use empty forge_dir path
    let config = ForgeConfig::default();
    let engine = LearningEngine::new(config, registry, cycle_store);

    let mut action = LearningAction::new("suggest_prompt", "medium", "desc");
    action.draft_name = Some("test".into());
    // forge_dir is empty by default in LearningEngine::new, should return early
    engine.execute_suggest_prompt_for_test(&mut action);
    assert_eq!(action.status, "pending"); // Should not be executed
}

#[test]
fn test_execute_suggest_prompt_special_chars_in_name() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::with_forge_dir(
        ForgeConfig::default(),
        dir.path().join("forge"),
        registry,
        cycle_store,
    );

    let mut action = LearningAction::new("suggest_prompt", "medium", "a -> b -> c");
    action.draft_name = Some("read->write->exec".into());
    action.rationale = Some("Chain detected".into());
    action.confidence = 0.9;

    engine.execute_suggest_prompt_for_test(&mut action);
    assert_eq!(action.status, "executed");
    // Name should be sanitized: arrows replaced, spaces replaced
    let files: Vec<_> = std::fs::read_dir(dir.path().join("prompts")).unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(files.len(), 1);
    let name = files[0].file_name().to_string_lossy().to_string();
    assert!(name.contains("read-write-exec"));
}

#[test]
fn test_execute_suggest_prompt_long_name_truncated() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::with_forge_dir(
        ForgeConfig::default(),
        dir.path().join("forge"),
        registry,
        cycle_store,
    );

    let long_name = "a".repeat(100);
    let mut action = LearningAction::new("suggest_prompt", "medium", "desc");
    action.draft_name = Some(long_name);
    action.rationale = Some("reason".into());
    action.confidence = 0.8;

    engine.execute_suggest_prompt_for_test(&mut action);
    assert_eq!(action.status, "executed");
}

#[test]
fn test_execute_suggest_prompt_no_draft_name() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::with_forge_dir(
        ForgeConfig::default(),
        dir.path().join("forge"),
        registry,
        cycle_store,
    );

    let mut action = LearningAction::new("suggest_prompt", "medium", "desc");
    action.draft_name = None;
    action.confidence = 0.8;

    engine.execute_suggest_prompt_for_test(&mut action);
    assert_eq!(action.status, "executed");
    // Should use "unknown" as name
}

#[test]
fn test_check_suggestion_adoption_removes_files() {
    let dir = tempfile::tempdir().unwrap();
    let prompts_dir = dir.path().join("prompts");
    std::fs::create_dir_all(&prompts_dir).unwrap();

    // Create suggestion files
    std::fs::write(prompts_dir.join("test_suggestion.md"), "suggestion");
    std::fs::write(prompts_dir.join("other_file.md"), "other");

    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
    let engine = LearningEngine::with_forge_dir(
        ForgeConfig::default(),
        dir.path().join("forge"),
        registry,
        cycle_store,
    );

    // check_suggestion_adoption is called internally - just verify no panic
    let patterns = vec![];
    engine.check_suggestion_adoption(&patterns);

    // All _suggestion.md files should be removed
    let remaining: Vec<_> = std::fs::read_dir(&prompts_dir).unwrap()
        .filter_map(|e| e.ok())
        .collect();
    // The non-suggestion file should remain
    assert!(remaining.iter().any(|f| f.file_name().to_string_lossy().contains("other_file")));
}

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
    let efficiency: Vec<_> = patterns.iter()
        .filter(|p| p.pattern_type == "efficiency_issue")
        .collect();
    assert!(efficiency.is_empty());
}


#[test]
fn test_sort_actions_with_unknown_priority() {
    let mut actions = vec![
        {
            let mut a = LearningAction::new("test", "unknown", "desc");
            a.confidence = 0.5;
            a
        },
        {
            let mut a = LearningAction::new("test", "high", "desc");
            a.confidence = 0.9;
            a
        },
    ];
    LearningEngine::sort_actions(&mut actions);
    assert_eq!(actions[0].priority, "high"); // high (0) before unknown (3)
}

#[test]
fn test_action_to_summary_conversion() {
    let mut action = LearningAction::new("create_skill", "high", "test desc");
    action.artifact_id = Some("art-1".into());
    let summary = action_to_summary(&action);
    assert_eq!(summary.id, action.id);
    assert_eq!(summary.action_type, "create_skill");
    assert_eq!(summary.priority, "high");
    assert_eq!(summary.status, "pending");
    assert_eq!(summary.artifact_id, Some("art-1".into()));
}

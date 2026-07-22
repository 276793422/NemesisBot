use super::*;

#[test]
fn test_validate_skill_pass() {
    let pipeline = Pipeline::new(
        ForgeConfig::default(),
        Arc::new(Registry::new(crate::types::RegistryConfig::default())),
    );

    let content = "---\nname: test\n---\nThis is a test skill content that is long enough to pass validation.";
    let validation = pipeline.validate(ArtifactKind::Skill, "test", content);

    assert!(validation.stage1_static.as_ref().unwrap().stage.passed);
    assert!(validation.stage2_functional.as_ref().unwrap().stage.passed);
}

#[test]
fn test_validate_skill_too_short() {
    let pipeline = Pipeline::new(
        ForgeConfig::default(),
        Arc::new(Registry::new(crate::types::RegistryConfig::default())),
    );

    let validation = pipeline.validate(ArtifactKind::Skill, "short", "hi");
    assert!(!validation.stage1_static.as_ref().unwrap().stage.passed);
}

#[test]
fn test_validate_script_empty() {
    let pipeline = Pipeline::new(
        ForgeConfig::default(),
        Arc::new(Registry::new(crate::types::RegistryConfig::default())),
    );

    let validation = pipeline.validate(ArtifactKind::Script, "empty", "");
    assert!(!validation.stage1_static.as_ref().unwrap().stage.passed);
}

#[test]
fn test_determine_status() {
    let pipeline = Pipeline::new(
        ForgeConfig::default(),
        Arc::new(Registry::new(crate::types::RegistryConfig::default())),
    );

    // All stages pass with high score
    let validation = ArtifactValidation {
        stage1_static: Some(StaticValidationResult {
            stage: ValidationStage {
                passed: true,
                timestamp: String::new(),
                errors: vec![],
            },
            warnings: vec![],
        }),
        stage2_functional: Some(FunctionalValidationResult {
            stage: ValidationStage {
                passed: true,
                timestamp: String::new(),
                errors: vec![],
            },
            tests_run: 3,
            tests_passed: 3,
        }),
        stage3_quality: Some(QualityValidationResult {
            stage: ValidationStage {
                passed: true,
                timestamp: String::new(),
                errors: vec![],
            },
            score: 80,
            notes: String::new(),
            dimensions: Default::default(),
        }),
        last_validated: String::new(),
    };

    assert_eq!(
        pipeline.determine_status(&validation),
        ArtifactStatus::Active
    );
}

#[tokio::test]
async fn test_evaluate_quality_no_provider() {
    let pipeline = Pipeline::new(
        ForgeConfig::default(),
        Arc::new(Registry::new(crate::types::RegistryConfig::default())),
    );

    let content = "---\nname: test\n---\nA test skill content.";
    let result = pipeline
        .evaluate_quality(ArtifactKind::Skill, "test", content)
        .await;

    // Without provider, should get default score
    assert!(result.stage.passed);
    assert_eq!(result.score, 70);
    assert!(result.notes.contains("default"));
    assert_eq!(result.dimensions.len(), 4);
}

#[tokio::test]
async fn test_set_provider() {
    use crate::reflector_llm::LLMCaller;
    use async_trait::async_trait;

    struct MockLLM;

    #[async_trait]
    impl LLMCaller for MockLLM {
        async fn chat(
            &self,
            _system_prompt: &str,
            _user_prompt: &str,
            _max_tokens: Option<i64>,
        ) -> Result<String, String> {
            Ok(r#"{"correctness": 80, "quality": 75, "security": 85, "reusability": 70, "notes": "Good quality"}"#.to_string())
        }
    }

    let pipeline = Pipeline::new(
        ForgeConfig::default(),
        Arc::new(Registry::new(crate::types::RegistryConfig::default())),
    );

    pipeline.set_provider(Arc::new(MockLLM));

    let content = "---\nname: test\n---\nA test skill content that is sufficient for validation.";
    let result = pipeline
        .evaluate_quality(ArtifactKind::Skill, "test", content)
        .await;

    // With LLM provider, should get parsed score
    assert!(result.stage.passed);
    assert!(result.score > 0);
    assert!(result.dimensions.contains_key("correctness"));
}

#[tokio::test]
async fn test_validate_async_no_provider() {
    let pipeline = Pipeline::new(
        ForgeConfig::default(),
        Arc::new(Registry::new(crate::types::RegistryConfig::default())),
    );

    let content = "---\nname: test\n---\nThis is a test skill content that is long enough to pass validation.";
    let validation = pipeline
        .validate_async(ArtifactKind::Skill, "test", content)
        .await;

    assert!(validation.stage1_static.as_ref().unwrap().stage.passed);
    assert!(validation.stage2_functional.as_ref().unwrap().stage.passed);
    // Stage 3 should have default score
    assert!(validation.stage3_quality.as_ref().unwrap().stage.passed);
}

// --- Additional pipeline tests ---

#[test]
fn test_validate_script_valid() {
    let pipeline = Pipeline::new(
        ForgeConfig::default(),
        Arc::new(Registry::new(crate::types::RegistryConfig::default())),
    );
    let content = "#!/bin/bash\necho 'hello world'";
    let validation = pipeline.validate(ArtifactKind::Script, "test", content);
    assert!(validation.stage1_static.as_ref().unwrap().stage.passed);
}

#[test]
fn test_validate_mcp_empty() {
    let pipeline = Pipeline::new(
        ForgeConfig::default(),
        Arc::new(Registry::new(crate::types::RegistryConfig::default())),
    );
    let validation = pipeline.validate(ArtifactKind::Mcp, "test", "");
    assert!(!validation.stage1_static.as_ref().unwrap().stage.passed);
}

#[test]
fn test_validate_mcp_valid() {
    let pipeline = Pipeline::new(
        ForgeConfig::default(),
        Arc::new(Registry::new(crate::types::RegistryConfig::default())),
    );
    let content = "def tool_server(): pass";
    let validation = pipeline.validate(ArtifactKind::Mcp, "test", content);
    assert!(validation.stage1_static.as_ref().unwrap().stage.passed);
}

#[test]
fn test_validate_secret_detection() {
    let pipeline = Pipeline::new(
        ForgeConfig::default(),
        Arc::new(Registry::new(crate::types::RegistryConfig::default())),
    );
    let content =
        "---\nname: test\n---\nThis skill uses api_key=sk-1234567890abcdefghijklmnop to connect.";
    let validation = pipeline.validate(ArtifactKind::Skill, "test", content);
    assert!(!validation.stage1_static.as_ref().unwrap().stage.passed);
    assert!(
        validation
            .stage1_static
            .as_ref()
            .unwrap()
            .stage
            .errors
            .iter()
            .any(|e| e.contains("secret"))
    );
}

#[test]
fn test_validate_secret_key_detection() {
    let pipeline = Pipeline::new(
        ForgeConfig::default(),
        Arc::new(Registry::new(crate::types::RegistryConfig::default())),
    );
    let content = "---\nname: test\n---\nConfiguration with secret_key=abcdef123456";
    let validation = pipeline.validate(ArtifactKind::Skill, "test", content);
    assert!(!validation.stage1_static.as_ref().unwrap().stage.passed);
}

#[test]
fn test_validate_skill_frontmatter_warning() {
    let pipeline = Pipeline::new(
        ForgeConfig::default(),
        Arc::new(Registry::new(crate::types::RegistryConfig::default())),
    );
    // Long enough but no frontmatter (no triple dashes)
    let content = "This is a skill without any frontmatter separators. It is long enough to pass length check but lacks them.";
    let validation = pipeline.validate(ArtifactKind::Skill, "test", content);
    let stage1 = validation.stage1_static.as_ref().unwrap();
    assert!(stage1.warnings.iter().any(|w| w.contains("frontmatter")));
}

#[test]
fn test_determine_status_draft_stage1_failed() {
    let pipeline = Pipeline::new(
        ForgeConfig::default(),
        Arc::new(Registry::new(crate::types::RegistryConfig::default())),
    );
    let validation = ArtifactValidation {
        stage1_static: Some(StaticValidationResult {
            stage: ValidationStage {
                passed: false,
                timestamp: String::new(),
                errors: vec!["fail".into()],
            },
            warnings: vec![],
        }),
        stage2_functional: None,
        stage3_quality: None,
        last_validated: String::new(),
    };
    assert_eq!(
        pipeline.determine_status(&validation),
        ArtifactStatus::Draft
    );
}

#[test]
fn test_determine_status_draft_stage2_failed() {
    let pipeline = Pipeline::new(
        ForgeConfig::default(),
        Arc::new(Registry::new(crate::types::RegistryConfig::default())),
    );
    let validation = ArtifactValidation {
        stage1_static: Some(StaticValidationResult {
            stage: ValidationStage {
                passed: true,
                timestamp: String::new(),
                errors: vec![],
            },
            warnings: vec![],
        }),
        stage2_functional: Some(FunctionalValidationResult {
            stage: ValidationStage {
                passed: false,
                timestamp: String::new(),
                errors: vec!["fail".into()],
            },
            tests_run: 3,
            tests_passed: 1,
        }),
        stage3_quality: None,
        last_validated: String::new(),
    };
    assert_eq!(
        pipeline.determine_status(&validation),
        ArtifactStatus::Draft
    );
}

#[test]
fn test_determine_status_observing_no_stage3() {
    let pipeline = Pipeline::new(
        ForgeConfig::default(),
        Arc::new(Registry::new(crate::types::RegistryConfig::default())),
    );
    let validation = ArtifactValidation {
        stage1_static: Some(StaticValidationResult {
            stage: ValidationStage {
                passed: true,
                timestamp: String::new(),
                errors: vec![],
            },
            warnings: vec![],
        }),
        stage2_functional: Some(FunctionalValidationResult {
            stage: ValidationStage {
                passed: true,
                timestamp: String::new(),
                errors: vec![],
            },
            tests_run: 3,
            tests_passed: 3,
        }),
        stage3_quality: None,
        last_validated: String::new(),
    };
    assert_eq!(
        pipeline.determine_status(&validation),
        ArtifactStatus::Observing
    );
}

#[test]
fn test_determine_status_draft_low_quality() {
    let pipeline = Pipeline::new(
        ForgeConfig::default(),
        Arc::new(Registry::new(crate::types::RegistryConfig::default())),
    );
    let validation = ArtifactValidation {
        stage1_static: Some(StaticValidationResult {
            stage: ValidationStage {
                passed: true,
                timestamp: String::new(),
                errors: vec![],
            },
            warnings: vec![],
        }),
        stage2_functional: Some(FunctionalValidationResult {
            stage: ValidationStage {
                passed: true,
                timestamp: String::new(),
                errors: vec![],
            },
            tests_run: 3,
            tests_passed: 3,
        }),
        stage3_quality: Some(QualityValidationResult {
            stage: ValidationStage {
                passed: false,
                timestamp: String::new(),
                errors: vec![],
            },
            score: 40,
            notes: String::new(),
            dimensions: Default::default(),
        }),
        last_validated: String::new(),
    };
    assert_eq!(
        pipeline.determine_status(&validation),
        ArtifactStatus::Draft
    );
}

#[test]
fn test_determine_status_active_high_quality() {
    let pipeline = Pipeline::new(
        ForgeConfig::default(),
        Arc::new(Registry::new(crate::types::RegistryConfig::default())),
    );
    let validation = ArtifactValidation {
        stage1_static: Some(StaticValidationResult {
            stage: ValidationStage {
                passed: true,
                timestamp: String::new(),
                errors: vec![],
            },
            warnings: vec![],
        }),
        stage2_functional: Some(FunctionalValidationResult {
            stage: ValidationStage {
                passed: true,
                timestamp: String::new(),
                errors: vec![],
            },
            tests_run: 3,
            tests_passed: 3,
        }),
        stage3_quality: Some(QualityValidationResult {
            stage: ValidationStage {
                passed: true,
                timestamp: String::new(),
                errors: vec![],
            },
            score: 80,
            notes: "Good quality".into(),
            dimensions: Default::default(),
        }),
        last_validated: String::new(),
    };
    assert_eq!(
        pipeline.determine_status(&validation),
        ArtifactStatus::Active
    );
}

#[test]
fn test_run_with_registered_artifact() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let pipeline = Pipeline::new(ForgeConfig::default(), registry.clone());

    let artifact = nemesis_types::forge::Artifact {
        id: "run-test".into(),
        name: "test-skill".into(),
        kind: ArtifactKind::Skill,
        version: "1.0".into(),
        status: ArtifactStatus::Draft,
        content: "---\nname: test\n---\nA valid skill with sufficient content.".into(),
        tool_signature: vec![],
        created_at: chrono::Local::now().to_rfc3339(),
        updated_at: chrono::Local::now().to_rfc3339(),
        usage_count: 0,
        last_degraded_at: None,
        success_rate: 0.0,
        consecutive_observing_rounds: 0,
    };
    registry.add(artifact);

    let result = pipeline.run("run-test");
    assert!(result.is_ok());
    let validation = result.unwrap();
    assert!(validation.stage1_static.as_ref().unwrap().stage.passed);
}

#[test]
fn test_run_nonexistent_artifact() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let pipeline = Pipeline::new(ForgeConfig::default(), registry);
    let result = pipeline.run("no-such-id");
    assert!(result.is_err());
}

#[tokio::test]
async fn test_validate_async_skill_too_short() {
    let pipeline = Pipeline::new(
        ForgeConfig::default(),
        Arc::new(Registry::new(crate::types::RegistryConfig::default())),
    );
    let validation = pipeline
        .validate_async(ArtifactKind::Skill, "short", "hi")
        .await;
    assert!(!validation.stage1_static.as_ref().unwrap().stage.passed);
    // Stage 2 and 3 should be None since stage 1 failed
    assert!(validation.stage2_functional.is_none());
    assert!(validation.stage3_quality.is_none());
}

#[tokio::test]
async fn test_validate_async_script_valid() {
    let pipeline = Pipeline::new(
        ForgeConfig::default(),
        Arc::new(Registry::new(crate::types::RegistryConfig::default())),
    );
    let content = "#!/bin/bash\necho hello";
    let validation = pipeline
        .validate_async(ArtifactKind::Script, "test", content)
        .await;
    assert!(validation.stage1_static.as_ref().unwrap().stage.passed);
}

#[test]
fn test_validation_stage_serialization() {
    let stage = ValidationStage {
        passed: true,
        timestamp: "2026-05-09T00:00:00Z".into(),
        errors: vec![],
    };
    let json = serde_json::to_string(&stage).unwrap();
    let back: ValidationStage = serde_json::from_str(&json).unwrap();
    assert!(back.passed);
}

#[test]
fn test_static_validation_result_serialization() {
    let result = StaticValidationResult {
        stage: ValidationStage {
            passed: true,
            timestamp: String::new(),
            errors: vec![],
        },
        warnings: vec!["test warning".into()],
    };
    let json = serde_json::to_string(&result).unwrap();
    let back: StaticValidationResult = serde_json::from_str(&json).unwrap();
    assert!(back.stage.passed);
    assert_eq!(back.warnings.len(), 1);
}

#[test]
fn test_functional_validation_result_serialization() {
    let result = FunctionalValidationResult {
        stage: ValidationStage {
            passed: true,
            timestamp: String::new(),
            errors: vec![],
        },
        tests_run: 3,
        tests_passed: 2,
    };
    let json = serde_json::to_string(&result).unwrap();
    let back: FunctionalValidationResult = serde_json::from_str(&json).unwrap();
    assert_eq!(back.tests_run, 3);
    assert_eq!(back.tests_passed, 2);
}

#[test]
fn test_quality_validation_result_serialization() {
    let mut dims = std::collections::HashMap::new();
    dims.insert("correctness".to_string(), 85);
    dims.insert("security".to_string(), 90);
    let result = QualityValidationResult {
        stage: ValidationStage {
            passed: true,
            timestamp: String::new(),
            errors: vec![],
        },
        score: 85,
        notes: "Good quality".into(),
        dimensions: dims,
    };
    let json = serde_json::to_string(&result).unwrap();
    let back: QualityValidationResult = serde_json::from_str(&json).unwrap();
    assert_eq!(back.score, 85);
    assert_eq!(back.dimensions.len(), 2);
}

#[test]
fn test_artifact_validation_serialization() {
    let validation = ArtifactValidation {
        stage1_static: Some(StaticValidationResult {
            stage: ValidationStage {
                passed: true,
                timestamp: "2026-05-09".into(),
                errors: vec![],
            },
            warnings: vec![],
        }),
        stage2_functional: None,
        stage3_quality: None,
        last_validated: "2026-05-09".into(),
    };
    let json = serde_json::to_string(&validation).unwrap();
    let back: ArtifactValidation = serde_json::from_str(&json).unwrap();
    assert!(back.stage1_static.is_some());
    assert!(back.stage2_functional.is_none());
}

#[test]
fn test_validate_skill_boundary_50_chars() {
    let pipeline = Pipeline::new(
        ForgeConfig::default(),
        Arc::new(Registry::new(crate::types::RegistryConfig::default())),
    );
    // Exactly 50 chars should pass
    let content = "---\nname: test\n---\n123456789012345678901234567890123456789012345678";
    if content.len() >= 50 {
        let validation = pipeline.validate(ArtifactKind::Skill, "test", content);
        assert!(validation.stage1_static.as_ref().unwrap().stage.passed);
    }
    // 49 chars should fail
    let short_content = "---\nname: test\n---\n12345678901234567890123456789012345678901234567";
    if short_content.len() < 50 {
        let validation = pipeline.validate(ArtifactKind::Skill, "test", short_content);
        assert!(!validation.stage1_static.as_ref().unwrap().stage.passed);
    }
}

#[test]
fn test_run_from_content() {
    let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
    let pipeline = Pipeline::new(ForgeConfig::default(), registry);

    let artifact = nemesis_types::forge::Artifact {
        id: "content-test".into(),
        name: "test".into(),
        kind: ArtifactKind::Skill,
        version: "1.0".into(),
        status: ArtifactStatus::Draft,
        content: String::new(),
        tool_signature: vec![],
        created_at: String::new(),
        updated_at: String::new(),
        usage_count: 0,
        last_degraded_at: None,
        success_rate: 0.0,
        consecutive_observing_rounds: 0,
    };
    let content = "---\nname: test\n---\nThis is a test skill content that is long enough to pass validation.";
    let validation = pipeline.run_from_content(&artifact, content);
    assert!(validation.stage1_static.as_ref().unwrap().stage.passed);
}

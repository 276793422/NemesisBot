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
        deployed_at: None,
        last_efficacy: None,
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
    // 生产闸（validate_static）是字节长度严格 `< 50`。旧版用 if 护栏包断言，
    // 但「49 字符」样实为 66 字节——护栏恒假，失败分支从未执行（死测试）。
    // 现按真实边界构造 49/50 字节两样，无护栏直接断言两侧。
    let prefix = "---\nname: test\n---\n";
    let exact_50 = format!("{}{}", prefix, "5".repeat(50 - prefix.len()));
    let short_49 = format!("{}{}", prefix, "4".repeat(49 - prefix.len()));
    assert_eq!(exact_50.len(), 50);
    assert_eq!(short_49.len(), 49);

    let ok = pipeline.validate(ArtifactKind::Skill, "test", &exact_50);
    assert!(ok.stage1_static.as_ref().unwrap().stage.passed);

    let short = pipeline.validate(ArtifactKind::Skill, "test", &short_49);
    let stage1 = short.stage1_static.as_ref().unwrap();
    assert!(!stage1.stage.passed);
    assert!(
        stage1.stage.errors.iter().any(|e| e.contains("too short")),
        "expected too-short error, got {:?}",
        stage1.stage.errors
    );
    // stage1 失败必须短路：stage2/3 不执行。
    assert!(short.stage2_functional.is_none());
    assert!(short.stage3_quality.is_none());
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
        deployed_at: None,
        last_efficacy: None,
    };
    let content = "---\nname: test\n---\nThis is a test skill content that is long enough to pass validation.";
    let validation = pipeline.run_from_content(&artifact, content);
    assert!(validation.stage1_static.as_ref().unwrap().stage.passed);
}

// ---- S8 coverage batch ---- (quality-hardening goal 冲刺 S8)

#[test]
fn test_s8_validate_stage2_failure_skips_stage3() {
    let pipeline = Pipeline::new(
        ForgeConfig::default(),
        Arc::new(Registry::new(crate::types::RegistryConfig::default())),
    );

    // Static passes (>=50 chars, no errors) but the simple functional check
    // fails: the content contains no '#' and no '-' anywhere (no frontmatter,
    let content = "This is a long enough plain text without any hash or dash characters inside it, exceeding fifty chars.";
    let validation = pipeline.validate(ArtifactKind::Skill, "demo", content);
    assert!(validation.stage1_static.as_ref().unwrap().stage.passed);
    assert!(!validation.stage2_functional.as_ref().unwrap().stage.passed);
    // Stage 3 must not run after a stage 2 failure.
    assert!(validation.stage3_quality.is_none());
}

#[test]
fn test_s8_determine_status_all_stages_none_returns_observing() {
    let pipeline = Pipeline::new(
        ForgeConfig::default(),
        Arc::new(Registry::new(crate::types::RegistryConfig::default())),
    );

    let validation = ArtifactValidation {
        stage1_static: None,
        stage2_functional: None,
        stage3_quality: None,
        last_validated: chrono::Local::now().to_rfc3339(),
    };
    assert_eq!(
        pipeline.determine_status(&validation),
        ArtifactStatus::Observing
    );
}

#[tokio::test]
async fn test_s8_evaluate_quality_float_scores_script_kind() {
    use crate::reflector_llm::LLMCaller;
    use async_trait::async_trait;

    struct MockFloatLLM;

    #[async_trait]
    impl LLMCaller for MockFloatLLM {
        async fn chat(
            &self,
            _system_prompt: &str,
            _user_prompt: &str,
            _max_tokens: Option<i64>,
        ) -> Result<String, String> {
            // Float values exercise the as_f64() fallback (as_u64() is None).
            Ok(r#"{"correctness": 90.5, "quality": 80.5, "security": 85.0, "reusability": 70.5, "notes": "float scores"}"#.to_string())
        }
    }

    let pipeline = Pipeline::new(
        ForgeConfig::default(),
        Arc::new(Registry::new(crate::types::RegistryConfig::default())),
    );
    pipeline.set_provider(Arc::new(MockFloatLLM));

    // Script kind → the "script" kind_str arm.
    let result = pipeline
        .evaluate_quality(ArtifactKind::Script, "demo", "echo hello")
        .await;
    assert!(result.stage.passed);
    assert!(result.score > 0);
    assert_eq!(result.notes, "float scores");
    // 90*40/100 + 80*20/100 + 85*20/100 + 70*20/100 = 36+16+17+14 = 83
    assert_eq!(result.score, 83);

    // Mcp kind → the "mcp" kind_str arm (reuse same provider).
    let result2 = pipeline
        .evaluate_quality(ArtifactKind::Mcp, "demo2", "def x(): pass")
        .await;
    assert!(result2.score > 0);
}

#[tokio::test]
async fn test_s8_evaluate_quality_unparseable_llm_response() {
    use crate::reflector_llm::LLMCaller;
    use async_trait::async_trait;

    struct MockGarbageLLM;

    #[async_trait]
    impl LLMCaller for MockGarbageLLM {
        async fn chat(
            &self,
            _system_prompt: &str,
            _user_prompt: &str,
            _max_tokens: Option<i64>,
        ) -> Result<String, String> {
            Ok("this is not json at all".to_string())
        }
    }

    let pipeline = Pipeline::new(
        ForgeConfig::default(),
        Arc::new(Registry::new(crate::types::RegistryConfig::default())),
    );
    pipeline.set_provider(Arc::new(MockGarbageLLM));

    let result = pipeline
        .evaluate_quality(ArtifactKind::Skill, "demo", "content")
        .await;
    assert!(!result.stage.passed);
    assert_eq!(result.score, 0);
    assert!(
        result
            .stage
            .errors
            .iter()
            .any(|e| e.contains("Failed to parse LLM response as JSON"))
    );
}

#[tokio::test]
async fn test_s8_evaluate_quality_llm_call_error() {
    use crate::reflector_llm::LLMCaller;
    use async_trait::async_trait;

    struct MockFailingLLM;

    #[async_trait]
    impl LLMCaller for MockFailingLLM {
        async fn chat(
            &self,
            _system_prompt: &str,
            _user_prompt: &str,
            _max_tokens: Option<i64>,
        ) -> Result<String, String> {
            Err("connection refused".to_string())
        }
    }

    let pipeline = Pipeline::new(
        ForgeConfig::default(),
        Arc::new(Registry::new(crate::types::RegistryConfig::default())),
    );
    pipeline.set_provider(Arc::new(MockFailingLLM));

    let result = pipeline
        .evaluate_quality(ArtifactKind::Skill, "demo", "content")
        .await;
    assert!(!result.stage.passed);
    assert_eq!(result.score, 0);
    assert!(
        result
            .stage
            .errors
            .iter()
            .any(|e| e.contains("LLM call failed: connection refused"))
    );
}

#[tokio::test]
async fn test_s8_validate_async_stage2_failure_skips_stage3() {
    let pipeline = Pipeline::new(
        ForgeConfig::default(),
        Arc::new(Registry::new(crate::types::RegistryConfig::default())),
    );

    // Same shape as the sync variant, but exercising validate_async's own
    // stage-2 early return: no '#' and no '-' anywhere, >=50 chars.
    let content = "This is a long enough plain text without any hash or dash characters inside it, exceeding fifty chars.";
    let validation = pipeline
        .validate_async(ArtifactKind::Skill, "demo", content)
        .await;
    assert!(validation.stage1_static.as_ref().unwrap().stage.passed);
    assert!(!validation.stage2_functional.as_ref().unwrap().stage.passed);
    assert!(validation.stage3_quality.is_none());
}

// 三轮复查（2026-10-14）：Stage 2 judge 层 caller 槽优先级——judge_caller
// （杂务回落链）优先于 llm_caller（主模型）；judge_caller 缺席回落主模型。

#[tokio::test]
async fn behavior_gate_prefers_judge_caller_over_main_caller() {
    use crate::reflector_llm::LLMCaller;
    use async_trait::async_trait;

    struct ScriptedCaller {
        response: &'static str,
    }

    #[async_trait]
    impl LLMCaller for ScriptedCaller {
        async fn chat(
            &self,
            _system_prompt: &str,
            _user_prompt: &str,
            _max_tokens: Option<i64>,
        ) -> Result<String, String> {
            Ok(self.response.to_string())
        }
    }

    struct StubRunner;

    #[async_trait]
    impl crate::skill_tests::SkillTestRunner for StubRunner {
        async fn run_case(
            &self,
            _skill_content: &str,
            _case: &crate::skill_tests::TestCase,
        ) -> Result<crate::skill_tests::ExecutionTranscript, String> {
            Ok(crate::skill_tests::ExecutionTranscript {
                finals: vec!["done".into()],
                ..Default::default()
            })
        }
    }

    let content = "---\nname: test\ntests:\n  - name: judge-case\n    turns:\n      - user: hello\n    judge:\n      rubric: 回复得体\n---\nA test skill content that is sufficient for validation.";

    // 两个 caller 给出相反裁决：主模型判 fail，杂务链判 pass——
    // gate 必须采信 judge_caller。
    let failing_judge = r#"{"score": 0.1, "summary": "fail"}"#;
    let passing_judge = r#"{"score": 0.9, "summary": "pass"}"#;

    let pipeline = Pipeline::new(
        ForgeConfig::default(),
        Arc::new(Registry::new(crate::types::RegistryConfig::default())),
    );
    pipeline.set_skill_test_runner(Arc::new(StubRunner));
    pipeline.set_provider(Arc::new(ScriptedCaller {
        response: failing_judge,
    }));
    pipeline.set_judge_caller(Arc::new(ScriptedCaller {
        response: passing_judge,
    }));
    let v = pipeline
        .validate_async(ArtifactKind::Skill, "t", content)
        .await;
    let s2 = v.stage2_functional.as_ref().unwrap();
    assert!(
        s2.stage.passed,
        "judge_caller 应优先: {:?}",
        s2.stage.errors
    );

    // judge_caller 缺席 = 回落主模型槽（fail 裁决生效）。
    let pipeline2 = Pipeline::new(
        ForgeConfig::default(),
        Arc::new(Registry::new(crate::types::RegistryConfig::default())),
    );
    pipeline2.set_skill_test_runner(Arc::new(StubRunner));
    pipeline2.set_provider(Arc::new(ScriptedCaller {
        response: failing_judge,
    }));
    let v2 = pipeline2
        .validate_async(ArtifactKind::Skill, "t", content)
        .await;
    let s2b = v2.stage2_functional.as_ref().unwrap();
    assert!(
        !s2b.stage.passed,
        "judge_caller 缺席应回落主模型: {:?}",
        s2b.stage.errors
    );
}

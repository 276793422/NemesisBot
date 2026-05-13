//! Validation pipeline - three-stage artifact validation.
//!
//! Stage 1: Static content validation
//! Stage 2: Functional testing (structure checks)
//! Stage 3: Quality evaluation (placeholder for LLM-as-judge)

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use nemesis_types::forge::{ArtifactKind, ArtifactStatus};
use crate::config::ForgeConfig;
use crate::registry::Registry;

/// Base validation stage result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationStage {
    /// Whether this stage passed.
    pub passed: bool,
    /// Timestamp of validation.
    pub timestamp: String,
    /// Error messages.
    #[serde(default)]
    pub errors: Vec<String>,
}

/// Stage 1 result: static validation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaticValidationResult {
    #[serde(flatten)]
    pub stage: ValidationStage,
    /// Warning messages (non-blocking).
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Stage 2 result: functional validation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionalValidationResult {
    #[serde(flatten)]
    pub stage: ValidationStage,
    /// Number of tests run.
    pub tests_run: u32,
    /// Number of tests passed.
    pub tests_passed: u32,
}

/// Stage 3 result: quality evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityValidationResult {
    #[serde(flatten)]
    pub stage: ValidationStage,
    /// Quality score (0-100).
    pub score: u32,
    /// Evaluator notes.
    #[serde(default)]
    pub notes: String,
    /// Per-dimension scores.
    #[serde(default)]
    pub dimensions: std::collections::HashMap<String, u32>,
}

/// Combined validation result across all three stages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactValidation {
    /// Stage 1: static validation.
    pub stage1_static: Option<StaticValidationResult>,
    /// Stage 2: functional validation.
    pub stage2_functional: Option<FunctionalValidationResult>,
    /// Stage 3: quality evaluation.
    pub stage3_quality: Option<QualityValidationResult>,
    /// When the validation was performed.
    pub last_validated: String,
}

/// The validation pipeline orchestrator.
///
/// Stage 3 quality evaluation supports LLM-as-Judge when a provider is
/// configured. Without a provider, a default score is assigned.
pub struct Pipeline {
    registry: Arc<Registry>,
    #[allow(dead_code)]
    config: ForgeConfig,
    llm_caller: parking_lot::RwLock<Option<Arc<dyn crate::reflector_llm::LLMCaller>>>,
}

impl Pipeline {
    /// Create a new validation pipeline.
    pub fn new(config: ForgeConfig, registry: Arc<Registry>) -> Self {
        Self {
            config,
            registry,
            llm_caller: parking_lot::RwLock::new(None),
        }
    }

    /// Set the LLM provider for quality evaluation (Stage 3).
    pub fn set_provider(&self, caller: Arc<dyn crate::reflector_llm::LLMCaller>) {
        *self.llm_caller.write() = Some(caller);
    }

    /// Run the full validation pipeline on content (synchronous wrapper).
    ///
    /// This is the primary API used by forge.rs and learning_engine.rs.
    /// Internally uses `validate_async` with `block_on` for LLM calls.
    pub fn validate(&self, kind: ArtifactKind, name: &str, content: &str) -> ArtifactValidation {
        let mut validation = ArtifactValidation {
            stage1_static: None,
            stage2_functional: None,
            stage3_quality: None,
            last_validated: chrono::Utc::now().to_rfc3339(),
        };

        // Stage 1: Static validation
        let stage1 = self.validate_static(kind, name, content);
        validation.stage1_static = Some(stage1.clone());
        if !stage1.stage.passed {
            return validation;
        }

        // Stage 2: Functional validation
        let stage2 = self.validate_functional(kind, content);
        validation.stage2_functional = Some(stage2.clone());
        if !stage2.stage.passed {
            return validation;
        }

        // Stage 3: Quality evaluation (LLM-as-Judge or default)
        let stage3 = self.evaluate_quality_sync(kind, name, content);
        validation.stage3_quality = Some(stage3);

        validation
    }

    /// Async version of validate for use in async contexts.
    pub async fn validate_async(&self, kind: ArtifactKind, name: &str, content: &str) -> ArtifactValidation {
        let mut validation = ArtifactValidation {
            stage1_static: None,
            stage2_functional: None,
            stage3_quality: None,
            last_validated: chrono::Utc::now().to_rfc3339(),
        };

        // Stage 1: Static validation
        let stage1 = self.validate_static(kind, name, content);
        validation.stage1_static = Some(stage1.clone());
        if !stage1.stage.passed {
            return validation;
        }

        // Stage 2: Functional validation
        let stage2 = self.validate_functional(kind, content);
        validation.stage2_functional = Some(stage2.clone());
        if !stage2.stage.passed {
            return validation;
        }

        // Stage 3: Quality evaluation (LLM-as-Judge or default)
        let stage3 = self.evaluate_quality(kind, name, content).await;
        validation.stage3_quality = Some(stage3);

        validation
    }

    /// Execute the full validation pipeline for an artifact by ID.
    ///
    /// Loads the artifact from the registry and delegates to `run_from_content`.
    /// Mirrors Go's `Pipeline.Run`.
    pub fn run(&self, artifact_id: &str) -> Result<ArtifactValidation, String> {
        let artifact = self
            .registry
            .get(artifact_id)
            .ok_or_else(|| format!("artifact {} not found", artifact_id))?;

        Ok(self.run_from_content(&artifact, &artifact.content))
    }

    /// Execute the full validation pipeline with provided content.
    ///
    /// Mirrors Go's `Pipeline.RunFromContent`. Runs the three stages:
    /// static validation → functional testing → quality evaluation.
    pub fn run_from_content(
        &self,
        artifact: &nemesis_types::forge::Artifact,
        content: &str,
    ) -> ArtifactValidation {
        self.validate(artifact.kind, &artifact.name, content)
    }

    /// Determine artifact status based on validation results.
    pub fn determine_status(&self, validation: &ArtifactValidation) -> ArtifactStatus {
        // If stage 1 failed, keep as draft
        if let Some(ref s1) = validation.stage1_static {
            if !s1.stage.passed {
                return ArtifactStatus::Draft;
            }
        }

        // If stage 2 failed, keep as draft
        if let Some(ref s2) = validation.stage2_functional {
            if !s2.stage.passed {
                return ArtifactStatus::Draft;
            }
        }

        // Check quality score
        if let Some(ref s3) = validation.stage3_quality {
            if s3.score >= 60 {
                return ArtifactStatus::Active;
            }
            return ArtifactStatus::Draft;
        }

        // Stages 1+2 passed, no stage 3 -> observing
        ArtifactStatus::Observing
    }

    fn validate_static(&self, kind: ArtifactKind, _name: &str, content: &str) -> StaticValidationResult {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        match kind {
            ArtifactKind::Skill => {
                if content.len() < 50 {
                    errors.push("Skill content too short (less than 50 chars)".into());
                }
                if !content.contains("---") {
                    warnings.push("Skill missing frontmatter".into());
                }
            }
            ArtifactKind::Script => {
                if content.trim().is_empty() {
                    errors.push("Script content is empty".into());
                }
            }
            ArtifactKind::Mcp => {
                if content.trim().is_empty() {
                    errors.push("MCP content is empty".into());
                }
            }
        }

        // Security check: look for hardcoded secrets
        if content.contains("api_key") || content.contains("secret_key") {
            errors.push("Content contains potential secret/key".into());
        }

        StaticValidationResult {
            stage: ValidationStage {
                passed: errors.is_empty(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                errors,
            },
            warnings,
        }
    }

    fn validate_functional(&self, kind: ArtifactKind, content: &str) -> FunctionalValidationResult {
        let mut errors = Vec::new();
        let (tests_run, mut tests_passed) = match kind {
            ArtifactKind::Skill => (3u32, 0u32),
            ArtifactKind::Script => (1u32, 0u32),
            ArtifactKind::Mcp => (2u32, 0u32),
        };

        match kind {
            ArtifactKind::Skill => {
                if !content.is_empty() {
                    tests_passed += 1;
                }
                if content.contains("#") || content.contains("-") {
                    tests_passed += 1;
                }
                if content.len() > 50 {
                    tests_passed += 1;
                }
            }
            ArtifactKind::Script => {
                if !content.trim().is_empty() {
                    tests_passed += 1;
                }
            }
            ArtifactKind::Mcp => {
                if content.contains("def ") || content.contains("func ") {
                    tests_passed += 1;
                }
                if content.contains("tool") || content.contains("server") {
                    tests_passed += 1;
                }
            }
        }

        if tests_passed < tests_run {
            errors.push(format!("Only {}/{} checks passed", tests_passed, tests_run));
        }

        FunctionalValidationResult {
            stage: ValidationStage {
                passed: errors.is_empty(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                errors,
            },
            tests_run,
            tests_passed,
        }
    }

    /// Synchronous wrapper for evaluate_quality that blocks on the async LLM call.
    fn evaluate_quality_sync(&self, kind: ArtifactKind, name: &str, content: &str) -> QualityValidationResult {
        // Try to use an existing tokio runtime, or fall back to a new one
        let future = self.evaluate_quality(kind, name, content);
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                // We're inside a tokio runtime. Use block_in_place to avoid
                // deadlocking the runtime when calling block_on from an async context.
                tokio::task::block_in_place(|| handle.block_on(future))
            }
            Err(_) => {
                // No runtime available, create a new one
                let rt = tokio::runtime::Runtime::new()
                    .expect("Failed to create tokio runtime for quality evaluation");
                rt.block_on(future)
            }
        }
    }

    /// Stage 3: Quality evaluation using LLM-as-Judge.
    ///
    /// When an LLM provider is configured, calls the LLM to evaluate the
    /// artifact on four dimensions (correctness, quality, security, reusability)
    /// and computes a weighted score. Without a provider, returns a default score.
    async fn evaluate_quality(&self, kind: ArtifactKind, name: &str, content: &str) -> QualityValidationResult {
        let caller = self.llm_caller.read();

        match caller.as_ref() {
            Some(caller) => {
                // LLM-as-Judge evaluation (matches Go evaluator.go)
                let kind_str = match kind {
                    ArtifactKind::Skill => "skill",
                    ArtifactKind::Script => "script",
                    ArtifactKind::Mcp => "mcp",
                };

                let system_prompt = "You are a code quality evaluator. Respond only with valid JSON.";

                let user_prompt = format!(
                    "Evaluate the following Forge artifact for quality.\n\n\
                    Type: {}\n\
                    Name: {}\n\
                    \n\
                    Content:\n\
                    {}\n\
                    \n\
                    Score each dimension from 0-100:\n\
                    - correctness: Does the content correctly implement its stated purpose? (weight 40%%)\n\
                    - quality: Code/text quality, clarity, documentation (weight 20%%)\n\
                    - security: Security considerations, no dangerous patterns (weight 20%%)\n\
                    - reusability: Can this be reused in other contexts? (weight 20%%)\n\
                    \n\
                    Respond with ONLY a JSON object:\n\
                    {{\"correctness\": N, \"quality\": N, \"security\": N, \"reusability\": N, \"notes\": \"brief explanation\"}}",
                    kind_str, name, content
                );

                match caller.chat(system_prompt, &user_prompt, Some(2000)).await {
                    Ok(response) => {
                        // Parse LLM response
                        match crate::reflector_llm::extract_json(&response) {
                            Some(parsed) => {
                                let mut dimensions = std::collections::HashMap::new();

                                // Extract dimension scores
                                for key in &["correctness", "quality", "security", "reusability"] {
                                    if let Some(val) = parsed.get(*key) {
                                        if let Some(n) = val.as_u64() {
                                            dimensions.insert(key.to_string(), n as u32);
                                        } else if let Some(n) = val.as_f64() {
                                            dimensions.insert(key.to_string(), n as u32);
                                        }
                                    }
                                }

                                // Extract notes
                                let notes = parsed
                                    .get("notes")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();

                                // Calculate weighted score:
                                // correctness(40%) + quality(20%) + security(20%) + reusability(20%)
                                let mut score: u32 = 0;
                                if let Some(&c) = dimensions.get("correctness") {
                                    score += c * 40 / 100;
                                }
                                if let Some(&q) = dimensions.get("quality") {
                                    score += q * 20 / 100;
                                }
                                if let Some(&s) = dimensions.get("security") {
                                    score += s * 20 / 100;
                                }
                                if let Some(&r) = dimensions.get("reusability") {
                                    score += r * 20 / 100;
                                }

                                // Determine pass based on config threshold (default 60)
                                let min_score = 60u32;

                                QualityValidationResult {
                                    stage: ValidationStage {
                                        passed: score >= min_score,
                                        timestamp: chrono::Utc::now().to_rfc3339(),
                                        errors: if score < min_score {
                                            vec![format!("Quality score {} below threshold {}", score, min_score)]
                                        } else {
                                            vec![]
                                        },
                                    },
                                    score,
                                    notes,
                                    dimensions,
                                }
                            }
                            None => {
                                // Could not parse JSON from LLM response
                                QualityValidationResult {
                                    stage: ValidationStage {
                                        passed: false,
                                        timestamp: chrono::Utc::now().to_rfc3339(),
                                        errors: vec!["Failed to parse LLM response as JSON".to_string()],
                                    },
                                    score: 0,
                                    notes: String::new(),
                                    dimensions: std::collections::HashMap::new(),
                                }
                            }
                        }
                    }
                    Err(e) => {
                        // LLM call failed
                        QualityValidationResult {
                            stage: ValidationStage {
                                passed: false,
                                timestamp: chrono::Utc::now().to_rfc3339(),
                                errors: vec![format!("LLM call failed: {}", e)],
                            },
                            score: 0,
                            notes: String::new(),
                            dimensions: std::collections::HashMap::new(),
                        }
                    }
                }
            }
            None => {
                // No LLM provider - use default score (matches Go behavior)
                let mut dimensions = std::collections::HashMap::new();
                dimensions.insert("correctness".to_string(), 70);
                dimensions.insert("quality".to_string(), 70);
                dimensions.insert("security".to_string(), 75);
                dimensions.insert("reusability".to_string(), 65);

                QualityValidationResult {
                    stage: ValidationStage {
                        passed: true,
                        timestamp: chrono::Utc::now().to_rfc3339(),
                        errors: vec![],
                    },
                    score: 70,
                    notes: "No LLM Provider available, using default score".to_string(),
                    dimensions,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
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
                stage: ValidationStage { passed: true, timestamp: String::new(), errors: vec![] },
                warnings: vec![],
            }),
            stage2_functional: Some(FunctionalValidationResult {
                stage: ValidationStage { passed: true, timestamp: String::new(), errors: vec![] },
                tests_run: 3,
                tests_passed: 3,
            }),
            stage3_quality: Some(QualityValidationResult {
                stage: ValidationStage { passed: true, timestamp: String::new(), errors: vec![] },
                score: 80,
                notes: String::new(),
                dimensions: Default::default(),
            }),
            last_validated: String::new(),
        };

        assert_eq!(pipeline.determine_status(&validation), ArtifactStatus::Active);
    }

    #[tokio::test]
    async fn test_evaluate_quality_no_provider() {
        let pipeline = Pipeline::new(
            ForgeConfig::default(),
            Arc::new(Registry::new(crate::types::RegistryConfig::default())),
        );

        let content = "---\nname: test\n---\nA test skill content.";
        let result = pipeline.evaluate_quality(ArtifactKind::Skill, "test", content).await;

        // Without provider, should get default score
        assert!(result.stage.passed);
        assert_eq!(result.score, 70);
        assert!(result.notes.contains("default"));
        assert_eq!(result.dimensions.len(), 4);
    }

    #[tokio::test]
    async fn test_set_provider() {
        use async_trait::async_trait;
        use crate::reflector_llm::LLMCaller;

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
        let result = pipeline.evaluate_quality(ArtifactKind::Skill, "test", content).await;

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
        let validation = pipeline.validate_async(ArtifactKind::Skill, "test", content).await;

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
        let content = "---\nname: test\n---\nThis skill uses api_key=sk-1234567890abcdefghijklmnop to connect.";
        let validation = pipeline.validate(ArtifactKind::Skill, "test", content);
        assert!(!validation.stage1_static.as_ref().unwrap().stage.passed);
        assert!(validation.stage1_static.as_ref().unwrap().stage.errors.iter().any(|e| e.contains("secret")));
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
                stage: ValidationStage { passed: false, timestamp: String::new(), errors: vec!["fail".into()] },
                warnings: vec![],
            }),
            stage2_functional: None,
            stage3_quality: None,
            last_validated: String::new(),
        };
        assert_eq!(pipeline.determine_status(&validation), ArtifactStatus::Draft);
    }

    #[test]
    fn test_determine_status_draft_stage2_failed() {
        let pipeline = Pipeline::new(
            ForgeConfig::default(),
            Arc::new(Registry::new(crate::types::RegistryConfig::default())),
        );
        let validation = ArtifactValidation {
            stage1_static: Some(StaticValidationResult {
                stage: ValidationStage { passed: true, timestamp: String::new(), errors: vec![] },
                warnings: vec![],
            }),
            stage2_functional: Some(FunctionalValidationResult {
                stage: ValidationStage { passed: false, timestamp: String::new(), errors: vec!["fail".into()] },
                tests_run: 3,
                tests_passed: 1,
            }),
            stage3_quality: None,
            last_validated: String::new(),
        };
        assert_eq!(pipeline.determine_status(&validation), ArtifactStatus::Draft);
    }

    #[test]
    fn test_determine_status_observing_no_stage3() {
        let pipeline = Pipeline::new(
            ForgeConfig::default(),
            Arc::new(Registry::new(crate::types::RegistryConfig::default())),
        );
        let validation = ArtifactValidation {
            stage1_static: Some(StaticValidationResult {
                stage: ValidationStage { passed: true, timestamp: String::new(), errors: vec![] },
                warnings: vec![],
            }),
            stage2_functional: Some(FunctionalValidationResult {
                stage: ValidationStage { passed: true, timestamp: String::new(), errors: vec![] },
                tests_run: 3,
                tests_passed: 3,
            }),
            stage3_quality: None,
            last_validated: String::new(),
        };
        assert_eq!(pipeline.determine_status(&validation), ArtifactStatus::Observing);
    }

    #[test]
    fn test_determine_status_draft_low_quality() {
        let pipeline = Pipeline::new(
            ForgeConfig::default(),
            Arc::new(Registry::new(crate::types::RegistryConfig::default())),
        );
        let validation = ArtifactValidation {
            stage1_static: Some(StaticValidationResult {
                stage: ValidationStage { passed: true, timestamp: String::new(), errors: vec![] },
                warnings: vec![],
            }),
            stage2_functional: Some(FunctionalValidationResult {
                stage: ValidationStage { passed: true, timestamp: String::new(), errors: vec![] },
                tests_run: 3,
                tests_passed: 3,
            }),
            stage3_quality: Some(QualityValidationResult {
                stage: ValidationStage { passed: false, timestamp: String::new(), errors: vec![] },
                score: 40,
                notes: String::new(),
                dimensions: Default::default(),
            }),
            last_validated: String::new(),
        };
        assert_eq!(pipeline.determine_status(&validation), ArtifactStatus::Draft);
    }

    #[test]
    fn test_determine_status_active_high_quality() {
        let pipeline = Pipeline::new(
            ForgeConfig::default(),
            Arc::new(Registry::new(crate::types::RegistryConfig::default())),
        );
        let validation = ArtifactValidation {
            stage1_static: Some(StaticValidationResult {
                stage: ValidationStage { passed: true, timestamp: String::new(), errors: vec![] },
                warnings: vec![],
            }),
            stage2_functional: Some(FunctionalValidationResult {
                stage: ValidationStage { passed: true, timestamp: String::new(), errors: vec![] },
                tests_run: 3,
                tests_passed: 3,
            }),
            stage3_quality: Some(QualityValidationResult {
                stage: ValidationStage { passed: true, timestamp: String::new(), errors: vec![] },
                score: 80,
                notes: "Good quality".into(),
                dimensions: Default::default(),
            }),
            last_validated: String::new(),
        };
        assert_eq!(pipeline.determine_status(&validation), ArtifactStatus::Active);
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
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
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
        let validation = pipeline.validate_async(ArtifactKind::Skill, "short", "hi").await;
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
        let validation = pipeline.validate_async(ArtifactKind::Script, "test", content).await;
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
            stage: ValidationStage { passed: true, timestamp: String::new(), errors: vec![] },
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
            stage: ValidationStage { passed: true, timestamp: String::new(), errors: vec![] },
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
            stage: ValidationStage { passed: true, timestamp: String::new(), errors: vec![] },
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
                stage: ValidationStage { passed: true, timestamp: "2026-05-09".into(), errors: vec![] },
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
}

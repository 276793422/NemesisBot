//! Validation pipeline - three-stage artifact validation.
//!
//! Stage 1: Static content validation
//! Stage 2: Functional testing (structure checks)
//! Stage 3: Quality evaluation (placeholder for LLM-as-judge)

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::config::ForgeConfig;
use crate::registry::Registry;
use nemesis_types::forge::{ArtifactKind, ArtifactStatus};

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

/// A 线行为回归结果（可折入 Stage 2 计数与错误面）。
#[derive(Debug, Default)]
struct BehaviorOutcome {
    passed: bool,
    tests_run: u32,
    tests_passed: u32,
    errors: Vec<String>,
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
    /// A 线行为回归执行器（Skill artifact 带 tests 段时调用）。
    /// 未注入 + 带 tests = Stage 2 fail-closed（诚实报错，不静默跳过）。
    skill_test_runner: parking_lot::RwLock<Option<Arc<dyn crate::skill_tests::SkillTestRunner>>>,
    /// Stage 2 judge 层专用 caller（杂务模型回落链注入点，2026-10-14 三轮
    /// 复查对齐三入口口径）：None = 回落 `llm_caller`（主模型，测试注入
    /// 旧形态）。Stage 3 质量评估不受此槽影响（维持主模型语义）。
    judge_caller: parking_lot::RwLock<Option<Arc<dyn crate::reflector_llm::LLMCaller>>>,
}

impl Pipeline {
    /// Create a new validation pipeline.
    pub fn new(config: ForgeConfig, registry: Arc<Registry>) -> Self {
        Self {
            config,
            registry,
            llm_caller: parking_lot::RwLock::new(None),
            skill_test_runner: parking_lot::RwLock::new(None),
            judge_caller: parking_lot::RwLock::new(None),
        }
    }

    /// Set the LLM provider for quality evaluation (Stage 3).
    pub fn set_provider(&self, caller: Arc<dyn crate::reflector_llm::LLMCaller>) {
        *self.llm_caller.write() = Some(caller);
    }

    /// Set the skill behavior-test runner (A 线回归闸，2026-10-14)。
    pub fn set_skill_test_runner(&self, runner: Arc<dyn crate::skill_tests::SkillTestRunner>) {
        *self.skill_test_runner.write() = Some(runner);
    }

    /// Set the Stage 2 judge-layer caller（杂务模型回落链注入点）。未注入 =
    /// judge 层回落 `llm_caller`（主模型）。
    pub fn set_judge_caller(&self, caller: Arc<dyn crate::reflector_llm::LLMCaller>) {
        *self.judge_caller.write() = Some(caller);
    }

    /// Run the full validation pipeline on content (synchronous wrapper).
    ///
    /// This is the primary API used by forge.rs and learning_engine.rs.
    /// Internally uses `validate_async` with `block_on` for LLM calls.
    pub fn validate(&self, kind: ArtifactKind, name: &str, content: &str) -> ArtifactValidation {
        let future = self.validate_async(kind, name, content);
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| handle.block_on(future)),
            Err(_) => {
                let rt = tokio::runtime::Runtime::new()
                    .expect("Failed to create tokio runtime for validation");
                rt.block_on(future)
            }
        }
    }

    /// Async version of validate for use in async contexts.
    pub async fn validate_async(
        &self,
        kind: ArtifactKind,
        name: &str,
        content: &str,
    ) -> ArtifactValidation {
        let mut validation = ArtifactValidation {
            stage1_static: None,
            stage2_functional: None,
            stage3_quality: None,
            last_validated: chrono::Local::now().to_rfc3339(),
        };

        // Stage 1: Static validation
        let stage1 = self.validate_static(kind, name, content);
        validation.stage1_static = Some(stage1.clone());
        if !stage1.stage.passed {
            return validation;
        }

        // Stage 2: Functional validation（含 A 线行为回归闸）
        let mut stage2 = self.validate_functional(kind, content);
        if matches!(kind, ArtifactKind::Skill) {
            match crate::skill_tests::parse_skill_tests(content) {
                Ok(Some(spec)) => {
                    match self.run_behavior_tests(&spec, content).await {
                        Ok(behavior) => {
                            stage2.tests_run += behavior.tests_run;
                            stage2.tests_passed += behavior.tests_passed;
                            if !behavior.passed {
                                stage2.stage.passed = false;
                                stage2.stage.errors.extend(behavior.errors);
                            }
                        }
                        Err(e) => {
                            // fail-closed：runner 缺失或执行器报错 = 闸不通过
                            stage2.stage.passed = false;
                            stage2.stage.errors.push(format!("behavior tests: {e}"));
                        }
                    }
                }
                Ok(None) => {} // 存量无 tests 段：诚实跳过
                Err(e) => {
                    stage2.stage.passed = false;
                    stage2
                        .stage
                        .errors
                        .push(format!("tests section invalid: {e}"));
                }
            }
        }
        validation.stage2_functional = Some(stage2.clone());
        if !stage2.stage.passed {
            return validation;
        }

        // Stage 3: Quality evaluation (LLM-as-Judge or default)
        let stage3 = self.evaluate_quality(kind, name, content).await;
        validation.stage3_quality = Some(stage3);

        validation
    }

    /// A 线行为回归执行（Skill 带 tests 段）。返回可折入 Stage 2 的结果。
    async fn run_behavior_tests(
        &self,
        spec: &crate::skill_tests::SkillTestsSpec,
        content: &str,
    ) -> Result<BehaviorOutcome, String> {
        let runner = self.skill_test_runner.read().clone();
        let Some(runner) = runner else {
            return Err(
                "behavior tests declared but no test runner configured (fail-closed)".into(),
            );
        };
        // judge 层 caller：专用槽（杂务回落链）优先，回落主模型槽。
        let caller = match self.judge_caller.read().clone() {
            Some(c) => Some(c),
            None => self.llm_caller.read().clone(),
        };

        let mut outcome = BehaviorOutcome::default();
        let default_threshold = self.config.test.judge_threshold;
        for case in &spec.tests {
            outcome.tests_run += 1;
            let transcript = runner.run_case(content, case).await?;
            let result = crate::skill_tests::evaluate_case_full(
                case,
                &transcript,
                caller.clone(),
                default_threshold,
            )
            .await;

            if result.passed {
                outcome.tests_passed += 1;
            } else {
                outcome.errors.push(format!(
                    "case '{}': {}",
                    case.name,
                    result.failures.join("; ")
                ));
            }
        }
        outcome.passed = outcome.errors.is_empty();
        Ok(outcome)
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
        if let Some(ref s1) = validation.stage1_static
            && !s1.stage.passed
        {
            return ArtifactStatus::Draft;
        }

        // If stage 2 failed, keep as draft
        if let Some(ref s2) = validation.stage2_functional
            && !s2.stage.passed
        {
            return ArtifactStatus::Draft;
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

    fn validate_static(
        &self,
        kind: ArtifactKind,
        _name: &str,
        content: &str,
    ) -> StaticValidationResult {
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

        // 危险命令检查（F-S1 同类扫，2026-10-16 R16）：本函数是 learning_engine
        // refine 循环的唯一静态闸，此前只有密钥子串检查——含 `rm -rf /` /
        // `curl|bash` 的 LLM 产物能通过校验并部署为 agent 指令。规则与
        // StaticValidator 同源（dangerous_command_errors 单一真相源）；密钥
        // 正则面不重复接入（上方子串检查更宽，两套都报会重复计错）。
        for e in crate::validator::StaticValidator::new().dangerous_command_errors(content) {
            errors.push(e);
        }

        StaticValidationResult {
            stage: ValidationStage {
                passed: errors.is_empty(),
                timestamp: chrono::Local::now().to_rfc3339(),
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
                timestamp: chrono::Local::now().to_rfc3339(),
                errors,
            },
            tests_run,
            tests_passed,
        }
    }

    /// Stage 3: Quality evaluation using LLM-as-Judge.
    ///
    /// When an LLM provider is configured, calls the LLM to evaluate the
    /// artifact on four dimensions (correctness, quality, security, reusability)
    /// and computes a weighted score. Without a provider, returns a default score.
    async fn evaluate_quality(
        &self,
        kind: ArtifactKind,
        name: &str,
        content: &str,
    ) -> QualityValidationResult {
        // Clone the Arc inside the lock and drop the guard before awaiting —
        // the parking_lot read guard must not be held across the LLM call
        // (blocks set_llm_caller writers for the whole network round-trip).
        let caller = self.llm_caller.read().clone();

        match caller {
            Some(caller) => {
                // LLM-as-Judge evaluation (matches Go evaluator.go)
                let kind_str = match kind {
                    ArtifactKind::Skill => "skill",
                    ArtifactKind::Script => "script",
                    ArtifactKind::Mcp => "mcp",
                };

                // 评审提示走 forge 内单一真相源（与 evaluator/validator 共用）。
                let system_prompt = crate::reflector_llm::QUALITY_REVIEWER_SYSTEM_PROMPT;

                let user_prompt =
                    crate::reflector_llm::quality_review_prompt(kind_str, name, None, content);

                // 预算统一走 forge 侧常量（2026-09-28 从 2000 拉齐 8192：
                // 单一真相源，对非 anthropic lane thinking 不受控留余量）。
                match caller
                    .chat(
                        system_prompt,
                        &user_prompt,
                        Some(crate::reflector_llm::AUX_FORGE_MAX_TOKENS),
                    )
                    .await
                {
                    Ok(response) => {
                        // Parse LLM response
                        match crate::reflector_llm::extract_json(&response) {
                            Some(parsed) => {
                                let mut dimensions = std::collections::HashMap::new();

                                // Extract dimension scores（钳制 0-100，单一真
                                // 相源 parse_dimension_score；此前 as_u64 直取，
                                // LLM 回 1000 分时加权 400 恒过 + u32 溢出面）。
                                for key in &["correctness", "quality", "security", "reusability"] {
                                    if let Some(val) = parsed.get(*key)
                                        && let Some(n) =
                                            crate::reflector_llm::parse_dimension_score(val)
                                    {
                                        dimensions.insert(key.to_string(), n);
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
                                        timestamp: chrono::Local::now().to_rfc3339(),
                                        errors: if score < min_score {
                                            vec![format!(
                                                "Quality score {} below threshold {}",
                                                score, min_score
                                            )]
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
                                        timestamp: chrono::Local::now().to_rfc3339(),
                                        errors: vec![
                                            "Failed to parse LLM response as JSON".to_string(),
                                        ],
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
                                timestamp: chrono::Local::now().to_rfc3339(),
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
                        timestamp: chrono::Local::now().to_rfc3339(),
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
mod tests;

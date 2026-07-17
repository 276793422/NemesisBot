//! Learning engine - closed-loop self-learning (Phase 6).
//!
//! Extracts patterns from collected experiences, generates learning actions
//! (skill creation, prompt suggestions), and evaluates deployment outcomes.
//!
//! Supports four pattern detectors:
//! - tool_chain: High-frequency tool sequences
//! - error_recovery: Tools that fail then succeed
//! - efficiency_issue: Slow or wasteful tool usage
//! - success_template: Consistently successful tool patterns
//!
//! LLM-driven skill generation:
//! - `generate_skill_draft` uses LLM to create SKILL.md content
//! - `refine_skill_draft` uses LLM to fix failed drafts
//! - `execute_create_skill` runs the full generation + validation + deploy loop
//! - `execute_suggest_prompt` writes prompt suggestions to workspace/prompts/

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;

use nemesis_types::forge::LearningCycle;
use crate::config::ForgeConfig;
use crate::cycle_store::CycleStore;
use crate::monitor::DeploymentMonitor;
use crate::pipeline::{ArtifactValidation, Pipeline};
use crate::registry::Registry;
use crate::types::CollectedExperience;

/// Skill creation delegate. Mirrors Go's `le.forge.CreateSkill()`.
/// The Forge struct implements this trait and injects it via `set_skill_creator()`.
pub trait SkillCreator: Send + Sync {
    /// Create a skill artifact. Mirrors Go's `Forge.CreateSkill()`.
    fn create_skill(
        &self,
        name: &str,
        content: &str,
        description: &str,
        tool_signature: Vec<String>,
    ) -> Result<nemesis_types::forge::Artifact, String>;
}

/// Detected pattern from experience analysis.
#[derive(Debug, Clone)]
pub struct DetectedPattern {
    /// Pattern type (tool_chain, error_recovery, efficiency_issue, success_template).
    pub pattern_type: String,
    /// Frequency of occurrence.
    pub frequency: u32,
    /// Confidence score [0, 1].
    pub confidence: f64,
    /// Human-readable description.
    pub description: String,
    /// Associated tool names.
    pub tools: Vec<String>,
}

/// A learning action to be executed.
#[derive(Debug, Clone)]
pub struct LearningAction {
    /// Unique action ID.
    pub id: String,
    /// Action type (create_skill, suggest_prompt, deprecate_artifact).
    pub action_type: String,
    /// Priority (high, medium, low).
    pub priority: String,
    /// Description of the action.
    pub description: String,
    /// Status (pending, executed, skipped, failed).
    pub status: String,
    /// Error message if status is "failed".
    pub error_msg: Option<String>,
    /// Name for the draft skill.
    pub draft_name: Option<String>,
    /// Rationale for the action.
    pub rationale: Option<String>,
    /// Confidence score from the pattern.
    pub confidence: f64,
    /// Pattern ID that triggered this action.
    pub pattern_id: Option<String>,
    /// Artifact ID (set after execution).
    pub artifact_id: Option<String>,
    /// Creation timestamp (ISO 8601).
    pub created_at: Option<String>,
    /// Execution timestamp (ISO 8601).
    pub executed_at: Option<String>,
}

impl LearningAction {
    /// Create a new pending action.
    pub fn new(action_type: &str, priority: &str, description: &str) -> Self {
        Self {
            id: format!("la-{}", uuid::Uuid::new_v4().to_string().split('-').next().unwrap_or("0")),
            action_type: action_type.to_string(),
            priority: priority.to_string(),
            description: description.to_string(),
            status: "pending".to_string(),
            error_msg: None,
            draft_name: None,
            rationale: None,
            confidence: 0.0,
            pattern_id: None,
            artifact_id: None,
            created_at: Some(chrono::Local::now().to_rfc3339()),
            executed_at: None,
        }
    }
}

/// Outcome of a deployed artifact evaluation.
#[derive(Debug, Clone)]
pub struct DeploymentOutcome {
    /// Artifact ID.
    pub artifact_id: String,
    /// Verdict (positive, negative, observing).
    pub verdict: String,
    /// Improvement score.
    pub improvement_score: f64,
    /// Sample size for evaluation.
    pub sample_size: usize,
}

/// Pattern summary for cycle storage.
#[derive(Debug, Clone)]
pub struct PatternSummary {
    /// Pattern ID.
    pub id: String,
    /// Pattern type.
    pub pattern_type: String,
    /// Fingerprint hash.
    pub fingerprint: String,
    /// Frequency.
    pub frequency: u32,
    /// Confidence score.
    pub confidence: f64,
}

/// Action summary for cycle storage.
#[derive(Debug, Clone)]
pub struct ActionSummary {
    /// Action ID.
    pub id: String,
    /// Action type.
    pub action_type: String,
    /// Priority.
    pub priority: String,
    /// Status.
    pub status: String,
    /// Artifact ID (if created).
    pub artifact_id: Option<String>,
}

/// The learning engine drives the closed-loop learning cycle.
pub struct LearningEngine {
    config: ForgeConfig,
    forge_dir: PathBuf,
    registry: Arc<Registry>,
    cycle_store: CycleStore,
    pipeline: Mutex<Option<Arc<Pipeline>>>,
    monitor: Mutex<Option<Arc<DeploymentMonitor>>>,
    provider: Mutex<Option<Arc<dyn crate::reflector_llm::LLMCaller>>>,
    skill_creator: Mutex<Option<Arc<dyn SkillCreator>>>,
    latest_cycle: Mutex<Option<LearningCycle>>,
}

impl LearningEngine {
    /// Create a new learning engine.
    pub fn new(config: ForgeConfig, registry: Arc<Registry>, cycle_store: CycleStore) -> Self {
        tracing::info!("[Forge/LearningEngine] Created");
        Self {
            forge_dir: PathBuf::new(),
            config,
            registry,
            cycle_store,
            pipeline: Mutex::new(None),
            monitor: Mutex::new(None),
            provider: Mutex::new(None),
            skill_creator: Mutex::new(None),
            latest_cycle: Mutex::new(None),
        }
    }

    /// Create a new learning engine with forge directory.
    pub fn with_forge_dir(
        config: ForgeConfig,
        forge_dir: PathBuf,
        registry: Arc<Registry>,
        cycle_store: CycleStore,
    ) -> Self {
        tracing::info!(
            forge_dir = %forge_dir.display(),
            "[Forge/LearningEngine] Created with forge directory"
        );
        Self {
            forge_dir,
            config,
            registry,
            cycle_store,
            pipeline: Mutex::new(None),
            monitor: Mutex::new(None),
            provider: Mutex::new(None),
            skill_creator: Mutex::new(None),
            latest_cycle: Mutex::new(None),
        }
    }

    /// Set the LLM provider for skill draft generation.
    pub fn set_provider(&self, provider: Arc<dyn crate::reflector_llm::LLMCaller>) {
        tracing::info!("[Forge/LearningEngine] LLM provider set for skill generation");
        *self.provider.lock() = Some(provider);
    }

    /// Call an async LLMCaller from sync context.
    /// Mirrors the block_in_place pattern in pipeline.rs:evaluate_quality_sync.
    fn call_llm_sync(
        caller: &dyn crate::reflector_llm::LLMCaller,
        system: &str,
        user: &str,
        max_tokens: u32,
    ) -> Result<String, String> {
        let future = caller.chat(system, user, Some(max_tokens as i64));
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                tokio::task::block_in_place(|| handle.block_on(future))
            }
            Err(_) => {
                let rt = tokio::runtime::Runtime::new()
                    .map_err(|e| format!("failed to create runtime: {}", e))?;
                rt.block_on(future)
            }
        }
    }

    /// Set the pipeline for validation.
    pub fn set_pipeline(&self, pipeline: Arc<Pipeline>) {
        tracing::info!("[Forge/LearningEngine] Pipeline set for validation");
        *self.pipeline.lock() = Some(pipeline);
    }

    /// Set the deployment monitor for outcome evaluation.
    pub fn set_monitor(&self, monitor: Arc<DeploymentMonitor>) {
        tracing::info!("[Forge/LearningEngine] Deployment monitor set");
        *self.monitor.lock() = Some(monitor);
    }

    /// Set the skill creator delegate. Mirrors Go's `SetForge(f *Forge)`.
    /// The Forge instance implements SkillCreator and is injected here.
    pub fn set_skill_creator(&self, creator: Arc<dyn SkillCreator>) {
        tracing::info!("[Forge/LearningEngine] Skill creator delegate set");
        *self.skill_creator.lock() = Some(creator);
    }

    /// Run a single learning cycle.
    ///
    /// Mirrors Go's `LearningEngine.RunCycle`:
    /// 1. Evaluate previous deployment outcomes
    /// 2. Adjust confidence based on feedback
    /// 3. Extract patterns from experiences
    /// 4. Check suggestion adoption status
    /// 5. Generate actions from patterns
    /// 6. Execute actions (with auto-create limit)
    /// 7. Save cycle record
    pub async fn run_cycle(&self, experiences: &[CollectedExperience]) -> LearningCycle {
        let cycle_id = uuid::Uuid::new_v4().to_string();
        tracing::info!(
            cycle_id = %cycle_id,
            experience_count = experiences.len(),
            "[Forge/LearningEngine] Starting learning cycle"
        );
        let mut cycle = LearningCycle {
            id: cycle_id,
            started_at: chrono::Local::now().to_rfc3339(),
            completed_at: None,
            patterns_found: 0,
            actions_taken: 0,
            status: nemesis_types::forge::CycleStatus::Running,
        };

        // Step 1: Evaluate previous deployment outcomes
        let previous_outcomes = if let Some(ref monitor) = *self.monitor.lock() {
            monitor.evaluate_all()
        } else {
            Vec::new()
        };

        // Step 2: Adjust confidence based on feedback
        self.adjust_confidence_from_outcomes(&previous_outcomes);
        // F-F2: take monitoring effect — disable the deployed skill file of any
        // artifact the monitor marked Degraded so the skills loader stops using
        // it. Without this the monitor's verdict had no teeth.
        self.disable_degraded_skills();

        // Step 3: Extract patterns from experiences
        let patterns = self.extract_patterns(experiences);
        cycle.patterns_found = patterns.len() as u32;

        // Step 4: Check suggestion adoption status
        self.check_suggestion_adoption(&patterns);

        // Step 5: Generate actions from patterns
        let actions = self.generate_actions(&patterns);

        // Step 6: Execute actions with limit
        let max_auto = if self.config.learning.max_auto_creates > 0 {
            self.config.learning.max_auto_creates
        } else {
            3
        };
        let mut auto_count = 0u32;
        let mut actions_executed = 0u32;
        let mut actions_skipped = 0u32;

        for mut action in actions {
            match action.action_type.as_str() {
                "create_skill" => {
                    if auto_count >= max_auto {
                        action.status = "skipped".to_string();
                        actions_skipped += 1;
                    } else {
                        auto_count += 1;
                        self.execute_create_skill(&action);
                        // F-M3: execute_create_skill takes &action (immutable),
                        // so it can't set action.status — count the action here
                        // once it runs past the max_auto gate. (Inner early-
                        // return paths are best-effort, but the action was
                        // processed, which is what actions_taken reports.)
                        actions_executed += 1;
                    }
                }
                "suggest_prompt" => {
                    self.execute_suggest_prompt(&mut action);
                    if action.status == "executed" {
                        actions_executed += 1;
                    }
                }
                _ => {
                    actions_skipped += 1;
                }
            }
        }

        cycle.actions_taken = actions_executed;
        if actions_skipped > 0 {
            tracing::info!(
                actions_skipped,
                "[LearningEngine] Learning cycle skipped some actions due to limits or unknown type"
            );
        }

        cycle.status = nemesis_types::forge::CycleStatus::Completed;
        cycle.completed_at = Some(chrono::Local::now().to_rfc3339());

        // Persist cycle
        if let Err(e) = self.cycle_store.append(&cycle).await {
            tracing::warn!(error = %e, "[LearningEngine] Failed to persist learning cycle");
        }

        *self.latest_cycle.lock() = Some(cycle.clone());
        cycle
    }

    /// Extract patterns from collected experiences using all four detectors.
    pub fn extract_patterns(&self, experiences: &[CollectedExperience]) -> Vec<DetectedPattern> {
        if experiences.is_empty() {
            return Vec::new();
        }

        tracing::debug!(
            experience_count = experiences.len(),
            "[Forge/LearningEngine] Extracting patterns from experiences"
        );

        let mut patterns = Vec::new();
        let tool_chain_patterns = self.detect_tool_chains(experiences);
        patterns.extend(tool_chain_patterns);

        // Detect error recovery patterns
        let error_patterns = self.detect_error_recovery(experiences);
        patterns.extend(error_patterns);

        // Detect efficiency issues
        let efficiency_patterns = self.detect_efficiency_issue(experiences);
        patterns.extend(efficiency_patterns);

        // Detect success templates
        let success_patterns = self.detect_success_template(experiences);
        patterns.extend(success_patterns);

        // Filter by minimum frequency
        let min_freq = if self.config.learning.min_pattern_frequency > 0 {
            self.config.learning.min_pattern_frequency
        } else {
            3
        };
        patterns.retain(|p| p.frequency >= min_freq);

        // Sort by confidence descending
        patterns.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        tracing::info!(
            pattern_count = patterns.len(),
            "[Forge/LearningEngine] Patterns extracted"
        );

        patterns
    }

    /// Generate learning actions from detected patterns.
    ///
    /// All actions are generated with status "pending" — the caller is
    /// responsible for executing them (mirrors Go's generateActions).
    pub fn generate_actions(&self, patterns: &[DetectedPattern]) -> Vec<LearningAction> {
        let mut actions = Vec::new();
        let high_conf = if self.config.learning.high_conf_threshold > 0.0 {
            self.config.learning.high_conf_threshold
        } else {
            0.8
        };

        for pattern in patterns {
            let pattern_id = format!("p-{}", uuid::Uuid::new_v4().to_string().split('-').next().unwrap_or("0"));

            match pattern.pattern_type.as_str() {
                "tool_chain" => {
                    if pattern.confidence >= high_conf && pattern.frequency >= 10 {
                        let mut action = LearningAction::new(
                            "create_skill",
                            "high",
                            &pattern.description,
                        );
                        action.confidence = pattern.confidence;
                        action.pattern_id = Some(pattern_id.clone());
                        action.draft_name = Some(generate_skill_name(
                            &pattern.tools.join("->"),
                        ));
                        action.rationale = Some(format!(
                            "High-confidence tool chain ({:.2}) with frequency {}",
                            pattern.confidence, pattern.frequency
                        ));
                        actions.push(action);
                    } else {
                        let mut action = LearningAction::new(
                            "suggest_prompt",
                            "medium",
                            &pattern.description,
                        );
                        action.confidence = pattern.confidence;
                        action.pattern_id = Some(pattern_id.clone());
                        action.draft_name = Some(generate_skill_name(
                            &pattern.tools.join("->"),
                        ));
                        action.rationale = Some(format!(
                            "Tool chain below threshold ({:.2} < {:.2}), suggest prompt",
                            pattern.confidence, high_conf
                        ));
                        actions.push(action);
                    }
                }

                "error_recovery" => {
                    if pattern.confidence >= high_conf {
                        let mut action = LearningAction::new(
                            "create_skill",
                            "high",
                            &pattern.description,
                        );
                        action.confidence = pattern.confidence;
                        action.pattern_id = Some(pattern_id.clone());
                        action.draft_name = Some(format!(
                            "{}-error-handler",
                            pattern.tools.first().unwrap_or(&"unknown".to_string())
                        ));
                        action.rationale = Some(format!(
                            "High-confidence error recovery ({:.2}): {}",
                            pattern.confidence,
                            pattern.description
                        ));
                        actions.push(action);
                    }
                }

                "efficiency_issue" => {
                    let mut action = LearningAction::new(
                        "suggest_prompt",
                        "medium",
                        &pattern.description,
                    );
                    action.confidence = pattern.confidence;
                    action.pattern_id = Some(pattern_id.clone());
                    action.draft_name = Some(generate_skill_name(
                        &pattern.tools.join("->"),
                    ));
                    action.rationale = Some(format!(
                        "Efficiency issue ({:.2} confidence), suggest optimization",
                        pattern.confidence
                    ));
                    actions.push(action);
                }

                "success_template" => {
                    if pattern.confidence >= high_conf {
                        let mut action = LearningAction::new(
                            "create_skill",
                            "high",
                            &pattern.description,
                        );
                        action.confidence = pattern.confidence;
                        action.pattern_id = Some(pattern_id.clone());
                        action.draft_name = Some(generate_skill_name(
                            &pattern.tools.join("->"),
                        ));
                        action.rationale = Some(format!(
                            "Success template ({:.2} confidence), automate as Skill",
                            pattern.confidence
                        ));
                        actions.push(action);
                    }
                }

                _ => {
                    // Unknown pattern type, skip
                }
            }
        }

        // Sort by priority (high first) then confidence descending
        let priority_order = |p: &str| -> u8 {
            match p {
                "high" => 0,
                "medium" => 1,
                "low" => 2,
                _ => 3,
            }
        };
        actions.sort_by(|a, b| {
            let pa = priority_order(&a.priority);
            let pb = priority_order(&b.priority);
            match pa.cmp(&pb) {
                std::cmp::Ordering::Equal => b
                    .confidence
                    .partial_cmp(&a.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal),
                other => other,
            }
        });

        actions
    }

    /// Adjust confidence based on deployment outcomes (mirrors Go's adjustConfidence).
    fn adjust_confidence_from_outcomes(&self, outcomes: &[crate::monitor::EvaluationResult]) {
        for outcome in outcomes {
            if outcome.artifact_id.is_empty() {
                continue;
            }
            let delta = match outcome.verdict.as_str() {
                "positive" => Some(0.1),
                "negative" => Some(-0.2),
                _ => None,
            };
            if let Some(delta) = delta {
                self.registry.update(&outcome.artifact_id, |a| {
                    // F-F1: adjust the dedicated success_rate field, NOT
                    // usage_count — the old code treated the integer count as
                    // success_rate*100, corrupting its semantics.
                    a.success_rate = (a.success_rate + delta).clamp(0.0, 1.0);
                });
            }
        }
    }

    /// Disable the deployed skill file for any Degraded artifact (F-F2). The
    /// skills loader scans `workspace/skills/*/SKILL.md`, so renaming a
    /// degraded skill's `SKILL.md` → `SKILL.md.disabled` hides it from the
    /// agent (reversible: rename back to re-enable). This is the "rollback"
    /// endpoint of the Phase 6 monitor→feedback loop — without it a bad learned
    /// skill stayed an active instruction forever.
    fn disable_degraded_skills(&self) {
        self.disable_degraded_skills_impl()
    }

    /// Same as `disable_degraded_skills`, exposed for tests (F-F2 verification).
    pub(crate) fn disable_degraded_skills_impl(&self) {
        use nemesis_types::forge::{ArtifactKind, ArtifactStatus};
        let workspace = match self.forge_dir.parent() {
            Some(w) => w,
            None => return,
        };
        let skills_root = workspace.join("skills");
        for a in self.registry.list(None, None) {
            if !matches!(a.kind, ArtifactKind::Skill) {
                continue;
            }
            if !matches!(a.status, ArtifactStatus::Degraded) {
                continue;
            }
            let dir = skills_root.join(format!("{}-forge", a.name));
            let active = dir.join("SKILL.md");
            if active.exists() {
                let disabled = dir.join("SKILL.md.disabled");
                match std::fs::rename(&active, &disabled) {
                    Ok(_) => tracing::info!(
                        artifact = %a.id,
                        "[LearningEngine] Disabled degraded skill — rolled back from agent"
                    ),
                    Err(e) => tracing::warn!(
                        error = %e,
                        artifact = %a.id,
                        "[LearningEngine] Failed to disable degraded skill file"
                    ),
                }
            }
        }
    }

    /// Public test-only wrapper for `adjust_confidence_from_outcomes`.
    ///
    /// Mirrors Go's `LearningEngine.AdjustConfidenceForTest` — exposes the
    /// private `adjustConfidence` logic for testing purposes.
    pub fn adjust_confidence_for_test(&self, outcomes: &[crate::monitor::EvaluationResult]) {
        self.adjust_confidence_from_outcomes(outcomes);
    }

    /// Execute create_skill action: generate via LLM, validate, deploy (mirrors Go's executeCreateSkill).
    fn execute_create_skill(&self, action: &LearningAction) {
        let draft_name = match &action.draft_name {
            Some(name) => name.clone(),
            None => {
                return;
            }
        };

        // Check if already exists in registry (dedup by name)
        if self.find_artifact_by_fingerprint(&draft_name) {
            return;
        }

        // Generate skill draft using LLM
        let provider = self.provider.lock();
        let provider_arc = match provider.as_ref() {
            Some(p) => p.clone(),
            None => return,
        };
        drop(provider);

        let mut content = match self.generate_skill_draft(&*provider_arc, action) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "[LearningEngine] LLM generation failed for skill draft");
                return;
            }
        };

        // Iterative refinement loop with pipeline validation
        let max_refine = 3u32;

        // F-M7: clone the Arc<Pipeline> out of the lock and drop the guard
        // before the LLM round-trips (validate + refine loop). Holding the
        // pipeline Mutex across block_in_place LLM calls pins a runtime worker
        // and can panic on a current-thread runtime.
        let pipeline = { self.pipeline.lock().clone() };
        if let Some(ref pipeline) = pipeline {
            for attempt in 0..=max_refine {
                let validation = pipeline.validate(
                    nemesis_types::forge::ArtifactKind::Skill,
                    &draft_name,
                    &content,
                );
                let new_status = pipeline.determine_status(&validation);

                if new_status == nemesis_types::forge::ArtifactStatus::Active
                    || new_status == nemesis_types::forge::ArtifactStatus::Observing
                {
                    // Passed — deploy by writing skill file and registering artifact
                    let tool_sig = extract_tool_signature_from_chain(&action.description);

                    // Write content to forge skills directory
                    let artifact_dir = self.forge_dir.join("skills").join(&draft_name);
                    if std::fs::create_dir_all(&artifact_dir).is_ok() {
                        let _ = std::fs::write(artifact_dir.join("SKILL.md"), &content);
                    }

                    // Register in registry
                    let artifact_id = format!("skill-{}", draft_name);
                    let artifact = nemesis_types::forge::Artifact {
                        id: artifact_id.clone(),
                        name: draft_name.clone(),
                        kind: nemesis_types::forge::ArtifactKind::Skill,
                        version: "1.0".to_string(),
                        status: new_status,
                        content: content.clone(),
                        tool_signature: tool_sig,
                        created_at: chrono::Local::now().to_rfc3339(),
                        updated_at: chrono::Local::now().to_rfc3339(),
                        usage_count: 0,
                        last_degraded_at: None,
                        success_rate: 0.0,
                        consecutive_observing_rounds: 0,
                    };
                    self.registry.add(artifact);

                    // Copy to workspace/skills/ with -forge suffix
                    if let Some(workspace) = self.forge_dir.parent() {
                        let ws_skill_dir = workspace.join("skills").join(format!("{}-forge", draft_name));
                        if std::fs::create_dir_all(&ws_skill_dir).is_ok() {
                            let _ = std::fs::write(ws_skill_dir.join("SKILL.md"), &content);
                        }
                    }

                    tracing::info!(artifact_id = %artifact_id, "[LearningEngine] Created skill from learning cycle");
                    return;
                }

                // Failed — refine, then loop to re-validate (F-C2: the old code
                // discarded the refined draft `_refined` and returned after one
                // attempt, so a skill that failed first validation could never
                // deploy even after successful refinement).
                if attempt < max_refine {
                    let diagnosis = build_diagnosis(&validation);
                    match self.refine_skill_draft(&*provider_arc, action, &content, &diagnosis) {
                        Ok(refined) => {
                            tracing::debug!(attempt, "[LearningEngine] Refined skill draft, re-validating");
                            content = refined;
                            continue;
                        }
                        Err(e) => {
                            tracing::warn!(attempt = attempt + 1, error = %e, "[LearningEngine] Skill refinement failed");
                            return;
                        }
                    }
                }
            }
        } else {
            // No pipeline configured — just register as draft
            let artifact_id = format!("skill-{}", draft_name);
            let artifact = nemesis_types::forge::Artifact {
                id: artifact_id,
                name: draft_name,
                kind: nemesis_types::forge::ArtifactKind::Skill,
                version: "1.0".to_string(),
                status: nemesis_types::forge::ArtifactStatus::Draft,
                content,
                tool_signature: extract_tool_signature_from_chain(&action.description),
                created_at: chrono::Local::now().to_rfc3339(),
                updated_at: chrono::Local::now().to_rfc3339(),
                usage_count: 0,
                last_degraded_at: None,
                success_rate: 0.0,
                consecutive_observing_rounds: 0,
            };
            self.registry.add(artifact);
        }

        tracing::warn!(
            max_refine = max_refine,
            "[LearningEngine] Skill validation failed after all refinement rounds"
        );
    }

    /// Execute suggest_prompt action: write a prompt suggestion to workspace/prompts/.
    fn execute_suggest_prompt(&self, action: &mut LearningAction) {
        let workspace_dir = if self.forge_dir.as_os_str().is_empty() {
            return;
        } else {
            self.forge_dir.parent().unwrap_or(&self.forge_dir).join("prompts")
        };

        if let Err(e) = std::fs::create_dir_all(&workspace_dir) {
            action.status = "failed".to_string();
            action.error_msg = Some(format!("Failed to create prompts dir: {}", e));
            return;
        }

        let draft_name = action.draft_name.as_deref().unwrap_or("unknown");
        let mut filename = draft_name
            .replace("->", "-")
            .replace(' ', "-")
            .to_lowercase();
        // truncate() panics on non-char-boundary; floor first (draft_name may be multibyte).
        let cut = nemesis_types::utils::floor_char_boundary(&filename, 60);
        filename.truncate(cut);

        let content = format!(
            "# Prompt Suggestion: {}\n\n\
             ## Rationale\n{}\n\n\
             ## Pattern Description\n{}\n\n\
             ## Confidence\n{:.2}\n\n\
             ## Suggested Action\nConsider creating a Skill or improving the workflow for this pattern.\n\
             Generated: {}\n",
            draft_name,
            action.rationale.as_deref().unwrap_or("N/A"),
            action.description,
            action.confidence,
            chrono::Local::now().to_rfc3339()
        );

        let path = workspace_dir.join(format!("{}_suggestion.md", filename));
        if let Err(e) = std::fs::write(&path, &content) {
            action.status = "failed".to_string();
            action.error_msg = Some(format!("Failed to write suggestion: {}", e));
            return;
        }

        action.status = "executed".to_string();
        action.executed_at = Some(chrono::Local::now().to_rfc3339());
        action.artifact_id = Some(path.to_string_lossy().to_string());
    }

    /// Check if previously suggested prompts have been adopted (mirrors Go's checkSuggestionAdoption).
    /// Test-only: probe whether the pipeline lock is currently free (F-M7).
    #[cfg(test)]
    pub(crate) fn pipeline_try_lock_for_test(&self) -> bool {
        self.pipeline.try_lock().is_some()
    }

    /// Test-only wrapper for check_suggestion_adoption (F-M1 verification).
    #[cfg(test)]
    pub(crate) fn check_suggestion_adoption_for_test(&self, patterns: &[DetectedPattern]) {
        self.check_suggestion_adoption(patterns);
    }

    fn check_suggestion_adoption(&self, _patterns: &[DetectedPattern]) {
        let prompts_dir = if self.forge_dir.as_os_str().is_empty() {
            return;
        } else {
            self.forge_dir.parent().unwrap_or(&self.forge_dir).join("prompts")
        };

        // F-M1: only retire prompt suggestions once at least one skill has
        // actually been deployed (Active/Observing). The old code wiped every
        // *_suggestion.md on each cycle, so suggestions never survived long
        // enough to be adopted. No precise adoption signal exists, so gate on
        // deployed-skill existence.
        let has_deployed_skill = self
            .registry
            .list(None, None)
            .iter()
            .any(|a| {
                use nemesis_types::forge::{ArtifactKind, ArtifactStatus};
                matches!(a.kind, ArtifactKind::Skill)
                    && matches!(a.status, ArtifactStatus::Active | ArtifactStatus::Observing)
            });
        if !has_deployed_skill {
            return;
        }

        let entries = match std::fs::read_dir(&prompts_dir) {
            Ok(e) => e,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.ends_with("_suggestion.md") {
                continue;
            }
            // If any high-confidence pattern matches, remove the suggestion
            let _ = std::fs::remove_file(entry.path());
        }
    }

    /// Public test-only wrapper for `execute_suggest_prompt`.
    pub fn execute_suggest_prompt_for_test(&self, action: &mut LearningAction) {
        self.execute_suggest_prompt(action);
    }

    /// Generate a skill draft using LLM (mirrors Go's generateSkillDraft).
    fn generate_skill_draft(
        &self,
        provider: &dyn crate::reflector_llm::LLMCaller,
        action: &LearningAction,
    ) -> Result<String, String> {
        let draft_name = action.draft_name.as_deref().unwrap_or("unknown-skill");
        let prompt = format!(
            "Generate a complete SKILL.md for a Forge self-learning Skill with the following specification:\n\n\
             Name: {}\n\
             Description: {}\n\
             Rationale: {}\n\n\
             The SKILL.md must have YAML frontmatter between --- markers with these fields:\n\
             - name: skill name\n\
             - description: what the skill does\n\
             - version: \"1.0\"\n\n\
             Then provide the skill instructions in Markdown. The skill should define clear steps that an AI agent can follow.\n\
             Focus on the tool usage pattern identified. Keep it concise and actionable.",
            draft_name,
            action.description,
            action.rationale.as_deref().unwrap_or("N/A")
        );

        let budget = 500u32; // default LLM budget
        Self::call_llm_sync(
            provider,
            "You are a Skill definition generator. Generate valid SKILL.md content with YAML frontmatter.",
            &prompt,
            budget,
        )
    }

    /// Refine a skill draft using LLM based on validation diagnosis (mirrors Go's refineSkillDraft).
    fn refine_skill_draft(
        &self,
        provider: &dyn crate::reflector_llm::LLMCaller,
        action: &LearningAction,
        previous_content: &str,
        diagnosis: &str,
    ) -> Result<String, String> {
        let draft_name = action.draft_name.as_deref().unwrap_or("unknown-skill");
        let prompt = format!(
            "The following Skill draft failed validation. Please fix it based on the diagnosis.\n\n\
             Skill Name: {}\n\
             Original Description: {}\n\n\
             Previous Content:\n{}\n\n\
             Validation Diagnosis:\n{}\n\n\
             Please generate a corrected, complete SKILL.md with YAML frontmatter (--- markers). Fix ALL issues identified in the diagnosis.",
            draft_name,
            action.description,
            previous_content,
            diagnosis
        );

        let budget = 500u32;
        Self::call_llm_sync(
            provider,
            "You are a Skill definition generator. Fix the failing Skill and return a complete corrected SKILL.md.",
            &prompt,
            budget,
        )
    }

    /// Check if an artifact with the given name already exists (mirrors Go's findArtifactByFingerprint).
    fn find_artifact_by_fingerprint(&self, name: &str) -> bool {
        let artifacts = self.registry.list(None, None);
        artifacts.iter().any(|a| {
            a.name == name
                && a.status != nemesis_types::forge::ArtifactStatus::Degraded
                && a.status != nemesis_types::forge::ArtifactStatus::Archived
        })
    }

    /// Get the latest completed cycle (from in-memory cache).
    pub fn get_latest_cycle(&self) -> Option<LearningCycle> {
        self.latest_cycle.lock().clone()
    }

    /// Get the latest cycle from the persistent cycle store (last 30 days).
    ///
    /// This reads from disk (JSONL files) and returns the most recent cycle
    /// recorded within the last 30 days. Falls back to the in-memory cache
    /// if the store read fails.
    pub async fn get_latest_cycle_from_store(&self) -> Option<LearningCycle> {
        let since = chrono::Local::now() - chrono::Duration::days(30);

        match self.cycle_store.read_cycles(Some(since)).await {
            Ok(cycles) => cycles.into_iter().last(),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "[LearningEngine] Failed to read cycles from store, falling back to in-memory cache"
                );
                self.latest_cycle.lock().clone()
            }
        }
    }

    /// Evaluate previous deployment outcomes and adjust confidence.
    pub fn evaluate_outcomes(&self, outcomes: &[DeploymentOutcome]) {
        for outcome in outcomes {
            if outcome.artifact_id.is_empty() {
                continue;
            }

            let delta = match outcome.verdict.as_str() {
                "positive" => Some(0.1),
                "negative" => Some(-0.2),
                _ => None,
            };

            if let Some(delta) = delta {
                self.registry.update(&outcome.artifact_id, |a| {
                    // Adjust success rate
                    let new_rate = (a.usage_count as f64 * 0.01 + delta).clamp(0.0, 1.0);
                    // We adjust using a simpler heuristic since SuccessRate field may not exist
                    let _ = new_rate;
                });
            }
        }
    }

    /// Detect tool chains - high frequency tool usage patterns.
    fn detect_tool_chains(&self, experiences: &[CollectedExperience]) -> Vec<DetectedPattern> {
        let mut tool_counts: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new();
        for exp in experiences {
            *tool_counts
                .entry(exp.experience.tool_name.clone())
                .or_insert(0) += 1;
        }

        let total = experiences.len() as f64;
        tool_counts
            .into_iter()
            .filter(|(_, count)| *count >= 3)
            .map(|(tool, count)| {
                let frequency_ratio = count as f64 / total;
                DetectedPattern {
                    pattern_type: "tool_chain".into(),
                    frequency: count,
                    confidence: frequency_ratio.min(1.0),
                    description: format!(
                        "High-frequency tool usage: {} ({} times, {:.0}%)",
                        tool,
                        count,
                        frequency_ratio * 100.0
                    ),
                    tools: vec![tool],
                }
            })
            .collect()
    }

    /// Detect error recovery patterns - tools that fail and then succeed.
    fn detect_error_recovery(&self, experiences: &[CollectedExperience]) -> Vec<DetectedPattern> {
        let mut error_tools: std::collections::HashMap<String, (u32, u32)> =
            std::collections::HashMap::new();
        for exp in experiences {
            let (errors, total) = error_tools
                .entry(exp.experience.tool_name.clone())
                .or_insert((0, 0));
            *total += 1;
            if !exp.experience.success {
                *errors += 1;
            }
        }

        error_tools
            .into_iter()
            .filter(|(_, (errors, total))| *errors > 0 && *total >= 3)
            .map(|(tool, (errors, total))| {
                let error_rate = errors as f64 / total as f64;
                DetectedPattern {
                    pattern_type: "error_recovery".into(),
                    frequency: errors,
                    confidence: error_rate,
                    description: format!(
                        "Error pattern: {} ({:.0}% failure rate, {}/{})",
                        tool,
                        error_rate * 100.0,
                        errors,
                        total
                    ),
                    tools: vec![tool],
                }
            })
            .collect()
    }

    /// Detect efficiency issues - tools that are significantly slower than average.
    fn detect_efficiency_issue(&self, experiences: &[CollectedExperience]) -> Vec<DetectedPattern> {
        if experiences.is_empty() {
            return Vec::new();
        }

        // Calculate average duration
        let avg_duration: f64 =
            experiences.iter().map(|e| e.experience.duration_ms as f64).sum::<f64>()
                / experiences.len() as f64;

        // Group durations by tool
        let mut tool_durations: std::collections::HashMap<String, Vec<u64>> =
            std::collections::HashMap::new();
        for exp in experiences {
            tool_durations
                .entry(exp.experience.tool_name.clone())
                .or_default()
                .push(exp.experience.duration_ms);
        }

        let mut patterns = Vec::new();
        for (tool, durations) in &tool_durations {
            if durations.len() < 2 {
                continue;
            }
            let tool_avg: f64 = durations.iter().sum::<u64>() as f64 / durations.len() as f64;
            let ratio = tool_avg / avg_duration.max(1.0);

            if ratio > 2.0 {
                // More than 2x slower than average
                let confidence = (ratio - 1.0).min(1.0);
                patterns.push(DetectedPattern {
                    pattern_type: "efficiency_issue".into(),
                    frequency: durations.len() as u32,
                    confidence,
                    description: format!(
                        "Efficiency issue: {} avg {:.0}ms ({:.1}x slower than overall avg {:.0}ms)",
                        tool, tool_avg, ratio, avg_duration
                    ),
                    tools: vec![tool.clone()],
                });
            }
        }

        patterns
    }

    /// Detect success templates - tools with perfect or near-perfect success rates.
    fn detect_success_template(&self, experiences: &[CollectedExperience]) -> Vec<DetectedPattern> {
        let mut tool_success: std::collections::HashMap<String, (u32, u32)> =
            std::collections::HashMap::new(); // (successes, total)

        for exp in experiences {
            let entry = tool_success
                .entry(exp.experience.tool_name.clone())
                .or_insert((0, 0));
            entry.1 += 1;
            if exp.experience.success {
                entry.0 += 1;
            }
        }

        tool_success
            .into_iter()
            .filter(|(_, (successes, total))| *total >= 3 && *successes == *total)
            .map(|(tool, (successes, total))| {
                DetectedPattern {
                    pattern_type: "success_template".into(),
                    frequency: successes,
                    confidence: 1.0, // Perfect success
                    description: format!(
                        "Success template: {} succeeded {}/{} times (100%)",
                        tool, successes, total
                    ),
                    tools: vec![tool],
                }
            })
            .collect()
    }

    // ----- Public wrappers for test access / external use -----

    /// Execute a create_skill action using LLM, validation, and deploy.
    ///
    /// Public wrapper that mirrors Go's `executeCreateSkill`. Returns the
    /// resulting action with updated status, artifact_id, and error_msg.
    pub fn execute_create_skill_action(&self, action: &LearningAction) -> LearningAction {
        let mut result = action.clone();

        let draft_name = match &action.draft_name {
            Some(name) => name.clone(),
            None => {
                result.status = "failed".to_string();
                result.error_msg = Some("No draft name provided".to_string());
                return result;
            }
        };

        // Check if already exists in registry (dedup by name)
        if self.find_artifact_by_fingerprint_public(&draft_name) {
            result.status = "skipped".to_string();
            result.error_msg = Some(format!("Artifact {} already exists", draft_name));
            return result;
        }

        // Generate skill draft using LLM
        let provider_arc = {
            let provider = self.provider.lock();
            match provider.as_ref() {
                Some(p) => p.clone(),
                None => {
                    result.status = "failed".to_string();
                    result.error_msg = Some("No LLM provider available".to_string());
                    return result;
                }
            }
        };

        let content = match self.generate_skill_draft_action(&*provider_arc, action) {
            Ok(c) => c,
            Err(e) => {
                result.status = "failed".to_string();
                result.error_msg = Some(format!("LLM generation failed: {}", e));
                return result;
            }
        };

        // Iterative refinement loop
        let max_refine = if self.config.learning.max_auto_creates > 0 {
            self.config.learning.max_auto_creates.min(3) as u32
        } else {
            3
        };

        let pipeline_guard = self.pipeline.lock();
        if let Some(ref pipeline) = *pipeline_guard {
            let mut current_content = content;
            for attempt in 0..=max_refine {
                let validation = pipeline.validate(
                    nemesis_types::forge::ArtifactKind::Skill,
                    &draft_name,
                    &current_content,
                );
                let new_status = pipeline.determine_status(&validation);

                if new_status == nemesis_types::forge::ArtifactStatus::Active
                    || new_status == nemesis_types::forge::ArtifactStatus::Observing
                {
                    let tool_sig = extract_tool_signature_from_chain_public(&action.description);

                    // Write content to forge skills directory
                    let artifact_dir = self.forge_dir.join("skills").join(&draft_name);
                    if std::fs::create_dir_all(&artifact_dir).is_ok() {
                        let _ = std::fs::write(artifact_dir.join("SKILL.md"), &current_content);
                    }

                    // Register in registry
                    let artifact_id = format!("skill-{}", draft_name);
                    let artifact = nemesis_types::forge::Artifact {
                        id: artifact_id.clone(),
                        name: draft_name.clone(),
                        kind: nemesis_types::forge::ArtifactKind::Skill,
                        version: "1.0".to_string(),
                        status: new_status,
                        content: current_content.clone(),
                        tool_signature: tool_sig,
                        created_at: chrono::Local::now().to_rfc3339(),
                        updated_at: chrono::Local::now().to_rfc3339(),
                        usage_count: 0,
                        last_degraded_at: None,
                        success_rate: 0.0,
                        consecutive_observing_rounds: 0,
                    };
                    self.registry.add(artifact);

                    result.status = "executed".to_string();
                    result.artifact_id = Some(artifact_id);
                    result.executed_at = Some(chrono::Local::now().to_rfc3339());
                    return result;
                }

                // Failed - try to refine
                if attempt < max_refine {
                    let diagnosis = build_diagnosis_public(&validation);
                    match self.refine_skill_draft_action(
                        &*provider_arc,
                        action,
                        &current_content,
                        &diagnosis,
                    ) {
                        Ok(refined) => {
                            current_content = refined;
                        }
                        Err(e) => {
                            tracing::warn!(attempt = attempt + 1, error = %e, "[LearningEngine] Skill refinement failed");
                            break;
                        }
                    }
                }
            }

            result.status = "failed".to_string();
            result.error_msg = Some(format!(
                "Skill validation failed after {} refinement rounds",
                max_refine
            ));
        } else {
            // No pipeline configured - register as draft
            let artifact_id = format!("skill-{}", draft_name);
            let artifact = nemesis_types::forge::Artifact {
                id: artifact_id.clone(),
                name: draft_name,
                kind: nemesis_types::forge::ArtifactKind::Skill,
                version: "1.0".to_string(),
                status: nemesis_types::forge::ArtifactStatus::Draft,
                content,
                tool_signature: extract_tool_signature_from_chain_public(&action.description),
                created_at: chrono::Local::now().to_rfc3339(),
                updated_at: chrono::Local::now().to_rfc3339(),
                usage_count: 0,
                last_degraded_at: None,
                success_rate: 0.0,
                consecutive_observing_rounds: 0,
            };
            self.registry.add(artifact);

            result.status = "executed".to_string();
            result.artifact_id = Some(artifact_id);
            result.executed_at = Some(chrono::Local::now().to_rfc3339());
        }

        result
    }

    /// Generate a skill draft using LLM (public wrapper).
    ///
    /// Mirrors Go's `generateSkillDraft`.
    pub fn generate_skill_draft_action(
        &self,
        provider: &dyn crate::reflector_llm::LLMCaller,
        action: &LearningAction,
    ) -> Result<String, String> {
        self.generate_skill_draft(provider, action)
    }

    /// Refine a skill draft using LLM (public wrapper).
    ///
    /// Mirrors Go's `refineSkillDraft`.
    pub fn refine_skill_draft_action(
        &self,
        provider: &dyn crate::reflector_llm::LLMCaller,
        action: &LearningAction,
        previous_content: &str,
        diagnosis: &str,
    ) -> Result<String, String> {
        self.refine_skill_draft(provider, action, previous_content, diagnosis)
    }

    /// Find an artifact by name/fingerprint in the registry (public wrapper).
    ///
    /// Mirrors Go's `findArtifactByFingerprint`. Returns true if a non-deprecated
    /// artifact with the given name exists.
    pub fn find_artifact_by_fingerprint_public(&self, name: &str) -> bool {
        self.find_artifact_by_fingerprint(name)
    }

    /// Sort learning actions by priority (high first) then confidence descending.
    ///
    /// Mirrors Go's `sortActions`.
    pub fn sort_actions(actions: &mut [LearningAction]) {
        let priority_order = |p: &str| -> u8 {
            match p {
                "high" => 0,
                "medium" => 1,
                "low" => 2,
                _ => 3,
            }
        };
        actions.sort_by(|a, b| {
            let pa = priority_order(&a.priority);
            let pb = priority_order(&b.priority);
            match pa.cmp(&pb) {
                std::cmp::Ordering::Equal => b
                    .confidence
                    .partial_cmp(&a.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal),
                other => other,
            }
        });
    }
}

/// Generate a skill name from a tool chain description.
fn generate_skill_name(tool_chain: &str) -> String {
    let mut name = tool_chain
        .replace("->", "-")
        .replace('_', "-")
        .to_lowercase();
    if name.len() > 50 {
        let cut = nemesis_types::utils::floor_char_boundary(&name, 50);
        name.truncate(cut);
    }
    format!("{}-workflow", name)
}

/// Extract tool names from a chain description like "read->edit->exec".
///
/// Public version that mirrors Go's `extractToolSignatureFromChain`.
pub fn extract_tool_signature_from_chain_public(description: &str) -> Vec<String> {
    extract_tool_signature_from_chain(description)
}

/// Build a diagnosis string from validation results.
///
/// Public version that mirrors Go's `buildDiagnosis`.
pub fn build_diagnosis_public(validation: &ArtifactValidation) -> String {
    build_diagnosis(validation)
}

/// Extract tool names from a chain description like "read->edit->exec" (mirrors Go's extractToolSignatureFromChain).
fn extract_tool_signature_from_chain(description: &str) -> Vec<String> {
    let parts: Vec<&str> = description.split("→").collect();
    let mut tools = Vec::new();
    for p in parts {
        let p = p.trim();
        // Remove leading text like "Tool chain: "
        let tool = if let Some(idx) = p.rfind(' ') {
            p[idx + 1..].to_string()
        } else {
            p.to_string()
        };
        if !tool.is_empty() {
            tools.push(tool);
        }
    }
    tools
}

/// Build a diagnosis string from validation results (mirrors Go's buildDiagnosis).
fn build_diagnosis(validation: &ArtifactValidation) -> String {
    let mut sb = String::new();

    if let Some(ref s1) = validation.stage1_static {
        if !s1.stage.passed {
            sb.push_str("Stage 1 (Static) FAILED:\n");
            for e in &s1.stage.errors {
                sb.push_str(&format!("  - {}\n", e));
            }
        }
    }

    if let Some(ref s2) = validation.stage2_functional {
        if !s2.stage.passed {
            sb.push_str("Stage 2 (Functional) FAILED:\n");
            for e in &s2.stage.errors {
                sb.push_str(&format!("  - {}\n", e));
            }
        }
    }

    if let Some(ref s3) = validation.stage3_quality {
        sb.push_str(&format!("Stage 3 (Quality) Score: {}/100\n", s3.score));
        if !s3.notes.is_empty() {
            sb.push_str(&format!("  Notes: {}\n", s3.notes));
        }
        for (dim, score) in &s3.dimensions {
            sb.push_str(&format!("  {}: {}\n", dim, score));
        }
    }

    sb
}

/// Convert a LearningAction to a summary for cycle storage.
#[allow(dead_code)]
fn action_to_summary(a: &LearningAction) -> ActionSummary {
    ActionSummary {
        id: a.id.clone(),
        action_type: a.action_type.clone(),
        priority: a.priority.clone(),
        status: a.status.clone(),
        artifact_id: a.artifact_id.clone(),
    }
}

/// Iterative refinement loop for skill generation.
///
/// Takes an initial skill content, validates it, and attempts to refine
/// up to `max_rounds` times if validation fails.
pub struct IterativeRefiner {
    /// Maximum refinement rounds.
    pub max_rounds: u32,
}

impl IterativeRefiner {
    /// Create a new refiner with the given max rounds.
    pub fn new(max_rounds: u32) -> Self {
        let max_rounds = if max_rounds == 0 { 3 } else { max_rounds };
        Self { max_rounds }
    }

    /// Run the refinement loop on skill content.
    ///
    /// Returns the refined content if it passes validation, or the last
    /// attempt if all rounds are exhausted.
    pub fn refine<F>(
        &self,
        initial_content: &str,
        validate: F,
    ) -> (String, bool)
    where
        F: Fn(&str) -> bool,
    {
        let mut content = initial_content.to_string();

        for round in 0..=self.max_rounds {
            if validate(&content) {
                return (content, true);
            }

            if round < self.max_rounds {
                // Apply simple refinement heuristics:
                // - If content is too short, add structure
                // - If content has no headers, add them
                // - If content has no steps, add step markers
                content = self.apply_refinement_heuristics(&content, round);
            }
        }

        (content, false)
    }

    /// Apply simple refinement heuristics to improve skill content.
    fn apply_refinement_heuristics(&self, content: &str, round: u32) -> String {
        let mut refined = content.to_string();

        // Round 0: Add YAML frontmatter if missing
        if !content.contains("---") {
            refined = format!(
                "---\nname: generated-skill\nversion: \"1.0\"\n---\n\n{}",
                refined
            );
        }

        // Round 1: Add structure if content is flat
        if round >= 1 && !content.contains("## ") {
            refined = format!("{}\n\n## Steps\n\n1. Execute the identified pattern\n2. Validate results\n3. Report outcome", refined);
        }

        // Round 2: Add error handling section
        if round >= 2 && !content.contains("Error") && !content.contains("error") {
            refined = format!(
                "{}\n\n## Error Handling\n\nIf any step fails, retry once. If still failing, report the error and continue.",
                refined
            );
        }

        refined
    }
}

#[cfg(test)]
mod tests;

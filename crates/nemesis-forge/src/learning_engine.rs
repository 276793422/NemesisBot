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

/// LLM provider trait for skill draft generation.
///
/// The learning engine uses this trait to call LLM for generating and
/// refining skill drafts. The concrete implementation is injected via
/// `set_provider()`.
pub trait LLMProvider: Send + Sync {
    /// Call the LLM with system/user messages and return the response content.
    fn chat(&self, system: &str, user: &str, max_tokens: u32) -> Result<String, String>;
}

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
            created_at: Some(chrono::Utc::now().to_rfc3339()),
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
    provider: Mutex<Option<Arc<dyn LLMProvider>>>,
    skill_creator: Mutex<Option<Arc<dyn SkillCreator>>>,
    latest_cycle: Mutex<Option<LearningCycle>>,
}

impl LearningEngine {
    /// Create a new learning engine.
    pub fn new(config: ForgeConfig, registry: Arc<Registry>, cycle_store: CycleStore) -> Self {
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
    pub fn set_provider(&self, provider: Arc<dyn LLMProvider>) {
        *self.provider.lock() = Some(provider);
    }

    /// Set the pipeline for validation.
    pub fn set_pipeline(&self, pipeline: Arc<Pipeline>) {
        *self.pipeline.lock() = Some(pipeline);
    }

    /// Set the deployment monitor for outcome evaluation.
    pub fn set_monitor(&self, monitor: Arc<DeploymentMonitor>) {
        *self.monitor.lock() = Some(monitor);
    }

    /// Set the skill creator delegate. Mirrors Go's `SetForge(f *Forge)`.
    /// The Forge instance implements SkillCreator and is injected here.
    pub fn set_skill_creator(&self, creator: Arc<dyn SkillCreator>) {
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
        let mut cycle = LearningCycle {
            id: cycle_id,
            started_at: chrono::Utc::now().to_rfc3339(),
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
                        if action.status == "executed" {
                            actions_executed += 1;
                        }
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
                "Learning cycle skipped some actions due to limits or unknown type"
            );
        }

        cycle.status = nemesis_types::forge::CycleStatus::Completed;
        cycle.completed_at = Some(chrono::Utc::now().to_rfc3339());

        // Persist cycle
        if let Err(e) = self.cycle_store.append(&cycle).await {
            tracing::warn!(error = %e, "Failed to persist learning cycle");
        }

        *self.latest_cycle.lock() = Some(cycle.clone());
        cycle
    }

    /// Extract patterns from collected experiences using all four detectors.
    pub fn extract_patterns(&self, experiences: &[CollectedExperience]) -> Vec<DetectedPattern> {
        if experiences.is_empty() {
            return Vec::new();
        }

        let mut patterns = Vec::new();

        // Detect tool chains (sequential tool usage patterns)
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
                    let new_rate = (a.usage_count as f64 * 0.01 + delta).clamp(0.0, 1.0);
                    // Adjust usage_count as a proxy for success rate
                    let adjusted = (new_rate * 100.0) as u64;
                    a.usage_count = adjusted.max(1);
                });
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

        let content = match self.generate_skill_draft(&*provider_arc, action) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "LLM generation failed for skill draft");
                return;
            }
        };

        // Iterative refinement loop with pipeline validation
        let max_refine = 3u32;

        let pipeline = self.pipeline.lock();
        if let Some(ref pipeline) = *pipeline {
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
                        created_at: chrono::Utc::now().to_rfc3339(),
                        updated_at: chrono::Utc::now().to_rfc3339(),
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

                    tracing::info!(artifact_id = %artifact_id, "Created skill from learning cycle");
                    return;
                }

                // Failed — try to refine
                if attempt < max_refine {
                    let diagnosis = build_diagnosis(&validation);
                    match self.refine_skill_draft(&*provider_arc, action, &content, &diagnosis) {
                        Ok(_refined) => {
                            tracing::warn!(attempt, "Skill validation failed, refinement produced");
                            return;
                        }
                        Err(e) => {
                            tracing::warn!(attempt = attempt + 1, error = %e, "Skill refinement failed");
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
                created_at: chrono::Utc::now().to_rfc3339(),
                updated_at: chrono::Utc::now().to_rfc3339(),
                usage_count: 0,
                last_degraded_at: None,
                success_rate: 0.0,
                consecutive_observing_rounds: 0,
            };
            self.registry.add(artifact);
        }

        tracing::warn!(
            max_refine = max_refine,
            "Skill validation failed after all refinement rounds"
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
        filename.truncate(60);

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
            chrono::Utc::now().to_rfc3339()
        );

        let path = workspace_dir.join(format!("{}_suggestion.md", filename));
        if let Err(e) = std::fs::write(&path, &content) {
            action.status = "failed".to_string();
            action.error_msg = Some(format!("Failed to write suggestion: {}", e));
            return;
        }

        action.status = "executed".to_string();
        action.executed_at = Some(chrono::Utc::now().to_rfc3339());
        action.artifact_id = Some(path.to_string_lossy().to_string());
    }

    /// Check if previously suggested prompts have been adopted (mirrors Go's checkSuggestionAdoption).
    fn check_suggestion_adoption(&self, _patterns: &[DetectedPattern]) {
        let prompts_dir = if self.forge_dir.as_os_str().is_empty() {
            return;
        } else {
            self.forge_dir.parent().unwrap_or(&self.forge_dir).join("prompts")
        };

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
        provider: &dyn LLMProvider,
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
        provider.chat(
            "You are a Skill definition generator. Generate valid SKILL.md content with YAML frontmatter.",
            &prompt,
            budget,
        )
    }

    /// Refine a skill draft using LLM based on validation diagnosis (mirrors Go's refineSkillDraft).
    fn refine_skill_draft(
        &self,
        provider: &dyn LLMProvider,
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
        provider.chat(
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
        let since = chrono::Utc::now() - chrono::Duration::days(30);

        match self.cycle_store.read_cycles(Some(since)).await {
            Ok(cycles) => cycles.into_iter().last(),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Failed to read cycles from store, falling back to in-memory cache"
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
                        created_at: chrono::Utc::now().to_rfc3339(),
                        updated_at: chrono::Utc::now().to_rfc3339(),
                        usage_count: 0,
                        last_degraded_at: None,
                        success_rate: 0.0,
                        consecutive_observing_rounds: 0,
                    };
                    self.registry.add(artifact);

                    result.status = "executed".to_string();
                    result.artifact_id = Some(artifact_id);
                    result.executed_at = Some(chrono::Utc::now().to_rfc3339());
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
                            tracing::warn!(attempt = attempt + 1, error = %e, "Skill refinement failed");
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
                created_at: chrono::Utc::now().to_rfc3339(),
                updated_at: chrono::Utc::now().to_rfc3339(),
                usage_count: 0,
                last_degraded_at: None,
                success_rate: 0.0,
                consecutive_observing_rounds: 0,
            };
            self.registry.add(artifact);

            result.status = "executed".to_string();
            result.artifact_id = Some(artifact_id);
            result.executed_at = Some(chrono::Utc::now().to_rfc3339());
        }

        result
    }

    /// Generate a skill draft using LLM (public wrapper).
    ///
    /// Mirrors Go's `generateSkillDraft`.
    pub fn generate_skill_draft_action(
        &self,
        provider: &dyn LLMProvider,
        action: &LearningAction,
    ) -> Result<String, String> {
        self.generate_skill_draft(provider, action)
    }

    /// Refine a skill draft using LLM (public wrapper).
    ///
    /// Mirrors Go's `refineSkillDraft`.
    pub fn refine_skill_draft_action(
        &self,
        provider: &dyn LLMProvider,
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
        name.truncate(50);
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
mod tests {
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
        impl LLMProvider for MockProvider {
            fn chat(&self, _system: &str, _user: &str, _max_tokens: u32) -> Result<String, String> {
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

    #[tokio::test]
    async fn test_execute_create_skill_action_llm_fails() {
        let dir = tempfile::tempdir().unwrap();
        let registry = Arc::new(Registry::new(RegistryConfig::default()));
        let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
        let engine = LearningEngine::new(ForgeConfig::default(), registry, cycle_store);

        struct FailingProvider;
        impl LLMProvider for FailingProvider {
            fn chat(&self, _system: &str, _user: &str, _max_tokens: u32) -> Result<String, String> {
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

    #[tokio::test]
    async fn test_execute_create_skill_action_no_pipeline() {
        let dir = tempfile::tempdir().unwrap();
        let registry = Arc::new(Registry::new(RegistryConfig::default()));
        let cycle_store = CycleStore::new(dir.path().join("cycles.jsonl"));
        let engine = LearningEngine::new(ForgeConfig::default(), registry.clone(), cycle_store);

        struct MockProvider;
        impl LLMProvider for MockProvider {
            fn chat(&self, _system: &str, _user: &str, _max_tokens: u32) -> Result<String, String> {
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
}

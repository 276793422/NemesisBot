//! Deployment monitor - evaluates deployed artifact effectiveness.
//!
//! Tracks usage statistics and determines whether an artifact should be
//! promoted, kept in observation, or degraded.
//!
//! Enhanced with:
//! - Tool signature subsequence matching (matching Go's `matchesToolSignature`)
//! - Before/after comparison using conversation traces
//! - Effectiveness scoring: 0.4*rounds + 0.4*success + 0.2*duration
//! - 7-day cooldown check before degradation (respects `last_degraded_at`)
//! - Auto-upgrade: 3 consecutive "observing" rounds triggers "negative"
//!   verdict and automatic degradation
//! - `classify_verdict()` with configurable threshold from config
//! - `handle_verdict()` / `track_observing()` / `try_deprecate()` flow

use std::sync::Arc;

use nemesis_types::forge::{Artifact, ArtifactKind, ArtifactStatus};
use crate::config::ForgeConfig;
use crate::registry::Registry;

/// Evaluation result for a deployed artifact.
#[derive(Debug, Clone)]
pub struct EvaluationResult {
    /// Artifact ID.
    pub artifact_id: String,
    /// Verdict: positive, neutral, observing, negative, insufficient_data.
    pub verdict: String,
    /// Improvement score (positive = better, negative = worse).
    pub improvement_score: f64,
    /// Sample size used for evaluation.
    pub sample_size: usize,
}

/// Outcome of evaluating an artifact against conversation traces.
///
/// Matches Go's `ActionOutcome` struct.
#[derive(Debug, Clone)]
pub struct ActionOutcome {
    /// Artifact ID.
    pub artifact_id: String,
    /// When the measurement was taken.
    pub measured_at: String,
    /// Number of traces used for evaluation.
    pub sample_size: usize,
    /// Average rounds before deployment.
    pub rounds_before_avg: f64,
    /// Average rounds after deployment.
    pub rounds_after_avg: f64,
    /// Success rate before deployment (0.0-1.0).
    pub success_before: f64,
    /// Success rate after deployment (0.0-1.0).
    pub success_after: f64,
    /// Average duration before deployment (ms).
    pub duration_before_ms: i64,
    /// Average duration after deployment (ms).
    pub duration_after_ms: i64,
    /// Normalized improvement score.
    pub improvement_score: f64,
    /// Verdict: positive, neutral, observing, negative, insufficient_data.
    pub verdict: String,
}

/// Simplified conversation trace for evaluation purposes.
///
/// In Go, this maps to `ConversationTrace`. The monitor uses these
/// to compute before/after metrics for artifact evaluation.
#[derive(Debug, Clone)]
pub struct ConversationTrace {
    /// Trace start time (RFC 3339).
    pub start_time: String,
    /// Total rounds of interaction.
    pub total_rounds: u32,
    /// Duration in milliseconds.
    pub duration_ms: i64,
    /// Tool steps in the conversation.
    pub tool_steps: Vec<ToolStep>,
    /// Detected signals (empty = success).
    pub signals: Vec<String>,
}

/// A single tool step in a conversation trace.
#[derive(Debug, Clone)]
pub struct ToolStep {
    /// Tool name used.
    pub tool_name: String,
}

/// Monitors deployed artifacts and evaluates their effectiveness.
pub struct DeploymentMonitor {
    config: ForgeConfig,
    registry: Arc<Registry>,
}

impl DeploymentMonitor {
    /// Create a new deployment monitor.
    pub fn new(config: ForgeConfig, registry: Arc<Registry>) -> Self {
        Self { config, registry }
    }

    /// Evaluate a single artifact's deployment effectiveness (simple mode).
    ///
    /// Uses `success_rate` and `usage_count` as proxies when traces are not available.
    /// Matches Go's scoring logic using the same formula:
    /// `improvement = success_rate - baseline`, where baseline is 0.5.
    pub fn evaluate(&self, artifact: &Artifact) -> EvaluationResult {
        let sample_size = artifact.usage_count as usize;
        let min_samples = self.config.learning.min_outcome_samples as usize;
        let min_samples = if min_samples == 0 { 5 } else { min_samples };

        if sample_size < min_samples {
            return EvaluationResult {
                artifact_id: artifact.id.clone(),
                verdict: "observing".into(),
                improvement_score: 0.0,
                sample_size,
            };
        }

        // Calculate effectiveness score using success_rate field
        // Matching Go's pattern: success rate above 0.5 baseline is positive
        let improvement = artifact.success_rate - 0.5;

        let verdict = self.classify_verdict(improvement, artifact);

        EvaluationResult {
            artifact_id: artifact.id.clone(),
            verdict: verdict.to_string(),
            improvement_score: improvement,
            sample_size,
        }
    }

    /// Classify verdict based on improvement score and consecutive observing rounds.
    ///
    /// Matches Go's `classifyVerdict()`:
    /// - improvement > 0.1 => "positive"
    /// - improvement >= -0.1 => "neutral"
    /// - improvement >= threshold => "observing"
    /// - otherwise => "negative"
    pub fn classify_verdict(&self, improvement_score: f64, _artifact: &Artifact) -> String {
        let threshold = self.config.learning.degrade_threshold;
        let threshold = if threshold >= 0.0 { -0.2 } else { threshold };

        if improvement_score > 0.1 {
            "positive"
        } else if improvement_score >= -0.1 {
            "neutral"
        } else if improvement_score >= threshold {
            "observing"
        } else {
            "negative"
        }
        .to_string()
    }

    /// Evaluate outcomes for all active artifacts using conversation traces.
    ///
    /// Matches Go's `EvaluateOutcomes()`. This is the trace-based evaluation
    /// that compares before/after metrics.
    ///
    /// Returns `ActionOutcome` for each evaluated artifact.
    pub fn evaluate_outcomes(&self, traces: &[ConversationTrace]) -> Vec<ActionOutcome> {
        let mut outcomes = Vec::new();

        let window_days = self.config.learning.monitor_window_days;
        let window_days = if window_days == 0 { 7 } else { window_days };

        // Filter traces within the observation window
        let since = chrono::Local::now() - chrono::Duration::days(window_days as i64);
        let recent_traces: Vec<&ConversationTrace> = traces
            .iter()
            .filter(|t| {
                // Parse start_time and check if within window
                t.start_time
                    .get(..19)
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(&format!("{}Z", s)).ok())
                    .map(|dt| dt.with_timezone(&chrono::Local) >= since)
                    .unwrap_or(true)
            })
            .collect();

        if recent_traces.is_empty() {
            return outcomes;
        }

        // Get all active skills with ToolSignatures
        let artifacts = self.registry.list(None, None);
        for artifact in &artifacts {
            if artifact.status != ArtifactStatus::Active
                || artifact.kind != ArtifactKind::Skill
                || artifact.tool_signature.is_empty()
            {
                continue;
            }

            if let Some(outcome) = self.evaluate_artifact(artifact, &recent_traces) {
                // Handle auto-deprecation
                self.handle_verdict(artifact, &outcome);
                outcomes.push(outcome);
            }
        }

        outcomes
    }

    /// Evaluate a single artifact's effectiveness against traces.
    ///
    /// Matches Go's `evaluateArtifact()`. Uses tool signature subsequence
    /// matching to find relevant traces, then computes before/after metrics.
    pub fn evaluate_artifact(
        &self,
        artifact: &Artifact,
        traces: &[&ConversationTrace],
    ) -> Option<ActionOutcome> {
        let deploy_time = artifact.created_at.as_str();

        let mut before_traces: Vec<&ConversationTrace> = Vec::new();
        let mut after_traces: Vec<&ConversationTrace> = Vec::new();

        for t in traces {
            if !matches_tool_signature(t, &artifact.tool_signature) {
                continue;
            }
            if t.start_time.as_str() < deploy_time {
                before_traces.push(t);
            } else {
                after_traces.push(t);
            }
        }

        let min_samples = if self.config.learning.min_outcome_samples == 0 {
            5
        } else {
            self.config.learning.min_outcome_samples as usize
        };

        if after_traces.len() < min_samples {
            return Some(ActionOutcome {
                artifact_id: artifact.id.clone(),
                measured_at: chrono::Local::now().to_rfc3339(),
                sample_size: after_traces.len(),
                rounds_before_avg: 0.0,
                rounds_after_avg: 0.0,
                success_before: 0.0,
                success_after: 0.0,
                duration_before_ms: 0,
                duration_after_ms: 0,
                improvement_score: 0.0,
                verdict: "insufficient_data".to_string(),
            });
        }

        // Calculate metrics
        let before_rounds = avg_rounds(&before_traces);
        let after_rounds = avg_rounds(&after_traces);
        let before_success = success_rate(&before_traces);
        let after_success = success_rate(&after_traces);
        let before_dur = avg_duration(&before_traces);
        let after_dur = avg_duration(&after_traces);

        // Normalized improvement score: 0.4*rounds + 0.4*success + 0.2*duration
        let norm_rounds = normalize(before_rounds, after_rounds);
        let norm_success = after_success - before_success; // already 0-1 range
        let norm_duration = normalize(before_dur as f64, after_dur as f64);

        let improvement_score = 0.4 * norm_rounds + 0.4 * norm_success + 0.2 * norm_duration;

        let verdict = self.classify_verdict(improvement_score, artifact);

        Some(ActionOutcome {
            artifact_id: artifact.id.clone(),
            measured_at: chrono::Local::now().to_rfc3339(),
            sample_size: after_traces.len(),
            rounds_before_avg: before_rounds,
            rounds_after_avg: after_rounds,
            success_before: before_success,
            success_after: after_success,
            duration_before_ms: before_dur,
            duration_after_ms: after_dur,
            improvement_score,
            verdict,
        })
    }

    /// Handle the verdict for an artifact.
    ///
    /// Matches Go's `handleVerdict()`:
    /// - "negative" => try to deprecate
    /// - "observing" => track observing rounds
    /// - "positive" => reset observing counter
    pub fn handle_verdict(&self, artifact: &Artifact, outcome: &ActionOutcome) {
        match outcome.verdict.as_str() {
            "negative" => {
                self.try_deprecate(artifact);
            }
            "observing" => {
                self.track_observing(artifact);
            }
            "positive" => {
                // Reset observing counter on positive outcome
                self.registry.update(&artifact.id, |a| {
                    a.consecutive_observing_rounds = 0;
                });
            }
            _ => {}
        }
    }

    /// Try to deprecate an artifact with cooldown check.
    ///
    /// Matches Go's `tryDeprecate()`. Checks cooldown period and
    /// consecutive observing rounds before deprecating.
    pub fn try_deprecate(&self, artifact: &Artifact) {
        let cooldown_days = if self.config.learning.degradation_cooldown_days == 0 {
            7
        } else {
            self.config.learning.degradation_cooldown_days
        };

        // Check cooldown
        if let Some(ref last_degraded) = artifact.last_degraded_at {
            if let Ok(degraded_at) = chrono::DateTime::parse_from_rfc3339(last_degraded) {
                let now = chrono::Local::now();
                let days_since = (now - degraded_at.with_timezone(&chrono::Local)).num_days();
                if days_since < cooldown_days as i64 {
                    return; // still in cooldown
                }
            }
        }

        // Check if consecutive observing rounds >= 3 OR directly negative
        if artifact.consecutive_observing_rounds >= 3
            || artifact.consecutive_observing_rounds < 3
        {
            self.registry.update(&artifact.id, |a| {
                a.status = ArtifactStatus::Degraded;
                a.last_degraded_at = Some(chrono::Local::now().to_rfc3339());
                a.consecutive_observing_rounds = 0;
            });

            tracing::info!(
                artifact_id = %artifact.id,
                name = %artifact.name,
                "[Monitor] Artifact deprecated due to negative outcome"
            );
        }
    }

    /// Track observing rounds and auto-upgrade to deprecation if needed.
    ///
    /// Matches Go's `trackObserving()`. Increments the consecutive observing
    /// counter and triggers deprecation if it reaches 3.
    pub fn track_observing(&self, artifact: &Artifact) {
        self.registry.update(&artifact.id, |a| {
            a.consecutive_observing_rounds += 1;
            if a.consecutive_observing_rounds >= 3 {
                a.consecutive_observing_rounds = 3; // cap at 3
            }
        });

        // Re-read to check if we should deprecate
        if let Some(updated) = self.registry.get(&artifact.id) {
            if updated.consecutive_observing_rounds >= 3 {
                self.try_deprecate(&updated);
            }
        }
    }

    /// Evaluate all active/observing artifacts (simple mode).
    ///
    /// Also performs auto-upgrade: if an artifact has 3 or more consecutive
    /// "observing" rounds, it is automatically upgraded to "negative" verdict
    /// and degradation is triggered.
    pub fn evaluate_all(&self) -> Vec<EvaluationResult> {
        let artifacts = self.registry.list(None, None);
        let mut results = Vec::new();
        for artifact in artifacts
            .iter()
            .filter(|a| a.status == ArtifactStatus::Active || a.status == ArtifactStatus::Observing)
        {
            let result = self.evaluate(artifact);
            results.push(result);
        }

        // Post-process: auto-upgrade consecutive observing rounds to negative
        for result in results.iter_mut() {
            if let Some(artifact) = self.registry.get(&result.artifact_id) {
                if artifact.consecutive_observing_rounds >= 3 {
                    tracing::warn!(
                        artifact_id = %artifact.id,
                        consecutive_rounds = artifact.consecutive_observing_rounds,
                        "[Monitor] Auto-upgrading artifact to negative (3+ consecutive observing rounds)"
                    );
                    result.verdict = "negative".to_string();
                    result.improvement_score = -0.3;
                }
            }
        }

        results
    }

    /// Check if an artifact should be degraded based on consecutive observing rounds.
    pub fn should_degrade(&self, artifact: &Artifact) -> bool {
        artifact.consecutive_observing_rounds >= 3
    }

    /// Check if the cooldown period has elapsed since the last degradation.
    ///
    /// Returns `true` if degradation is allowed (no recent degradation or
    /// the cooldown period has passed). Returns `false` if the artifact was
    /// degraded recently and is still within the cooldown period.
    pub fn is_degradation_cooldown_elapsed(&self, artifact: &Artifact) -> bool {
        let cooldown_days = self.config.learning.degradation_cooldown_days;
        match &artifact.last_degraded_at {
            None => true, // Never degraded before
            Some(last_degraded) => {
                match chrono::DateTime::parse_from_rfc3339(last_degraded) {
                    Ok(degraded_at) => {
                        let now = chrono::Local::now();
                        let days_since = (now - degraded_at.with_timezone(&chrono::Local)).num_days();
                        days_since >= cooldown_days as i64
                    }
                    Err(_) => true, // Unparseable timestamp, allow degradation
                }
            }
        }
    }

    /// Apply degradation to an artifact if warranted and not in cooldown.
    ///
    /// Returns `true` if degradation was actually applied, `false` if
    /// skipped due to cooldown or artifact not found.
    pub fn apply_degradation(&self, artifact_id: &str) -> bool {
        // Check cooldown before updating
        if let Some(artifact) = self.registry.get(artifact_id) {
            if !self.is_degradation_cooldown_elapsed(&artifact) {
                return false;
            }
        }

        self.registry.update(artifact_id, |a| {
            a.status = ArtifactStatus::Degraded;
            a.last_degraded_at = Some(chrono::Local::now().to_rfc3339());
            a.consecutive_observing_rounds = 0;
        })
    }

    /// Run a full evaluation and auto-degradation cycle.
    ///
    /// Evaluates all active/observing artifacts, auto-upgrades consecutive
    /// observing rounds, and degrades any with "negative" verdict.
    ///
    /// Returns the list of evaluation results.
    pub fn run_evaluation_cycle(&self) -> Vec<EvaluationResult> {
        let results = self.evaluate_all();

        for result in &results {
            if result.verdict == "negative" {
                if let Some(artifact) = self.registry.get(&result.artifact_id) {
                    if self.is_degradation_cooldown_elapsed(&artifact) {
                        tracing::info!(
                            artifact_id = %result.artifact_id,
                            improvement_score = result.improvement_score,
                            "[Monitor] Degrading artifact due to negative evaluation"
                        );
                        self.apply_degradation(&result.artifact_id);
                    } else {
                        tracing::info!(
                            artifact_id = %result.artifact_id,
                            "[Monitor] Skipping degradation: cooldown period active"
                        );
                    }
                }
            }
        }

        results
    }
}

// ----- Helper functions (matching Go's helper functions) -----

/// Check if a trace's tool steps contain the given signature as a subsequence.
///
/// Matches Go's `matchesToolSignature()`.
pub fn matches_tool_signature(trace: &ConversationTrace, signature: &[String]) -> bool {
    if signature.is_empty() {
        return false;
    }

    let mut sig_idx = 0;
    for step in &trace.tool_steps {
        if step.tool_name == signature[sig_idx] {
            sig_idx += 1;
            if sig_idx == signature.len() {
                return true;
            }
        }
    }
    false
}

/// Calculate (before - after) / max(before, 1).
///
/// Matches Go's `normalize()`.
pub fn normalize(before: f64, after: f64) -> f64 {
    if before <= 0.0 {
        return 0.0;
    }
    (before - after) / before
}

/// Calculate average total rounds across traces.
///
/// Matches Go's `avgRounds()`.
pub fn avg_rounds(traces: &[&ConversationTrace]) -> f64 {
    if traces.is_empty() {
        return 0.0;
    }
    let total: u32 = traces.iter().map(|t| t.total_rounds).sum();
    total as f64 / traces.len() as f64
}

/// Calculate success rate (traces with no signals = success).
///
/// Matches Go's `successRate()`.
pub fn success_rate(traces: &[&ConversationTrace]) -> f64 {
    if traces.is_empty() {
        return 0.0;
    }
    let successes = traces.iter().filter(|t| t.signals.is_empty()).count();
    successes as f64 / traces.len() as f64
}

/// Calculate average duration across traces.
///
/// Matches Go's `avgDuration()`.
pub fn avg_duration(traces: &[&ConversationTrace]) -> i64 {
    if traces.is_empty() {
        return 0;
    }
    let total: i64 = traces.iter().map(|t| t.duration_ms).sum();
    total / traces.len() as i64
}

#[cfg(test)]
mod tests;

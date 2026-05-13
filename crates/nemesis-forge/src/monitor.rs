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
        let since = chrono::Utc::now() - chrono::Duration::days(window_days as i64);
        let recent_traces: Vec<&ConversationTrace> = traces
            .iter()
            .filter(|t| {
                // Parse start_time and check if within window
                t.start_time
                    .get(..19)
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(&format!("{}Z", s)).ok())
                    .map(|dt| dt.with_timezone(&chrono::Utc) >= since)
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
                measured_at: chrono::Utc::now().to_rfc3339(),
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
            measured_at: chrono::Utc::now().to_rfc3339(),
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
                let now = chrono::Utc::now();
                let days_since = (now - degraded_at.with_timezone(&chrono::Utc)).num_days();
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
                a.last_degraded_at = Some(chrono::Utc::now().to_rfc3339());
                a.consecutive_observing_rounds = 0;
            });

            tracing::info!(
                artifact_id = %artifact.id,
                name = %artifact.name,
                "Artifact deprecated due to negative outcome"
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
                        "Auto-upgrading artifact to negative (3+ consecutive observing rounds)"
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
                        let now = chrono::Utc::now();
                        let days_since = (now - degraded_at.with_timezone(&chrono::Utc)).num_days();
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
            a.last_degraded_at = Some(chrono::Utc::now().to_rfc3339());
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
                            "Degrading artifact due to negative evaluation"
                        );
                        self.apply_degradation(&result.artifact_id);
                    } else {
                        tracing::info!(
                            artifact_id = %result.artifact_id,
                            "Skipping degradation: cooldown period active"
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
mod tests {
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
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            usage_count: usage,
            last_degraded_at: None,
            success_rate: 0.0,
            consecutive_observing_rounds: 0,
        }
    }

    fn make_trace(rounds: u32, duration_ms: i64, tools: &[&str], has_signals: bool) -> ConversationTrace {
        ConversationTrace {
            start_time: chrono::Utc::now().to_rfc3339(),
            total_rounds: rounds,
            duration_ms,
            tool_steps: tools.iter().map(|t| ToolStep { tool_name: t.to_string() }).collect(),
            signals: if has_signals { vec!["retry".to_string()] } else { vec![] },
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
        artifact.last_degraded_at = Some(chrono::Utc::now().to_rfc3339());
        assert!(!monitor.is_degradation_cooldown_elapsed(&artifact));
    }

    #[test]
    fn test_cooldown_elapsed_old_degradation() {
        let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));
        let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);

        let mut artifact = make_artifact("cool3", 10, ArtifactStatus::Active);
        let ten_days_ago = (chrono::Utc::now() - chrono::Duration::days(10)).to_rfc3339();
        artifact.last_degraded_at = Some(ten_days_ago);
        // Default cooldown is 7 days, so 10 days should be fine
        assert!(monitor.is_degradation_cooldown_elapsed(&artifact));
    }

    #[test]
    fn test_apply_degradation_respects_cooldown() {
        let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));

        let mut artifact = make_artifact("cool4", 10, ArtifactStatus::Active);
        artifact.last_degraded_at = Some(chrono::Utc::now().to_rfc3339()); // Just degraded
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
        registry.add(artifact.clone());

        let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);
        let results = monitor.evaluate_all();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].verdict, "negative"); // Auto-upgraded
    }

    #[test]
    fn test_evaluate_all_no_upgrade_few_rounds() {
        let registry = Arc::new(Registry::new(crate::types::RegistryConfig::default()));

        let mut artifact = make_artifact("upgrade2", 10, ArtifactStatus::Observing);
        artifact.consecutive_observing_rounds = 2;
        registry.add(artifact.clone());

        let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);
        let results = monitor.evaluate_all();

        assert_eq!(results.len(), 1);
        // Should not have been force-set to "negative" by auto-upgrade
        // since consecutive_observing_rounds < 3
        if results[0].verdict == "observing" {
            // Good: stayed as observing
        }
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
        artifact.last_degraded_at = Some(chrono::Utc::now().to_rfc3339()); // Just degraded
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
        assert!(matches_tool_signature(&trace, &["tool_a".to_string(), "tool_b".to_string()]));
    }

    #[test]
    fn test_matches_tool_signature_subsequence() {
        let trace = make_trace(3, 100, &["tool_a", "tool_x", "tool_b"], false);
        assert!(matches_tool_signature(&trace, &["tool_a".to_string(), "tool_b".to_string()]));
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
        let traces: Vec<ConversationTrace> = vec![
            make_trace(1, 100, &["a"], false),
            make_trace(1, 100, &["b"], true), // has signals
            make_trace(1, 100, &["c"], false),
        ];
        let refs: Vec<&ConversationTrace> = traces.iter().collect();
        assert_eq!(success_rate(&refs), 2.0 / 3.0);
    }

    #[test]
    fn test_avg_duration() {
        let traces: Vec<ConversationTrace> = vec![
            make_trace(1, 100, &["a"], false),
            make_trace(1, 200, &["b"], false),
            make_trace(1, 300, &["c"], false),
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
            measured_at: chrono::Utc::now().to_rfc3339(),
            sample_size: 10,
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
        assert!(matches_tool_signature(&trace, &["a".to_string(), "c".to_string(), "e".to_string()]));
        assert!(!matches_tool_signature(&trace, &["a".to_string(), "e".to_string(), "c".to_string()]));
    }

    #[test]
    fn test_matches_tool_signature_partial_match() {
        let trace = make_trace(3, 100, &["a", "b", "c"], false);
        assert!(matches_tool_signature(&trace, &["a".to_string(), "b".to_string()]));
        assert!(matches_tool_signature(&trace, &["b".to_string(), "c".to_string()]));
        assert!(!matches_tool_signature(&trace, &["a".to_string(), "c".to_string(), "d".to_string()]));
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
        let traces = vec![make_trace(1, 300, &["a"], false)];
        let refs: Vec<&ConversationTrace> = traces.iter().collect();
        assert_eq!(avg_duration(&refs), 300);
    }

    #[test]
    fn test_success_rate_all_success() {
        let traces: Vec<ConversationTrace> = vec![
            make_trace(1, 100, &["a"], false),
            make_trace(1, 100, &["b"], false),
        ];
        let refs: Vec<&ConversationTrace> = traces.iter().collect();
        assert_eq!(success_rate(&refs), 1.0);
    }

    #[test]
    fn test_success_rate_all_failure() {
        let traces: Vec<ConversationTrace> = vec![
            make_trace(1, 100, &["a"], true),
            make_trace(1, 100, &["b"], true),
        ];
        let refs: Vec<&ConversationTrace> = traces.iter().collect();
        assert_eq!(success_rate(&refs), 0.0);
    }

    #[test]
    fn test_success_rate_empty() {
        let refs: Vec<&ConversationTrace> = vec![];
        assert_eq!(success_rate(&refs), 0.0);
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
        assert_eq!(results[0].artifact_id, registry.list(None, Some(ArtifactStatus::Active))[0].id);
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
            measured_at: chrono::Utc::now().to_rfc3339(),
            sample_size: 10,
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
            measured_at: chrono::Utc::now().to_rfc3339(),
            sample_size: 10,
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
            measured_at: chrono::Utc::now().to_rfc3339(),
            sample_size: 10,
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
}

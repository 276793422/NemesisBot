package forge

import (
	"os"
	"path/filepath"
	"time"

	"github.com/276793422/NemesisBot/module/logger"
)

// DeploymentMonitor evaluates the effectiveness of deployed Forge artifacts
// by comparing conversation traces before and after deployment.
type DeploymentMonitor struct {
	traceStore *TraceStore
	registry   *Registry
	config     *ForgeConfig
}

// NewDeploymentMonitor creates a new DeploymentMonitor.
func NewDeploymentMonitor(traceStore *TraceStore, registry *Registry, config *ForgeConfig) *DeploymentMonitor {
	return &DeploymentMonitor{
		traceStore: traceStore,
		registry:   registry,
		config:     config,
	}
}

// EvaluateOutcomes measures the effect of all active Forge Skills with ToolSignatures.
// Returns ActionOutcome for each evaluated artifact.
func (dm *DeploymentMonitor) EvaluateOutcomes() []*ActionOutcome {
	var outcomes []*ActionOutcome

	windowDays := dm.config.Learning.MonitorWindowDays
	if windowDays <= 0 {
		windowDays = 7
	}
	since := time.Now().UTC().AddDate(0, 0, -windowDays)

	// Read recent traces
	traces, err := dm.traceStore.ReadTraces(since)
	if err != nil || len(traces) == 0 {
		return nil
	}

	// Get all active skills with ToolSignatures
	artifacts := dm.registry.ListAll()
	for i := range artifacts {
		a := &artifacts[i]
		if a.Status != StatusActive || a.Type != ArtifactSkill || len(a.ToolSignature) == 0 {
			continue
		}

		outcome := dm.evaluateArtifact(a, traces)
		if outcome != nil {
			outcomes = append(outcomes, outcome)

			// Handle auto-deprecation
			dm.handleVerdict(a, outcome)
		}
	}

	return outcomes
}

// evaluateArtifact measures a single artifact's effectiveness.
func (dm *DeploymentMonitor) evaluateArtifact(artifact *Artifact, traces []*ConversationTrace) *ActionOutcome {
	deployTime := artifact.CreatedAt

	// Find traces that match the artifact's ToolSignature (subsequence match)
	var beforeTraces, afterTraces []*ConversationTrace

	for _, t := range traces {
		if !matchesToolSignature(t, artifact.ToolSignature) {
			continue
		}
		if t.StartTime.Before(deployTime) {
			beforeTraces = append(beforeTraces, t)
		} else {
			afterTraces = append(afterTraces, t)
		}
	}

	minSamples := dm.config.Learning.MinOutcomeSamples
	if minSamples <= 0 {
		minSamples = 5
	}

	// Check sample size threshold
	if len(afterTraces) < minSamples {
		return &ActionOutcome{
			ArtifactID: artifact.ID,
			MeasuredAt: time.Now().UTC(),
			SampleSize: len(afterTraces),
			Verdict:    "insufficient_data",
		}
	}

	// Calculate metrics
	beforeRounds := avgRounds(beforeTraces)
	afterRounds := avgRounds(afterTraces)
	beforeSuccess := successRate(beforeTraces)
	afterSuccess := successRate(afterTraces)
	beforeDur := avgDuration(beforeTraces)
	afterDur := avgDuration(afterTraces)

	// Normalized improvement score
	normRounds := normalize(beforeRounds, afterRounds)
	normSuccess := afterSuccess - beforeSuccess // already 0-1 range
	normDuration := normalize(float64(beforeDur), float64(afterDur))

	improvementScore := 0.4*normRounds + 0.4*normSuccess + 0.2*normDuration

	verdict := dm.classifyVerdict(improvementScore, artifact)

	return &ActionOutcome{
		ArtifactID:       artifact.ID,
		MeasuredAt:       time.Now().UTC(),
		SampleSize:       len(afterTraces),
		RoundsBeforeAvg:  beforeRounds,
		RoundsAfterAvg:   afterRounds,
		SuccessBefore:    beforeSuccess,
		SuccessAfter:     afterSuccess,
		DurationBeforeMs: beforeDur,
		DurationAfterMs:  afterDur,
		ImprovementScore: improvementScore,
		Verdict:          verdict,
	}
}

// classifyVerdict determines the verdict based on improvement score and consecutive observing rounds.
func (dm *DeploymentMonitor) classifyVerdict(improvementScore float64, artifact *Artifact) string {
	threshold := dm.config.Learning.DegradeThreshold
	if threshold >= 0 {
		threshold = -0.2
	}

	switch {
	case improvementScore > 0.1:
		return "positive"
	case improvementScore >= -0.1:
		return "neutral"
	case improvementScore >= threshold:
		return "observing"
	default:
		return "negative"
	}
}

// handleVerdict applies actions based on the outcome verdict.
func (dm *DeploymentMonitor) handleVerdict(artifact *Artifact, outcome *ActionOutcome) {
	switch outcome.Verdict {
	case "negative":
		dm.tryDeprecate(artifact)
	case "observing":
		dm.trackObserving(artifact)
	case "positive":
		// Reset observing counter on positive outcome
		dm.registry.Update(artifact.ID, func(a *Artifact) {
			a.ConsecutiveObservingRounds = 0
		})
	}
}

// tryDeprecate attempts to deprecate an artifact with cooldown check.
func (dm *DeploymentMonitor) tryDeprecate(artifact *Artifact) {
	cooldownDays := dm.config.Learning.DegradeCooldownDays
	if cooldownDays <= 0 {
		cooldownDays = 7
	}

	// Check cooldown
	if artifact.LastDegradedAt != nil && time.Since(*artifact.LastDegradedAt) < time.Duration(cooldownDays)*24*time.Hour {
		return // still in cooldown
	}

	// Check if consecutive observing rounds >= 3 (upgrade to deprecation)
	// OR directly negative verdict
	if artifact.ConsecutiveObservingRounds >= 3 || outcomeIsDirectNegative(artifact) {
		now := time.Now().UTC()
		dm.registry.Update(artifact.ID, func(a *Artifact) {
			a.Status = StatusDeprecated
			a.LastDegradedAt = &now
			a.ConsecutiveObservingRounds = 0
		})

		// Remove the -forge copy from workspace/skills/
		skillsDir := filepath.Join(filepath.Dir(dm.registry.path), "..", "..", "skills", artifact.Name+"-forge")
		if _, err := os.Stat(skillsDir); err == nil {
			os.RemoveAll(skillsDir)
		}

		logger.InfoCF("forge", "Artifact deprecated due to negative outcome", map[string]interface{}{
			"artifact_id": artifact.ID,
			"name":        artifact.Name,
		})
	}
}

// trackObserving increments the consecutive observing rounds counter.
func (dm *DeploymentMonitor) trackObserving(artifact *Artifact) {
	dm.registry.Update(artifact.ID, func(a *Artifact) {
		a.ConsecutiveObservingRounds++
		// If 3 consecutive observing rounds, upgrade to negative and deprecate
		if a.ConsecutiveObservingRounds >= 3 {
			a.ConsecutiveObservingRounds = 3 // cap at 3, handleVerdict will pick it up
		}
	})

	// Re-read to check if we should deprecate
	updated, found := dm.registry.Get(artifact.ID)
	if found && updated.ConsecutiveObservingRounds >= 3 {
		dm.tryDeprecate(&updated)
	}
}

// --- Helper functions ---

// matchesToolSignature checks if a trace's tool steps contain the given signature as a subsequence.
func matchesToolSignature(trace *ConversationTrace, signature []string) bool {
	if len(signature) == 0 {
		return false
	}

	sigIdx := 0
	for _, step := range trace.ToolSteps {
		if step.ToolName == signature[sigIdx] {
			sigIdx++
			if sigIdx == len(signature) {
				return true
			}
		}
	}
	return false
}

// normalize calculates (before - after) / max(before, 1).
func normalize(before, after float64) float64 {
	if before <= 0 {
		return 0
	}
	return (before - after) / before
}

func avgRounds(traces []*ConversationTrace) float64 {
	if len(traces) == 0 {
		return 0
	}
	var total float64
	for _, t := range traces {
		total += float64(t.TotalRounds)
	}
	return total / float64(len(traces))
}

func successRate(traces []*ConversationTrace) float64 {
	if len(traces) == 0 {
		return 0
	}
	successes := 0
	for _, t := range traces {
		if len(t.Signals) == 0 {
			successes++
		}
	}
	return float64(successes) / float64(len(traces))
}

func avgDuration(traces []*ConversationTrace) int64 {
	if len(traces) == 0 {
		return 0
	}
	var total int64
	for _, t := range traces {
		total += t.DurationMs
	}
	return total / int64(len(traces))
}

func outcomeIsDirectNegative(artifact *Artifact) bool {
	// Direct negative if there are no consecutive observing rounds
	// (meaning the verdict was directly "negative", not upgraded from "observing")
	return artifact.ConsecutiveObservingRounds < 3
}

// MatchesToolSignatureForTest exposes matchesToolSignature for testing.
func MatchesToolSignatureForTest(trace *ConversationTrace, signature []string) bool {
	return matchesToolSignature(trace, signature)
}

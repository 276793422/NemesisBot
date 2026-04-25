package forge

import (
	"fmt"
	"testing"
	"time"
)

// --- matchesToolSignature tests ---

func TestMatchesToolSignature_EmptySignature(t *testing.T) {
	trace := &ConversationTrace{
		ToolSteps: []ToolStep{{ToolName: "read_file"}},
	}
	if matchesToolSignature(trace, nil) {
		t.Error("Empty signature should not match")
	}
	if matchesToolSignature(trace, []string{}) {
		t.Error("Empty signature should not match")
	}
}

func TestMatchesToolSignature_ExactMatch(t *testing.T) {
	trace := &ConversationTrace{
		ToolSteps: []ToolStep{
			{ToolName: "read_file"},
			{ToolName: "edit_file"},
			{ToolName: "exec"},
		},
	}
	if !matchesToolSignature(trace, []string{"read_file", "edit_file", "exec"}) {
		t.Error("Exact match should succeed")
	}
}

func TestMatchesToolSignature_Subsequence(t *testing.T) {
	trace := &ConversationTrace{
		ToolSteps: []ToolStep{
			{ToolName: "read_file"},
			{ToolName: "other"},
			{ToolName: "edit_file"},
			{ToolName: "exec"},
		},
	}
	if !matchesToolSignature(trace, []string{"read_file", "edit_file", "exec"}) {
		t.Error("Subsequence match should succeed")
	}
}

func TestMatchesToolSignature_PartialNoMatch(t *testing.T) {
	trace := &ConversationTrace{
		ToolSteps: []ToolStep{
			{ToolName: "read_file"},
			{ToolName: "edit_file"},
		},
	}
	if matchesToolSignature(trace, []string{"read_file", "exec"}) {
		t.Error("Partial match without full signature should fail")
	}
}

func TestMatchesToolSignature_SingleTool(t *testing.T) {
	trace := &ConversationTrace{
		ToolSteps: []ToolStep{
			{ToolName: "read_file"},
			{ToolName: "edit_file"},
		},
	}
	if !matchesToolSignature(trace, []string{"read_file"}) {
		t.Error("Single tool should match as subsequence")
	}
}

// --- normalize tests ---

func TestNormalize_NormalCase(t *testing.T) {
	result := normalize(10.0, 8.0)
	// (10-8)/10 = 0.2
	if result < 0.19 || result > 0.21 {
		t.Errorf("Expected ~0.2, got %f", result)
	}
}

func TestNormalize_ZeroBefore(t *testing.T) {
	result := normalize(0.0, 5.0)
	if result != 0 {
		t.Errorf("Expected 0 when before is 0, got %f", result)
	}
}

func TestNormalize_Improvement(t *testing.T) {
	result := normalize(10.0, 5.0)
	// (10-5)/10 = 0.5
	if result < 0.49 || result > 0.51 {
		t.Errorf("Expected ~0.5, got %f", result)
	}
}

func TestNormalize_Regression(t *testing.T) {
	result := normalize(5.0, 10.0)
	// (5-10)/5 = -1.0
	if result > 0 {
		t.Errorf("Expected negative for regression, got %f", result)
	}
}

// --- avgRounds, successRate, avgDuration tests ---

func TestAvgRounds_Empty(t *testing.T) {
	result := avgRounds(nil)
	if result != 0 {
		t.Errorf("Expected 0 for nil traces, got %f", result)
	}
}

func TestAvgRounds_Values(t *testing.T) {
	traces := []*ConversationTrace{
		{TotalRounds: 4},
		{TotalRounds: 6},
	}
	result := avgRounds(traces)
	if result != 5.0 {
		t.Errorf("Expected 5.0, got %f", result)
	}
}

func TestSuccessRate_Empty(t *testing.T) {
	result := successRate(nil)
	if result != 0 {
		t.Errorf("Expected 0 for nil traces, got %f", result)
	}
}

func TestSuccessRate_Values(t *testing.T) {
	traces := []*ConversationTrace{
		{},                               // no signals = success
		{Signals: []SessionSignal{{}}},   // has signals = failure
		{},                               // no signals = success
	}
	result := successRate(traces)
	if result != 2.0/3.0 {
		t.Errorf("Expected 2/3, got %f", result)
	}
}

func TestAvgDuration_Empty(t *testing.T) {
	result := avgDuration(nil)
	if result != 0 {
		t.Errorf("Expected 0 for nil traces, got %d", result)
	}
}

func TestAvgDuration_Values(t *testing.T) {
	traces := []*ConversationTrace{
		{DurationMs: 100},
		{DurationMs: 300},
	}
	result := avgDuration(traces)
	if result != 200 {
		t.Errorf("Expected 200, got %d", result)
	}
}

// --- classifyVerdict tests ---

func TestClassifyVerdict(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	cfg.Learning.DegradeThreshold = -0.2
	traceStore := NewTraceStore(tmpDir, cfg)
	registry := NewRegistry(tmpDir + "/registry.json")
	dm := NewDeploymentMonitor(traceStore, registry, cfg)

	artifact := &Artifact{ID: "test"}

	tests := []struct {
		score    float64
		expected string
	}{
		{0.5, "positive"},
		{0.11, "positive"},      // > 0.1 is positive
		{0.1, "neutral"},        // not > 0.1
		{0.0, "neutral"},        // >= -0.1
		{-0.1, "neutral"},       // >= -0.1 (boundary)
		{-0.11, "observing"},    // >= threshold (-0.2) but < -0.1
		{-0.3, "negative"},      // < threshold (-0.2)
	}

	for _, tt := range tests {
		result := dm.classifyVerdict(tt.score, artifact)
		if result != tt.expected {
			t.Errorf("classifyVerdict(%.2f) = %q, expected %q", tt.score, result, tt.expected)
		}
	}
}

// --- outcomeIsDirectNegative tests ---

func TestOutcomeIsDirectNegative(t *testing.T) {
	artifact := &Artifact{ConsecutiveObservingRounds: 0}
	if !outcomeIsDirectNegative(artifact) {
		t.Error("Should be direct negative when consecutive rounds < 3")
	}

	artifact.ConsecutiveObservingRounds = 2
	if !outcomeIsDirectNegative(artifact) {
		t.Error("Should be direct negative when consecutive rounds < 3")
	}

	artifact.ConsecutiveObservingRounds = 3
	if outcomeIsDirectNegative(artifact) {
		t.Error("Should NOT be direct negative when consecutive rounds >= 3")
	}
}

// --- DeploymentMonitor evaluateArtifact tests ---

func TestEvaluateArtifact_InsufficientData(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	cfg.Learning.MinOutcomeSamples = 5
	traceStore := NewTraceStore(tmpDir, cfg)
	registry := NewRegistry(tmpDir + "/registry.json")
	dm := NewDeploymentMonitor(traceStore, registry, cfg)

	artifact := &Artifact{
		ID:            "skill-test",
		Type:          ArtifactSkill,
		Status:        StatusActive,
		ToolSignature: []string{"read_file", "edit_file"},
		CreatedAt:     time.Now().UTC().Add(-7 * 24 * time.Hour),
	}

	// Only 2 traces (below MinOutcomeSamples=5)
	traces := []*ConversationTrace{
		{
			StartTime:   time.Now().UTC().Add(-1 * time.Hour),
			TotalRounds: 3,
			DurationMs:  100,
			ToolSteps:   []ToolStep{{ToolName: "read_file"}, {ToolName: "edit_file"}},
		},
		{
			StartTime:   time.Now().UTC().Add(-30 * time.Minute),
			TotalRounds: 2,
			DurationMs:  80,
			ToolSteps:   []ToolStep{{ToolName: "read_file"}, {ToolName: "edit_file"}},
		},
	}

	outcome := dm.evaluateArtifact(artifact, traces)
	if outcome == nil {
		t.Fatal("Expected non-nil outcome")
	}
	if outcome.Verdict != "insufficient_data" {
		t.Errorf("Expected 'insufficient_data', got '%s'", outcome.Verdict)
	}
	if outcome.SampleSize != 2 {
		t.Errorf("Expected SampleSize 2, got %d", outcome.SampleSize)
	}
}

func TestEvaluateArtifact_SufficientData(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	cfg.Learning.MinOutcomeSamples = 3
	traceStore := NewTraceStore(tmpDir, cfg)
	registry := NewRegistry(tmpDir + "/registry.json")
	dm := NewDeploymentMonitor(traceStore, registry, cfg)

	deployTime := time.Now().UTC().Add(-3 * 24 * time.Hour)
	artifact := &Artifact{
		ID:            "skill-test",
		Type:          ArtifactSkill,
		Status:        StatusActive,
		ToolSignature: []string{"read_file", "edit_file"},
		CreatedAt:     deployTime,
	}

	// 2 before traces + 5 after traces
	var traces []*ConversationTrace
	for i := 0; i < 2; i++ {
		traces = append(traces, &ConversationTrace{
			StartTime:   deployTime.Add(-48 * time.Hour),
			TotalRounds: 8,
			DurationMs:  1000,
			ToolSteps:   []ToolStep{{ToolName: "read_file"}, {ToolName: "edit_file"}},
		})
	}
	for i := 0; i < 5; i++ {
		traces = append(traces, &ConversationTrace{
			StartTime:   deployTime.Add(24 * time.Hour),
			TotalRounds: 3,
			DurationMs:  300,
			ToolSteps:   []ToolStep{{ToolName: "read_file"}, {ToolName: "edit_file"}},
		})
	}

	outcome := dm.evaluateArtifact(artifact, traces)
	if outcome == nil {
		t.Fatal("Expected non-nil outcome")
	}
	if outcome.Verdict == "insufficient_data" {
		t.Error("Should have sufficient data (5 >= 3)")
	}
	if outcome.SampleSize != 5 {
		t.Errorf("Expected 5 after traces, got %d", outcome.SampleSize)
	}
	if outcome.ImprovementScore <= 0 {
		t.Errorf("Expected positive improvement (fewer rounds), got %f", outcome.ImprovementScore)
	}
}

// --- handleVerdict tests ---

func TestHandleVerdict_Positive(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	traceStore := NewTraceStore(tmpDir, cfg)
	registry := NewRegistry(tmpDir + "/registry.json")
	dm := NewDeploymentMonitor(traceStore, registry, cfg)

	artifact := Artifact{
		ID:                       "skill-test",
		Type:                     ArtifactSkill,
		Status:                   StatusActive,
		ConsecutiveObservingRounds: 3,
	}
	registry.Add(artifact)

	outcome := &ActionOutcome{
		ArtifactID: "skill-test",
		Verdict:    "positive",
	}

	dm.handleVerdict(&artifact, outcome)

	updated, _ := registry.Get("skill-test")
	if updated.ConsecutiveObservingRounds != 0 {
		t.Errorf("Positive verdict should reset consecutive observing rounds, got %d", updated.ConsecutiveObservingRounds)
	}
}

func TestHandleVerdict_Observing(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	traceStore := NewTraceStore(tmpDir, cfg)
	registry := NewRegistry(tmpDir + "/registry.json")
	dm := NewDeploymentMonitor(traceStore, registry, cfg)

	artifact := Artifact{
		ID:     "skill-test",
		Type:   ArtifactSkill,
		Status: StatusActive,
	}
	registry.Add(artifact)

	outcome := &ActionOutcome{
		ArtifactID: "skill-test",
		Verdict:    "observing",
	}

	dm.handleVerdict(&artifact, outcome)

	updated, _ := registry.Get("skill-test")
	if updated.ConsecutiveObservingRounds != 1 {
		t.Errorf("Observing verdict should increment to 1, got %d", updated.ConsecutiveObservingRounds)
	}
}

func TestHandleVerdict_TriggerDeprecation(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	cfg.Learning.DegradeCooldownDays = 0 // no cooldown
	traceStore := NewTraceStore(tmpDir, cfg)
	registry := NewRegistry(tmpDir + "/registry.json")
	dm := NewDeploymentMonitor(traceStore, registry, cfg)

	artifact := Artifact{
		ID:                        "skill-test",
		Type:                      ArtifactSkill,
		Status:                    StatusActive,
		ConsecutiveObservingRounds: 2, // will be incremented to 3
	}
	registry.Add(artifact)

	outcome := &ActionOutcome{
		ArtifactID: "skill-test",
		Verdict:    "observing",
	}

	dm.handleVerdict(&artifact, outcome)

	updated, _ := registry.Get("skill-test")
	if updated.Status != StatusDeprecated {
		t.Errorf("3 consecutive observing rounds should trigger deprecation, got %s", updated.Status)
	}
}

// --- EvaluateOutcomes with empty traceStore ---

func TestEvaluateOutcomes_NoTraces(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	traceStore := NewTraceStore(tmpDir, cfg)
	registry := NewRegistry(tmpDir + "/registry.json")
	dm := NewDeploymentMonitor(traceStore, registry, cfg)

	outcomes := dm.EvaluateOutcomes()
	if outcomes != nil {
		t.Error("Expected nil outcomes when no traces exist")
	}
}

func TestEvaluateOutcomes_WithActiveArtifact(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	cfg.Learning.MinOutcomeSamples = 2
	cfg.Learning.MonitorWindowDays = 30
	traceStore := NewTraceStore(tmpDir, cfg)
	registry := NewRegistry(tmpDir + "/registry.json")
	dm := NewDeploymentMonitor(traceStore, registry, cfg)

	// Add artifact
	artifact := Artifact{
		ID:            "skill-test",
		Type:          ArtifactSkill,
		Status:        StatusActive,
		ToolSignature: []string{"a", "b"},
		CreatedAt:     time.Now().UTC().Add(-7 * 24 * time.Hour),
	}
	registry.Add(artifact)

	// Write traces
	for i := 0; i < 3; i++ {
		trace := &ConversationTrace{
			TraceID:     fmt.Sprintf("trace-%d", i),
			StartTime:   time.Now().UTC(),
			EndTime:     time.Now().UTC().Add(time.Minute),
			DurationMs:  60000,
			TotalRounds: 3,
			ToolSteps: []ToolStep{
				{ToolName: "a", Success: true},
				{ToolName: "b", Success: true},
			},
		}
		if err := traceStore.Append(trace); err != nil {
			t.Fatalf("Failed to append trace: %v", err)
		}
	}

	outcomes := dm.EvaluateOutcomes()
	if len(outcomes) == 0 {
		t.Fatal("Expected at least one outcome")
	}
	if outcomes[0].ArtifactID != "skill-test" {
		t.Errorf("Expected artifact ID 'skill-test', got '%s'", outcomes[0].ArtifactID)
	}
}

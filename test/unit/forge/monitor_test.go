package forge_test

import (
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/forge"
)

func TestMonitorMatchesToolSignature(t *testing.T) {
	trace := &forge.ConversationTrace{
		ToolSteps: []forge.ToolStep{
			{ToolName: "read"},
			{ToolName: "edit"},
			{ToolName: "exec"},
		},
	}

	if !forge.MatchesToolSignatureForTest(trace, []string{"read", "edit", "exec"}) {
		t.Error("Should match exact signature")
	}

	if !forge.MatchesToolSignatureForTest(trace, []string{"read", "exec"}) {
		t.Error("Should match subsequence signature")
	}

	if forge.MatchesToolSignatureForTest(trace, []string{"exec", "read"}) {
		t.Error("Should not match reversed signature")
	}

	if forge.MatchesToolSignatureForTest(trace, []string{}) {
		t.Error("Should not match empty signature")
	}
}

func TestMonitorEvaluateOutcomesInsufficientData(t *testing.T) {
	tmpDir := t.TempDir()
	forgeDir := filepath.Join(tmpDir, "forge")
	os.MkdirAll(forgeDir, 0755)

	cfg := forge.DefaultForgeConfig()
	cfg.Learning.MinOutcomeSamples = 5
	cfg.Learning.MonitorWindowDays = 30 // use 30 day window so traces are included

	traceStore := forge.NewTraceStore(forgeDir, cfg)
	registryPath := filepath.Join(forgeDir, "registry.json")
	registry := forge.NewRegistry(registryPath)

	registry.Add(forge.Artifact{
		ID:            "skill-test",
		Type:          forge.ArtifactSkill,
		Name:          "test-skill",
		Status:        forge.StatusActive,
		ToolSignature: []string{"read", "edit"},
		CreatedAt:     time.Now().UTC().AddDate(0, 0, -1),
	})

	now := time.Now().UTC()
	for i := 0; i < 3; i++ {
		trace := &forge.ConversationTrace{
			TraceID:   "trace-" + string(rune(i)),
			StartTime: now.Add(-time.Duration(i) * time.Hour),
			ToolSteps: []forge.ToolStep{
				{ToolName: "read", Success: true},
				{ToolName: "edit", Success: true},
			},
		}
		traceStore.Append(trace)
	}

	monitor := forge.NewDeploymentMonitor(traceStore, registry, cfg)
	outcomes := monitor.EvaluateOutcomes()

	if len(outcomes) != 1 {
		t.Fatalf("Expected 1 outcome, got %d", len(outcomes))
	}
	if outcomes[0].Verdict != "insufficient_data" {
		t.Errorf("Expected verdict 'insufficient_data', got '%s'", outcomes[0].Verdict)
	}
}

func TestMonitorEvaluateOutcomesPositive(t *testing.T) {
	tmpDir := t.TempDir()
	forgeDir := filepath.Join(tmpDir, "forge")
	os.MkdirAll(forgeDir, 0755)

	cfg := forge.DefaultForgeConfig()
	cfg.Learning.MinOutcomeSamples = 5
	cfg.Learning.MonitorWindowDays = 30

	traceStore := forge.NewTraceStore(forgeDir, cfg)
	registryPath := filepath.Join(forgeDir, "registry.json")
	registry := forge.NewRegistry(registryPath)

	// Add artifact first, then use its actual CreatedAt
	registry.Add(forge.Artifact{
		ID:            "skill-test",
		Type:          forge.ArtifactSkill,
		Name:          "test-skill",
		Status:        forge.StatusActive,
		ToolSignature: []string{"read", "edit"},
	})
	artifact, _ := registry.Get("skill-test")
	deployTime := artifact.CreatedAt

	// Before traces: slow, low success (before deployTime)
	for i := 0; i < 8; i++ {
		trace := &forge.ConversationTrace{
			TraceID:     "before-" + string(rune(i)),
			StartTime:   deployTime.Add(-time.Duration(i+1) * time.Minute),
			DurationMs:  5000,
			TotalRounds: 10,
			ToolSteps: []forge.ToolStep{
				{ToolName: "read", Success: true},
				{ToolName: "edit", Success: true},
			},
			Signals: []forge.SessionSignal{{Type: "retry"}},
		}
		traceStore.Append(trace)
	}

	// After traces: fast, high success (after deployTime)
	for i := 0; i < 6; i++ {
		trace := &forge.ConversationTrace{
			TraceID:     "after-" + string(rune(i)),
			StartTime:   deployTime.Add(time.Duration(i+1) * time.Minute),
			DurationMs:  1000,
			TotalRounds: 3,
			ToolSteps: []forge.ToolStep{
				{ToolName: "read", Success: true},
				{ToolName: "edit", Success: true},
			},
		}
		traceStore.Append(trace)
	}

	monitor := forge.NewDeploymentMonitor(traceStore, registry, cfg)
	outcomes := monitor.EvaluateOutcomes()

	if len(outcomes) != 1 {
		t.Fatalf("Expected 1 outcome, got %d", len(outcomes))
	}
	o := outcomes[0]
	if o.Verdict != "positive" {
		t.Errorf("Expected verdict 'positive', got '%s' (score=%.3f)", o.Verdict, o.ImprovementScore)
	}
	if o.SampleSize != 6 {
		t.Errorf("Expected sample size 6, got %d", o.SampleSize)
	}
}

func TestMonitorEvaluateOutcomesNegative(t *testing.T) {
	tmpDir := t.TempDir()
	forgeDir := filepath.Join(tmpDir, "forge")
	os.MkdirAll(forgeDir, 0755)

	cfg := forge.DefaultForgeConfig()
	cfg.Learning.MinOutcomeSamples = 5
	cfg.Learning.MonitorWindowDays = 30

	traceStore := forge.NewTraceStore(forgeDir, cfg)
	registryPath := filepath.Join(forgeDir, "registry.json")
	registry := forge.NewRegistry(registryPath)

	registry.Add(forge.Artifact{
		ID:            "skill-bad",
		Type:          forge.ArtifactSkill,
		Name:          "bad-skill",
		Status:        forge.StatusActive,
		ToolSignature: []string{"read"},
	})
	artifact, _ := registry.Get("skill-bad")
	deployTime := artifact.CreatedAt

	for i := 0; i < 8; i++ {
		trace := &forge.ConversationTrace{
			TraceID:     "before-" + string(rune(i)),
			StartTime:   deployTime.Add(-time.Duration(i+1) * time.Minute),
			DurationMs:  1000,
			TotalRounds: 2,
			ToolSteps:   []forge.ToolStep{{ToolName: "read", Success: true}},
		}
		traceStore.Append(trace)
	}

	for i := 0; i < 6; i++ {
		trace := &forge.ConversationTrace{
			TraceID:     "after-" + string(rune(i)),
			StartTime:   deployTime.Add(time.Duration(i+1) * time.Minute),
			DurationMs:  10000,
			TotalRounds: 20,
			ToolSteps:   []forge.ToolStep{{ToolName: "read", Success: false}},
			Signals:     []forge.SessionSignal{{Type: "retry"}},
		}
		traceStore.Append(trace)
	}

	monitor := forge.NewDeploymentMonitor(traceStore, registry, cfg)
	outcomes := monitor.EvaluateOutcomes()

	if len(outcomes) != 1 {
		t.Fatalf("Expected 1 outcome, got %d", len(outcomes))
	}
	if outcomes[0].Verdict != "negative" {
		t.Errorf("Expected verdict 'negative', got '%s' (score=%.3f)", outcomes[0].Verdict, outcomes[0].ImprovementScore)
	}
}

func TestMonitorAutoDeprecation(t *testing.T) {
	tmpDir := t.TempDir()
	forgeDir := filepath.Join(tmpDir, "forge")
	os.MkdirAll(forgeDir, 0755)

	cfg := forge.DefaultForgeConfig()
	cfg.Learning.MinOutcomeSamples = 5
	cfg.Learning.DegradeCooldownDays = 7
	cfg.Learning.MonitorWindowDays = 30

	traceStore := forge.NewTraceStore(forgeDir, cfg)
	registryPath := filepath.Join(forgeDir, "registry.json")
	registry := forge.NewRegistry(registryPath)

	registry.Add(forge.Artifact{
		ID:            "skill-deprecate",
		Type:          forge.ArtifactSkill,
		Name:          "deprecate-skill",
		Status:        forge.StatusActive,
		ToolSignature: []string{"read"},
	})
	artifact, _ := registry.Get("skill-deprecate")
	deployTime := artifact.CreatedAt

	for i := 0; i < 8; i++ {
		trace := &forge.ConversationTrace{
			TraceID:     "before-" + string(rune(i)),
			StartTime:   deployTime.Add(-time.Duration(i+1) * time.Minute),
			TotalRounds: 2,
			ToolSteps:   []forge.ToolStep{{ToolName: "read", Success: true}},
		}
		traceStore.Append(trace)
	}

	for i := 0; i < 6; i++ {
		trace := &forge.ConversationTrace{
			TraceID:     "after-" + string(rune(i)),
			StartTime:   deployTime.Add(time.Duration(i+1) * time.Minute),
			DurationMs:  50000,
			TotalRounds: 50,
			ToolSteps:   []forge.ToolStep{{ToolName: "read", Success: false}},
			Signals:     []forge.SessionSignal{{Type: "retry"}},
		}
		traceStore.Append(trace)
	}

	monitor := forge.NewDeploymentMonitor(traceStore, registry, cfg)
	monitor.EvaluateOutcomes()

	artifact, found := registry.Get("skill-deprecate")
	if !found {
		t.Fatal("Artifact not found")
	}
	if artifact.Status != forge.StatusDeprecated {
		t.Errorf("Expected artifact to be deprecated, got status '%s'", artifact.Status)
	}
}

func TestMonitorDeprecationCooldown(t *testing.T) {
	tmpDir := t.TempDir()
	forgeDir := filepath.Join(tmpDir, "forge")
	os.MkdirAll(forgeDir, 0755)

	cfg := forge.DefaultForgeConfig()
	cfg.Learning.MinOutcomeSamples = 5
	cfg.Learning.DegradeCooldownDays = 7
	cfg.Learning.MonitorWindowDays = 30

	traceStore := forge.NewTraceStore(forgeDir, cfg)
	registryPath := filepath.Join(forgeDir, "registry.json")
	registry := forge.NewRegistry(registryPath)

	recentlyDegraded := time.Now().UTC().AddDate(0, 0, -2)
	registry.Add(forge.Artifact{
		ID:             "skill-cooldown",
		Type:           forge.ArtifactSkill,
		Name:           "cooldown-skill",
		Status:         forge.StatusActive,
		ToolSignature:  []string{"read"},
		CreatedAt:      time.Now().UTC().Add(-2 * time.Hour),
		LastDegradedAt: &recentlyDegraded,
	})

	now := time.Now().UTC()
	for i := 0; i < 6; i++ {
		trace := &forge.ConversationTrace{
			TraceID:     "after-" + string(rune(i)),
			StartTime:   now.Add(-time.Duration(i) * time.Minute),
			DurationMs:  50000,
			TotalRounds: 50,
			ToolSteps:   []forge.ToolStep{{ToolName: "read", Success: false}},
			Signals:     []forge.SessionSignal{{Type: "retry"}},
		}
		traceStore.Append(trace)
	}

	monitor := forge.NewDeploymentMonitor(traceStore, registry, cfg)
	monitor.EvaluateOutcomes()

	artifact, found := registry.Get("skill-cooldown")
	if !found {
		t.Fatal("Artifact not found")
	}
	if artifact.Status == forge.StatusDeprecated {
		t.Error("Artifact should NOT be deprecated during cooldown period")
	}
}

func TestMonitorConsecutiveObserving(t *testing.T) {
	tmpDir := t.TempDir()
	forgeDir := filepath.Join(tmpDir, "forge")
	os.MkdirAll(forgeDir, 0755)

	cfg := forge.DefaultForgeConfig()
	cfg.Learning.MinOutcomeSamples = 5
	cfg.Learning.MonitorWindowDays = 30

	traceStore := forge.NewTraceStore(forgeDir, cfg)
	registryPath := filepath.Join(forgeDir, "registry.json")
	registry := forge.NewRegistry(registryPath)

	registry.Add(forge.Artifact{
		ID:                        "skill-observing",
		Type:                      forge.ArtifactSkill,
		Name:                      "observing-skill",
		Status:                    forge.StatusActive,
		ToolSignature:             []string{"read"},
		ConsecutiveObservingRounds: 2,
	})
	artifact, _ := registry.Get("skill-observing")
	deployTime := artifact.CreatedAt

	for i := 0; i < 6; i++ {
		trace := &forge.ConversationTrace{
			TraceID:     "after-" + string(rune(i)),
			StartTime:   deployTime.Add(time.Duration(i+1) * time.Minute),
			DurationMs:  3000,
			TotalRounds: 6,
			ToolSteps:   []forge.ToolStep{{ToolName: "read", Success: true}},
		}
		traceStore.Append(trace)
	}

	for i := 0; i < 8; i++ {
		trace := &forge.ConversationTrace{
			TraceID:     "before-" + string(rune(i)),
			StartTime:   deployTime.Add(-time.Duration(i+1) * time.Minute),
			DurationMs:  2000,
			TotalRounds: 4,
			ToolSteps:   []forge.ToolStep{{ToolName: "read", Success: true}},
		}
		traceStore.Append(trace)
	}

	monitor := forge.NewDeploymentMonitor(traceStore, registry, cfg)
	monitor.EvaluateOutcomes()

	artifact, found := registry.Get("skill-observing")
	if !found {
		t.Fatal("Artifact not found")
	}
	// After 2 consecutive observing rounds + 1 more observing round = 3 total
	// This triggers auto-deprecation via the consecutive observing upgrade
	if artifact.Status != forge.StatusDeprecated {
		t.Errorf("Expected artifact to be deprecated after 3 consecutive observing rounds, got status '%s'", artifact.Status)
	}
}

func TestMonitorNoToolSignature(t *testing.T) {
	tmpDir := t.TempDir()
	forgeDir := filepath.Join(tmpDir, "forge")
	os.MkdirAll(forgeDir, 0755)

	cfg := forge.DefaultForgeConfig()
	traceStore := forge.NewTraceStore(forgeDir, cfg)
	registryPath := filepath.Join(forgeDir, "registry.json")
	registry := forge.NewRegistry(registryPath)

	registry.Add(forge.Artifact{
		ID:     "skill-no-sig",
		Type:   forge.ArtifactSkill,
		Name:   "no-sig-skill",
		Status: forge.StatusActive,
	})

	monitor := forge.NewDeploymentMonitor(traceStore, registry, cfg)
	outcomes := monitor.EvaluateOutcomes()

	if len(outcomes) != 0 {
		t.Errorf("Expected 0 outcomes for artifact without ToolSignature, got %d", len(outcomes))
	}
}

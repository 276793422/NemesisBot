package forge_test

import (
	"context"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/forge"
)

func TestLearningEngineGenerateActions(t *testing.T) {
	cfg := forge.DefaultForgeConfig()
	cfg.Learning.HighConfThreshold = 0.8
	cfg.Learning.MinPatternFrequency = 5

	tmpDir := t.TempDir()
	forgeDir := filepath.Join(tmpDir, "forge")
	os.MkdirAll(forgeDir, 0755)

	registry := forge.NewRegistry(filepath.Join(forgeDir, "registry.json"))
	le := forge.NewLearningEngine(forgeDir, registry, nil, nil, nil, nil, cfg)

	patterns := []*forge.ConversationPattern{
		{
			ID:          "tc-test",
			Type:        forge.PatternToolChain,
			Fingerprint: "fp1",
			Frequency:   15,
			Confidence:  0.9,
			ToolChain:   "read→edit→exec",
		},
		{
			ID:          "tc-low",
			Type:        forge.PatternToolChain,
			Fingerprint: "fp2",
			Frequency:   8,
			Confidence:  0.6,
			ToolChain:   "read→write",
		},
		{
			ID:           "er-test",
			Type:         forge.PatternErrorRecovery,
			Fingerprint:  "fp3",
			Frequency:    7,
			Confidence:   0.85,
			ErrorTool:    "file_read",
			RecoveryTool: "file_edit",
		},
		{
			ID:              "ef-test",
			Type:            forge.PatternEfficiencyIssue,
			Fingerprint:     "fp4",
			Frequency:       6,
			Confidence:      0.5,
			ToolChain:       "slow-chain",
			EfficiencyScore: 0.5,
		},
		{
			ID:          "st-test",
			Type:        forge.PatternSuccessTemplate,
			Fingerprint: "fp5",
			Frequency:   12,
			Confidence:  0.88,
			ToolChain:   "fast→chain",
		},
	}

	actions := le.GenerateActionsForTest(patterns)

	createCount := 0
	suggestCount := 0
	for _, a := range actions {
		if a.Type == forge.ActionCreateSkill {
			createCount++
		}
		if a.Type == forge.ActionSuggestPrompt {
			suggestCount++
		}
	}
	if createCount != 3 {
		t.Errorf("Expected 3 create_skill actions, got %d", createCount)
	}
	if suggestCount != 2 {
		t.Errorf("Expected 2 suggest_prompt actions, got %d", suggestCount)
	}
}

func TestLearningEngineMaxAutoCreates(t *testing.T) {
	cfg := forge.DefaultForgeConfig()
	cfg.Learning.MaxAutoCreates = 2

	tmpDir := t.TempDir()
	forgeDir := filepath.Join(tmpDir, "forge")
	os.MkdirAll(forgeDir, 0755)

	registry := forge.NewRegistry(filepath.Join(forgeDir, "registry.json"))
	traceStore := forge.NewTraceStore(forgeDir, cfg)
	cycleStore := forge.NewCycleStore(forgeDir, cfg)
	monitor := forge.NewDeploymentMonitor(traceStore, registry, cfg)

	le := forge.NewLearningEngine(forgeDir, registry, traceStore, nil, cycleStore, monitor, cfg)

	cycle := le.RunCycle(context.Background(), nil, nil, nil)

	executedOrFailed := 0
	skipped := 0
	for _, a := range cycle.ActionSummary {
		if a.Type == string(forge.ActionCreateSkill) {
			if a.Status == "skipped" {
				skipped++
			} else {
				executedOrFailed++
			}
		}
	}

	if executedOrFailed > cfg.Learning.MaxAutoCreates {
		t.Errorf("MaxAutoCreates exceeded: %d attempted, limit is %d", executedOrFailed, cfg.Learning.MaxAutoCreates)
	}
}

func TestLearningEngineDeduplication(t *testing.T) {
	cfg := forge.DefaultForgeConfig()

	tmpDir := t.TempDir()
	forgeDir := filepath.Join(tmpDir, "forge")
	os.MkdirAll(forgeDir, 0755)

	registry := forge.NewRegistry(filepath.Join(forgeDir, "registry.json"))
	registry.Add(forge.Artifact{
		ID:     "skill-existing-workflow",
		Type:   forge.ArtifactSkill,
		Name:   "existing-workflow",
		Status: forge.StatusActive,
	})

	le := forge.NewLearningEngine(forgeDir, registry, nil, nil, nil, nil, cfg)

	action := &forge.LearningAction{
		ID:        "la-test",
		Type:      forge.ActionCreateSkill,
		Priority:  "high",
		Status:    "pending",
		DraftName: "existing-workflow",
	}

	le.ExecuteCreateSkillForTest(context.Background(), action)
	if action.Status != "skipped" {
		t.Errorf("Expected action to be skipped due to dedup, got '%s'", action.Status)
	}
}

func TestLearningEngineRunCycleBasic(t *testing.T) {
	cfg := forge.DefaultForgeConfig()
	cfg.Learning.MinPatternFrequency = 5

	tmpDir := t.TempDir()
	forgeDir := filepath.Join(tmpDir, "forge")
	os.MkdirAll(forgeDir, 0755)

	registry := forge.NewRegistry(filepath.Join(forgeDir, "registry.json"))
	traceStore := forge.NewTraceStore(forgeDir, cfg)
	cycleStore := forge.NewCycleStore(forgeDir, cfg)
	monitor := forge.NewDeploymentMonitor(traceStore, registry, cfg)

	le := forge.NewLearningEngine(forgeDir, registry, traceStore, nil, cycleStore, monitor, cfg)

	traces := makeToolChainTracesForge("read→edit", 10)

	cycle := le.RunCycle(context.Background(), traces, nil, nil)

	if cycle == nil {
		t.Fatal("Expected non-nil cycle")
	}
	if cycle.PatternsFound == 0 {
		t.Error("Expected patterns to be found")
	}
	if cycle.CompletedAt == nil {
		t.Error("Expected cycle to be completed")
	}
}

func TestGenerateSkillName(t *testing.T) {
	tests := []struct {
		chain    string
		expected string
	}{
		{"read→edit→exec", "read-edit-exec-workflow"},
		{"file_read", "file-read-workflow"},
		{"single", "single-workflow"},
	}
	for _, tc := range tests {
		result := forge.GenerateSkillNameForTest(tc.chain)
		if result != tc.expected {
			t.Errorf("generateSkillName(%q) = %q, want %q", tc.chain, result, tc.expected)
		}
	}
}

func TestLearningEngineAdjustConfidence(t *testing.T) {
	cfg := forge.DefaultForgeConfig()

	tmpDir := t.TempDir()
	forgeDir := filepath.Join(tmpDir, "forge")
	os.MkdirAll(forgeDir, 0755)

	registry := forge.NewRegistry(filepath.Join(forgeDir, "registry.json"))
	registry.Add(forge.Artifact{
		ID:          "skill-adj",
		Type:        forge.ArtifactSkill,
		Name:        "adj-skill",
		SuccessRate: 0.5,
	})

	le := forge.NewLearningEngine(forgeDir, registry, nil, nil, nil, nil, cfg)

	le.AdjustConfidenceForTest([]*forge.ActionOutcome{
		{ArtifactID: "skill-adj", Verdict: "positive"},
	})

	artifact, _ := registry.Get("skill-adj")
	if artifact.SuccessRate < 0.59 || artifact.SuccessRate > 0.61 {
		t.Errorf("Expected success rate ~0.6 after positive feedback, got %f", artifact.SuccessRate)
	}

	le.AdjustConfidenceForTest([]*forge.ActionOutcome{
		{ArtifactID: "skill-adj", Verdict: "negative"},
	})

	artifact, _ = registry.Get("skill-adj")
	if artifact.SuccessRate < 0.39 || artifact.SuccessRate > 0.41 {
		t.Errorf("Expected success rate ~0.4 after negative feedback, got %f", artifact.SuccessRate)
	}
}

// --- Helpers ---

func makeToolChainTracesForge(chain string, count int) []*forge.ConversationTrace {
	now := time.Now().UTC()
	var traces []*forge.ConversationTrace

	tools := splitChain(chain)
	for i := 0; i < count; i++ {
		steps := make([]forge.ToolStep, len(tools))
		for j, tool := range tools {
			steps[j] = forge.ToolStep{
				ToolName: tool,
				Success:  true,
				ArgKeys:  []string{"path"},
			}
		}

		traces = append(traces, &forge.ConversationTrace{
			TraceID:     "trace-" + chain + "-" + string(rune(i)),
			StartTime:   now.Add(-time.Duration(i) * time.Hour),
			DurationMs:  1000,
			TotalRounds: len(tools),
			ToolSteps:   steps,
		})
	}
	return traces
}

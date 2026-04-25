package forge

import (
	"testing"
	"time"
)

// --- generateSkillName tests ---

func TestGenerateSkillName(t *testing.T) {
	tests := []struct {
		input    string
		expected string
	}{
		{"read→edit→exec", "read-edit-exec-workflow"},
		{"tool_a→tool_b", "tool-a-tool-b-workflow"},
		{"single", "single-workflow"},
	}

	for _, tt := range tests {
		result := generateSkillName(tt.input)
		if result != tt.expected {
			t.Errorf("generateSkillName(%q) = %q, expected %q", tt.input, result, tt.expected)
		}
	}
}

func TestGenerateSkillName_LongInput(t *testing.T) {
	input := "very_long_tool_name_that_exceeds_fifty_characters_limit_for_testing"
	result := generateSkillName(input)
	if len(result) > 60 { // 50 + "-workflow" = 59
		t.Errorf("Result should be truncated, got %d chars: %s", len(result), result)
	}
	if result == "" {
		t.Error("Result should not be empty")
	}
}

// --- extractToolSignatureFromChain tests ---

func TestExtractToolSignatureFromChain(t *testing.T) {
	tests := []struct {
		input    string
		expected []string
	}{
		{"read→edit→exec", []string{"read", "edit", "exec"}},
		{"single_tool", []string{"single_tool"}},
		{"Tool chain: a→b→c", []string{"a", "b", "c"}},
	}

	for _, tt := range tests {
		result := extractToolSignatureFromChain(tt.input)
		if len(result) != len(tt.expected) {
			t.Errorf("extractToolSignatureFromChain(%q) = %v, expected %v", tt.input, result, tt.expected)
			continue
		}
		for i, v := range result {
			if v != tt.expected[i] {
				t.Errorf("Result[%d] = %q, expected %q", i, v, tt.expected[i])
			}
		}
	}
}

// --- buildDiagnosis tests ---

func TestBuildDiagnosis_Stage1Failed(t *testing.T) {
	validation := &ArtifactValidation{
		Stage1Static: &StaticValidationResult{
			ValidationStage: ValidationStage{Passed: false, Errors: []string{"Error 1", "Error 2"}},
		},
	}

	result := buildDiagnosis(validation)
	if result == "" {
		t.Error("Diagnosis should not be empty")
	}
}

func TestBuildDiagnosis_AllPassed(t *testing.T) {
	validation := &ArtifactValidation{
		Stage1Static: &StaticValidationResult{
			ValidationStage: ValidationStage{Passed: true},
		},
		Stage2Functional: &FunctionalValidationResult{
			ValidationStage: ValidationStage{Passed: true},
		},
		Stage3Quality: &QualityValidationResult{
			ValidationStage: ValidationStage{Passed: true},
			Score:           85,
			Notes:           "Good quality",
			Dimensions:      map[string]int{"correctness": 90},
		},
	}

	result := buildDiagnosis(validation)
	if result == "" {
		t.Error("Diagnosis should not be empty when stage 3 has data")
	}
}

func TestBuildDiagnosis_Nil(t *testing.T) {
	// buildDiagnosis dereferences validation, so nil causes panic.
	// This is expected behavior — it's only called with non-nil validation.
	// Test that empty validation returns empty string.
	validation := &ArtifactValidation{}
	result := buildDiagnosis(validation)
	if result != "" {
		t.Errorf("Expected empty diagnosis for empty validation, got %q", result)
	}
}

// --- actionToSummary tests ---

func TestActionToSummary(t *testing.T) {
	action := &LearningAction{
		ID:         "la-test",
		Type:       ActionCreateSkill,
		Priority:   "high",
		Status:     "executed",
		ArtifactID: "skill-test",
	}

	summary := actionToSummary(action)
	if summary.ID != "la-test" {
		t.Errorf("Expected ID 'la-test', got '%s'", summary.ID)
	}
	if summary.Type != "create_skill" {
		t.Errorf("Expected Type 'create_skill', got '%s'", summary.Type)
	}
	if summary.Priority != "high" {
		t.Errorf("Expected Priority 'high', got '%s'", summary.Priority)
	}
	if summary.Status != "executed" {
		t.Errorf("Expected Status 'executed', got '%s'", summary.Status)
	}
	if summary.ArtifactID != "skill-test" {
		t.Errorf("Expected ArtifactID 'skill-test', got '%s'", summary.ArtifactID)
	}
}

// --- sortActions tests ---

func TestSortActions_ByPriority(t *testing.T) {
	actions := []*LearningAction{
		{Priority: "low", Confidence: 0.9},
		{Priority: "high", Confidence: 0.7},
		{Priority: "medium", Confidence: 0.8},
	}

	sortActions(actions)

	if actions[0].Priority != "high" {
		t.Errorf("First action should be 'high', got '%s'", actions[0].Priority)
	}
	if actions[1].Priority != "medium" {
		t.Errorf("Second action should be 'medium', got '%s'", actions[1].Priority)
	}
	if actions[2].Priority != "low" {
		t.Errorf("Third action should be 'low', got '%s'", actions[2].Priority)
	}
}

func TestSortActions_ByConfidence(t *testing.T) {
	actions := []*LearningAction{
		{Priority: "high", Confidence: 0.7},
		{Priority: "high", Confidence: 0.9},
		{Priority: "high", Confidence: 0.8},
	}

	sortActions(actions)

	if actions[0].Confidence < actions[1].Confidence {
		t.Error("Should be sorted by confidence descending within same priority")
	}
}

// --- LearningEngine generateActions tests ---

func TestLearningEngine_GenerateActions_HighConfToolChain(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	cfg.Learning.HighConfThreshold = 0.8
	registry := NewRegistry(tmpDir + "/registry.json")
	traceStore := NewTraceStore(tmpDir, cfg)
	pipeline := NewPipeline(registry, cfg)
	cycleStore := NewCycleStore(tmpDir, cfg)
	monitor := NewDeploymentMonitor(traceStore, registry, cfg)

	le := NewLearningEngine(tmpDir, registry, traceStore, pipeline, cycleStore, monitor, cfg)

	patterns := []*ConversationPattern{
		{
			Type:        PatternToolChain,
			ID:          "tc-test",
			ToolChain:   "read→edit",
			Confidence:  0.95,
			Frequency:   15,
			Description: "Tool chain: read→edit",
		},
	}

	actions := le.GenerateActionsForTest(patterns)
	if len(actions) == 0 {
		t.Fatal("Expected at least one action")
	}

	// High confidence + high frequency should create a skill
	foundCreateSkill := false
	for _, a := range actions {
		if a.Type == ActionCreateSkill {
			foundCreateSkill = true
			if a.Priority != "high" {
				t.Errorf("Expected 'high' priority, got '%s'", a.Priority)
			}
		}
	}
	if !foundCreateSkill {
		t.Error("Expected ActionCreateSkill for high-confidence tool chain")
	}
}

func TestLearningEngine_GenerateActions_LowConfToolChain(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	cfg.Learning.HighConfThreshold = 0.8
	registry := NewRegistry(tmpDir + "/registry.json")
	traceStore := NewTraceStore(tmpDir, cfg)
	pipeline := NewPipeline(registry, cfg)
	cycleStore := NewCycleStore(tmpDir, cfg)
	monitor := NewDeploymentMonitor(traceStore, registry, cfg)

	le := NewLearningEngine(tmpDir, registry, traceStore, pipeline, cycleStore, monitor, cfg)

	patterns := []*ConversationPattern{
		{
			Type:       PatternToolChain,
			ID:         "tc-low",
			ToolChain:  "a→b",
			Confidence: 0.5,
			Frequency:  3,
		},
	}

	actions := le.GenerateActionsForTest(patterns)
	if len(actions) == 0 {
		t.Fatal("Expected at least one action")
	}

	// Low confidence should suggest prompt, not create skill
	for _, a := range actions {
		if a.Type == ActionCreateSkill {
			t.Error("Low confidence tool chain should not auto-create skill")
		}
		if a.Type == ActionSuggestPrompt {
			if a.Priority != "medium" {
				t.Errorf("Expected 'medium' priority for suggest, got '%s'", a.Priority)
			}
		}
	}
}

func TestLearningEngine_GenerateActions_ErrorRecovery(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	cfg.Learning.HighConfThreshold = 0.8
	registry := NewRegistry(tmpDir + "/registry.json")
	traceStore := NewTraceStore(tmpDir, cfg)
	pipeline := NewPipeline(registry, cfg)
	cycleStore := NewCycleStore(tmpDir, cfg)
	monitor := NewDeploymentMonitor(traceStore, registry, cfg)

	le := NewLearningEngine(tmpDir, registry, traceStore, pipeline, cycleStore, monitor, cfg)

	patterns := []*ConversationPattern{
		{
			Type:         PatternErrorRecovery,
			ID:           "er-test",
			Confidence:   0.9,
			ErrorTool:    "exec",
			RecoveryTool: "read_file",
		},
	}

	actions := le.GenerateActionsForTest(patterns)
	if len(actions) == 0 {
		t.Fatal("Expected at least one action for error recovery")
	}

	if actions[0].Type != ActionCreateSkill {
		t.Errorf("High confidence error recovery should create skill, got %s", actions[0].Type)
	}
}

func TestLearningEngine_GenerateActions_EfficiencyIssue(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(tmpDir + "/registry.json")
	traceStore := NewTraceStore(tmpDir, cfg)
	pipeline := NewPipeline(registry, cfg)
	cycleStore := NewCycleStore(tmpDir, cfg)
	monitor := NewDeploymentMonitor(traceStore, registry, cfg)

	le := NewLearningEngine(tmpDir, registry, traceStore, pipeline, cycleStore, monitor, cfg)

	patterns := []*ConversationPattern{
		{
			Type:            PatternEfficiencyIssue,
			ID:              "ef-test",
			ToolChain:       "a→b",
			Confidence:      0.5,
			EfficiencyScore: 0.3,
		},
	}

	actions := le.GenerateActionsForTest(patterns)
	if len(actions) == 0 {
		t.Fatal("Expected at least one action for efficiency issue")
	}

	if actions[0].Type != ActionSuggestPrompt {
		t.Errorf("Efficiency issue should suggest prompt, got %s", actions[0].Type)
	}
}

func TestLearningEngine_GenerateActions_SuccessTemplate(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	cfg.Learning.HighConfThreshold = 0.8
	registry := NewRegistry(tmpDir + "/registry.json")
	traceStore := NewTraceStore(tmpDir, cfg)
	pipeline := NewPipeline(registry, cfg)
	cycleStore := NewCycleStore(tmpDir, cfg)
	monitor := NewDeploymentMonitor(traceStore, registry, cfg)

	le := NewLearningEngine(tmpDir, registry, traceStore, pipeline, cycleStore, monitor, cfg)

	// High confidence success template
	patterns := []*ConversationPattern{
		{
			Type:       PatternSuccessTemplate,
			ID:         "st-test",
			ToolChain:  "fast→path",
			Confidence: 0.95,
		},
	}

	actions := le.GenerateActionsForTest(patterns)
	foundCreateSkill := false
	for _, a := range actions {
		if a.Type == ActionCreateSkill && a.Priority == "high" {
			foundCreateSkill = true
		}
	}
	if !foundCreateSkill {
		t.Error("High confidence success template should create skill")
	}
}

// --- LearningEngine adjustConfidence tests ---

func TestLearningEngine_AdjustConfidence_Positive(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(tmpDir + "/registry.json")
	registry.Add(Artifact{
		ID:          "skill-test",
		SuccessRate: 0.7,
	})
	traceStore := NewTraceStore(tmpDir, cfg)
	pipeline := NewPipeline(registry, cfg)
	cycleStore := NewCycleStore(tmpDir, cfg)
	monitor := NewDeploymentMonitor(traceStore, registry, cfg)

	le := NewLearningEngine(tmpDir, registry, traceStore, pipeline, cycleStore, monitor, cfg)

	outcomes := []*ActionOutcome{
		{ArtifactID: "skill-test", Verdict: "positive"},
	}

	le.AdjustConfidenceForTest(outcomes)

	artifact, _ := registry.Get("skill-test")
	if artifact.SuccessRate < 0.79 || artifact.SuccessRate > 0.81 {
		t.Errorf("Expected SuccessRate ~0.8 (0.7 + 0.1), got %f", artifact.SuccessRate)
	}
}

func TestLearningEngine_AdjustConfidence_Negative(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(tmpDir + "/registry.json")
	registry.Add(Artifact{
		ID:          "skill-test",
		SuccessRate: 0.7,
	})
	traceStore := NewTraceStore(tmpDir, cfg)
	pipeline := NewPipeline(registry, cfg)
	cycleStore := NewCycleStore(tmpDir, cfg)
	monitor := NewDeploymentMonitor(traceStore, registry, cfg)

	le := NewLearningEngine(tmpDir, registry, traceStore, pipeline, cycleStore, monitor, cfg)

	outcomes := []*ActionOutcome{
		{ArtifactID: "skill-test", Verdict: "negative"},
	}

	le.AdjustConfidenceForTest(outcomes)

	artifact, _ := registry.Get("skill-test")
	if artifact.SuccessRate < 0.49 || artifact.SuccessRate > 0.51 {
		t.Errorf("Expected SuccessRate ~0.5 (0.7 - 0.2), got %f", artifact.SuccessRate)
	}
}

func TestLearningEngine_AdjustConfidence_ClampToZero(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(tmpDir + "/registry.json")
	registry.Add(Artifact{
		ID:          "skill-test",
		SuccessRate: 0.1,
	})
	traceStore := NewTraceStore(tmpDir, cfg)
	pipeline := NewPipeline(registry, cfg)
	cycleStore := NewCycleStore(tmpDir, cfg)
	monitor := NewDeploymentMonitor(traceStore, registry, cfg)

	le := NewLearningEngine(tmpDir, registry, traceStore, pipeline, cycleStore, monitor, cfg)

	outcomes := []*ActionOutcome{
		{ArtifactID: "skill-test", Verdict: "negative"},
	}

	le.AdjustConfidenceForTest(outcomes)

	artifact, _ := registry.Get("skill-test")
	if artifact.SuccessRate < 0 {
		t.Errorf("SuccessRate should be clamped to >= 0, got %f", artifact.SuccessRate)
	}
	if artifact.SuccessRate > 0.01 {
		t.Errorf("Expected SuccessRate ~0.0, got %f", artifact.SuccessRate)
	}
}

func TestLearningEngine_AdjustConfidence_ClampToOne(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(tmpDir + "/registry.json")
	registry.Add(Artifact{
		ID:          "skill-test",
		SuccessRate: 0.95,
	})
	traceStore := NewTraceStore(tmpDir, cfg)
	pipeline := NewPipeline(registry, cfg)
	cycleStore := NewCycleStore(tmpDir, cfg)
	monitor := NewDeploymentMonitor(traceStore, registry, cfg)

	le := NewLearningEngine(tmpDir, registry, traceStore, pipeline, cycleStore, monitor, cfg)

	// Two positive outcomes
	outcomes := []*ActionOutcome{
		{ArtifactID: "skill-test", Verdict: "positive"},
		{ArtifactID: "skill-test", Verdict: "positive"},
	}

	le.AdjustConfidenceForTest(outcomes)

	artifact, _ := registry.Get("skill-test")
	if artifact.SuccessRate > 1.0 {
		t.Errorf("SuccessRate should be clamped to <= 1.0, got %f", artifact.SuccessRate)
	}
}

func TestLearningEngine_AdjustConfidence_Neutral(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(tmpDir + "/registry.json")
	registry.Add(Artifact{
		ID:          "skill-test",
		SuccessRate: 0.7,
	})
	traceStore := NewTraceStore(tmpDir, cfg)
	pipeline := NewPipeline(registry, cfg)
	cycleStore := NewCycleStore(tmpDir, cfg)
	monitor := NewDeploymentMonitor(traceStore, registry, cfg)

	le := NewLearningEngine(tmpDir, registry, traceStore, pipeline, cycleStore, monitor, cfg)

	outcomes := []*ActionOutcome{
		{ArtifactID: "skill-test", Verdict: "neutral"},
	}

	le.AdjustConfidenceForTest(outcomes)

	artifact, _ := registry.Get("skill-test")
	if artifact.SuccessRate != 0.7 {
		t.Errorf("Neutral verdict should not change rate, expected 0.7, got %f", artifact.SuccessRate)
	}
}

// --- LearningEngine findArtifactByFingerprint tests ---

func TestLearningEngine_FindArtifactByFingerprint(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(tmpDir + "/registry.json")
	registry.Add(Artifact{
		ID:     "skill-existing",
		Name:   "existing-skill",
		Status: StatusActive,
	})
	registry.Add(Artifact{
		ID:     "skill-deprecated",
		Name:   "deprecated-skill",
		Status: StatusDeprecated,
	})
	traceStore := NewTraceStore(tmpDir, cfg)
	pipeline := NewPipeline(registry, cfg)
	cycleStore := NewCycleStore(tmpDir, cfg)
	monitor := NewDeploymentMonitor(traceStore, registry, cfg)

	le := NewLearningEngine(tmpDir, registry, traceStore, pipeline, cycleStore, monitor, cfg)

	// Should find active artifact
	found := le.findArtifactByFingerprint("existing-skill")
	if found == nil {
		t.Error("Should find active artifact")
	}

	// Should NOT find deprecated artifact
	found = le.findArtifactByFingerprint("deprecated-skill")
	if found != nil {
		t.Error("Should not find deprecated artifact")
	}

	// Non-existent
	found = le.findArtifactByFingerprint("nonexistent")
	if found != nil {
		t.Error("Should not find non-existent artifact")
	}
}

// --- LearningEngine executeSuggestPrompt tests ---

func TestLearningEngine_ExecuteSuggestPrompt(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(tmpDir + "/registry.json")
	traceStore := NewTraceStore(tmpDir, cfg)
	pipeline := NewPipeline(registry, cfg)
	cycleStore := NewCycleStore(tmpDir, cfg)
	monitor := NewDeploymentMonitor(traceStore, registry, cfg)

	le := NewLearningEngine(tmpDir, registry, traceStore, pipeline, cycleStore, monitor, cfg)

	action := &LearningAction{
		ID:          "la-suggest",
		Type:        ActionSuggestPrompt,
		Priority:    "medium",
		Confidence:  0.6,
		DraftName:   "test-workflow",
		Description: "Test pattern",
		Rationale:   "Below threshold",
		CreatedAt:   time.Now().UTC(),
	}

	le.ExecuteSuggestPromptForTest(action)

	if action.Status != "executed" {
		t.Errorf("Expected status 'executed', got '%s'", action.Status)
	}
	if action.ExecutedAt == nil {
		t.Error("ExecutedAt should be set")
	}
}

// --- LearningEngine GetLatestCycle tests ---

func TestLearningEngine_GetLatestCycle_NoCycleStore(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(tmpDir + "/registry.json")
	traceStore := NewTraceStore(tmpDir, cfg)
	pipeline := NewPipeline(registry, cfg)

	le := NewLearningEngine(tmpDir, registry, traceStore, pipeline, nil, nil, cfg)

	result := le.GetLatestCycle()
	if result != nil {
		t.Error("Should return nil when cycleStore is nil")
	}
}

// --- ActionType constants ---

func TestActionTypeConstants(t *testing.T) {
	if ActionCreateSkill != "create_skill" {
		t.Errorf("Expected 'create_skill', got '%s'", ActionCreateSkill)
	}
	if ActionSuggestPrompt != "suggest_prompt" {
		t.Errorf("Expected 'suggest_prompt', got '%s'", ActionSuggestPrompt)
	}
	if ActionDeprecate != "deprecate_artifact" {
		t.Errorf("Expected 'deprecate_artifact', got '%s'", ActionDeprecate)
	}
}

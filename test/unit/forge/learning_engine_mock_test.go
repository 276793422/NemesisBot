package forge_test

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/forge"
	"github.com/276793422/NemesisBot/module/providers"
	"github.com/276793422/NemesisBot/module/tools"
)

// ============================================================
// Mock LLM Provider for Phase 6 tests
// ============================================================

type p6MockLLM struct {
	defaultModel string
	chatFunc     func(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error)
	callCount    int
	lastMessages []providers.Message
}

func (m *p6MockLLM) Chat(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
	m.callCount++
	m.lastMessages = messages
	if m.chatFunc != nil {
		return m.chatFunc(ctx, messages, tools, model, options)
	}
	return &providers.LLMResponse{Content: "---\nname: mock-skill\n---\nMock content", FinishReason: "stop"}, nil
}

func (m *p6MockLLM) GetDefaultModel() string {
	if m.defaultModel != "" {
		return m.defaultModel
	}
	return "mock-llm-v1"
}

// ============================================================
// Helper: create a fully wired LearningEngine for testing
// ============================================================

func newTestLearningEngine(t *testing.T) (*forge.LearningEngine, string) {
	t.Helper()
	tmpDir := t.TempDir()
	forgeDir := filepath.Join(tmpDir, "forge")
	os.MkdirAll(forgeDir, 0755)

	cfg := forge.DefaultForgeConfig()
	cfg.Learning.MinPatternFrequency = 5
	cfg.Learning.HighConfThreshold = 0.8
	cfg.Learning.MaxAutoCreates = 3
	cfg.Learning.MaxRefineRounds = 3
	cfg.Learning.MinOutcomeSamples = 5
	cfg.Learning.MonitorWindowDays = 30

	registry := forge.NewRegistry(filepath.Join(forgeDir, "registry.json"))
	traceStore := forge.NewTraceStore(forgeDir, cfg)
	cycleStore := forge.NewCycleStore(forgeDir, cfg)
	monitor := forge.NewDeploymentMonitor(traceStore, registry, cfg)

	le := forge.NewLearningEngine(forgeDir, registry, traceStore, nil, cycleStore, monitor, cfg)
	return le, forgeDir
}

// ============================================================
// Test: BuildDiagnosis
// ============================================================

func TestBuildDiagnosis_Stage1Failed(t *testing.T) {
	validation := &forge.ArtifactValidation{
		Stage1Static: &forge.StaticValidationResult{
			ValidationStage: forge.ValidationStage{
				Passed: false,
				Errors: []string{"Missing frontmatter", "No description"},
			},
		},
	}
	diagnosis := forge.BuildDiagnosisForTest(validation)
	if !strings.Contains(diagnosis, "Stage 1 (Static) FAILED") {
		t.Error("Should mention Stage 1 failure")
	}
	if !strings.Contains(diagnosis, "Missing frontmatter") {
		t.Error("Should include error details")
	}
}

func TestBuildDiagnosis_Stage3Quality(t *testing.T) {
	validation := &forge.ArtifactValidation{
		Stage1Static:     &forge.StaticValidationResult{ValidationStage: forge.ValidationStage{Passed: true}},
		Stage2Functional: &forge.FunctionalValidationResult{ValidationStage: forge.ValidationStage{Passed: true}},
		Stage3Quality: &forge.QualityValidationResult{
			ValidationStage: forge.ValidationStage{Passed: false},
			Score:           45,
			Notes:           "Poor structure",
			Dimensions:      map[string]int{"clarity": 40, "completeness": 50},
		},
	}
	diagnosis := forge.BuildDiagnosisForTest(validation)
	if !strings.Contains(diagnosis, "Stage 3 (Quality) Score: 45/100") {
		t.Error("Should include quality score")
	}
	if !strings.Contains(diagnosis, "clarity: 40") {
		t.Error("Should include dimension scores")
	}
}

func TestBuildDiagnosis_Empty(t *testing.T) {
	validation := &forge.ArtifactValidation{}
	diagnosis := forge.BuildDiagnosisForTest(validation)
	if diagnosis != "" {
		t.Errorf("Empty validation should produce empty diagnosis, got: %s", diagnosis)
	}
}

// ============================================================
// Test: ExtractToolSignatureFromChain
// ============================================================

func TestExtractToolSignatureFromChain(t *testing.T) {
	tests := []struct {
		input    string
		expected []string
	}{
		{"read→edit→exec", []string{"read", "edit", "exec"}},
		{"single", []string{"single"}},
		{"file_read", []string{"file_read"}},
		{"Tool chain: read→edit", []string{"read", "edit"}},
	}
	for _, tc := range tests {
		result := forge.ExtractToolSignatureFromChainForTest(tc.input)
		if len(result) != len(tc.expected) {
			t.Errorf("extractToolSignature(%q): expected %v, got %v", tc.input, tc.expected, result)
			continue
		}
		for i, v := range result {
			if v != tc.expected[i] {
				t.Errorf("extractToolSignature(%q)[%d]: expected %q, got %q", tc.input, i, tc.expected[i], v)
			}
		}
	}
}

// ============================================================
// Test: executeCreateSkill — No LLM provider
// ============================================================

func TestExecuteCreateSkill_NoProvider(t *testing.T) {
	le, _ := newTestLearningEngine(t)

	action := &forge.LearningAction{
		ID:        "la-noprovider",
		Type:      forge.ActionCreateSkill,
		Priority:  "high",
		Status:    "pending",
		DraftName: "test-skill",
	}

	le.ExecuteCreateSkillForTest(context.Background(), action)
	if action.Status != "failed" {
		t.Errorf("Expected status 'failed', got '%s'", action.Status)
	}
	if !strings.Contains(action.ErrorMsg, "No LLM provider") {
		t.Errorf("Expected 'No LLM provider' error, got: %s", action.ErrorMsg)
	}
}

// ============================================================
// Test: executeCreateSkill — LLM generation failure
// ============================================================

func TestExecuteCreateSkill_LLMGenerationFails(t *testing.T) {
	le, _ := newTestLearningEngine(t)

	// Inject failing LLM
	le.SetProvider(&p6MockLLM{
		chatFunc: func(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
			return nil, context.DeadlineExceeded
		},
	})

	action := &forge.LearningAction{
		ID:        "la-llm-fail",
		Type:      forge.ActionCreateSkill,
		Priority:  "high",
		Status:    "pending",
		DraftName: "test-skill-fail",
	}

	le.ExecuteCreateSkillForTest(context.Background(), action)
	if action.Status != "failed" {
		t.Errorf("Expected status 'failed', got '%s'", action.Status)
	}
	if !strings.Contains(action.ErrorMsg, "LLM generation failed") {
		t.Errorf("Expected LLM generation error, got: %s", action.ErrorMsg)
	}
}

// ============================================================
// Test: executeCreateSkill — Dedup (artifact already exists)
// ============================================================

func TestExecuteCreateSkill_Dedup(t *testing.T) {
	le, _ := newTestLearningEngine(t)

	// Pre-add an artifact with the same name
	le.SetProvider(&p6MockLLM{})

	// Access registry through a separate test — but we need registry access
	// The LearningEngine has internal registry. We can add through a Forge instance.
	// Instead, let's use the test export pattern differently.
	// Since we can't access the registry directly from test, we create a Forge with registry.
	tmpDir := t.TempDir()
	forgeDir := filepath.Join(tmpDir, "forge")
	os.MkdirAll(forgeDir, 0755)

	cfg := forge.DefaultForgeConfig()
	registry := forge.NewRegistry(filepath.Join(forgeDir, "registry.json"))
	registry.Add(forge.Artifact{
		ID:     "skill-existing-test",
		Type:   forge.ArtifactSkill,
		Name:   "existing-test",
		Status: forge.StatusActive,
	})

	traceStore := forge.NewTraceStore(forgeDir, cfg)
	cycleStore := forge.NewCycleStore(forgeDir, cfg)
	monitor := forge.NewDeploymentMonitor(traceStore, registry, cfg)
	le2 := forge.NewLearningEngine(forgeDir, registry, traceStore, nil, cycleStore, monitor, cfg)

	action := &forge.LearningAction{
		ID:        "la-dedup",
		Type:      forge.ActionCreateSkill,
		Priority:  "high",
		Status:    "pending",
		DraftName: "existing-test",
	}

	le2.ExecuteCreateSkillForTest(context.Background(), action)
	if action.Status != "skipped" {
		t.Errorf("Expected status 'skipped' for dedup, got '%s'", action.Status)
	}
}

// ============================================================
// Test: executeCreateSkill — LLM generates, Pipeline passes, Forge deploys
// ============================================================

func TestExecuteCreateSkill_LLMGenerateAndDeploy(t *testing.T) {
	tmpDir := t.TempDir()
	forgeDir := filepath.Join(tmpDir, "forge")
	os.MkdirAll(forgeDir, 0755)
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(filepath.Join(workspace, "skills"), 0755)

	cfg := forge.DefaultForgeConfig()
	cfg.Learning.MaxRefineRounds = 3

	registry := forge.NewRegistry(filepath.Join(forgeDir, "registry.json"))
	traceStore := forge.NewTraceStore(forgeDir, cfg)
	cycleStore := forge.NewCycleStore(forgeDir, cfg)
	monitor := forge.NewDeploymentMonitor(traceStore, registry, cfg)
	pipeline := forge.NewPipeline(registry, cfg)

	le := forge.NewLearningEngine(forgeDir, registry, traceStore, pipeline, cycleStore, monitor, cfg)

	// Create Forge instance for CreateSkill shared method
	forgeInstance, _ := forge.NewForge(workspace, nil)
	le.SetForge(forgeInstance)

	// Inject mock LLM that returns valid SKILL.md
	mockLLM := &p6MockLLM{
		chatFunc: func(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
			return &providers.LLMResponse{
				Content:      "---\nname: test-auto-skill\ndescription: Auto-generated\n---\n# Test Skill\nSteps here.",
				FinishReason: "stop",
			}, nil
		},
	}
	le.SetProvider(mockLLM)

	action := &forge.LearningAction{
		ID:          "la-deploy",
		Type:        forge.ActionCreateSkill,
		Priority:    "high",
		Status:      "pending",
		DraftName:   "test-auto-skill",
		Description: "read→edit",
	}

	le.ExecuteCreateSkillForTest(context.Background(), action)

	// Should have called LLM at least once
	if mockLLM.callCount < 1 {
		t.Error("Expected at least 1 LLM call")
	}
	// Verify the LLM was called with skill generation prompt
	if len(mockLLM.lastMessages) < 2 {
		t.Error("Expected at least 2 messages (system + user)")
	}
	if mockLLM.lastMessages[0].Role != "system" {
		t.Error("First message should be system prompt")
	}
	if !strings.Contains(mockLLM.lastMessages[1].Content, "test-auto-skill") {
		t.Error("User prompt should contain skill name")
	}
}

// ============================================================
// Test: executeCreateSkill — Iterative refinement loop
// ============================================================

func TestExecuteCreateSkill_IterativeRefinement(t *testing.T) {
	tmpDir := t.TempDir()
	forgeDir := filepath.Join(tmpDir, "forge")
	os.MkdirAll(forgeDir, 0755)
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(filepath.Join(workspace, "skills"), 0755)

	cfg := forge.DefaultForgeConfig()
	cfg.Learning.MaxRefineRounds = 2

	registry := forge.NewRegistry(filepath.Join(forgeDir, "registry.json"))
	traceStore := forge.NewTraceStore(forgeDir, cfg)
	cycleStore := forge.NewCycleStore(forgeDir, cfg)
	monitor := forge.NewDeploymentMonitor(traceStore, registry, cfg)
	pipeline := forge.NewPipeline(registry, cfg)

	le := forge.NewLearningEngine(forgeDir, registry, traceStore, pipeline, cycleStore, monitor, cfg)

	forgeInstance, _ := forge.NewForge(workspace, nil)
	le.SetForge(forgeInstance)

	callNum := 0
	// First call: generate, fails validation
	// Second call: refine, still fails
	// Third call (if MaxRefineRounds=2): refine again
	mockLLM := &p6MockLLM{
		chatFunc: func(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
			callNum++
			// Always return content that will fail static validation (no frontmatter)
			return &providers.LLMResponse{
				Content:      "Invalid content without frontmatter",
				FinishReason: "stop",
			}, nil
		},
	}
	le.SetProvider(mockLLM)

	action := &forge.LearningAction{
		ID:          "la-refine",
		Type:        forge.ActionCreateSkill,
		Priority:    "high",
		Status:      "pending",
		DraftName:   "refine-skill",
		Description: "read→edit",
	}

	le.ExecuteCreateSkillForTest(context.Background(), action)

	if action.Status != "failed" {
		t.Errorf("Expected 'failed' after refinement exhaustion, got '%s'", action.Status)
	}
	if !strings.Contains(action.ErrorMsg, "refinement rounds") {
		t.Errorf("Expected refinement failure message, got: %s", action.ErrorMsg)
	}

	// Should have called LLM: 1 initial + MaxRefineRounds refinement attempts = 3 total
	// But the pipeline may short-circuit at Stage 1, so refinement happens
	// Initial generation (1) + refine attempts (up to MaxRefineRounds)
	expectedCalls := 1 + cfg.Learning.MaxRefineRounds
	if mockLLM.callCount != expectedCalls {
		t.Errorf("Expected %d LLM calls (1 gen + %d refine), got %d", expectedCalls, cfg.Learning.MaxRefineRounds, mockLLM.callCount)
	}
}

// ============================================================
// Test: executeCreateSkill — Refinement loop calls LLM multiple times
// ============================================================

func TestExecuteCreateSkill_RefinementLoopCalls(t *testing.T) {
	tmpDir := t.TempDir()
	forgeDir := filepath.Join(tmpDir, "forge")
	os.MkdirAll(forgeDir, 0755)
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(filepath.Join(workspace, "skills"), 0755)

	cfg := forge.DefaultForgeConfig()
	cfg.Learning.MaxRefineRounds = 2

	registry := forge.NewRegistry(filepath.Join(forgeDir, "registry.json"))
	traceStore := forge.NewTraceStore(forgeDir, cfg)
	cycleStore := forge.NewCycleStore(forgeDir, cfg)
	monitor := forge.NewDeploymentMonitor(traceStore, registry, cfg)
	pipeline := forge.NewPipeline(registry, cfg)

	le := forge.NewLearningEngine(forgeDir, registry, traceStore, pipeline, cycleStore, monitor, cfg)

	forgeInstance, _ := forge.NewForge(workspace, nil)
	le.SetForge(forgeInstance)

	// Track LLM calls and verify refinement messages
	callNum := 0
	var secondPrompt string
	mockLLM := &p6MockLLM{
		chatFunc: func(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
			callNum++
			if callNum == 2 {
				// Capture the refinement prompt to verify it contains diagnosis
				secondPrompt = messages[1].Content
			}
			// Always return content that will fail static validation (no frontmatter)
			return &providers.LLMResponse{
				Content:      "Invalid content without frontmatter",
				FinishReason: "stop",
			}, nil
		},
	}
	le.SetProvider(mockLLM)

	action := &forge.LearningAction{
		ID:          "la-refine",
		Type:        forge.ActionCreateSkill,
		Priority:    "high",
		Status:      "pending",
		DraftName:   "refine-skill",
		Description: "read→edit",
	}

	le.ExecuteCreateSkillForTest(context.Background(), action)

	// Should have called LLM: 1 initial + MaxRefineRounds refinement = 3 total
	expectedCalls := 1 + cfg.Learning.MaxRefineRounds
	if mockLLM.callCount != expectedCalls {
		t.Errorf("Expected %d LLM calls (1 gen + %d refine), got %d", expectedCalls, cfg.Learning.MaxRefineRounds, mockLLM.callCount)
	}

	// Verify the refinement prompt includes diagnosis feedback
	if secondPrompt == "" {
		t.Fatal("Expected second LLM prompt to be captured")
	}
	if !strings.Contains(secondPrompt, "failed validation") || !strings.Contains(secondPrompt, "Skill Name") {
		t.Errorf("Refinement prompt should contain diagnosis info, got: %s", secondPrompt[:min(200, len(secondPrompt))])
	}

	// Action should be failed after exhaustion
	if action.Status != "failed" {
		t.Errorf("Expected 'failed' after refinement exhaustion, got '%s'", action.Status)
	}
	if !strings.Contains(action.ErrorMsg, "refinement rounds") {
		t.Errorf("Expected refinement failure message, got: %s", action.ErrorMsg)
	}
}

func min(a, b int) int {
	if a < b {
		return a
	}
	return b
}

// ============================================================
// Test: executeSuggestPrompt — writes suggestion file
// ============================================================

func TestExecuteSuggestPrompt_WritesFile(t *testing.T) {
	le, forgeDir := newTestLearningEngine(t)

	action := &forge.LearningAction{
		ID:          "la-suggest",
		Type:        forge.ActionSuggestPrompt,
		Priority:    "medium",
		Status:      "pending",
		DraftName:   "test-suggestion",
		Description: "A pattern detected",
		Rationale:   "Below threshold confidence",
		Confidence:  0.6,
	}

	le.ExecuteSuggestPromptForTest(action)

	if action.Status != "executed" {
		t.Errorf("Expected status 'executed', got '%s'", action.Status)
	}

	// Verify the file was written
	promptsDir := filepath.Join(filepath.Dir(forgeDir), "prompts")
	entries, err := os.ReadDir(promptsDir)
	if err != nil {
		t.Fatalf("Failed to read prompts dir: %v", err)
	}
	if len(entries) == 0 {
		t.Fatal("Expected at least 1 suggestion file")
	}

	// Verify file content
	found := false
	for _, e := range entries {
		if strings.HasSuffix(e.Name(), "_suggestion.md") {
			found = true
			content, err := os.ReadFile(filepath.Join(promptsDir, e.Name()))
			if err != nil {
				t.Fatalf("Failed to read suggestion file: %v", err)
			}
			s := string(content)
			if !strings.Contains(s, "test-suggestion") {
				t.Error("Suggestion file should contain the draft name")
			}
			if !strings.Contains(s, "0.60") {
				t.Error("Suggestion file should contain confidence value")
			}
		}
	}
	if !found {
		t.Error("Expected a _suggestion.md file")
	}
}

// ============================================================
// Test: executeSuggestPrompt — sanitize filename
// ============================================================

func TestExecuteSuggestPrompt_SanitizeFilename(t *testing.T) {
	le, _ := newTestLearningEngine(t)

	action := &forge.LearningAction{
		ID:          "la-special-chars",
		Type:        forge.ActionSuggestPrompt,
		Priority:    "medium",
		Status:      "pending",
		DraftName:   "read→edit→exec Workflow",
		Description: "test",
		Rationale:   "test",
	}

	le.ExecuteSuggestPromptForTest(action)
	if action.Status != "executed" {
		t.Errorf("Expected 'executed', got '%s'", action.Status)
	}
}

// ============================================================
// Test: RunCycle with mock LLM — full integration
// ============================================================

func TestRunCycle_FullIntegrationWithMockLLM(t *testing.T) {
	tmpDir := t.TempDir()
	forgeDir := filepath.Join(tmpDir, "forge")
	os.MkdirAll(forgeDir, 0755)
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(filepath.Join(workspace, "skills"), 0755)

	cfg := forge.DefaultForgeConfig()
	cfg.Learning.MinPatternFrequency = 5
	cfg.Learning.HighConfThreshold = 0.8
	cfg.Learning.MaxAutoCreates = 3
	cfg.Learning.MonitorWindowDays = 30

	registry := forge.NewRegistry(filepath.Join(forgeDir, "registry.json"))
	traceStore := forge.NewTraceStore(forgeDir, cfg)
	cycleStore := forge.NewCycleStore(forgeDir, cfg)
	monitor := forge.NewDeploymentMonitor(traceStore, registry, cfg)
	pipeline := forge.NewPipeline(registry, cfg)

	le := forge.NewLearningEngine(forgeDir, registry, traceStore, pipeline, cycleStore, monitor, cfg)

	forgeInstance, _ := forge.NewForge(workspace, nil)
	le.SetForge(forgeInstance)

	// Track LLM calls
	llmCallCount := 0
	le.SetProvider(&p6MockLLM{
		chatFunc: func(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
			llmCallCount++
			return &providers.LLMResponse{
				Content:      "Invalid - no frontmatter", // Will fail validation
				FinishReason: "stop",
			}, nil
		},
	})

	// Create traces that trigger tool_chain + suggest_prompt (below threshold)
	traces := makeToolChainTracesForge("read→write", 6)

	cycle := le.RunCycle(context.Background(), traces, nil, nil)

	if cycle == nil {
		t.Fatal("Expected non-nil cycle")
	}
	if cycle.PatternsFound == 0 {
		t.Error("Expected patterns to be found")
	}
	if cycle.ActionsCreated == 0 {
		t.Error("Expected actions to be created")
	}
	if cycle.CompletedAt == nil {
		t.Error("Expected cycle to be completed")
	}

	// Verify cycle was saved
	savedCycles, err := cycleStore.ReadCycles(time.Now().UTC().AddDate(0, 0, -1))
	if err != nil || len(savedCycles) == 0 {
		t.Error("Expected cycle to be saved to CycleStore")
	}
}

// ============================================================
// Test: RunCycle with no traces — empty cycle
// ============================================================

func TestRunCycle_EmptyTraces(t *testing.T) {
	le, _ := newTestLearningEngine(t)

	cycle := le.RunCycle(context.Background(), nil, nil, nil)

	if cycle == nil {
		t.Fatal("Expected non-nil cycle even with no traces")
	}
	if cycle.PatternsFound != 0 {
		t.Error("Expected 0 patterns with no traces")
	}
	if cycle.ActionsCreated != 0 {
		t.Error("Expected 0 actions with no traces")
	}
}

// ============================================================
// Test: Reflector + LearningEngine integration (Stage 1.7)
// ============================================================

func TestReflectorWithLearningEngine_Stage17(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	cfg.Reflection.MinExperiences = 1
	cfg.Learning.MinPatternFrequency = 5
	cfg.Learning.MonitorWindowDays = 30

	store := forge.NewExperienceStore(tmpDir, cfg)
	registry := forge.NewRegistry(filepath.Join(tmpDir, "registry.json"))
	traceStore := forge.NewTraceStore(tmpDir, cfg)
	cycleStore := forge.NewCycleStore(tmpDir, cfg)
	monitor := forge.NewDeploymentMonitor(traceStore, registry, cfg)

	reflector := forge.NewReflector(tmpDir, store, registry, cfg)

	le := forge.NewLearningEngine(tmpDir, registry, traceStore, nil, cycleStore, monitor, cfg)

	// Track LLM calls for the semantic analysis
	semLLMCallCount := 0
	mockProvider := &p6MockLLM{
		chatFunc: func(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
			semLLMCallCount++
			return &providers.LLMResponse{
				Content:      "分析结果: 系统运行正常",
				FinishReason: "stop",
			}, nil
		},
	}
	reflector.SetProvider(mockProvider)
	le.SetProvider(mockProvider)
	reflector.SetLearningEngine(le)

	// Seed experience data
	store.AppendAggregated(&forge.AggregatedExperience{
		PatternHash: "sha256:stage17",
		ToolName:    "read_file",
		Count:       5,
		LastSeen:    time.Now().UTC(),
	})

	// Run reflection with Stage 1.7
	reportPath, err := reflector.Reflect(context.Background(), "today", "all")
	if err != nil {
		t.Fatalf("Reflect failed: %v", err)
	}

	content, _ := os.ReadFile(reportPath)
	reportStr := string(content)

	// Report should contain statistical section
	if !strings.Contains(reportStr, "统计概要") {
		t.Error("Report should contain statistical section")
	}

	// LLM should have been called for semantic analysis
	if semLLMCallCount == 0 {
		t.Error("Expected at least 1 LLM call for semantic analysis")
	}
}

// ============================================================
// Test: formatLearningInsights
// ============================================================

func TestFormatLearningInsights_FullCycle(t *testing.T) {
	now := time.Now().UTC()
	cycle := &forge.LearningCycle{
		ID:               "lc-report-test",
		StartedAt:        now,
		CompletedAt:      &now,
		PatternsFound:    3,
		ActionsCreated:   2,
		ActionsExecuted:  1,
		ActionsSkipped:   1,
		PatternSummary: []forge.PatternSummary{
			{ID: "p1", Type: "tool_chain", Fingerprint: "abc123def456", Frequency: 15, Confidence: 0.92},
			{ID: "p2", Type: "error_recovery", Fingerprint: "def789abc012", Frequency: 8, Confidence: 0.75},
		},
		ActionSummary: []forge.ActionSummary{
			{ID: "a1", Type: "create_skill", Priority: "high", Status: "executed", ArtifactID: "skill-read-edit"},
			{ID: "a2", Type: "suggest_prompt", Priority: "medium", Status: "skipped"},
		},
		PreviousOutcomes: []*forge.ActionOutcome{
			{
				ArtifactID:       "skill-read-edit",
				Verdict:          "positive",
				ImprovementScore: 0.35,
				SampleSize:       23,
			},
		},
	}

	report := forge.FormatLearningInsightsForTest(cycle)

	if !strings.Contains(report, "闭环学习状态") {
		t.Error("Report should contain Phase 6 header")
	}
	if !strings.Contains(report, "检测到的模式") {
		t.Error("Report should contain patterns section")
	}
	if !strings.Contains(report, "tool_chain") {
		t.Error("Report should contain tool_chain pattern type")
	}
	if !strings.Contains(report, "学习行动") {
		t.Error("Report should contain actions section")
	}
	if !strings.Contains(report, "create_skill") {
		t.Error("Report should contain action type")
	}
	if !strings.Contains(report, "上一轮反馈") {
		t.Error("Report should contain previous outcomes section")
	}
	if !strings.Contains(report, "positive") {
		t.Error("Report should contain verdict")
	}
	if !strings.Contains(report, "3 模式") {
		t.Error("Report should contain summary stats")
	}
}

func TestFormatLearningInsights_EmptyCycle(t *testing.T) {
	now := time.Now().UTC()
	cycle := &forge.LearningCycle{
		ID:        "lc-empty",
		StartedAt: now,
	}

	report := forge.FormatLearningInsightsForTest(cycle)

	if !strings.Contains(report, "闭环学习状态") {
		t.Error("Report should contain Phase 6 header even for empty cycle")
	}
	if strings.Contains(report, "检测到的模式") {
		t.Error("Empty cycle should not have patterns section")
	}
}

// ============================================================
// Test: forge_learning_status tool
// ============================================================

func TestForgeLearningStatusTool(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	forgeInstance, _ := forge.NewForge(workspace, nil)
	forgeTools := forge.NewForgeTools(forgeInstance)

	var statusTool tools.Tool
	for _, t2 := range forgeTools {
		if t2.Name() == "forge_learning_status" {
			statusTool = t2
			break
		}
	}
	if statusTool == nil {
		t.Fatal("forge_learning_status tool not found")
	}

	result := statusTool.Execute(context.Background(), map[string]interface{}{})
	if result.IsError {
		t.Errorf("forge_learning_status should not error: %s", result.ForLLM)
	}
}

// ============================================================
// Test: RunCycle MaxAutoCreates enforcement
// ============================================================

func TestRunCycle_MaxAutoCreates_Enforced(t *testing.T) {
	tmpDir := t.TempDir()
	forgeDir := filepath.Join(tmpDir, "forge")
	os.MkdirAll(forgeDir, 0755)

	cfg := forge.DefaultForgeConfig()
	cfg.Learning.MinPatternFrequency = 5
	cfg.Learning.HighConfThreshold = 0.5 // Lower threshold to generate more actions
	cfg.Learning.MaxAutoCreates = 1       // Only 1 auto-create allowed
	cfg.Learning.MonitorWindowDays = 30

	registry := forge.NewRegistry(filepath.Join(forgeDir, "registry.json"))
	traceStore := forge.NewTraceStore(forgeDir, cfg)
	cycleStore := forge.NewCycleStore(forgeDir, cfg)
	monitor := forge.NewDeploymentMonitor(traceStore, registry, cfg)

	le := forge.NewLearningEngine(forgeDir, registry, traceStore, nil, cycleStore, monitor, cfg)

	// Create traces that generate multiple high-confidence patterns
	traces := makeToolChainTracesForge("alpha→beta", 12)
	traces = append(traces, makeToolChainTracesForge("gamma→delta", 12)...)

	// No LLM provider → all executeCreateSkill will fail, but attempts are still counted
	cycle := le.RunCycle(context.Background(), traces, nil, nil)

	// Count how many create_skill actions were attempted vs skipped
	attemptedCount := 0
	skippedCount := 0
	for _, a := range cycle.ActionSummary {
		if a.Type == string(forge.ActionCreateSkill) {
			if a.Status == "skipped" {
				skippedCount++
			} else {
				attemptedCount++
			}
		}
	}

	// After fix: attempts are counted (not just successes), so only 1 should be attempted
	if attemptedCount > cfg.Learning.MaxAutoCreates {
		t.Errorf("MaxAutoCreates violated: %d attempted, limit is %d", attemptedCount, cfg.Learning.MaxAutoCreates)
	}

	totalCreateActions := attemptedCount + skippedCount
	if totalCreateActions < 2 {
		t.Errorf("Expected at least 2 create_skill actions total (1 attempted + 1 skipped), got %d", totalCreateActions)
	}

	// At least one should be skipped because of the limit
	if skippedCount == 0 {
		t.Error("Expected at least 1 skipped action due to MaxAutoCreates limit")
	}
}

// ============================================================
// Test: Confidence clamping [0, 1]
// ============================================================

func TestAdjustConfidence_Clamping(t *testing.T) {
	tmpDir := t.TempDir()
	forgeDir := filepath.Join(tmpDir, "forge")
	os.MkdirAll(forgeDir, 0755)

	cfg := forge.DefaultForgeConfig()
	registry := forge.NewRegistry(filepath.Join(forgeDir, "registry.json"))

	// Start near 0 — negative feedback should clamp to 0
	registry.Add(forge.Artifact{
		ID:          "skill-clamp-low",
		Type:        forge.ArtifactSkill,
		Name:        "clamp-low",
		SuccessRate: 0.05,
	})

	// Start near 1 — positive feedback should clamp to 1
	registry.Add(forge.Artifact{
		ID:          "skill-clamp-high",
		Type:        forge.ArtifactSkill,
		Name:        "clamp-high",
		SuccessRate: 0.95,
	})

	traceStore := forge.NewTraceStore(forgeDir, cfg)
	cycleStore := forge.NewCycleStore(forgeDir, cfg)
	monitor := forge.NewDeploymentMonitor(traceStore, registry, cfg)
	le := forge.NewLearningEngine(forgeDir, registry, traceStore, nil, cycleStore, monitor, cfg)

	// Negative feedback on low-success artifact
	le.AdjustConfidenceForTest([]*forge.ActionOutcome{
		{ArtifactID: "skill-clamp-low", Verdict: "negative"},
	})
	artifact, _ := registry.Get("skill-clamp-low")
	if artifact.SuccessRate < 0 {
		t.Errorf("SuccessRate should not go below 0, got %f", artifact.SuccessRate)
	}

	// Positive feedback on high-success artifact
	le.AdjustConfidenceForTest([]*forge.ActionOutcome{
		{ArtifactID: "skill-clamp-high", Verdict: "positive"},
	})
	artifact, _ = registry.Get("skill-clamp-high")
	if artifact.SuccessRate > 1.0 {
		t.Errorf("SuccessRate should not exceed 1.0, got %f", artifact.SuccessRate)
	}
}

// ============================================================
// Test: adjustConfidence ignores unknown artifact IDs
// ============================================================

func TestAdjustConfidence_UnknownArtifact(t *testing.T) {
	le, _ := newTestLearningEngine(t)

	// Should not panic
	le.AdjustConfidenceForTest([]*forge.ActionOutcome{
		{ArtifactID: "nonexistent", Verdict: "positive"},
	})
	le.AdjustConfidenceForTest([]*forge.ActionOutcome{
		{ArtifactID: "", Verdict: "positive"},
	})
}

// ============================================================
// Test: adjustConfidence neutral verdict does nothing
// ============================================================

func TestAdjustConfidence_NeutralVerdict(t *testing.T) {
	tmpDir := t.TempDir()
	forgeDir := filepath.Join(tmpDir, "forge")
	os.MkdirAll(forgeDir, 0755)

	cfg := forge.DefaultForgeConfig()
	registry := forge.NewRegistry(filepath.Join(forgeDir, "registry.json"))
	registry.Add(forge.Artifact{
		ID:          "skill-neutral",
		Type:        forge.ArtifactSkill,
		Name:        "neutral",
		SuccessRate: 0.5,
	})

	traceStore := forge.NewTraceStore(forgeDir, cfg)
	cycleStore := forge.NewCycleStore(forgeDir, cfg)
	monitor := forge.NewDeploymentMonitor(traceStore, registry, cfg)
	le := forge.NewLearningEngine(forgeDir, registry, traceStore, nil, cycleStore, monitor, cfg)

	le.AdjustConfidenceForTest([]*forge.ActionOutcome{
		{ArtifactID: "skill-neutral", Verdict: "neutral"},
	})

	artifact, _ := registry.Get("skill-neutral")
	if artifact.SuccessRate != 0.5 {
		t.Errorf("Neutral verdict should not change SuccessRate, got %f", artifact.SuccessRate)
	}
}

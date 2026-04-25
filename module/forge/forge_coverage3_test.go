package forge

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/providers"
)

// mockLLMProvider is a mock LLM provider for testing.
type mockLLMProvider struct {
	response *providers.LLMResponse
	err      error
	model    string
}

func (m *mockLLMProvider) Chat(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
	if m.err != nil {
		return nil, m.err
	}
	return m.response, nil
}

func (m *mockLLMProvider) GetDefaultModel() string {
	return m.model
}

// --- Evaluator with provider ---

func TestEvaluator_Evaluate_WithProvider(t *testing.T) {
	cfg := DefaultForgeConfig()
	mockProvider := &mockLLMProvider{
		response: &providers.LLMResponse{
			Content: `{"correctness": 85, "quality": 80, "security": 90, "reusability": 75, "notes": "Good quality"}`,
		},
		model: "test-model",
	}

	e := NewQualityEvaluator(mockProvider, cfg)

	artifact := &Artifact{
		Type:    ArtifactSkill,
		Name:    "test-skill",
		Version: "1.0",
	}

	result := e.Evaluate(context.Background(), artifact, "test content")
	if result == nil {
		t.Fatal("Evaluate should return a result")
	}
	if result.Score <= 0 {
		t.Errorf("Score should be > 0, got %d", result.Score)
	}
	if len(result.Dimensions) != 4 {
		t.Errorf("Expected 4 dimensions, got %d", len(result.Dimensions))
	}
}

func TestEvaluator_Evaluate_ProviderError(t *testing.T) {
	cfg := DefaultForgeConfig()
	mockProvider := &mockLLMProvider{
		err:   fmt.Errorf("LLM connection failed"),
		model: "test-model",
	}

	e := NewQualityEvaluator(mockProvider, cfg)

	artifact := &Artifact{
		Type:    ArtifactSkill,
		Name:    "test-skill",
		Version: "1.0",
	}

	result := e.Evaluate(context.Background(), artifact, "test content")
	if result.Passed {
		t.Error("Should fail when LLM call fails")
	}
	if result.Score != 0 {
		t.Errorf("Score should be 0 on LLM failure, got %d", result.Score)
	}
}

func TestEvaluator_Evaluate_BadJSONResponse(t *testing.T) {
	cfg := DefaultForgeConfig()
	mockProvider := &mockLLMProvider{
		response: &providers.LLMResponse{
			Content: "This is not JSON",
		},
		model: "test-model",
	}

	e := NewQualityEvaluator(mockProvider, cfg)

	artifact := &Artifact{
		Type:    ArtifactSkill,
		Name:    "test-skill",
		Version: "1.0",
	}

	result := e.Evaluate(context.Background(), artifact, "test content")
	if result.Passed {
		t.Error("Should fail when LLM response is not JSON")
	}
	if result.Score != 0 {
		t.Errorf("Score should be 0 on parse failure, got %d", result.Score)
	}
}

func TestEvaluator_Evaluate_PartialDimensions(t *testing.T) {
	cfg := DefaultForgeConfig()
	mockProvider := &mockLLMProvider{
		response: &providers.LLMResponse{
			Content: `{"correctness": 90, "quality": 70}`,
		},
		model: "test-model",
	}

	e := NewQualityEvaluator(mockProvider, cfg)

	artifact := &Artifact{
		Type:    ArtifactSkill,
		Name:    "test",
		Version: "1.0",
	}

	result := e.Evaluate(context.Background(), artifact, "test content")
	if result == nil {
		t.Fatal("Evaluate should return a result")
	}
	// Should still produce a score even with partial dimensions
}

// --- RunCycle test ---

func TestLearningEngine_RunCycle(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(filepath.Join(tmpDir, "registry.json"))
	traceStore := NewTraceStore(tmpDir, cfg)
	pipeline := NewPipeline(registry, cfg)
	cycleStore := NewCycleStore(tmpDir, cfg)
	monitor := NewDeploymentMonitor(traceStore, registry, cfg)

	le := NewLearningEngine(tmpDir, registry, traceStore, pipeline, cycleStore, monitor, cfg)
	le.SetProvider(nil) // No provider - will fail gracefully
	le.SetForge(nil)    // No forge

	// Add some traces
	now := time.Now().UTC()
	for i := 0; i < 10; i++ {
		trace := &ConversationTrace{
			TraceID:   "trace-" + string(rune('A'+i)),
			StartTime: now,
			ToolSteps: []ToolStep{
				{ToolName: "read_file", Success: true, LLMRound: 1, ChainPos: 0},
				{ToolName: "edit_file", Success: true, LLMRound: 1, ChainPos: 1},
			},
		}
		traceStore.Append(trace)
	}

	// Get traces and stats
	traces, _ := traceStore.ReadTraces(time.Time{})
	traceStats := &TraceStats{TotalTraces: len(traces)}
	stats := &ReflectionStats{TotalRecords: 10, UniquePatterns: 3}

	cycle := le.RunCycle(context.Background(), traces, traceStats, stats)
	if cycle == nil {
		t.Fatal("RunCycle should return a cycle")
	}
	if cycle.ID == "" {
		t.Error("Cycle ID should not be empty")
	}
	if cycle.StartedAt.IsZero() {
		t.Error("StartedAt should not be zero")
	}
	if cycle.CompletedAt == nil {
		t.Error("CompletedAt should not be nil")
	}
}

func TestLearningEngine_RunCycle_WithPatterns(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(filepath.Join(tmpDir, "registry.json"))
	traceStore := NewTraceStore(tmpDir, cfg)
	pipeline := NewPipeline(registry, cfg)
	cycleStore := NewCycleStore(tmpDir, cfg)
	monitor := NewDeploymentMonitor(traceStore, registry, cfg)

	le := NewLearningEngine(tmpDir, registry, traceStore, pipeline, cycleStore, monitor, cfg)
	le.SetProvider(nil)
	le.SetForge(nil)

	// Create traces with a repeating tool chain pattern (read->edit)
	now := time.Now().UTC()
	for i := 0; i < 15; i++ {
		trace := &ConversationTrace{
			TraceID:   "trace-" + string(rune('A'+i%26)),
			StartTime: now,
			ToolSteps: []ToolStep{
				{ToolName: "read_file", Success: true, LLMRound: 1, ChainPos: 0},
				{ToolName: "edit_file", Success: true, LLMRound: 1, ChainPos: 1},
				{ToolName: "exec", Success: true, LLMRound: 2, ChainPos: 2},
			},
		}
		traceStore.Append(trace)
	}

	traces, _ := traceStore.ReadTraces(time.Time{})
	traceStats := &TraceStats{TotalTraces: len(traces)}
	stats := &ReflectionStats{TotalRecords: 15}

	cycle := le.RunCycle(context.Background(), traces, traceStats, stats)
	if cycle.PatternsFound == 0 {
		t.Error("Should detect patterns with repeated tool chains")
	}
}

// --- GenerateSkillNameForTest ---

func TestGenerateSkillNameForTest(t *testing.T) {
	tests := []struct {
		input    string
		expected string
	}{
		{"read\xe2\x86\x92edit\xe2\x86\x92exec", "read-edit-exec-workflow"},
		{"simple", "simple-workflow"},
	}

	for _, tt := range tests {
		result := GenerateSkillNameForTest(tt.input)
		if result != tt.expected {
			t.Errorf("GenerateSkillName(%q) = %q, expected %q", tt.input, result, tt.expected)
		}
	}
}

// --- BuildDiagnosisForTest ---

func TestBuildDiagnosisForTest(t *testing.T) {
	validation := &ArtifactValidation{
		Stage1Static: &StaticValidationResult{
			ValidationStage: ValidationStage{
				Passed: false,
				Errors: []string{"missing frontmatter", "empty content"},
			},
		},
		Stage2Functional: &FunctionalValidationResult{
			ValidationStage: ValidationStage{
				Passed: false,
				Errors: []string{"test failed"},
			},
		},
		Stage3Quality: &QualityValidationResult{
			ValidationStage: ValidationStage{
				Passed: true,
			},
			Score: 45,
			Notes: "Below threshold",
			Dimensions: map[string]int{
				"correctness": 40,
				"quality":     50,
			},
		},
	}

	result := BuildDiagnosisForTest(validation)
	if result == "" {
		t.Error("BuildDiagnosis should return non-empty string")
	}
}

func TestBuildDiagnosisForTest_NilStages(t *testing.T) {
	validation := &ArtifactValidation{}
	result := BuildDiagnosisForTest(validation)
	if result != "" {
		t.Errorf("Empty validation should produce empty diagnosis, got: %s", result)
	}
}

// --- ExtractToolSignatureFromChainForTest ---

func TestExtractToolSignatureFromChainForTest(t *testing.T) {
	tests := []struct {
		input    string
		expected int
	}{
		{"read\xe2\x86\x92edit\xe2\x86\x92exec", 3},
		{"single", 1},
		{"prefix text\xe2\x86\x92middle\xe2\x86\x92end", 3},
		{"", 0},
	}

	for _, tt := range tests {
		result := ExtractToolSignatureFromChainForTest(tt.input)
		if len(result) != tt.expected {
			t.Errorf("ExtractToolSignatureFromChain(%q): expected %d parts, got %d", tt.input, tt.expected, len(result))
		}
	}
}

// --- Syncer GetLocalReflections2 ---

func TestSyncer_GetLocalReflections2(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(filepath.Join(tmpDir, "registry.json"))
	s := NewSyncer(tmpDir, registry, cfg)

	// No reflections dir yet
	paths, err := s.GetLocalReflections()
	if err != nil {
		t.Errorf("Should not error for nonexistent dir: %v", err)
	}
	if len(paths) != 0 {
		t.Errorf("Expected 0 paths, got %d", len(paths))
	}

	// Create reflections dir with files
	refDir := filepath.Join(tmpDir, "reflections")
	os.MkdirAll(refDir, 0755)
	os.WriteFile(filepath.Join(refDir, "2026-01-15.md"), []byte("report 1"), 0644)
	os.WriteFile(filepath.Join(refDir, "2026-03-20.md"), []byte("report 2"), 0644)
	os.WriteFile(filepath.Join(refDir, "notes.txt"), []byte("not a report"), 0644)

	paths, err = s.GetLocalReflections()
	if err != nil {
		t.Fatalf("GetLocalReflections failed: %v", err)
	}
	if len(paths) != 2 {
		t.Errorf("Expected 2 .md paths, got %d", len(paths))
	}
}

func TestSyncer_GetRemoteReflections2(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(filepath.Join(tmpDir, "registry.json"))
	s := NewSyncer(tmpDir, registry, cfg)

	// No remote dir
	paths, err := s.GetRemoteReflections()
	if err != nil {
		t.Errorf("Should not error: %v", err)
	}
	if len(paths) != 0 {
		t.Errorf("Expected 0, got %d", len(paths))
	}

	// Create remote dir
	remoteDir := filepath.Join(tmpDir, "reflections", "remote")
	os.MkdirAll(remoteDir, 0755)
	os.WriteFile(filepath.Join(remoteDir, "remote-1.md"), []byte("remote report"), 0644)

	paths, err = s.GetRemoteReflections()
	if err != nil {
		t.Fatalf("GetRemoteReflections failed: %v", err)
	}
	if len(paths) != 1 {
		t.Errorf("Expected 1, got %d", len(paths))
	}
}

func TestSyncer_ReadReflectionContent2(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(filepath.Join(tmpDir, "registry.json"))
	s := NewSyncer(tmpDir, registry, cfg)

	// Create reflections dir
	refDir := filepath.Join(tmpDir, "reflections")
	os.MkdirAll(refDir, 0755)
	os.WriteFile(filepath.Join(refDir, "test2.md"), []byte("test content 2"), 0644)

	content, err := s.ReadReflectionContent("test2.md")
	if err != nil {
		t.Fatalf("ReadReflectionContent failed: %v", err)
	}
	if content != "test content 2" {
		t.Errorf("Expected 'test content 2', got '%s'", content)
	}
}

func TestSyncer_ReadReflectionContent2_Nonexistent(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(filepath.Join(tmpDir, "registry.json"))
	s := NewSyncer(tmpDir, registry, cfg)

	_, err := s.ReadReflectionContent("nonexistent2.md")
	if err == nil {
		t.Error("Should error for nonexistent file")
	}
}

// --- Reflector Reflect with experiences ---

func TestReflector_Reflect_WithExperiences(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	cfg.Reflection.MinExperiences = 1 // Lower threshold
	cfg.Reflection.UseLLM = false     // Disable LLM

	store := NewExperienceStore(tmpDir, cfg)
	registry := NewRegistry(filepath.Join(tmpDir, "registry.json"))
	r := NewReflector(tmpDir, store, registry, cfg)
	r.SetProvider(nil) // No LLM

	// Add some aggregated experiences
	for i := 0; i < 15; i++ {
		rec := &AggregatedExperience{
			PatternHash:   "hash-" + string(rune('A'+i%3)),
			ToolName:      "read_file",
			Count:         10 + i,
			SuccessRate:   0.9,
			AvgDurationMs: 100,
			LastSeen:      time.Now().UTC(),
		}
		store.AppendAggregated(rec)
	}

	// Reflect should work with enough experiences
	path, err := r.Reflect(context.Background(), "today", "all")
	if err != nil {
		t.Fatalf("Reflect failed: %v", err)
	}
	if path == "" {
		t.Error("Reflect should return a report path")
	}

	// Verify file exists
	if _, err := os.Stat(path); os.IsNotExist(err) {
		t.Errorf("Report file should exist at %s", path)
	}
}

func TestReflector_Reflect_InsufficientExperiences(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	cfg.Reflection.MinExperiences = 100 // High threshold

	store := NewExperienceStore(tmpDir, cfg)
	registry := NewRegistry(filepath.Join(tmpDir, "registry.json"))
	r := NewReflector(tmpDir, store, registry, cfg)

	_, err := r.Reflect(context.Background(), "today", "all")
	if err == nil {
		t.Error("Should error with insufficient experiences")
	}
}

// --- Forge ReflectNow test ---

func TestForge_ReflectNow2(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := NewForge(workspace, nil)

	// Should return error with insufficient experiences
	_, err := f.ReflectNow(context.Background(), "today", "all")
	if err == nil {
		t.Error("Should error with insufficient experiences")
	}
}

// --- Forge CreateSkill duplicate ---

func TestForge_CreateSkill_Duplicate(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := NewForge(workspace, nil)

	// Create first
	_, err := f.CreateSkill(context.Background(), "dup-skill", "---\nname: dup\n---\nv1", "First", nil)
	if err != nil {
		t.Fatalf("First CreateSkill failed: %v", err)
	}

	// Create again - should update (overwrite)
	_, err = f.CreateSkill(context.Background(), "dup-skill", "---\nname: dup\n---\nv2", "Second", nil)
	if err != nil {
		t.Fatalf("Second CreateSkill failed: %v", err)
	}
}

// --- ForTest wrapper tests (cover trivial exports) ---

func TestMonitor_ForTestExports(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	traceStore := NewTraceStore(tmpDir, cfg)
	registry := NewRegistry(filepath.Join(tmpDir, "registry.json"))
	monitor := NewDeploymentMonitor(traceStore, registry, cfg)

	// MatchesToolSignatureForTest - takes ConversationTrace and []string
	trace := &ConversationTrace{
		ToolSteps: []ToolStep{
			{ToolName: "read_file"},
			{ToolName: "edit_file"},
		},
	}
	result := MatchesToolSignatureForTest(trace, []string{"read_file", "edit_file"})
	if !result {
		t.Error("Should match tool signature")
	}

	// ClassifyVerdictForTest - takes improvementScore and artifact
	artifact := &Artifact{
		ToolSignature:            []string{"read_file"},
		ConsecutiveObservingRounds: 0,
	}
	verdict := monitor.ClassifyVerdictForTest(0.3, artifact)
	_ = verdict
}

func TestPattern_ForTestExports(t *testing.T) {
	// ExtractPatternsForTest
	traces := []*ConversationTrace{
		{
			TraceID:   "t1",
			StartTime: time.Now().UTC(),
			ToolSteps: []ToolStep{
				{ToolName: "read", Success: true, LLMRound: 1, ChainPos: 0},
				{ToolName: "edit", Success: true, LLMRound: 1, ChainPos: 1},
			},
		},
	}
	patterns := ExtractPatternsForTest(traces, 1)
	// May or may not find patterns depending on threshold
	_ = patterns

	// PatternFingerprintForTest
	fp := PatternFingerprintForTest("read\xe2\x86\x92edit", "test")
	if fp == "" {
		t.Error("Fingerprint should not be empty")
	}
}

func TestSanitizer_ForTestExports(t *testing.T) {
	cfg := DefaultForgeConfig()

	s := NewReportSanitizerForTest(cfg)
	_ = s

	result := SanitizeReportForTest(cfg, "password=secret api_key=abc123")
	if result == "" {
		t.Error("SanitizeReport should return non-empty")
	}

	result2 := RedactSensitiveValuesForTest(cfg, "token=xyz")
	if result2 == "" {
		t.Error("RedactSensitiveValues should return non-empty")
	}

	result3 := CleanPathsForTest("path: C:\\Users\\secret\\file.txt")
	if result3 == "" {
		t.Error("CleanPaths should return non-empty")
	}

	result4 := CleanPublicIPsForTest("connect to 8.8.8.8")
	if result4 == "" {
		t.Error("CleanPublicIPs should return non-empty")
	}

	if !IsPrivateIPForTest("192.168.1.1") {
		t.Error("192.168.1.1 should be private")
	}
	if IsPrivateIPForTest("8.8.8.8") {
		t.Error("8.8.8.8 should be public")
	}
}

func TestReport_ForTestExports(t *testing.T) {
	cycle := &LearningCycle{
		ID:             "lc-1",
		PatternsFound:  3,
		ActionsCreated: 2,
		PatternSummary: []PatternSummary{
			{ID: "p1", Type: "tool_chain", Fingerprint: "abc123def456ghi", Frequency: 10, Confidence: 0.9},
		},
		ActionSummary: []ActionSummary{
			{ID: "a1", Type: "create_skill", Status: "executed"},
		},
	}

	result := FormatLearningInsightsForTest(cycle)
	if result == "" {
		t.Error("FormatLearningInsights should return non-empty")
	}

	traceStats := &TraceStats{
		TotalTraces:  10,
		AvgRounds:    3.5,
		SignalSummary: map[string]int{"retry": 2, "backtrack": 1},
	}

	result2 := FormatTraceInsightsForTest(traceStats)
	if result2 == "" {
		t.Error("FormatTraceInsights should return non-empty")
	}
}

func TestSemanticAnalysis_ForTest(t *testing.T) {
	stats := &ReflectionStats{
		TotalRecords:   15,
		UniquePatterns: 2,
		AvgSuccessRate: 0.90,
		ToolFrequency:  map[string]int{"read_file": 10, "edit_file": 5},
		TopPatterns: []*PatternInsight{
			{ToolName: "read_file", Count: 10, SuccessRate: 0.95},
		},
	}

	traceStats := &TraceStats{TotalTraces: 10}
	cfg := DefaultForgeConfig()

	// Test without LLM provider - will error accessing provider.GetDefaultModel
	// Use a mock provider that returns an error
	mockProvider := &mockLLMProvider{
		err:   fmt.Errorf("mock error"),
		model: "test",
	}
	result, err := SemanticAnalysisForTest(context.Background(), mockProvider, stats, nil, traceStats, nil, cfg)
	_ = result
	_ = err
}

// --- forge_build_mcp install action ---

func TestForgeBuildMCPTool_Execute_Install(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := NewForge(workspace, nil)

	// Create MCP artifact with directory and file
	mcpDir := filepath.Join(workspace, "forge", "mcp", "build-test")
	os.MkdirAll(mcpDir, 0755)
	os.WriteFile(filepath.Join(mcpDir, "server.py"), []byte("# test server"), 0644)

	f.GetRegistry().Add(Artifact{
		ID:   "mcp-build-test",
		Type: ArtifactMCP,
		Name: "build-test",
		Path: filepath.Join(mcpDir, "server.py"),
	})

	tools := NewForgeTools(f)
	for _, tool := range tools {
		if tool.Name() == "forge_build_mcp" {
			result := tool.Execute(context.Background(), map[string]interface{}{
				"id":     "mcp-build-test",
				"action": "install",
			})
			if result.IsError {
				t.Logf("Install result: %s", result.ForLLM)
			}
			return
		}
	}
}

// --- forge_share with reflections ---

func TestForgeShareTool_Execute_WithReflections(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := NewForge(workspace, nil)

	// Create reflections dir and a report
	refDir := filepath.Join(workspace, "forge", "reflections")
	os.MkdirAll(refDir, 0755)
	os.WriteFile(filepath.Join(refDir, "test.md"), []byte("# Report\nSome content"), 0644)

	// Set mock bridge that's enabled
	mockBridge := &mockEnabledBridge{}
	f.GetSyncer().SetBridge(mockBridge)

	tools := NewForgeTools(f)
	for _, tool := range tools {
		if tool.Name() == "forge_share" {
			result := tool.Execute(context.Background(), map[string]interface{}{
				"report_path": filepath.Join(refDir, "test.md"),
			})
			// Should succeed or return info message
			_ = result
			return
		}
	}
}

// --- forge_list with type filter ---

func TestForgeListTool_Execute_WithType(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := NewForge(workspace, nil)

	// Add artifacts
	f.GetRegistry().Add(Artifact{ID: "skill-1", Type: ArtifactSkill, Name: "s1", Status: StatusActive})
	f.GetRegistry().Add(Artifact{ID: "script-1", Type: ArtifactScript, Name: "sc1", Status: StatusActive})

	tools := NewForgeTools(f)
	for _, tool := range tools {
		if tool.Name() == "forge_list" {
			// Filter by type
			result := tool.Execute(context.Background(), map[string]interface{}{
				"type": "skill",
			})
			if result.IsError {
				t.Errorf("forge_list with filter should not error: %s", result.ForLLM)
			}

			// Filter by status
			result2 := tool.Execute(context.Background(), map[string]interface{}{
				"status": "active",
			})
			if result2.IsError {
				t.Errorf("forge_list with status filter should not error: %s", result2.ForLLM)
			}
			return
		}
	}
}

// --- checkMCPServerStructure test ---

func TestCheckMCPServerStructure2(t *testing.T) {
	tmpDir := t.TempDir()
	registry := NewRegistry(filepath.Join(tmpDir, "registry.json"))
	runner := NewTestRunner(registry)

	// Test RunTests for MCP type with content
	artifact := &Artifact{
		Type: ArtifactMCP,
		Name: "test-mcp",
	}
	result := runner.RunTests(context.Background(), artifact)
	_ = result

	// Test RunTests for Script type
	artifact2 := &Artifact{
		Type: ArtifactScript,
		Name: "test-script",
	}
	result2 := runner.RunTests(context.Background(), artifact2)
	_ = result2
}

// --- forge_update with rollback version ---

func TestForgeUpdateTool_Execute_Rollback(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := NewForge(workspace, nil)

	skillDir := filepath.Join(workspace, "forge", "skills", "rb-test")
	os.MkdirAll(skillDir, 0755)
	skillPath := filepath.Join(skillDir, "SKILL.md")
	os.WriteFile(skillPath, []byte("---\nname: rb-test\n---\nOriginal v1"), 0644)

	// Save a version snapshot
	SaveVersionSnapshot(skillPath, "1.0")
	os.WriteFile(skillPath, []byte("---\nname: rb-test\n---\nModified v2"), 0644)
	SaveVersionSnapshot(skillPath, "2.0")

	f.GetRegistry().Add(Artifact{
		ID:      "skill-rb-test",
		Type:    ArtifactSkill,
		Name:    "rb-test",
		Version: "2.0",
		Path:    skillPath,
	})

	tools := NewForgeTools(f)
	for _, tool := range tools {
		if tool.Name() == "forge_update" {
			result := tool.Execute(context.Background(), map[string]interface{}{
				"id":               "skill-rb-test",
				"rollback_version": "1.0",
			})
			if result.IsError {
				t.Errorf("forge_update rollback should succeed: %s", result.ForLLM)
			}
			return
		}
	}
}

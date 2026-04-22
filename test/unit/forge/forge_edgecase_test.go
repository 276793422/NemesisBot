package forge_test

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/forge"
	"github.com/276793422/NemesisBot/module/providers"
)

// ============================================================
// Edge case: CreateSkill with invalid name characters
// ============================================================
func TestForge_CreateSkill_InvalidName(t *testing.T) {
	f, _ := newTestForge(t)

	tests := []struct {
		name    string
		skillID string
	}{
		{"empty name", ""},
		{"slashes", "a/b/c"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			// Should not panic even with weird names
			_, _ = f.CreateSkill(context.Background(), tt.skillID, "content", "desc", nil)
		})
	}
}

// ============================================================
// Edge case: CreateSkill content with existing frontmatter
// preserves original frontmatter
// ============================================================
func TestForge_CreateSkill_FrontmatterPreserved(t *testing.T) {
	f, _ := newTestForge(t)
	content := "---\nname: custom-name\nversion: \"2.0\"\nauthor: test\n---\n\nCustom body content."
	artifact, err := f.CreateSkill(context.Background(), "preserve-fm", content, "desc", nil)
	if err != nil {
		t.Fatalf("CreateSkill failed: %v", err)
	}
	data, _ := os.ReadFile(artifact.Path)
	fileStr := string(data)
	if !contains(fileStr, "custom-name") {
		t.Error("Original frontmatter name should be preserved")
	}
	if !contains(fileStr, "Custom body content") {
		t.Error("Body should be preserved")
	}
}

// ============================================================
// Edge case: Reflector with empty period falls back to "today"
// ============================================================
func TestReflector_ResolvePeriodDefault(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	cfg.Reflection.MinExperiences = 1
	store := forge.NewExperienceStore(tmpDir, cfg)
	registry := forge.NewRegistry(filepath.Join(tmpDir, "registry.json"))
	reflector := forge.NewReflector(tmpDir, store, registry, cfg)

	store.AppendAggregated(&forge.AggregatedExperience{
		PatternHash: "sha256:period-default",
		ToolName:    "test_tool",
		Count:       5,
		LastSeen:    now(),
	})

	// Empty string period — should fall back to "today" default
	reportPath, err := reflector.Reflect(context.Background(), "", "all")
	if err != nil {
		t.Fatalf("Reflect with empty period should succeed: %v", err)
	}
	if reportPath == "" {
		t.Error("Should generate a report")
	}
}

// ============================================================
// Edge case: Reflector with zero MinExperiences
// ============================================================
func TestReflector_ZeroMinExperiences(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	cfg.Reflection.MinExperiences = 0 // Allow zero
	store := forge.NewExperienceStore(tmpDir, cfg)
	registry := forge.NewRegistry(filepath.Join(tmpDir, "registry.json"))
	reflector := forge.NewReflector(tmpDir, store, registry, cfg)

	// No data at all — but MinExperiences=0, so it should still produce a report
	reportPath, err := reflector.Reflect(context.Background(), "today", "all")
	if err != nil {
		t.Fatalf("Reflect with zero min and no data should succeed: %v", err)
	}
	content, _ := os.ReadFile(reportPath)
	if !contains(string(content), "统计概要") {
		t.Error("Report should have statistical summary even with zero data")
	}
}

// ============================================================
// Edge case: Registry concurrent access safety
// ============================================================
func TestRegistry_ConcurrentAccess(t *testing.T) {
	tmpDir := t.TempDir()
	registry := forge.NewRegistry(filepath.Join(tmpDir, "registry.json"))

	var wg sync.WaitGroup
	for i := 0; i < 50; i++ {
		wg.Add(1)
		go func(idx int) {
			defer wg.Done()
			artifact := forge.Artifact{
				ID:     "skill-concurrent-" + string(rune('A'+idx%26)),
				Type:   forge.ArtifactSkill,
				Name:   "concurrent-" + string(rune('A'+idx%26)),
				Status: forge.StatusDraft,
			}
			registry.Add(artifact)
		}(i)
	}
	wg.Wait()

	count := registry.Count("")
	if count != 50 {
		t.Errorf("Expected 50 artifacts, got %d", count)
	}
}

// ============================================================
// Edge case: Registry concurrent read/write
// ============================================================
func TestRegistry_ConcurrentReadWrite(t *testing.T) {
	tmpDir := t.TempDir()
	registry := forge.NewRegistry(filepath.Join(tmpDir, "registry.json"))

	// Seed initial data
	for i := 0; i < 10; i++ {
		registry.Add(forge.Artifact{
			ID:     "skill-rw-" + string(rune('A'+i)),
			Type:   forge.ArtifactSkill,
			Name:   "rw-" + string(rune('A'+i)),
			Status: forge.StatusDraft,
		})
	}

	var wg sync.WaitGroup
	// Concurrent reads
	for i := 0; i < 20; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			registry.ListAll()
			registry.List(forge.ArtifactSkill, "")
			registry.Count(forge.ArtifactSkill)
			registry.Get("skill-rw-A")
		}()
	}
	// Concurrent writes
	for i := 0; i < 10; i++ {
		wg.Add(1)
		go func(idx int) {
			defer wg.Done()
			registry.Add(forge.Artifact{
				ID:     "skill-rw-new-" + string(rune('A'+idx)),
				Type:   forge.ArtifactSkill,
				Name:   "rw-new-" + string(rune('A'+idx)),
				Status: forge.StatusActive,
			})
		}(i)
	}
	wg.Wait()

	// Should not panic
	total := registry.Count("")
	if total < 20 { // 10 initial + 10 new minimum
		t.Errorf("Expected at least 20, got %d", total)
	}
}

// ============================================================
// Edge case: ToolSignature matching — empty signature, partial match
// ============================================================
func TestMatchesToolSignature_EdgeCases(t *testing.T) {
	trace := &forge.ConversationTrace{
		ToolSteps: []forge.ToolStep{
			{ToolName: "read_file"},
			{ToolName: "edit_file"},
			{ToolName: "exec"},
		},
	}

	// Empty signature — should return false
	if forge.MatchesToolSignatureForTest(trace, nil) {
		t.Error("Empty signature should not match")
	}
	if forge.MatchesToolSignatureForTest(trace, []string{}) {
		t.Error("Empty signature slice should not match")
	}

	// Partial match — signature is prefix of tool chain
	if !forge.MatchesToolSignatureForTest(trace, []string{"read_file", "edit_file"}) {
		t.Error("Partial prefix should match as subsequence")
	}

	// Non-matching order
	if forge.MatchesToolSignatureForTest(trace, []string{"edit_file", "read_file"}) {
		t.Error("Wrong order should not match")
	}

	// Non-existent tool
	if forge.MatchesToolSignatureForTest(trace, []string{"nonexistent"}) {
		t.Error("Non-existent tool should not match")
	}

	// Exact match
	if !forge.MatchesToolSignatureForTest(trace, []string{"read_file", "edit_file", "exec"}) {
		t.Error("Exact match should match")
	}
}

// ============================================================
// Edge case: Monitor with no traces (nil from ReadTraces)
// ============================================================
func TestMonitor_EvaluateOutcomes_NoTraces(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	registry := forge.NewRegistry(filepath.Join(tmpDir, "registry.json"))
	traceStore := forge.NewTraceStore(tmpDir, cfg)
	monitor := forge.NewDeploymentMonitor(traceStore, registry, cfg)

	// Add an active artifact
	registry.Add(forge.Artifact{
		ID:            "skill-monitor-notrace",
		Type:          forge.ArtifactSkill,
		Name:          "monitor-notrace",
		Status:        forge.StatusActive,
		ToolSignature: []string{"read_file"},
	})

	// No traces at all — should return nil (no outcomes)
	outcomes := monitor.EvaluateOutcomes()
	if outcomes != nil {
		t.Errorf("Expected nil outcomes with no traces, got %d", len(outcomes))
	}
}

// ============================================================
// Edge case: Monitor classifyVerdict boundaries
// ============================================================
func TestMonitor_ClassifyVerdict_Thresholds(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	cfg.Learning.DegradeThreshold = -0.2
	registry := forge.NewRegistry(filepath.Join(tmpDir, "registry.json"))
	traceStore := forge.NewTraceStore(tmpDir, cfg)
	monitor := forge.NewDeploymentMonitor(traceStore, registry, cfg)

	tests := []struct {
		score    float64
		expected string
	}{
		{0.5, "positive"},
		{0.11, "positive"},
		{0.1001, "positive"},
		{0.1, "neutral"},   // 0.1 is NOT > 0.1, so neutral
		{0.05, "neutral"},
		{0.0, "neutral"},
		{-0.05, "neutral"},
		{-0.1, "neutral"},  // -0.1 >= -0.1, so neutral
		{-0.1001, "observing"},
		{-0.15, "observing"},
		{-0.2, "observing"}, // -0.2 >= -0.2, so observing
		{-0.2001, "negative"},
		{-0.5, "negative"},
	}

	for _, tt := range tests {
		artifact := &forge.Artifact{ID: "test"}
		verdict := monitor.ClassifyVerdictForTest(tt.score, artifact)
		if verdict != tt.expected {
			t.Errorf("classifyVerdict(%+.2f) = %q, want %q", tt.score, verdict, tt.expected)
		}
	}
}

// ============================================================
// Edge case: Syncer ShareReflection with nonexistent file
// ============================================================
func TestSyncer_ShareReflection_NonexistentFile(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	registry := forge.NewRegistry(filepath.Join(tmpDir, "registry.json"))
	syncer := forge.NewSyncer(tmpDir, registry, cfg)

	bridge := &mockBridge{clusterRun: true, peers: []forge.PeerInfo{{ID: "p1", Name: "P1"}}}
	syncer.SetBridge(bridge)

	err := syncer.ShareReflection(context.Background(), "/nonexistent/path/report.md")
	if err == nil {
		t.Error("Should fail with nonexistent file")
	}
	if !contains(err.Error(), "failed to read report") {
		t.Errorf("Error should mention read failure, got: %v", err)
	}
}

// ============================================================
// Edge case: ParseLLMInsights with various delimiters
// ============================================================
func TestParseLLMInsights_Unicode(t *testing.T) {
	response := "- 建议：创建配置编辑器 Skill\n- 模式：exec 工具错误率高\n- 分析：read→write 效率佳"
	insights := forge.ParseLLMInsights(response)
	if len(insights) != 3 {
		t.Fatalf("Expected 3 insights from Chinese bullets, got %d", len(insights))
	}
	if !contains(insights[0], "建议") {
		t.Errorf("First insight should contain Chinese: %s", insights[0])
	}
}

// ============================================================
// Edge case: ExtractJSONFromResponse with nested braces
// ============================================================
func TestExtractJSONFromResponse_NestedJSON(t *testing.T) {
	response := "Result: {\"key\": \"{nested}\", \"count\": 5}"
	result, err := forge.ExtractJSONFromResponse(response)
	if err != nil {
		t.Fatalf("Should extract nested JSON: %v", err)
	}
	if result["key"] != "{nested}" {
		t.Errorf("Expected nested value, got: %v", result["key"])
	}
}

func TestExtractJSONFromResponse_MultipleObjects(t *testing.T) {
	// Multiple separate JSON objects — extracts full span which may be invalid
	response := "First: {\"a\": 1} Second: {\"b\": 2}"
	_, err := forge.ExtractJSONFromResponse(response)
	// This produces {a:1} Second: {b:2} which is invalid JSON — should error
	if err == nil {
		t.Error("Should error on multiple separate JSON objects")
	}
}

// ============================================================
// Edge case: FormatReport with all nil sections
// ============================================================
func TestFormatReport_AllNil(t *testing.T) {
	report := &forge.ReflectionReport{
		Date:  "2026-04-21",
		Stats: nil,
	}
	// Should not panic
	result := forge.FormatReport(report)
	if !contains(result, "2026-04-21") {
		t.Error("Should contain date")
	}
}

// ============================================================
// Edge case: Sanitizer with empty input
// ============================================================
func TestSanitizer_EmptyInput(t *testing.T) {
	cfg := forge.DefaultForgeConfig()
	result := forge.SanitizeReportForTest(cfg, "")
	if result != "" {
		t.Errorf("Empty input should return empty, got: %s", result)
	}
}

func TestSanitizer_NoSecrets(t *testing.T) {
	cfg := forge.DefaultForgeConfig()
	input := "Just a normal report with no secrets at all."
	result := forge.SanitizeReportForTest(cfg, input)
	if result != input {
		t.Errorf("Normal text should not be modified, got: %s", result)
	}
}

// ============================================================
// Edge case: ExperienceStore concurrent writes
// ============================================================
func TestExperienceStore_ConcurrentWrites(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	store := forge.NewExperienceStore(tmpDir, cfg)

	var wg sync.WaitGroup
	for i := 0; i < 20; i++ {
		wg.Add(1)
		go func(idx int) {
			defer wg.Done()
			store.AppendAggregated(&forge.AggregatedExperience{
				PatternHash: "sha256:concurrent-" + string(rune('A'+idx%26)),
				ToolName:    "tool-" + string(rune('A'+idx%26)),
				Count:       idx + 1,
				LastSeen:    now(),
			})
		}(i)
	}
	wg.Wait()

	// Verify data was written
	records, err := store.ReadAggregated(time.Now().UTC().AddDate(0, 0, -1))
	if err != nil {
		t.Fatalf("ReadAggregated failed: %v", err)
	}
	if len(records) == 0 {
		t.Error("Should have records after concurrent writes")
	}
}

// ============================================================
// Edge case: LearningEngine adjustConfidence clamping
// ============================================================
func TestLearningEngine_AdjustConfidence_Clamping(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	registry := forge.NewRegistry(filepath.Join(tmpDir, "registry.json"))

	// Create artifact with very high success rate
	registry.Add(forge.Artifact{
		ID:          "skill-clamp-high",
		Type:        forge.ArtifactSkill,
		Name:        "clamp-high",
		Status:      forge.StatusActive,
		SuccessRate: 0.95,
	})

	registry.Add(forge.Artifact{
		ID:          "skill-clamp-low",
		Type:        forge.ArtifactSkill,
		Name:        "clamp-low",
		Status:      forge.StatusActive,
		SuccessRate: 0.05,
	})

	traceStore := forge.NewTraceStore(tmpDir, cfg)
	cycleStore := forge.NewCycleStore(tmpDir, cfg)
	pipeline := forge.NewPipeline(registry, cfg)
	monitor := forge.NewDeploymentMonitor(traceStore, registry, cfg)
	le := forge.NewLearningEngine(tmpDir, registry, traceStore, pipeline, cycleStore, monitor, cfg)

	// Positive outcome on high rate → should clamp to 1.0
	le.AdjustConfidenceForTest([]*forge.ActionOutcome{
		{ArtifactID: "skill-clamp-high", Verdict: "positive"},
	})
	high, _ := registry.Get("skill-clamp-high")
	if high.SuccessRate > 1.0 {
		t.Errorf("Success rate should be clamped to <= 1.0, got %f", high.SuccessRate)
	}
	if high.SuccessRate != 1.0 {
		t.Errorf("Expected 1.0 after positive on 0.95, got %f", high.SuccessRate)
	}

	// Negative outcome on low rate → should clamp to 0.0
	le.AdjustConfidenceForTest([]*forge.ActionOutcome{
		{ArtifactID: "skill-clamp-low", Verdict: "negative"},
	})
	low, _ := registry.Get("skill-clamp-low")
	if low.SuccessRate < 0 {
		t.Errorf("Success rate should be clamped to >= 0, got %f", low.SuccessRate)
	}
	if low.SuccessRate != 0.0 {
		t.Errorf("Expected 0.0 after negative on 0.05, got %f", low.SuccessRate)
	}
}

// ============================================================
// Edge case: LearningEngine adjustConfidence with unknown artifact
// ============================================================
func TestLearningEngine_AdjustConfidence_UnknownArtifact(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	registry := forge.NewRegistry(filepath.Join(tmpDir, "registry.json"))
	traceStore := forge.NewTraceStore(tmpDir, cfg)
	cycleStore := forge.NewCycleStore(tmpDir, cfg)
	pipeline := forge.NewPipeline(registry, cfg)
	monitor := forge.NewDeploymentMonitor(traceStore, registry, cfg)
	le := forge.NewLearningEngine(tmpDir, registry, traceStore, pipeline, cycleStore, monitor, cfg)

	// Should not panic with unknown artifact
	le.AdjustConfidenceForTest([]*forge.ActionOutcome{
		{ArtifactID: "nonexistent", Verdict: "positive"},
	})

	// Empty artifact ID should be skipped
	le.AdjustConfidenceForTest([]*forge.ActionOutcome{
		{ArtifactID: "", Verdict: "positive"},
	})
}

// ============================================================
// Edge case: forge_update tool with MCP type (non-skill)
// ============================================================
func TestForgeUpdateTool_MCPType(t *testing.T) {
	f, workspace := newTestForge(t)
	configDir := filepath.Join(workspace, "config")
	os.MkdirAll(configDir, 0755)

	// Create MCP artifact manually
	mcpDir := filepath.Join(f.GetWorkspace(), "mcp", "update-mcp-test")
	os.MkdirAll(mcpDir, 0755)
	mcpPath := filepath.Join(mcpDir, "server.py")
	os.WriteFile(mcpPath, []byte("from mcp.server import Server\nimport mcp\n\ndef main():\n    pass\n"), 0644)

	f.GetRegistry().Add(forge.Artifact{
		ID:      "mcp-update-mcp-test",
		Type:    forge.ArtifactMCP,
		Name:    "update-mcp-test",
		Version: "1.0",
		Status:  forge.StatusActive,
		Path:    mcpPath,
	})

	ftools := forge.NewForgeTools(f)
	tool := findTool(t, ftools, "forge_update")

	result := tool.Execute(context.Background(), map[string]interface{}{
		"id":                 "mcp-update-mcp-test",
		"content":            "from mcp.server import Server\nimport mcp\n\ndef main():\n    print('updated')\n",
		"change_description": "Updated MCP",
	})
	if result.IsError {
		t.Fatalf("forge_update MCP should succeed: %s", result.ForLLM)
	}

	// Verify version incremented
	artifact, _ := f.GetRegistry().Get("mcp-update-mcp-test")
	if artifact.Version == "1.0" {
		t.Error("MCP version should be incremented")
	}
}

// ============================================================
// Edge case: forge_create tool with script that has no test_cases (should fail)
// ============================================================
func TestForgeCreateTool_ScriptNoTestCases(t *testing.T) {
	f, _ := newTestForge(t)
	ftools := forge.NewForgeTools(f)
	tool := findTool(t, ftools, "forge_create")

	result := tool.Execute(context.Background(), map[string]interface{}{
		"type":        "script",
		"name":        "no-test-script",
		"content":     "echo hello",
		"description": "Script without test cases",
	})
	// Scripts also require test_cases
	if !result.IsError {
		t.Error("Script without test_cases should error")
	}
	if !contains(result.ForLLM, "test_cases") {
		t.Errorf("Error should mention test_cases, got: %s", result.ForLLM)
	}
}

// ============================================================
// Edge case: forge_create tool with MCP missing test_cases
// ============================================================
func TestForgeCreateTool_MCPNoTestCases(t *testing.T) {
	f, _ := newTestForge(t)
	ftools := forge.NewForgeTools(f)
	tool := findTool(t, ftools, "forge_create")

	result := tool.Execute(context.Background(), map[string]interface{}{
		"type":    "mcp",
		"name":    "no-test-mcp",
		"content": "from mcp.server import Server\n",
	})
	if !result.IsError {
		t.Error("MCP without test_cases should error")
	}
	if !contains(result.ForLLM, "test_cases") {
		t.Errorf("Error should mention test_cases, got: %s", result.ForLLM)
	}
}

// ============================================================
// Edge case: forge_create tool with invalid type
// ============================================================
func TestForgeCreateTool_InvalidType(t *testing.T) {
	f, _ := newTestForge(t)
	ftools := forge.NewForgeTools(f)
	tool := findTool(t, ftools, "forge_create")

	result := tool.Execute(context.Background(), map[string]interface{}{
		"type":        "invalid_type",
		"name":        "test",
		"content":     "content",
		"description": "desc",
	})
	if !result.IsError {
		t.Error("Should error with invalid type")
	}
}

// ============================================================
// Edge case: forge_evaluate tool with MCP artifact
// ============================================================
func TestForgeEvaluateTool_MCPArtifact(t *testing.T) {
	f, _ := newTestForge(t)

	mcpDir := filepath.Join(f.GetWorkspace(), "mcp", "eval-mcp")
	os.MkdirAll(mcpDir, 0755)
	mcpPath := filepath.Join(mcpDir, "server.py")
	os.WriteFile(mcpPath, []byte("from mcp.server import Server\nimport mcp\n\ndef main():\n    pass\n"), 0644)

	f.GetRegistry().Add(forge.Artifact{
		ID:      "mcp-eval-mcp",
		Type:    forge.ArtifactMCP,
		Name:    "eval-mcp",
		Version: "1.0",
		Status:  forge.StatusDraft,
		Path:    mcpPath,
	})

	ftools := forge.NewForgeTools(f)
	tool := findTool(t, ftools, "forge_evaluate")
	result := tool.Execute(context.Background(), map[string]interface{}{
		"id": "mcp-eval-mcp",
	})
	if result.IsError {
		t.Errorf("Evaluate MCP should succeed: %s", result.ForLLM)
	}
}

// ============================================================
// Edge case: CycleStore with corrupt JSONL file
// ============================================================
func TestCycleStore_CorruptFile(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	cs := forge.NewCycleStore(tmpDir, cfg)

	// Write corrupt JSONL data
	learningDir := filepath.Join(tmpDir, "learning")
	os.MkdirAll(learningDir, 0755)
	corruptFile := filepath.Join(learningDir, time.Now().UTC().Format("20060102")+".jsonl")
	os.WriteFile(corruptFile, []byte("not valid json\n{broken json\n"), 0644)

	// Should not panic, just skip corrupt lines
	cycles, err := cs.ReadCycles(time.Now().UTC().AddDate(0, 0, -1))
	if err != nil {
		t.Fatalf("ReadCycles should not fail on corrupt data: %v", err)
	}
	// Corrupt entries should be skipped
	if len(cycles) != 0 {
		t.Errorf("Expected 0 valid cycles from corrupt file, got %d", len(cycles))
	}
}

// ============================================================
// Edge case: ExperienceStore with corrupt JSONL
// ============================================================
func TestExperienceStore_CorruptFile(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	store := forge.NewExperienceStore(tmpDir, cfg)

	// Write corrupt data
	monthDir := filepath.Join(tmpDir, "experiences", time.Now().UTC().Format("200601"))
	os.MkdirAll(monthDir, 0755)
	dayFile := filepath.Join(monthDir, time.Now().UTC().Format("20060102")+".jsonl")
	os.WriteFile(dayFile, []byte("corrupt line\nalso bad\n"), 0644)

	// Should not panic
	records, err := store.ReadAggregated(time.Now().UTC().AddDate(0, 0, -1))
	if err != nil {
		t.Fatalf("ReadAggregated should not fail on corrupt data: %v", err)
	}
	if len(records) != 0 {
		t.Errorf("Expected 0 valid records from corrupt file, got %d", len(records))
	}
}

// ============================================================
// Edge case: SemanticAnalysis with minimal stats
// ============================================================
func TestSemanticAnalysis_MinimalStats(t *testing.T) {
	cfg := forge.DefaultForgeConfig()
	mockProvider := &mockLLMProvider{
		chatFunc: func(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
			return &providers.LLMResponse{Content: "Analysis done", FinishReason: "stop"}, nil
		},
	}

	// Minimal stats (empty tool frequency)
	stats := &forge.ReflectionStats{
		TotalRecords:   0,
		UniquePatterns: 0,
		AvgSuccessRate: 0,
		ToolFrequency:  map[string]int{},
	}
	_, err := forge.SemanticAnalysisForTest(context.Background(), mockProvider, stats, nil, nil, nil, cfg)
	if err != nil {
		t.Fatalf("Should handle minimal stats gracefully: %v", err)
	}
}

// ============================================================
// Edge case: Forge Start/Stop basic lifecycle
// ============================================================
func TestForge_StartStopBasic(t *testing.T) {
	f, _ := newTestForge(t)

	// Start should not panic
	f.Start()
	time.Sleep(50 * time.Millisecond)

	// Stop should complete
	done := make(chan struct{})
	go func() {
		f.Stop()
		close(done)
	}()
	select {
	case <-done:
		// ok
	case <-time.After(5 * time.Second):
		t.Fatal("Stop should complete within 5 seconds")
	}
}

// ============================================================
// Edge case: Forge create then delete artifact
// ============================================================
func TestForge_CreateAndDelete(t *testing.T) {
	f, _ := newTestForge(t)

	artifact, err := f.CreateSkill(context.Background(), "delete-test", "content", "desc", nil)
	if err != nil {
		t.Fatalf("CreateSkill failed: %v", err)
	}

	// Verify exists
	_, found := f.GetRegistry().Get(artifact.ID)
	if !found {
		t.Fatal("Artifact should exist")
	}

	// Delete
	err = f.GetRegistry().Delete(artifact.ID)
	if err != nil {
		t.Fatalf("Delete failed: %v", err)
	}

	// Verify deleted
	_, found = f.GetRegistry().Get(artifact.ID)
	if found {
		t.Error("Artifact should be deleted")
	}
}

// ============================================================
// Edge case: Registry delete nonexistent
// ============================================================
func TestRegistry_DeleteNonexistent(t *testing.T) {
	tmpDir := t.TempDir()
	registry := forge.NewRegistry(filepath.Join(tmpDir, "registry.json"))

	err := registry.Delete("nonexistent")
	if err != nil {
		t.Errorf("Deleting nonexistent should not error: %v", err)
	}
}

// ============================================================
// Edge case: Syncer ShareReflection with no bridge
// ============================================================
func TestSyncer_ShareReflection_NoBridge(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	registry := forge.NewRegistry(filepath.Join(tmpDir, "registry.json"))
	syncer := forge.NewSyncer(tmpDir, registry, cfg)

	err := syncer.ShareReflection(context.Background(), "/tmp/fake.md")
	if err == nil {
		t.Error("Should fail without bridge")
	}
	if !contains(err.Error(), "not enabled") {
		t.Errorf("Error should mention not enabled, got: %v", err)
	}
}

// ============================================================
// Edge case: Reflector CleanupReports removes old files
// ============================================================
func TestReflector_CleanupReports_RemovesOld(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	store := forge.NewExperienceStore(tmpDir, cfg)
	registry := forge.NewRegistry(filepath.Join(tmpDir, "registry.json"))
	reflector := forge.NewReflector(tmpDir, store, registry, cfg)

	reflectionsDir := filepath.Join(tmpDir, "reflections")
	os.MkdirAll(reflectionsDir, 0755)

	// Create an old report
	oldDate := time.Now().UTC().AddDate(0, 0, -31).Format("2006-01-02")
	oldPath := filepath.Join(reflectionsDir, oldDate+".md")
	os.WriteFile(oldPath, []byte("# Old report"), 0644)

	// Create a recent report
	recentDate := time.Now().UTC().Format("2006-01-02")
	recentPath := filepath.Join(reflectionsDir, recentDate+".md")
	os.WriteFile(recentPath, []byte("# Recent report"), 0644)

	err := reflector.CleanupReports(30)
	if err != nil {
		t.Fatalf("CleanupReports failed: %v", err)
	}

	// Old should be removed, recent should remain
	if _, err := os.Stat(oldPath); !os.IsNotExist(err) {
		t.Error("Old report should be cleaned up")
	}
	if _, err := os.Stat(recentPath); os.IsNotExist(err) {
		t.Error("Recent report should NOT be cleaned up")
	}
}

// ============================================================
// Edge case: Reflector GetLatestReport empty dir
// ============================================================
func TestReflector_GetLatestReport_EmptyDir(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	store := forge.NewExperienceStore(tmpDir, cfg)
	registry := forge.NewRegistry(filepath.Join(tmpDir, "registry.json"))
	reflector := forge.NewReflector(tmpDir, store, registry, cfg)

	_, err := reflector.GetLatestReport()
	if err == nil {
		t.Error("Should error when no reports exist")
	}
	// Error could be either "no reflection reports found" or directory not exists
	// Both are acceptable failure modes for empty dir
}

// ============================================================
// Edge case: FormatReport with very long content (no panic)
// ============================================================
func TestFormatReport_LongContent(t *testing.T) {
	// Generate a very long LLM insights string
	longStr := strings.Repeat("This is a very long insight about patterns. ", 500)
	report := &forge.ReflectionReport{
		Date:        "2026-04-21",
		Stats:       &forge.ReflectionStats{TotalRecords: 10, UniquePatterns: 2, AvgSuccessRate: 0.5},
		LLMInsights: longStr,
	}
	// Should not panic — long content is allowed
	result := forge.FormatReport(report)
	if result == "" {
		t.Error("Should produce output")
	}
}

// ============================================================
// Edge case: Version increment patterns
// ============================================================
func TestVersionIncrement_Patterns(t *testing.T) {
	tests := []struct {
		input    string
		expected string
	}{
		{"1.0", "1.1"},
		{"1.9", "1.10"},
		{"2.0", "2.1"},
		{"0.0", "0.1"},
		{"10.99", "10.100"},
	}
	for _, tt := range tests {
		result := forge.IncrementVersionForTest(tt.input)
		if result != tt.expected {
			t.Errorf("IncrementVersion(%s) = %s, want %s", tt.input, result, tt.expected)
		}
	}
}

// ============================================================
// Edge case: SemanticAnalysis context cancellation
// ============================================================
func TestSemanticAnalysis_ContextCancelled(t *testing.T) {
	cfg := forge.DefaultForgeConfig()
	stats := &forge.ReflectionStats{TotalRecords: 10, UniquePatterns: 2, AvgSuccessRate: 0.5}

	// Create cancelled context
	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	mockProvider := &mockLLMProvider{
		chatFunc: func(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
			return nil, ctx.Err()
		},
	}

	_, err := forge.SemanticAnalysisForTest(ctx, mockProvider, stats, nil, nil, nil, cfg)
	if err == nil {
		t.Error("Should error with cancelled context")
	}
}

package forge_test

import (
	"context"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/forge"
	"github.com/276793422/NemesisBot/module/providers"
)

// IT: Create Skill and immediately validate through Pipeline
func TestIT_ForgeCreateAndValidate(t *testing.T) {
	f, _ := newTestForge(t)

	// Create a skill with good content
	content := "---\nname: it-validate\n---\n\n# IT Validate Skill\n\nThis skill validates config files.\n\n## Steps\n\n1. Read config\n2. Validate schema\n3. Report errors"
	artifact, err := f.CreateSkill(context.Background(), "it-validate", content, "IT validation skill", nil)
	if err != nil {
		t.Fatalf("CreateSkill failed: %v", err)
	}

	// Run pipeline validation
	validation := f.GetPipeline().RunFromContent(context.Background(), &artifact, content)
	if validation == nil {
		t.Fatal("Validation should not be nil")
	}
	if validation.Stage1Static == nil {
		t.Error("Stage 1 should have run")
	}

	// Verify artifact is in registry
	regArtifact, found := f.GetRegistry().Get(artifact.ID)
	if !found {
		t.Fatal("Artifact should be in registry")
	}
	if regArtifact.Name != "it-validate" {
		t.Errorf("Registry name mismatch: %s", regArtifact.Name)
	}
}

// IT: Collect experiences and trigger reflection
func TestIT_CollectAndReflect(t *testing.T) {
	f, _ := newTestForge(t)
	cfg := f.GetConfig()
	cfg.Reflection.MinExperiences = 1

	// Use the experience store directly to seed data
	store := forge.NewExperienceStore(f.GetWorkspace(), cfg)

	// Add multiple experiences
	tools := []string{"read_file", "exec", "write_file", "read_file", "exec"}
	for i, tool := range tools {
		store.AppendAggregated(&forge.AggregatedExperience{
			PatternHash:   "sha256:it-" + tool,
			ToolName:      tool,
			Count:         i + 5,
			AvgDurationMs: int64((i + 1) * 100),
			SuccessRate:   0.8 + float64(i)*0.02,
			LastSeen:      time.Now().UTC(),
		})
	}

	// Run reflection
	reportPath, err := f.ReflectNow(context.Background(), "today", "all")
	if err != nil {
		t.Fatalf("ReflectNow failed: %v", err)
	}

	// Verify report was generated
	content, err := os.ReadFile(reportPath)
	if err != nil {
		t.Fatalf("Failed to read report: %v", err)
	}

	reportStr := string(content)
	if !contains(reportStr, "统计概要") {
		t.Error("Report should contain statistical summary")
	}
	if !contains(reportStr, "read_file") {
		t.Error("Report should contain tool data")
	}
}

// IT: Create artifact, update, and verify version chain
func TestIT_UpdateWithVersioning(t *testing.T) {
	f, _ := newTestForge(t)

	// Step 1: Create
	artifact, err := f.CreateSkill(context.Background(), "version-chain", "v1 content", "desc", nil)
	if err != nil {
		t.Fatalf("CreateSkill failed: %v", err)
	}
	if artifact.Version != "1.0" {
		t.Errorf("Initial version should be 1.0, got %s", artifact.Version)
	}

	// Step 2: Save snapshot
	forge.SaveVersionSnapshot(artifact.Path, "1.0")

	// Step 3: Update via tool
	ftools := forge.NewForgeTools(f)
	updateTool := findTool(t, ftools, "forge_update")

	result := updateTool.Execute(context.Background(), map[string]interface{}{
		"id":                 "skill-version-chain",
		"content":            "v2 updated content",
		"change_description": "Updated to v2",
	})
	if result.IsError {
		t.Fatalf("Update failed: %s", result.ForLLM)
	}

	// Verify version incremented
	updated, _ := f.GetRegistry().Get("skill-version-chain")
	if updated.Version == "1.0" {
		t.Error("Version should have been incremented")
	}
	if len(updated.Evolution) < 2 {
		t.Errorf("Expected at least 2 evolution entries, got %d", len(updated.Evolution))
	}

	// Step 4: Rollback
	result = updateTool.Execute(context.Background(), map[string]interface{}{
		"id":               "skill-version-chain",
		"rollback_version": "1.0",
	})
	if result.IsError {
		t.Fatalf("Rollback failed: %s", result.ForLLM)
	}

	// Verify file content was rolled back
	data, _ := os.ReadFile(updated.Path)
	if !contains(string(data), "v1 content") {
		t.Error("File should contain v1 content after rollback")
	}
}

// IT: Create→Monitor→Deprecate lifecycle
func TestIT_CreateMonitorDeprecate(t *testing.T) {
	f, _ := newTestForge(t)

	// Create a skill with tool signature
	sig := []string{"read_file", "edit_file"}
	artifact, err := f.CreateSkill(context.Background(), "monitor-target", "content", "desc", sig)
	if err != nil {
		t.Fatalf("CreateSkill failed: %v", err)
	}

	// Manually set to active
	f.GetRegistry().Update(artifact.ID, func(a *forge.Artifact) {
		a.Status = forge.StatusActive
	})

	// Verify it's active
	active, _ := f.GetRegistry().Get(artifact.ID)
	if active.Status != forge.StatusActive {
		t.Fatal("Artifact should be active")
	}

	// Deprecate
	f.GetRegistry().Update(artifact.ID, func(a *forge.Artifact) {
		a.Status = forge.StatusDeprecated
	})

	// Verify deprecated
	deprecated, _ := f.GetRegistry().Get(artifact.ID)
	if deprecated.Status != forge.StatusDeprecated {
		t.Error("Artifact should be deprecated")
	}
}

// IT: Full reflection cycle with LLM
func TestIT_FullReflectionCycle(t *testing.T) {
	f, _ := newTestForge(t)
	cfg := f.GetConfig()
	cfg.Reflection.MinExperiences = 1
	cfg.Reflection.UseLLM = true

	// Create experience store and seed data
	store := forge.NewExperienceStore(f.GetWorkspace(), cfg)
	store.AppendAggregated(&forge.AggregatedExperience{
		PatternHash:   "sha256:full-1",
		ToolName:      "read_file",
		Count:         15,
		AvgDurationMs: 200,
		SuccessRate:   0.9,
		LastSeen:      time.Now().UTC(),
	})

	// Create a reflector with mock LLM
	reflector := forge.NewReflector(f.GetWorkspace(), store, f.GetRegistry(), cfg)
	reflector.SetProvider(&mockLLMProvider{
		chatFunc: func(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
			return &providers.LLMResponse{
				Content:      "Pattern analysis: read_file is most used. Suggest creating Skill.",
				FinishReason: "stop",
			}, nil
		},
	})

	reportPath, err := reflector.Reflect(context.Background(), "today", "all")
	if err != nil {
		t.Fatalf("Full reflection cycle failed: %v", err)
	}

	content, _ := os.ReadFile(reportPath)
	reportStr := string(content)

	if !contains(reportStr, "统计概要") {
		t.Error("Report should have statistical summary")
	}
	if !contains(reportStr, "LLM 深度分析") {
		t.Error("Report should have LLM insights section")
	}
}

// IT: MCP install/uninstall lifecycle
func TestIT_MCPInstallLifecycle(t *testing.T) {
	f, workspace := newTestForge(t)

	// Create MCP config dir
	configDir := filepath.Join(workspace, "config")
	os.MkdirAll(configDir, 0755)

	// Create MCP artifact via tool
	ftools := forge.NewForgeTools(f)
	createTool := findTool(t, ftools, "forge_create")

	result := createTool.Execute(context.Background(), map[string]interface{}{
		"type":        "mcp",
		"name":        "lifecycle-mcp",
		"content":     "from mcp.server import Server\nimport mcp\n\ndef main():\n    pass\n",
		"description": "IT lifecycle test MCP",
		"test_cases":  []interface{}{map[string]interface{}{"test": true}},
	})
	if result.IsError {
		t.Fatalf("Create MCP failed: %s", result.ForLLM)
	}

	// Verify file created
	mcpPath := filepath.Join(f.GetWorkspace(), "mcp", "lifecycle-mcp", "server.py")
	if _, err := os.Stat(mcpPath); os.IsNotExist(err) {
		t.Error("MCP server.py should exist")
	}

	// Install
	inst := f.GetMCPInstaller()
	artifact, _ := f.GetRegistry().Get("mcp-lifecycle-mcp")
	err := inst.Install(&artifact, filepath.Dir(mcpPath))
	if err != nil {
		t.Fatalf("Install failed: %v", err)
	}
	if !inst.IsInstalled("lifecycle-mcp") {
		t.Error("MCP should be installed")
	}

	// Uninstall
	err = inst.Uninstall("lifecycle-mcp")
	if err != nil {
		t.Fatalf("Uninstall failed: %v", err)
	}
	if inst.IsInstalled("lifecycle-mcp") {
		t.Error("MCP should not be installed after uninstall")
	}
}

// IT: Export an artifact
func TestIT_ExportArtifact(t *testing.T) {
	f, workspace := newTestForge(t)

	// Create a skill
	_, err := f.CreateSkill(context.Background(), "export-test", "export content", "desc", nil)
	if err != nil {
		t.Fatalf("CreateSkill failed: %v", err)
	}

	// Export to temp dir
	exportDir := filepath.Join(workspace, "exports")
	os.MkdirAll(exportDir, 0755)

	err = f.GetExporter().ExportArtifact("skill-export-test", exportDir)
	if err != nil {
		t.Fatalf("ExportArtifact failed: %v", err)
	}

	// Verify export directory was created
	exportedDir := filepath.Join(exportDir, "export-test-1.0")
	if _, err := os.Stat(exportedDir); os.IsNotExist(err) {
		t.Error("Export directory should exist")
	}

	// Verify SKILL.md was copied
	skillFile := filepath.Join(exportedDir, "SKILL.md")
	data, err := os.ReadFile(skillFile)
	if err != nil {
		t.Fatalf("Exported SKILL.md should be readable: %v", err)
	}
	if !contains(string(data), "export-test") {
		t.Error("Exported content should contain skill name")
	}
}

// IT: Trace collection and analysis chain
func TestIT_TraceAndAnalyze(t *testing.T) {
	f, _ := newTestForge(t)

	// Verify trace store is available (trace enabled by default)
	traceStore := f.GetTraceStore()
	if traceStore == nil {
		t.Skip("Trace store not available (trace may be disabled)")
	}

	// Write a trace using the Forge's trace store
	trace := &forge.ConversationTrace{
		TraceID:     "trace-it-001",
		SessionKey:  "hashed-session-key",
		Channel:     "test",
		StartTime:   time.Now().UTC().Add(-5 * time.Minute),
		EndTime:     time.Now().UTC(),
		DurationMs:  300000,
		TotalRounds: 3,
		ToolSteps: []forge.ToolStep{
			{ToolName: "read_file", ArgKeys: []string{"path"}, DurationMs: 100, Success: true},
			{ToolName: "edit_file", ArgKeys: []string{"path"}, DurationMs: 200, Success: true},
		},
	}

	err := traceStore.Append(trace)
	if err != nil {
		t.Fatalf("TraceStore.Append failed: %v", err)
	}

	// Read traces back from the same store
	traces, err := traceStore.ReadTraces(time.Now().UTC().AddDate(0, 0, -1))
	if err != nil {
		t.Fatalf("ReadTraces failed: %v", err)
	}
	if len(traces) == 0 {
		t.Error("Should read back at least 1 trace")
	}
}

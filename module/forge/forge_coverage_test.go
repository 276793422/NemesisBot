package forge

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/plugin"
)

// --- ForgePlugin tests ---

func TestNewForgePlugin(t *testing.T) {
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(t.TempDir(), cfg)
	collector := NewCollector(store, cfg)

	fp := NewForgePlugin(collector)
	if fp == nil {
		t.Fatal("NewForgePlugin returned nil")
	}
	if fp.Name() != "forge" {
		t.Errorf("Expected plugin name 'forge', got '%s'", fp.Name())
	}
}

func TestForgePlugin_Execute_Basic(t *testing.T) {
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(t.TempDir(), cfg)
	collector := NewCollector(store, cfg)
	fp := NewForgePlugin(collector)

	invocation := &plugin.ToolInvocation{
		ToolName: "test_tool",
		Args:     map[string]interface{}{"path": "/some/path"},
		Metadata: map[string]interface{}{"session_id": "sess-123"},
	}

	allowed, err, modified := fp.Execute(context.Background(), invocation)
	if !allowed {
		t.Error("ForgePlugin should always allow operations")
	}
	if err != nil {
		t.Errorf("ForgePlugin should not return error: %v", err)
	}
	if modified {
		t.Error("ForgePlugin should never modify operations")
	}
}

func TestForgePlugin_Execute_NoMetadata(t *testing.T) {
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(t.TempDir(), cfg)
	collector := NewCollector(store, cfg)
	fp := NewForgePlugin(collector)

	invocation := &plugin.ToolInvocation{
		ToolName: "test_tool",
		Args:     map[string]interface{}{"key": "value"},
	}

	allowed, _, _ := fp.Execute(context.Background(), invocation)
	if !allowed {
		t.Error("ForgePlugin should allow operations without metadata")
	}
}

func TestForgePlugin_Execute_SensitiveArgs(t *testing.T) {
	cfg := DefaultForgeConfig()
	cfg.Collection.SanitizeFields = []string{"password", "token"}
	store := NewExperienceStore(t.TempDir(), cfg)
	collector := NewCollector(store, cfg)
	fp := NewForgePlugin(collector)

	invocation := &plugin.ToolInvocation{
		ToolName: "login",
		Args: map[string]interface{}{
			"username": "user",
			"password": "secret123",
			"api_token": "tok-abc",
		},
	}

	allowed, _, _ := fp.Execute(context.Background(), invocation)
	if !allowed {
		t.Error("ForgePlugin should allow operations with sensitive args")
	}
}

// --- Collector InputChannel test ---

func TestCollector_InputChannel(t *testing.T) {
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(t.TempDir(), cfg)
	collector := NewCollector(store, cfg)

	ch := collector.InputChannel()
	if ch == nil {
		t.Error("InputChannel should not return nil")
	}
}

// --- Forge lifecycle tests ---

func TestForge_StartStop(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, err := NewForge(workspace, nil)
	if err != nil {
		t.Fatalf("NewForge failed: %v", err)
	}

	// Start and stop quickly
	f.Start()
	time.Sleep(50 * time.Millisecond)
	f.Stop()
}

func TestForge_StartDisabled(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, err := NewForge(workspace, nil)
	if err != nil {
		t.Fatalf("NewForge failed: %v", err)
	}

	// Disable collection
	f.config.Collection.Enabled = false
	f.Start()
	// Stop should work even if never started
	f.Stop()
}

// --- Forge SetProvider test ---

func TestForge_SetProvider(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, err := NewForge(workspace, nil)
	if err != nil {
		t.Fatalf("NewForge failed: %v", err)
	}

	// Should not panic with nil provider
	f.SetProvider(nil)
}

// --- Forge SetBridge test ---

func TestForge_SetBridge(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, err := NewForge(workspace, nil)
	if err != nil {
		t.Fatalf("NewForge failed: %v", err)
	}

	// Should not panic with nil bridge
	f.SetBridge(nil)
}

// --- Forge getter tests ---

func TestForge_Getters(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, err := NewForge(workspace, nil)
	if err != nil {
		t.Fatalf("NewForge failed: %v", err)
	}

	if f.GetCollector() == nil {
		t.Error("GetCollector should not return nil")
	}
	if f.GetReflector() == nil {
		t.Error("GetReflector should not return nil")
	}
	if f.GetPipeline() == nil {
		t.Error("GetPipeline should not return nil")
	}
	if f.GetMCPInstaller() == nil {
		t.Error("GetMCPInstaller should not return nil")
	}
	if f.GetExporter() == nil {
		t.Error("GetExporter should not return nil")
	}
	if f.GetSyncer() == nil {
		t.Error("GetSyncer should not return nil")
	}
	if f.GetConfig() == nil {
		t.Error("GetConfig should not return nil")
	}
	if f.GetWorkspace() == "" {
		t.Error("GetWorkspace should not return empty string")
	}
}

func TestForge_GetTraceComponents_Disabled(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	// Write config with trace and learning disabled
	forgeDir := filepath.Join(workspace, "forge")
	os.MkdirAll(forgeDir, 0755)
	configContent := `{
		"trace": {"enabled": false},
		"learning": {"enabled": false},
		"collection": {"enabled": false}
	}`
	os.WriteFile(filepath.Join(forgeDir, "forge.json"), []byte(configContent), 0644)

	f, err := NewForge(workspace, nil)
	if err != nil {
		t.Fatalf("NewForge failed: %v", err)
	}

	// Trace is disabled by default
	if f.GetTraceCollector() != nil {
		t.Error("GetTraceCollector should return nil when trace disabled")
	}
	if f.GetTraceStore() != nil {
		t.Error("GetTraceStore should return nil when trace disabled")
	}
	if f.GetLearningEngine() != nil {
		t.Error("GetLearningEngine should return nil when learning disabled")
	}
	if f.GetDeploymentMonitor() != nil {
		t.Error("GetDeploymentMonitor should return nil when learning disabled")
	}
	if f.GetCycleStore() != nil {
		t.Error("GetCycleStore should return nil when learning disabled")
	}
}

// --- Forge CreateSkill tests ---

func TestForge_CreateSkill(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, err := NewForge(workspace, nil)
	if err != nil {
		t.Fatalf("NewForge failed: %v", err)
	}

	artifact, err := f.CreateSkill(context.Background(), "my-skill", "# My Skill\n\nDo things", "A test skill", nil)
	if err != nil {
		t.Fatalf("CreateSkill failed: %v", err)
	}
	if artifact.ID != "skill-my-skill" {
		t.Errorf("Expected ID 'skill-my-skill', got '%s'", artifact.ID)
	}
	if artifact.Type != ArtifactSkill {
		t.Errorf("Expected type 'skill', got '%s'", artifact.Type)
	}
	if artifact.Name != "my-skill" {
		t.Errorf("Expected name 'my-skill', got '%s'", artifact.Name)
	}

	// Verify file exists
	if _, err := os.Stat(artifact.Path); os.IsNotExist(err) {
		t.Errorf("Artifact file should exist at %s", artifact.Path)
	}

	// Verify in registry
	found, ok := f.GetRegistry().Get("skill-my-skill")
	if !ok {
		t.Fatal("Artifact should be in registry")
	}
	if found.Name != "my-skill" {
		t.Errorf("Registry artifact name should be 'my-skill', got '%s'", found.Name)
	}
}

func TestForge_CreateSkill_AutoFrontmatter(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := NewForge(workspace, nil)

	// Content without frontmatter - should auto-generate
	artifact, err := f.CreateSkill(context.Background(), "auto-fm", "Just some content", "Auto frontmatter test", nil)
	if err != nil {
		t.Fatalf("CreateSkill failed: %v", err)
	}

	data, err := os.ReadFile(artifact.Path)
	if err != nil {
		t.Fatalf("Failed to read artifact: %v", err)
	}

	content := string(data)
	if !strings.Contains(content, "---") {
		t.Error("Auto-generated frontmatter should contain ---")
	}
	if !strings.Contains(content, "name: auto-fm") {
		t.Error("Auto-generated frontmatter should contain name")
	}
}

func TestForge_CreateSkill_WithToolSignature(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := NewForge(workspace, nil)

	sig := []string{"read_file", "edit_file"}
	_, err := f.CreateSkill(context.Background(), "sig-skill", "---\nname: sig\n---\nContent", "Sig test", sig)
	if err != nil {
		t.Fatalf("CreateSkill failed: %v", err)
	}

	found, ok := f.GetRegistry().Get("skill-sig-skill")
	if !ok {
		t.Fatal("Should be in registry")
	}
	if len(found.ToolSignature) != 2 {
		t.Errorf("Expected 2 tool signatures, got %d", len(found.ToolSignature))
	}
}

// --- Forge ReceiveReflection test ---

func TestForge_ReceiveReflection(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, err := NewForge(workspace, nil)
	if err != nil {
		t.Fatalf("NewForge failed: %v", err)
	}

	err = f.ReceiveReflection(map[string]interface{}{"test": "data"})
	// Should error because syncer has no bridge
	if err == nil {
		t.Error("ReceiveReflection should fail when syncer has no bridge")
	}
}

// --- Forge cleanupPromptSuggestions test ---

func TestForge_CleanupPromptSuggestions(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	promptsDir := filepath.Join(workspace, "prompts")
	os.MkdirAll(promptsDir, 0755)

	f, err := NewForge(workspace, nil)
	if err != nil {
		t.Fatalf("NewForge failed: %v", err)
	}

	// Create an old suggestion file
	oldFile := filepath.Join(promptsDir, "old_suggestion.md")
	os.WriteFile(oldFile, []byte("old content"), 0644)
	// Set modification time to 10 days ago
	oldTime := time.Now().AddDate(0, 0, -10)
	os.Chtimes(oldFile, oldTime, oldTime)

	// Create a new suggestion file
	newFile := filepath.Join(promptsDir, "new_suggestion.md")
	os.WriteFile(newFile, []byte("new content"), 0644)

	// Also create a non-suggestion file
	normalFile := filepath.Join(promptsDir, "normal.md")
	os.WriteFile(normalFile, []byte("normal content"), 0644)

	f.cleanupPromptSuggestions(7)

	// Old file should be removed
	if _, err := os.Stat(oldFile); !os.IsNotExist(err) {
		t.Error("Old suggestion file should have been removed")
	}
	// New file should still exist
	if _, err := os.Stat(newFile); os.IsNotExist(err) {
		t.Error("New suggestion file should still exist")
	}
	// Normal file should still exist
	if _, err := os.Stat(normalFile); os.IsNotExist(err) {
		t.Error("Normal file should still exist")
	}
}

// --- Forge with learning enabled ---

func TestForge_LearningEnabled(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	// Write config with learning enabled
	forgeDir := filepath.Join(workspace, "forge")
	os.MkdirAll(forgeDir, 0755)
	configContent := `{
		"trace": {"enabled": false},
		"learning": {"enabled": true},
		"collection": {"enabled": false}
	}`
	os.WriteFile(filepath.Join(forgeDir, "forge.json"), []byte(configContent), 0644)

	f, err := NewForge(workspace, nil)
	if err != nil {
		t.Fatalf("NewForge with learning failed: %v", err)
	}

	if f.GetLearningEngine() == nil {
		t.Error("LearningEngine should be initialized when learning enabled")
	}
	if f.GetDeploymentMonitor() == nil {
		t.Error("DeploymentMonitor should be initialized when learning enabled")
	}
	if f.GetCycleStore() == nil {
		t.Error("CycleStore should be initialized when learning enabled")
	}
	// Learning cascade: trace should also be enabled
	if f.GetTraceCollector() == nil {
		t.Error("TraceCollector should be initialized (cascade from learning)")
	}
	if f.GetTraceStore() == nil {
		t.Error("TraceStore should be initialized (cascade from learning)")
	}
}

// --- Reflector buildReport test ---

func TestReflector_BuildReport(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)
	registry := NewRegistry(tmpDir + "/registry.json")
	r := NewReflector(tmpDir, store, registry, cfg)

	stats := &ReflectionStats{
		TotalRecords:   10,
		UniquePatterns: 3,
		AvgSuccessRate: 0.85,
		ToolFrequency:  map[string]int{"tool1": 5, "tool2": 3, "tool3": 2},
	}

	report := r.buildReport(stats, nil, "today", "all", nil)
	if report == nil {
		t.Fatal("buildReport should not return nil")
	}
	if report.Period != "today" {
		t.Errorf("Expected period 'today', got '%s'", report.Period)
	}
	if report.Focus != "all" {
		t.Errorf("Expected focus 'all', got '%s'", report.Focus)
	}
	if report.Stats != stats {
		t.Error("Report stats should be the same object")
	}
}

// --- Reflector writeReport test ---

func TestReflector_WriteReport(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)
	registry := NewRegistry(tmpDir + "/registry.json")
	r := NewReflector(tmpDir, store, registry, cfg)

	report := &ReflectionReport{
		Date:   "2026-01-15",
		Period: "today",
		Focus:  "all",
		Stats: &ReflectionStats{
			TotalRecords:   5,
			UniquePatterns: 2,
			AvgSuccessRate: 0.90,
			ToolFrequency:  map[string]int{"read_file": 3, "edit_file": 2},
		},
	}

	path, err := r.writeReport(report)
	if err != nil {
		t.Fatalf("writeReport failed: %v", err)
	}
	if _, err := os.Stat(path); os.IsNotExist(err) {
		t.Errorf("Report file should exist at %s", path)
	}
}

// --- Reflector CleanupReports test ---

func TestReflector_CleanupReports(t *testing.T) {
	tmpDir := t.TempDir()
	reflectionsDir := filepath.Join(tmpDir, "reflections")
	os.MkdirAll(reflectionsDir, 0755)

	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)
	registry := NewRegistry(tmpDir + "/registry.json")
	r := NewReflector(tmpDir, store, registry, cfg)

	// Create an old report
	oldPath := filepath.Join(reflectionsDir, "2020-01-01.md")
	os.WriteFile(oldPath, []byte("old report"), 0644)

	// Create a new report
	now := time.Now().UTC()
	newPath := filepath.Join(reflectionsDir, now.Format("2006-01-02")+".md")
	os.WriteFile(newPath, []byte("new report"), 0644)

	err := r.CleanupReports(30)
	if err != nil {
		t.Fatalf("CleanupReports failed: %v", err)
	}

	if _, err := os.Stat(oldPath); !os.IsNotExist(err) {
		t.Error("Old report should be cleaned up")
	}
	if _, err := os.Stat(newPath); os.IsNotExist(err) {
		t.Error("New report should still exist")
	}
}

// --- Reflector CleanupReports nonexistent dir test ---

func TestReflector_CleanupReports_NonexistentDir(t *testing.T) {
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(t.TempDir(), cfg)
	registry := NewRegistry(t.TempDir() + "/registry.json")
	r := NewReflector("/nonexistent/path", store, registry, cfg)

	err := r.CleanupReports(30)
	if err != nil {
		t.Errorf("CleanupReports on nonexistent dir should return nil, got: %v", err)
	}
}

// --- Reflector GetLatestReport test ---

func TestReflector_GetLatestReport(t *testing.T) {
	tmpDir := t.TempDir()
	reflectionsDir := filepath.Join(tmpDir, "reflections")
	os.MkdirAll(reflectionsDir, 0755)

	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)
	registry := NewRegistry(tmpDir + "/registry.json")
	r := NewReflector(tmpDir, store, registry, cfg)

	// No reports yet
	_, err := r.GetLatestReport()
	if err == nil {
		t.Error("GetLatestReport should error when no reports exist")
	}

	// Create a report
	os.WriteFile(filepath.Join(reflectionsDir, "2026-01-15.md"), []byte("report 1"), 0644)
	os.WriteFile(filepath.Join(reflectionsDir, "2026-03-20.md"), []byte("report 2"), 0644)

	path, err := r.GetLatestReport()
	if err != nil {
		t.Fatalf("GetLatestReport failed: %v", err)
	}
	if !strings.HasSuffix(path, "2026-03-20.md") {
		t.Errorf("Expected latest report to be 2026-03-20.md, got %s", path)
	}
}

// --- Reflector MergeRemoteReflections test ---

func TestReflector_MergeRemoteReflections(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)
	registry := NewRegistry(tmpDir + "/registry.json")
	r := NewReflector(tmpDir, store, registry, cfg)

	result := r.MergeRemoteReflections(nil)
	if result == nil {
		t.Fatal("MergeRemoteReflections should not return nil")
	}
	if result.CommonTools == nil {
		t.Error("CommonTools map should be initialized")
	}
}

func TestReflector_MergeRemoteReflections_WithReports(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)
	registry := NewRegistry(tmpDir + "/registry.json")
	r := NewReflector(tmpDir, store, registry, cfg)

	// Create a remote report with a markdown table
	reportDir := filepath.Join(tmpDir, "remote")
	os.MkdirAll(reportDir, 0755)
	reportContent := `# Report

| Tool | Count |
|------|-------|
| read_file | 10 |
| edit_file | 5 |
`
	reportPath := filepath.Join(reportDir, "remote.md")
	os.WriteFile(reportPath, []byte(reportContent), 0644)

	result := r.MergeRemoteReflections([]string{reportPath})
	if result == nil {
		t.Fatal("MergeRemoteReflections should not return nil")
	}
}

// --- Reflector SetTraceStore and SetLearningEngine tests ---

func TestReflector_SetTraceStore(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)
	registry := NewRegistry(tmpDir + "/registry.json")
	r := NewReflector(tmpDir, store, registry, cfg)

	traceStore := NewTraceStore(tmpDir, cfg)
	r.SetTraceStore(traceStore)
	// Should not panic
}

func TestReflector_SetLearningEngine(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)
	registry := NewRegistry(tmpDir + "/registry.json")
	r := NewReflector(tmpDir, store, registry, cfg)

	le := NewLearningEngine(tmpDir, registry, nil, nil, nil, nil, cfg)
	r.SetLearningEngine(le)
	// Should not panic
}

// --- Registry List, Count, Delete tests ---

func TestRegistry_List(t *testing.T) {
	tmpDir := t.TempDir()
	registry := NewRegistry(filepath.Join(tmpDir, "registry.json"))

	registry.Add(Artifact{ID: "skill-1", Type: ArtifactSkill, Name: "s1", Status: StatusActive})
	registry.Add(Artifact{ID: "script-1", Type: ArtifactScript, Name: "sc1", Status: StatusDraft})
	registry.Add(Artifact{ID: "skill-2", Type: ArtifactSkill, Name: "s2", Status: StatusDraft})

	tests := []struct {
		name         string
		artifactType ArtifactType
		status       ArtifactStatus
		expected     int
	}{
		{"all", "", "", 3},
		{"skills only", ArtifactSkill, "", 2},
		{"scripts only", ArtifactScript, "", 1},
		{"active only", "", StatusActive, 1},
		{"active skills", ArtifactSkill, StatusActive, 1},
		{"draft skills", ArtifactSkill, StatusDraft, 1},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := registry.List(tt.artifactType, tt.status)
			if len(result) != tt.expected {
				t.Errorf("List(%s, %s): expected %d, got %d", tt.artifactType, tt.status, tt.expected, len(result))
			}
		})
	}
}

func TestRegistry_Count(t *testing.T) {
	tmpDir := t.TempDir()
	registry := NewRegistry(filepath.Join(tmpDir, "registry.json"))

	registry.Add(Artifact{ID: "skill-1", Type: ArtifactSkill, Name: "s1"})
	registry.Add(Artifact{ID: "skill-2", Type: ArtifactSkill, Name: "s2"})
	registry.Add(Artifact{ID: "script-1", Type: ArtifactScript, Name: "sc1"})

	if count := registry.Count(""); count != 3 {
		t.Errorf("Count all: expected 3, got %d", count)
	}
	if count := registry.Count(ArtifactSkill); count != 2 {
		t.Errorf("Count skills: expected 2, got %d", count)
	}
	if count := registry.Count(ArtifactScript); count != 1 {
		t.Errorf("Count scripts: expected 1, got %d", count)
	}
	if count := registry.Count(ArtifactMCP); count != 0 {
		t.Errorf("Count mcp: expected 0, got %d", count)
	}
}

func TestRegistry_Delete(t *testing.T) {
	tmpDir := t.TempDir()
	registry := NewRegistry(filepath.Join(tmpDir, "registry.json"))

	registry.Add(Artifact{ID: "skill-1", Type: ArtifactSkill, Name: "s1"})
	registry.Add(Artifact{ID: "skill-2", Type: ArtifactSkill, Name: "s2"})

	err := registry.Delete("skill-1")
	if err != nil {
		t.Fatalf("Delete failed: %v", err)
	}

	_, found := registry.Get("skill-1")
	if found {
		t.Error("Artifact should be deleted")
	}

	// Delete nonexistent should not error
	err = registry.Delete("nonexistent")
	if err != nil {
		t.Errorf("Delete nonexistent should not error: %v", err)
	}
}

// --- Pipeline tests ---

func TestPipeline_SetProvider(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(filepath.Join(tmpDir, "registry.json"))
	p := NewPipeline(registry, cfg)

	p.SetProvider(nil)
	// Should not panic
}

func TestPipeline_Run_NotFound(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(filepath.Join(tmpDir, "registry.json"))
	p := NewPipeline(registry, cfg)

	_, err := p.Run(context.Background(), "nonexistent")
	if err == nil {
		t.Error("Run should error for nonexistent artifact")
	}
}

func TestPipeline_Run_FileNotFound(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(filepath.Join(tmpDir, "registry.json"))
	p := NewPipeline(registry, cfg)

	registry.Add(Artifact{ID: "skill-1", Type: ArtifactSkill, Name: "s1", Path: "/nonexistent/SKILL.md"})

	_, err := p.Run(context.Background(), "skill-1")
	if err == nil {
		t.Error("Run should error when file not found")
	}
}

func TestPipeline_DetermineStatus_AllCases(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(filepath.Join(tmpDir, "registry.json"))
	p := NewPipeline(registry, cfg)

	tests := []struct {
		name       string
		validation *ArtifactValidation
		expected   ArtifactStatus
	}{
		{
			"nil validation",
			nil,
			StatusDraft,
		},
		{
			"stage1 failed",
			&ArtifactValidation{Stage1Static: &StaticValidationResult{ValidationStage: ValidationStage{Passed: false}}},
			StatusDraft,
		},
		{
			"stage2 failed",
			&ArtifactValidation{
				Stage1Static:     &StaticValidationResult{ValidationStage: ValidationStage{Passed: true}},
				Stage2Functional: &FunctionalValidationResult{ValidationStage: ValidationStage{Passed: false}},
			},
			StatusDraft,
		},
		{
			"no stage3",
			&ArtifactValidation{
				Stage1Static:     &StaticValidationResult{ValidationStage: ValidationStage{Passed: true}},
				Stage2Functional: &FunctionalValidationResult{ValidationStage: ValidationStage{Passed: true}},
			},
			StatusTesting,
		},
		{
			"stage3 high score",
			&ArtifactValidation{
				Stage1Static:     &StaticValidationResult{ValidationStage: ValidationStage{Passed: true}},
				Stage2Functional: &FunctionalValidationResult{ValidationStage: ValidationStage{Passed: true}},
				Stage3Quality:    &QualityValidationResult{ValidationStage: ValidationStage{Passed: true}, Score: 85},
			},
			StatusActive,
		},
		{
			"stage3 medium score",
			&ArtifactValidation{
				Stage1Static:     &StaticValidationResult{ValidationStage: ValidationStage{Passed: true}},
				Stage2Functional: &FunctionalValidationResult{ValidationStage: ValidationStage{Passed: true}},
				Stage3Quality:    &QualityValidationResult{ValidationStage: ValidationStage{Passed: true}, Score: 65},
			},
			StatusActive,
		},
		{
			"stage3 low score",
			&ArtifactValidation{
				Stage1Static:     &StaticValidationResult{ValidationStage: ValidationStage{Passed: true}},
				Stage2Functional: &FunctionalValidationResult{ValidationStage: ValidationStage{Passed: true}},
				Stage3Quality:    &QualityValidationResult{ValidationStage: ValidationStage{Passed: false}, Score: 40},
			},
			StatusDraft,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := p.DetermineStatus(tt.validation)
			if result != tt.expected {
				t.Errorf("DetermineStatus(%s): expected %s, got %s", tt.name, tt.expected, result)
			}
		})
	}
}

// --- MCPInstaller tests ---

func TestMCPInstaller_InstallAndUninstall(t *testing.T) {
	tmpDir := t.TempDir()
	configDir := filepath.Join(tmpDir, "config")
	os.MkdirAll(configDir, 0755)

	inst := NewMCPInstaller(tmpDir)
	artifact := &Artifact{
		Name:    "test-mcp",
		Type:    ArtifactMCP,
		Version: "1.0",
	}

	// Create mcp directory with a Python server
	mcpDir := filepath.Join(tmpDir, "mcp", "test-mcp")
	os.MkdirAll(mcpDir, 0755)
	os.WriteFile(filepath.Join(mcpDir, "server.py"), []byte("# server"), 0644)

	err := inst.Install(artifact, mcpDir)
	if err != nil {
		t.Fatalf("Install failed: %v", err)
	}

	// Check it's installed
	if !inst.IsInstalled("test-mcp") {
		t.Error("MCP should be installed")
	}

	// Uninstall
	err = inst.Uninstall("test-mcp")
	if err != nil {
		t.Fatalf("Uninstall failed: %v", err)
	}

	// Check it's no longer installed
	if inst.IsInstalled("test-mcp") {
		t.Error("MCP should be uninstalled")
	}
}

func TestMCPInstaller_IsInstalled_NotInstalled(t *testing.T) {
	tmpDir := t.TempDir()
	inst := NewMCPInstaller(tmpDir)

	if inst.IsInstalled("nonexistent") {
		t.Error("nonexistent MCP should not be installed")
	}
}

func TestMCPInstaller_BuildCommand(t *testing.T) {
	tmpDir := t.TempDir()
	inst := NewMCPInstaller(tmpDir)

	tests := []struct {
		name          string
		setupDir      func(dir string)
		expectedCmd   string
		expectedArgsC int
	}{
		{
			"python server",
			func(dir string) {
				os.MkdirAll(dir, 0755)
				os.WriteFile(filepath.Join(dir, "server.py"), []byte("# py"), 0644)
			},
			"uv",
			4,
		},
		{
			"go main",
			func(dir string) {
				os.MkdirAll(dir, 0755)
				os.WriteFile(filepath.Join(dir, "main.go"), []byte("// go"), 0644)
			},
			"go",
			2,
		},
		{
			"fallback .go suffix",
			func(dir string) {
				os.MkdirAll(dir, 0755)
			},
			"go",
			2,
		},
		{
			"default python",
			func(dir string) {
				os.MkdirAll(dir, 0755)
			},
			"uv",
			4,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			dir := filepath.Join(tmpDir, tt.name)
			tt.setupDir(dir)

			mcpDir := dir
			if tt.name == "fallback .go suffix" {
				mcpDir = dir + "/server.go"
			}

			cmd, args := inst.buildCommand("test", mcpDir)
			if cmd != tt.expectedCmd {
				t.Errorf("Expected cmd '%s', got '%s'", tt.expectedCmd, cmd)
			}
			if len(args) != tt.expectedArgsC {
				t.Errorf("Expected %d args, got %d", tt.expectedArgsC, len(args))
			}
		})
	}
}

// --- Evaluator tests ---

func TestEvaluator_SetProvider(t *testing.T) {
	cfg := DefaultForgeConfig()
	e := NewQualityEvaluator(nil, cfg)
	e.SetProvider(nil)
	// Should not panic
}

func TestEvaluator_Evaluate_NoProvider(t *testing.T) {
	cfg := DefaultForgeConfig()
	e := NewQualityEvaluator(nil, cfg)

	artifact := &Artifact{
		Type:    ArtifactSkill,
		Name:    "test",
		Version: "1.0",
	}

	result := e.Evaluate(context.Background(), artifact, "test content")
	if result == nil {
		t.Fatal("Evaluate should return a result even without provider")
	}
	if !result.Passed {
		t.Error("Default evaluation should pass")
	}
	if result.Score != 70 {
		t.Errorf("Default score should be 70, got %d", result.Score)
	}
}

// --- Reflector SetProvider test ---

func TestReflector_SetProvider(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)
	registry := NewRegistry(tmpDir + "/registry.json")
	r := NewReflector(tmpDir, store, registry, cfg)

	r.SetProvider(nil)
	// Should not panic
}

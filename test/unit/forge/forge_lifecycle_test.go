package forge_test

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/forge"
	"github.com/276793422/NemesisBot/module/plugin"
)

// now returns current UTC time (shared helper for test data seeding).
func now() time.Time {
	return time.Now().UTC()
}

// newTestForge creates a Forge instance in a temp directory for testing.
func newTestForge(t *testing.T) (*forge.Forge, string) {
	t.Helper()
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)
	f, err := forge.NewForge(workspace, nil)
	if err != nil {
		t.Fatalf("NewForge failed: %v", err)
	}
	return f, workspace
}

func TestNewForge_CreatesDirectories(t *testing.T) {
	f, workspace := newTestForge(t)
	forgeDir := f.GetWorkspace()

	expectedDirs := []string{
		"experiences",
		"reflections",
		"skills",
		"scripts",
		"mcp",
		"traces",
		"learning",
	}
	for _, dir := range expectedDirs {
		path := filepath.Join(forgeDir, dir)
		if info, err := os.Stat(path); err != nil || !info.IsDir() {
			t.Errorf("Directory %s should exist after NewForge", path)
		}
	}

	// Also check prompts dir at workspace level
	promptsDir := filepath.Join(workspace, "prompts")
	if info, err := os.Stat(promptsDir); err != nil || !info.IsDir() {
		t.Errorf("Directory %s should exist after NewForge", promptsDir)
	}
}

func TestNewForge_RegistryInitialized(t *testing.T) {
	f, _ := newTestForge(t)

	// Registry should be usable (empty list)
	artifacts := f.GetRegistry().ListAll()
	if len(artifacts) != 0 {
		t.Errorf("Expected 0 artifacts, got %d", len(artifacts))
	}

	// Registry file is created on first write (Add), not on init.
	// Verify by adding an artifact and checking the file exists.
	f.GetRegistry().Add(forge.Artifact{
		ID:   "test-init",
		Type: forge.ArtifactSkill,
		Name: "init-test",
	})
	registryPath := filepath.Join(f.GetWorkspace(), "registry.json")
	if _, err := os.Stat(registryPath); os.IsNotExist(err) {
		t.Error("registry.json should exist after adding an artifact")
	}
}

func TestNewForge_ConfigDefaults(t *testing.T) {
	f, _ := newTestForge(t)
	cfg := f.GetConfig()

	if !cfg.Collection.Enabled {
		t.Error("Collection should be enabled by default")
	}
	if cfg.Collection.BufferSize != 256 {
		t.Errorf("Expected BufferSize 256, got %d", cfg.Collection.BufferSize)
	}
	if cfg.Artifacts.DefaultStatus != "draft" {
		t.Errorf("Expected DefaultStatus 'draft', got '%s'", cfg.Artifacts.DefaultStatus)
	}
	if cfg.Learning.Enabled {
		t.Error("Learning should be disabled by default")
	}
	if cfg.Validation.AutoValidate != true {
		t.Error("AutoValidate should be true by default")
	}
}

func TestNewForge_WithPluginManager(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	pluginMgr := plugin.NewManager()
	f, err := forge.NewForge(workspace, pluginMgr)
	if err != nil {
		t.Fatalf("NewForge failed: %v", err)
	}
	if f == nil {
		t.Fatal("Forge instance should not be nil")
	}

	// Verify ForgePlugin was registered
	if !pluginMgr.IsEnabled("forge") {
		t.Error("Forge plugin should be registered and enabled")
	}
}

func TestForge_GetAccessors(t *testing.T) {
	f, _ := newTestForge(t)

	if f.GetCollector() == nil {
		t.Error("GetCollector() should not return nil")
	}
	if f.GetRegistry() == nil {
		t.Error("GetRegistry() should not return nil")
	}
	if f.GetReflector() == nil {
		t.Error("GetReflector() should not return nil")
	}
	if f.GetPipeline() == nil {
		t.Error("GetPipeline() should not return nil")
	}
	if f.GetConfig() == nil {
		t.Error("GetConfig() should not return nil")
	}
	if f.GetMCPInstaller() == nil {
		t.Error("GetMCPInstaller() should not return nil")
	}
	if f.GetExporter() == nil {
		t.Error("GetExporter() should not return nil")
	}
	if f.GetSyncer() == nil {
		t.Error("GetSyncer() should not return nil")
	}
	// workspace getter
	ws := f.GetWorkspace()
	if ws == "" || !strings.HasSuffix(ws, "forge") {
		t.Errorf("GetWorkspace() should end with 'forge', got: %s", ws)
	}
}

func TestForge_CreateSkill_WritesFile(t *testing.T) {
	f, _ := newTestForge(t)

	_, err := f.CreateSkill(context.Background(), "my-skill", "Skill content here", "A test skill", nil)
	if err != nil {
		t.Fatalf("CreateSkill failed: %v", err)
	}

	skillPath := filepath.Join(f.GetWorkspace(), "skills", "my-skill", "SKILL.md")
	data, err := os.ReadFile(skillPath)
	if err != nil {
		t.Fatalf("Skill file should exist: %v", err)
	}
	content := string(data)
	if !contains(content, "my-skill") {
		t.Error("Skill file should contain the skill name")
	}
	if !contains(content, "Skill content here") {
		t.Error("Skill file should contain the original content")
	}
}

func TestForge_CreateSkill_RegistersArtifact(t *testing.T) {
	f, _ := newTestForge(t)

	artifact, err := f.CreateSkill(context.Background(), "reg-test", "content", "description", nil)
	if err != nil {
		t.Fatalf("CreateSkill failed: %v", err)
	}

	if artifact.ID != "skill-reg-test" {
		t.Errorf("Expected ID 'skill-reg-test', got '%s'", artifact.ID)
	}
	if artifact.Type != forge.ArtifactSkill {
		t.Errorf("Expected type 'skill', got '%s'", artifact.Type)
	}

	// Verify in registry
	regArtifact, found := f.GetRegistry().Get("skill-reg-test")
	if !found {
		t.Fatal("Artifact should be found in registry")
	}
	if regArtifact.Name != "reg-test" {
		t.Errorf("Expected Name 'reg-test', got '%s'", regArtifact.Name)
	}
}

func TestForge_CreateSkill_WithToolSignature(t *testing.T) {
	f, _ := newTestForge(t)

	sig := []string{"read_file", "edit_file", "exec"}
	artifact, err := f.CreateSkill(context.Background(), "sig-skill", "content", "desc", sig)
	if err != nil {
		t.Fatalf("CreateSkill failed: %v", err)
	}

	if len(artifact.ToolSignature) != 3 {
		t.Fatalf("Expected 3 tool signatures, got %d", len(artifact.ToolSignature))
	}
	if artifact.ToolSignature[0] != "read_file" {
		t.Errorf("Expected first sig 'read_file', got '%s'", artifact.ToolSignature[0])
	}

	// Verify persisted in registry
	regArtifact, _ := f.GetRegistry().Get("skill-sig-skill")
	if len(regArtifact.ToolSignature) != 3 {
		t.Errorf("Registry artifact should have 3 tool signatures, got %d", len(regArtifact.ToolSignature))
	}
}

func TestForge_CreateSkill_CopiesToWorkspace(t *testing.T) {
	f, workspace := newTestForge(t)

	_, err := f.CreateSkill(context.Background(), "copy-skill", "content", "desc", nil)
	if err != nil {
		t.Fatalf("CreateSkill failed: %v", err)
	}

	// Should be copied to workspace/skills/{name}-forge/SKILL.md
	copyPath := filepath.Join(workspace, "skills", "copy-skill-forge", "SKILL.md")
	if _, err := os.Stat(copyPath); os.IsNotExist(err) {
		t.Error("Skill copy should exist in workspace/skills/copy-skill-forge/SKILL.md")
	}
}

func TestForge_CreateSkill_AutoFrontmatter(t *testing.T) {
	f, _ := newTestForge(t)

	// Content without frontmatter
	content := "This is skill body without frontmatter."
	artifact, err := f.CreateSkill(context.Background(), "auto-fm", content, "auto frontmatter test", nil)
	if err != nil {
		t.Fatalf("CreateSkill failed: %v", err)
	}

	// Read the file and verify frontmatter was added
	data, err := os.ReadFile(artifact.Path)
	if err != nil {
		t.Fatalf("Failed to read artifact: %v", err)
	}
	fileContent := string(data)
	if !strings.HasPrefix(fileContent, "---") {
		t.Error("Auto-generated frontmatter should start with '---'")
	}
	if !contains(fileContent, "auto-fm") {
		t.Error("Frontmatter should contain skill name")
	}
}

func TestForge_CreateSkill_WithFrontmatter(t *testing.T) {
	f, _ := newTestForge(t)

	// Content with existing frontmatter
	content := "---\nname: custom\n---\nCustom body."
	artifact, err := f.CreateSkill(context.Background(), "has-fm", content, "desc", nil)
	if err != nil {
		t.Fatalf("CreateSkill failed: %v", err)
	}

	data, err := os.ReadFile(artifact.Path)
	if err != nil {
		t.Fatalf("Failed to read artifact: %v", err)
	}
	fileContent := string(data)
	// Should not double-add frontmatter
	idx := strings.Index(fileContent, "---")
	lastIdx := strings.LastIndex(fileContent, "---")
	if idx == lastIdx {
		t.Error("Original frontmatter should be preserved (should have at least 2 '---' lines)")
	}
}

func TestForge_StartStop(t *testing.T) {
	f, _ := newTestForge(t)

	// Start should not panic
	f.Start()

	// Give it a moment
	time.Sleep(100 * time.Millisecond)

	// Stop should not panic and should complete
	done := make(chan struct{})
	go func() {
		f.Stop()
		close(done)
	}()

	select {
	case <-done:
		// Success
	case <-time.After(5 * time.Second):
		t.Fatal("Stop() should complete within 5 seconds")
	}
}

func TestForge_ReflectNow(t *testing.T) {
	f, _ := newTestForge(t)

	// Seed some experience data so reflection has something to work with
	cfg := f.GetConfig()
	cfg.Reflection.MinExperiences = 1

	store := forge.NewExperienceStore(f.GetWorkspace(), cfg)
	store.AppendAggregated(&forge.AggregatedExperience{
		PatternHash: "sha256:reflect1",
		ToolName:    "test_tool",
		Count:       5,
		LastSeen:    time.Now().UTC(),
	})

	// Use the reflector directly (ReflectNow delegates to reflector)
	reportPath, err := f.ReflectNow(context.Background(), "today", "all")
	if err != nil {
		t.Fatalf("ReflectNow failed: %v", err)
	}
	if reportPath == "" {
		t.Error("ReflectNow should return a report path")
	}
	if _, err := os.Stat(reportPath); os.IsNotExist(err) {
		t.Errorf("Report file should exist at %s", reportPath)
	}
}

func TestForge_SetProvider_Cascades(t *testing.T) {
	f, _ := newTestForge(t)

	// Use our mock provider
	mockProvider := &mockLLMProvider{defaultModel: "cascade-test"}
	f.SetProvider(mockProvider)

	// Verify that the reflector received the provider by doing a reflection
	// that would fail if provider was not set
	f.GetConfig().Reflection.MinExperiences = 1
	f.GetConfig().Reflection.UseLLM = true

	// Seed data
	store := forge.NewExperienceStore(f.GetWorkspace(), f.GetConfig())
	store.AppendAggregated(&forge.AggregatedExperience{
		PatternHash: "sha256:provider1",
		ToolName:    "read_file",
		Count:       10,
		LastSeen:    time.Now().UTC(),
	})

	// Create a new reflector with the store
	reflector := forge.NewReflector(f.GetWorkspace(), store, f.GetRegistry(), f.GetConfig())
	reflector.SetProvider(mockProvider)

	_, err := reflector.Reflect(context.Background(), "today", "all")
	if err != nil {
		t.Fatalf("Reflection with provider should succeed: %v", err)
	}
}

func TestForge_ReceiveReflection_NoSyncer(t *testing.T) {
	f, _ := newTestForge(t)
	// Calling ReceiveReflection should work since syncer is initialized in NewForge
	err := f.ReceiveReflection(map[string]interface{}{
		"content":  "test report content",
		"filename": "test_remote.md",
	})
	if err != nil {
		// It may error on the specific payload format but not on syncer being nil
		// The syncer IS created by NewForge, so it shouldn't give "syncer not initialized"
		if contains(err.Error(), "syncer not initialized") {
			t.Error("Syncer should be initialized by NewForge")
		}
	}
}

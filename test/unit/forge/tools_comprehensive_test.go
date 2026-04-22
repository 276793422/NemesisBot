package forge_test

import (
	"context"
	"os"
	"path/filepath"
	"testing"

	"github.com/276793422/NemesisBot/module/forge"
	"github.com/276793422/NemesisBot/module/tools"
)

// findTool finds a tool by name in the tool list.
func findTool(t *testing.T, toolList []tools.Tool, name string) tools.Tool {
	t.Helper()
	for _, tl := range toolList {
		if tl.Name() == name {
			return tl
		}
	}
	t.Fatalf("Tool '%s' not found", name)
	return nil
}

func TestForgeReflectTool_Success(t *testing.T) {
	f, _ := newTestForge(t)

	// Seed data so reflection has something to analyze
	cfg := f.GetConfig()
	cfg.Reflection.MinExperiences = 1
	store := forge.NewExperienceStore(f.GetWorkspace(), cfg)
	store.AppendAggregated(&forge.AggregatedExperience{
		PatternHash: "sha256:reflect-tool-1",
		ToolName:    "test_tool",
		Count:       5,
		LastSeen:    now(),
	})

	ftools := forge.NewForgeTools(f)
	tool := findTool(t, ftools, "forge_reflect")

	result := tool.Execute(context.Background(), map[string]interface{}{})
	if result.IsError {
		t.Errorf("forge_reflect should succeed: %s", result.ForLLM)
	}
	if !contains(result.ForLLM, "反思报告") {
		t.Errorf("Result should contain '反思报告', got: %s", result.ForLLM)
	}
}

func TestForgeReflectTool_WithPeriod(t *testing.T) {
	f, _ := newTestForge(t)

	cfg := f.GetConfig()
	cfg.Reflection.MinExperiences = 1
	store := forge.NewExperienceStore(f.GetWorkspace(), cfg)
	store.AppendAggregated(&forge.AggregatedExperience{
		PatternHash: "sha256:period-test",
		ToolName:    "test_tool",
		Count:       3,
		LastSeen:    now(),
	})

	ftools := forge.NewForgeTools(f)
	tool := findTool(t, ftools, "forge_reflect")

	result := tool.Execute(context.Background(), map[string]interface{}{
		"period": "week",
		"focus":  "skill",
	})
	if result.IsError {
		t.Errorf("forge_reflect with period should succeed: %s", result.ForLLM)
	}
}

func TestForgeCreateTool_SkillType(t *testing.T) {
	f, _ := newTestForge(t)
	ftools := forge.NewForgeTools(f)
	tool := findTool(t, ftools, "forge_create")

	result := tool.Execute(context.Background(), map[string]interface{}{
		"type":        "skill",
		"name":        "test-skill-create",
		"content":     "Skill content for testing",
		"description": "A test skill created via tool",
	})
	if result.IsError {
		t.Fatalf("forge_create skill should succeed: %s", result.ForLLM)
	}
	if !contains(result.ForLLM, "skill") {
		t.Errorf("Result should mention 'skill', got: %s", result.ForLLM)
	}
	if !contains(result.ForLLM, "test-skill-create") {
		t.Errorf("Result should mention artifact name, got: %s", result.ForLLM)
	}

	// Verify registry
	_, found := f.GetRegistry().Get("skill-test-skill-create")
	if !found {
		t.Error("Skill should be registered in registry")
	}
}

func TestForgeCreateTool_ScriptType(t *testing.T) {
	f, _ := newTestForge(t)
	ftools := forge.NewForgeTools(f)
	tool := findTool(t, ftools, "forge_create")

	result := tool.Execute(context.Background(), map[string]interface{}{
		"type":        "script",
		"name":        "test-script",
		"content":     "echo hello",
		"description": "A test script",
		"test_cases":  []interface{}{map[string]interface{}{"input": "test"}},
	})
	if result.IsError {
		t.Fatalf("forge_create script should succeed: %s", result.ForLLM)
	}
	if !contains(result.ForLLM, "script") {
		t.Errorf("Result should mention 'script', got: %s", result.ForLLM)
	}

	// Verify file was created
	scriptPath := filepath.Join(f.GetWorkspace(), "scripts", "utils", "test-script")
	if _, err := os.Stat(scriptPath); os.IsNotExist(err) {
		t.Error("Script file should exist")
	}
}

func TestForgeCreateTool_MissingRequiredArgs(t *testing.T) {
	f, _ := newTestForge(t)
	ftools := forge.NewForgeTools(f)
	tool := findTool(t, ftools, "forge_create")

	tests := []struct {
		name string
		args map[string]interface{}
	}{
		{"missing type", map[string]interface{}{"name": "x", "content": "c"}},
		{"missing name", map[string]interface{}{"type": "skill", "content": "c"}},
		{"missing content", map[string]interface{}{"type": "skill", "name": "x"}},
		{"empty all", map[string]interface{}{}},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := tool.Execute(context.Background(), tt.args)
			if !result.IsError {
				t.Error("Should error when required args are missing")
			}
		})
	}
}

func TestForgeUpdateTool_UpdateContent(t *testing.T) {
	f, _ := newTestForge(t)

	// Create a skill first
	_, err := f.CreateSkill(context.Background(), "update-test", "original content", "desc", nil)
	if err != nil {
		t.Fatalf("CreateSkill failed: %v", err)
	}

	ftools := forge.NewForgeTools(f)
	tool := findTool(t, ftools, "forge_update")

	result := tool.Execute(context.Background(), map[string]interface{}{
		"id":                 "skill-update-test",
		"content":            "updated content here",
		"change_description": "Updated for testing",
	})
	if result.IsError {
		t.Fatalf("forge_update should succeed: %s", result.ForLLM)
	}
	if !contains(result.ForLLM, "已更新") {
		t.Errorf("Result should say updated, got: %s", result.ForLLM)
	}

	// Verify version was incremented
	artifact, _ := f.GetRegistry().Get("skill-update-test")
	if artifact.Version == "1.0" {
		t.Error("Version should be incremented after update")
	}

	// Verify file content changed
	data, _ := os.ReadFile(artifact.Path)
	if !contains(string(data), "updated content here") {
		t.Error("File content should be updated")
	}
}

func TestForgeUpdateTool_NotFound(t *testing.T) {
	f, _ := newTestForge(t)
	ftools := forge.NewForgeTools(f)
	tool := findTool(t, ftools, "forge_update")

	result := tool.Execute(context.Background(), map[string]interface{}{
		"id":      "skill-nonexistent",
		"content": "whatever",
	})
	if !result.IsError {
		t.Error("Should error on non-existent artifact")
	}
	if !contains(result.ForLLM, "不存在") {
		t.Errorf("Error should mention non-existence, got: %s", result.ForLLM)
	}
}

func TestForgeUpdateTool_RollbackVersion(t *testing.T) {
	f, _ := newTestForge(t)

	// Create and update a skill to create version history
	artifact, err := f.CreateSkill(context.Background(), "rollback-test", "v1 content", "desc", nil)
	if err != nil {
		t.Fatalf("CreateSkill failed: %v", err)
	}

	// Save version snapshot manually
	forge.SaveVersionSnapshot(artifact.Path, artifact.Version)

	// Update content
	ftools := forge.NewForgeTools(f)
	updateTool := findTool(t, ftools, "forge_update")
	updateTool.Execute(context.Background(), map[string]interface{}{
		"id":                 "skill-rollback-test",
		"content":            "v2 content",
		"change_description": "Update to v2",
	})

	// Now rollback to v1
	result := updateTool.Execute(context.Background(), map[string]interface{}{
		"id":               "skill-rollback-test",
		"rollback_version": "1.0",
	})
	if result.IsError {
		t.Fatalf("Rollback should succeed: %s", result.ForLLM)
	}
	if !contains(result.ForLLM, "回滚") {
		t.Errorf("Result should mention rollback, got: %s", result.ForLLM)
	}
}

func TestForgeUpdateTool_MissingID(t *testing.T) {
	f, _ := newTestForge(t)
	ftools := forge.NewForgeTools(f)
	tool := findTool(t, ftools, "forge_update")

	result := tool.Execute(context.Background(), map[string]interface{}{
		"content": "some content",
	})
	if !result.IsError {
		t.Error("Should error when id is missing")
	}
}

func TestForgeUpdateTool_NoContentNoRollback(t *testing.T) {
	f, _ := newTestForge(t)

	// Create a skill first
	f.CreateSkill(context.Background(), "nocontent-test", "content", "desc", nil)

	ftools := forge.NewForgeTools(f)
	tool := findTool(t, ftools, "forge_update")

	result := tool.Execute(context.Background(), map[string]interface{}{
		"id": "skill-nocontent-test",
	})
	if !result.IsError {
		t.Error("Should error when neither content nor rollback_version provided")
	}
}

func TestForgeEvaluateTool_Success(t *testing.T) {
	f, _ := newTestForge(t)

	// Create a skill with valid content
	artifact, err := f.CreateSkill(context.Background(), "eval-test",
		"---\nname: eval-test\n---\n\nValid skill content with proper structure.", "desc", nil)
	if err != nil {
		t.Fatalf("CreateSkill failed: %v", err)
	}

	ftools := forge.NewForgeTools(f)
	tool := findTool(t, ftools, "forge_evaluate")

	result := tool.Execute(context.Background(), map[string]interface{}{
		"id": artifact.ID,
	})
	if result.IsError {
		t.Errorf("forge_evaluate should succeed: %s", result.ForLLM)
	}
	if !contains(result.ForLLM, "评估") {
		t.Errorf("Result should mention evaluation, got: %s", result.ForLLM)
	}
}

func TestForgeEvaluateTool_NotFound(t *testing.T) {
	f, _ := newTestForge(t)
	ftools := forge.NewForgeTools(f)
	tool := findTool(t, ftools, "forge_evaluate")

	result := tool.Execute(context.Background(), map[string]interface{}{
		"id": "skill-nonexistent",
	})
	if !result.IsError {
		t.Error("Should error on non-existent artifact")
	}
}

func TestForgeEvaluateTool_MissingID(t *testing.T) {
	f, _ := newTestForge(t)
	ftools := forge.NewForgeTools(f)
	tool := findTool(t, ftools, "forge_evaluate")

	result := tool.Execute(context.Background(), map[string]interface{}{})
	if !result.IsError {
		t.Error("Should error when id is missing")
	}
}

func TestForgeListTool_ByType(t *testing.T) {
	f, _ := newTestForge(t)

	// Create a skill
	f.CreateSkill(context.Background(), "list-skill", "content", "desc", nil)

	// Manually add a script artifact
	f.GetRegistry().Add(forge.Artifact{
		ID:     "script-list-script",
		Type:   forge.ArtifactScript,
		Name:   "list-script",
		Status: forge.StatusDraft,
		Path:   "/tmp/fake",
	})

	ftools := forge.NewForgeTools(f)
	tool := findTool(t, ftools, "forge_list")

	// Filter by skill type
	result := tool.Execute(context.Background(), map[string]interface{}{
		"type": "skill",
	})
	if result.IsError {
		t.Errorf("forge_list should not error: %s", result.ForLLM)
	}
	if !contains(result.ForLLM, "list-skill") {
		t.Errorf("Result should contain 'list-skill', got: %s", result.ForLLM)
	}
	if contains(result.ForLLM, "list-script") {
		t.Error("Result should not contain script when filtering by skill type")
	}
}

func TestForgeListTool_ByStatus(t *testing.T) {
	f, _ := newTestForge(t)

	f.GetRegistry().Add(forge.Artifact{
		ID:     "skill-active-1",
		Type:   forge.ArtifactSkill,
		Name:   "active-1",
		Status: forge.StatusActive,
		Path:   "/tmp/fake1",
	})
	f.GetRegistry().Add(forge.Artifact{
		ID:     "skill-draft-1",
		Type:   forge.ArtifactSkill,
		Name:   "draft-1",
		Status: forge.StatusDraft,
		Path:   "/tmp/fake2",
	})

	ftools := forge.NewForgeTools(f)
	tool := findTool(t, ftools, "forge_list")

	// Filter by type=skill and status=active to use the List() path
	result := tool.Execute(context.Background(), map[string]interface{}{
		"type":   "skill",
		"status": "active",
	})
	if result.IsError {
		t.Errorf("forge_list should not error: %s", result.ForLLM)
	}
	if !contains(result.ForLLM, "active-1") {
		t.Errorf("Result should contain 'active-1', got: %s", result.ForLLM)
	}
	if contains(result.ForLLM, "draft-1") {
		t.Error("Result should not contain draft artifact when filtering by active status")
	}
}

func TestForgeListTool_EmptyRegistry(t *testing.T) {
	f, _ := newTestForge(t)
	ftools := forge.NewForgeTools(f)
	tool := findTool(t, ftools, "forge_list")

	result := tool.Execute(context.Background(), map[string]interface{}{})
	if result.IsError {
		t.Errorf("forge_list should not error on empty: %s", result.ForLLM)
	}
	if !contains(result.ForLLM, "暂无") {
		t.Errorf("Should say '暂无', got: %s", result.ForLLM)
	}
}

func TestForgeShareTool_NoBridge(t *testing.T) {
	f, _ := newTestForge(t)
	ftools := forge.NewForgeTools(f)
	tool := findTool(t, ftools, "forge_share")

	result := tool.Execute(context.Background(), map[string]interface{}{})
	if result.IsError {
		t.Errorf("Should not be error, just info message: %s", result.ForLLM)
	}
	if !contains(result.ForLLM, "未启用") {
		t.Errorf("Should mention cluster sharing not enabled, got: %s", result.ForLLM)
	}
}

func TestForgeLearningStatusTool_Disabled(t *testing.T) {
	f, _ := newTestForge(t)
	ftools := forge.NewForgeTools(f)
	tool := findTool(t, ftools, "forge_learning_status")

	result := tool.Execute(context.Background(), map[string]interface{}{})
	if result.IsError {
		t.Errorf("Should not error: %s", result.ForLLM)
	}
	if !contains(result.ForLLM, "未启用") {
		t.Errorf("Should mention learning not enabled, got: %s", result.ForLLM)
	}
}

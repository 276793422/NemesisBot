package forge

import (
	"context"
	"os"
	"path/filepath"
	"testing"
)

// --- Tool name/description tests ---

func TestForgeTools_NamesAndDescriptions(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, err := NewForge(workspace, nil)
	if err != nil {
		t.Fatalf("NewForge failed: %v", err)
	}

	tools := NewForgeTools(f)

	expectedNames := []string{
		"forge_reflect", "forge_create", "forge_update",
		"forge_list", "forge_evaluate", "forge_build_mcp",
		"forge_share", "forge_learning_status",
	}

	for i, expected := range expectedNames {
		if tools[i].Name() != expected {
			t.Errorf("Tool %d: expected name '%s', got '%s'", i, expected, tools[i].Name())
		}
		if tools[i].Description() == "" {
			t.Errorf("Tool %d (%s): description should not be empty", i, expected)
		}
	}
}

func TestForgeReflectTool_Execute(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := NewForge(workspace, nil)
	tools := NewForgeTools(f)

	for _, tool := range tools {
		if tool.Name() == "forge_reflect" {
			result := tool.Execute(context.Background(), map[string]interface{}{
				"period": "today",
				"focus":  "all",
			})
			// With no experiences, reflect will error with "insufficient experiences"
			if !result.IsError {
				t.Error("forge_reflect should error when insufficient experiences")
			}
			return
		}
	}
	t.Fatal("forge_reflect tool not found")
}

func TestForgeListTool_Execute(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := NewForge(workspace, nil)
	tools := NewForgeTools(f)

	for _, tool := range tools {
		if tool.Name() == "forge_list" {
			result := tool.Execute(context.Background(), map[string]interface{}{})
			if result.IsError {
				t.Errorf("forge_list on empty should not error: %s", result.ForLLM)
			}
			return
		}
	}
	t.Fatal("forge_list tool not found")
}

func TestForgeCreateTool_Execute_Skill(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := NewForge(workspace, nil)
	tools := NewForgeTools(f)

	for _, tool := range tools {
		if tool.Name() == "forge_create" {
			result := tool.Execute(context.Background(), map[string]interface{}{
				"type":        "skill",
				"name":        "test-skill",
				"content":     "---\nname: test-skill\ndescription: A test\n---\nSteps here that are long enough to pass validation.",
				"description": "Test skill",
			})
			if result.IsError {
				t.Errorf("forge_create skill should succeed: %s", result.ForLLM)
			}

			// Verify in registry
			artifact, found := f.GetRegistry().Get("skill-test-skill")
			if !found {
				t.Fatal("Artifact should be registered")
			}
			if artifact.Type != ArtifactSkill {
				t.Errorf("Expected type 'skill', got '%s'", artifact.Type)
			}
			return
		}
	}
	t.Fatal("forge_create tool not found")
}

func TestForgeCreateTool_Execute_MissingType(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := NewForge(workspace, nil)
	tools := NewForgeTools(f)

	for _, tool := range tools {
		if tool.Name() == "forge_create" {
			result := tool.Execute(context.Background(), map[string]interface{}{
				"name":    "test",
				"content": "content",
			})
			if !result.IsError {
				t.Error("Should error without type")
			}
			return
		}
	}
	t.Fatal("forge_create tool not found")
}

func TestForgeUpdateTool_Execute_NotFound(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := NewForge(workspace, nil)
	tools := NewForgeTools(f)

	for _, tool := range tools {
		if tool.Name() == "forge_update" {
			result := tool.Execute(context.Background(), map[string]interface{}{
				"id":                 "nonexistent",
				"content":            "updated",
				"change_description": "test",
			})
			if !result.IsError {
				t.Error("Should error for non-existent artifact")
			}
			return
		}
	}
	t.Fatal("forge_update tool not found")
}

func TestForgeEvaluateTool_Execute_NotFound(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := NewForge(workspace, nil)
	tools := NewForgeTools(f)

	for _, tool := range tools {
		if tool.Name() == "forge_evaluate" {
			result := tool.Execute(context.Background(), map[string]interface{}{
				"id": "nonexistent",
			})
			if !result.IsError {
				t.Error("Should error for non-existent artifact")
			}
			return
		}
	}
	t.Fatal("forge_evaluate tool not found")
}

func TestForgeShareTool_Execute_NotEnabled(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := NewForge(workspace, nil)
	tools := NewForgeTools(f)

	for _, tool := range tools {
		if tool.Name() == "forge_share" {
			result := tool.Execute(context.Background(), map[string]interface{}{
				"report": "test.md",
			})
			// When not enabled, returns info message (not error)
			if result.IsError {
				t.Errorf("Should return info message, not error: %s", result.ForLLM)
			}
			if result.ForLLM == "" {
				t.Error("Should have a message about cluster sharing not enabled")
			}
			return
		}
	}
	t.Fatal("forge_share tool not found")
}

func TestForgeLearningStatusTool_Execute(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := NewForge(workspace, nil)
	tools := NewForgeTools(f)

	for _, tool := range tools {
		if tool.Name() == "forge_learning_status" {
			result := tool.Execute(context.Background(), map[string]interface{}{})
			if result.IsError {
				t.Errorf("forge_learning_status should not error: %s", result.ForLLM)
			}
			return
		}
	}
	t.Fatal("forge_learning_status tool not found")
}

// --- Tool version snapshot tests ---

func TestVersionSnapshot_SaveAndLoad(t *testing.T) {
	tmpDir := t.TempDir()

	// Create an artifact file
	artifactPath := filepath.Join(tmpDir, "SKILL.md")
	os.WriteFile(artifactPath, []byte("---\nname: test\n---\nOriginal content"), 0644)

	// Save snapshot
	if err := SaveVersionSnapshot(artifactPath, "1.0"); err != nil {
		t.Fatalf("SaveVersionSnapshot failed: %v", err)
	}

	// Modify the file
	os.WriteFile(artifactPath, []byte("---\nname: test\n---\nModified content"), 0644)

	// Load snapshot
	content, err := LoadVersionSnapshot(artifactPath, "1.0")
	if err != nil {
		t.Fatalf("LoadVersionSnapshot failed: %v", err)
	}
	if content != "---\nname: test\n---\nOriginal content" {
		t.Errorf("Expected original content, got: %s", content)
	}
}

func TestVersionSnapshot_LoadNonexistent(t *testing.T) {
	_, err := LoadVersionSnapshot("/nonexistent/SKILL.md", "1.0")
	if err == nil {
		t.Error("Should error for non-existent snapshot")
	}
}

func TestVersionSnapshot_IncrementVersion(t *testing.T) {
	tests := []struct {
		input    string
		expected string
	}{
		{"1.0", "1.1"},
		{"1.9", "1.10"},
		{"2.0", "2.1"},
		{"", ".1"}, // empty version becomes ".1"
	}

	for _, tt := range tests {
		result := IncrementVersionForTest(tt.input)
		if result != tt.expected {
			t.Errorf("IncrementVersion(%q) = %q, expected %q", tt.input, result, tt.expected)
		}
	}
}

// --- Exporter tests ---

func TestExporter_ExportArtifact_NotFound(t *testing.T) {
	tmpDir := t.TempDir()
	registry := NewRegistry(tmpDir + "/registry.json")
	exporter := NewExporter(tmpDir, registry)

	err := exporter.ExportArtifact("nonexistent", tmpDir)
	if err == nil {
		t.Error("Should error for non-existent artifact")
	}
}

func TestExporter_ExportArtifact_Success(t *testing.T) {
	tmpDir := t.TempDir()
	registry := NewRegistry(tmpDir + "/registry.json")

	// Create artifact file
	skillDir := filepath.Join(tmpDir, "skills", "test-skill")
	os.MkdirAll(skillDir, 0755)
	skillPath := filepath.Join(skillDir, "SKILL.md")
	os.WriteFile(skillPath, []byte("---\nname: test-skill\n---\nContent"), 0644)

	registry.Add(Artifact{
		ID:      "skill-test",
		Type:    ArtifactSkill,
		Name:    "test-skill",
		Version: "1.0",
		Status:  StatusActive,
		Path:    skillPath,
	})

	exportDir := filepath.Join(tmpDir, "export")
	os.MkdirAll(exportDir, 0755)

	exporter := NewExporter(tmpDir, registry)
	err := exporter.ExportArtifact("skill-test", exportDir)
	if err != nil {
		t.Fatalf("ExportArtifact failed: %v", err)
	}

	// Verify exported files
	exportedDir := filepath.Join(exportDir, "test-skill-1.0")
	if _, err := os.Stat(exportedDir); os.IsNotExist(err) {
		t.Error("Export directory should exist")
	}

	// Verify SKILL.md was copied
	copiedFile := filepath.Join(exportedDir, "SKILL.md")
	if _, err := os.Stat(copiedFile); os.IsNotExist(err) {
		t.Error("SKILL.md should be copied")
	}

	// Verify manifest was created
	manifestPath := filepath.Join(exportedDir, "forge-manifest.json")
	if _, err := os.Stat(manifestPath); os.IsNotExist(err) {
		t.Error("forge-manifest.json should be created")
	}
}

func TestExporter_ExportAll_Empty(t *testing.T) {
	tmpDir := t.TempDir()
	registry := NewRegistry(tmpDir + "/registry.json")
	exporter := NewExporter(tmpDir, registry)

	count, err := exporter.ExportAll(filepath.Join(tmpDir, "export"))
	if err != nil {
		t.Fatalf("ExportAll failed: %v", err)
	}
	if count != 0 {
		t.Errorf("Expected 0 exports, got %d", count)
	}
}

func TestExporter_ExportAll_OnlyActive(t *testing.T) {
	tmpDir := t.TempDir()
	registry := NewRegistry(tmpDir + "/registry.json")

	// Create files
	for _, name := range []string{"active-skill", "draft-skill"} {
		skillDir := filepath.Join(tmpDir, "skills", name)
		os.MkdirAll(skillDir, 0755)
		skillPath := filepath.Join(skillDir, "SKILL.md")
		os.WriteFile(skillPath, []byte("---\nname: "+name+"\n---\nContent"), 0644)
	}

	registry.Add(Artifact{ID: "skill-active", Type: ArtifactSkill, Name: "active-skill", Version: "1.0", Status: StatusActive, Path: filepath.Join(tmpDir, "skills", "active-skill", "SKILL.md")})
	registry.Add(Artifact{ID: "skill-draft", Type: ArtifactSkill, Name: "draft-skill", Version: "1.0", Status: StatusDraft, Path: filepath.Join(tmpDir, "skills", "draft-skill", "SKILL.md")})

	exportDir := filepath.Join(tmpDir, "export")
	os.MkdirAll(exportDir, 0755)

	exporter := NewExporter(tmpDir, registry)
	count, err := exporter.ExportAll(exportDir)
	if err != nil {
		t.Fatalf("ExportAll failed: %v", err)
	}
	if count != 1 {
		t.Errorf("Expected 1 export (only active), got %d", count)
	}
}

// --- copyFile tests ---

func TestCopyFile(t *testing.T) {
	tmpDir := t.TempDir()
	src := filepath.Join(tmpDir, "src.txt")
	dst := filepath.Join(tmpDir, "dst.txt")

	os.WriteFile(src, []byte("hello world"), 0644)
	if err := copyFile(src, dst); err != nil {
		t.Fatalf("copyFile failed: %v", err)
	}

	data, err := os.ReadFile(dst)
	if err != nil {
		t.Fatalf("Failed to read dst: %v", err)
	}
	if string(data) != "hello world" {
		t.Errorf("Expected 'hello world', got '%s'", string(data))
	}
}

func TestCopyFile_SourceNotFound(t *testing.T) {
	err := copyFile("/nonexistent/src.txt", "/tmp/dst.txt")
	if err == nil {
		t.Error("Should error when source doesn't exist")
	}
}

// --- copyDir tests ---

func TestCopyDir(t *testing.T) {
	tmpDir := t.TempDir()
	srcDir := filepath.Join(tmpDir, "src")
	dstDir := filepath.Join(tmpDir, "dst")
	os.MkdirAll(filepath.Join(srcDir, "sub"), 0755)
	os.WriteFile(filepath.Join(srcDir, "a.txt"), []byte("a"), 0644)
	os.WriteFile(filepath.Join(srcDir, "sub", "b.txt"), []byte("b"), 0644)

	files := copyDir(srcDir, dstDir)
	if len(files) != 2 {
		t.Errorf("Expected 2 files copied, got %d: %v", len(files), files)
	}
}

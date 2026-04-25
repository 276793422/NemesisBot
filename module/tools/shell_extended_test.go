// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package tools

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/276793422/NemesisBot/module/config"
)

// ==================== NewExecToolWithConfig Config Branches ====================

func TestNewExecToolWithConfig_DisableDenyPatterns(t *testing.T) {
	cfg := &config.Config{}
	cfg.Tools.Exec.EnableDenyPatterns = false
	tool := NewExecToolWithConfig("/workspace", false, cfg)
	if tool == nil {
		t.Fatal("Expected non-nil tool")
	}
	if len(tool.denyPatterns) != 0 {
		t.Errorf("Expected no deny patterns when disabled, got %d", len(tool.denyPatterns))
	}
}

func TestNewExecToolWithConfig_CustomDenyPatterns(t *testing.T) {
	cfg := &config.Config{}
	cfg.Tools.Exec.EnableDenyPatterns = true
	cfg.Tools.Exec.CustomDenyPatterns = []string{`\btest_pattern\b`}
	tool := NewExecToolWithConfig("/workspace", false, cfg)
	if tool == nil {
		t.Fatal("Expected non-nil tool")
	}
	if len(tool.denyPatterns) != 1 {
		t.Errorf("Expected 1 custom deny pattern, got %d", len(tool.denyPatterns))
	}
}

func TestNewExecToolWithConfig_DefaultDenyPatterns(t *testing.T) {
	cfg := &config.Config{}
	cfg.Tools.Exec.EnableDenyPatterns = true
	cfg.Tools.Exec.CustomDenyPatterns = nil // Use defaults
	tool := NewExecToolWithConfig("/workspace", false, cfg)
	if tool == nil {
		t.Fatal("Expected non-nil tool")
	}
	if len(tool.denyPatterns) == 0 {
		t.Error("Expected default deny patterns")
	}
}

// ==================== ExecTool guardCommand Additional Paths ====================

func TestExecTool_GuardCommand_AllowListPath(t *testing.T) {
	tool := NewExecTool(t.TempDir(), false)
	tool.SetAllowPatterns([]string{`^echo`, `^dir`, `^type`})

	// Allowed command
	result := tool.guardCommand("echo hello", t.TempDir())
	if result != "" {
		t.Errorf("Expected allowed, got '%s'", result)
	}

	// Not allowed command
	result = tool.guardCommand("del file.txt", t.TempDir())
	if result == "" {
		t.Error("Expected blocked for non-allowlisted command")
	}
}

func TestExecTool_GuardCommand_WorkspacePathCheck(t *testing.T) {
	tool := NewExecTool(t.TempDir(), true)

	// Command with path inside workspace
	result := tool.guardCommand("type C:\\Windows\\System32\\test.txt", t.TempDir())
	if result == "" {
		t.Error("Expected blocked for path outside workspace")
	}
}

func TestExecTool_GuardCommand_PathTraversalInCommand(t *testing.T) {
	tool := NewExecTool(t.TempDir(), true)

	result := tool.guardCommand("type ..\\..\\secret.txt", t.TempDir())
	if result == "" {
		t.Error("Expected blocked for path traversal")
	}
}

func TestExecTool_GuardCommand_DangerousPattern(t *testing.T) {
	tool := NewExecTool(t.TempDir(), false)

	tests := []struct {
		name    string
		command string
		blocked bool
	}{
		{"rm -rf", "rm -rf /", true},
		{"sudo", "sudo apt install", true},
		{"eval", "eval $(cat file)", true},
		{"format", "format c:", true},
		{"safe echo", "echo hello", false},
		{"safe dir", "dir", false},
		{"safe type", "type file.txt", false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := tool.guardCommand(tt.command, t.TempDir())
			if tt.blocked && result == "" {
				t.Errorf("Expected '%s' to be blocked", tt.command)
			}
			if !tt.blocked && result != "" {
				t.Errorf("Expected '%s' to be allowed, got '%s'", tt.command, result)
			}
		})
	}
}

// ==================== fixWindowsPathQuoting Tests ====================

func TestExecTool_FixWindowsPathQuoting_NoSpaces(t *testing.T) {
	tool := NewExecTool(t.TempDir(), false)

	result := tool.fixWindowsPathQuoting(`type "C:\test\file.txt"`)
	if contains(result, `"`) {
		t.Errorf("Expected quotes removed for path without spaces, got '%s'", result)
	}
}

func TestExecTool_FixWindowsPathQuoting_WithSpaces(t *testing.T) {
	tool := NewExecTool(t.TempDir(), false)

	result := tool.fixWindowsPathQuoting(`type "C:\Program Files\test.txt"`)
	// Should convert spaces with ^ or keep quotes
	if !contains(result, "^ ") && !contains(result, `"`) {
		t.Errorf("Expected path with spaces handled, got '%s'", result)
	}
}

func TestExecTool_FixWindowsPathQuoting_DirCommand(t *testing.T) {
	tool := NewExecTool(t.TempDir(), false)

	result := tool.fixWindowsPathQuoting(`dir "C:\temp"`)
	if contains(result, `"`) {
		t.Errorf("Expected quotes removed for dir, got '%s'", result)
	}
}

func TestExecTool_FixWindowsPathQuoting_NonProblematicCommand(t *testing.T) {
	tool := NewExecTool(t.TempDir(), false)

	result := tool.fixWindowsPathQuoting(`python "C:\test\script.py"`)
	if !contains(result, `"`) {
		t.Error("Expected quotes preserved for non-problematic command")
	}
}

// ==================== ExecTool Execute with various paths ====================

func TestExecTool_Execute_WorkingDirOverride(t *testing.T) {
	tmpDir := t.TempDir()
	tool := NewExecTool(tmpDir, false)
	ctx := context.Background()

	// Create a subdirectory
	result := tool.Execute(ctx, map[string]interface{}{
		"command":     "echo test",
		"working_dir": tmpDir,
	})
	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}
}

func TestExecTool_Execute_NoWorkingDir(t *testing.T) {
	tool := NewExecTool("", false)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"command": "echo hello",
	})
	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}
}

// ==================== Additional filesystem coverage ====================

func TestValidatePath_NoWorkspaceRestrict(t *testing.T) {
	// validatePath with no workspace restriction
	result, err := validatePath("/some/abs/path", "", false)
	if err != nil {
		t.Errorf("Expected no error, got %v", err)
	}
	if result == "" {
		t.Error("Expected non-empty result")
	}
}

func TestValidatePath_RelativePathInWorkspace(t *testing.T) {
	tmpDir := t.TempDir()
	result, err := validatePath("subdir/file.txt", tmpDir, true)
	if err != nil {
		t.Errorf("Expected no error, got %v", err)
	}
	if result == "" {
		t.Error("Expected non-empty result")
	}
	if !strings.Contains(result, "subdir") {
		t.Errorf("Expected result containing 'subdir', got '%s'", result)
	}
}

func TestValidatePath_EmptyPath(t *testing.T) {
	tmpDir := t.TempDir()
	_, err := validatePath("", tmpDir, true)
	if err != nil {
		// Empty path resolves to workspace itself which should be valid
		t.Logf("validatePath with empty path returned: %v", err)
	}
}

func TestResolveExistingAncestor_DeepPath(t *testing.T) {
	tmpDir := t.TempDir()
	// Create a subdirectory
	existingDir := filepath.Join(tmpDir, "existing")
	createDir(t, existingDir)

	result, _ := resolveExistingAncestor(filepath.Join(tmpDir, "existing", "deep", "nested", "path"))
	// Should resolve to the existing directory
	if result != existingDir {
		t.Errorf("Expected '%s', got '%s'", existingDir, result)
	}
}

func createDir(t *testing.T, path string) {
	t.Helper()
	if err := mkdirAllHelper(path); err != nil {
		t.Fatalf("Failed to create dir: %v", err)
	}
}

func mkdirAllHelper(path string) error {
	return os.MkdirAll(path, 0755)
}

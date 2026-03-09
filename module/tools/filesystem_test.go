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
)

func TestValidatePath_NoWorkspace(t *testing.T) {
	result, err := validatePath("test.txt", "", false)
	if err != nil {
		t.Errorf("Expected no error, got %v", err)
	}
	if result != "test.txt" {
		t.Errorf("Expected 'test.txt', got '%s'", result)
	}
}

func TestValidatePath_RelativePath(t *testing.T) {
	tempDir := t.TempDir()
	result, err := validatePath("test.txt", tempDir, false)
	if err != nil {
		t.Errorf("Expected no error, got %v", err)
	}
	expected := filepath.Join(tempDir, "test.txt")
	if result != expected {
		t.Errorf("Expected '%s', got '%s'", expected, result)
	}
}

func TestValidatePath_AbsolutePath(t *testing.T) {
	tempDir := t.TempDir()
	testFile := filepath.Join(tempDir, "test.txt")
	result, err := validatePath(testFile, tempDir, false)
	if err != nil {
		t.Errorf("Expected no error, got %v", err)
	}
	if result != testFile {
		t.Errorf("Expected '%s', got '%s'", testFile, result)
	}
}

func TestValidatePath_PathTraversalBlocked(t *testing.T) {
	tempDir := t.TempDir()
	_, err := validatePath("../etc/passwd", tempDir, true)
	if err == nil {
		t.Error("Expected error for path traversal")
	}
	if !strings.Contains(err.Error(), "access denied") {
		t.Errorf("Expected access denied error, got %v", err)
	}
}

func TestReadFileTool_Execute_Success(t *testing.T) {
	tempDir := t.TempDir()
	testFile := filepath.Join(tempDir, "test.txt")
	expectedContent := "Hello, World!"
	err := os.WriteFile(testFile, []byte(expectedContent), 0644)
	if err != nil {
		t.Fatalf("Failed to create test file: %v", err)
	}

	tool := NewReadFileTool(tempDir, true)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"path": "test.txt",
	})

	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}

	if result.ForLLM != expectedContent {
		t.Errorf("Expected content '%s', got '%s'", expectedContent, result.ForLLM)
	}
}

func TestReadFileTool_Execute_FileNotFound(t *testing.T) {
	tempDir := t.TempDir()
	tool := NewReadFileTool(tempDir, true)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"path": "nonexistent.txt",
	})

	if !result.IsError {
		t.Error("Expected error for non-existent file")
	}
}

func TestReadFileTool_Execute_MissingPath(t *testing.T) {
	tool := NewReadFileTool("", false)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{})

	if !result.IsError {
		t.Error("Expected error for missing path parameter")
	}

	if !strings.Contains(result.ForLLM, "path is required") {
		t.Errorf("Expected 'path is required' error, got '%s'", result.ForLLM)
	}
}

func TestReadFileTool_Execute_PathEscapedWorkspace(t *testing.T) {
	tempDir := t.TempDir()
	tool := NewReadFileTool(tempDir, true)
	ctx := context.Background()

	// Try to read a file outside the workspace
	result := tool.Execute(ctx, map[string]interface{}{
		"path": "../../../etc/passwd",
	})

	if !result.IsError {
		t.Error("Expected error for path outside workspace")
	}
}

func TestWriteFileTool_Execute_Success(t *testing.T) {
	tempDir := t.TempDir()
	tool := NewWriteFileTool(tempDir, true)
	ctx := context.Background()

	content := "Test content"
	result := tool.Execute(ctx, map[string]interface{}{
		"path":    "test.txt",
		"content": content,
	})

	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}

	// Verify file was written
	testFile := filepath.Join(tempDir, "test.txt")
	data, err := os.ReadFile(testFile)
	if err != nil {
		t.Fatalf("Failed to read written file: %v", err)
	}

	if string(data) != content {
		t.Errorf("Expected content '%s', got '%s'", content, string(data))
	}
}

func TestWriteFileTool_Execute_CreateDirectories(t *testing.T) {
	tempDir := t.TempDir()
	tool := NewWriteFileTool(tempDir, true)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"path":    "subdir/test.txt",
		"content": "content",
	})

	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}

	// Verify file was created
	testFile := filepath.Join(tempDir, "subdir", "test.txt")
	if _, err := os.Stat(testFile); os.IsNotExist(err) {
		t.Error("File was not created in subdirectory")
	}
}

func TestWriteFileTool_Execute_MissingPath(t *testing.T) {
	tool := NewWriteFileTool("", false)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"content": "test",
	})

	if !result.IsError {
		t.Error("Expected error for missing path parameter")
	}
}

func TestWriteFileTool_Execute_MissingContent(t *testing.T) {
	tool := NewWriteFileTool("", false)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"path": "test.txt",
	})

	if !result.IsError {
		t.Error("Expected error for missing content parameter")
	}
}

func TestListDirTool_Execute_Success(t *testing.T) {
	tempDir := t.TempDir()
	// Create some test files and directories
	_ = os.WriteFile(filepath.Join(tempDir, "file1.txt"), []byte("content"), 0644)
	_ = os.WriteFile(filepath.Join(tempDir, "file2.txt"), []byte("content"), 0644)
	_ = os.Mkdir(filepath.Join(tempDir, "subdir"), 0755)

	tool := NewListDirTool(tempDir, true)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"path": ".",
	})

	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}

	// Check that the output contains the expected entries
	output := result.ForLLM
	if !strings.Contains(output, "file1.txt") {
		t.Errorf("Expected output to contain 'file1.txt', got '%s'", output)
	}
	if !strings.Contains(output, "file2.txt") {
		t.Errorf("Expected output to contain 'file2.txt', got '%s'", output)
	}
	if !strings.Contains(output, "subdir") {
		t.Errorf("Expected output to contain 'subdir', got '%s'", output)
	}
}

func TestListDirTool_Execute_DefaultPath(t *testing.T) {
	tempDir := t.TempDir()
	tool := NewListDirTool(tempDir, true)
	ctx := context.Background()

	// Call without path parameter - should default to "."
	result := tool.Execute(ctx, map[string]interface{}{})

	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}
}

func TestDeleteFileTool_Execute_Success(t *testing.T) {
	tempDir := t.TempDir()
	testFile := filepath.Join(tempDir, "test.txt")
	_ = os.WriteFile(testFile, []byte("content"), 0644)

	tool := NewDeleteFileTool(tempDir, true)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"path": "test.txt",
	})

	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}

	// Verify file was deleted
	if _, err := os.Stat(testFile); !os.IsNotExist(err) {
		t.Error("File was not deleted")
	}
}

func TestDeleteFileTool_Execute_FileNotFound(t *testing.T) {
	tempDir := t.TempDir()
	tool := NewDeleteFileTool(tempDir, true)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"path": "nonexistent.txt",
	})

	if !result.IsError {
		t.Error("Expected error for non-existent file")
	}
}

func TestCreateDirTool_Execute_Success(t *testing.T) {
	tempDir := t.TempDir()
	tool := NewCreateDirTool(tempDir, true)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"path": "newdir",
	})

	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}

	// Verify directory was created
	newDir := filepath.Join(tempDir, "newdir")
	if info, err := os.Stat(newDir); err != nil {
		t.Errorf("Directory was not created: %v", err)
	} else if !info.IsDir() {
		t.Error("Path is not a directory")
	}
}

func TestCreateDirTool_Execute_NestedDirectories(t *testing.T) {
	tempDir := t.TempDir()
	tool := NewCreateDirTool(tempDir, true)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"path": "parent/child/grandchild",
	})

	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}

	// Verify nested directories were created
	nestedDir := filepath.Join(tempDir, "parent", "child", "grandchild")
	if info, err := os.Stat(nestedDir); err != nil {
		t.Errorf("Nested directories were not created: %v", err)
	} else if !info.IsDir() {
		t.Error("Path is not a directory")
	}
}

func TestDeleteDirTool_Execute_Success(t *testing.T) {
	tempDir := t.TempDir()
	testDir := filepath.Join(tempDir, "testdir")
	_ = os.Mkdir(testDir, 0755)
	_ = os.WriteFile(filepath.Join(testDir, "file.txt"), []byte("content"), 0644)

	tool := NewDeleteDirTool(tempDir, true)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"path": "testdir",
	})

	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}

	// Verify directory was deleted
	if _, err := os.Stat(testDir); !os.IsNotExist(err) {
		t.Error("Directory was not deleted")
	}
}

func TestDeleteDirTool_Execute_NonEmptyDirectory(t *testing.T) {
	tempDir := t.TempDir()
	testDir := filepath.Join(tempDir, "testdir")
	_ = os.Mkdir(testDir, 0755)
	_ = os.WriteFile(filepath.Join(testDir, "file.txt"), []byte("content"), 0644)

	tool := NewDeleteDirTool(tempDir, true)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"path": "testdir",
	})

	if result.IsError {
		t.Errorf("Expected success for non-empty directory, got error: %s", result.ForLLM)
	}

	// Verify directory and contents were deleted
	if _, err := os.Stat(testDir); !os.IsNotExist(err) {
		t.Error("Non-empty directory was not deleted")
	}
}

func TestIsWithinWorkspace(t *testing.T) {
	tests := []struct {
		name      string
		candidate string
		workspace string
		want      bool
	}{
		{
			name:      "file in workspace",
			candidate: "/workspace/file.txt",
			workspace: "/workspace",
			want:      true,
		},
		{
			name:      "subdirectory in workspace",
			candidate: "/workspace/subdir/file.txt",
			workspace: "/workspace",
			want:      true,
		},
		{
			name:      "file outside workspace",
			candidate: "/etc/passwd",
			workspace: "/workspace",
			want:      false,
		},
		{
			name:      "parent directory",
			candidate: "/workspace/../etc/passwd",
			workspace: "/workspace",
			want:      false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := isWithinWorkspace(tt.candidate, tt.workspace)
			if result != tt.want {
				t.Errorf("isWithinWorkspace() = %v, want %v", result, tt.want)
			}
		})
	}
}

func TestResolveExistingAncestor(t *testing.T) {
	tempDir := t.TempDir()
	// Create a directory structure
	subdir := filepath.Join(tempDir, "subdir")
	_ = os.Mkdir(subdir, 0755)

	// Test resolving an existing path
	result, err := resolveExistingAncestor(subdir)
	if err != nil {
		t.Errorf("Expected no error, got %v", err)
	}
	if result != subdir {
		t.Errorf("Expected '%s', got '%s'", subdir, result)
	}

	// Test resolving a non-existing path with existing ancestor
	nonExisting := filepath.Join(subdir, "nonexistent")
	result, err = resolveExistingAncestor(nonExisting)
	if err != nil {
		t.Errorf("Expected no error, got %v", err)
	}
	if result != subdir {
		t.Errorf("Expected '%s', got '%s'", subdir, result)
	}

	// Test resolving a path with no existing ancestor - this should succeed on Windows
	// since the drive root exists, but may fail on Unix
	result, err = resolveExistingAncestor("/nonexistent/deeply/nested/path")
	// On Windows, this might resolve to the drive root
	// On Unix, this will fail with ErrNotExist
	if err != nil && !os.IsNotExist(err) {
		t.Errorf("Expected ErrNotExist or no error, got %v", err)
	}
}

// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package tools_test

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"

	. "github.com/276793422/NemesisBot/module/tools"
)

func TestReadFileTool_Execute_Success(t *testing.T) {
	tmpDir := t.TempDir()
	testFile := filepath.Join(tmpDir, "test.txt")
	content := "Hello, World!"

	os.WriteFile(testFile, []byte(content), 0644)

	tool := NewReadFileTool(tmpDir, false)
	ctx := context.Background()
	args := map[string]interface{}{
		"path": testFile,
	}

	result := tool.Execute(ctx, args)

	if result.IsError {
		t.Errorf("ReadFile failed: %s", result.ForLLM)
	}

	if !strings.Contains(result.ForLLM, "Hello, World!") {
		t.Errorf("Expected content 'Hello, World!', got '%s'", result.ForLLM)
	}
}

func TestReadFileTool_Execute_MissingPath(t *testing.T) {
	tmpDir := t.TempDir()
	tool := NewReadFileTool(tmpDir, false)
	ctx := context.Background()
	args := map[string]interface{}{}

	result := tool.Execute(ctx, args)

	if !result.IsError {
		t.Error("Expected error for missing path parameter")
	}
}

func TestReadFileTool_Execute_FileNotFound(t *testing.T) {
	tmpDir := t.TempDir()
	tool := NewReadFileTool(tmpDir, false)
	ctx := context.Background()
	args := map[string]interface{}{
		"path": "nonexistent.txt",
	}

	result := tool.Execute(ctx, args)

	if !result.IsError {
		t.Error("Expected error for non-existent file")
	}
}

func TestReadFileTool_PathValidation_WorkspaceRestriction(t *testing.T) {
	tmpDir := t.TempDir()
	tool := NewReadFileTool(tmpDir, true)
	ctx := context.Background()

	// Try to read outside workspace (should fail)
	args := map[string]interface{}{
		"path": "../../../etc/passwd",
	}

	result := tool.Execute(ctx, args)

	if !result.IsError {
		t.Error("Expected error when accessing path outside workspace")
	}
}

func TestWriteFileTool_Execute_Success(t *testing.T) {
	tmpDir := t.TempDir()
	content := "Test content"

	tool := NewWriteFileTool(tmpDir, false)
	ctx := context.Background()
	args := map[string]interface{}{
		"path":    "write_test.txt",
		"content": content,
	}

	result := tool.Execute(ctx, args)

	if result.IsError {
		t.Errorf("WriteFile failed: %s", result.ForLLM)
	}

	// Verify file was written
	data, err := os.ReadFile(filepath.Join(tmpDir, "write_test.txt"))
	if err != nil {
		t.Fatalf("Failed to read written file: %v", err)
	}

	if string(data) != content {
		t.Errorf("Expected content '%s', got '%s'", content, string(data))
	}
}

func TestWriteFileTool_Execute_MissingPath(t *testing.T) {
	tmpDir := t.TempDir()
	tool := NewWriteFileTool(tmpDir, false)
	ctx := context.Background()
	args := map[string]interface{}{
		"content": "test",
	}

	result := tool.Execute(ctx, args)

	if !result.IsError {
		t.Error("Expected error for missing path parameter")
	}
}

func TestWriteFileTool_Execute_MissingContent(t *testing.T) {
	tmpDir := t.TempDir()
	tool := NewWriteFileTool(tmpDir, false)
	ctx := context.Background()
	args := map[string]interface{}{
		"path": "test.txt",
	}

	result := tool.Execute(ctx, args)

	if !result.IsError {
		t.Error("Expected error for missing content parameter")
	}
}

func TestWriteFileTool_Execute_CreatesDirectories(t *testing.T) {
	tmpDir := t.TempDir()
	content := "Nested file content"

	tool := NewWriteFileTool(tmpDir, false)
	ctx := context.Background()
	args := map[string]interface{}{
		"path":    "nested/dir/file.txt",
		"content": content,
	}

	result := tool.Execute(ctx, args)

	if result.IsError {
		t.Errorf("WriteFile failed: %s", result.ForLLM)
	}

	// Verify file was created in nested directory
	filePath := filepath.Join(tmpDir, "nested", "dir", "file.txt")
	data, err := os.ReadFile(filePath)
	if err != nil {
		t.Fatalf("Failed to read written file: %v", err)
	}

	if string(data) != content {
		t.Errorf("Expected content '%s', got '%s'", content, string(data))
	}
}

func TestWriteFileTool_PathValidation_WorkspaceRestriction(t *testing.T) {
	tmpDir := t.TempDir()
	tool := NewWriteFileTool(tmpDir, true)
	ctx := context.Background()

	// Try to write outside workspace (should fail)
	args := map[string]interface{}{
		"path":    "../../../etc/test.txt",
		"content": "malicious",
	}

	result := tool.Execute(ctx, args)

	if !result.IsError {
		t.Error("Expected error when writing outside workspace")
	}
}

func TestListDirTool_Execute_Success(t *testing.T) {
	tmpDir := t.TempDir()

	// Create test files and directories
	os.WriteFile(filepath.Join(tmpDir, "file1.txt"), []byte("content1"), 0644)
	os.WriteFile(filepath.Join(tmpDir, "file2.txt"), []byte("content2"), 0644)
	os.Mkdir(filepath.Join(tmpDir, "dir1"), 0755)

	tool := NewListDirTool(tmpDir, false)
	ctx := context.Background()
	args := map[string]interface{}{
		"path": tmpDir,
	}

	result := tool.Execute(ctx, args)

	if result.IsError {
		t.Errorf("ListDir failed: %s", result.ForLLM)
	}

	content := result.ForLLM
	if !strings.Contains(content, "file1.txt") {
		t.Error("Result should contain file1.txt")
	}
	if !strings.Contains(content, "file2.txt") {
		t.Error("Result should contain file2.txt")
	}
	if !strings.Contains(content, "dir1") {
		t.Error("Result should contain dir1")
	}
}

func TestListDirTool_Execute_DefaultPath(t *testing.T) {
	tmpDir := t.TempDir()

	tool := NewListDirTool(tmpDir, false)
	ctx := context.Background()
	args := map[string]interface{}{} // No path provided

	result := tool.Execute(ctx, args)

	if result.IsError {
		t.Errorf("ListDir with default path failed: %s", result.ForLLM)
	}
}

func TestListDirTool_Execute_NonExistentPath(t *testing.T) {
	tmpDir := t.TempDir()
	tool := NewListDirTool(tmpDir, false)
	ctx := context.Background()
	args := map[string]interface{}{
		"path": "nonexistent_dir",
	}

	result := tool.Execute(ctx, args)

	if !result.IsError {
		t.Error("Expected error for non-existent directory")
	}
}

func TestDeleteFileTool_Execute_Success(t *testing.T) {
	tmpDir := t.TempDir()
	testFile := filepath.Join(tmpDir, "delete_me.txt")
	os.WriteFile(testFile, []byte("content"), 0644)

	tool := NewDeleteFileTool(tmpDir, false)
	ctx := context.Background()
	args := map[string]interface{}{
		"path": "delete_me.txt",
	}

	result := tool.Execute(ctx, args)

	if result.IsError {
		t.Errorf("DeleteFile failed: %s", result.ForLLM)
	}

	// Verify file was deleted
	if _, err := os.Stat(testFile); !os.IsNotExist(err) {
		t.Error("File should have been deleted")
	}
}

func TestDeleteFileTool_Execute_MissingPath(t *testing.T) {
	tmpDir := t.TempDir()
	tool := NewDeleteFileTool(tmpDir, false)
	ctx := context.Background()
	args := map[string]interface{}{}

	result := tool.Execute(ctx, args)

	if !result.IsError {
		t.Error("Expected error for missing path parameter")
	}
}

func TestDeleteFileTool_Execute_NonExistentFile(t *testing.T) {
	tmpDir := t.TempDir()
	tool := NewDeleteFileTool(tmpDir, false)
	ctx := context.Background()
	args := map[string]interface{}{
		"path": "nonexistent.txt",
	}

	result := tool.Execute(ctx, args)

	if !result.IsError {
		t.Error("Expected error when deleting non-existent file")
	}
}

func TestCreateDirTool_Execute_Success(t *testing.T) {
	tmpDir := t.TempDir()

	tool := NewCreateDirTool(tmpDir, false)
	ctx := context.Background()
	args := map[string]interface{}{
		"path": "new_dir",
	}

	result := tool.Execute(ctx, args)

	if result.IsError {
		t.Errorf("CreateDir failed: %s", result.ForLLM)
	}

	// Verify directory was created
	dirPath := filepath.Join(tmpDir, "new_dir")
	info, err := os.Stat(dirPath)
	if err != nil {
		t.Fatalf("Failed to stat directory: %v", err)
	}

	if !info.IsDir() {
		t.Error("Created path should be a directory")
	}
}

func TestCreateDirTool_Execute_Nested(t *testing.T) {
	tmpDir := t.TempDir()

	tool := NewCreateDirTool(tmpDir, false)
	ctx := context.Background()
	args := map[string]interface{}{
		"path": "parent/child/grandchild",
	}

	result := tool.Execute(ctx, args)

	if result.IsError {
		t.Errorf("CreateDir failed for nested path: %s", result.ForLLM)
	}

	// Verify all directories were created
	dirPath := filepath.Join(tmpDir, "parent", "child", "grandchild")
	info, err := os.Stat(dirPath)
	if err != nil {
		t.Fatalf("Failed to stat nested directory: %v", err)
	}

	if !info.IsDir() {
		t.Error("Created path should be a directory")
	}
}

func TestCreateDirTool_Execute_MissingPath(t *testing.T) {
	tmpDir := t.TempDir()
	tool := NewCreateDirTool(tmpDir, false)
	ctx := context.Background()
	args := map[string]interface{}{}

	result := tool.Execute(ctx, args)

	if !result.IsError {
		t.Error("Expected error for missing path parameter")
	}
}

func TestDeleteDirTool_Execute_Success(t *testing.T) {
	tmpDir := t.TempDir()
	testDir := filepath.Join(tmpDir, "delete_me_dir")
	os.Mkdir(testDir, 0755)
	os.WriteFile(filepath.Join(testDir, "file.txt"), []byte("content"), 0644)

	tool := NewDeleteDirTool(tmpDir, false)
	ctx := context.Background()
	args := map[string]interface{}{
		"path": "delete_me_dir",
	}

	result := tool.Execute(ctx, args)

	if result.IsError {
		t.Errorf("DeleteDir failed: %s", result.ForLLM)
	}

	// Verify directory was deleted
	if _, err := os.Stat(testDir); !os.IsNotExist(err) {
		t.Error("Directory should have been deleted")
	}
}

func TestDeleteDirTool_Execute_NonExistent(t *testing.T) {
	tmpDir := t.TempDir()
	tool := NewDeleteDirTool(tmpDir, false)
	ctx := context.Background()
	args := map[string]interface{}{
		"path": "nonexistent_dir",
	}

	result := tool.Execute(ctx, args)

	// Note: os.RemoveAll doesn't return an error if the directory doesn't exist
	// So this might succeed (idempotent operation)
	// We just verify it doesn't crash
	_ = result.ForLLM
}

func TestDeleteDirTool_Execute_MissingPath(t *testing.T) {
	tmpDir := t.TempDir()
	tool := NewDeleteDirTool(tmpDir, false)
	ctx := context.Background()
	args := map[string]interface{}{}

	result := tool.Execute(ctx, args)

	if !result.IsError {
		t.Error("Expected error for missing path parameter")
	}
}

func TestFileTools_RelativePathResolution(t *testing.T) {
	tmpDir := t.TempDir()

	// Create a file in tmpDir
	os.WriteFile(filepath.Join(tmpDir, "test.txt"), []byte("content"), 0644)

	tool := NewReadFileTool(tmpDir, false)
	ctx := context.Background()
	args := map[string]interface{}{
		"path": "test.txt", // Relative path
	}

	result := tool.Execute(ctx, args)

	if result.IsError {
		t.Errorf("Failed to read file with relative path: %s", result.ForLLM)
	}

	if !strings.Contains(result.ForLLM, "content") {
		t.Errorf("Expected 'content' in result, got: %s", result.ForLLM)
	}
}

func TestFileTools_EmptyPathHandling(t *testing.T) {
	tmpDir := t.TempDir()
	tool := NewReadFileTool(tmpDir, false)
	ctx := context.Background()
	args := map[string]interface{}{
		"path": "",
	}

	result := tool.Execute(ctx, args)

	if !result.IsError {
		t.Error("Expected error for empty path")
	}
}

func TestListDirTool_Execute_EmptyDirectory(t *testing.T) {
	tmpDir := t.TempDir()

	tool := NewListDirTool(tmpDir, false)
	ctx := context.Background()
	args := map[string]interface{}{
		"path": tmpDir,
	}

	result := tool.Execute(ctx, args)

	if result.IsError {
		t.Errorf("ListDir failed on empty directory: %s", result.ForLLM)
	}

	// Result may be empty string for empty directory
	// Just verify it doesn't crash
	_ = result.ForLLM
}

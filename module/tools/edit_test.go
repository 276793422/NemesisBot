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

func TestEditFileTool_Execute_Success(t *testing.T) {
	tempDir := t.TempDir()
	testFile := filepath.Join(tempDir, "test.txt")
	initialContent := "Hello World\nLine 2\nLine 3"
	err := os.WriteFile(testFile, []byte(initialContent), 0644)
	if err != nil {
		t.Fatalf("Failed to create test file: %v", err)
	}

	tool := NewEditFileTool(tempDir, true)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"path":     "test.txt",
		"old_text": "Line 2",
		"new_text": "Modified Line 2",
	})

	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}

	// Verify the edit
	content, err := os.ReadFile(testFile)
	if err != nil {
		t.Fatalf("Failed to read file: %v", err)
	}

	expectedContent := "Hello World\nModified Line 2\nLine 3"
	if string(content) != expectedContent {
		t.Errorf("Expected content '%s', got '%s'", expectedContent, string(content))
	}
}

func TestEditFileTool_Execute_OldTextNotFound(t *testing.T) {
	tempDir := t.TempDir()
	testFile := filepath.Join(tempDir, "test.txt")
	_ = os.WriteFile(testFile, []byte("Hello World"), 0644)

	tool := NewEditFileTool(tempDir, true)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"path":     "test.txt",
		"old_text": "NonExistent",
		"new_text": "New Text",
	})

	if !result.IsError {
		t.Error("Expected error when old_text not found")
	}

	if !strings.Contains(result.ForLLM, "old_text not found") {
		t.Errorf("Expected 'old_text not found' error, got '%s'", result.ForLLM)
	}
}

func TestEditFileTool_Execute_MultipleOccurrences(t *testing.T) {
	tempDir := t.TempDir()
	testFile := filepath.Join(tempDir, "test.txt")
	initialContent := "Line 1\nLine 2\nLine 2\nLine 3"
	_ = os.WriteFile(testFile, []byte(initialContent), 0644)

	tool := NewEditFileTool(tempDir, true)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"path":     "test.txt",
		"old_text": "Line 2",
		"new_text": "Modified Line 2",
	})

	if !result.IsError {
		t.Error("Expected error when old_text appears multiple times")
	}

	if !strings.Contains(result.ForLLM, "appears") && !strings.Contains(result.ForLLM, "times") {
		t.Errorf("Expected error about multiple occurrences, got '%s'", result.ForLLM)
	}
}

func TestEditFileTool_Execute_FileNotFound(t *testing.T) {
	tempDir := t.TempDir()
	tool := NewEditFileTool(tempDir, true)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"path":     "nonexistent.txt",
		"old_text": "text",
		"new_text": "new",
	})

	if !result.IsError {
		t.Error("Expected error for non-existent file")
	}

	if !strings.Contains(result.ForLLM, "file not found") {
		t.Errorf("Expected 'file not found' error, got '%s'", result.ForLLM)
	}
}

func TestEditFileTool_Execute_MissingPath(t *testing.T) {
	tool := NewEditFileTool("", false)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"old_text": "text",
		"new_text": "new",
	})

	if !result.IsError {
		t.Error("Expected error for missing path parameter")
	}

	if !strings.Contains(result.ForLLM, "path is required") {
		t.Errorf("Expected 'path is required' error, got '%s'", result.ForLLM)
	}
}

func TestEditFileTool_Execute_MissingOldText(t *testing.T) {
	tool := NewEditFileTool("", false)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"path":     "test.txt",
		"new_text": "new",
	})

	if !result.IsError {
		t.Error("Expected error for missing old_text parameter")
	}
}

func TestEditFileTool_Execute_MissingNewText(t *testing.T) {
	tool := NewEditFileTool("", false)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"path":     "test.txt",
		"old_text": "old",
	})

	if !result.IsError {
		t.Error("Expected error for missing new_text parameter")
	}
}

func TestEditFileTool_Execute_PathTraversalBlocked(t *testing.T) {
	tempDir := t.TempDir()
	tool := NewEditFileTool(tempDir, true)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"path":     "../../../etc/passwd",
		"old_text": "root",
		"new_text": "hacker",
	})

	if !result.IsError {
		t.Error("Expected error for path traversal attempt")
	}
}

func TestAppendFileTool_Execute_Success(t *testing.T) {
	tempDir := t.TempDir()
	testFile := filepath.Join(tempDir, "test.txt")
	initialContent := "Line 1\n"
	_ = os.WriteFile(testFile, []byte(initialContent), 0644)

	tool := NewAppendFileTool(tempDir, true)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"path":    "test.txt",
		"content": "Line 2\n",
	})

	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}

	// Verify the append
	content, err := os.ReadFile(testFile)
	if err != nil {
		t.Fatalf("Failed to read file: %v", err)
	}

	expectedContent := "Line 1\nLine 2\n"
	if string(content) != expectedContent {
		t.Errorf("Expected content '%s', got '%s'", expectedContent, string(content))
	}
}

func TestAppendFileTool_Execute_CreateFile(t *testing.T) {
	tempDir := t.TempDir()
	tool := NewAppendFileTool(tempDir, true)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"path":    "newfile.txt",
		"content": "New content\n",
	})

	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}

	// Verify file was created
	testFile := filepath.Join(tempDir, "newfile.txt")
	content, err := os.ReadFile(testFile)
	if err != nil {
		t.Fatalf("Failed to read file: %v", err)
	}

	if string(content) != "New content\n" {
		t.Errorf("Expected 'New content\\n', got '%s'", string(content))
	}
}

func TestAppendFileTool_Execute_MissingPath(t *testing.T) {
	tool := NewAppendFileTool("", false)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"content": "text",
	})

	if !result.IsError {
		t.Error("Expected error for missing path parameter")
	}
}

func TestAppendFileTool_Execute_MissingContent(t *testing.T) {
	tool := NewAppendFileTool("", false)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"path": "test.txt",
	})

	if !result.IsError {
		t.Error("Expected error for missing content parameter")
	}
}

func TestAppendFileTool_Execute_PathTraversalBlocked(t *testing.T) {
	tempDir := t.TempDir()
	tool := NewAppendFileTool(tempDir, true)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"path":    "../../../etc/passwd",
		"content": "malicious content",
	})

	if !result.IsError {
		t.Error("Expected error for path traversal attempt")
	}
}

func TestEditFileTool_Name(t *testing.T) {
	tool := NewEditFileTool("", false)
	if tool.Name() != "edit_file" {
		t.Errorf("Expected name 'edit_file', got '%s'", tool.Name())
	}
}

func TestEditFileTool_Description(t *testing.T) {
	tool := NewEditFileTool("", false)
	desc := tool.Description()
	if desc == "" {
		t.Error("Description should not be empty")
	}
}

func TestEditFileTool_Parameters(t *testing.T) {
	tool := NewEditFileTool("", false)
	params := tool.Parameters()

	if params == nil {
		t.Error("Parameters should not be nil")
	}

	if params["type"] != "object" {
		t.Errorf("Expected type 'object', got '%v'", params["type"])
	}

	props, ok := params["properties"].(map[string]interface{})
	if !ok {
		t.Fatal("Properties should be a map")
	}

	// Check required parameters
	required, ok := params["required"].([]string)
	if !ok {
		t.Fatal("Required should be a string slice")
	}

	if len(required) != 3 {
		t.Errorf("Expected 3 required parameters, got %d", len(required))
	}

	// Check that path, old_text, new_text are in properties
	if _, ok := props["path"]; !ok {
		t.Error("path should be in properties")
	}
	if _, ok := props["old_text"]; !ok {
		t.Error("old_text should be in properties")
	}
	if _, ok := props["new_text"]; !ok {
		t.Error("new_text should be in properties")
	}
}

func TestAppendFileTool_Name(t *testing.T) {
	tool := NewAppendFileTool("", false)
	if tool.Name() != "append_file" {
		t.Errorf("Expected name 'append_file', got '%s'", tool.Name())
	}
}

func TestAppendFileTool_Description(t *testing.T) {
	tool := NewAppendFileTool("", false)
	desc := tool.Description()
	if desc == "" {
		t.Error("Description should not be empty")
	}
}

func TestAppendFileTool_Parameters(t *testing.T) {
	tool := NewAppendFileTool("", false)
	params := tool.Parameters()

	if params == nil {
		t.Error("Parameters should not be nil")
	}

	props, ok := params["properties"].(map[string]interface{})
	if !ok {
		t.Fatal("Properties should be a map")
	}

	// Check required parameters
	required, ok := params["required"].([]string)
	if !ok {
		t.Fatal("Required should be a string slice")
	}

	if len(required) != 2 {
		t.Errorf("Expected 2 required parameters, got %d", len(required))
	}

	// Check that path and content are in properties
	if _, ok := props["path"]; !ok {
		t.Error("path should be in properties")
	}
	if _, ok := props["content"]; !ok {
		t.Error("content should be in properties")
	}
}

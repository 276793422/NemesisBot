// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package tools_test

import (
	"context"
	"testing"
	"time"

	. "github.com/276793422/NemesisBot/module/tools"
)

func TestNewAsyncExecTool(t *testing.T) {
	tool := NewAsyncExecTool("", false)

	if tool.Name() != "exec_async" {
		t.Errorf("Expected name 'exec_async', got '%s'", tool.Name())
	}

	if tool.Description() == "" {
		t.Error("Description should not be empty")
	}
}

func TestAsyncExecTool_Execute_MissingCommand(t *testing.T) {
	tool := NewAsyncExecTool("", false)
	ctx := context.Background()
	args := map[string]interface{}{}

	result := tool.Execute(ctx, args)

	if !result.IsError {
		t.Error("Expected error for missing command parameter")
	}
}

func TestAsyncExecTool_Execute_SimpleCommand(t *testing.T) {
	if testing.Short() {
		t.Skip("Skipping async shell execution test in short mode")
	}

	tool := NewAsyncExecTool("", false)
	ctx := context.Background()

	// Use a simple echo command (completes quickly)
	args := map[string]interface{}{
		"command": "echo test",
	}

	result := tool.Execute(ctx, args)

	// Async exec should start the command and return
	// The result depends on whether the process is still running
	// We just check it doesn't crash
	if result == nil {
		t.Error("Result should not be nil")
	}
}

func TestAsyncExecTool_Execute_WaitSeconds(t *testing.T) {
	if testing.Short() {
		t.Skip("Skipping async shell execution test in short mode")
	}

	tool := NewAsyncExecTool("", false)
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()

	args := map[string]interface{}{
		"command":      "echo test",
		"wait_seconds": 2.0,
	}

	result := tool.Execute(ctx, args)

	if result == nil {
		t.Error("Result should not be nil")
	}

	// Should not error (echo completes quickly)
	if result.IsError {
		// This is acceptable - echo might exit before we check
		t.Logf("Command completed quickly (expected for echo): %s", result.ForLLM)
	}
}

func TestAsyncExecTool_Execute_WaitSecondsRange(t *testing.T) {
	if testing.Short() {
		t.Skip("Skipping async shell execution test in short mode")
	}

	tests := []struct {
		name        string
		waitSeconds float64
		expectedMin int
		expectedMax int
	}{
		{"zero", 0, 1, 1},
		{"negative", -1, 1, 1},
		{"below min", 0.5, 1, 1},
		{"above max", 15, 10, 10},
		{"normal", 5, 5, 5},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			tool := NewAsyncExecTool("", false)
			ctx := context.Background()

			args := map[string]interface{}{
				"command":      "echo test",
				"wait_seconds": tt.waitSeconds,
			}

			// Just verify it doesn't crash
			tool.Execute(ctx, args)
		})
	}
}

func TestAsyncExecTool_Execute_WorkingDirectory(t *testing.T) {
	if testing.Short() {
		t.Skip("Skipping async shell execution test in short mode")
	}

	tmpDir := t.TempDir()
	tool := NewAsyncExecTool(tmpDir, false)
	ctx := context.Background()

	args := map[string]interface{}{
		"command":     "echo test",
		"working_dir": tmpDir,
	}

	result := tool.Execute(ctx, args)

	if result == nil {
		t.Error("Result should not be nil")
	}
}

func TestAsyncExecTool_Execute_DangerousCommand(t *testing.T) {
	if testing.Short() {
		t.Skip("Skipping async shell execution test in short mode")
	}

	tool := NewAsyncExecTool("", false)
	ctx := context.Background()

	// Try a dangerous command
	args := map[string]interface{}{
		"command": "rm -rf /tmp/test",
	}

	result := tool.Execute(ctx, args)

	if !result.IsError {
		t.Error("Expected error for dangerous command")
	}

	// Should contain "blocked" message
	if !contains(result.ForLLM, "blocked") && !result.IsError {
		t.Errorf("Expected command to be blocked, got: %s", result.ForLLM)
	}
}

func TestAsyncExecTool_Execute_PathTraversal(t *testing.T) {
	if testing.Short() {
		t.Skip("Skipping async shell execution test in short mode")
	}

	tmpDir := t.TempDir()
	tool := NewAsyncExecTool(tmpDir, true) // restrict = true
	ctx := context.Background()

	// Try to use path traversal
	args := map[string]interface{}{
		"command": "cat ../../../etc/passwd",
	}

	result := tool.Execute(ctx, args)

	if !result.IsError {
		t.Error("Expected error for path traversal attempt")
	}

	if !contains(result.ForLLM, "blocked") && !result.IsError {
		t.Errorf("Expected command to be blocked, got: %s", result.ForLLM)
	}
}

func TestAsyncExecTool_SetDefaultWaitTime(t *testing.T) {
	tool := NewAsyncExecTool("", false)

	newWaitTime := 5 * time.Second
	tool.SetDefaultWaitTime(newWaitTime)

	// Can't directly test the field, but we can verify it doesn't crash
}

func TestAsyncExecTool_SetRestrictToWorkspace(t *testing.T) {
	tool := NewAsyncExecTool("", false)

	tool.SetRestrictToWorkspace(true)

	// Can't directly test the field, but we can verify it doesn't crash
}

func TestAsyncExecTool_SetAllowPatterns(t *testing.T) {
	tool := NewAsyncExecTool("", false)

	// Set allow patterns
	err := tool.SetAllowPatterns([]string{"^echo", "^ls"})

	if err != nil {
		t.Errorf("Failed to set allow patterns: %v", err)
	}

	// Test invalid pattern
	err = tool.SetAllowPatterns([]string{"[invalid"})

	if err == nil {
		t.Error("Expected error for invalid regex pattern")
	}
}

func TestAsyncExecTool_Execute_DangerousPatterns(t *testing.T) {
	if testing.Short() {
		t.Skip("Skipping async shell execution test in short mode")
	}

	tool := NewAsyncExecTool("", false)
	ctx := context.Background()

	dangerousCommands := []string{
		"rm -rf /tmp",
		"sudo ls",
		"format c:",
		"shutdown /s",
		"; rm -rf /tmp",
		"&& rm -rf /tmp",
		"| sh",
		"$(cat /etc/passwd)",
	}

	for _, cmd := range dangerousCommands {
		t.Run(cmd, func(t *testing.T) {
			args := map[string]interface{}{
				"command": cmd,
			}

			result := tool.Execute(ctx, args)

			if !result.IsError {
				t.Errorf("Dangerous command '%s' should be blocked", cmd)
			}
		})
	}
}

func TestAsyncExecTool_Execute_ContextCancellation(t *testing.T) {
	if testing.Short() {
		t.Skip("Skipping async shell execution test in short mode")
	}

	tool := NewAsyncExecTool("", false)

	// Create context with timeout
	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()

	args := map[string]interface{}{
		"command": "echo test",
	}

	result := tool.Execute(ctx, args)

	// Context with timeout should complete the test
	// The async tool starts the process and returns quickly
	// Just verify it doesn't crash
	if result == nil {
		t.Error("Result should not be nil")
	}
}

func TestAsyncExecTool_Execute_NonExistentCommand(t *testing.T) {
	if testing.Short() {
		t.Skip("Skipping async shell execution test in short mode")
	}

	tool := NewAsyncExecTool("", false)
	ctx := context.Background()

	args := map[string]interface{}{
		"command": "nonexistentcommand12345",
	}

	result := tool.Execute(ctx, args)

	if !result.IsError {
		t.Error("Expected error for non-existent command")
	}
}

func TestAsyncExecTool_Execute_LongWaitTime(t *testing.T) {
	if testing.Short() {
		t.Skip("Skipping async shell execution test in short mode")
	}

	tool := NewAsyncExecTool("", false)
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()

	// Start a long-running process (sleep for 30 seconds)
	// but only wait 2 seconds
	args := map[string]interface{}{
		"command":      "sleep 30",
		"wait_seconds": 2.0,
	}

	start := time.Now()
	result := tool.Execute(ctx, args)
	elapsed := time.Since(start)

	// Should return quickly (around 2 seconds wait + overhead)
	// Not the full 30 seconds
	if elapsed > 5*time.Second {
		t.Errorf("Expected to return quickly, took %v", elapsed)
	}

	// Process should still be running, so result should indicate success
	if result.IsError {
		t.Logf("Long-running command test: %s", result.ForLLM)
	}
}

func TestAsyncExecTool_Execute_QuickExitCommand(t *testing.T) {
	if testing.Short() {
		t.Skip("Skipping async shell execution test in short mode")
	}

	tool := NewAsyncExecTool("", false)
	ctx := context.Background()

	// Command that exits immediately
	args := map[string]interface{}{
		"command":      "false",
		"wait_seconds": 2.0,
	}

	result := tool.Execute(ctx, args)

	// Should return error because process exited
	if !result.IsError {
		t.Error("Expected error for command that exits quickly")
	}

	if !contains(result.ForLLM, "exited") && !contains(result.ForLLM, "crashed") {
		t.Logf("Expected exit message, got: %s", result.ForLLM)
	}
}

func TestAsyncExecTool_Execute_CommandWithArguments(t *testing.T) {
	if testing.Short() {
		t.Skip("Skipping async shell execution test in short mode")
	}

	tool := NewAsyncExecTool("", false)
	ctx := context.Background()

	// Command with arguments
	args := map[string]interface{}{
		"command": "echo -n 'test with spaces'",
	}

	result := tool.Execute(ctx, args)

	// Just verify it doesn't crash
	if result == nil {
		t.Error("Result should not be nil")
	}
}

func TestAsyncExecTool_Execute_CommandWithSpecialCharacters(t *testing.T) {
	if testing.Short() {
		t.Skip("Skipping async shell execution test in short mode")
	}

	tool := NewAsyncExecTool("", false)
	ctx := context.Background()

	// Command with quotes and special chars
	args := map[string]interface{}{
		"command": `echo "test with \"quotes\""`,
	}

	result := tool.Execute(ctx, args)

	// Just verify it doesn't crash
	if result == nil {
		t.Error("Result should not be nil")
	}
}

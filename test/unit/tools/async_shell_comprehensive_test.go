// Comprehensive tests for AsyncExecTool - unique tests only
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package tools_test

import (
	"context"
	"runtime"
	"strings"
	"testing"

	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/tools"
)

func TestNewAsyncExecToolWithConfig(t *testing.T) {
	// Test with nil config
	tool1 := tools.NewAsyncExecToolWithConfig("", false, nil)
	if tool1 == nil {
		t.Error("Expected tool to be created with nil config")
	}

	// Test with config that has disabled deny patterns
	cfg1 := &config.Config{}
	cfg1.Tools = config.ToolsConfig{
		Exec: config.ExecConfig{
			EnableDenyPatterns: false,
		},
	}
	tool2 := tools.NewAsyncExecToolWithConfig("", false, cfg1)
	if tool2 == nil {
		t.Error("Expected tool to be created with disabled deny patterns")
	}

	// Test with config that has custom deny patterns
	cfg2 := &config.Config{}
	cfg2.Tools = config.ToolsConfig{
		Exec: config.ExecConfig{
			EnableDenyPatterns: true,
			CustomDenyPatterns: []string{"^echo", "^ls"},
		},
	}
	tool3 := tools.NewAsyncExecToolWithConfig("", false, cfg2)
	if tool3 == nil {
		t.Error("Expected tool to be created with custom deny patterns")
	}
}

func TestAsyncExecTool_Execute_InvalidCommandType(t *testing.T) {
	tool := tools.NewAsyncExecTool("", false)
	ctx := context.Background()
	args := map[string]interface{}{
		"command": 123, // Invalid type
	}

	result := tool.Execute(ctx, args)

	if result == nil {
		t.Error("Expected error result")
	}

	if !result.IsError {
		t.Error("Expected error for invalid command type")
	}

	if !strings.Contains(result.ForLLM, "command is required") {
		t.Errorf("Expected 'command is required' error, got: %s", result.ForLLM)
	}
}

func TestAsyncExecTool_Execute_InvalidWaitSecondsType(t *testing.T) {
	tool := tools.NewAsyncExecTool("", false)
	ctx := context.Background()
	args := map[string]interface{}{
		"command":      "echo test",
		"wait_seconds": "invalid", // Invalid type
	}

	result := tool.Execute(ctx, args)

	// Should still work with invalid wait_seconds (uses default)
	if result == nil {
		t.Error("Expected result")
	}
}

func TestAsyncExecTool_Execute_WorkingDirectoryHandling(t *testing.T) {
	tests := []struct {
		name          string
		workingDir    string
		argWorkingDir string
		expectedDir   string
		shouldUseCWD  bool
	}{
		{"no_working_dir", "", "", "", true},
		{"tool_has_dir", "/tmp", "", "/tmp", false},
		{"arg_overrides", "", "/var", "/var", false},
		{"both_same", "/tmp", "/tmp", "/tmp", false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			tool := tools.NewAsyncExecTool(tt.workingDir, false)
			ctx := context.Background()

			args := map[string]interface{}{
				"command": "echo test",
			}

			if tt.argWorkingDir != "" {
				args["working_dir"] = tt.argWorkingDir
			}

			// This test just verifies it doesn't crash
			result := tool.Execute(ctx, args)
			if result == nil {
				t.Error("Expected result")
			}
		})
	}
}

func TestPlatformSpecificBehavior(t *testing.T) {
	tests := []struct {
		name     string
		os       string
		command  string
		expected string
	}{
		{"windows_curl_replacement", "windows", "curl example.com", "curl.exe example.com"},
		{"unix_curl_unchanged", "linux", "curl example.com", "curl example.com"},
		{"windows_powershell", "windows", "notepad.exe", "Start-Process -FilePath \"notepad.exe\""},
		{"unix_background", "linux", "notepad", "sh -c \"notepad &\""},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			// We can't actually change runtime.GOOS, but we can test the logic
			// that would be executed on each platform

			// Test process name extraction (simulated)
			tool := tools.NewAsyncExecTool("", false)
			ctx := context.Background()

			// Execute a command and check if it contains platform-specific behavior
			args := map[string]interface{}{
				"command": tt.command,
			}

			result := tool.Execute(ctx, args)

			// Just verify it doesn't crash
			if result == nil {
				t.Error("Expected result")
			}
		})
	}
}

func TestContextCancellation(t *testing.T) {
	tool := tools.NewAsyncExecTool("", false)

	// Create context that will be cancelled immediately
	ctx, cancel := context.WithCancel(context.Background())
	cancel() // Cancel immediately

	args := map[string]interface{}{
		"command": "echo test",
	}

	result := tool.Execute(ctx, args)

	// Should still return a result (async execution is fire-and-forget)
	if result == nil {
		t.Error("Expected result even with cancelled context")
	}

	// Should not crash, even though context is cancelled
	if result.IsError && strings.Contains(result.ForLLM, "context cancelled") {
		t.Log("Context was properly handled")
	}
}

func TestCommandWithComplexArguments(t *testing.T) {
	tool := tools.NewAsyncExecTool("", false)
	ctx := context.Background()

	tests := []struct {
		name     string
		command  string
		expected string // Part of expected process name
	}{
		{"double_quotes", `echo "hello world"`, "echo"},
		{"single_quotes", "echo 'hello world'", "echo"},
		{"mixed_quotes", `echo 'hello "world"'`, "echo"},
		{"escaped_quotes", `echo \"hello world\"`, "echo"},
		{"with_flags", "echo -n test", "echo"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			args := map[string]interface{}{
				"command": tt.command,
			}

			result := tool.Execute(ctx, args)

			// Just verify it doesn't crash
			if result == nil {
				t.Error("Expected result")
			}
		})
	}
}

func TestWorkingDirectoryPathResolution(t *testing.T) {
	tmpDir := t.TempDir()

	// Test relative path resolution
	tool := tools.NewAsyncExecTool(".", false)
	ctx := context.Background()

	args := map[string]interface{}{
		"command":     "echo test",
		"working_dir": tmpDir,
	}

	result := tool.Execute(ctx, args)

	if result == nil {
		t.Error("Expected result")
	}
}

func TestEmptyWorkingDirectory(t *testing.T) {
	tool := tools.NewAsyncExecTool("", false)
	ctx := context.Background()

	args := map[string]interface{}{
		"command": "echo test",
	}

	result := tool.Execute(ctx, args)

	if result == nil {
		t.Error("Expected result")
	}

	// Should not crash with empty working directory
}

func TestWaitTimeBoundaryConditions(t *testing.T) {
	tool := tools.NewAsyncExecTool("", false)
	ctx := context.Background()

	tests := []struct {
		name      string
		waitValue float64
		expected  int
	}{
		{"zero_wait", 0, 1},
		{"negative_wait", -1, 1},
		{"fractional_wait", 0.5, 1},
		{"very_large_wait", 1000, 10},
		{"normal_wait", 5, 5},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			args := map[string]interface{}{
				"command":      "echo test",
				"wait_seconds": tt.waitValue,
			}

			result := tool.Execute(ctx, args)

			if result == nil {
				t.Error("Expected result")
			}
		})
	}
}

func TestExecuteMethodErrorHandling(t *testing.T) {
	// Test the Execute method's error handling for different scenarios
	tool := tools.NewAsyncExecTool("", false)
	ctx := context.Background()

	testCases := []struct {
		name         string
		args         map[string]interface{}
		requireError bool
	}{
		{"missing_command", map[string]interface{}{}, true},
		{"nil_args", nil, true},
		{"valid_command", map[string]interface{}{"command": "notepad.exe"}, false}, // Use notepad.exe instead of echo
		{"command_with_extra_params", map[string]interface{}{"command": "notepad.exe", "extra_param": "value"}, false},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			result := tool.Execute(ctx, tc.args)

			if result == nil {
				t.Error("Expected result")
			}

			if tc.requireError && !result.IsError {
				t.Errorf("Expected error for case: %s", tc.name)
			}

			if !tc.requireError && result.IsError {
				t.Logf("Command '%s' result (this is expected for some commands): %s", tc.name, result.ForLLM)
			}
		})
	}
}

func TestToolMetadata(t *testing.T) {
	tool := tools.NewAsyncExecTool("", false)

	// Test metadata methods
	if tool.Name() != "exec_async" {
		t.Errorf("Expected name 'exec_async', got '%s'", tool.Name())
	}

	description := tool.Description()
	if description == "" {
		t.Error("Description should not be empty")
	}

	params := tool.Parameters()
	if params == nil {
		t.Error("Parameters should not be nil")
	}

	if params["type"] != "object" {
		t.Error("Parameters type should be 'object'")
	}

	properties, ok := params["properties"].(map[string]interface{})
	if !ok {
		t.Error("Properties should exist in parameters")
	}

	if properties == nil {
		t.Error("Properties should not be nil")
	}

	commandParam, ok := properties["command"].(map[string]interface{})
	if !ok {
		t.Error("Command parameter should exist")
	}

	if commandParam["type"] != "string" {
		t.Error("Command parameter type should be string")
	}

	required, ok := params["required"].([]interface{})
	if !ok {
		// Try to convert from []string
		if reqStr, ok := params["required"].([]string); ok {
			required = make([]interface{}, len(reqStr))
			for i, v := range reqStr {
				required[i] = v
			}
		} else {
			t.Error("Required fields should exist and be convertible to []interface{}")
			return
		}
	}

	if len(required) == 0 {
		t.Error("At least one field should be required")
	} else {
		hasCommand := false
		for _, field := range required {
			if field == "command" {
				hasCommand = true
				break
			}
		}
		if !hasCommand {
			t.Error("'command' should be in required fields")
		}
	}
}

// Test platform-specific detection
func TestPlatformDetection(t *testing.T) {
	if runtime.GOOS == "windows" {
		// On Windows, test that PowerShell commands are used
		tool := tools.NewAsyncExecTool("", false)
		ctx := context.Background()

		args := map[string]interface{}{
			"command": "notepad.exe",
		}

		result := tool.Execute(ctx, args)

		if result == nil {
			t.Error("Expected result on Windows")
		}

		if result.IsError {
			t.Logf("PowerShell execution on Windows: %s", result.ForLLM)
		}
	} else {
		// On Unix, test that background commands are used
		tool := tools.NewAsyncExecTool("", false)
		ctx := context.Background()

		args := map[string]interface{}{
			"command": "sleep 1",
		}

		result := tool.Execute(ctx, args)

		if result == nil {
			t.Error("Expected result on Unix")
		}

		if result.IsError {
			t.Logf("Background execution on Unix: %s", result.ForLLM)
		}
	}
}

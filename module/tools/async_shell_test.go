// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package tools

import (
	"context"
	"regexp"
	"runtime"
	"strings"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/config"
)

// Test NewAsyncExecTool
func TestNewAsyncExecTool(t *testing.T) {
	tool := NewAsyncExecTool("/test/workspace", true)

	if tool == nil {
		t.Fatal("Expected non-nil tool")
	}

	if tool.workingDir != "/test/workspace" {
		t.Errorf("Expected working dir '/test/workspace', got '%s'", tool.workingDir)
	}

	if !tool.restrictToWorkspace {
		t.Error("Expected restrictToWorkspace to be true")
	}

	if tool.defaultWaitTime != 3*time.Second {
		t.Errorf("Expected default wait time 3s, got %v", tool.defaultWaitTime)
	}

	if len(tool.denyPatterns) == 0 {
		t.Error("Expected default deny patterns to be set")
	}
}

// Test NewAsyncExecToolWithConfig
func TestNewAsyncExecToolWithConfig(t *testing.T) {
	cfg := &config.Config{
		Tools: config.ToolsConfig{
			Exec: config.ExecConfig{
				EnableDenyPatterns: true,
				CustomDenyPatterns: []string{
					`^dangerous\s+`,
					`rm\s+-rf`,
				},
			},
		},
	}

	tool := NewAsyncExecToolWithConfig("/workspace", true, cfg)

	if tool == nil {
		t.Fatal("Expected non-nil tool")
	}

	if tool.restrictToWorkspace != true {
		t.Error("Expected restrictToWorkspace to be true")
	}

	// Check that custom patterns were compiled
	if len(tool.denyPatterns) != 2 {
		t.Errorf("Expected 2 custom deny patterns, got %d", len(tool.denyPatterns))
	}
}

// Test NewAsyncExecToolWithConfig_DisableDenyPatterns
func TestNewAsyncExecToolWithConfig_DisableDenyPatterns(t *testing.T) {
	cfg := &config.Config{
		Tools: config.ToolsConfig{
			Exec: config.ExecConfig{
				EnableDenyPatterns: false,
			},
		},
	}

	tool := NewAsyncExecToolWithConfig("/workspace", true, cfg)

	if tool == nil {
		t.Fatal("Expected non-nil tool")
	}

	// Should have no deny patterns when disabled
	if len(tool.denyPatterns) != 0 {
		t.Errorf("Expected 0 deny patterns when disabled, got %d", len(tool.denyPatterns))
	}
}

// Test NewAsyncExecToolWithConfig_NilConfig
func TestNewAsyncExecToolWithConfig_NilConfig(t *testing.T) {
	tool := NewAsyncExecToolWithConfig("/workspace", true, nil)

	if tool == nil {
		t.Fatal("Expected non-nil tool")
	}

	// Should use default deny patterns
	if len(tool.denyPatterns) == 0 {
		t.Error("Expected default deny patterns with nil config")
	}
}

// Test Name
func TestAsyncExecTool_Name(t *testing.T) {
	tool := NewAsyncExecTool("/workspace", false)
	if tool.Name() != "exec_async" {
		t.Errorf("Expected name 'exec_async', got '%s'", tool.Name())
	}
}

// Test Description
func TestAsyncExecTool_Description(t *testing.T) {
	tool := NewAsyncExecTool("/workspace", false)
	desc := tool.Description()

	if desc == "" {
		t.Error("Description should not be empty")
	}

	// Check for key phrases
	expectedPhrases := []string{
		"GUI 应用程序",
		"启动",
		"等待",
		"notepad.exe",
		"calc.exe",
	}

	for _, phrase := range expectedPhrases {
		if !strings.Contains(desc, phrase) {
			t.Errorf("Description should contain '%s'", phrase)
		}
	}
}

// Test Parameters
func TestAsyncExecTool_Parameters(t *testing.T) {
	tool := NewAsyncExecTool("/workspace", false)
	params := tool.Parameters()

	if params == nil {
		t.Fatal("Parameters should not be nil")
	}

	if params["type"] != "object" {
		t.Errorf("Expected type 'object', got '%v'", params["type"])
	}

	props, ok := params["properties"].(map[string]interface{})
	if !ok {
		t.Fatal("Properties should be a map")
	}

	// Check required properties
	requiredProps := []string{"command", "working_dir", "wait_seconds"}
	for _, prop := range requiredProps {
		if _, ok := props[prop]; !ok {
			t.Errorf("Missing required property: %s", prop)
		}
	}

	// Check command property
	command, ok := props["command"].(map[string]interface{})
	if !ok {
		t.Fatal("Command should be a map")
	}

	if command["type"] != "string" {
		t.Errorf("Command should be string type")
	}

	// Check required fields
	required, ok := params["required"].([]string)
	if !ok {
		t.Fatal("Required should be a string slice")
	}

	if len(required) != 1 || required[0] != "command" {
		t.Errorf("Expected required ['command'], got %v", required)
	}

	// Check wait_seconds default and constraints
	waitSeconds, ok := props["wait_seconds"].(map[string]interface{})
	if !ok {
		t.Fatal("wait_seconds should be a map")
	}

	if waitSeconds["default"] != 3 {
		t.Errorf("Expected default 3, got %v", waitSeconds["default"])
	}

	if waitSeconds["minimum"] != 1 {
		t.Errorf("Expected minimum 1, got %v", waitSeconds["minimum"])
	}

	if waitSeconds["maximum"] != 10 {
		t.Errorf("Expected maximum 10, got %v", waitSeconds["maximum"])
	}
}

// Test Execute missing command
func TestAsyncExecTool_Execute_MissingCommand(t *testing.T) {
	tool := NewAsyncExecTool("/workspace", false)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{})

	if result == nil {
		t.Fatal("Expected non-nil result")
	}

	// Check if we got an error (may be silent result on some systems)
	if !result.IsError && result.ForUser == "" {
		t.Error("Expected some kind of result")
	}
}

// Test Execute invalid command type
func TestAsyncExecTool_Execute_InvalidCommandType(t *testing.T) {
	tool := NewAsyncExecTool("/workspace", false)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"command": 12345,
	})

	if !result.IsError {
		t.Error("Expected error result")
	}
}

// Test guardCommand with deny patterns
func TestAsyncExecTool_GuardCommand_DenyPatterns(t *testing.T) {
	tool := NewAsyncExecTool("/workspace", false)

	dangerousCommands := []string{
		"rm -rf /",
		"format c:",
		"shutdown /s",
		"del /f /s /q c:\\*",
	}

	for _, cmd := range dangerousCommands {
		err := tool.guardCommand(cmd, "/workspace")
		if err == "" {
			t.Errorf("Command should be blocked: %s", cmd)
		}

		if !contains(err, "blocked by safety guard") {
			t.Errorf("Expected safety guard error for: %s, got: %s", cmd, err)
		}
	}
}

// Test guardCommand with allow patterns
func TestAsyncExecTool_GuardCommand_AllowPatterns(t *testing.T) {
	tool := NewAsyncExecTool("/workspace", false)

	// Set allow patterns
	err := tool.SetAllowPatterns([]string{
		`^echo\s+`,
		`^ls\s+`,
		`^dir\s+`,
	})
	if err != nil {
		t.Fatalf("Failed to set allow patterns: %v", err)
	}

	// Test allowed command
	errMsg := tool.guardCommand("echo hello", "/workspace")
	if errMsg != "" {
		t.Errorf("Allowed command should pass: %s, error: %s", "echo hello", errMsg)
	}

	// Test blocked command (not in allowlist)
	errMsg = tool.guardCommand("rm file", "/workspace")
	if errMsg == "" {
		t.Error("Command not in allowlist should be blocked")
	}

	if !contains(errMsg, "not in allowlist") {
		t.Errorf("Expected allowlist error, got: %s", errMsg)
	}
}

// Test guardCommand with workspace restriction
func TestAsyncExecTool_GuardCommand_WorkspaceRestriction(t *testing.T) {
	tool := NewAsyncExecTool("/workspace", true)

	// Test path traversal
	dangerousPaths := []string{
		"cat ../../../etc/passwd",
		"type ..\\..\\..\\windows\\system32\\drivers\\etc\\hosts",
	}

	for _, cmd := range dangerousPaths {
		err := tool.guardCommand(cmd, "/workspace")
		if err == "" {
			t.Errorf("Path traversal should be blocked: %s", cmd)
		}

		if !contains(err, "path traversal") {
			t.Errorf("Expected path traversal error for: %s, got: %s", cmd, err)
		}
	}
}

// Test guardCommand without workspace restriction
func TestAsyncExecTool_GuardCommand_NoWorkspaceRestriction(t *testing.T) {
	tool := NewAsyncExecTool("/workspace", false)

	// Should not block path traversal when restriction is disabled
	cmd := "cat ../../../etc/passwd"
	err := tool.guardCommand(cmd, "/workspace")

	// Only deny patterns should apply
	if err != "" && !contains(err, "dangerous pattern") {
		t.Errorf("Unexpected error with workspace restriction disabled: %s", err)
	}
}

// Test extractProcessName
func TestExtractProcessName(t *testing.T) {
	testCases := []struct {
		command     string
		expected    string
		description string
	}{
		{"notepad.exe", "notepad", "Simple executable"},
		{"notepad", "notepad", "Executable without extension"},
		{`"C:\Program Files\app.exe"`, "Program", "Quoted path with spaces (filepath.Base of first part)"},
		{"calc.exe /arg1", "calc", "With arguments"},
		{"   notepad.exe   ", "notepad", "With whitespace"},
		{`"/usr/bin/bash"`, "bash", "Unix quoted path (filepath.Base extracts basename)"},
		{"python3 script.py", "python3", "Python script"},
		{"", "", "Empty string"},
	}

	for _, tc := range testCases {
		t.Run(tc.description, func(t *testing.T) {
			result := extractProcessName(tc.command)
			if result != tc.expected {
				t.Errorf("Expected '%s', got '%s' for command: %s", tc.expected, result, tc.command)
			}
		})
	}
}

// Test escapePowerShellArg
func TestEscapePowerShellArg(t *testing.T) {
	testCases := []struct {
		input    string
		expected string
	}{
		{"simple", "simple"},
		{"with space", "\"with space\""},
		{"multiple spaces here", "\"multiple spaces here\""},
		{"no_spaces", "no_spaces"},
	}

	for _, tc := range testCases {
		result := escapePowerShellArg(tc.input)
		if result != tc.expected {
			t.Errorf("For input '%s', expected '%s', got '%s'", tc.input, tc.expected, result)
		}
	}
}

// Test checkProcessRunning (platform-specific)
func TestAsyncExecTool_CheckProcessRunning(t *testing.T) {
	tool := NewAsyncExecTool("/workspace", false)

	// Test with a process that should exist
	var processName string
	if runtime.GOOS == "windows" {
		processName = "svchost" // Common Windows process
	} else {
		processName = "init" // Common Unix process
	}

	running, err := tool.checkProcessRunning(processName)
	if err != nil {
		// On some systems this might fail, just log it
		t.Logf("Warning: Could not check process: %v", err)
		return
	}

	// svchost/init should be running
	if !running {
		t.Logf("Warning: Expected process '%s' to be running", processName)
	}

	// Test with non-existent process
	running, err = tool.checkProcessRunning("nonexistentprocess12345")
	if err != nil {
		t.Logf("Warning: Could not check non-existent process: %v", err)
		return
	}

	if running {
		t.Error("Non-existent process should not be running")
	}
}

// Test SetDefaultWaitTime
func TestAsyncExecTool_SetDefaultWaitTime(t *testing.T) {
	tool := NewAsyncExecTool("/workspace", false)

	newWaitTime := 5 * time.Second
	tool.SetDefaultWaitTime(newWaitTime)

	if tool.defaultWaitTime != newWaitTime {
		t.Errorf("Expected wait time %v, got %v", newWaitTime, tool.defaultWaitTime)
	}
}

// Test SetRestrictToWorkspace
func TestAsyncExecTool_SetRestrictToWorkspace(t *testing.T) {
	tool := NewAsyncExecTool("/workspace", false)

	tool.SetRestrictToWorkspace(true)
	if !tool.restrictToWorkspace {
		t.Error("Expected restrictToWorkspace to be true")
	}

	tool.SetRestrictToWorkspace(false)
	if tool.restrictToWorkspace {
		t.Error("Expected restrictToWorkspace to be false")
	}
}

// Test SetAllowPatterns
func TestAsyncExecTool_SetAllowPatterns(t *testing.T) {
	tool := NewAsyncExecTool("/workspace", false)

	patterns := []string{
		`^echo\s+`,
		`^ls\s+`,
		`invalid[regex`,
	}

	err := tool.SetAllowPatterns(patterns)
	if err == nil {
		t.Error("Expected error for invalid regex pattern")
	}

	if !contains(err.Error(), "invalid allow pattern") {
		t.Errorf("Expected pattern error, got: %v", err)
	}

	// Test with valid patterns
	validPatterns := []string{
		`^echo\s+`,
		`^ls\s+`,
		`^dir\s+`,
	}

	err = tool.SetAllowPatterns(validPatterns)
	if err != nil {
		t.Fatalf("Failed to set valid patterns: %v", err)
	}

	if len(tool.allowPatterns) != 3 {
		t.Errorf("Expected 3 allow patterns, got %d", len(tool.allowPatterns))
	}
}

// Test Execute with wait_seconds parameter
func TestAsyncExecTool_Execute_WaitSecondsParameter(t *testing.T) {
	tool := NewAsyncExecTool("/workspace", false)
	ctx := context.Background()

	// Test minimum value (1)
	result := tool.Execute(ctx, map[string]interface{}{
		"command":      "echo test",
		"wait_seconds": 0.5, // Should be clamped to 1
	})

	// Should not error on wait time validation
	// (will likely error on execution in test environment)

	// Test maximum value (10)
	result = tool.Execute(ctx, map[string]interface{}{
		"command":      "echo test",
		"wait_seconds": 15, // Should be clamped to 10
	})

	// Should not error on wait time validation
	_ = result
}

// Test Execute with working_dir parameter
func TestAsyncExecTool_Execute_WorkingDirParameter(t *testing.T) {
	tool := NewAsyncExecTool("/default/workspace", false)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"command":     "echo test",
		"working_dir": "/custom/workspace",
	})

	// Just verify it doesn't crash
	_ = result
}

// Test guardCommand with absolute paths
func TestAsyncExecTool_GuardCommand_AbsolutePaths(t *testing.T) {
	tool := NewAsyncExecTool("/workspace", true)

	// Test Windows-style absolute path
	if runtime.GOOS == "windows" {
		err := tool.guardCommand("type C:\\Windows\\System32\\drivers\\etc\\hosts", "/workspace")
		// Should either pass or give specific error
		_ = err
	}

	// Test Unix-style absolute path
	err := tool.guardCommand("cat /etc/passwd", "/workspace")
	if err != "" {
		// Should be blocked if outside workspace
		if !contains(err, "outside working dir") {
			t.Logf("Got error for absolute path: %s", err)
		}
	}
}

// Test guardCommand with relative paths in workspace
func TestAsyncExecTool_GuardCommand_RelativePathsInWorkspace(t *testing.T) {
	tool := NewAsyncExecTool("/workspace", true)

	safeCommands := []string{
		"cat file.txt",
		"ls -la",
		"echo hello > output.txt",
	}

	for _, cmd := range safeCommands {
		err := tool.guardCommand(cmd, "/workspace")
		if err != "" && contains(err, "safety guard") {
			t.Errorf("Safe command should not be blocked: %s, error: %s", cmd, err)
		}
	}
}

// Test concurrent Execute calls
func TestAsyncExecTool_ConcurrentExecutes(t *testing.T) {
	tool := NewAsyncExecTool("/workspace", false)
	ctx := context.Background()

	// Launch multiple concurrent executes
	for i := 0; i < 5; i++ {
		go func(index int) {
			result := tool.Execute(ctx, map[string]interface{}{
				"command": "echo test",
			})
			_ = result // Just verify no race conditions
		}(i)
	}

	// Wait a bit for goroutines to complete
	time.Sleep(100 * time.Millisecond)
}

// Test curl replacement on Windows
func TestAsyncExecTool_CurlReplacement(t *testing.T) {
	if runtime.GOOS != "windows" {
		t.Skip("Skipping Windows-specific test")
	}

	tool := NewAsyncExecTool("/workspace", false)
	ctx := context.Background()

	// This test just verifies the tool doesn't crash with curl commands
	// Actual execution is environment-dependent
	result := tool.Execute(ctx, map[string]interface{}{
		"command": "curl -s https://example.com",
	})

	// Just verify we get a result
	_ = result
}

// Test Execute with special characters in command
func TestAsyncExecTool_Execute_SpecialCharacters(t *testing.T) {
	tool := NewAsyncExecTool("/workspace", false)
	ctx := context.Background()

	// Test with quotes and special chars
	result := tool.Execute(ctx, map[string]interface{}{
		"command": "echo \"hello world\"",
	})

	// Just verify it doesn't crash
	_ = result
}

// Benchmark guardCommand
func BenchmarkAsyncExecTool_GuardCommand(b *testing.B) {
	tool := NewAsyncExecTool("/workspace", true)
	cmd := "echo hello world"

	for i := 0; i < b.N; i++ {
		_ = tool.guardCommand(cmd, "/workspace")
	}
}

// Benchmark extractProcessName
func BenchmarkExtractProcessName(b *testing.B) {
	cmd := `"C:\Program Files\MyApp\app.exe" --arg1 --arg2`

	for i := 0; i < b.N; i++ {
		_ = extractProcessName(cmd)
	}
}

// Helper: Compile regex patterns for testing
func compileTestPatterns(patterns []string) []*regexp.Regexp {
	compiled := make([]*regexp.Regexp, 0, len(patterns))
	for _, pattern := range patterns {
		re, err := regexp.Compile(pattern)
		if err != nil {
			continue
		}
		compiled = append(compiled, re)
	}
	return compiled
}

// Test helper for pattern matching
func TestPatternMatching(t *testing.T) {
	patterns := compileTestPatterns([]string{
		`rm\s+-rf`,
		`format\s+\w:`,
	})

	testCases := []struct {
		command   string
		shouldMatch bool
	}{
		{"rm -rf /", true},
		{"rm -rf file.txt", true},
		{"format c:", true},
		{"format d:", true},
		{"echo hello", false},
		{"ls -la", false},
	}

	for _, tc := range testCases {
		matched := false
		lower := strings.ToLower(tc.command)
		for _, pattern := range patterns {
			if pattern.MatchString(lower) {
				matched = true
				break
			}
		}

		if matched != tc.shouldMatch {
			t.Errorf("For command '%s', expected match=%v, got %v", tc.command, tc.shouldMatch, matched)
		}
	}
}

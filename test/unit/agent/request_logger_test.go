// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package agent_test

import (
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/agent"
	"github.com/276793422/NemesisBot/module/config"
)

// TestNewRequestLogger_Disabled tests creating a logger when logging is disabled
func TestNewRequestLogger_Disabled(t *testing.T) {
	// Test with nil config
	logger := agent.NewRequestLogger(nil, "")
	if logger.IsEnabled() {
		t.Error("Expected logger to be disabled with nil config")
	}

	// Test with logging disabled
	cfg := &config.LoggingConfig{
		LLM: &config.LLMLogConfig{
			Enabled: false,
		},
	}
	logger = agent.NewRequestLogger(cfg, "")
	if logger.IsEnabled() {
		t.Error("Expected logger to be disabled when LLM.Enabled is false")
	}
}

// TestNewRequestLogger_Enabled tests creating a logger when logging is enabled
func TestNewRequestLogger_Enabled(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.LoggingConfig{
		LLM: &config.LLMLogConfig{
			Enabled:     true,
			LogDir:      "logs/request_logs",
			DetailLevel: "full",
		},
	}

	logger := agent.NewRequestLogger(cfg, tempDir)
	if !logger.IsEnabled() {
		t.Error("Expected logger to be enabled")
	}

	// Test CreateSession
	if err := logger.CreateSession(); err != nil {
		t.Errorf("Failed to create session: %v", err)
	}
}

// TestNewRequestLogger_RelativePath tests that relative paths are resolved correctly
func TestNewRequestLogger_RelativePath(t *testing.T) {
	workspace := t.TempDir()
	cfg := &config.LoggingConfig{
		LLM: &config.LLMLogConfig{
			Enabled:     true,
			LogDir:      "logs/request_logs",
			DetailLevel: "full",
		},
	}

	logger := agent.NewRequestLogger(cfg, workspace)

	// Create session to trigger directory creation
	if err := logger.CreateSession(); err != nil {
		t.Errorf("Failed to create session: %v", err)
	}

	// Verify that the log directory was created in the workspace
	expectedDir := filepath.Join(workspace, "logs", "request_logs")
	if _, err := os.Stat(expectedDir); os.IsNotExist(err) {
		t.Errorf("Expected log directory %s to be created", expectedDir)
	}
}

// TestNewRequestLogger_AbsolutePath tests that absolute paths are used as-is
func TestNewRequestLogger_AbsolutePath(t *testing.T) {
	workspace := t.TempDir()
	logDir := t.TempDir() // Use temp dir as absolute path

	cfg := &config.LoggingConfig{
		LLM: &config.LLMLogConfig{
			Enabled:     true,
			LogDir:      logDir,
			DetailLevel: "full",
		},
	}

	logger := agent.NewRequestLogger(cfg, workspace)

	// Create session
	if err := logger.CreateSession(); err != nil {
		t.Errorf("Failed to create session: %v", err)
	}

	// Verify that files are created in the specified absolute path
	// The session directory should be inside logDir
	entries, err := os.ReadDir(logDir)
	if err != nil {
		t.Errorf("Failed to read log directory: %v", err)
	}

	if len(entries) == 0 {
		t.Error("Expected session directory to be created in logDir")
	}
}

// TestNewRequestLogger_TildeExpansion tests tilde expansion
func TestNewRequestLogger_TildeExpansion(t *testing.T) {
	homeDir, err := os.UserHomeDir()
	if err != nil {
		t.Skip("Cannot get home directory for tilde expansion test")
	}

	// Create a subdirectory in home for testing
	testLogDir := filepath.Join(homeDir, "nemesisbot_test_logs")
	defer os.RemoveAll(testLogDir)

	cfg := &config.LoggingConfig{
		LLM: &config.LLMLogConfig{
			Enabled:     true,
			LogDir:      "~/nemesisbot_test_logs",
			DetailLevel: "full",
		},
	}

	logger := agent.NewRequestLogger(cfg, "")
	if err := logger.CreateSession(); err != nil {
		t.Errorf("Failed to create session: %v", err)
	}

	// Verify directory was created in home directory
	if _, err := os.Stat(testLogDir); os.IsNotExist(err) {
		t.Errorf("Expected tilde to expand to home directory, %s not found", testLogDir)
	}
}

// TestNewRequestLogger_EmptyLogDir tests default behavior when log dir is empty
func TestNewRequestLogger_EmptyLogDir(t *testing.T) {
	workspace := t.TempDir()

	cfg := &config.LoggingConfig{
		LLM: &config.LLMLogConfig{
			Enabled:     true,
			LogDir:      "", // Empty should use workspace
			DetailLevel: "full",
		},
	}

	logger := agent.NewRequestLogger(cfg, workspace)
	if !logger.IsEnabled() {
		t.Error("Expected logger to be enabled")
	}

	// Session creation should still work
	if err := logger.CreateSession(); err != nil {
		t.Errorf("Failed to create session: %v", err)
	}
}

// TestNewRequestLogger_UnixStylePath tests Unix-style absolute paths on Windows
func TestNewRequestLogger_UnixStylePath(t *testing.T) {
	if os.PathSeparator == '/' {
		t.Skip("This test is primarily for Windows compatibility")
	}

	workspace := t.TempDir()

	// Unix-style absolute path should be treated as absolute
	cfg := &config.LoggingConfig{
		LLM: &config.LLMLogConfig{
			Enabled:     true,
			LogDir:      "/var/log/nemesisbot",
			DetailLevel: "full",
		},
	}

	logger := agent.NewRequestLogger(cfg, workspace)

	// Should not panic, and logger should be enabled
	if !logger.IsEnabled() {
		t.Error("Expected logger to be enabled")
	}

	// CreateSession may fail on Windows (cannot create /var/log),
	// but should not panic
	_ = logger.CreateSession()
}

// TestRequestLogger_LogFileCreation tests that log files are created correctly
func TestRequestLogger_LogFileCreation(t *testing.T) {
	workspace := t.TempDir()
	cfg := &config.LoggingConfig{
		LLM: &config.LLMLogConfig{
			Enabled:     true,
			LogDir:      "logs",
			DetailLevel: "full",
		},
	}

	logger := agent.NewRequestLogger(cfg, workspace)
	if err := logger.CreateSession(); err != nil {
		t.Fatalf("Failed to create session: %v", err)
	}

	// Try to log a user request
	err := logger.LogUserRequest(agent.UserRequestInfo{
		Timestamp: time.Now(),
		Channel:   "test",
		SenderID:  "user123",
		ChatID:    "chat456",
		Content:   "Hello, world!",
	})

	if err != nil {
		t.Errorf("Failed to log user request: %v", err)
	}

	// Verify that files were created in the workspace
	logDir := filepath.Join(workspace, "logs")
	entries, err := os.ReadDir(logDir)
	if err != nil {
		t.Errorf("Failed to read log directory: %v", err)
	}

	if len(entries) == 0 {
		t.Error("Expected log files to be created")
	}
}

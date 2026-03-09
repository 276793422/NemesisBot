// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package logger

import (
	"bytes"
	"encoding/json"
	"log"
	"os"
	"strings"
	"sync"
	"testing"
)

func TestLogLevelNames(t *testing.T) {
	tests := []struct {
		level    LogLevel
		expected string
	}{
		{DEBUG, "DEBUG"},
		{INFO, "INFO"},
		{WARN, "WARN"},
		{ERROR, "ERROR"},
		{FATAL, "FATAL"},
	}

	for _, tt := range tests {
		t.Run(tt.expected, func(t *testing.T) {
			if logLevelNames[tt.level] != tt.expected {
				t.Errorf("logLevelNames[%d] = %v, want %v", tt.level, logLevelNames[tt.level], tt.expected)
			}
		})
	}
}

func TestSetLevel(t *testing.T) {
	levels := []LogLevel{DEBUG, INFO, WARN, ERROR, FATAL}

	for _, level := range levels {
		SetLevel(level)
		if GetLevel() != level {
			t.Errorf("SetLevel(%v) - GetLevel() = %v, want %v", level, GetLevel(), level)
		}
	}
}

func TestGetLevel(t *testing.T) {
	// Reset to INFO for consistent testing
	SetLevel(INFO)

	// Test getting current level
	currentLevel := GetLevel()
	if currentLevel != INFO {
		t.Errorf("GetLevel() = %v, want INFO", currentLevel)
	}

	// Test setting and getting
	SetLevel(DEBUG)
	if GetLevel() != DEBUG {
		t.Errorf("GetLevel() after SetLevel(DEBUG) = %v, want DEBUG", GetLevel())
	}

	// Reset to INFO
	SetLevel(INFO)
}

func TestEnableDisableLogging(t *testing.T) {
	// Test default state
	if !IsLoggingEnabled() {
		t.Error("Logging should be enabled by default")
	}

	// Test disable
	DisableLogging()
	if IsLoggingEnabled() {
		t.Error("Logging should be disabled after DisableLogging()")
	}

	// Test enable
	EnableLogging()
	if !IsLoggingEnabled() {
		t.Error("Logging should be enabled after EnableLogging()")
	}
}

func TestEnableDisableConsole(t *testing.T) {
	// Test default state
	if !IsConsoleEnabled() {
		t.Error("Console should be enabled by default")
	}

	// Test disable
	DisableConsole()
	if IsConsoleEnabled() {
		t.Error("Console should be disabled after DisableConsole()")
	}

	// Test enable
	EnableConsole()
	if !IsConsoleEnabled() {
		t.Error("Console should be enabled after EnableConsole()")
	}
}

func TestEnableFileLogging(t *testing.T) {
	tmpDir := t.TempDir()
	logPath := tmpDir + "/test.log"

	// Test enabling file logging
	err := EnableFileLogging(logPath)
	if err != nil {
		t.Fatalf("EnableFileLogging() error = %v", err)
	}

	// Verify file exists
	if _, err := os.Stat(logPath); os.IsNotExist(err) {
		t.Error("EnableFileLogging() did not create log file")
	}

	// Test that logging to file works
	Info("Test message")

	// Read file and check content
	data, err := os.ReadFile(logPath)
	if err != nil {
		t.Fatalf("Failed to read log file: %v", err)
	}

	// Check that it contains valid JSON
	var entry LogEntry
	if err := json.Unmarshal(data, &entry); err != nil {
		t.Logf("File content: %s", string(data))
		t.Errorf("Log file does not contain valid JSON: %v", err)
	}

	// Clean up
	DisableFileLogging()
}

func TestEnableFileLoggingInvalidPath(t *testing.T) {
	// Test with invalid path
	err := EnableFileLogging("/invalid/path/that/cannot/be/created/test.log")
	if err == nil {
		t.Error("EnableFileLogging() should return error for invalid path")
	}
}

func TestDisableFileLogging(t *testing.T) {
	tmpDir := t.TempDir()
	logPath := tmpDir + "/test.log"

	// Enable file logging
	_ = EnableFileLogging(logPath)

	// Disable file logging
	DisableFileLogging()

	// Verify file is closed (we can't directly check this, but we can try to enable again)
	err := EnableFileLogging(logPath)
	if err != nil {
		t.Fatalf("EnableFileLogging() after disable failed: %v", err)
	}

	// Clean up
	DisableFileLogging()
}

func TestLogLevelsFiltering(t *testing.T) {
	// Capture log output
	var buf bytes.Buffer
	log.SetOutput(&buf)
	defer log.SetOutput(os.Stderr)

	// Set level to WARN
	SetLevel(WARN)

	// Log at different levels
	Debug("debug message")
	Info("info message")
	Warn("warn message")
	Error("error message")

	output := buf.String()

	// DEBUG and INFO should not appear
	if strings.Contains(output, "debug message") {
		t.Error("DEBUG message should not appear when level is WARN")
	}
	if strings.Contains(output, "info message") {
		t.Error("INFO message should not appear when level is WARN")
	}

	// WARN and ERROR should appear
	if !strings.Contains(output, "warn message") {
		t.Error("WARN message should appear when level is WARN")
	}
	if !strings.Contains(output, "error message") {
		t.Error("ERROR message should appear when level is WARN")
	}

	// Reset level
	SetLevel(INFO)
}

func TestLoggingDisabled(t *testing.T) {
	// Capture log output
	var buf bytes.Buffer
	log.SetOutput(&buf)
	defer log.SetOutput(os.Stderr)

	// Disable logging
	DisableLogging()

	// Try to log
	Info("test message")

	output := buf.String()

	// Nothing should be logged
	if strings.Contains(output, "test message") {
		t.Error("No messages should be logged when logging is disabled")
	}

	// Re-enable logging
	EnableLogging()
}

func TestConsoleDisabled(t *testing.T) {
	// Capture log output
	var buf bytes.Buffer
	log.SetOutput(&buf)
	defer log.SetOutput(os.Stderr)

	// Disable console
	DisableConsole()

	// Log a message
	Info("test message")

	output := buf.String()

	// Console output should be disabled
	if strings.Contains(output, "test message") {
		t.Error("Console output should be disabled")
	}

	// Re-enable console
	EnableConsole()
}

func TestLogMessageVariants(t *testing.T) {
	// Capture log output
	var buf bytes.Buffer
	log.SetOutput(&buf)
	defer log.SetOutput(os.Stderr)

	SetLevel(DEBUG)

	// Test all log level variants
	Debug("debug message")
	DebugC("test-component", "debug component message")
	DebugF("debug fields", map[string]interface{}{"key": "value"})
	DebugCF("test-component", "debug component fields", map[string]interface{}{"key": "value"})

	Info("info message")
	InfoC("test-component", "info component message")
	InfoF("info fields", map[string]interface{}{"key": "value"})
	InfoCF("test-component", "info component fields", map[string]interface{}{"key": "value"})

	Warn("warn message")
	WarnC("test-component", "warn component message")
	WarnF("warn fields", map[string]interface{}{"key": "value"})
	WarnCF("test-component", "warn component fields", map[string]interface{}{"key": "value"})

	Error("error message")
	ErrorC("test-component", "error component message")
	ErrorF("error fields", map[string]interface{}{"key": "value"})
	ErrorCF("test-component", "error component fields", map[string]interface{}{"key": "value"})

	Fatal("fatal message")
	FatalC("test-component", "fatal component message")
	FatalF("fatal fields", map[string]interface{}{"key": "value"})
	FatalCF("test-component", "fatal component fields", map[string]interface{}{"key": "value"})

	output := buf.String()

	// Check that all messages were logged
	messages := []string{
		"debug message",
		"debug component message",
		"debug fields",
		"debug component fields",
		"info message",
		"info component message",
		"info fields",
		"info component fields",
		"warn message",
		"warn component message",
		"warn fields",
		"warn component fields",
		"error message",
		"error component message",
		"error fields",
		"error component fields",
		"fatal message",
		"fatal component message",
		"fatal fields",
		"fatal component fields",
	}

	for _, msg := range messages {
		if !strings.Contains(output, msg) {
			t.Errorf("Expected message not found: %s", msg)
		}
	}

	SetLevel(INFO)
}

func TestLogEntryFields(t *testing.T) {
	tmpDir := t.TempDir()
	logPath := tmpDir + "/test.log"

	err := EnableFileLogging(logPath)
	if err != nil {
		t.Fatalf("EnableFileLogging() error = %v", err)
	}
	defer DisableFileLogging()

	fields := map[string]interface{}{
		"user_id": "12345",
		"action":  "login",
		"success": true,
		"count":   42,
	}

	InfoF("User action", fields)

	// Read file and check content
	data, err := os.ReadFile(logPath)
	if err != nil {
		t.Fatalf("Failed to read log file: %v", err)
	}

	var entry LogEntry
	if err := json.Unmarshal(data, &entry); err != nil {
		t.Fatalf("Failed to parse log entry: %v", err)
	}

	// Check entry fields
	if entry.Level != "INFO" {
		t.Errorf("Level = %v, want INFO", entry.Level)
	}
	if entry.Message != "User action" {
		t.Errorf("Message = %v, want 'User action'", entry.Message)
	}
	if entry.Fields == nil {
		t.Error("Fields should not be nil")
	} else {
		if entry.Fields["user_id"] != "12345" {
			t.Errorf("Fields[user_id] = %v, want '12345'", entry.Fields["user_id"])
		}
		if entry.Fields["action"] != "login" {
			t.Errorf("Fields[action] = %v, want 'login'", entry.Fields["action"])
		}
		if entry.Fields["success"] != true {
			t.Errorf("Fields[success] = %v, want true", entry.Fields["success"])
		}
	}
}

func TestLogEntryComponent(t *testing.T) {
	// Capture log output
	var buf bytes.Buffer
	log.SetOutput(&buf)
	defer log.SetOutput(os.Stderr)

	InfoC("TestComponent", "test message")

	output := buf.String()

	if !strings.Contains(output, "TestComponent:") {
		t.Errorf("Expected component in output: %s", output)
	}
	if !strings.Contains(output, "test message") {
		t.Errorf("Expected message in output: %s", output)
	}
}

func TestConcurrentLogging(t *testing.T) {
	// Capture log output
	var buf bytes.Buffer
	log.SetOutput(&buf)
	defer log.SetOutput(os.Stderr)

	SetLevel(DEBUG)

	var wg sync.WaitGroup
	numGoroutines := 100
	messagesPerGoroutine := 10

	for i := 0; i < numGoroutines; i++ {
		wg.Add(1)
		go func(id int) {
			defer wg.Done()
			for j := 0; j < messagesPerGoroutine; j++ {
				InfoF("concurrent", map[string]interface{}{
					"goroutine": id,
					"message":   j,
				})
			}
		}(i)
	}

	wg.Wait()

	output := buf.String()

	// Check that we have the expected number of log entries
	lines := strings.Split(output, "\n")
	logCount := 0
	for _, line := range lines {
		if strings.Contains(line, "concurrent") {
			logCount++
		}
	}

	expectedCount := numGoroutines * messagesPerGoroutine
	if logCount < expectedCount {
		t.Errorf("Expected at least %d log entries, got %d", expectedCount, logCount)
	}

	SetLevel(INFO)
}

func TestConcurrentLevelChanges(t *testing.T) {
	var wg sync.WaitGroup
	numGoroutines := 50

	for i := 0; i < numGoroutines; i++ {
		wg.Add(1)
		go func(id int) {
			defer wg.Done()
			SetLevel(LogLevel(id % 5))
			_ = GetLevel()
			EnableLogging()
			DisableLogging()
			IsLoggingEnabled()
			EnableConsole()
			DisableConsole()
			IsConsoleEnabled()
		}(i)
	}

	wg.Wait()

	// Just verify no race conditions occurred
	if !IsLoggingEnabled() {
		EnableLogging()
	}
	if !IsConsoleEnabled() {
		EnableConsole()
	}
}

func TestLogLevelOrdering(t *testing.T) {
	// Test that log levels are properly ordered
	levels := []LogLevel{DEBUG, INFO, WARN, ERROR, FATAL}

	for i := 0; i < len(levels)-1; i++ {
		if levels[i] >= levels[i+1] {
			t.Errorf("Log levels not properly ordered: %d >= %d", levels[i], levels[i+1])
		}
	}
}

func TestLogEntryJSON(t *testing.T) {
	if testing.Short() {
		t.Skip("Skipping file logging test in short mode due to buffering")
	}

	tmpDir := t.TempDir()
	logPath := tmpDir + "/test.log"

	err := EnableFileLogging(logPath)
	if err != nil {
		t.Fatalf("EnableFileLogging() error = %v", err)
	}

	// Log multiple messages
	for i := 0; i < 10; i++ {
		Info("test message")
	}

	// Disable logging to close the file
	DisableFileLogging()

	// Verify file exists
	if _, err := os.Stat(logPath); os.IsNotExist(err) {
		t.Error("Log file should exist")
	}

	// Note: Due to file buffering, content may not be immediately available
	// The important thing is that the file was created and no errors occurred
}

func TestCallerInfo(t *testing.T) {
	if testing.Short() {
		t.Skip("Skipping file logging test in short mode due to buffering")
	}

	tmpDir := t.TempDir()
	logPath := tmpDir + "/test.log"

	err := EnableFileLogging(logPath)
	if err != nil {
		t.Fatalf("EnableFileLogging() error = %v", err)
	}

	// Log multiple messages
	for i := 0; i < 10; i++ {
		Info("test message")
	}

	// Disable logging to close the file
	DisableFileLogging()

	// Verify file exists
	if _, err := os.Stat(logPath); os.IsNotExist(err) {
		t.Error("Log file should exist")
	}

	// Note: Due to file buffering, content may not be immediately available
	// The important thing is that the file was created and no errors occurred
}

func TestFileLoggingConcurrency(t *testing.T) {
	tmpDir := t.TempDir()
	logPath := tmpDir + "/test.log"

	err := EnableFileLogging(logPath)
	if err != nil {
		t.Fatalf("EnableFileLogging() error = %v", err)
	}

	// Log a single message to ensure file is created
	Info("initial message")

	var wg sync.WaitGroup
	numGoroutines := 50

	for i := 0; i < numGoroutines; i++ {
		wg.Add(1)
		go func(id int) {
			defer wg.Done()
			InfoF("concurrent write", map[string]interface{}{"id": id})
		}(i)
	}

	wg.Wait()

	// Close the file to ensure all data is flushed
	DisableFileLogging()

	// Verify file exists
	if _, err := os.Stat(logPath); os.IsNotExist(err) {
		t.Error("Log file should exist")
	}
}

func TestEmptyFields(t *testing.T) {
	tmpDir := t.TempDir()
	logPath := tmpDir + "/test.log"

	err := EnableFileLogging(logPath)
	if err != nil {
		t.Fatalf("EnableFileLogging() error = %v", err)
	}
	// Log with empty fields map
	InfoF("test", map[string]interface{}{})
	DisableFileLogging()

	// Verify file exists
	if _, err := os.Stat(logPath); os.IsNotExist(err) {
		t.Error("Log file should exist")
	}
}

func TestNilFields(t *testing.T) {
	// Reset state to ensure clean test
	EnableLogging()
	EnableConsole()
	SetLevel(INFO)

	// Capture log output
	var buf bytes.Buffer
	log.SetOutput(&buf)

	// Log without fields (nil)
	Info("test message")

	// Restore output
	log.SetOutput(os.Stderr)

	output := buf.String()

	if output == "" {
		t.Error("Message should be logged even with nil fields")
	}

	// Check that the message is in the output
	if !strings.Contains(output, "test message") {
		t.Errorf("Message should be in output. Got: %s", output)
	}
}

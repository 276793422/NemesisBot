// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package logger_test

import (
	"bytes"
	"log"
	"os"
	"strings"
	"sync"
	"testing"
	"time"

	. "github.com/276793422/NemesisBot/module/logger"
)

// TestLoggingEnabledDisabled tests the master switch functionality
func TestLoggingEnabledDisabled(t *testing.T) {
	// Save original state
	originalEnabled := IsLoggingEnabled()

	tests := []struct {
		name          string
		enable        bool
		expectEnabled bool
	}{
		{"Disable logging", false, false},
		{"Enable logging", true, true},
		{"Disable again", false, false},
		{"Enable again", true, true},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if tt.enable {
				EnableLogging()
			} else {
				DisableLogging()
			}

			if got := IsLoggingEnabled(); got != tt.expectEnabled {
				t.Errorf("IsLoggingEnabled() = %v, want %v", got, tt.expectEnabled)
			}
		})
	}

	// Restore original state
	if originalEnabled {
		EnableLogging()
	} else {
		DisableLogging()
	}
}

// TestConsoleEnabledDisabled tests the console switch functionality
func TestConsoleEnabledDisabled(t *testing.T) {
	// Save original state
	originalEnabled := IsConsoleEnabled()

	tests := []struct {
		name          string
		enable        bool
		expectEnabled bool
	}{
		{"Disable console", false, false},
		{"Enable console", true, true},
		{"Disable again", false, false},
		{"Enable again", true, true},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if tt.enable {
				EnableConsole()
			} else {
				DisableConsole()
			}

			if got := IsConsoleEnabled(); got != tt.expectEnabled {
				t.Errorf("IsConsoleEnabled() = %v, want %v", got, tt.expectEnabled)
			}
		})
	}

	// Restore original state
	if originalEnabled {
		EnableConsole()
	} else {
		DisableConsole()
	}
}

// TestDoubleLayerControl tests the interaction between master and console switches
func TestDoubleLayerControl(t *testing.T) {
	// Save original state
	originalLoggingEnabled := IsLoggingEnabled()
	originalConsoleEnabled := IsConsoleEnabled()
	originalLevel := GetLevel()

	// Set up for test
	EnableLogging()
	EnableConsole()
	SetLevel(INFO)

	tests := []struct {
		name               string
		loggingEnabled     bool
		consoleEnabled     bool
		shouldLogToFile    bool
		shouldLogToConsole bool
	}{
		{
			name:               "Both enabled",
			loggingEnabled:     true,
			consoleEnabled:     true,
			shouldLogToFile:    true,
			shouldLogToConsole: true,
		},
		{
			name:               "Master disabled, console enabled",
			loggingEnabled:     false,
			consoleEnabled:     true,
			shouldLogToFile:    false,
			shouldLogToConsole: false,
		},
		{
			name:               "Master enabled, console disabled",
			loggingEnabled:     true,
			consoleEnabled:     false,
			shouldLogToFile:    true,
			shouldLogToConsole: false,
		},
		{
			name:               "Both disabled",
			loggingEnabled:     false,
			consoleEnabled:     false,
			shouldLogToFile:    false,
			shouldLogToConsole: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			// Set up switches
			if tt.loggingEnabled {
				EnableLogging()
			} else {
				DisableLogging()
			}

			if tt.consoleEnabled {
				EnableConsole()
			} else {
				DisableConsole()
			}

			// Create temp file for file logging
			tempFile, err := os.CreateTemp("", "logger_test_*.log")
			if err != nil {
				t.Fatalf("Failed to create temp file: %v", err)
			}
			tempPath := tempFile.Name()
			tempFile.Close()
			defer os.Remove(tempPath)

			// Enable file logging
			if err := EnableFileLogging(tempPath); err != nil {
				t.Fatalf("Failed to enable file logging: %v", err)
			}
			defer DisableFileLogging()

			// Capture console output
			var buf bytes.Buffer
			log.SetOutput(&buf)

			// Log a message
			testMessage := "Test double layer control"
			Info(testMessage)

			// Give time for write
			time.Sleep(10 * time.Millisecond)

			// Check file logging
			fileContent, err := os.ReadFile(tempPath)
			if err != nil {
				t.Fatalf("Failed to read log file: %v", err)
			}

			fileHasLog := strings.Contains(string(fileContent), testMessage)
			if fileHasLog != tt.shouldLogToFile {
				t.Errorf("File logging: got log=%v, want %v", fileHasLog, tt.shouldLogToFile)
			}

			// Check console logging
			consoleHasLog := buf.Len() > 0 && strings.Contains(buf.String(), testMessage)
			if consoleHasLog != tt.shouldLogToConsole {
				t.Errorf("Console logging: got log=%v, want %v", consoleHasLog, tt.shouldLogToConsole)
			}
		})
	}

	// Restore original state
	if originalLoggingEnabled {
		EnableLogging()
	} else {
		DisableLogging()
	}
	if originalConsoleEnabled {
		EnableConsole()
	} else {
		DisableConsole()
	}
	SetLevel(originalLevel)
}

// TestLogLevelFilteringWithSwitches tests that log level filtering still works with switches
func TestLogLevelFilteringWithSwitches(t *testing.T) {
	// Save original state
	originalLoggingEnabled := IsLoggingEnabled()
	originalConsoleEnabled := IsConsoleEnabled()
	originalLevel := GetLevel()

	// Enable both switches
	EnableLogging()
	EnableConsole()

	// Set to WARN level
	SetLevel(WARN)

	// Create temp file
	tempFile, err := os.CreateTemp("", "logger_test_*.log")
	if err != nil {
		t.Fatalf("Failed to create temp file: %v", err)
	}
	tempPath := tempFile.Name()
	tempFile.Close()
	defer os.Remove(tempPath)

	// Enable file logging
	if err := EnableFileLogging(tempPath); err != nil {
		t.Fatalf("Failed to enable file logging: %v", err)
	}
	defer DisableFileLogging()

	// Capture console output
	var buf bytes.Buffer
	log.SetOutput(&buf)

	// Log at different levels
	Debug("Debug message")
	Info("Info message")
	Warn("Warn message")
	Error("Error message")

	// Give time for writes
	time.Sleep(10 * time.Millisecond)

	// Check file content
	fileContent, err := os.ReadFile(tempPath)
	if err != nil {
		t.Fatalf("Failed to read log file: %v", err)
	}
	fileStr := string(fileContent)

	// DEBUG and INFO should not appear
	if strings.Contains(fileStr, "Debug message") {
		t.Error("DEBUG message should not appear at WARN level")
	}
	if strings.Contains(fileStr, "Info message") {
		t.Error("INFO message should not appear at WARN level")
	}

	// WARN and ERROR should appear
	if !strings.Contains(fileStr, "Warn message") {
		t.Error("WARN message should appear at WARN level")
	}
	if !strings.Contains(fileStr, "Error message") {
		t.Error("ERROR message should appear at WARN level")
	}

	// Check console content
	consoleStr := buf.String()
	if strings.Contains(consoleStr, "Debug message") {
		t.Error("DEBUG message should not appear in console at WARN level")
	}
	if strings.Contains(consoleStr, "Info message") {
		t.Error("INFO message should not appear in console at WARN level")
	}

	// Restore original state
	if originalLoggingEnabled {
		EnableLogging()
	} else {
		DisableLogging()
	}
	if originalConsoleEnabled {
		EnableConsole()
	} else {
		DisableConsole()
	}
	SetLevel(originalLevel)
}

// TestMasterSwitchDisablesAll tests that master switch disables both file and console logging
func TestMasterSwitchDisablesAll(t *testing.T) {
	// Save original state
	originalLoggingEnabled := IsLoggingEnabled()
	originalConsoleEnabled := IsConsoleEnabled()
	originalLevel := GetLevel()

	// Enable both switches initially
	EnableLogging()
	EnableConsole()
	SetLevel(INFO)

	// Create temp file
	tempFile, err := os.CreateTemp("", "logger_test_*.log")
	if err != nil {
		t.Fatalf("Failed to create temp file: %v", err)
	}
	tempPath := tempFile.Name()
	tempFile.Close()
	defer os.Remove(tempPath)

	// Enable file logging
	if err := EnableFileLogging(tempPath); err != nil {
		t.Fatalf("Failed to enable file logging: %v", err)
	}
	defer DisableFileLogging()

	// Disable master switch
	DisableLogging()

	// Capture console output
	var buf bytes.Buffer
	log.SetOutput(&buf)

	// Log at all levels
	testMessage := "Master switch test"
	Debug(testMessage)
	Info(testMessage)
	Warn(testMessage)
	Error(testMessage)

	// Give time for writes
	time.Sleep(10 * time.Millisecond)

	// Check file - should be empty
	fileContent, err := os.ReadFile(tempPath)
	if err != nil {
		t.Fatalf("Failed to read log file: %v", err)
	}
	if strings.Contains(string(fileContent), testMessage) {
		t.Error("File should not contain logs when master switch is disabled")
	}

	// Check console - should be empty
	if buf.Len() > 0 && strings.Contains(buf.String(), testMessage) {
		t.Error("Console should not contain logs when master switch is disabled")
	}

	// Restore original state
	if originalLoggingEnabled {
		EnableLogging()
	} else {
		DisableLogging()
	}
	if originalConsoleEnabled {
		EnableConsole()
	} else {
		DisableConsole()
	}
	SetLevel(originalLevel)
}

// TestConsoleSwitchDisablesOnlyConsole tests that console switch only affects console output
func TestConsoleSwitchDisablesOnlyConsole(t *testing.T) {
	// Save original state
	originalLoggingEnabled := IsLoggingEnabled()
	originalConsoleEnabled := IsConsoleEnabled()
	originalLevel := GetLevel()

	// Enable master switch, disable console
	EnableLogging()
	DisableConsole()
	SetLevel(INFO)

	// Create temp file
	tempFile, err := os.CreateTemp("", "logger_test_*.log")
	if err != nil {
		t.Fatalf("Failed to create temp file: %v", err)
	}
	tempPath := tempFile.Name()
	tempFile.Close()
	defer os.Remove(tempPath)

	// Enable file logging
	if err := EnableFileLogging(tempPath); err != nil {
		t.Fatalf("Failed to enable file logging: %v", err)
	}
	defer DisableFileLogging()

	// Capture console output
	var buf bytes.Buffer
	log.SetOutput(&buf)

	// Log a message
	testMessage := "Console switch test"
	Info(testMessage)

	// Give time for writes
	time.Sleep(10 * time.Millisecond)

	// Check file - should have log
	fileContent, err := os.ReadFile(tempPath)
	if err != nil {
		t.Fatalf("Failed to read log file: %v", err)
	}
	if !strings.Contains(string(fileContent), testMessage) {
		t.Error("File should contain logs when only console switch is disabled")
	}

	// Check console - should be empty
	if buf.Len() > 0 && strings.Contains(buf.String(), testMessage) {
		t.Error("Console should not contain logs when console switch is disabled")
	}

	// Restore original state
	if originalLoggingEnabled {
		EnableLogging()
	} else {
		DisableLogging()
	}
	if originalConsoleEnabled {
		EnableConsole()
	} else {
		DisableConsole()
	}
	SetLevel(originalLevel)
}

// TestConcurrentLogging tests thread safety of double-layer switches
func TestConcurrentLogging(t *testing.T) {
	// Save original state
	originalLoggingEnabled := IsLoggingEnabled()
	originalConsoleEnabled := IsConsoleEnabled()
	originalLevel := GetLevel()

	// Enable both switches
	EnableLogging()
	EnableConsole()
	SetLevel(INFO)

	// Create temp file
	tempFile, err := os.CreateTemp("", "logger_test_*.log")
	if err != nil {
		t.Fatalf("Failed to create temp file: %v", err)
	}
	tempPath := tempFile.Name()
	tempFile.Close()
	defer os.Remove(tempPath)

	// Enable file logging
	if err := EnableFileLogging(tempPath); err != nil {
		t.Fatalf("Failed to enable file logging: %v", err)
	}
	defer DisableFileLogging()

	// Run concurrent operations
	var wg sync.WaitGroup
	numGoroutines := 10
	numOperations := 100

	for i := 0; i < numGoroutines; i++ {
		wg.Add(1)
		go func(id int) {
			defer wg.Done()
			for j := 0; j < numOperations; j++ {
				// Mostly log, occasionally check/set switches
				// This tests thread safety while ensuring most logs get through
				switch j % 10 {
				case 0:
					// Occasionally check switch status (read operation)
					_ = IsLoggingEnabled()
					_ = IsConsoleEnabled()
				case 5:
					// Rarely toggle a switch
					if j%20 == 5 {
						DisableConsole()
					} else {
						EnableConsole()
					}
				default:
					// Mostly log messages
					InfoF("Concurrent log", map[string]interface{}{
						"goroutine": id,
						"operation": j,
					})
				}
			}
		}(i)
	}

	wg.Wait()

	// Restore state
	EnableLogging()
	EnableConsole()

	// Give time for writes
	time.Sleep(100 * time.Millisecond)

	// Check that file was written (most logs should have made it through)
	fileContent, err := os.ReadFile(tempPath)
	if err != nil {
		t.Fatalf("Failed to read log file: %v", err)
	}

	// We should have many logs in the file
	// 10 goroutines * 100 operations * ~80% logging = ~800 logs expected
	fileStr := string(fileContent)
	if len(fileStr) == 0 {
		t.Error("Expected logs in file after concurrent operations")
	}

	// Count how many log entries we have (each log entry should be a JSON object)
	logCount := 0
	for _, b := range fileStr {
		if b == '{' {
			logCount++
		}
	}

	// We should have at least 500 log entries (allowing for some loss)
	if logCount < 500 {
		t.Errorf("Expected at least 500 log entries, got %d", logCount)
	}

	t.Logf("Successfully logged %d entries concurrently", logCount)

	// Restore original state
	if originalLoggingEnabled {
		EnableLogging()
	} else {
		DisableLogging()
	}
	if originalConsoleEnabled {
		EnableConsole()
	} else {
		DisableConsole()
	}
	SetLevel(originalLevel)
}

// TestComponentLogging tests component-specific logging with switches
func TestComponentLogging(t *testing.T) {
	// Save original state
	originalLoggingEnabled := IsLoggingEnabled()
	originalConsoleEnabled := IsConsoleEnabled()
	originalLevel := GetLevel()

	// Enable both switches
	EnableLogging()
	EnableConsole()
	SetLevel(INFO)

	// Create temp file
	tempFile, err := os.CreateTemp("", "logger_test_*.log")
	if err != nil {
		t.Fatalf("Failed to create temp file: %v", err)
	}
	tempPath := tempFile.Name()
	tempFile.Close()
	defer os.Remove(tempPath)

	// Enable file logging
	if err := EnableFileLogging(tempPath); err != nil {
		t.Fatalf("Failed to enable file logging: %v", err)
	}
	defer DisableFileLogging()

	// Capture console output
	var buf bytes.Buffer
	log.SetOutput(&buf)

	// Log with components
	InfoC("agent", "Agent started")
	InfoC("gateway", "Gateway started")
	InfoC("channel", "Channel connected")

	// Give time for writes
	time.Sleep(10 * time.Millisecond)

	// Check file
	fileContent, err := os.ReadFile(tempPath)
	if err != nil {
		t.Fatalf("Failed to read log file: %v", err)
	}
	fileStr := string(fileContent)

	if !strings.Contains(fileStr, "agent") {
		t.Error("Expected 'agent' component in log file")
	}
	if !strings.Contains(fileStr, "gateway") {
		t.Error("Expected 'gateway' component in log file")
	}
	if !strings.Contains(fileStr, "channel") {
		t.Error("Expected 'channel' component in log file")
	}

	// Check console
	consoleStr := buf.String()
	if !strings.Contains(consoleStr, "agent") {
		t.Error("Expected 'agent' component in console output")
	}
	if !strings.Contains(consoleStr, "gateway") {
		t.Error("Expected 'gateway' component in console output")
	}

	// Restore original state
	if originalLoggingEnabled {
		EnableLogging()
	} else {
		DisableLogging()
	}
	if originalConsoleEnabled {
		EnableConsole()
	} else {
		DisableConsole()
	}
	SetLevel(originalLevel)
}

// TestFieldLogging tests field logging with switches
func TestFieldLogging(t *testing.T) {
	// Save original state
	originalLoggingEnabled := IsLoggingEnabled()
	originalConsoleEnabled := IsConsoleEnabled()
	originalLevel := GetLevel()

	// Enable both switches
	EnableLogging()
	EnableConsole()
	SetLevel(INFO)

	// Create temp file
	tempFile, err := os.CreateTemp("", "logger_test_*.log")
	if err != nil {
		t.Fatalf("Failed to create temp file: %v", err)
	}
	tempPath := tempFile.Name()
	tempFile.Close()
	defer os.Remove(tempPath)

	// Enable file logging
	if err := EnableFileLogging(tempPath); err != nil {
		t.Fatalf("Failed to enable file logging: %v", err)
	}
	defer DisableFileLogging()

	// Capture console output
	var buf bytes.Buffer
	log.SetOutput(&buf)

	// Log with fields
	InfoCF("test", "Test message", map[string]interface{}{
		"user_id": 12345,
		"action":  "login",
		"status":  "success",
	})

	// Give time for writes
	time.Sleep(10 * time.Millisecond)

	// Check file
	fileContent, err := os.ReadFile(tempPath)
	if err != nil {
		t.Fatalf("Failed to read log file: %v", err)
	}
	fileStr := string(fileContent)

	if !strings.Contains(fileStr, "user_id") {
		t.Error("Expected 'user_id' field in log file")
	}
	if !strings.Contains(fileStr, "action") {
		t.Error("Expected 'action' field in log file")
	}
	if !strings.Contains(fileStr, "12345") {
		t.Error("Expected user_id value in log file")
	}

	// Restore original state
	if originalLoggingEnabled {
		EnableLogging()
	} else {
		DisableLogging()
	}
	if originalConsoleEnabled {
		EnableConsole()
	} else {
		DisableConsole()
	}
	SetLevel(originalLevel)
}

// TestDefaultValues tests that default values are correct
func TestDefaultValues(t *testing.T) {
	// Re-initialize to test defaults
	// Note: This test assumes init() has already been called

	// Test that logging is enabled by default
	if !IsLoggingEnabled() {
		t.Error("Logging should be enabled by default")
	}

	// Test that console is enabled by default
	if !IsConsoleEnabled() {
		t.Error("Console should be enabled by default")
	}

	// Test that default level is INFO
	if GetLevel() != INFO {
		t.Errorf("Default log level should be INFO, got %v", GetLevel())
	}
}

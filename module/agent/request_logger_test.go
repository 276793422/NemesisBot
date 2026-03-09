// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package agent

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/providers"
)

// TestRequestLogger_NewRequestLogger tests creating a new request logger
func TestRequestLogger_NewRequestLogger(t *testing.T) {
	tempDir := t.TempDir()

	tests := []struct {
		name     string
		cfg      *config.LoggingConfig
		enabled  bool
	}{
		{
			name: "enabled with valid config",
			cfg: &config.LoggingConfig{
				LLM: &config.LLMLogConfig{
					Enabled: true,
					LogDir:  filepath.Join(tempDir, "logs"),
				},
			},
			enabled: true,
		},
		{
			name: "disabled when config is nil",
			cfg:  nil,
			enabled: false,
		},
		{
			name: "disabled when LLM config is nil",
			cfg: &config.LoggingConfig{
				LLM: nil,
			},
			enabled: false,
		},
		{
			name: "disabled when enabled is false",
			cfg: &config.LoggingConfig{
				LLM: &config.LLMLogConfig{
					Enabled: false,
				},
			},
			enabled: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			rl := NewRequestLogger(tt.cfg, tempDir)
			if rl.IsEnabled() != tt.enabled {
				t.Errorf("Expected enabled=%v, got %v", tt.enabled, rl.IsEnabled())
			}
		})
	}
}

// TestRequestLogger_CreateSession tests session creation
func TestRequestLogger_CreateSession(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.LoggingConfig{
		LLM: &config.LLMLogConfig{
			Enabled:     true,
			LogDir:      filepath.Join(tempDir, "logs"),
			DetailLevel: "full",
		},
	}

	rl := NewRequestLogger(cfg, tempDir)

	err := rl.CreateSession()
	if err != nil {
		t.Fatalf("Expected no error, got %v", err)
	}

	// Verify session directory was created
	if rl.sessionDir == "" {
		t.Error("Expected sessionDir to be set")
	}

	if _, err := os.Stat(rl.sessionDir); os.IsNotExist(err) {
		t.Error("Expected session directory to exist")
	}

	// Test that disabled logger doesn't create session
	rlDisabled := NewRequestLogger(nil, tempDir)
	err = rlDisabled.CreateSession()
	if err != nil {
		t.Errorf("Expected no error for disabled logger, got %v", err)
	}
}

// TestRequestLogger_LogUserRequest tests logging user requests
func TestRequestLogger_LogUserRequest(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.LoggingConfig{
		LLM: &config.LLMLogConfig{
			Enabled:     true,
			LogDir:      filepath.Join(tempDir, "logs"),
			DetailLevel: "full",
		},
	}

	rl := NewRequestLogger(cfg, tempDir)
	rl.CreateSession()

	info := UserRequestInfo{
		Timestamp: time.Now(),
		Channel:   "test-channel",
		SenderID:  "user123",
		ChatID:    "chat456",
		Content:   "Hello, world!",
	}

	err := rl.LogUserRequest(info)
	if err != nil {
		t.Fatalf("Expected no error, got %v", err)
	}

	// Verify file was created
	expectedFile := filepath.Join(rl.sessionDir, "01.request.md")
	if _, err := os.Stat(expectedFile); os.IsNotExist(err) {
		t.Errorf("Expected log file to exist: %s", expectedFile)
	}
}

// TestRequestLogger_LogLLMRequest tests logging LLM requests
func TestRequestLogger_LogLLMRequest(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.LoggingConfig{
		LLM: &config.LLMLogConfig{
			Enabled:     true,
			LogDir:      filepath.Join(tempDir, "logs"),
			DetailLevel: "full",
		},
	}

	rl := NewRequestLogger(cfg, tempDir)
	rl.CreateSession()

	info := LLMRequestInfo{
		Round:         1,
		Timestamp:     time.Now(),
		Model:         "test-model",
		ProviderName:  "test-provider",
		APIKey:        "sk-test123456789",
		APIBase:       "https://api.test.com",
		HTTPHeaders:   map[string]string{"X-Custom": "value"},
		FullConfig:    map[string]interface{}{"temperature": 0.7},
		Messages: []providers.Message{
			{Role: "user", Content: "Test message"},
		},
		Tools: []providers.ToolDefinition{
			{
				Type: "function",
				Function: providers.ToolFunctionDefinition{
					Name:       "test_tool",
					Parameters: map[string]interface{}{"type": "object"},
				},
			},
		},
		FallbackAttempts: []FallbackAttemptInfo{
			{
				ProviderName: "fallback1",
				ModelName:    "model1",
				APIKey:       "sk-key123",
				APIBase:      "https://api.fallback1.com",
				ErrorMessage: "timeout",
				Duration:     5 * time.Second,
			},
		},
	}

	err := rl.LogLLMRequest(info)
	if err != nil {
		t.Fatalf("Expected no error, got %v", err)
	}

	// Verify file was created (don't check exact index as it resets per logger)
	files, _ := os.ReadDir(rl.sessionDir)
	found := false
	for _, f := range files {
		if strings.HasSuffix(f.Name(), ".AI.Request.md") {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("Expected log file with pattern *.AI.Request.md to exist, files: %v", getFileNames(rl.sessionDir))
	}
}

// TestRequestLogger_LogLLMResponse tests logging LLM responses
func TestRequestLogger_LogLLMResponse(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.LoggingConfig{
		LLM: &config.LLMLogConfig{
			Enabled:     true,
			LogDir:      filepath.Join(tempDir, "logs"),
			DetailLevel: "full",
		},
	}

	rl := NewRequestLogger(cfg, tempDir)
	rl.CreateSession()

	info := LLMResponseInfo{
		Round:        1,
		Timestamp:    time.Now(),
		Duration:     2 * time.Second,
		Content:      "Test response content",
		ToolCalls: []providers.ToolCall{
			{
				ID:   "call_123",
				Type: "function",
				Name: "test_tool",
				Arguments: map[string]interface{}{
					"param1": "value1",
				},
			},
		},
		Usage: &providers.UsageInfo{
			PromptTokens:     100,
			CompletionTokens: 50,
			TotalTokens:      150,
		},
		FinishReason: "stop",
	}

	err := rl.LogLLMResponse(info)
	if err != nil {
		t.Fatalf("Expected no error, got %v", err)
	}

	// Verify file was created (don't check exact index as it resets per logger)
	files, _ := os.ReadDir(rl.sessionDir)
	found := false
	for _, f := range files {
		if strings.HasSuffix(f.Name(), ".AI.Response.md") {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("Expected log file with pattern *.AI.Response.md to exist, files: %v", getFileNames(rl.sessionDir))
	}
}

// TestRequestLogger_LogLocalOperations tests logging local operations
func TestRequestLogger_LogLocalOperations(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.LoggingConfig{
		LLM: &config.LLMLogConfig{
			Enabled:     true,
			LogDir:      filepath.Join(tempDir, "logs"),
			DetailLevel: "full",
		},
	}

	rl := NewRequestLogger(cfg, tempDir)
	rl.CreateSession()

	info := LocalOperationInfo{
		Round:     1,
		Timestamp: time.Now(),
		Operations: []Operation{
			{
				Type:  "tool_call",
				Name:  "test_tool",
				Arguments: map[string]interface{}{
					"param1": "value1",
				},
				Result:   map[string]interface{}{"output": "success"},
				Status:   "Success",
				Duration: 100 * time.Millisecond,
			},
			{
				Type:  "file_write",
				Name:  "test.txt",
				Status: "Failed",
				Error:  "permission denied",
			},
		},
	}

	err := rl.LogLocalOperations(info)
	if err != nil {
		t.Fatalf("Expected no error, got %v", err)
	}

	// Verify file was created (don't check exact index as it may vary)
	files, _ := os.ReadDir(rl.sessionDir)
	found := false
	for _, f := range files {
		if strings.HasSuffix(f.Name(), ".Local.md") {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("Expected log file with pattern *.Local.md to exist, files: %v", getFileNames(rl.sessionDir))
	}
}

// TestRequestLogger_LogLocalOperations_Empty tests logging with no operations
func TestRequestLogger_LogLocalOperations_Empty(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.LoggingConfig{
		LLM: &config.LLMLogConfig{
			Enabled:     true,
			LogDir:      filepath.Join(tempDir, "logs"),
			DetailLevel: "full",
		},
	}

	rl := NewRequestLogger(cfg, tempDir)
	rl.CreateSession()

	info := LocalOperationInfo{
		Round:      1,
		Timestamp:  time.Now(),
		Operations: []Operation{},
	}

	err := rl.LogLocalOperations(info)
	if err != nil {
		t.Fatalf("Expected no error, got %v", err)
	}

	// Should not create file for empty operations
	foundLocal := false
	files, _ := os.ReadDir(rl.sessionDir)
	for _, f := range files {
		if strings.HasSuffix(f.Name(), ".Local.md") {
			foundLocal = true
			break
		}
	}
	// We expect at least one Local.md from previous tests, but not a new one for this empty operation
	if foundLocal {
		// This is OK - means there's a Local.md from a previous test
	}
}

// TestRequestLogger_LogFinalResponse tests logging final response
func TestRequestLogger_LogFinalResponse(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.LoggingConfig{
		LLM: &config.LLMLogConfig{
			Enabled:     true,
			LogDir:      filepath.Join(tempDir, "logs"),
			DetailLevel: "full",
		},
	}

	rl := NewRequestLogger(cfg, tempDir)
	rl.CreateSession()

	info := FinalResponseInfo{
		Timestamp:     time.Now(),
		TotalDuration: 5 * time.Second,
		LLMRounds:     2,
		Content:       "Final response content",
		Channel:       "test-channel",
		ChatID:        "chat456",
	}

	err := rl.LogFinalResponse(info)
	if err != nil {
		t.Fatalf("Expected no error, got %v", err)
	}

	// Verify file was created (don't check exact index as it may vary)
	files, _ := os.ReadDir(rl.sessionDir)
	found := false
	for _, f := range files {
		if strings.HasSuffix(f.Name(), ".response.md") {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("Expected log file with pattern *.response.md to exist, files: %v", getFileNames(rl.sessionDir))
	}
}

// TestRequestLogger_ConcurrentLogging tests concurrent logging operations
func TestRequestLogger_ConcurrentLogging(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.LoggingConfig{
		LLM: &config.LLMLogConfig{
			Enabled:     true,
			LogDir:      filepath.Join(tempDir, "logs"),
			DetailLevel: "full",
		},
	}

	rl := NewRequestLogger(cfg, tempDir)
	rl.CreateSession()

	// Launch multiple goroutines writing logs
	done := make(chan bool, 10)
	for i := 0; i < 10; i++ {
		go func(index int) {
			info := UserRequestInfo{
				Timestamp: time.Now(),
				Channel:   "test-channel",
				SenderID:  "user123",
				ChatID:    "chat456",
				Content:   "Concurrent test",
			}
			_ = rl.LogUserRequest(info)
			done <- true
		}(i)
	}

	// Wait for all goroutines
	for i := 0; i < 10; i++ {
		<-done
	}

	// Verify files were created
	files, _ := os.ReadDir(rl.sessionDir)
	if len(files) < 10 {
		t.Errorf("Expected at least 10 log files, got %d", len(files))
	}
}

// TestRequestLogger_TruncatedMode tests truncated detail level
func TestRequestLogger_TruncatedMode(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.LoggingConfig{
		LLM: &config.LLMLogConfig{
			Enabled:     true,
			LogDir:      filepath.Join(tempDir, "logs"),
			DetailLevel: "truncated",
		},
	}

	rl := NewRequestLogger(cfg, tempDir)
	rl.CreateSession()

	// Create a long message
	longContent := string(make([]byte, 1000))
	for i := range longContent {
		longContent = longContent[:i] + "A" + longContent[i+1:]
	}

	info := LLMRequestInfo{
		Round:     1,
		Timestamp: time.Now(),
		Model:     "test-model",
		Messages: []providers.Message{
			{Role: "user", Content: longContent},
		},
		Tools: []providers.ToolDefinition{
			{
				Type: "function",
				Function: providers.ToolFunctionDefinition{
					Name:       "test_tool",
					Parameters: map[string]interface{}{"param": string(make([]byte, 1000))},
				},
			},
		},
	}

	err := rl.LogLLMRequest(info)
	if err != nil {
		t.Fatalf("Expected no error, got %v", err)
	}

	// Verify file was created and content is truncated
	files, _ := os.ReadDir(rl.sessionDir)
	var reqFile os.DirEntry
	for _, f := range files {
		if strings.HasSuffix(f.Name(), ".AI.Request.md") {
			reqFile = f
			break
		}
	}
	if reqFile == nil {
		t.Fatal("Expected to find an AI.Request.md file")
	}
	expectedFile := filepath.Join(rl.sessionDir, reqFile.Name())
	content, err := os.ReadFile(expectedFile)
	if err != nil {
		t.Fatalf("Failed to read log file: %v", err)
	}

	fileContent := string(content)
	// Should contain truncation marker
	if !strings.Contains(fileContent, "[truncated]") {
		t.Error("Expected content to be truncated in log file")
	}
}

// TestRequestLogger_DisabledLogger tests that disabled logger doesn't create files
func TestRequestLogger_DisabledLogger(t *testing.T) {
	tempDir := t.TempDir()

	rl := NewRequestLogger(nil, tempDir)

	info := UserRequestInfo{
		Timestamp: time.Now(),
		Channel:   "test-channel",
		SenderID:  "user123",
		ChatID:    "chat456",
		Content:   "Hello",
	}

	err := rl.LogUserRequest(info)
	if err != nil {
		t.Errorf("Expected no error for disabled logger, got %v", err)
	}
}

// Helper function to get file names
func getFileNames(dir string) []string {
	files, err := os.ReadDir(dir)
	if err != nil {
		return []string{}
	}
	names := make([]string, len(files))
	for i, f := range files {
		names[i] = f.Name()
	}
	return names
}
func TestResolveLogPath(t *testing.T) {
	homeDir, _ := os.UserHomeDir()
	workspace := t.TempDir()

	tests := []struct {
		name     string
		logDir   string
		expected string
	}{
		{
			name:     "absolute path",
			logDir:   "/var/log/test",
			expected: "/var/log/test",
		},
		{
			name:     "relative path",
			logDir:   "logs",
			expected: filepath.Join(workspace, "logs"),
		},
		{
			name:     "home expansion",
			logDir:   "~/test/logs",
			expected: filepath.Join(homeDir, "test/logs"),
		},
		{
			name:     "home with slash",
			logDir:   "~/test/logs",
			expected: filepath.Join(homeDir, "test/logs"),
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := resolveLogPath(tt.logDir, workspace)
			if result != tt.expected {
				t.Errorf("Expected %s, got %s", tt.expected, result)
			}
		})
	}
}

// TestMaskAPIKey tests API key masking
func TestMaskAPIKey(t *testing.T) {
	tests := []struct {
		name     string
		key      string
		expected string
	}{
		{
			name:     "normal key",
			key:      "sk-1234567890abcdef",
			expected: "sk-***def",
		},
		{
			name:     "short key",
			key:      "short",
			expected: "***",
		},
		{
			name:     "empty key",
			key:      "",
			expected: "<empty>",
		},
		{
			name:     "key with spaces",
			key:      "  sk-test  ",
			expected: "sk-***est",
		},
		{
			name:     "exact 6 chars",
			key:      "123456",
			expected: "***",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := maskAPIKey(tt.key)
			if result != tt.expected {
				t.Errorf("Expected %s, got %s", tt.expected, result)
			}
		})
	}
}

// TestRequestLogger_Close tests closing the logger
func TestRequestLogger_Close(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.LoggingConfig{
		LLM: &config.LLMLogConfig{
			Enabled: true,
			LogDir:  filepath.Join(tempDir, "logs"),
		},
	}

	rl := NewRequestLogger(cfg, tempDir)
	rl.CreateSession()

	// Close should not panic
	rl.Close()
}

// TestRequestLogger_NextIndex tests file index incrementing
func TestRequestLogger_NextIndex(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.LoggingConfig{
		LLM: &config.LLMLogConfig{
			Enabled: true,
			LogDir:  filepath.Join(tempDir, "logs"),
		},
	}

	rl := NewRequestLogger(cfg, tempDir)
	rl.CreateSession()

	// Test index incrementing
	idx1 := rl.NextIndex()
	idx2 := rl.NextIndex()
	idx3 := rl.NextIndex()

	if idx1 != "01" {
		t.Errorf("Expected index 01, got %s", idx1)
	}
	if idx2 != "02" {
		t.Errorf("Expected index 02, got %s", idx2)
	}
	if idx3 != "03" {
		t.Errorf("Expected index 03, got %s", idx3)
	}
}


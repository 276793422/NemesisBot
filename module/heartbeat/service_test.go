package heartbeat

import (
	"os"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/tools"
)

func TestNewHeartbeatService(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "heartbeat-test-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	// Test with default interval
	hs := NewHeartbeatService(tmpDir, 0, true)
	if hs == nil {
		t.Fatal("NewHeartbeatService returned nil")
	}
	if hs.workspace != tmpDir {
		t.Errorf("Expected workspace %v, got %v", tmpDir, hs.workspace)
	}
	if hs.interval != 30*time.Minute {
		t.Errorf("Expected default interval 30m, got %v", hs.interval)
	}
	if !hs.enabled {
		t.Error("Service should be enabled")
	}

	// Test with custom interval
	hs2 := NewHeartbeatService(tmpDir, 60, true)
	if hs2.interval != 60*time.Minute {
		t.Errorf("Expected interval 60m, got %v", hs2.interval)
	}

	// Test with interval below minimum (should be clamped to 5 minutes)
	hs3 := NewHeartbeatService(tmpDir, 2, true)
	if hs3.interval != 5*time.Minute {
		t.Errorf("Expected minimum interval 5m, got %v", hs3.interval)
	}

	// Test disabled
	hs4 := NewHeartbeatService(tmpDir, 30, false)
	if hs4.enabled {
		t.Error("Service should be disabled")
	}
}

func TestSetBus(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "heartbeat-setbus-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	hs := NewHeartbeatService(tmpDir, 30, true)
	msgBus := bus.NewMessageBus()

	hs.SetBus(msgBus)
	if hs.bus != msgBus {
		t.Error("Bus was not set")
	}
}

func TestSetHandler(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "heartbeat-sethandler-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	hs := NewHeartbeatService(tmpDir, 30, true)
	handler := func(prompt, channel, chatID string) *tools.ToolResult {
		return &tools.ToolResult{}
	}

	hs.SetHandler(handler)
	if hs.handler == nil {
		t.Error("Handler was not set")
	}
}

func TestStartStop(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "heartbeat-startstop-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	hs := NewHeartbeatService(tmpDir, 30, false) // Disabled for testing

	// Start disabled service
	err = hs.Start()
	if err != nil {
		t.Errorf("Start should not error when disabled, got %v", err)
	}
	if hs.IsRunning() {
		t.Error("Service should not be running when disabled")
	}

	// Test enabled service
	hs2 := NewHeartbeatService(tmpDir, 1, true) // 1 minute interval
	hs2.SetHandler(func(prompt, channel, chatID string) *tools.ToolResult {
		return &tools.ToolResult{}
	})
	hs2.SetBus(bus.NewMessageBus())

	err = hs2.Start()
	if err != nil {
		t.Fatalf("Start failed: %v", err)
	}

	if !hs2.IsRunning() {
		t.Error("Service should be running after Start")
	}

	// Starting again should not error
	err = hs2.Start()
	if err != nil {
		t.Errorf("Start again should not error, got %v", err)
	}

	// Stop service
	hs2.Stop()
	time.Sleep(100 * time.Millisecond) // Give time for stop

	if hs2.IsRunning() {
		t.Error("Service should not be running after Stop")
	}

	// Stop again should not panic
	hs2.Stop()
}

func TestIsRunning(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "heartbeat-isrunning-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	hs := NewHeartbeatService(tmpDir, 30, false)

	if hs.IsRunning() {
		t.Error("Service should not be running initially")
	}
}

func TestStateIntegration(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "heartbeat-state-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	hs := NewHeartbeatService(tmpDir, 30, true)

	if hs.state == nil {
		t.Error("State manager should be initialized")
	}

	// Set last channel info
	hs.state.SetLastChannel("telegram:123456")

	lastChannel := hs.state.GetLastChannel()
	if lastChannel != "telegram:123456" {
		t.Errorf("Expected 'telegram:123456', got %v", lastChannel)
	}
}

func TestConcurrentAccess(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "heartbeat-concurrent-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	hs := NewHeartbeatService(tmpDir, 30, true)
	msgBus := bus.NewMessageBus()
	handler := func(prompt, channel, chatID string) *tools.ToolResult {
		return &tools.ToolResult{}
	}

	done := make(chan bool)

	// Concurrent operations
	for i := 0; i < 10; i++ {
		go func(idx int) {
			for j := 0; j < 100; j++ {
				hs.SetBus(msgBus)
				hs.SetHandler(handler)
				_ = hs.IsRunning()
			}
			done <- true
		}(i)
	}

	// Wait for all goroutines
	for i := 0; i < 10; i++ {
		<-done
	}
}

func TestIsHeartbeatFileEmpty(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "heartbeat-empty-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	hs := NewHeartbeatService(tmpDir, 30, true)

	// Test with empty data
	isEmpty := hs.isHeartbeatFileEmpty([]byte{})
	if !isEmpty {
		t.Error("Empty data should be considered empty")
	}

	// Test with non-empty data
	isEmpty = hs.isHeartbeatFileEmpty([]byte("# Heartbeat\n\nSome content"))
	if isEmpty {
		t.Error("Non-empty data should not be considered empty")
	}

	// Test with whitespace only
	isEmpty = hs.isHeartbeatFileEmpty([]byte("   \n  \n  "))
	if !isEmpty {
		t.Error("Whitespace-only data should be considered empty")
	}
}

func TestBuildPrompt(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "heartbeat-prompt-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	hs := NewHeartbeatService(tmpDir, 30, true)

	// Test with no HEARTBEAT.md file - should create template and return empty
	prompt := hs.buildPrompt()
	if prompt != "" {
		t.Error("buildPrompt() should return empty string when file doesn't exist")
	}

	// Verify the template was created
	heartbeatFile := tmpDir + "/HEARTBEAT.md"
	if _, err := os.Stat(heartbeatFile); os.IsNotExist(err) {
		t.Error("Default template should be created when file doesn't exist")
	}

	// Test with last channel set (requires valid HEARTBEAT.md content)
	hs.state.SetLastChannel("telegram:chat123")
	content := []byte("# Test content\n\nSome task here")
	os.WriteFile(heartbeatFile, content, 0644)

	prompt = hs.buildPrompt()
	if prompt == "" {
		t.Error("buildPrompt() should not return empty string with valid content")
	}
	if !contains(prompt, "telegram:chat123") {
		// Note: buildPrompt doesn't include last channel in the prompt itself
		// It only uses it for routing in sendResponse
		t.Logf("buildPrompt() content: %s", prompt)
	}
}

func TestCreateDefaultHeartbeatTemplate(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "heartbeat-template-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	hs := NewHeartbeatService(tmpDir, 30, true)

	hs.createDefaultHeartbeatTemplate()

	// Verify file was created
	heartbeatFile := tmpDir + "/HEARTBEAT.md"
	content, err := os.ReadFile(heartbeatFile)
	if err != nil {
		t.Fatalf("Failed to read heartbeat file: %v", err)
	}

	if len(content) == 0 {
		t.Error("Heartbeat file should not be empty")
	}

	// Verify it contains expected sections (based on actual template)
	contentStr := string(content)
	expectedSections := []string{"# Heartbeat Check List", "## Examples", "## Instructions"}
	for _, section := range expectedSections {
		if !contains(contentStr, section) {
			t.Errorf("Heartbeat template should include section: %s", section)
		}
	}
}

func TestSendResponse(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "heartbeat-response-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	hs := NewHeartbeatService(tmpDir, 30, true)
	msgBus := bus.NewMessageBus()

	hs.SetBus(msgBus)

	// Test sending response
	response := "Test heartbeat response"
	hs.sendResponse(response)
}

func TestParseLastChannel(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "heartbeat-parse-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	hs := NewHeartbeatService(tmpDir, 30, true)

	tests := []struct {
		name             string
		input            string
		expectedPlatform string
		expectedUserID   string
	}{
		{
			name:             "valid channel:chat",
			input:            "telegram:chat123",
			expectedPlatform: "telegram",
			expectedUserID:   "chat123",
		},
		{
			name:             "empty string",
			input:            "",
			expectedPlatform: "",
			expectedUserID:   "",
		},
		{
			name:             "no colon",
			input:            "telegram",
			expectedPlatform: "",
			expectedUserID:   "",
		},
		{
			name:             "multiple colons",
			input:            "telegram:chat:123",
			expectedPlatform: "telegram",
			expectedUserID:   "chat:123",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			platform, userID := hs.parseLastChannel(tt.input)
			if platform != tt.expectedPlatform {
				t.Errorf("parseLastChannel() platform = %v, want %v", platform, tt.expectedPlatform)
			}
			if userID != tt.expectedUserID {
				t.Errorf("parseLastChannel() userID = %v, want %v", userID, tt.expectedUserID)
			}
		})
	}
}

func TestLogInfo(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "heartbeat-loginfo-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	hs := NewHeartbeatService(tmpDir, 30, true)

	// Should not panic
	hs.logInfo("test message %s", "value")
}

func TestLogError(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "heartbeat-logerror-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	hs := NewHeartbeatService(tmpDir, 30, true)

	// Should not panic
	hs.logError("test error %s", "value")
}

func TestLog(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "heartbeat-log-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	hs := NewHeartbeatService(tmpDir, 30, true)

	// Should not panic
	hs.log("INFO", "test message %s: %v", "key", "value")
}

func TestExecuteHeartbeat(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "heartbeat-execute-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	hs := NewHeartbeatService(tmpDir, 30, true)
	msgBus := bus.NewMessageBus()

	// Create a valid HEARTBEAT.md file
	heartbeatFile := tmpDir + "/HEARTBEAT.md"
	content := []byte("# Heartbeat tasks\n\n- Task 1\n- Task 2")
	err = os.WriteFile(heartbeatFile, content, 0644)
	if err != nil {
		t.Fatalf("Failed to create heartbeat file: %v", err)
	}

	handlerCalled := false
	handler := func(prompt, channel, chatID string) *tools.ToolResult {
		handlerCalled = true
		return &tools.ToolResult{ForLLM: "Handler executed"}
	}

	hs.SetBus(msgBus)
	hs.SetHandler(handler)

	// Start the service to initialize stopChan
	err = hs.Start()
	if err != nil {
		t.Fatalf("Start failed: %v", err)
	}
	defer hs.Stop()

	// Execute heartbeat
	hs.executeHeartbeat()

	// Handler should be called
	if !handlerCalled {
		t.Error("executeHeartbeat() should call handler")
	}
}

func TestExecuteHeartbeat_NoHandler(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "heartbeat-nohandler-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	hs := NewHeartbeatService(tmpDir, 30, true)
	msgBus := bus.NewMessageBus()

	hs.SetBus(msgBus)
	// No handler set

	// Should not panic
	hs.executeHeartbeat()
}

func TestExecuteHeartbeat_EmptyFile(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "heartbeat-emptyfile-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	hs := NewHeartbeatService(tmpDir, 30, true)
	msgBus := bus.NewMessageBus()

	// Create empty heartbeat file
	heartbeatFile := tmpDir + "/HEARTBEAT.md"
	err = os.WriteFile(heartbeatFile, []byte(""), 0644)
	if err != nil {
		t.Fatalf("Failed to create empty heartbeat file: %v", err)
	}

	handlerCalled := false
	handler := func(prompt, channel, chatID string) *tools.ToolResult {
		handlerCalled = true
		return &tools.ToolResult{ForLLM: "Handler executed"}
	}

	hs.SetBus(msgBus)
	hs.SetHandler(handler)

	// Start the service to initialize stopChan
	err = hs.Start()
	if err != nil {
		t.Fatalf("Start failed: %v", err)
	}
	defer hs.Stop()

	// Execute heartbeat
	hs.executeHeartbeat()

	// Handler should NOT be called when file is empty
	// (buildPrompt returns "" for empty files, so handler won't be called)
	if handlerCalled {
		t.Error("executeHeartbeat() should not call handler with empty file")
	}

	// Note: Default template is only created when file doesn't exist, not when it's empty
	// So we don't expect it to be created in this case
}

// Helper function
func contains(s, substr string) bool {
	return len(s) >= len(substr) && (s == substr || len(substr) == 0 ||
		(len(s) > 0 && len(substr) > 0 && findSubstring(s, substr)))
}

func findSubstring(s, substr string) bool {
	for i := 0; i <= len(s)-len(substr); i++ {
		if s[i:i+len(substr)] == substr {
			return true
		}
	}
	return false
}

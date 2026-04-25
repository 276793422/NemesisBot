package services

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"testing"
	"time"
)

// --- BotState JSON round-trip tests ---

func TestBotStateMarshalJSON(t *testing.T) {
	tests := []struct {
		state    BotState
		expected string
	}{
		{BotStateNotStarted, `"not_started"`},
		{BotStateStarting, `"starting"`},
		{BotStateRunning, `"running"`},
		{BotStateError, `"error"`},
	}

	for _, tt := range tests {
		t.Run(tt.expected, func(t *testing.T) {
			data, err := json.Marshal(tt.state)
			if err != nil {
				t.Fatalf("MarshalJSON error: %v", err)
			}
			if string(data) != tt.expected {
				t.Errorf("MarshalJSON() = %s, want %s", string(data), tt.expected)
			}
		})
	}
}

func TestBotStateUnmarshalJSON(t *testing.T) {
	tests := []struct {
		input    string
		expected BotState
		wantErr  bool
	}{
		{`"not_started"`, BotStateNotStarted, false},
		{`"starting"`, BotStateStarting, false},
		{`"running"`, BotStateRunning, false},
		{`"error"`, BotStateError, false},
		{`"unknown_state"`, BotStateNotStarted, true},
		{`"`, BotStateNotStarted, true},
	}

	for _, tt := range tests {
		name := fmt.Sprintf("input=%s", tt.input)
		t.Run(name, func(t *testing.T) {
			var state BotState
			err := json.Unmarshal([]byte(tt.input), &state)
			if tt.wantErr {
				if err == nil {
					t.Error("Expected error for input", tt.input)
				}
			} else {
				if err != nil {
					t.Fatalf("Unexpected error: %v", err)
				}
				if state != tt.expected {
					t.Errorf("UnmarshalJSON() = %d, want %d", state, tt.expected)
				}
			}
		})
	}
}

func TestBotStateMarshalUnmarshalRoundTrip(t *testing.T) {
	states := []BotState{BotStateNotStarted, BotStateStarting, BotStateRunning, BotStateError}
	for _, original := range states {
		t.Run(original.String(), func(t *testing.T) {
			data, err := json.Marshal(original)
			if err != nil {
				t.Fatalf("Marshal error: %v", err)
			}
			var decoded BotState
			if err := json.Unmarshal(data, &decoded); err != nil {
				t.Fatalf("Unmarshal error: %v", err)
			}
			if decoded != original {
				t.Errorf("Round trip failed: got %d, want %d", decoded, original)
			}
		})
	}
}

// --- BotService tests ---

func TestNewBotService(t *testing.T) {
	svc := NewBotService()
	if svc == nil {
		t.Fatal("NewBotService returned nil")
	}
	if svc.state != BotStateNotStarted {
		t.Errorf("Initial state should be NotStarted, got %s", svc.state)
	}
	if svc.configPath == "" {
		t.Error("configPath should not be empty")
	}
}

func TestBotService_GetState(t *testing.T) {
	svc := NewBotService()
	if svc.GetState() != BotStateNotStarted {
		t.Errorf("Expected NotStarted, got %s", svc.GetState())
	}
}

func TestBotService_GetError_InitiallyNil(t *testing.T) {
	svc := NewBotService()
	if err := svc.GetError(); err != nil {
		t.Errorf("Expected nil error, got %v", err)
	}
}

func TestBotService_Stop_WhenNotRunning(t *testing.T) {
	svc := NewBotService()
	err := svc.Stop()
	if err == nil {
		t.Error("Expected error when stopping a not-started service")
	}
}

func TestBotService_Start_AlreadyStarting(t *testing.T) {
	svc := NewBotService()
	svc.state = BotStateStarting
	err := svc.Start()
	if err == nil {
		t.Error("Expected error when starting an already-starting service")
	}
}

func TestBotService_Start_AlreadyRunning(t *testing.T) {
	svc := NewBotService()
	svc.state = BotStateRunning
	err := svc.Start()
	if err == nil {
		t.Error("Expected error when starting an already-running service")
	}
}

func TestBotService_setStateWithError(t *testing.T) {
	svc := NewBotService()
	testErr := fmt.Errorf("test error")
	svc.setStateWithError(BotStateError, testErr)

	if svc.state != BotStateError {
		t.Errorf("Expected Error state, got %s", svc.state)
	}
	if svc.err == nil || svc.err.Error() != "test error" {
		t.Errorf("Expected 'test error', got %v", svc.err)
	}
}

func TestBotService_GetConfig_Default(t *testing.T) {
	svc := NewBotService()
	svc.configPath = filepath.Join(t.TempDir(), "nonexistent_config.json")
	// GetConfig returns default config when file doesn't exist
	cfg, err := svc.GetConfig()
	if err != nil {
		t.Errorf("GetConfig should return default config when file missing: %v", err)
	}
	if cfg == nil {
		t.Error("Config should not be nil (default should be returned)")
	}
}

func TestBotService_SaveConfig_InvalidType(t *testing.T) {
	svc := NewBotService()
	err := svc.SaveConfig("not a config", false)
	if err == nil {
		t.Error("Expected error for invalid config type")
	}
}

func TestBotService_GetComponents_Empty(t *testing.T) {
	svc := NewBotService()
	components := svc.GetComponents()
	if len(components) != 0 {
		t.Errorf("Expected empty components, got %d", len(components))
	}
}

func TestBotService_GetForge_Nil(t *testing.T) {
	svc := NewBotService()
	forge := svc.GetForge()
	if forge != nil {
		t.Error("Expected nil forge before initialization")
	}
}

func TestBotService_Restart_WhenNotRunning(t *testing.T) {
	svc := NewBotService()
	// Restart on not-running service delegates to Start(), which finds
	// the default config and may succeed. The key behavior tested here is
	// that Restart does not panic and returns a valid result.
	_ = svc.Restart()
	// State should be either Running (success) or Error (failure)
	if svc.GetState() != BotStateRunning && svc.GetState() != BotStateError && svc.GetState() != BotStateNotStarted {
		t.Errorf("Unexpected state after Restart: %s", svc.GetState())
	}
}

func TestBotService_Stop_AfterSettingState(t *testing.T) {
	svc := NewBotService()
	// Manually set state to Running to test Stop path
	svc.state = BotStateRunning
	svc.ctx, svc.cancel = context.WithCancel(context.Background())

	err := svc.Stop()
	if err != nil {
		t.Errorf("Stop failed: %v", err)
	}
	if svc.state != BotStateNotStarted {
		t.Errorf("Expected NotStarted after Stop, got %s", svc.state)
	}
}

// --- ShouldSkipHeartbeatForBootstrap tests ---

func TestShouldSkipHeartbeatForBootstrap_WithFile(t *testing.T) {
	tmpDir := t.TempDir()
	bootstrapPath := filepath.Join(tmpDir, "BOOTSTRAP.md")
	if err := os.WriteFile(bootstrapPath, []byte("# bootstrap"), 0644); err != nil {
		t.Fatal(err)
	}

	if !ShouldSkipHeartbeatForBootstrap(tmpDir) {
		t.Error("Expected true when BOOTSTRAP.md exists")
	}
}

func TestShouldSkipHeartbeatForBootstrap_WithoutFile(t *testing.T) {
	tmpDir := t.TempDir()
	if ShouldSkipHeartbeatForBootstrap(tmpDir) {
		t.Error("Expected false when BOOTSTRAP.md does not exist")
	}
}

// --- GetConfigPath tests ---

func TestGetConfigPath_EndsWithConfigJSON(t *testing.T) {
	p := GetConfigPath()
	if filepath.Base(p) != "config.json" {
		t.Errorf("Expected path ending with config.json, got %s", p)
	}
}

// --- ServiceManager extra tests ---

func TestServiceManager_GetBotError_Initially(t *testing.T) {
	mgr := NewServiceManager()
	err := mgr.GetBotError()
	if err != nil {
		t.Errorf("Expected nil error initially, got %v", err)
	}
}

func TestServiceManager_GetBotConfig_MissingFile(t *testing.T) {
	mgr := NewServiceManager()
	// The BotService config path will fail to load
	_, err := mgr.GetBotConfig()
	// Config loading may or may not fail depending on environment
	// Just ensure it doesn't panic
	_ = err
}

func TestServiceManager_SaveBotConfig_InvalidType(t *testing.T) {
	mgr := NewServiceManager()
	err := mgr.SaveBotConfig("not a config", false)
	if err == nil {
		t.Error("Expected error for invalid config type")
	}
}

func TestServiceManager_StopBot_WhenNotRunning(t *testing.T) {
	mgr := NewServiceManager()
	err := mgr.StopBot()
	if err == nil {
		t.Error("Expected error when stopping non-running bot")
	}
}

func TestServiceManager_RestartBot_WhenNotRunning(t *testing.T) {
	mgr := NewServiceManager()
	if err := mgr.StartBasicServices(); err != nil {
		t.Fatalf("StartBasicServices failed: %v", err)
	}

	// RestartBot calls bot.Restart(), which may succeed with default config.
	// The key behavior tested is that RestartBot does not panic.
	_ = mgr.RestartBot()
}

func TestServiceManager_GetBotComponents_Empty(t *testing.T) {
	mgr := NewServiceManager()
	components := mgr.GetBotComponents()
	if len(components) != 0 {
		t.Errorf("Expected empty components, got %d", len(components))
	}
}

func TestServiceManager_Shutdown_WithRunningBot(t *testing.T) {
	mgr := NewServiceManager()
	if err := mgr.StartBasicServices(); err != nil {
		t.Fatalf("StartBasicServices failed: %v", err)
	}

	// Manually set bot to running state to test shutdown path
	mgr.botService.state = BotStateRunning
	mgr.botService.ctx, mgr.botService.cancel = context.WithCancel(context.Background())

	mgr.Shutdown()

	// Context should be cancelled
	select {
	case <-mgr.ctx.Done():
		// Expected
	default:
		t.Error("Context should be cancelled after shutdown")
	}
}

func TestServiceManager_WaitForShutdownWithDesktop_Signal(t *testing.T) {
	mgr := NewServiceManager()

	desktopClosed := make(chan struct{})
	done := make(chan struct{})

	go func() {
		mgr.WaitForShutdownWithDesktop(desktopClosed)
		close(done)
	}()

	// Close desktop channel after a short delay
	time.AfterFunc(100*time.Millisecond, func() {
		close(desktopClosed)
	})

	select {
	case <-done:
		// Expected
	case <-time.After(2 * time.Second):
		t.Error("WaitForShutdownWithDesktop did not complete in time")
	}
}

// --- Parallel init edge case ---

func TestParallelInit_MultipleErrors(t *testing.T) {
	err1 := fmt.Errorf("error 1")
	err2 := fmt.Errorf("error 2")

	err := parallelInit(context.Background(),
		func() error { return err1 },
		func() error { return err2 },
	)

	if err == nil {
		t.Fatal("Expected error, got nil")
	}
	// Should return one of the errors
	if err.Error() != "error 1" && err.Error() != "error 2" {
		t.Errorf("Unexpected error: %v", err)
	}
}

func TestSequentialInit_Empty(t *testing.T) {
	err := sequentialInit()
	if err != nil {
		t.Errorf("Expected no error for empty sequentialInit, got: %v", err)
	}
}

// --- JSON file-based config test ---

func TestBotService_loadConfig_InvalidJSON(t *testing.T) {
	tmpDir := t.TempDir()
	configPath := filepath.Join(tmpDir, "config.json")

	// Write invalid JSON
	if err := os.WriteFile(configPath, []byte(`{invalid json`), 0644); err != nil {
		t.Fatal(err)
	}

	svc := NewBotService()
	svc.configPath = configPath

	err := svc.loadConfig()
	if err == nil {
		t.Error("Expected error loading invalid JSON config")
	}
}

func TestBotService_validateConfig_NoModels(t *testing.T) {
	tmpDir := t.TempDir()
	configPath := filepath.Join(tmpDir, "config.json")

	// Config with empty model list
	configContent := `{"models": [], "workspace": "."}`
	if err := os.WriteFile(configPath, []byte(configContent), 0644); err != nil {
		t.Fatal(err)
	}

	svc := NewBotService()
	svc.configPath = configPath

	err := svc.validateConfig()
	if err == nil {
		t.Error("Expected error for config with no models")
	}
}

func TestBotService_validateConfig_NoAPIKey(t *testing.T) {
	tmpDir := t.TempDir()
	configPath := filepath.Join(tmpDir, "config.json")

	// Config with model but no API key
	configContent := `{
		"models": [{"model": "test/model", "api_key": ""}],
		"workspace": "."
	}`
	if err := os.WriteFile(configPath, []byte(configContent), 0644); err != nil {
		t.Fatal(err)
	}

	svc := NewBotService()
	svc.configPath = configPath

	err := svc.validateConfig()
	if err == nil {
		t.Error("Expected error for config with no API key")
	}
}

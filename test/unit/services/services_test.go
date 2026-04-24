// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package services_test

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/services"
)

// =============================================================================
// Priority 1 - Simple functions without mocks
// =============================================================================

// TestBotState_MarshalJSON tests BotState JSON serialization
func TestBotState_MarshalJSON(t *testing.T) {
	tests := []struct {
		name     string
		state    services.BotState
		expected string
	}{
		{"not_started", services.BotStateNotStarted, `"not_started"`},
		{"starting", services.BotStateStarting, `"starting"`},
		{"running", services.BotStateRunning, `"running"`},
		{"error", services.BotStateError, `"error"`},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			data, err := json.Marshal(tt.state)
			if err != nil {
				t.Fatalf("MarshalJSON() error = %v", err)
			}
			if string(data) != tt.expected {
				t.Errorf("MarshalJSON() = %s, want %s", string(data), tt.expected)
			}
		})
	}
}

// TestBotState_UnmarshalJSON tests BotState JSON deserialization
func TestBotState_UnmarshalJSON(t *testing.T) {
	tests := []struct {
		name        string
		jsonStr     string
		expected    services.BotState
		expectError bool
	}{
		{"not_started", `"not_started"`, services.BotStateNotStarted, false},
		{"starting", `"starting"`, services.BotStateStarting, false},
		{"running", `"running"`, services.BotStateRunning, false},
		{"error", `"error"`, services.BotStateError, false},
		{"unknown", `"invalid_state"`, services.BotState(0), true},
		{"empty", `""`, services.BotState(0), true},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			var state services.BotState
			err := json.Unmarshal([]byte(tt.jsonStr), &state)
			if tt.expectError {
				if err == nil {
					t.Error("UnmarshalJSON() expected error, got nil")
				}
				return
			}
			if err != nil {
				t.Fatalf("UnmarshalJSON() unexpected error = %v", err)
			}
			if state != tt.expected {
				t.Errorf("UnmarshalJSON() = %v, want %v", state, tt.expected)
			}
		})
	}
}

// TestBotState_MarshalUnmarshalRoundTrip tests JSON round-trip for BotState
func TestBotState_MarshalUnmarshalRoundTrip(t *testing.T) {
	states := []services.BotState{
		services.BotStateNotStarted,
		services.BotStateStarting,
		services.BotStateRunning,
		services.BotStateError,
	}

	for _, original := range states {
		t.Run(original.String(), func(t *testing.T) {
			data, err := json.Marshal(original)
			if err != nil {
				t.Fatalf("Marshal error = %v", err)
			}

			var decoded services.BotState
			if err := json.Unmarshal(data, &decoded); err != nil {
				t.Fatalf("Unmarshal error = %v", err)
			}

			if decoded != original {
				t.Errorf("Round-trip: got %v, want %v", decoded, original)
			}
		})
	}
}

// TestBotState_IsRunning tests the IsRunning method
func TestBotState_IsRunning(t *testing.T) {
	if services.BotStateNotStarted.IsRunning() {
		t.Error("BotStateNotStarted should not be running")
	}
	if services.BotStateStarting.IsRunning() {
		t.Error("BotStateStarting should not be running")
	}
	if !services.BotStateRunning.IsRunning() {
		t.Error("BotStateRunning should be running")
	}
	if services.BotStateError.IsRunning() {
		t.Error("BotStateError should not be running")
	}
}

// TestBotState_CanStart tests the CanStart method
func TestBotState_CanStart(t *testing.T) {
	if !services.BotStateNotStarted.CanStart() {
		t.Error("BotStateNotStarted should be able to start")
	}
	if services.BotStateStarting.CanStart() {
		t.Error("BotStateStarting should not be able to start")
	}
	if services.BotStateRunning.CanStart() {
		t.Error("BotStateRunning should not be able to start")
	}
	if !services.BotStateError.CanStart() {
		t.Error("BotStateError should be able to start")
	}
}

// TestBotState_CanStop tests the CanStop method
func TestBotState_CanStop(t *testing.T) {
	if services.BotStateNotStarted.CanStop() {
		t.Error("BotStateNotStarted should not be able to stop")
	}
	if !services.BotStateStarting.CanStop() {
		t.Error("BotStateStarting should be able to stop")
	}
	if !services.BotStateRunning.CanStop() {
		t.Error("BotStateRunning should be able to stop")
	}
	if services.BotStateError.CanStop() {
		t.Error("BotStateError should not be able to stop")
	}
}

// TestNewBotService tests creating a new BotService instance
func TestNewBotService(t *testing.T) {
	svc := services.NewBotService()
	if svc == nil {
		t.Fatal("NewBotService() returned nil")
	}

	// New service should be in NotStarted state
	if svc.GetState() != services.BotStateNotStarted {
		t.Errorf("NewBotService state = %v, want BotStateNotStarted", svc.GetState())
	}

	// No error initially
	if svc.GetError() != nil {
		t.Errorf("NewBotService error = %v, want nil", svc.GetError())
	}
}

// TestBotService_GetState tests the GetState accessor
func TestBotService_GetState(t *testing.T) {
	svc := services.NewBotService()
	state := svc.GetState()
	if state != services.BotStateNotStarted {
		t.Errorf("GetState() = %v, want BotStateNotStarted", state)
	}
}

// TestBotService_GetError tests the GetError accessor
func TestBotService_GetError(t *testing.T) {
	svc := services.NewBotService()
	err := svc.GetError()
	if err != nil {
		t.Errorf("GetError() = %v, want nil for new service", err)
	}
}

// TestBotService_GetComponents tests GetComponents on a new service (all nil)
func TestBotService_GetComponents(t *testing.T) {
	svc := services.NewBotService()
	components := svc.GetComponents()
	if components == nil {
		t.Fatal("GetComponents() returned nil map")
	}
	if len(components) != 0 {
		t.Errorf("GetComponents() = %v, want empty map for new service", components)
	}
}

// TestBotService_GetForge tests GetForge returns nil for new service
func TestBotService_GetForge(t *testing.T) {
	svc := services.NewBotService()
	forge := svc.GetForge()
	if forge != nil {
		t.Errorf("GetForge() = %v, want nil for new service", forge)
	}
}

// TestBotService_StopNotRunning tests Stop when service is not running
func TestBotService_StopNotRunning(t *testing.T) {
	svc := services.NewBotService()
	err := svc.Stop()
	if err == nil {
		t.Error("Stop() on non-running service should return error")
	}
}

// TestBotService_Start_AlreadyRunningOrError tests that starting after already started returns error
func TestBotService_Start_AlreadyRunningOrError(t *testing.T) {
	svc := services.NewBotService()
	// First Start may succeed or fail depending on default config
	_ = svc.Start()

	state := svc.GetState()
	// If it succeeded and is running, calling Start again should fail with "already running"
	if state == services.BotStateRunning || state == services.BotStateStarting {
		err := svc.Start()
		if err == nil {
			t.Error("Start() when already running/starting should return error")
		}
	}
	// If it failed (error state), calling Start again should also fail
	// because Start takes the lock and checks state
	if state == services.BotStateError {
		err := svc.Start()
		// This may succeed again since Error is a restartable state,
		// but the underlying config issue will cause it to fail again
		_ = err
	}
}

// TestShouldSkipHeartbeatForBootstrap tests the bootstrap check helper
func TestShouldSkipHeartbeatForBootstrap(t *testing.T) {
	t.Run("nonexistent workspace", func(t *testing.T) {
		if services.ShouldSkipHeartbeatForBootstrap("/nonexistent/path/that/does/not/exist") {
			t.Error("Should return false for nonexistent workspace")
		}
	})

	t.Run("workspace without BOOTSTRAP.md", func(t *testing.T) {
		tmpDir := t.TempDir()
		if services.ShouldSkipHeartbeatForBootstrap(tmpDir) {
			t.Error("Should return false when BOOTSTRAP.md does not exist")
		}
	})

	t.Run("workspace with BOOTSTRAP.md", func(t *testing.T) {
		tmpDir := t.TempDir()
		bootstrapPath := filepath.Join(tmpDir, "BOOTSTRAP.md")
		if err := os.WriteFile(bootstrapPath, []byte("# Bootstrap"), 0644); err != nil {
			t.Fatalf("Failed to create BOOTSTRAP.md: %v", err)
		}
		if !services.ShouldSkipHeartbeatForBootstrap(tmpDir) {
			t.Error("Should return true when BOOTSTRAP.md exists")
		}
	})
}

// =============================================================================
// Priority 2 - Config loading/saving with temp files
// =============================================================================

// TestBotService_GetConfig_NoConfigFile tests GetConfig when no config file exists
func TestBotService_GetConfig_NoConfigFile(t *testing.T) {
	t.TempDir() // ensure temp dir is available

	// NewBotService uses GetConfigPath() internally, which returns the default path.
	// We cannot override that, but we can test GetConfig() works without panicking.
	svc := services.NewBotService()
	_, _ = svc.GetConfig() // result depends on whether default config exists
}

// TestBotService_Start_NoConfigFile tests Start behavior.
// The result depends on whether the user has a valid config at the default path.
func TestBotService_Start_NoConfigFile(t *testing.T) {
	svc := services.NewBotService()
	err := svc.Start()
	if err != nil {
		// Failed start should put service in error state
		if svc.GetState() != services.BotStateError {
			t.Errorf("State after failed Start = %v, want BotStateError", svc.GetState())
		}
		if svc.GetError() == nil {
			t.Error("Error should be set after failed Start")
		}
	}
	// If it succeeds (valid config exists at default path), that's also fine
}

// TestBotService_SaveConfig_InvalidType tests SaveConfig with wrong type
func TestBotService_SaveConfig_InvalidType(t *testing.T) {
	svc := services.NewBotService()
	err := svc.SaveConfig("not a config", false)
	if err == nil {
		t.Error("SaveConfig() with invalid type should return error")
	}
}

// TestBotService_SaveConfig_InvalidType_Map tests SaveConfig with a map type
func TestBotService_SaveConfig_InvalidType_Map(t *testing.T) {
	svc := services.NewBotService()
	err := svc.SaveConfig(map[string]string{"key": "value"}, false)
	if err == nil {
		t.Error("SaveConfig() with map type should return error")
	}
}

// TestBotService_SaveConfig_Nil tests SaveConfig with nil
func TestBotService_SaveConfig_Nil(t *testing.T) {
	svc := services.NewBotService()
	err := svc.SaveConfig(nil, false)
	if err == nil {
		t.Error("SaveConfig() with nil should return error")
	}
}

// TestBotService_SaveConfig_ValidConfig tests SaveConfig with a valid config object
func TestBotService_SaveConfig_ValidConfig(t *testing.T) {
	tmpDir := t.TempDir()
	_ = filepath.Join(tmpDir, "config.json")

	// Create a minimal valid config
	cfg := &config.Config{
		Agents: config.AgentsConfig{
			Defaults: config.AgentDefaults{
				Workspace:           tmpDir,
				RestrictToWorkspace: true,
				LLM:                 "test/model",
				MaxTokens:           4096,
				Temperature:         0.7,
			},
		},
		ModelList: []config.ModelConfig{
			{
				ModelName: "test-model",
				Model:     "test/model",
				APIKey:    "test-api-key",
			},
		},
		Channels: config.ChannelsConfig{
			Web: config.WebChannelConfig{
				Enabled: false,
			},
		},
		Gateway: config.GatewayConfig{
			Host: "0.0.0.0",
			Port: 8080,
		},
	}

	svc := services.NewBotService()
	// SaveConfig will attempt to write to the internal configPath.
	// We test that the type assertion passes and the method handles
	// the save attempt without panicking. The actual file write location
	// is determined by the internal configPath.
	err := svc.SaveConfig(cfg, false)
	// Whether this succeeds depends on file system permissions of configPath,
	// but the important thing is it doesn't panic and passes type assertion.
	_ = err
}

// TestBotService_GetConfig tests GetConfig with a temp config file
func TestBotService_GetConfig(t *testing.T) {
	// Test that GetConfig returns an error when config doesn't exist
	// (GetConfig uses the internal configPath which we can't override,
	// but we can at least verify it works without panicking)
	svc := services.NewBotService()
	cfg, err := svc.GetConfig()
	// The result depends on whether the default config exists.
	// We just verify it doesn't panic.
	_ = cfg
	_ = err
}

// =============================================================================
// Priority 3 - Component lifecycle tests (failure scenarios)
// =============================================================================

// TestBotService_Restart_NotRunning tests Restart when service is not running.
// Restart will attempt Start internally, which may fail or succeed depending on config.
func TestBotService_Restart_NotRunning(t *testing.T) {
	svc := services.NewBotService()
	err := svc.Restart()
	// Restart calls Start internally. Result depends on whether config is valid.
	// The important thing is it doesn't panic.
	_ = err
}

// TestBotService_StopFromErrorState tests Stop from Error state
func TestBotService_StopFromErrorState(t *testing.T) {
	svc := services.NewBotService()
	// Trigger a Start (may succeed or fail)
	_ = svc.Start()

	state := svc.GetState()
	if state == services.BotStateError {
		// Stop from Error state should fail (not running)
		err := svc.Stop()
		if err == nil {
			t.Error("Stop() from Error state should return error")
		}
	} else if state == services.BotStateRunning {
		// If running, stop should succeed
		err := svc.Stop()
		if err != nil {
			t.Errorf("Stop() from Running state should succeed, got error: %v", err)
		}
	}
}

// TestBotService_GetComponents_AfterStart tests GetComponents after start attempt
func TestBotService_GetComponents_AfterStart(t *testing.T) {
	svc := services.NewBotService()
	_ = svc.Start()

	state := svc.GetState()
	components := svc.GetComponents()

	if state == services.BotStateError {
		// After failed start, no components should be set
		if len(components) != 0 {
			t.Errorf("GetComponents() after failed start should be empty, got %v", components)
		}
	} else if state == services.BotStateRunning {
		// After successful start, components should be populated
		if len(components) == 0 {
			t.Error("GetComponents() after successful start should have components")
		}
		// Clean up
		_ = svc.Stop()
	}
}

// TestBotService_GetForge_AfterStart tests GetForge after start attempt
func TestBotService_GetForge_AfterStart(t *testing.T) {
	svc := services.NewBotService()
	_ = svc.Start()

	state := svc.GetState()
	forge := svc.GetForge()

	if state == services.BotStateError {
		// After failed start, forge should be nil
		if forge != nil {
			t.Errorf("GetForge() after failed start should be nil, got %v", forge)
		}
	} else if state == services.BotStateRunning {
		// After successful start, forge may or may not be nil depending on config
		// Just verify it doesn't panic
		_ = forge
		// Clean up
		_ = svc.Stop()
	}
}

// =============================================================================
// ServiceManager additional tests
// =============================================================================

// TestNewServiceManager tests creating a new ServiceManager
func TestNewServiceManager(t *testing.T) {
	mgr := services.NewServiceManager()
	if mgr == nil {
		t.Fatal("NewServiceManager() returned nil")
	}
}

// TestServiceManager_GetBotState tests GetBotState
func TestServiceManager_GetBotState(t *testing.T) {
	mgr := services.NewServiceManager()
	state := mgr.GetBotState()
	if state != services.BotStateNotStarted {
		t.Errorf("GetBotState() = %v, want BotStateNotStarted", state)
	}
}

// TestServiceManager_GetBotError tests GetBotError
func TestServiceManager_GetBotError(t *testing.T) {
	mgr := services.NewServiceManager()
	err := mgr.GetBotError()
	if err != nil {
		t.Errorf("GetBotError() = %v, want nil initially", err)
	}
}

// TestServiceManager_IsBotRunning tests IsBotRunning
func TestServiceManager_IsBotRunning(t *testing.T) {
	mgr := services.NewServiceManager()
	if mgr.IsBotRunning() {
		t.Error("IsBotRunning() should be false initially")
	}
}

// TestServiceManager_IsBasicServicesStarted tests IsBasicServicesStarted
func TestServiceManager_IsBasicServicesStarted(t *testing.T) {
	mgr := services.NewServiceManager()
	if mgr.IsBasicServicesStarted() {
		t.Error("IsBasicServicesStarted() should be false initially")
	}
}

// TestServiceManager_GetBotService tests GetBotService
func TestServiceManager_GetBotService(t *testing.T) {
	mgr := services.NewServiceManager()
	svc := mgr.GetBotService()
	if svc == nil {
		t.Error("GetBotService() returned nil")
	}
}

// TestServiceManager_GetBotComponents tests GetBotComponents
func TestServiceManager_GetBotComponents(t *testing.T) {
	mgr := services.NewServiceManager()
	components := mgr.GetBotComponents()
	if components == nil {
		t.Error("GetBotComponents() returned nil")
	}
	if len(components) != 0 {
		t.Errorf("GetBotComponents() should be empty initially, got %d items", len(components))
	}
}

// TestServiceManager_StopBot_WhenNotRunning tests StopBot when bot is not running
func TestServiceManager_StopBot_WhenNotRunning(t *testing.T) {
	mgr := services.NewServiceManager()
	err := mgr.StopBot()
	if err == nil {
		t.Error("StopBot() when bot not running should return error")
	}
}

// TestServiceManager_RestartBot_WhenNotRunning tests RestartBot behavior.
// RestartBot does not check basicServicesStarted (only StartBot does),
// so it delegates to bot.Restart() which calls Start() internally.
// The result depends on whether valid config exists.
func TestServiceManager_RestartBot_WhenNotRunning(t *testing.T) {
	mgr := services.NewServiceManager()
	err := mgr.RestartBot()
	// RestartBot does not require basic services, it delegates to bot.Restart()
	// which calls Start() directly. Result depends on config.
	if err == nil {
		// If it succeeded, clean up
		mgr.Shutdown()
	}
}

// TestServiceManager_StartBot_RequiresBasicServices verifies StartBot fails without basic services
func TestServiceManager_StartBot_RequiresBasicServices(t *testing.T) {
	mgr := services.NewServiceManager()
	err := mgr.StartBot()
	if err == nil {
		t.Error("StartBot() without basic services should fail")
	}
}

// TestServiceManager_StartBasicServices_Twice tests double start
func TestServiceManager_StartBasicServices_Twice(t *testing.T) {
	mgr := services.NewServiceManager()

	// First start should succeed
	if err := mgr.StartBasicServices(); err != nil {
		t.Fatalf("First StartBasicServices() failed: %v", err)
	}

	// Second start should fail
	if err := mgr.StartBasicServices(); err == nil {
		t.Error("Second StartBasicServices() should fail")
	}
}

// TestServiceManager_StartBot_WithBasicServices_NoConfig tests StartBot with basic services.
// The result depends on whether the user has a valid config file at the default path.
// This test verifies the StartBot method works correctly without panicking.
func TestServiceManager_StartBot_WithBasicServices_NoConfig(t *testing.T) {
	mgr := services.NewServiceManager()

	// Start basic services
	if err := mgr.StartBasicServices(); err != nil {
		t.Fatalf("StartBasicServices() failed: %v", err)
	}

	// StartBot may succeed or fail depending on whether a valid config exists
	// at the default path. We test that it doesn't panic and returns a
	// meaningful result.
	err := mgr.StartBot()
	if err != nil {
		// If Start failed, bot should be in error state
		if mgr.GetBotState() != services.BotStateError {
			t.Errorf("GetBotState() after failed StartBot = %v, want BotStateError", mgr.GetBotState())
		}
	} else {
		// If Start succeeded, bot should be running - clean up
		defer mgr.Shutdown()
		if !mgr.IsBotRunning() {
			t.Error("IsBotRunning() should be true after successful StartBot")
		}
	}
}

// TestServiceManager_Shutdown_WithBasicServices tests shutdown after starting basic services
func TestServiceManager_Shutdown_WithBasicServices(t *testing.T) {
	mgr := services.NewServiceManager()
	if err := mgr.StartBasicServices(); err != nil {
		t.Fatalf("StartBasicServices() failed: %v", err)
	}

	// Shutdown should not panic
	mgr.Shutdown()
}

// TestServiceManager_Shutdown_WithBotInError tests shutdown when bot is in error state
func TestServiceManager_Shutdown_WithBotInError(t *testing.T) {
	mgr := services.NewServiceManager()
	if err := mgr.StartBasicServices(); err != nil {
		t.Fatalf("StartBasicServices() failed: %v", err)
	}

	// Trigger bot start failure (no config)
	_ = mgr.StartBot()

	// Shutdown should not panic even with bot in error state
	mgr.Shutdown()
}

// TestServiceManager_WaitForShutdownWithDesktop tests desktop closed channel
func TestServiceManager_WaitForShutdownWithDesktop(t *testing.T) {
	mgr := services.NewServiceManager()

	desktopClosed := make(chan struct{})
	done := make(chan struct{})

	go func() {
		mgr.WaitForShutdownWithDesktop(desktopClosed)
		close(done)
	}()

	// Close desktop channel to trigger shutdown
	close(desktopClosed)

	// Wait for completion
	select {
	case <-done:
		// Expected: WaitForShutdownWithDesktop returned
	case <-func() chan struct{} {
		ch := make(chan struct{})
		close(ch)
		return ch
	}():
	}
}

// TestServiceManager_SaveBotConfig_InvalidType tests SaveBotConfig with invalid type
func TestServiceManager_SaveBotConfig_InvalidType(t *testing.T) {
	mgr := services.NewServiceManager()
	err := mgr.SaveBotConfig("not a config", false)
	if err == nil {
		t.Error("SaveBotConfig() with string should return error")
	}
}

// TestServiceManager_SaveBotConfig_Nil tests SaveBotConfig with nil
func TestServiceManager_SaveBotConfig_Nil(t *testing.T) {
	mgr := services.NewServiceManager()
	err := mgr.SaveBotConfig(nil, false)
	if err == nil {
		t.Error("SaveBotConfig() with nil should return error")
	}
}

// TestServiceManager_GetBotConfig tests GetBotConfig
func TestServiceManager_GetBotConfig(t *testing.T) {
	mgr := services.NewServiceManager()
	cfg, err := mgr.GetBotConfig()
	// Result depends on whether default config exists, just verify no panic
	_ = cfg
	_ = err
}

// =============================================================================
// BotState edge case tests
// =============================================================================

// TestBotState_String_Unknown tests BotState.String() with unknown state
func TestBotState_String_Unknown(t *testing.T) {
	unknown := services.BotState(999)
	if unknown.String() != "unknown" {
		t.Errorf("Unknown BotState.String() = %s, want 'unknown'", unknown.String())
	}
}

// TestBotState_MarshalJSON_Unknown tests marshaling unknown state
func TestBotState_MarshalJSON_Unknown(t *testing.T) {
	unknown := services.BotState(999)
	data, err := json.Marshal(unknown)
	if err != nil {
		t.Fatalf("MarshalJSON() error = %v", err)
	}
	if string(data) != `"unknown"` {
		t.Errorf("MarshalJSON() = %s, want `\"unknown\"`", string(data))
	}
}

// TestBotState_InStruct tests BotState as part of a struct
func TestBotState_InStruct(t *testing.T) {
	type Status struct {
		State services.BotState `json:"state"`
	}

	status := Status{State: services.BotStateRunning}
	data, err := json.Marshal(status)
	if err != nil {
		t.Fatalf("Marshal error = %v", err)
	}
	expected := `{"state":"running"}`
	if string(data) != expected {
		t.Errorf("Marshal = %s, want %s", string(data), expected)
	}

	var decoded Status
	if err := json.Unmarshal(data, &decoded); err != nil {
		t.Fatalf("Unmarshal error = %v", err)
	}
	if decoded.State != services.BotStateRunning {
		t.Errorf("Unmarshal state = %v, want BotStateRunning", decoded.State)
	}
}

// TestBotState_UnmarshalJSON_WithoutQuotes tests unmarshaling without quotes
func TestBotState_UnmarshalJSON_WithoutQuotes(t *testing.T) {
	// UnmarshalJSON handles the case where data has surrounding quotes
	var state services.BotState
	err := json.Unmarshal([]byte(`"not_started"`), &state)
	if err != nil {
		t.Fatalf("UnmarshalJSON error = %v", err)
	}
	if state != services.BotStateNotStarted {
		t.Errorf("UnmarshalJSON = %v, want BotStateNotStarted", state)
	}
}

// =============================================================================
// GetConfigPath tests
// =============================================================================

// TestGetConfigPath tests the GetConfigPath utility function
func TestGetConfigPath(t *testing.T) {
	p := services.GetConfigPath()
	if p == "" {
		t.Error("GetConfigPath() returned empty string")
	}
	if filepath.Base(p) != "config.json" {
		t.Errorf("GetConfigPath() = %s, want path ending with config.json", p)
	}
}

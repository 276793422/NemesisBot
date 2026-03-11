package services

import (
	"testing"
	"time"
)

// TestNewServiceManager tests creating a new ServiceManager
func TestNewServiceManager(t *testing.T) {
	svcMgr := NewServiceManager()

	if svcMgr == nil {
		t.Fatal("NewServiceManager returned nil")
	}

	if svcMgr.ctx == nil {
		t.Error("ServiceManager context is nil")
	}

	if svcMgr.cancel == nil {
		t.Error("ServiceManager cancel function is nil")
	}

	if svcMgr.botService == nil {
		t.Error("BotService is nil")
	}

	if svcMgr.basicServicesStarted {
		t.Error("Basic services should not be started initially")
	}
}

// TestServiceManager_StartBasicServices tests starting basic services
func TestServiceManager_StartBasicServices(t *testing.T) {
	svcMgr := NewServiceManager()

	err := svcMgr.StartBasicServices()
	if err != nil {
		t.Fatalf("StartBasicServices failed: %v", err)
	}

	if !svcMgr.basicServicesStarted {
		t.Error("Basic services should be started")
	}

	// Test starting again should fail
	err = svcMgr.StartBasicServices()
	if err == nil {
		t.Error("Starting basic services twice should fail")
	}
}

// TestServiceManager_BotServiceLifecycle tests bot service lifecycle
func TestServiceManager_BotServiceLifecycle(t *testing.T) {
	svcMgr := NewServiceManager()

	// Start basic services first
	if err := svcMgr.StartBasicServices(); err != nil {
		t.Fatalf("StartBasicServices failed: %v", err)
	}

	// Initial state should be NotStarted
	state := svcMgr.GetBotState()
	if state != BotStateNotStarted {
		t.Errorf("Initial state should be NotStarted, got %s", state)
	}

	// Note: Actual StartBot() test would require valid configuration
	// This test only checks the state management without starting real services
}

// TestServiceManager_StartBotWithoutBasicServices tests starting bot without basic services
func TestServiceManager_StartBotWithoutBasicServices(t *testing.T) {
	svcMgr := NewServiceManager()

	// Try to start bot without starting basic services first
	err := svcMgr.StartBot()
	if err == nil {
		t.Error("Starting bot without basic services should fail")
	}
}

// TestServiceManager_Shutdown tests graceful shutdown
func TestServiceManager_Shutdown(t *testing.T) {
	svcMgr := NewServiceManager()

	// Start basic services
	if err := svcMgr.StartBasicServices(); err != nil {
		t.Fatalf("StartBasicServices failed: %v", err)
	}

	// Shutdown should not panic
	svcMgr.Shutdown()

	// Context should be cancelled
	select {
	case <-svcMgr.ctx.Done():
		// Expected
	default:
		t.Error("Context should be cancelled after shutdown")
	}
}

// TestServiceManager_WaitForShutdown tests shutdown signal handling
func TestServiceManager_WaitForShutdown(t *testing.T) {
	t.Skip("Skipping signal handling test - requires OS signal environment")

	svcMgr := NewServiceManager()

	// Start basic services
	if err := svcMgr.StartBasicServices(); err != nil {
		t.Fatalf("StartBasicServices failed: %v", err)
	}

	// Test WaitForShutdown in a goroutine
	done := make(chan struct{})
	go func() {
		svcMgr.WaitForShutdown()
		close(done)
	}()

	// Cancel context to simulate shutdown signal
	time.AfterFunc(100*time.Millisecond, func() {
		svcMgr.cancel()
	})

	// Should complete within reasonable time
	select {
	case <-done:
		// Expected
	case <-time.After(2 * time.Second):
		t.Error("WaitForShutdown did not complete in time")
	}
}

// TestServiceManager_WaitForShutdownWithDesktop tests desktop UI shutdown handling
func TestServiceManager_WaitForShutdownWithDesktop(t *testing.T) {
	svcMgr := NewServiceManager()

	// Start basic services
	if err := svcMgr.StartBasicServices(); err != nil {
		t.Fatalf("StartBasicServices failed: %v", err)
	}

	// Test desktop closed signal
	desktopClosed := make(chan struct{})
	done := make(chan struct{})

	go func() {
		svcMgr.WaitForShutdownWithDesktop(desktopClosed)
		close(done)
	}()

	// Simulate desktop UI closing
	time.AfterFunc(100*time.Millisecond, func() {
		close(desktopClosed)
	})

	// Should complete within reasonable time
	select {
	case <-done:
		// Expected
	case <-time.After(2 * time.Second):
		t.Error("WaitForShutdownWithDesktop did not complete in time")
	}
}

// TestServiceManager_IsBasicServicesStarted tests checking if basic services are started
func TestServiceManager_IsBasicServicesStarted(t *testing.T) {
	svcMgr := NewServiceManager()

	// Initially should be false
	if svcMgr.IsBasicServicesStarted() {
		t.Error("Basic services should not be started initially")
	}

	// After starting should be true
	if err := svcMgr.StartBasicServices(); err != nil {
		t.Fatalf("StartBasicServices failed: %v", err)
	}

	if !svcMgr.IsBasicServicesStarted() {
		t.Error("Basic services should be started")
	}
}

// TestServiceManager_IsBotRunning tests checking if bot is running
func TestServiceManager_IsBotRunning(t *testing.T) {
	svcMgr := NewServiceManager()

	// Initially should be false (not started)
	if svcMgr.IsBotRunning() {
		t.Error("Bot should not be running initially")
	}

	// After starting basic services but not bot, should still be false
	if err := svcMgr.StartBasicServices(); err != nil {
		t.Fatalf("StartBasicServices failed: %v", err)
	}

	if svcMgr.IsBotRunning() {
		t.Error("Bot should not be running without being started")
	}
}

// TestServiceManager_GetBotService tests getting bot service instance
func TestServiceManager_GetBotService(t *testing.T) {
	svcMgr := NewServiceManager()

	botSvc := svcMgr.GetBotService()
	if botSvc == nil {
		t.Error("GetBotService returned nil")
	}

	if botSvc != svcMgr.botService {
		t.Error("GetBotService returned wrong instance")
	}
}

// TestBotState_String tests BotState string representation
func TestBotState_String(t *testing.T) {
	tests := []struct {
		state    BotState
		expected string
	}{
		{BotStateNotStarted, "not_started"},
		{BotStateStarting, "starting"},
		{BotStateRunning, "running"},
		{BotStateError, "error"},
		{BotState(999), "unknown"},
	}

	for _, tt := range tests {
		t.Run(tt.expected, func(t *testing.T) {
			if got := tt.state.String(); got != tt.expected {
				t.Errorf("BotState.String() = %s, want %s", got, tt.expected)
			}
		})
	}
}

// TestBotState_IsRunning tests BotState IsRunning method
func TestBotState_IsRunning(t *testing.T) {
	tests := []struct {
		state    BotState
		expected bool
	}{
		{BotStateNotStarted, false},
		{BotStateStarting, false},
		{BotStateRunning, true},
		{BotStateError, false},
	}

	for _, tt := range tests {
		t.Run(tt.state.String(), func(t *testing.T) {
			if got := tt.state.IsRunning(); got != tt.expected {
				t.Errorf("BotState.IsRunning() = %v, want %v", got, tt.expected)
			}
		})
	}
}

// TestBotState_CanStart tests BotState CanStart method
func TestBotState_CanStart(t *testing.T) {
	tests := []struct {
		state    BotState
		expected bool
	}{
		{BotStateNotStarted, true},
		{BotStateStarting, false},
		{BotStateRunning, false},
		{BotStateError, true},
	}

	for _, tt := range tests {
		t.Run(tt.state.String(), func(t *testing.T) {
			if got := tt.state.CanStart(); got != tt.expected {
				t.Errorf("BotState.CanStart() = %v, want %v", got, tt.expected)
			}
		})
	}
}

// TestBotState_CanStop tests BotState CanStop method
func TestBotState_CanStop(t *testing.T) {
	tests := []struct {
		state    BotState
		expected bool
	}{
		{BotStateNotStarted, false},
		{BotStateStarting, true},
		{BotStateRunning, true},
		{BotStateError, false},
	}

	for _, tt := range tests {
		t.Run(tt.state.String(), func(t *testing.T) {
			if got := tt.state.CanStop(); got != tt.expected {
				t.Errorf("BotState.CanStop() = %v, want %v", got, tt.expected)
			}
		})
	}
}

// TestGetConfigPath tests GetConfigPath utility function
func TestGetConfigPath(t *testing.T) {
	path := GetConfigPath()
	if path == "" {
		t.Error("GetConfigPath returned empty string")
	}

	// Path should contain "config.json"
	if len(path) < 11 || path[len(path)-11:] != "config.json" {
		t.Error("GetConfigPath should return path ending with config.json")
	}
}

// TestShouldSkipHeartbeatForBootstrap tests ShouldSkipHeartbeatForBootstrap utility function
func TestShouldSkipHeartbeatForBootstrap(t *testing.T) {
	// Test with non-existent workspace
	workspace := "/nonexistent/workspace"
	if ShouldSkipHeartbeatForBootstrap(workspace) {
		t.Error("Should return false for non-existent workspace")
	}

	// Test with current directory (likely doesn't have BOOTSTRAP.md)
	workspace = "."
	if ShouldSkipHeartbeatForBootstrap(workspace) {
		// This might fail if there's actually a BOOTSTRAP.md
		t.Log("Note: BOOTSTRAP.md exists in current directory")
	}
}

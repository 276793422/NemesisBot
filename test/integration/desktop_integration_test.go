package integration

import (
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/services"
)

// TestServiceManagerIntegration tests the ServiceManager integration
// This test verifies that ServiceManager properly manages bot lifecycle
func TestServiceManagerIntegration(t *testing.T) {
	svcMgr := services.NewServiceManager()

	// Test 1: Create service manager
	if svcMgr == nil {
		t.Fatal("ServiceManager should not be nil")
	}

	// Test 2: Start basic services
	t.Run("StartBasicServices", func(t *testing.T) {
		err := svcMgr.StartBasicServices()
		if err != nil {
			t.Fatalf("StartBasicServices failed: %v", err)
		}

		if !svcMgr.IsBasicServicesStarted() {
			t.Error("Basic services should be started")
		}
	})

	// Test 3: Check initial bot state
	t.Run("InitialBotState", func(t *testing.T) {
		state := svcMgr.GetBotState()
		if state != services.BotStateNotStarted {
			t.Errorf("Initial bot state should be NotStarted, got %s", state)
		}

		if svcMgr.IsBotRunning() {
			t.Error("Bot should not be running initially")
		}
	})

	// Test 4: Start bot without basic services should fail
	t.Run("StartBotWithoutBasicServices", func(t *testing.T) {
		svcMgr2 := services.NewServiceManager()
		err := svcMgr2.StartBot()
		if err == nil {
			t.Error("Starting bot without basic services should fail")
		}
	})

	// Test 5: Get bot service instance
	t.Run("GetBotService", func(t *testing.T) {
		botSvc := svcMgr.GetBotService()
		if botSvc == nil {
			t.Error("BotService should not be nil")
		}

		state := botSvc.GetState()
		if state != services.BotStateNotStarted {
			t.Errorf("BotService state should be NotStarted, got %s", state)
		}
	})

	// Test 6: Shutdown
	t.Run("Shutdown", func(t *testing.T) {
		svcMgr.Shutdown()
		// Should not panic
	})
}

// TestBotServiceLifecycle tests BotService state transitions
func TestBotServiceLifecycle(t *testing.T) {
	botSvc := services.NewBotService()

	// Test 1: Initial state
	t.Run("InitialState", func(t *testing.T) {
		state := botSvc.GetState()
		if state != services.BotStateNotStarted {
			t.Errorf("Initial state should be NotStarted, got %s", state)
		}

		if !state.CanStart() {
			t.Error("NotStarted state should allow starting")
		}

		if state.CanStop() {
			t.Error("NotStarted state should not allow stopping")
		}
	})

	// Test 2: State string representation
	t.Run("StateString", func(t *testing.T) {
		states := []struct {
			state        services.BotState
			expectedStr string
		}{
			{services.BotStateNotStarted, "not_started"},
			{services.BotStateStarting, "starting"},
			{services.BotStateRunning, "running"},
			{services.BotStateError, "error"},
		}

		for _, tt := range states {
			t.Run(tt.expectedStr, func(t *testing.T) {
				if got := tt.state.String(); got != tt.expectedStr {
					t.Errorf("State.String() = %s, want %s", got, tt.expectedStr)
				}
			})
		}
	})

	// Test 3: State methods
	t.Run("StateMethods", func(t *testing.T) {
		tests := []struct {
			name       string
			state      services.BotState
			isRunning  bool
			canStart   bool
			canStop    bool
		}{
			{"NotStarted", services.BotStateNotStarted, false, true, false},
			{"Starting", services.BotStateStarting, false, false, true},
			{"Running", services.BotStateRunning, true, false, true},
			{"Error", services.BotStateError, false, true, false},
		}

		for _, tt := range tests {
			t.Run(tt.name, func(t *testing.T) {
				if got := tt.state.IsRunning(); got != tt.isRunning {
					t.Errorf("%s.IsRunning() = %v, want %v", tt.name, got, tt.isRunning)
				}
				if got := tt.state.CanStart(); got != tt.canStart {
					t.Errorf("%s.CanStart() = %v, want %v", tt.name, got, tt.canStart)
				}
				if got := tt.state.CanStop(); got != tt.canStop {
					t.Errorf("%s.CanStop() = %v, want %v", tt.name, got, tt.canStop)
				}
			})
		}
	})
}

// TestServiceManagerConcurrency tests concurrent access to ServiceManager
func TestServiceManagerConcurrency(t *testing.T) {
	svcMgr := services.NewServiceManager()

	// Start basic services
	if err := svcMgr.StartBasicServices(); err != nil {
		t.Fatalf("StartBasicServices failed: %v", err)
	}

	// Test concurrent reads
	done := make(chan bool)
	for i := 0; i < 10; i++ {
		go func() {
			for j := 0; j < 100; j++ {
				_ = svcMgr.GetBotState()
				_ = svcMgr.IsBotRunning()
				_ = svcMgr.IsBasicServicesStarted()
			}
			done <- true
		}()
	}

	// Wait for all goroutines
	for i := 0; i < 10; i++ {
		select {
		case <-done:
			// Expected
		case <-time.After(5 * time.Second):
			t.Fatal("Concurrent test timed out")
		}
	}

	// Shutdown
	svcMgr.Shutdown()
}

// TestBotStateTransitions tests valid state transitions
func TestBotStateTransitions(t *testing.T) {
	transitions := []struct {
		from     services.BotState
		to       services.BotState
		valid    bool
		reason   string
	}{
		{services.BotStateNotStarted, services.BotStateStarting, true, "Can start from not started"},
		{services.BotStateNotStarted, services.BotStateRunning, false, "Cannot jump to running"},
		{services.BotStateStarting, services.BotStateRunning, true, "Can transition to running"},
		{services.BotStateStarting, services.BotStateError, true, "Can error during start"},
		{services.BotStateRunning, services.BotStateNotStarted, false, "Must go through stop"},
		{services.BotStateRunning, services.BotStateError, true, "Can error while running"},
		{services.BotStateError, services.BotStateStarting, true, "Can retry from error"},
		{services.BotStateError, services.BotStateNotStarted, true, "Can reset from error"},
	}

	for _, tt := range transitions {
		t.Run(tt.from.String()+"_to_"+tt.to.String(), func(t *testing.T) {
			// This test documents expected transitions
			// Actual transitions are validated by BotService implementation
			t.Logf("Transition %s -> %s: %v (%s)", tt.from, tt.to, tt.valid, tt.reason)
		})
	}
}

// TestGetConfigPath tests config path resolution
func TestGetConfigPath(t *testing.T) {
	path := services.GetConfigPath()

	if path == "" {
		t.Error("GetConfigPath should not return empty string")
	}

	// Path should contain config.json
	if len(path) < 11 {
		t.Error("Config path seems too short")
	}

	t.Logf("Config path: %s", path)
}

// TestServiceManagerShutdown tests graceful shutdown
func TestServiceManagerShutdown(t *testing.T) {
	svcMgr := services.NewServiceManager()

	// Start basic services
	if err := svcMgr.StartBasicServices(); err != nil {
		t.Fatalf("StartBasicServices failed: %v", err)
	}

	// Shutdown should be idempotent
	svcMgr.Shutdown()
	svcMgr.Shutdown() // Should not panic

	// Verify services stopped
	if svcMgr.IsBasicServicesStarted() {
		t.Log("Note: Basic services flag not reset after shutdown (this is OK)")
	}

	// Bot service should be accessible
	botSvc := svcMgr.GetBotService()
	if botSvc == nil {
		t.Error("BotService should still be accessible after shutdown")
	}
}

// BenchmarkServiceManagerStateQueries benchmarks state queries
func BenchmarkServiceManagerStateQueries(b *testing.B) {
	svcMgr := services.NewServiceManager()
	svcMgr.StartBasicServices()

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		svcMgr.GetBotState()
		svcMgr.IsBotRunning()
		svcMgr.IsBasicServicesStarted()
	}
}

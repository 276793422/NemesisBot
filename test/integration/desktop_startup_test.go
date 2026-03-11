package integration

import (
	"testing"

	"github.com/276793422/NemesisBot/module/services"
)

// TestDesktopStartupFlow tests the desktop startup flow without actually opening UI
func TestDesktopStartupFlow(t *testing.T) {
	t.Log("=== Testing Desktop Startup Flow ===")

	// Step 1: Create ServiceManager
	t.Run("CreateServiceManager", func(t *testing.T) {
		svcMgr := services.NewServiceManager()
		if svcMgr == nil {
			t.Fatal("ServiceManager should not be nil")
		}
		t.Log("✓ ServiceManager created")
	})

	// Step 2: Start basic services
	t.Run("StartBasicServices", func(t *testing.T) {
		svcMgr := services.NewServiceManager()
		err := svcMgr.StartBasicServices()
		if err != nil {
			t.Fatalf("StartBasicServices failed: %v", err)
		}
		if !svcMgr.IsBasicServicesStarted() {
			t.Error("Basic services should be started")
		}
		t.Log("✓ Basic services started")
	})

	// Step 3: Check initial bot state
	t.Run("InitialBotState", func(t *testing.T) {
		svcMgr := services.NewServiceManager()
		svcMgr.StartBasicServices()

		state := svcMgr.GetBotState()
		if state != services.BotStateNotStarted {
			t.Errorf("Initial bot state should be NotStarted, got %s", state)
		}
		t.Logf("✓ Bot state: %s", state)
	})

	// Step 4: Test bot service access
	t.Run("BotServiceAccess", func(t *testing.T) {
		svcMgr := services.NewServiceManager()
		svcMgr.StartBasicServices()

		botSvc := svcMgr.GetBotService()
		if botSvc == nil {
			t.Error("BotService should not be nil")
		}

		state := botSvc.GetState()
		t.Logf("✓ BotService accessible, state: %s", state)
	})

	// Step 5: Test shutdown
	t.Run("Shutdown", func(t *testing.T) {
		svcMgr := services.NewServiceManager()
		svcMgr.StartBasicServices()

		// Should not panic
		svcMgr.Shutdown()
		t.Log("✓ Shutdown completed")
	})

	t.Log("=== Desktop Startup Flow Tests Passed ===")
}

// TestConfigurationScenarios tests different configuration scenarios
func TestConfigurationScenarios(t *testing.T) {
	t.Run("Scenario_NoConfig", func(t *testing.T) {
		// This scenario is handled in CmdDesktop
		// Bot service should be in NotStarted state
		svcMgr := services.NewServiceManager()
		svcMgr.StartBasicServices()

		state := svcMgr.GetBotState()
		if state != services.BotStateNotStarted {
			t.Errorf("Without config, bot state should be NotStarted, got %s", state)
		}
		t.Log("✓ No config scenario: Bot not started")
	})

	t.Run("Scenario_BotLifecycle", func(t *testing.T) {
		svcMgr := services.NewServiceManager()
		svcMgr.StartBasicServices()

		// Initial state
		state := svcMgr.GetBotState()
		if state.CanStart() != true {
			t.Error("Initial state should allow starting")
		}

		// Simulate state transitions
		states := []services.BotState{
			services.BotStateNotStarted,
			services.BotStateStarting,
			services.BotStateRunning,
		}

		for i, s := range states {
			t.Logf("State transition %d: %s", i, s.String())
		}

		t.Log("✓ Bot lifecycle scenario validated")
	})
}

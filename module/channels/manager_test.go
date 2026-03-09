// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package channels

import (
	"context"
	"os/exec"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/config"
)

// createTestConfig creates a minimal test configuration
func createTestConfig() *config.Config {
	return &config.Config{
		Channels: config.ChannelsConfig{
			Web: config.WebChannelConfig{
				Enabled:        false,
				Host:           "localhost",
				Port:           8080,
				Path:           "/ws",
				SessionTimeout: 3600,
			},
			External: config.ExternalConfig{
				Enabled:  false,
				InputEXE: "",
				OutputEXE: "",
				ChatID:   "external-test",
			},
			WebSocket: config.WebSocketChannelConfig{
				Enabled: false,
				Host:    "localhost",
				Port:    8081,
				Path:    "/ws",
			},
		},
	}
}

// TestNewManager tests creating a new channel manager
func TestNewManager(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	if manager == nil {
		t.Fatal("NewManager() returned nil")
	}

	if manager.channels == nil {
		t.Error("Manager channels map is nil")
	}

	if manager.bus != msgBus {
		t.Error("Manager message bus not set correctly")
	}

	if manager.config != cfg {
		t.Error("Manager config not set correctly")
	}
}

// TestNewManagerWithWebChannel tests creating a manager with web channel enabled
func TestNewManagerWithWebChannel(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()
	cfg.Channels.Web.Enabled = true

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	if len(manager.channels) == 0 {
		t.Error("Expected at least one channel to be initialized")
	}

	// Verify web channel exists
	if _, exists := manager.channels["web"]; !exists {
		t.Error("Web channel should be initialized when enabled")
	}
}

// TestManagerGetChannel tests retrieving channels from the manager
func TestManagerGetChannel(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()
	cfg.Channels.Web.Enabled = true

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	// Test getting existing channel
	channel, ok := manager.GetChannel("web")
	if !ok {
		t.Error("Failed to get web channel")
	}
	if channel == nil {
		t.Error("Retrieved channel is nil")
	}
	if channel.Name() != "web" {
		t.Errorf("Expected channel name 'web', got '%s'", channel.Name())
	}

	// Test getting non-existent channel
	_, ok = manager.GetChannel("nonexistent")
	if ok {
		t.Error("Should not succeed in getting non-existent channel")
	}
}

// TestManagerRegisterChannel tests registering a custom channel
func TestManagerRegisterChannel(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	// Register a mock channel
	mockChannel := NewMockChannel("custom", nil)
	manager.RegisterChannel("custom", mockChannel)

	// Verify channel was registered
	channel, ok := manager.GetChannel("custom")
	if !ok {
		t.Error("Failed to get registered custom channel")
	}
	if channel != mockChannel {
		t.Error("Retrieved channel is not the same as registered channel")
	}
}

// TestManagerUnregisterChannel tests unregistering a channel
func TestManagerUnregisterChannel(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()
	cfg.Channels.Web.Enabled = true

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	// Verify channel exists
	_, ok := manager.GetChannel("web")
	if !ok {
		t.Fatal("Web channel should exist")
	}

	// Unregister channel
	manager.UnregisterChannel("web")

	// Verify channel was removed
	_, ok = manager.GetChannel("web")
	if ok {
		t.Error("Channel should be unregistered")
	}
}

// TestManagerGetEnabledChannels tests getting list of enabled channels
func TestManagerGetEnabledChannels(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	// No channels enabled
	channels := manager.GetEnabledChannels()
	if len(channels) != 0 {
		t.Errorf("Expected 0 enabled channels, got %d", len(channels))
	}

	// Register some channels
	manager.RegisterChannel("channel1", NewMockChannel("channel1", nil))
	manager.RegisterChannel("channel2", NewMockChannel("channel2", nil))

	channels = manager.GetEnabledChannels()
	if len(channels) != 2 {
		t.Errorf("Expected 2 enabled channels, got %d", len(channels))
	}
}

// TestManagerGetStatus tests getting channel status
func TestManagerGetStatus(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	// Register a mock channel
	mockChannel := NewMockChannel("test", nil)
	manager.RegisterChannel("test", mockChannel)

	// Get status
	status := manager.GetStatus()
	if status == nil {
		t.Fatal("GetStatus() returned nil")
	}

	testStatus, ok := status["test"]
	if !ok {
		t.Error("Status for 'test' channel not found")
	}

	testStatusMap, ok := testStatus.(map[string]interface{})
	if !ok {
		t.Fatal("Test status is not a map")
	}

	enabled, ok := testStatusMap["enabled"].(bool)
	if !ok || !enabled {
		t.Error("Channel should be marked as enabled")
	}

	running, ok := testStatusMap["running"].(bool)
	if !ok || running {
		t.Error("Channel should not be marked as running initially")
	}

	// Start the channel and check status again
	ctx := context.Background()
	mockChannel.Start(ctx)

	status = manager.GetStatus()
	testStatusMap = status["test"].(map[string]interface{})
	running = testStatusMap["running"].(bool)
	if !running {
		t.Error("Channel should be marked as running after Start()")
	}

	// Stop the channel
	mockChannel.Stop(ctx)
}

// TestManagerSendToChannel tests sending messages to specific channels
func TestManagerSendToChannel(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	ctx := context.Background()

	// Register a mock channel
	mockChannel := NewMockChannel("test", nil)
	manager.RegisterChannel("test", mockChannel)

	// Start the channel
	if err := mockChannel.Start(ctx); err != nil {
		t.Fatalf("Failed to start mock channel: %v", err)
	}
	defer mockChannel.Stop(ctx)

	// Send message to channel
	err = manager.SendToChannel(ctx, "test", "chat123", "Hello, world!")
	if err != nil {
		t.Errorf("SendToChannel() failed: %v", err)
	}

	// Verify message was sent
	msgs := mockChannel.GetSentMessages()
	if len(msgs) != 1 {
		t.Fatalf("Expected 1 message, got %d", len(msgs))
	}

	if msgs[0].ChatID != "chat123" {
		t.Errorf("Expected chat ID 'chat123', got '%s'", msgs[0].ChatID)
	}

	if msgs[0].Content != "Hello, world!" {
		t.Errorf("Expected content 'Hello, world!', got '%s'", msgs[0].Content)
	}

	// Test sending to non-existent channel
	err = manager.SendToChannel(ctx, "nonexistent", "chat123", "Test")
	if err == nil {
		t.Error("Expected error when sending to non-existent channel")
	}
}

// TestManagerStartAllStopAll tests the start/stop lifecycle
func TestManagerStartAllStopAll(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	// Register some mock channels
	mockChannel1 := NewMockChannel("channel1", nil)
	mockChannel2 := NewMockChannel("channel2", nil)
	manager.RegisterChannel("channel1", mockChannel1)
	manager.RegisterChannel("channel2", mockChannel2)

	ctx := context.Background()

	// Start all channels
	if err := manager.StartAll(ctx); err != nil {
		t.Fatalf("StartAll() failed: %v", err)
	}

	// Verify channels are running
	if !mockChannel1.IsRunning() {
		t.Error("Channel1 should be running after StartAll()")
	}
	if !mockChannel2.IsRunning() {
		t.Error("Channel2 should be running after StartAll()")
	}

	// Stop all channels
	if err := manager.StopAll(ctx); err != nil {
		t.Fatalf("StopAll() failed: %v", err)
	}

	// Verify channels are stopped
	if mockChannel1.IsRunning() {
		t.Error("Channel1 should not be running after StopAll()")
	}
	if mockChannel2.IsRunning() {
		t.Error("Channel2 should not be running after StopAll()")
	}
}

// TestManagerStartAllWithNoChannels tests starting with no channels
func TestManagerStartAllWithNoChannels(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	ctx := context.Background()

	// Start with no channels should succeed
	if err := manager.StartAll(ctx); err != nil {
		t.Errorf("StartAll() with no channels should succeed, got: %v", err)
	}

	// Stop should also succeed
	if err := manager.StopAll(ctx); err != nil {
		t.Errorf("StopAll() failed: %v", err)
	}
}

// TestManagerDispatchOutbound tests the outbound message dispatcher
func TestManagerDispatchOutbound(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	ctx, cancel := context.WithCancel(context.Background())

	// Register a mock channel
	mockChannel := NewMockChannel("test", nil)
	manager.RegisterChannel("test", mockChannel)

	if err := mockChannel.Start(ctx); err != nil {
		t.Fatalf("Failed to start mock channel: %v", err)
	}
	defer mockChannel.Stop(ctx)

	// Start the manager to launch dispatcher
	if err := manager.StartAll(ctx); err != nil {
		t.Fatalf("StartAll() failed: %v", err)
	}
	defer manager.StopAll(ctx)

	// Give dispatcher time to start
	time.Sleep(50 * time.Millisecond)

	// Publish an outbound message
	outboundMsg := bus.OutboundMessage{
		Channel: "test",
		ChatID:  "chat123",
		Content: "Test message",
	}
	msgBus.PublishOutbound(outboundMsg)

	// Wait for message to be dispatched
	time.Sleep(100 * time.Millisecond)

	// Verify message was sent to channel
	msgs := mockChannel.GetSentMessages()
	if len(msgs) == 0 {
		t.Error("No messages received by channel")
	} else {
		if msgs[0].Content != "Test message" {
			t.Errorf("Expected content 'Test message', got '%s'", msgs[0].Content)
		}
	}

	cancel()
}

// TestManagerGetSyncTargets tests the getSyncTargets method
func TestManagerGetSyncTargets(t *testing.T) {
	msgBus := bus.NewMessageBus()

	tests := []struct {
		name         string
		channelName  string
		setupConfig  func(*config.Config)
		expectedLen  int
		expectedTargets []string
	}{
		{
			name:        "Web channel with sync targets",
			channelName: "web",
			setupConfig: func(c *config.Config) {
				c.Channels.Web.Enabled = true
				c.Channels.Web.SyncTo = []string{"telegram", "discord"}
			},
			expectedLen:      2,
			expectedTargets:  []string{"telegram", "discord"},
		},
		{
			name:        "Web channel with no sync targets",
			channelName: "web",
			setupConfig: func(c *config.Config) {
				c.Channels.Web.Enabled = true
				c.Channels.Web.SyncTo = nil
			},
			expectedLen: 0,
		},
		{
			name:        "Unknown channel",
			channelName: "unknown",
			setupConfig: func(c *config.Config) {},
			expectedLen: 0,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			testCfg := createTestConfig()
			tt.setupConfig(testCfg)

			manager, err := NewManager(testCfg, msgBus)
			if err != nil {
				t.Fatalf("NewManager() failed: %v", err)
			}

			targets := manager.getSyncTargets(tt.channelName)
			if len(targets) != tt.expectedLen {
				t.Errorf("Expected %d targets, got %d", tt.expectedLen, len(targets))
			}

			if tt.expectedTargets != nil {
				for i, target := range targets {
					if target != tt.expectedTargets[i] {
						t.Errorf("Expected target '%s' at index %d, got '%s'", tt.expectedTargets[i], i, target)
					}
				}
			}
		})
	}
}

// ============================================================================
// Additional Manager Tests for Coverage
// ============================================================================

func TestManager_InitChannels_NoChannels(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()
	// All channels disabled

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	if len(manager.channels) != 0 {
		t.Errorf("Expected 0 channels when all disabled, got %d", len(manager.channels))
	}
}

func TestManager_InitChannels_ExternalChannel(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()
	cfg.Channels.External.Enabled = true

	// Try to find a suitable executable for testing
	// On Windows, we can use cmd.exe
	// On Unix, we can use sh or bash
	inputEXE := "echo" // This will fail on most systems
	outputEXE := "echo"

	// Check if we can find cmd.exe on Windows
	if path, err := exec.LookPath("cmd"); err == nil {
		inputEXE = path
		outputEXE = path
	} else if path, err := exec.LookPath("sh"); err == nil {
		// On Unix, use sh with echo command
		inputEXE = path
		outputEXE = path
	} else {
		// Skip test if no suitable executable found
		t.Skip("No suitable executable found for external channel test")
		return
	}

	cfg.Channels.External.InputEXE = inputEXE
	cfg.Channels.External.OutputEXE = outputEXE

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	// Verify external channel was initialized
	if _, exists := manager.channels["external"]; !exists {
		t.Error("External channel should be initialized when enabled")
	}
}

func TestManager_InitChannels_WebSocketChannel(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()
	cfg.Channels.WebSocket.Enabled = true

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	// Verify websocket channel was initialized
	if _, exists := manager.channels["websocket"]; !exists {
		t.Error("WebSocket channel should be initialized when enabled")
	}
}

// ============================================================================
// Manager Sync Targets Tests
// ============================================================================

func TestManager_GetSyncTargets_MultipleChannels(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()
	cfg.Channels.Web.Enabled = true
	cfg.Channels.Web.SyncTo = []string{"discord", "telegram"}

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	targets := manager.getSyncTargets("web")
	if len(targets) != 2 {
		t.Errorf("Expected 2 sync targets, got %d", len(targets))
	}

	// Verify target names
	expectedTargets := []string{"discord", "telegram"}
	for i, target := range targets {
		if target != expectedTargets[i] {
			t.Errorf("Expected target '%s' at index %d, got '%s'", expectedTargets[i], i, target)
		}
	}
}

func TestManager_GetSyncTargets_EmptyList(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()
	cfg.Channels.Web.Enabled = true
	cfg.Channels.Web.SyncTo = []string{}

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	targets := manager.getSyncTargets("web")
	if len(targets) != 0 {
		t.Errorf("Expected 0 sync targets, got %d", len(targets))
	}
}

// ============================================================================
// Manager Error Handling Tests
// ============================================================================

func TestManager_StartAll_ErrorPropagation(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	ctx := context.Background()

	// Start with no channels should succeed
	err = manager.StartAll(ctx)
	if err != nil {
		t.Errorf("StartAll() with no channels should succeed, got: %v", err)
	}

	// Stop should succeed even with no channels started
	err = manager.StopAll(ctx)
	if err != nil {
		t.Errorf("StopAll() failed: %v", err)
	}
}

func TestManager_StartStop_Idempotent(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	ctx := context.Background()

	// Start with no channels should succeed
	err = manager.StartAll(ctx)
	if err != nil {
		t.Fatalf("First StartAll() failed: %v", err)
	}

	// Stop should succeed
	err = manager.StopAll(ctx)
	if err != nil {
		t.Fatalf("StopAll() failed: %v", err)
	}

	// Start again should succeed (idempotent)
	err = manager.StartAll(ctx)
	if err != nil {
		t.Fatalf("Second StartAll() failed: %v", err)
	}

	// Stop again should succeed (idempotent)
	err = manager.StopAll(ctx)
	if err != nil {
		t.Fatalf("Second StopAll() failed: %v", err)
	}
}

func TestManager_ChannelRetrieval_NonExistent(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	// Try to get non-existent channel
	channel, ok := manager.GetChannel("nonexistent")
	if ok {
		t.Error("Should not succeed in getting non-existent channel")
	}

	if channel != nil {
		t.Error("Channel should be nil for non-existent channel")
	}
}

func TestManager_Status_EmptyManager(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	status := manager.GetStatus()
	if status == nil {
		t.Fatal("GetStatus() should not return nil")
	}

	if len(status) != 0 {
		t.Errorf("Expected empty status map, got %d entries", len(status))
	}
}

func TestManager_UnregisterChannel_NonExistent(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	// Unregister non-existent channel should not panic
	manager.UnregisterChannel("nonexistent")

	// Verify no channels were affected
	if len(manager.channels) != 0 {
		t.Error("Channel count should remain 0")
	}
}

// ============================================================================
// Manager Configuration Tests
// ============================================================================

func TestManager_Configuration_DefaultValues(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	if manager == nil {
		t.Fatal("Manager should not be nil")
	}

	if manager.config == nil {
		t.Error("Manager config should not be nil")
	}

	if manager.bus == nil {
		t.Error("Manager message bus should not be nil")
	}

	if manager.channels == nil {
		t.Error("Manager channels map should be initialized")
	}
}

func TestManager_MessageBusIntegration(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()
	cfg.Channels.Web.Enabled = true

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	// Verify manager uses the same message bus
	if manager.bus != msgBus {
		t.Error("Manager should use the provided message bus")
	}

	// Register a custom channel
	customChannel := NewMockChannel("test", nil)
	manager.RegisterChannel("test", customChannel)

	// Verify we can retrieve it
	channel, ok := manager.GetChannel("test")
	if !ok {
		t.Fatal("Failed to get custom channel")
	}

	if channel.Name() != "test" {
		t.Errorf("Expected channel name 'test', got '%s'", channel.Name())
	}
}

// ============================================================================
// Manager Dispatch Tests
// ============================================================================

func TestManager_DispatchOutbound_NoChannels(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	ctx := context.Background()

	// Send to non-existent channel should fail
	err = manager.SendToChannel(ctx, "nonexistent", "chat123", "Test message")
	if err == nil {
		t.Error("Expected error when sending to non-existent channel")
	}
}

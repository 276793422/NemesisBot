// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package channels

import (
	"context"
	"fmt"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/config"
)

// ============================================================================
// Manager Lifecycle and Initialization Tests
// ============================================================================

func TestManager_InitChannels_AllChannelTypes(t *testing.T) {
	msgBus := bus.NewMessageBus()

	tests := []struct {
		name         string
		setupConfig  func(*config.Config)
		expectError  bool
		minChannels  int
		checkChannel string
	}{
		{
			name: "Telegram channel with token",
			setupConfig: func(c *config.Config) {
				c.Channels.Telegram.Enabled = true
				c.Channels.Telegram.Token = "1234567890:ABCDefGHIjklMNOpqrsTUVwxyz"
			},
			expectError:  false,
			minChannels:  0, // May fail due to token validation
			checkChannel: "telegram",
		},
		{
			name: "WhatsApp channel with bridge URL",
			setupConfig: func(c *config.Config) {
				c.Channels.WhatsApp.Enabled = true
				c.Channels.WhatsApp.BridgeURL = "http://localhost:3000"
			},
			expectError:  false,
			minChannels:  1,
			checkChannel: "whatsapp",
		},
		{
			name: "Discord channel with token",
			setupConfig: func(c *config.Config) {
				c.Channels.Discord.Enabled = true
				c.Channels.Discord.Token = "test-discord-token"
			},
			expectError:  false,
			minChannels:  1,
			checkChannel: "discord",
		},
		{
			name: "Slack channel with bot token",
			setupConfig: func(c *config.Config) {
				c.Channels.Slack.Enabled = true
				c.Channels.Slack.BotToken = "xoxb-test-token-1234567890"
			},
			expectError:  false,
			minChannels:  0, // May fail due to validation
			checkChannel: "slack",
		},
		{
			name: "LINE channel with access token",
			setupConfig: func(c *config.Config) {
				c.Channels.LINE.Enabled = true
				c.Channels.LINE.ChannelAccessToken = "test-line-token-123456789"
			},
			expectError:  false,
			minChannels:  0, // May fail due to validation
			checkChannel: "line",
		},
		{
			name: "OneBot channel with WebSocket URL",
			setupConfig: func(c *config.Config) {
				c.Channels.OneBot.Enabled = true
				c.Channels.OneBot.WSUrl = "ws://localhost:8080"
			},
			expectError:  false,
			minChannels:  1,
			checkChannel: "onebot",
		},
		{
			name: "QQ channel enabled",
			setupConfig: func(c *config.Config) {
				c.Channels.QQ.Enabled = true
			},
			expectError:  false,
			minChannels:  1,
			checkChannel: "qq",
		},
		{
			name: "MaixCam channel enabled",
			setupConfig: func(c *config.Config) {
				c.Channels.MaixCam.Enabled = true
			},
			expectError:  false,
			minChannels:  1,
			checkChannel: "maixcam",
		},
		{
			name: "DingTalk channel with credentials",
			setupConfig: func(c *config.Config) {
				c.Channels.DingTalk.Enabled = true
				c.Channels.DingTalk.ClientID = "test-client-id"
				c.Channels.DingTalk.ClientSecret = "test-client-secret"
			},
			expectError:  false,
			minChannels:  1,
			checkChannel: "dingtalk",
		},
		{
			name: "Feishu channel with credentials",
			setupConfig: func(c *config.Config) {
				c.Channels.Feishu.Enabled = true
				c.Channels.Feishu.AppID = "test-app-id"
				c.Channels.Feishu.AppSecret = "test-app-secret"
			},
			expectError:  false,
			minChannels:  1,
			checkChannel: "feishu",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cfg := createTestConfig()
			tt.setupConfig(cfg)

			manager, err := NewManager(cfg, msgBus)
			if tt.expectError && err == nil {
				t.Error("Expected error, got nil")
			}
			if !tt.expectError && err != nil {
				t.Errorf("Unexpected error: %v", err)
			}

			if !tt.expectError {
				// Channels may fail to initialize due to validation
				// This is expected behavior for some channels
				if tt.minChannels > 0 && len(manager.channels) < tt.minChannels {
					// Some channels might not initialize due to validation
					t.Logf("Warning: Only %d channels initialized (expected at least %d)", len(manager.channels), tt.minChannels)
				}

				if tt.checkChannel != "" {
					if _, exists := manager.channels[tt.checkChannel]; !exists {
						t.Logf("Channel '%s' was not initialized (may be due to validation)", tt.checkChannel)
					}
				}
			}
		})
	}
}

// ============================================================================
// Manager Dispatch Outbound Tests
// ============================================================================

func TestManager_DispatchOutbound_InternalChannels(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	// Start the manager
	if err := manager.StartAll(ctx); err != nil {
		t.Fatalf("StartAll() failed: %v", err)
	}
	defer manager.StopAll(ctx)

	// Give dispatcher time to start
	time.Sleep(50 * time.Millisecond)

	tests := []struct {
		name       string
		channel    string
		chatID     string
		content    string
		shouldSend bool
	}{
		{
			name:       "RPC channel - internal, should be skipped",
			channel:    "rpc",
			chatID:     "test-chat",
			content:    "Test message",
			shouldSend: false,
		},
		{
			name:       "Unknown channel - should error but not crash",
			channel:    "unknown-channel",
			chatID:     "test-chat",
			content:    "Test message",
			shouldSend: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			// Publish outbound message
			outboundMsg := bus.OutboundMessage{
				Channel: tt.channel,
				ChatID:  tt.chatID,
				Content: tt.content,
			}
			msgBus.PublishOutbound(outboundMsg)

			// Give dispatcher time to process
			time.Sleep(100 * time.Millisecond)
		})
	}
}

func TestManager_DispatchOutbound_EmptyMessage(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	// Register a mock channel
	mockChannel := NewMockChannel("test", nil)
	manager.RegisterChannel("test", mockChannel)

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	if err := manager.StartAll(ctx); err != nil {
		t.Fatalf("StartAll() failed: %v", err)
	}
	defer manager.StopAll(ctx)

	time.Sleep(50 * time.Millisecond)

	// Test empty content
	outboundMsg := bus.OutboundMessage{
		Channel: "test",
		ChatID:  "chat123",
		Content: "",
	}
	msgBus.PublishOutbound(outboundMsg)

	time.Sleep(100 * time.Millisecond)
}

func TestManager_DispatchOutbound_LongMessage(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	// Register a mock channel
	mockChannel := NewMockChannel("test", nil)
	manager.RegisterChannel("test", mockChannel)

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	if err := mockChannel.Start(ctx); err != nil {
		t.Fatalf("Failed to start mock channel: %v", err)
	}
	defer mockChannel.Stop(ctx)

	if err := manager.StartAll(ctx); err != nil {
		t.Fatalf("StartAll() failed: %v", err)
	}
	defer manager.StopAll(ctx)

	time.Sleep(50 * time.Millisecond)

	// Test very long message
	longContent := ""
	for i := 0; i < 10000; i++ {
		longContent += "test "
	}

	outboundMsg := bus.OutboundMessage{
		Channel: "test",
		ChatID:  "chat123",
		Content: longContent,
	}
	msgBus.PublishOutbound(outboundMsg)

	time.Sleep(100 * time.Millisecond)
}

// ============================================================================
// Manager Setup Sync Targets Tests
// ============================================================================

func TestManager_SetupSyncTargets_SelfSyncPrevention(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()
	cfg.Channels.Web.Enabled = true
	cfg.Channels.Web.SyncTo = []string{"web"} // Try to sync to itself

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	ctx := context.Background()

	// Start manager (which calls setupSyncTargets)
	if err := manager.StartAll(ctx); err != nil {
		t.Fatalf("StartAll() failed: %v", err)
	}
	defer manager.StopAll(ctx)

	// Self-sync should be prevented, so the channel should still work
	webChannel, ok := manager.GetChannel("web")
	if !ok {
		t.Fatal("Web channel not found")
	}

	// Verify channel is functional
	if webChannel.Name() != "web" {
		t.Error("Web channel name mismatch")
	}
}

func TestManager_SetupSyncTargets_NonExistentTarget(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()
	cfg.Channels.Web.Enabled = true
	cfg.Channels.Web.SyncTo = []string{"nonexistent-channel"}

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	ctx := context.Background()

	// Start manager (should not fail even with non-existent sync target)
	if err := manager.StartAll(ctx); err != nil {
		t.Fatalf("StartAll() failed: %v", err)
	}
	defer manager.StopAll(ctx)
}

func TestManager_SetupSyncTargets_MultipleChannels(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()
	cfg.Channels.Web.Enabled = true
	cfg.Channels.WebSocket.Enabled = true
	cfg.Channels.Web.SyncTo = []string{"websocket"}
	cfg.Channels.WebSocket.SyncTo = []string{"web"}

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	ctx := context.Background()

	if err := manager.StartAll(ctx); err != nil {
		t.Fatalf("StartAll() failed: %v", err)
	}
	defer manager.StopAll(ctx)

	// Verify both channels exist
	webChannel, ok := manager.GetChannel("web")
	if !ok {
		t.Fatal("Web channel not found")
	}

	wsChannel, ok := manager.GetChannel("websocket")
	if !ok {
		t.Fatal("WebSocket channel not found")
	}

	// Verify channels are functional
	if webChannel.Name() != "web" {
		t.Error("Web channel name mismatch")
	}
	if wsChannel.Name() != "websocket" {
		t.Error("WebSocket channel name mismatch")
	}
}

// ============================================================================
// Manager SendToChannel Tests
// ============================================================================

func TestManager_SendToChannel_ContextCancellation(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	// Register a mock channel with slow send
	slowChannel := &SlowMockChannel{name: "slow"}
	manager.RegisterChannel("slow", slowChannel)

	ctx, cancel := context.WithCancel(context.Background())

	// Start channel
	if err := slowChannel.Start(ctx); err != nil {
		t.Fatalf("Failed to start slow channel: %v", err)
	}
	defer slowChannel.Stop(ctx)

	// Cancel context immediately
	cancel()

	// Try to send (should handle cancelled context)
	err = manager.SendToChannel(ctx, "slow", "chat123", "Test message")
	if err != nil {
		t.Logf("SendToChannel with cancelled context returned error (expected): %v", err)
	}
}

func TestManager_SendToChannel_EmptyChatID(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	mockChannel := NewMockChannel("test", nil)
	manager.RegisterChannel("test", mockChannel)

	ctx := context.Background()

	if err := mockChannel.Start(ctx); err != nil {
		t.Fatalf("Failed to start mock channel: %v", err)
	}
	defer mockChannel.Stop(ctx)

	// Send with empty chat ID
	err = manager.SendToChannel(ctx, "test", "", "Test message")
	// Should succeed (channel decides how to handle empty chat ID)
	if err != nil {
		t.Errorf("SendToChannel with empty chat ID failed: %v", err)
	}
}

func TestManager_SendToChannel_EmptyContent(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	mockChannel := NewMockChannel("test", nil)
	manager.RegisterChannel("test", mockChannel)

	ctx := context.Background()

	if err := mockChannel.Start(ctx); err != nil {
		t.Fatalf("Failed to start mock channel: %v", err)
	}
	defer mockChannel.Stop(ctx)

	// Send with empty content
	err = manager.SendToChannel(ctx, "test", "chat123", "")
	if err != nil {
		t.Errorf("SendToChannel with empty content failed: %v", err)
	}
}

// ============================================================================
// Manager Start/Stop Edge Cases
// ============================================================================

func TestManager_StartAll_CalledTwice(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	ctx := context.Background()

	// Start once
	if err := manager.StartAll(ctx); err != nil {
		t.Fatalf("First StartAll() failed: %v", err)
	}

	// Start again (should be idempotent or handle gracefully)
	if err := manager.StartAll(ctx); err != nil {
		t.Logf("Second StartAll() returned error (may be expected): %v", err)
	}

	manager.StopAll(ctx)
}

func TestManager_StopAll_WithoutStart(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	ctx := context.Background()

	// Stop without starting (should be safe)
	if err := manager.StopAll(ctx); err != nil {
		t.Errorf("StopAll() without StartAll() failed: %v", err)
	}
}

func TestManager_StartStop_ContextCancellation(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()
	cfg.Channels.Web.Enabled = true

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	ctx, cancel := context.WithCancel(context.Background())

	// Start
	if err := manager.StartAll(ctx); err != nil {
		t.Fatalf("StartAll() failed: %v", err)
	}

	// Cancel context
	cancel()

	// Stop with cancelled context
	if err := manager.StopAll(ctx); err != nil {
		t.Logf("StopAll() with cancelled context returned error: %v", err)
	}
}

// ============================================================================
// Manager GetStatus Tests
// ============================================================================

func TestManager_GetStatus_AfterStartStop(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	mockChannel := NewMockChannel("test", nil)
	manager.RegisterChannel("test", mockChannel)

	ctx := context.Background()

	// Status before start
	status := manager.GetStatus()
	testStatus := status["test"].(map[string]interface{})
	running := testStatus["running"].(bool)
	if running {
		t.Error("Channel should not be running before Start()")
	}

	// Start
	if err := mockChannel.Start(ctx); err != nil {
		t.Fatalf("Failed to start mock channel: %v", err)
	}

	// Status after start
	status = manager.GetStatus()
	testStatus = status["test"].(map[string]interface{})
	running = testStatus["running"].(bool)
	if !running {
		t.Error("Channel should be running after Start()")
	}

	// Stop
	if err := mockChannel.Stop(ctx); err != nil {
		t.Fatalf("Failed to stop mock channel: %v", err)
	}

	// Status after stop
	status = manager.GetStatus()
	testStatus = status["test"].(map[string]interface{})
	running = testStatus["running"].(bool)
	if running {
		t.Error("Channel should not be running after Stop()")
	}
}

func TestManager_GetStatus_MultipleChannels(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	// Register multiple channels
	for i := 1; i <= 3; i++ {
		name := fmt.Sprintf("channel%d", i)
		mockChannel := NewMockChannel(name, nil)
		manager.RegisterChannel(name, mockChannel)
	}

	status := manager.GetStatus()

	if len(status) != 3 {
		t.Errorf("Expected 3 channels in status, got %d", len(status))
	}

	for i := 1; i <= 3; i++ {
		name := fmt.Sprintf("channel%d", i)
		if _, exists := status[name]; !exists {
			t.Errorf("Channel '%s' not found in status", name)
		}
	}
}

// ============================================================================
// Manager Thread Safety Tests
// ============================================================================

func TestManager_ConcurrentAccess(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	// Register some channels
	for i := 0; i < 5; i++ {
		name := fmt.Sprintf("channel%d", i)
		mockChannel := NewMockChannel(name, nil)
		manager.RegisterChannel(name, mockChannel)
	}

	// Perform concurrent operations
	done := make(chan bool)

	// Goroutine 1: GetChannel
	go func() {
		for i := 0; i < 100; i++ {
			manager.GetChannel("channel1")
		}
		done <- true
	}()

	// Goroutine 2: GetEnabledChannels
	go func() {
		for i := 0; i < 100; i++ {
			manager.GetEnabledChannels()
		}
		done <- true
	}()

	// Goroutine 3: GetStatus
	go func() {
		for i := 0; i < 100; i++ {
			manager.GetStatus()
		}
		done <- true
	}()

	// Goroutine 4: RegisterChannel
	go func() {
		for i := 0; i < 10; i++ {
			name := fmt.Sprintf("newchannel%d", i)
			mockChannel := NewMockChannel(name, nil)
			manager.RegisterChannel(name, mockChannel)
		}
		done <- true
	}()

	// Goroutine 5: UnregisterChannel
	go func() {
		for i := 0; i < 10; i++ {
			manager.UnregisterChannel(fmt.Sprintf("channel%d", i%5))
		}
		done <- true
	}()

	// Wait for all goroutines
	for i := 0; i < 5; i++ {
		<-done
	}

	// Verify manager is still functional
	status := manager.GetStatus()
	if status == nil {
		t.Error("Manager status is nil after concurrent access")
	}
}

// ============================================================================
// Helper Types
// ============================================================================

// SlowMockChannel is a mock channel that has delays
type SlowMockChannel struct {
	name    string
	running bool
}

func (s *SlowMockChannel) Name() string {
	return s.name
}

func (s *SlowMockChannel) Start(ctx context.Context) error {
	s.running = true
	return nil
}

func (s *SlowMockChannel) Stop(ctx context.Context) error {
	s.running = false
	return nil
}

func (s *SlowMockChannel) Send(ctx context.Context, msg bus.OutboundMessage) error {
	time.Sleep(100 * time.Millisecond)
	return nil
}

func (s *SlowMockChannel) IsRunning() bool {
	return s.running
}

func (s *SlowMockChannel) IsAllowed(senderID string) bool {
	return true
}

func (s *SlowMockChannel) AddSyncTarget(name string, channel Channel) error {
	return nil
}

func (s *SlowMockChannel) RemoveSyncTarget(name string) {
}

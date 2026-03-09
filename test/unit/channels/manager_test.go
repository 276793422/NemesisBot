// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package channels_test

import (
	"context"
	"fmt"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/channels"
	"github.com/276793422/NemesisBot/module/config"
)

// TestNewManager tests creating a new channel manager
func TestNewManager(t *testing.T) {
	cfg := &config.Config{
		Channels: config.ChannelsConfig{
			Web: config.WebChannelConfig{
				Enabled: false, // Disable all channels for empty manager test
			},
			External: config.ExternalConfig{
				Enabled: false,
			},
			WebSocket: config.WebSocketChannelConfig{
				Enabled: false,
			},
			Telegram: config.TelegramConfig{
				Enabled: false,
			},
		},
	}

	msgBus := bus.NewMessageBus()

	// Test with minimal config (no channels enabled)
	manager, err := channels.NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("Failed to create manager: %v", err)
	}

	if manager == nil {
		t.Fatal("Expected manager to be created, got nil")
	}

	// Should have no enabled channels
	channels := manager.GetEnabledChannels()
	if len(channels) != 0 {
		t.Errorf("Expected 0 channels, got %d", len(channels))
	}
}

// TestNewManagerWithWebChannel tests creating manager with web channel
func TestNewManagerWithWebChannel(t *testing.T) {
	cfg := &config.Config{
		Channels: config.ChannelsConfig{
			Web: config.WebChannelConfig{
				Enabled: true,
				Host:    "127.0.0.1",
				Port:    49100,
				Path:    "/ws",
			},
			External: config.ExternalConfig{
				Enabled: false,
			},
			WebSocket: config.WebSocketChannelConfig{
				Enabled: false,
			},
			Telegram: config.TelegramConfig{
				Enabled: false,
			},
		},
	}

	msgBus := bus.NewMessageBus()

	manager, err := channels.NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("Failed to create manager: %v", err)
	}

	enabledChannels := manager.GetEnabledChannels()
	if len(enabledChannels) != 1 {
		t.Errorf("Expected 1 channel, got %d", len(enabledChannels))
	}

	if enabledChannels[0] != "web" {
		t.Errorf("Expected 'web' channel, got '%s'", enabledChannels[0])
	}
}

// TestManagerGetChannel tests retrieving channels from manager
func TestManagerGetChannel(t *testing.T) {
	cfg := &config.Config{
		Channels: config.ChannelsConfig{
			Web: config.WebChannelConfig{
				Enabled: true,
				Host:    "127.0.0.1",
				Port:    49101,
				Path:    "/ws",
			},
			External: config.ExternalConfig{
				Enabled: false,
			},
			WebSocket: config.WebSocketChannelConfig{
				Enabled: false,
			},
			Telegram: config.TelegramConfig{
				Enabled: false,
			},
		},
	}

	msgBus := bus.NewMessageBus()

	manager, err := channels.NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("Failed to create manager: %v", err)
	}

	// Test getting existing channel
	channel, exists := manager.GetChannel("web")
	if !exists {
		t.Error("Expected web channel to exist")
	}

	if channel == nil {
		t.Error("Expected channel to be non-nil")
	}

	if channel.Name() != "web" {
		t.Errorf("Expected channel name 'web', got '%s'", channel.Name())
	}

	// Test getting non-existent channel
	_, exists = manager.GetChannel("nonexistent")
	if exists {
		t.Error("Expected non-existent channel to not exist")
	}
}

// TestManagerGetEnabledChannels tests listing enabled channels
func TestManagerGetEnabledChannels(t *testing.T) {
	cfg := &config.Config{
		Channels: config.ChannelsConfig{
			Web: config.WebChannelConfig{
				Enabled: true,
				Host:    "127.0.0.1",
				Port:    49102,
				Path:    "/ws",
			},
			External: config.ExternalConfig{
				Enabled: false,
			},
			WebSocket: config.WebSocketChannelConfig{
				Enabled: false,
			},
			Telegram: config.TelegramConfig{
				Enabled: false,
			},
		},
	}

	msgBus := bus.NewMessageBus()

	manager, err := channels.NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("Failed to create manager: %v", err)
	}

	channels := manager.GetEnabledChannels()
	if len(channels) != 1 {
		t.Errorf("Expected 1 channel, got %d", len(channels))
	}

	if channels[0] != "web" {
		t.Errorf("Expected 'web' channel, got '%s'", channels[0])
	}
}

// TestManagerStartStop tests starting and stopping all channels
func TestManagerStartStop(t *testing.T) {
	cfg := &config.Config{
		Channels: config.ChannelsConfig{
			Web: config.WebChannelConfig{
				Enabled: true,
				Host:    "127.0.0.1",
				Port:    49103,
				Path:    "/ws",
			},
			External: config.ExternalConfig{
				Enabled: false,
			},
			WebSocket: config.WebSocketChannelConfig{
				Enabled: false,
			},
			Telegram: config.TelegramConfig{
				Enabled: false,
			},
		},
	}

	msgBus := bus.NewMessageBus()

	manager, err := channels.NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("Failed to create manager: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	// Start all channels
	if err := manager.StartAll(ctx); err != nil {
		t.Fatalf("Failed to start all channels: %v", err)
	}

	// Verify channel is running
	channel, exists := manager.GetChannel("web")
	if !exists {
		t.Fatal("Expected web channel to exist")
	}

	if !channel.IsRunning() {
		t.Error("Expected channel to be running after StartAll")
	}

	// Give server time to start
	time.Sleep(200 * time.Millisecond)

	// Stop all channels
	if err := manager.StopAll(ctx); err != nil {
		t.Fatalf("Failed to stop all channels: %v", err)
	}

	// Give server time to stop
	time.Sleep(100 * time.Millisecond)

	// Verify channel is stopped
	if channel.IsRunning() {
		t.Error("Expected channel to be stopped after StopAll")
	}
}

// TestManagerStartStopEmptyManager tests starting/stopping manager with no channels
func TestManagerStartStopEmptyManager(t *testing.T) {
	cfg := &config.Config{
		Channels: config.ChannelsConfig{
			Web: config.WebChannelConfig{
				Enabled: false,
			},
			External: config.ExternalConfig{
				Enabled: false,
			},
			WebSocket: config.WebSocketChannelConfig{
				Enabled: false,
			},
			Telegram: config.TelegramConfig{
				Enabled: false,
			},
		},
	}

	msgBus := bus.NewMessageBus()

	manager, err := channels.NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("Failed to create manager: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	// Should not fail with no channels
	if err := manager.StartAll(ctx); err != nil {
		t.Errorf("StartAll should not fail with no channels: %v", err)
	}

	if err := manager.StopAll(ctx); err != nil {
		t.Errorf("StopAll should not fail with no channels: %v", err)
	}
}

// TestManagerGetStatus tests getting channel status
func TestManagerGetStatus(t *testing.T) {
	cfg := &config.Config{
		Channels: config.ChannelsConfig{
			Web: config.WebChannelConfig{
				Enabled: true,
				Host:    "127.0.0.1",
				Port:    49104,
				Path:    "/ws",
			},
			External: config.ExternalConfig{
				Enabled: false,
			},
			WebSocket: config.WebSocketChannelConfig{
				Enabled: false,
			},
			Telegram: config.TelegramConfig{
				Enabled: false,
			},
		},
	}

	msgBus := bus.NewMessageBus()

	manager, err := channels.NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("Failed to create manager: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	// Get status before starting
	status := manager.GetStatus()
	webStatus, exists := status["web"]
	if !exists {
		t.Fatal("Expected web channel in status")
	}

	webStatusMap, ok := webStatus.(map[string]interface{})
	if !ok {
		t.Fatal("Expected status to be a map")
	}

	if !webStatusMap["enabled"].(bool) {
		t.Error("Expected web channel to be enabled")
	}

	if webStatusMap["running"].(bool) {
		t.Error("Expected web channel to not be running before StartAll")
	}

	// Start and check status again
	if err := manager.StartAll(ctx); err != nil {
		t.Fatalf("Failed to start all channels: %v", err)
	}
	defer manager.StopAll(ctx)

	time.Sleep(200 * time.Millisecond)

	status = manager.GetStatus()
	webStatus = status["web"]
	webStatusMap = webStatus.(map[string]interface{})

	if !webStatusMap["running"].(bool) {
		t.Error("Expected web channel to be running after StartAll")
	}
}

// TestManagerRegisterChannel tests registering a custom channel
func TestManagerRegisterChannel(t *testing.T) {
	cfg := &config.Config{
		Channels: config.ChannelsConfig{
			Web: config.WebChannelConfig{
				Enabled: false,
			},
			External: config.ExternalConfig{
				Enabled: false,
			},
			WebSocket: config.WebSocketChannelConfig{
				Enabled: false,
			},
			Telegram: config.TelegramConfig{
				Enabled: false,
			},
		},
	}

	msgBus := bus.NewMessageBus()

	manager, err := channels.NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("Failed to create manager: %v", err)
	}

	// Register a custom channel
	customChannel := newTestMockChannel("custom", nil)
	manager.RegisterChannel("custom", customChannel)

	// Verify it was registered
	channel, exists := manager.GetChannel("custom")
	if !exists {
		t.Error("Expected custom channel to exist after registration")
	}

	if channel.Name() != "custom" {
		t.Errorf("Expected channel name 'custom', got '%s'", channel.Name())
	}
}

// TestManagerUnregisterChannel tests unregistering a channel
func TestManagerUnregisterChannel(t *testing.T) {
	cfg := &config.Config{
		Channels: config.ChannelsConfig{
			Web: config.WebChannelConfig{
				Enabled: false,
			},
			External: config.ExternalConfig{
				Enabled: false,
			},
			WebSocket: config.WebSocketChannelConfig{
				Enabled: false,
			},
			Telegram: config.TelegramConfig{
				Enabled: false,
			},
		},
	}

	msgBus := bus.NewMessageBus()

	manager, err := channels.NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("Failed to create manager: %v", err)
	}

	// Register and then unregister
	customChannel := newTestMockChannel("custom", nil)
	manager.RegisterChannel("custom", customChannel)

	manager.UnregisterChannel("custom")

	// Verify it was unregistered
	_, exists := manager.GetChannel("custom")
	if exists {
		t.Error("Expected custom channel to not exist after unregistration")
	}
}

// TestManagerSendToChannel tests sending messages through manager
func TestManagerSendToChannel(t *testing.T) {
	cfg := &config.Config{
		Channels: config.ChannelsConfig{
			Web: config.WebChannelConfig{
				Enabled: false,
			},
			External: config.ExternalConfig{
				Enabled: false,
			},
			WebSocket: config.WebSocketChannelConfig{
				Enabled: false,
			},
			Telegram: config.TelegramConfig{
				Enabled: false,
			},
		},
	}

	msgBus := bus.NewMessageBus()

	manager, err := channels.NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("Failed to create manager: %v", err)
	}

	// Register a mock channel
	received := make(chan bus.OutboundMessage, 1)
	mockCh := newTestMockChannel("test", nil)
	mockCh.sendFunc = func(ctx context.Context, msg bus.OutboundMessage) error {
		received <- msg
		return nil
	}
	manager.RegisterChannel("test", mockCh)

	ctx := context.Background()

	// Send message
	err = manager.SendToChannel(ctx, "test", "chat1", "Hello")
	if err != nil {
		t.Fatalf("Failed to send to channel: %v", err)
	}

	// Verify message was received
	select {
	case msg := <-received:
		if msg.ChatID != "chat1" {
			t.Errorf("Expected chat ID 'chat1', got '%s'", msg.ChatID)
		}
		if msg.Content != "Hello" {
			t.Errorf("Expected content 'Hello', got '%s'", msg.Content)
		}
	case <-time.After(1 * time.Second):
		t.Fatal("Timeout waiting for message")
	}
}

// TestManagerSendToNonExistentChannel tests sending to non-existent channel
func TestManagerSendToNonExistentChannel(t *testing.T) {
	cfg := &config.Config{
		Channels: config.ChannelsConfig{
			Web: config.WebChannelConfig{
				Enabled: false,
			},
			External: config.ExternalConfig{
				Enabled: false,
			},
			WebSocket: config.WebSocketChannelConfig{
				Enabled: false,
			},
			Telegram: config.TelegramConfig{
				Enabled: false,
			},
		},
	}

	msgBus := bus.NewMessageBus()

	manager, err := channels.NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("Failed to create manager: %v", err)
	}

	ctx := context.Background()

	// Try to send to non-existent channel
	err = manager.SendToChannel(ctx, "nonexistent", "chat1", "Hello")
	if err == nil {
		t.Error("Expected error when sending to non-existent channel")
	}

	expectedErr := "channel nonexistent not found"
	if err.Error() != expectedErr {
		t.Errorf("Expected error '%s', got '%s'", expectedErr, err.Error())
	}
}

// TestManagerDispatchOutbound tests the outbound message dispatcher
func TestManagerDispatchOutbound(t *testing.T) {
	cfg := &config.Config{
		Channels: config.ChannelsConfig{
			Web: config.WebChannelConfig{
				Enabled: false,
			},
			External: config.ExternalConfig{
				Enabled: false,
			},
			WebSocket: config.WebSocketChannelConfig{
				Enabled: false,
			},
			Telegram: config.TelegramConfig{
				Enabled: false,
			},
		},
	}

	msgBus := bus.NewMessageBus()

	manager, err := channels.NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("Failed to create manager: %v", err)
	}

	// Register a mock channel
	received := make(chan bus.OutboundMessage, 1)
	mockCh := newTestMockChannel("test", nil)
	mockCh.sendFunc = func(ctx context.Context, msg bus.OutboundMessage) error {
		received <- msg
		return nil
	}
	manager.RegisterChannel("test", mockCh)

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	// Start manager (starts dispatcher)
	if err := manager.StartAll(ctx); err != nil {
		t.Fatalf("Failed to start manager: %v", err)
	}
	defer manager.StopAll(ctx)

	// Give dispatcher time to start
	time.Sleep(100 * time.Millisecond)

	// Publish outbound message
	msgBus.PublishOutbound(bus.OutboundMessage{
		Channel: "test",
		ChatID:  "chat1",
		Content: "Hello",
	})

	// Verify message was dispatched
	select {
	case msg := <-received:
		if msg.ChatID != "chat1" {
			t.Errorf("Expected chat ID 'chat1', got '%s'", msg.ChatID)
		}
		if msg.Content != "Hello" {
			t.Errorf("Expected content 'Hello', got '%s'", msg.Content)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("Timeout waiting for dispatched message")
	}
}

// TestManagerDispatchOutboundUnknownChannel tests dispatching to unknown channel
func TestManagerDispatchOutboundUnknownChannel(t *testing.T) {
	cfg := &config.Config{
		Channels: config.ChannelsConfig{
			Web: config.WebChannelConfig{
				Enabled: false,
			},
			External: config.ExternalConfig{
				Enabled: false,
			},
			WebSocket: config.WebSocketChannelConfig{
				Enabled: false,
			},
			Telegram: config.TelegramConfig{
				Enabled: false,
			},
		},
	}

	msgBus := bus.NewMessageBus()

	manager, err := channels.NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("Failed to create manager: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	// Start manager
	if err := manager.StartAll(ctx); err != nil {
		t.Fatalf("Failed to start manager: %v", err)
	}
	defer manager.StopAll(ctx)

	time.Sleep(100 * time.Millisecond)

	// Publish to unknown channel - should not panic
	msgBus.PublishOutbound(bus.OutboundMessage{
		Channel: "nonexistent",
		ChatID:  "chat1",
		Content: "Hello",
	})

	// Give dispatcher time to process
	time.Sleep(100 * time.Millisecond)
}

// TestManagerConcurrentAccess tests concurrent access to manager methods
func TestManagerConcurrentAccess(t *testing.T) {
	cfg := &config.Config{
		Channels: config.ChannelsConfig{
			Web: config.WebChannelConfig{
				Enabled: false,
			},
			External: config.ExternalConfig{
				Enabled: false,
			},
			WebSocket: config.WebSocketChannelConfig{
				Enabled: false,
			},
			Telegram: config.TelegramConfig{
				Enabled: false,
			},
		},
	}

	msgBus := bus.NewMessageBus()

	manager, err := channels.NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("Failed to create manager: %v", err)
	}

	// Register multiple channels
	for i := 0; i < 5; i++ {
		name := fmt.Sprintf("channel%d", i)
		ch := newTestMockChannel(name, nil)
		manager.RegisterChannel(name, ch)
	}

	// Run concurrent operations
	done := make(chan bool)

	// Concurrent GetChannel calls
	for i := 0; i < 10; i++ {
		go func() {
			for j := 0; j < 100; j++ {
				manager.GetChannel("channel1")
			}
			done <- true
		}()
	}

	// Concurrent GetEnabledChannels calls
	for i := 0; i < 10; i++ {
		go func() {
			for j := 0; j < 100; j++ {
				manager.GetEnabledChannels()
			}
			done <- true
		}()
	}

	// Concurrent GetStatus calls
	for i := 0; i < 10; i++ {
		go func() {
			for j := 0; j < 100; j++ {
				manager.GetStatus()
			}
			done <- true
		}()
	}

	// Wait for all goroutines
	for i := 0; i < 30; i++ {
		<-done
	}

	// Verify manager still works
	channels := manager.GetEnabledChannels()
	if len(channels) != 5 {
		t.Errorf("Expected 5 channels, got %d", len(channels))
	}
}

// TestManagerMultipleChannels tests manager with multiple channels
func TestManagerMultipleChannels(t *testing.T) {
	cfg := &config.Config{
		Channels: config.ChannelsConfig{
			Web: config.WebChannelConfig{
				Enabled: true,
				Host:    "127.0.0.1",
				Port:    49105,
				Path:    "/ws",
			},
			WebSocket: config.WebSocketChannelConfig{
				Enabled: true,
				Host:    "127.0.0.1",
				Port:    49106,
				Path:    "/ws",
			},
			External: config.ExternalConfig{
				Enabled: false,
			},
			Telegram: config.TelegramConfig{
				Enabled: false,
			},
		},
	}

	msgBus := bus.NewMessageBus()

	manager, err := channels.NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("Failed to create manager: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	// Start all channels
	if err := manager.StartAll(ctx); err != nil {
		t.Fatalf("Failed to start all channels: %v", err)
	}

	time.Sleep(200 * time.Millisecond)

	// Verify both channels are running
	webCh, exists := manager.GetChannel("web")
	if !exists {
		t.Error("Expected web channel to exist")
	}
	if !webCh.IsRunning() {
		t.Error("Expected web channel to be running")
	}

	wsCh, exists := manager.GetChannel("websocket")
	if !exists {
		t.Error("Expected websocket channel to exist")
	}
	if !wsCh.IsRunning() {
		t.Error("Expected websocket channel to be running")
	}

	// Stop all
	if err := manager.StopAll(ctx); err != nil {
		t.Fatalf("Failed to stop all channels: %v", err)
	}

	time.Sleep(100 * time.Millisecond)

	// Verify both are stopped
	if webCh.IsRunning() {
		t.Error("Expected web channel to be stopped")
	}
	if wsCh.IsRunning() {
		t.Error("Expected websocket channel to be stopped")
	}
}

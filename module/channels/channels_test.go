// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package channels

import (
	"context"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/config"
)

// TestWebSocketChannel tests WebSocket channel creation and lifecycle
func TestWebSocketChannel(t *testing.T) {
	msgBus := bus.NewMessageBus()

	cfg := &config.WebSocketChannelConfig{
		Enabled: false,
		Host:    "localhost",
		Port:    8081,
		Path:    "/ws",
	}

	channel, err := NewWebSocketChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewWebSocketChannel() failed: %v", err)
	}

	if channel == nil {
		t.Fatal("NewWebSocketChannel() returned nil")
	}

	if channel.Name() != "websocket" {
		t.Errorf("Expected name 'websocket', got '%s'", channel.Name())
	}

	// Test Send when not running
	ctx := context.Background()
	err = channel.Send(ctx, bus.OutboundMessage{
		Channel: "websocket",
		ChatID:  "test",
		Content: "test",
	})
	if err == nil {
		t.Error("Expected error when Send is called on stopped channel")
	}

	// Test Start/Stop
	err = channel.Start(ctx)
	if err != nil {
		t.Fatalf("Start() failed: %v", err)
	}

	if !channel.IsRunning() {
		t.Error("Channel should be running after Start()")
	}

	// Stop the channel
	err = channel.Stop(ctx)
	if err != nil {
		t.Fatalf("Stop() failed: %v", err)
	}

	if channel.IsRunning() {
		t.Error("Channel should not be running after Stop()")
	}
}

// TestExternalChannel tests External channel creation and validation
func TestExternalChannel(t *testing.T) {
	msgBus := bus.NewMessageBus()

	tests := []struct {
		name    string
		config  *config.ExternalConfig
		wantErr bool
	}{
		{
			name: "Missing input exe",
			config: &config.ExternalConfig{
				InputEXE:  "",
				OutputEXE: "test.exe",
				ChatID:    "test",
			},
			wantErr: true,
		},
		{
			name: "Missing output exe",
			config: &config.ExternalConfig{
				InputEXE:  "test.exe",
				OutputEXE: "",
				ChatID:    "test",
			},
			wantErr: true,
		},
		{
			name: "Non-existent input exe",
			config: &config.ExternalConfig{
				InputEXE:  "nonexistent.exe",
				OutputEXE: "nonexistent.exe",
				ChatID:    "test",
			},
			wantErr: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			_, err := NewExternalChannel(tt.config, msgBus)
			if tt.wantErr && err == nil {
				t.Error("Expected error, got nil")
			}
		})
	}
}

// TestWebChannel tests Web channel creation and lifecycle
func TestWebChannel(t *testing.T) {
	msgBus := bus.NewMessageBus()

	cfg := &config.WebChannelConfig{
		Enabled:        false,
		Host:           "localhost",
		Port:           8080,
		Path:           "/ws",
		SessionTimeout: 3600,
	}

	channel, err := NewWebChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewWebChannel() failed: %v", err)
	}

	if channel == nil {
		t.Fatal("NewWebChannel() returned nil")
	}

	if channel.Name() != "web" {
		t.Errorf("Expected name 'web', got '%s'", channel.Name())
	}

	// Test Send when not running
	ctx := context.Background()
	err = channel.Send(ctx, bus.OutboundMessage{
		Channel: "web",
		ChatID:  "web:test",
		Content: "test",
	})
	if err == nil {
		t.Error("Expected error when Send is called on stopped channel")
	}

	// Test invalid chat ID format
	channel.setRunning(true)
	err = channel.Send(ctx, bus.OutboundMessage{
		Channel: "web",
		ChatID:  "invalid-format",
		Content: "test",
	})
	if err == nil {
		t.Error("Expected error for invalid chat ID format")
	}

	// Test broadcast
	err = channel.BroadcastToAll("test broadcast")
	if err != nil {
		t.Errorf("BroadcastToAll() failed: %v", err)
	}

	channel.setRunning(false)
}

// TestWebChannel_Send_HistoryType tests that Send with Type="history" is correctly typed
func TestWebChannel_Send_HistoryType(t *testing.T) {
	msgBus := bus.NewMessageBus()

	cfg := &config.WebChannelConfig{
		Enabled:        false,
		Host:           "localhost",
		Port:           8080,
		Path:           "/ws",
		SessionTimeout: 3600,
	}

	channel, err := NewWebChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewWebChannel() failed: %v", err)
	}

	// When not running, should error regardless of type
	ctx := context.Background()
	err = channel.Send(ctx, bus.OutboundMessage{
		Channel: "web",
		ChatID:  "web:test-session",
		Content: `{"request_id":"r1","messages":[]}`,
		Type:    "history",
	})
	if err == nil {
		t.Error("Expected error when sending history to stopped channel")
	}
}

// TestWebChannel_OutboundMessage_TypeField tests that the Type field on OutboundMessage
// correctly distinguishes normal from history messages
func TestWebChannel_OutboundMessage_TypeField(t *testing.T) {
	normalMsg := bus.OutboundMessage{
		Channel: "web",
		ChatID:  "web:session-1",
		Content: "hello",
		Type:    "",
	}
	if normalMsg.Type != "" {
		t.Errorf("Normal message Type should be empty, got %q", normalMsg.Type)
	}

	historyMsg := bus.OutboundMessage{
		Channel: "web",
		ChatID:  "web:session-1",
		Content: `{"request_id":"r1"}`,
		Type:    "history",
	}
	if historyMsg.Type != "history" {
		t.Errorf("History message Type should be 'history', got %q", historyMsg.Type)
	}
}
func TestTelegramChannel(t *testing.T) {
	msgBus := bus.NewMessageBus()

	cfg := &config.Config{
		Channels: config.ChannelsConfig{
			Telegram: config.TelegramConfig{
				Enabled: false,
				Token:   "",
			},
		},
	}

	// Test creation with empty token - should fail
	_, err := NewTelegramChannel(cfg, msgBus)
	if err == nil {
		t.Error("Expected error when creating Telegram channel with empty token")
	}
}

// TestWhatsAppChannel tests WhatsApp channel creation
func TestWhatsAppChannel(t *testing.T) {
	msgBus := bus.NewMessageBus()

	cfg := config.WhatsAppConfig{
		Enabled:   false,
		BridgeURL: "",
	}

	// Test creation with empty bridge URL (should still create channel)
	channel, err := NewWhatsAppChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewWhatsAppChannel() failed: %v", err)
	}

	if channel == nil {
		t.Fatal("NewWhatsAppChannel() returned nil")
	}

	if channel.Name() != "whatsapp" {
		t.Errorf("Expected name 'whatsapp', got '%s'", channel.Name())
	}
}

// TestDiscordChannel tests Discord channel creation
func TestDiscordChannel(t *testing.T) {
	msgBus := bus.NewMessageBus()

	cfg := config.DiscordConfig{
		Enabled: false,
		Token:   "",
	}

	// Test creation with empty token (should still create channel)
	channel, err := NewDiscordChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewDiscordChannel() failed: %v", err)
	}

	if channel == nil {
		t.Fatal("NewDiscordChannel() returned nil")
	}

	if channel.Name() != "discord" {
		t.Errorf("Expected name 'discord', got '%s'", channel.Name())
	}
}

// TestFeishuChannel tests Feishu channel creation
func TestFeishuChannel(t *testing.T) {
	msgBus := bus.NewMessageBus()

	cfg := config.FeishuConfig{
		Enabled: false,
	}

	// Test creation
	channel, err := NewFeishuChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewFeishuChannel() failed: %v", err)
	}

	if channel == nil {
		t.Fatal("NewFeishuChannel() returned nil")
	}

	if channel.Name() != "feishu" {
		t.Errorf("Expected name 'feishu', got '%s'", channel.Name())
	}
}

// TestSlackChannel tests Slack channel creation
func TestSlackChannel(t *testing.T) {
	msgBus := bus.NewMessageBus()

	cfg := config.SlackConfig{
		Enabled:  false,
		BotToken: "",
	}

	// Test creation with empty token - should fail
	_, err := NewSlackChannel(cfg, msgBus)
	if err == nil {
		t.Error("Expected error when creating Slack channel with empty token")
	}
}

// TestLINEChannel tests LINE channel creation
func TestLINEChannel(t *testing.T) {
	msgBus := bus.NewMessageBus()

	cfg := config.LINEConfig{
		Enabled:            false,
		ChannelAccessToken: "",
	}

	// Test creation with empty token - should fail
	_, err := NewLINEChannel(cfg, msgBus)
	if err == nil {
		t.Error("Expected error when creating LINE channel with empty token")
	}
}

// TestOneBotChannel tests OneBot channel creation
func TestOneBotChannel(t *testing.T) {
	msgBus := bus.NewMessageBus()

	cfg := config.OneBotConfig{
		Enabled: false,
		WSUrl:   "",
	}

	// Test creation with empty URL (should still create channel)
	channel, err := NewOneBotChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewOneBotChannel() failed: %v", err)
	}

	if channel == nil {
		t.Fatal("NewOneBotChannel() returned nil")
	}

	if channel.Name() != "onebot" {
		t.Errorf("Expected name 'onebot', got '%s'", channel.Name())
	}
}

// TestQQChannel tests QQ channel creation
func TestQQChannel(t *testing.T) {
	msgBus := bus.NewMessageBus()

	cfg := config.QQConfig{
		Enabled: false,
	}

	// Test creation
	channel, err := NewQQChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewQQChannel() failed: %v", err)
	}

	if channel == nil {
		t.Fatal("NewQQChannel() returned nil")
	}

	if channel.Name() != "qq" {
		t.Errorf("Expected name 'qq', got '%s'", channel.Name())
	}
}

// TestDingTalkChannel tests DingTalk channel creation
func TestDingTalkChannel(t *testing.T) {
	msgBus := bus.NewMessageBus()

	cfg := config.DingTalkConfig{
		Enabled:  false,
		ClientID: "",
	}

	// Test creation with empty client ID - should fail
	_, err := NewDingTalkChannel(cfg, msgBus)
	if err == nil {
		t.Error("Expected error when creating DingTalk channel with empty client ID")
	}
}

// TestMaixCamChannel tests MaixCam channel creation
func TestMaixCamChannel(t *testing.T) {
	msgBus := bus.NewMessageBus()

	cfg := config.MaixCamConfig{
		Enabled: false,
	}

	// Test creation
	channel, err := NewMaixCamChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewMaixCamChannel() failed: %v", err)
	}

	if channel == nil {
		t.Fatal("NewMaixCamChannel() returned nil")
	}

	if channel.Name() != "maixcam" {
		t.Errorf("Expected name 'maixcam', got '%s'", channel.Name())
	}
}

// TestMinFunction tests the min helper function
func TestMinFunction(t *testing.T) {
	tests := []struct {
		a, b     int
		expected int
	}{
		{1, 2, 1},
		{2, 1, 1},
		{0, 0, 0},
		{-1, 1, -1},
		{100, 100, 100},
	}

	for _, tt := range tests {
		result := min(tt.a, tt.b)
		if result != tt.expected {
			t.Errorf("min(%d, %d) = %d, expected %d", tt.a, tt.b, result, tt.expected)
		}
	}
}

// TestManagerInitChannels tests manager initialization with various channel configs
func TestManagerInitChannels(t *testing.T) {
	tests := []struct {
		name    string
		setup   func(*config.Config)
		wantMin int // minimum number of channels expected
		wantMax int // maximum number of channels expected
	}{
		{
			name: "No channels enabled",
			setup: func(c *config.Config) {
				c.Channels.Web.Enabled = false
				c.Channels.External.Enabled = false
				c.Channels.WebSocket.Enabled = false
			},
			wantMin: 0,
			wantMax: 0,
		},
		{
			name: "Web channel enabled",
			setup: func(c *config.Config) {
				c.Channels.Web.Enabled = true
			},
			wantMin: 1,
			wantMax: 1,
		},
		{
			name: "WebSocket channel enabled",
			setup: func(c *config.Config) {
				c.Channels.WebSocket.Enabled = true
			},
			wantMin: 1,
			wantMax: 1,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cfg := createTestConfig()
			tt.setup(cfg)

			msgBus := bus.NewMessageBus()
			manager, err := NewManager(cfg, msgBus)
			if err != nil {
				t.Fatalf("NewManager() failed: %v", err)
			}

			count := len(manager.GetEnabledChannels())
			if count < tt.wantMin || count > tt.wantMax {
				t.Errorf("Expected %d-%d channels, got %d", tt.wantMin, tt.wantMax, count)
			}
		})
	}
}

// TestManagerStopIdempotency tests that StopAll can be called multiple times
func TestManagerStopIdempotency(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := createTestConfig()
	cfg.Channels.Web.Enabled = true

	manager, err := NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewManager() failed: %v", err)
	}

	ctx := context.Background()

	// Start
	if err := manager.StartAll(ctx); err != nil {
		t.Fatalf("StartAll() failed: %v", err)
	}

	// Stop multiple times
	for i := 0; i < 3; i++ {
		if err := manager.StopAll(ctx); err != nil {
			t.Errorf("StopAll() iteration %d failed: %v", i, err)
		}
	}
}

// TestBaseChannelHandleMessageWithMedia tests HandleMessage with media
func TestBaseChannelHandleMessageWithMedia(t *testing.T) {
	msgBus := bus.NewMessageBus()
	channel := NewBaseChannel("test", nil, msgBus, nil)

	received := make(chan bus.InboundMessage, 1)
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 200*time.Millisecond)
		defer cancel()
		msg, ok := msgBus.ConsumeInbound(ctx)
		if ok {
			received <- msg
		}
	}()

	media := []string{"image1.jpg", "image2.png", "video.mp4"}
	metadata := map[string]string{"key1": "value1", "key2": "value2"}

	channel.HandleMessage("sender123", "chat456", "Check out these images!", media, metadata)

	select {
	case msg := <-received:
		if len(msg.Media) != 3 {
			t.Errorf("Expected 3 media items, got %d", len(msg.Media))
		}
		if msg.Metadata == nil {
			t.Error("Expected metadata to be preserved")
		} else if msg.Metadata["key1"] != "value1" {
			t.Errorf("Expected metadata key1='value1', got '%s'", msg.Metadata["key1"])
		}
	case <-time.After(300 * time.Millisecond):
		t.Error("No message received within timeout")
	}
}

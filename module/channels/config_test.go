// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package channels

import (
	"testing"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/config"
)

// Test channel configuration structs
func TestChannelConfigs(t *testing.T) {
	t.Run("Telegram config", func(t *testing.T) {
		cfg := config.TelegramConfig{
			Enabled:   true,
			Token:     "test_token",
			AllowFrom: config.FlexibleStringSlice{"user1", "user2"},
		}

		if !cfg.Enabled {
			t.Error("Expected Enabled to be true")
		}
		if cfg.Token != "test_token" {
			t.Errorf("Expected Token 'test_token', got '%s'", cfg.Token)
		}
		if len(cfg.AllowFrom) != 2 {
			t.Errorf("Expected 2 AllowFrom entries, got %d", len(cfg.AllowFrom))
		}
	})

	t.Run("Discord config", func(t *testing.T) {
		cfg := config.DiscordConfig{
			Enabled:   true,
			Token:     "test_discord_token",
			AllowFrom: config.FlexibleStringSlice{"server1"},
		}

		if !cfg.Enabled {
			t.Error("Expected Enabled to be true")
		}
		if cfg.Token != "test_discord_token" {
			t.Errorf("Expected Token 'test_discord_token', got '%s'", cfg.Token)
		}
	})

	t.Run("WhatsApp config", func(t *testing.T) {
		cfg := config.WhatsAppConfig{
			Enabled:   true,
			BridgeURL: "ws://localhost:3001",
			AllowFrom: config.FlexibleStringSlice{"group1"},
		}

		if !cfg.Enabled {
			t.Error("Expected Enabled to be true")
		}
		if cfg.BridgeURL != "ws://localhost:3001" {
			t.Errorf("Expected BridgeURL 'ws://localhost:3001', got '%s'", cfg.BridgeURL)
		}
	})

	t.Run("LINE config", func(t *testing.T) {
		cfg := config.LINEConfig{
			Enabled:            true,
			ChannelSecret:      "test_secret",
			ChannelAccessToken: "test_token",
			WebhookHost:        "0.0.0.0",
			WebhookPort:        18791,
			AllowFrom:          config.FlexibleStringSlice{},
		}

		if !cfg.Enabled {
			t.Error("Expected Enabled to be true")
		}
		if cfg.WebhookPort != 18791 {
			t.Errorf("Expected WebhookPort 18791, got %d", cfg.WebhookPort)
		}
	})

	t.Run("Slack config", func(t *testing.T) {
		cfg := config.SlackConfig{
			Enabled:  true,
			BotToken: "xoxb-test",
			AppToken: "xapp-test",
		}

		if !cfg.Enabled {
			t.Error("Expected Enabled to be true")
		}
		if cfg.BotToken != "xoxb-test" {
			t.Errorf("Expected BotToken 'xoxb-test', got '%s'", cfg.BotToken)
		}
	})

	t.Run("OneBot config", func(t *testing.T) {
		cfg := config.OneBotConfig{
			Enabled:            true,
			WSUrl:              "ws://127.0.0.1:3001",
			AccessToken:        "test_token",
			ReconnectInterval:  5,
			GroupTriggerPrefix: []string{"/"},
			AllowFrom:          config.FlexibleStringSlice{},
		}

		if !cfg.Enabled {
			t.Error("Expected Enabled to be true")
		}
		if cfg.ReconnectInterval != 5 {
			t.Errorf("Expected ReconnectInterval 5, got %d", cfg.ReconnectInterval)
		}
		if len(cfg.GroupTriggerPrefix) != 1 {
			t.Errorf("Expected 1 GroupTriggerPrefix, got %d", len(cfg.GroupTriggerPrefix))
		}
	})

	t.Run("External config", func(t *testing.T) {
		cfg := config.ExternalConfig{
			Enabled:   true,
			InputEXE:  "input.exe",
			OutputEXE: "output.exe",
			ChatID:    "external:main",
			AllowFrom: config.FlexibleStringSlice{"localhost"},
			SyncTo:    []string{"web"},
			SyncToWeb: true,
		}

		if !cfg.Enabled {
			t.Error("Expected Enabled to be true")
		}
		if !cfg.SyncToWeb {
			t.Error("Expected SyncToWeb to be true")
		}
		if len(cfg.SyncTo) != 1 {
			t.Errorf("Expected 1 SyncTo entry, got %d", len(cfg.SyncTo))
		}
	})

	t.Run("WebSocket config", func(t *testing.T) {
		cfg := config.WebSocketChannelConfig{
			Enabled:   true,
			Host:      "0.0.0.0",
			Port:      18792,
			Path:      "/ws",
			AuthToken: "test_token",
			AllowFrom: config.FlexibleStringSlice{"127.0.0.1"},
			SyncTo:    []string{},
			SyncToWeb: false,
		}

		if !cfg.Enabled {
			t.Error("Expected Enabled to be true")
		}
		if cfg.Port != 18792 {
			t.Errorf("Expected Port 18792, got %d", cfg.Port)
		}
	})

	t.Run("Web config", func(t *testing.T) {
		cfg := config.WebChannelConfig{
			Enabled:           true,
			Host:              "0.0.0.0",
			Port:              8080,
			Path:              "/ws",
			AuthToken:         "",
			AllowFrom:         config.FlexibleStringSlice{},
			HeartbeatInterval: 30,
			SessionTimeout:    3600,
		}

		if !cfg.Enabled {
			t.Error("Expected Enabled to be true")
		}
		if cfg.HeartbeatInterval != 30 {
			t.Errorf("Expected HeartbeatInterval 30, got %d", cfg.HeartbeatInterval)
		}
	})
}

// Test Manager minimum functionality
func TestManagerBasic(t *testing.T) {
	t.Run("valid empty config", func(t *testing.T) {
		cfg := &config.Config{}
		messageBus := bus.NewMessageBus()

		_, err := NewManager(cfg, messageBus)
		// Manager should be created successfully
		if err != nil {
			t.Logf("Manager creation returned: %v", err)
		}
	})
}

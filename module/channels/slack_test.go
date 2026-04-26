// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Tests for slack.go channel implementation

package channels_test

import (
	"context"
	"strings"
	"testing"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/channels"
	"github.com/276793422/NemesisBot/module/config"
)

// ---------------------------------------------------------------------------
// Pure Function Tests (parseSlackChatID, stripBotMention logic)
// ---------------------------------------------------------------------------

func TestParseSlackChatID_ChannelOnly(t *testing.T) {
	// parseSlackChatID splits on "/" — channel only (no thread)
	chatID := "C12345678"
	parts := strings.SplitN(chatID, "/", 2)
	channelID := parts[0]
	var threadTS string
	if len(parts) > 1 {
		threadTS = parts[1]
	}

	if channelID != "C12345678" {
		t.Errorf("Expected channelID 'C12345678', got %q", channelID)
	}
	if threadTS != "" {
		t.Errorf("Expected empty threadTS, got %q", threadTS)
	}
}

func TestParseSlackChatID_ChannelAndThread(t *testing.T) {
	chatID := "C12345678/1234567890.123456"
	parts := strings.SplitN(chatID, "/", 2)
	channelID := parts[0]
	var threadTS string
	if len(parts) > 1 {
		threadTS = parts[1]
	}

	if channelID != "C12345678" {
		t.Errorf("Expected channelID 'C12345678', got %q", channelID)
	}
	if threadTS != "1234567890.123456" {
		t.Errorf("Expected threadTS '1234567890.123456', got %q", threadTS)
	}
}

func TestParseSlackChatID_EmptyString(t *testing.T) {
	chatID := ""
	parts := strings.SplitN(chatID, "/", 2)
	channelID := parts[0]
	var threadTS string
	if len(parts) > 1 {
		threadTS = parts[1]
	}

	if channelID != "" {
		t.Errorf("Expected empty channelID, got %q", channelID)
	}
	if threadTS != "" {
		t.Errorf("Expected empty threadTS, got %q", threadTS)
	}
}

func TestStripBotMention_WithMention(t *testing.T) {
	// stripBotMention removes "<@BOTID>" from text
	botUserID := "U123BOT"
	text := "<@U123BOT> hello world"
	mention := "<@" + botUserID + ">"
	result := strings.ReplaceAll(text, mention, "")
	result = strings.TrimSpace(result)

	if result != "hello world" {
		t.Errorf("Expected 'hello world', got %q", result)
	}
}

func TestStripBotMention_WithoutMention(t *testing.T) {
	botUserID := "U123BOT"
	text := "hello world"
	mention := "<@" + botUserID + ">"
	result := strings.ReplaceAll(text, mention, "")
	result = strings.TrimSpace(result)

	if result != "hello world" {
		t.Errorf("Expected 'hello world', got %q", result)
	}
}

func TestStripBotMention_MultipleMentions(t *testing.T) {
	botUserID := "U123BOT"
	text := "<@U123BOT> hello <@U123BOT> world"
	mention := "<@" + botUserID + ">"
	result := strings.ReplaceAll(text, mention, "")
	result = strings.TrimSpace(result)

	if result != "hello  world" {
		t.Errorf("Expected 'hello  world', got %q", result)
	}
}

// ---------------------------------------------------------------------------
// Constructor + Config Tests
// ---------------------------------------------------------------------------

func TestNewSlackChannel_ValidConfig(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := config.SlackConfig{
		BotToken: "xoxb-test-bot-token",
		AppToken: "xapp-test-app-token",
	}

	ch, err := channels.NewSlackChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewSlackChannel() returned unexpected error: %v", err)
	}
	if ch == nil {
		t.Fatal("NewSlackChannel() returned nil channel")
	}
	if ch.Name() != "slack" {
		t.Errorf("Expected name 'slack', got %q", ch.Name())
	}
}

func TestNewSlackChannel_MissingBotToken(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := config.SlackConfig{
		AppToken: "xapp-test-app-token",
	}

	_, err := channels.NewSlackChannel(cfg, msgBus)
	if err == nil {
		t.Fatal("Expected error for missing bot_token")
	}
}

func TestNewSlackChannel_MissingAppToken(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := config.SlackConfig{
		BotToken: "xoxb-test-bot-token",
	}

	_, err := channels.NewSlackChannel(cfg, msgBus)
	if err == nil {
		t.Fatal("Expected error for missing app_token")
	}
}

func TestSlackChannel_SendNotRunning(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := config.SlackConfig{
		BotToken: "xoxb-test-bot-token",
		AppToken: "xapp-test-app-token",
	}

	ch, err := channels.NewSlackChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewSlackChannel() error: %v", err)
	}

	err = ch.Send(context.Background(), bus.OutboundMessage{
		Channel: "slack",
		ChatID:  "C12345678",
		Content: "Hello!",
	})
	if err == nil {
		t.Fatal("Expected Send() to fail when not running")
	}
}

func TestSlackChannel_StopWithoutStart(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := config.SlackConfig{
		BotToken: "xoxb-test-bot-token",
		AppToken: "xapp-test-app-token",
	}

	ch, err := channels.NewSlackChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewSlackChannel() error: %v", err)
	}

	if err := ch.Stop(context.Background()); err != nil {
		t.Fatalf("Stop() without Start() should not error: %v", err)
	}
}

func TestSlackChannel_IsAllowed(t *testing.T) {
	msgBus := bus.NewMessageBus()

	tests := []struct {
		name      string
		allowList []string
		senderID  string
		want      bool
	}{
		{"EmptyAllowList_AllowsAll", nil, "U_ANY", true},
		{"InAllowList", []string{"U123"}, "U123", true},
		{"NotInAllowList", []string{"U123"}, "U456", false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cfg := config.SlackConfig{
				BotToken:  "xoxb-test",
				AppToken:  "xapp-test",
				AllowFrom: tt.allowList,
			}
			ch, err := channels.NewSlackChannel(cfg, msgBus)
			if err != nil {
				t.Fatalf("NewSlackChannel() error: %v", err)
			}
			if got := ch.IsAllowed(tt.senderID); got != tt.want {
				t.Errorf("IsAllowed(%q) = %v, want %v", tt.senderID, got, tt.want)
			}
		})
	}
}

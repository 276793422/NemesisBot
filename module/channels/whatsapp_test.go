// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Tests for whatsapp.go channel implementation

package channels_test

import (
	"context"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/channels"
	"github.com/276793422/NemesisBot/module/config"
)

// ---------------------------------------------------------------------------
// Constructor + Config Tests
// ---------------------------------------------------------------------------

func TestNewWhatsAppChannel_ValidConfig(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := config.WhatsAppConfig{
		BridgeURL: "ws://localhost:3001",
	}

	ch, err := channels.NewWhatsAppChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewWhatsAppChannel() returned unexpected error: %v", err)
	}
	if ch == nil {
		t.Fatal("NewWhatsAppChannel() returned nil channel")
	}
	if ch.Name() != "whatsapp" {
		t.Errorf("Expected name 'whatsapp', got %q", ch.Name())
	}
}

func TestNewWhatsAppChannel_DefaultAllowFrom(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := config.WhatsAppConfig{
		BridgeURL: "ws://localhost:3001",
	}

	ch, err := channels.NewWhatsAppChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewWhatsAppChannel() error: %v", err)
	}

	// Empty allowlist should allow everyone
	if !ch.IsAllowed("anyone") {
		t.Error("Expected empty allowlist to allow all senders")
	}
}

func TestWhatsAppChannel_StartFails_InvalidURL(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := config.WhatsAppConfig{
		BridgeURL: "ws://invalid-host-that-does-not-exist.local:9999",
	}

	ch, err := channels.NewWhatsAppChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewWhatsAppChannel() error: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()

	if err := ch.Start(ctx); err == nil {
		t.Fatal("Expected Start() to fail with invalid URL")
		ch.Stop(ctx)
	}
}

func TestWhatsAppChannel_StopWithoutStart(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := config.WhatsAppConfig{
		BridgeURL: "ws://localhost:3001",
	}

	ch, err := channels.NewWhatsAppChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewWhatsAppChannel() error: %v", err)
	}

	if err := ch.Stop(context.Background()); err != nil {
		t.Fatalf("Stop() without Start() should not error: %v", err)
	}
}

func TestWhatsAppChannel_SendNotRunning(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := config.WhatsAppConfig{
		BridgeURL: "ws://localhost:3001",
	}

	ch, err := channels.NewWhatsAppChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewWhatsAppChannel() error: %v", err)
	}

	err = ch.Send(context.Background(), bus.OutboundMessage{
		Channel: "whatsapp",
		ChatID:  "1234567890",
		Content: "Hello!",
	})
	if err == nil {
		t.Fatal("Expected Send() to fail when not running")
	}
}

func TestWhatsAppChannel_IsAllowed(t *testing.T) {
	msgBus := bus.NewMessageBus()

	tests := []struct {
		name      string
		allowList []string
		senderID  string
		want      bool
	}{
		{"EmptyAllowList_AllowsAll", nil, "1234567890", true},
		{"InAllowList", []string{"1111111111"}, "1111111111", true},
		{"NotInAllowList", []string{"1111111111"}, "2222222222", false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cfg := config.WhatsAppConfig{
				BridgeURL: "ws://localhost:3001",
				AllowFrom: tt.allowList,
			}
			ch, err := channels.NewWhatsAppChannel(cfg, msgBus)
			if err != nil {
				t.Fatalf("NewWhatsAppChannel() error: %v", err)
			}
			if got := ch.IsAllowed(tt.senderID); got != tt.want {
				t.Errorf("IsAllowed(%q) = %v, want %v", tt.senderID, got, tt.want)
			}
		})
	}
}

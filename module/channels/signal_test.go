// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Tests for signal.go channel implementation

package channels_test

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/channels"
)

// ---------------------------------------------------------------------------
// Constructor + Config Tests
// ---------------------------------------------------------------------------

func TestNewSignalChannel_ValidConfig(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.SignalConfig{
		APIURL:      "http://localhost:8080",
		PhoneNumber: "+1234567890",
	}

	ch, err := channels.NewSignalChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewSignalChannel() returned unexpected error: %v", err)
	}
	if ch == nil {
		t.Fatal("NewSignalChannel() returned nil channel")
	}
	if ch.Name() != "signal" {
		t.Errorf("Expected name 'signal', got %q", ch.Name())
	}
}

func TestNewSignalChannel_MissingAPIURL(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.SignalConfig{
		PhoneNumber: "+1234567890",
	}

	_, err := channels.NewSignalChannel(cfg, msgBus)
	if err == nil {
		t.Fatal("Expected error for missing api_url")
	}
}

func TestNewSignalChannel_MissingPhoneNumber(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.SignalConfig{
		APIURL: "http://localhost:8080",
	}

	_, err := channels.NewSignalChannel(cfg, msgBus)
	if err == nil {
		t.Fatal("Expected error for missing phone_number")
	}
}

func TestNewSignalChannel_DefaultPollInterval(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.SignalConfig{
		APIURL:      "http://localhost:8080",
		PhoneNumber: "+1234567890",
	}

	ch, err := channels.NewSignalChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewSignalChannel() error: %v", err)
	}
	_ = ch // Poll interval is internal; just verify construction succeeds
}

func TestNewSignalChannel_CustomChannelName(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.SignalConfig{
		APIURL:       "http://localhost:8080",
		PhoneNumber:  "+1234567890",
		ChannelName:  "custom-signal",
	}

	ch, err := channels.NewSignalChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewSignalChannel() error: %v", err)
	}
	if ch.Name() != "custom-signal" {
		t.Errorf("Expected name 'custom-signal', got %q", ch.Name())
	}
}

func TestNewSignalChannel_TrailingSlash(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.SignalConfig{
		APIURL:      "http://localhost:8080/",
		PhoneNumber: "+1234567890",
	}

	ch, err := channels.NewSignalChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewSignalChannel() error: %v", err)
	}
	_ = ch
}

// ---------------------------------------------------------------------------
// HTTP Mock Tests
// ---------------------------------------------------------------------------

func newSignalMockServer() *httptest.Server {
	mux := http.NewServeMux()

	// GET /v1/about
	mux.HandleFunc("/v1/about", func(w http.ResponseWriter, r *http.Request) {
		resp := map[string]interface{}{
			"versions": []string{"v1", "v2"},
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(resp)
	})

	// GET /v1/receive/{phone}
	mux.HandleFunc("/v1/receive/", func(w http.ResponseWriter, r *http.Request) {
		// Return empty array (no messages)
		w.Header().Set("Content-Type", "application/json")
		w.Write([]byte("[]"))
	})

	// POST /v2/send
	mux.HandleFunc("/v2/send", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
			return
		}

		var reqBody map[string]interface{}
		if err := json.NewDecoder(r.Body).Decode(&reqBody); err != nil {
			http.Error(w, "bad request", http.StatusBadRequest)
			return
		}

		resp := map[string]interface{}{
			"timestamp": time.Now().Unix(),
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(resp)
	})

	return httptest.NewServer(mux)
}

func newSignalFailingServer() *httptest.Server {
	return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		http.Error(w, "service unavailable", http.StatusServiceUnavailable)
	}))
}

func TestSignalChannel_StartWithMockServer(t *testing.T) {
	server := newSignalMockServer()
	defer server.Close()

	msgBus := bus.NewMessageBus()
	cfg := channels.SignalConfig{
		APIURL:       server.URL,
		PhoneNumber:  "+1234567890",
		PollInterval: 3600,
	}

	ch, err := channels.NewSignalChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewSignalChannel() error: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := ch.Start(ctx); err != nil {
		t.Fatalf("Start() error: %v", err)
	}
	if !ch.IsRunning() {
		t.Fatal("Expected channel to be running after Start()")
	}

	ch.Stop(ctx)
}

func TestSignalChannel_StartFails_APIUnavailable(t *testing.T) {
	server := newSignalFailingServer()
	defer server.Close()

	msgBus := bus.NewMessageBus()
	cfg := channels.SignalConfig{
		APIURL:       server.URL,
		PhoneNumber:  "+1234567890",
		PollInterval: 3600,
	}

	ch, err := channels.NewSignalChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewSignalChannel() error: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := ch.Start(ctx); err == nil {
		t.Fatal("Expected Start() to fail when API is unavailable")
		ch.Stop(ctx)
	}
}

func TestSignalChannel_StopWithoutStart(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.SignalConfig{
		APIURL:      "http://localhost:8080",
		PhoneNumber: "+1234567890",
	}

	ch, err := channels.NewSignalChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewSignalChannel() error: %v", err)
	}

	if err := ch.Stop(context.Background()); err != nil {
		t.Fatalf("Stop() without Start() should not error: %v", err)
	}
}

func TestSignalChannel_SendNotRunning(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.SignalConfig{
		APIURL:      "http://localhost:8080",
		PhoneNumber: "+1234567890",
	}

	ch, err := channels.NewSignalChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewSignalChannel() error: %v", err)
	}

	err = ch.Send(context.Background(), bus.OutboundMessage{
		Channel: "signal",
		ChatID:  "+0987654321",
		Content: "Hello!",
	})
	if err == nil {
		t.Fatal("Expected Send() to fail when not running")
	}
}

func TestSignalChannel_SendWithMockServer(t *testing.T) {
	server := newSignalMockServer()
	defer server.Close()

	msgBus := bus.NewMessageBus()
	cfg := channels.SignalConfig{
		APIURL:       server.URL,
		PhoneNumber:  "+1234567890",
		PollInterval: 3600,
	}

	ch, err := channels.NewSignalChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewSignalChannel() error: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := ch.Start(ctx); err != nil {
		t.Fatalf("Start() error: %v", err)
	}
	defer ch.Stop(ctx)

	// Individual message
	err = ch.Send(ctx, bus.OutboundMessage{
		Channel: "signal",
		ChatID:  "+0987654321",
		Content: "Hello Signal!",
	})
	if err != nil {
		t.Fatalf("Send() error: %v", err)
	}
}

func TestSignalChannel_Send_GroupMessage(t *testing.T) {
	server := newSignalMockServer()
	defer server.Close()

	msgBus := bus.NewMessageBus()
	cfg := channels.SignalConfig{
		APIURL:       server.URL,
		PhoneNumber:  "+1234567890",
		PollInterval: 3600,
	}

	ch, err := channels.NewSignalChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewSignalChannel() error: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := ch.Start(ctx); err != nil {
		t.Fatalf("Start() error: %v", err)
	}
	defer ch.Stop(ctx)

	// Group message (group: prefix)
	err = ch.Send(ctx, bus.OutboundMessage{
		Channel: "signal",
		ChatID:  "group:test-group-id",
		Content: "Hello group!",
	})
	if err != nil {
		t.Fatalf("Send() group message error: %v", err)
	}
}

func TestSignalChannel_Send_EmptyChatID(t *testing.T) {
	server := newSignalMockServer()
	defer server.Close()

	msgBus := bus.NewMessageBus()
	cfg := channels.SignalConfig{
		APIURL:       server.URL,
		PhoneNumber:  "+1234567890",
		PollInterval: 3600,
	}

	ch, err := channels.NewSignalChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewSignalChannel() error: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := ch.Start(ctx); err != nil {
		t.Fatalf("Start() error: %v", err)
	}
	defer ch.Stop(ctx)

	err = ch.Send(ctx, bus.OutboundMessage{
		Channel: "signal",
		ChatID:  "",
		Content: "Hello!",
	})
	if err == nil {
		t.Fatal("Expected Send() to fail with empty chat_id")
	}
}

func TestSignalChannel_IsAllowed(t *testing.T) {
	msgBus := bus.NewMessageBus()

	tests := []struct {
		name      string
		allowList []string
		senderID  string
		want      bool
	}{
		{"EmptyAllowList_AllowsAll", nil, "+1234567890", true},
		{"InAllowList", []string{"+1111111111"}, "+1111111111", true},
		{"NotInAllowList", []string{"+1111111111"}, "+2222222222", false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cfg := channels.SignalConfig{
				APIURL:      "http://localhost:8080",
				PhoneNumber: "+1234567890",
				AllowFrom:   tt.allowList,
			}
			ch, err := channels.NewSignalChannel(cfg, msgBus)
			if err != nil {
				t.Fatalf("NewSignalChannel() error: %v", err)
			}
			if got := ch.IsAllowed(tt.senderID); got != tt.want {
				t.Errorf("IsAllowed(%q) = %v, want %v", tt.senderID, got, tt.want)
			}
		})
	}
}

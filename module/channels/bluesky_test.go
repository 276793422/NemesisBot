// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Tests for bluesky.go channel implementation

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

func TestNewBlueskyChannel_ValidConfig(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.BlueskyConfig{
		Server:   "https://bsky.social",
		Handle:   "test.bsky.social",
		Password: "test-password",
	}

	ch, err := channels.NewBlueskyChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewBlueskyChannel() returned unexpected error: %v", err)
	}
	if ch == nil {
		t.Fatal("NewBlueskyChannel() returned nil channel")
	}
	if ch.Name() != "bluesky" {
		t.Errorf("Expected name 'bluesky', got %q", ch.Name())
	}
}

func TestNewBlueskyChannel_MissingHandle(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.BlueskyConfig{
		Server:   "https://bsky.social",
		Password: "test-password",
	}

	_, err := channels.NewBlueskyChannel(cfg, msgBus)
	if err == nil {
		t.Fatal("Expected error for missing handle")
	}
}

func TestNewBlueskyChannel_MissingPassword(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.BlueskyConfig{
		Server: "https://bsky.social",
		Handle: "test.bsky.social",
	}

	_, err := channels.NewBlueskyChannel(cfg, msgBus)
	if err == nil {
		t.Fatal("Expected error for missing password")
	}
}

func TestNewBlueskyChannel_MissingServer(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.BlueskyConfig{
		Handle:   "test.bsky.social",
		Password: "test-password",
	}

	_, err := channels.NewBlueskyChannel(cfg, msgBus)
	if err == nil {
		t.Fatal("Expected error for missing server")
	}
}

func TestNewBlueskyChannel_DefaultPollInterval(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.BlueskyConfig{
		Server:   "https://bsky.social",
		Handle:   "test.bsky.social",
		Password: "test-password",
	}

	ch, err := channels.NewBlueskyChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewBlueskyChannel() returned unexpected error: %v", err)
	}
	_ = ch // Poll interval is internal; just verify construction succeeds
}

func TestNewBlueskyChannel_CustomChannelName(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.BlueskyConfig{
		Server:      "https://bsky.social",
		Handle:      "test.bsky.social",
		Password:    "test-password",
		ChannelName: "custom-bluesky",
	}

	ch, err := channels.NewBlueskyChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewBlueskyChannel() returned unexpected error: %v", err)
	}
	if ch.Name() != "custom-bluesky" {
		t.Errorf("Expected name 'custom-bluesky', got %q", ch.Name())
	}
}

func TestNewBlueskyChannel_ServerTrailingSlash(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.BlueskyConfig{
		Server:   "https://bsky.social/",
		Handle:   "test.bsky.social",
		Password: "test-password",
	}

	ch, err := channels.NewBlueskyChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewBlueskyChannel() returned unexpected error: %v", err)
	}
	_ = ch
}

// ---------------------------------------------------------------------------
// HTTP Mock Tests
// ---------------------------------------------------------------------------

// newBlueskyMockServer creates an httptest.Server that simulates the Bluesky AT Protocol API.
func newBlueskyMockServer() *httptest.Server {
	mux := http.NewServeMux()

	// POST /xrpc/com.atproto.server.createSession
	mux.HandleFunc("/xrpc/com.atproto.server.createSession", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
			return
		}

		var reqBody map[string]string
		if err := json.NewDecoder(r.Body).Decode(&reqBody); err != nil {
			http.Error(w, "bad request", http.StatusBadRequest)
			return
		}

		if reqBody["identifier"] == "" || reqBody["password"] == "" {
			http.Error(w, `{"error":"InvalidRequest"}`, http.StatusBadRequest)
			return
		}

		// Simulate invalid credentials
		if reqBody["password"] == "wrong-password" {
			http.Error(w, `{"error":"AuthenticationRequired"}`, http.StatusUnauthorized)
			return
		}

		resp := map[string]interface{}{
			"did":        "did:plc:test123",
			"handle":     "test.bsky.social",
			"accessJwt":  "test-access-token",
			"refreshJwt": "test-refresh-token",
			"active":     true,
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(resp)
	})

	// GET /xrpc/app.bsky.notification.listNotifications
	mux.HandleFunc("/xrpc/app.bsky.notification.listNotifications", func(w http.ResponseWriter, r *http.Request) {
		auth := r.Header.Get("Authorization")
		if auth != "Bearer test-access-token" {
			http.Error(w, "unauthorized", http.StatusUnauthorized)
			return
		}

		resp := map[string]interface{}{
			"notifications": []interface{}{},
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(resp)
	})

	// POST /xrpc/app.bsky.notification.updateSeen
	mux.HandleFunc("/xrpc/app.bsky.notification.updateSeen", func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
	})

	// GET /xrpc/com.atproto.repo.getRecord
	mux.HandleFunc("/xrpc/com.atproto.repo.getRecord", func(w http.ResponseWriter, r *http.Request) {
		auth := r.Header.Get("Authorization")
		if auth != "Bearer test-access-token" {
			http.Error(w, "unauthorized", http.StatusUnauthorized)
			return
		}

		rkey := r.URL.Query().Get("rkey")
		if rkey == "fail-cid" {
			http.Error(w, "record not found", http.StatusNotFound)
			return
		}

		resp := map[string]interface{}{
			"uri":   "at://did:plc:test123/app.bsky.feed.post/" + rkey,
			"cid":   "bafyrei-test-cid",
			"value": map[string]string{"text": "test post"},
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(resp)
	})

	// POST /xrpc/com.atproto.repo.createRecord
	mux.HandleFunc("/xrpc/com.atproto.repo.createRecord", func(w http.ResponseWriter, r *http.Request) {
		auth := r.Header.Get("Authorization")
		if auth != "Bearer test-access-token" {
			http.Error(w, "unauthorized", http.StatusUnauthorized)
			return
		}

		resp := map[string]interface{}{
			"uri": "at://did:plc:test123/app.bsky.feed.post/new-post-123",
			"cid": "bafyrei-new-cid",
		}
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusCreated)
		json.NewEncoder(w).Encode(resp)
	})

	return httptest.NewServer(mux)
}

func TestBlueskyChannel_StartWithMockServer(t *testing.T) {
	server := newBlueskyMockServer()
	defer server.Close()

	msgBus := bus.NewMessageBus()
	cfg := channels.BlueskyConfig{
		Server:      server.URL,
		Handle:      "test.bsky.social",
		Password:    "test-password",
		PollInterval: 3600, // long interval to avoid actual polling
	}

	ch, err := channels.NewBlueskyChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewBlueskyChannel() error: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := ch.Start(ctx); err != nil {
		t.Fatalf("Start() error: %v", err)
	}
	if !ch.IsRunning() {
		t.Fatal("Expected channel to be running after Start()")
	}

	if err := ch.Stop(ctx); err != nil {
		t.Fatalf("Stop() error: %v", err)
	}
}

func TestBlueskyChannel_StartFails_InvalidCredentials(t *testing.T) {
	server := newBlueskyMockServer()
	defer server.Close()

	msgBus := bus.NewMessageBus()
	cfg := channels.BlueskyConfig{
		Server:      server.URL,
		Handle:      "test.bsky.social",
		Password:    "wrong-password",
		PollInterval: 3600,
	}

	ch, err := channels.NewBlueskyChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewBlueskyChannel() error: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := ch.Start(ctx); err == nil {
		t.Fatal("Expected Start() to fail with invalid credentials")
		ch.Stop(ctx)
	}
}

func TestBlueskyChannel_StopWithoutStart(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.BlueskyConfig{
		Server:   "https://bsky.social",
		Handle:   "test.bsky.social",
		Password: "test-password",
	}

	ch, err := channels.NewBlueskyChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewBlueskyChannel() error: %v", err)
	}

	ctx := context.Background()
	if err := ch.Stop(ctx); err != nil {
		t.Fatalf("Stop() without Start() should not error: %v", err)
	}
}

func TestBlueskyChannel_SendNotRunning(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.BlueskyConfig{
		Server:   "https://bsky.social",
		Handle:   "test.bsky.social",
		Password: "test-password",
	}

	ch, err := channels.NewBlueskyChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewBlueskyChannel() error: %v", err)
	}

	ctx := context.Background()
	err = ch.Send(ctx, bus.OutboundMessage{
		Channel: "bluesky",
		ChatID:  "at://did:plc:test123/app.bsky.feed.post/test-post",
		Content: "Hello!",
	})
	if err == nil {
		t.Fatal("Expected Send() to fail when not running")
	}
}

func TestBlueskyChannel_SendWithMockServer(t *testing.T) {
	server := newBlueskyMockServer()
	defer server.Close()

	msgBus := bus.NewMessageBus()
	cfg := channels.BlueskyConfig{
		Server:      server.URL,
		Handle:      "test.bsky.social",
		Password:    "test-password",
		PollInterval: 3600,
	}

	ch, err := channels.NewBlueskyChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewBlueskyChannel() error: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := ch.Start(ctx); err != nil {
		t.Fatalf("Start() error: %v", err)
	}
	defer ch.Stop(ctx)

	err = ch.Send(ctx, bus.OutboundMessage{
		Channel: "bluesky",
		ChatID:  "at://did:plc:test123/app.bsky.feed.post/test-post-123",
		Content: "Hello reply!",
	})
	if err != nil {
		t.Fatalf("Send() error: %v", err)
	}
}

func TestBlueskyChannel_Send_ResolveCIDFails(t *testing.T) {
	server := newBlueskyMockServer()
	defer server.Close()

	msgBus := bus.NewMessageBus()
	cfg := channels.BlueskyConfig{
		Server:      server.URL,
		Handle:      "test.bsky.social",
		Password:    "test-password",
		PollInterval: 3600,
	}

	ch, err := channels.NewBlueskyChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewBlueskyChannel() error: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := ch.Start(ctx); err != nil {
		t.Fatalf("Start() error: %v", err)
	}
	defer ch.Stop(ctx)

	err = ch.Send(ctx, bus.OutboundMessage{
		Channel: "bluesky",
		ChatID:  "at://did:plc:test123/app.bsky.feed.post/fail-cid",
		Content: "This should fail",
	})
	if err == nil {
		t.Fatal("Expected Send() to fail when getRecord returns 404")
	}
}

func TestBlueskyChannel_Send_EmptyChatID(t *testing.T) {
	server := newBlueskyMockServer()
	defer server.Close()

	msgBus := bus.NewMessageBus()
	cfg := channels.BlueskyConfig{
		Server:      server.URL,
		Handle:      "test.bsky.social",
		Password:    "test-password",
		PollInterval: 3600,
	}

	ch, err := channels.NewBlueskyChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewBlueskyChannel() error: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := ch.Start(ctx); err != nil {
		t.Fatalf("Start() error: %v", err)
	}
	defer ch.Stop(ctx)

	err = ch.Send(ctx, bus.OutboundMessage{
		Channel: "bluesky",
		ChatID:  "",
		Content: "Hello!",
	})
	if err == nil {
		t.Fatal("Expected Send() to fail with empty chat_id")
	}
}

func TestBlueskyChannel_IsAllowed(t *testing.T) {
	msgBus := bus.NewMessageBus()

	tests := []struct {
		name      string
		allowList []string
		senderID  string
		want      bool
	}{
		{"EmptyAllowList_AllowsAll", nil, "anyone.bsky.social", true},
		{"InAllowList", []string{"alice.bsky.social"}, "alice.bsky.social", true},
		{"NotInAllowList", []string{"alice.bsky.social"}, "bob.bsky.social", false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cfg := channels.BlueskyConfig{
				Server:      "https://bsky.social",
				Handle:      "test.bsky.social",
				Password:    "test-password",
				AllowFrom:   tt.allowList,
			}
			ch, err := channels.NewBlueskyChannel(cfg, msgBus)
			if err != nil {
				t.Fatalf("NewBlueskyChannel() error: %v", err)
			}
			if got := ch.IsAllowed(tt.senderID); got != tt.want {
				t.Errorf("IsAllowed(%q) = %v, want %v", tt.senderID, got, tt.want)
			}
		})
	}
}

func TestBlueskyChannel_PollReceivesNotification(t *testing.T) {
	msgBus := bus.NewMessageBus()

	// Create a mock server that returns a mention notification
	mux := http.NewServeMux()
	mux.HandleFunc("/xrpc/com.atproto.server.createSession", func(w http.ResponseWriter, r *http.Request) {
		resp := map[string]interface{}{
			"did":       "did:plc:testbot",
			"handle":    "testbot.bsky.social",
			"accessJwt": "test-token",
			"active":    true,
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(resp)
	})

	mux.HandleFunc("/xrpc/app.bsky.notification.listNotifications", func(w http.ResponseWriter, r *http.Request) {
		notifs := []map[string]interface{}{
			{
				"id":     "notif-mention-1",
				"reason": "mention",
				"author": map[string]string{
					"did":    "did:plc:alice",
					"handle": "alice.bsky.social",
				},
				"record": map[string]string{
					"$type":     "app.bsky.feed.post",
					"text":      "Hello @testbot.bsky.social!",
					"createdAt": "2026-01-01T00:00:00Z",
				},
				"isRead":     false,
				"indexedAt":  "2026-01-01T00:00:00Z",
			},
		}
		resp := map[string]interface{}{
			"notifications": notifs,
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(resp)
	})

	mux.HandleFunc("/xrpc/app.bsky.notification.updateSeen", func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
	})

	server := httptest.NewServer(mux)
	defer server.Close()

	cfg := channels.BlueskyConfig{
		Server:      server.URL,
		Handle:      "testbot.bsky.social",
		Password:    "test-password",
		PollInterval: 3600,
	}

	ch, err := channels.NewBlueskyChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewBlueskyChannel() error: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	if err := ch.Start(ctx); err != nil {
		t.Fatalf("Start() error: %v", err)
	}

	// Wait for the initial poll to process
	msg, ok := msgBus.ConsumeInbound(ctx)
	if !ok {
		t.Fatal("Expected to receive an inbound message from notification poll")
	}

	if msg.Channel != "bluesky" {
		t.Errorf("Expected channel 'bluesky', got %q", msg.Channel)
	}
	if msg.Content != "Hello @testbot.bsky.social!" {
		t.Errorf("Unexpected content: %q", msg.Content)
	}
	if msg.SenderID != "alice.bsky.social" {
		t.Errorf("Expected sender 'alice.bsky.social', got %q", msg.SenderID)
	}

	ch.Stop(ctx)
}

func TestBlueskyChannel_IgnoresNonMentionNotifs(t *testing.T) {
	msgBus := bus.NewMessageBus()

	mux := http.NewServeMux()
	mux.HandleFunc("/xrpc/com.atproto.server.createSession", func(w http.ResponseWriter, r *http.Request) {
		resp := map[string]interface{}{
			"did":       "did:plc:testbot",
			"handle":    "testbot.bsky.social",
			"accessJwt": "test-token",
			"active":    true,
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(resp)
	})

	mux.HandleFunc("/xrpc/app.bsky.notification.listNotifications", func(w http.ResponseWriter, r *http.Request) {
		// Only non-mention notifications (like, follow)
		notifs := []map[string]interface{}{
			{
				"id":     "notif-like-1",
				"reason": "like",
				"author": map[string]string{
					"did":    "did:plc:alice",
					"handle": "alice.bsky.social",
				},
				"record": map[string]string{
					"$type":     "app.bsky.feed.like",
					"createdAt": "2026-01-01T00:00:00Z",
				},
				"isRead":    false,
				"indexedAt": "2026-01-01T00:00:00Z",
			},
			{
				"id":     "notif-follow-1",
				"reason": "follow",
				"author": map[string]string{
					"did":    "did:plc:bob",
					"handle": "bob.bsky.social",
				},
				"record": map[string]string{
					"$type":     "app.bsky.graph.follow",
					"createdAt": "2026-01-01T00:00:00Z",
				},
				"isRead":    false,
				"indexedAt": "2026-01-01T00:00:00Z",
			},
		}
		resp := map[string]interface{}{
			"notifications": notifs,
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(resp)
	})

	mux.HandleFunc("/xrpc/app.bsky.notification.updateSeen", func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
	})

	server := httptest.NewServer(mux)
	defer server.Close()

	cfg := channels.BlueskyConfig{
		Server:      server.URL,
		Handle:      "testbot.bsky.social",
		Password:    "test-password",
		PollInterval: 3600,
	}

	ch, err := channels.NewBlueskyChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewBlueskyChannel() error: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()

	if err := ch.Start(ctx); err != nil {
		t.Fatalf("Start() error: %v", err)
	}

	// Give time for poll, then verify no messages on bus
	time.Sleep(500 * time.Millisecond)

	// Non-blocking check: no inbound messages expected
	_, ok := msgBus.ConsumeInbound(ctx)
	if ok {
		t.Error("Expected no inbound messages for non-mention notifications")
	}

	ch.Stop(ctx)
}

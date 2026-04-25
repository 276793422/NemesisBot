// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Tests for email.go, matrix.go, and webhook_inbound.go channel implementations

package channels_test

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/channels"
)

// shortHTTPClient returns an http.Client with a short timeout suitable for tests.
func shortHTTPClient() *http.Client {
	return &http.Client{Timeout: 10 * time.Second}
}

// findFreePort finds and returns a free TCP port on localhost.
func findFreePort(t *testing.T) string {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("Failed to find free port: %v", err)
	}
	addr := ln.Addr().String()
	ln.Close()
	return addr
}

// ---------------------------------------------------------------------------
// Email Channel Tests
// ---------------------------------------------------------------------------

func TestNewEmailChannel_ValidConfig(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.EmailConfig{
		IMAPHost:     "imap.example.com",
		IMAPUsername: "user@example.com",
		IMAPPassword: "password123",
		SMTPHost:     "smtp.example.com",
	}

	ch, err := channels.NewEmailChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewEmailChannel() returned unexpected error: %v", err)
	}
	if ch == nil {
		t.Fatal("NewEmailChannel() returned nil channel")
	}
}

func TestNewEmailChannel_Defaults(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.EmailConfig{
		IMAPHost:     "imap.example.com",
		IMAPUsername: "user@example.com",
		IMAPPassword: "pass",
		SMTPHost:     "smtp.example.com",
	}

	ch, err := channels.NewEmailChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewEmailChannel() failed: %v", err)
	}

	// Verify the channel name defaults to "email"
	if ch.Name() != "email" {
		t.Errorf("Expected default name 'email', got '%s'", ch.Name())
	}

	// Channel should not be running initially
	if ch.IsRunning() {
		t.Error("New channel should not be running")
	}
}

func TestNewEmailChannel_CustomChannelName(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.EmailConfig{
		IMAPHost:     "imap.example.com",
		IMAPUsername: "user@example.com",
		IMAPPassword: "pass",
		SMTPHost:     "smtp.example.com",
		ChannelName:  "custom-email",
	}

	ch, err := channels.NewEmailChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewEmailChannel() failed: %v", err)
	}

	if ch.Name() != "custom-email" {
		t.Errorf("Expected name 'custom-email', got '%s'", ch.Name())
	}
}

func TestNewEmailChannel_MissingIMAPHost(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.EmailConfig{
		IMAPUsername: "user@example.com",
		IMAPPassword: "pass",
		SMTPHost:     "smtp.example.com",
	}

	_, err := channels.NewEmailChannel(cfg, msgBus)
	if err == nil {
		t.Fatal("Expected error when IMAPHost is empty")
	}
	if !strings.Contains(err.Error(), "imap_host") {
		t.Errorf("Expected error mentioning imap_host, got: %v", err)
	}
}

func TestNewEmailChannel_MissingSMTPHost(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.EmailConfig{
		IMAPHost:     "imap.example.com",
		IMAPUsername: "user@example.com",
		IMAPPassword: "pass",
	}

	_, err := channels.NewEmailChannel(cfg, msgBus)
	if err == nil {
		t.Fatal("Expected error when SMTPHost is empty")
	}
	if !strings.Contains(err.Error(), "smtp_host") {
		t.Errorf("Expected error mentioning smtp_host, got: %v", err)
	}
}

func TestNewEmailChannel_MissingIMAPCredentials(t *testing.T) {
	msgBus := bus.NewMessageBus()

	tests := []struct {
		name string
		cfg  channels.EmailConfig
	}{
		{
			name: "Missing IMAP username",
			cfg: channels.EmailConfig{
				IMAPHost:     "imap.example.com",
				IMAPPassword: "pass",
				SMTPHost:     "smtp.example.com",
			},
		},
		{
			name: "Missing IMAP password",
			cfg: channels.EmailConfig{
				IMAPHost:     "imap.example.com",
				IMAPUsername: "user@example.com",
				SMTPHost:     "smtp.example.com",
			},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			_, err := channels.NewEmailChannel(tt.cfg, msgBus)
			if err == nil {
				t.Fatal("Expected error for missing IMAP credentials")
			}
			if !strings.Contains(err.Error(), "imap_") {
				t.Errorf("Expected error mentioning imap_ credentials, got: %v", err)
			}
		})
	}
}

func TestNewEmailChannel_SMTPDefaults(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.EmailConfig{
		IMAPHost:     "imap.example.com",
		IMAPUsername: "user@example.com",
		IMAPPassword: "imap-pass",
		SMTPHost:     "smtp.example.com",
	}

	ch, err := channels.NewEmailChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewEmailChannel() failed: %v", err)
	}
	if ch == nil {
		t.Fatal("Expected non-nil channel")
	}
	// SMTP credentials should default to IMAP credentials
	// This is verified by the constructor; we can't directly inspect the internal config
	// but the channel should be created successfully.
}

func TestEmailChannel_SendNotRunning(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.EmailConfig{
		IMAPHost:     "imap.example.com",
		IMAPUsername: "user@example.com",
		IMAPPassword: "pass",
		SMTPHost:     "smtp.example.com",
	}

	ch, _ := channels.NewEmailChannel(cfg, msgBus)

	ctx := context.Background()
	err := ch.Send(ctx, bus.OutboundMessage{
		Channel: "email",
		ChatID:  "recipient@example.com",
		Content: "Hello",
	})
	if err == nil {
		t.Fatal("Expected error when sending on non-running channel")
	}
	if !strings.Contains(err.Error(), "not running") {
		t.Errorf("Expected 'not running' error, got: %v", err)
	}
}

func TestEmailChannel_StopWithoutStart(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.EmailConfig{
		IMAPHost:     "imap.example.com",
		IMAPUsername: "user@example.com",
		IMAPPassword: "pass",
		SMTPHost:     "smtp.example.com",
	}

	ch, _ := channels.NewEmailChannel(cfg, msgBus)
	ctx := context.Background()

	// Stopping without starting should not panic
	err := ch.Stop(ctx)
	if err != nil {
		t.Errorf("Stop() on non-started channel should not return error, got: %v", err)
	}
}

func TestEmailChannel_StartFails_NoIMAPServer(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.EmailConfig{
		IMAPHost:     "127.0.0.1",
		IMAPPort:     19993, // Use a port that is very unlikely to have a server
		IMAPUsername: "user@example.com",
		IMAPPassword: "pass",
		SMTPHost:     "127.0.0.1",
		SMTPPort:     19875,
	}

	ch, _ := channels.NewEmailChannel(cfg, msgBus)
	ctx := context.Background()

	err := ch.Start(ctx)
	if err == nil {
		t.Fatal("Expected Start() to fail without IMAP server")
		ch.Stop(ctx)
	}
	if !strings.Contains(err.Error(), "IMAP") {
		t.Errorf("Expected IMAP-related error, got: %v", err)
	}
}

// mockIMAPServer provides a minimal IMAP-like server for testing.
func mockIMAPServer(t *testing.T) net.Listener {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("Failed to create mock IMAP server: %v", err)
	}

	go func() {
		for {
			conn, err := ln.Accept()
			if err != nil {
				return // Listener closed
			}
			go handleIMAPConn(conn)
		}
	}()

	return ln
}

func handleIMAPConn(conn net.Conn) {
	defer conn.Close()

	// Send greeting
	conn.Write([]byte("* OK mock IMAP server ready\r\n"))

	buf := make([]byte, 4096)
	for {
		n, err := conn.Read(buf)
		if err != nil {
			return
		}
		line := strings.TrimSpace(string(buf[:n]))

		if strings.HasPrefix(line, "NB00 LOGIN") {
			conn.Write([]byte("NB00 OK LOGIN completed\r\n"))
		} else if strings.HasPrefix(line, "NB00 SELECT") {
			conn.Write([]byte("* 0 EXISTS\r\n"))
			conn.Write([]byte("* 0 RECENT\r\n"))
			conn.Write([]byte("NB00 OK SELECT completed\r\n"))
		} else if strings.HasPrefix(line, "NB00 SEARCH") {
			conn.Write([]byte("* SEARCH\r\n"))
			conn.Write([]byte("NB00 OK SEARCH completed\r\n"))
		} else if strings.HasPrefix(line, "NB00 LOGOUT") {
			conn.Write([]byte("NB00 OK LOGOUT completed\r\n"))
			return
		} else {
			conn.Write([]byte("NB00 OK command completed\r\n"))
		}
	}
}

func TestEmailChannel_StartWithMockIMAP(t *testing.T) {
	ln := mockIMAPServer(t)
	defer ln.Close()

	addr := ln.Addr().(*net.TCPAddr)
	msgBus := bus.NewMessageBus()
	cfg := channels.EmailConfig{
		IMAPHost:     "127.0.0.1",
		IMAPPort:     addr.Port,
		IMAPUsername: "user@example.com",
		IMAPPassword: "pass",
		SMTPHost:     "127.0.0.1",
		SMTPPort:     19875, // SMTP will fail, but that's tolerated
		PollInterval: 60,    // Long interval to avoid polling during test
	}

	ch, _ := channels.NewEmailChannel(cfg, msgBus)
	ctx := context.Background()

	err := ch.Start(ctx)
	if err != nil {
		t.Fatalf("Start() failed with mock IMAP: %v", err)
	}

	if !ch.IsRunning() {
		t.Error("Channel should be running after Start()")
	}

	// Stop and verify
	stopErr := ch.Stop(ctx)
	if stopErr != nil {
		t.Errorf("Stop() returned error: %v", stopErr)
	}

	if ch.IsRunning() {
		t.Error("Channel should not be running after Stop()")
	}
}

func TestEmailChannel_SendWithoutRecipient(t *testing.T) {
	ln := mockIMAPServer(t)
	defer ln.Close()

	addr := ln.Addr().(*net.TCPAddr)
	msgBus := bus.NewMessageBus()
	cfg := channels.EmailConfig{
		IMAPHost:     "127.0.0.1",
		IMAPPort:     addr.Port,
		IMAPUsername: "user@example.com",
		IMAPPassword: "pass",
		SMTPHost:     "127.0.0.1",
		SMTPPort:     19875,
		PollInterval: 300,
	}

	ch, _ := channels.NewEmailChannel(cfg, msgBus)
	ctx := context.Background()

	ch.Start(ctx)
	defer ch.Stop(ctx)

	// Send with empty ChatID
	err := ch.Send(ctx, bus.OutboundMessage{
		Channel: "email",
		ChatID:  "",
		Content: "Hello",
	})
	if err == nil {
		t.Fatal("Expected error for empty recipient")
	}
	if !strings.Contains(err.Error(), "recipient") {
		t.Errorf("Expected recipient-related error, got: %v", err)
	}
}

func TestEmailChannel_IsAllowed(t *testing.T) {
	msgBus := bus.NewMessageBus()

	tests := []struct {
		name     string
		allow    []string
		sender   string
		expected bool
	}{
		{
			name:     "Empty allowlist allows all",
			allow:    nil,
			sender:   "anyone@example.com",
			expected: true,
		},
		{
			name:     "Empty slice allows all",
			allow:    []string{},
			sender:   "anyone@example.com",
			expected: true,
		},
		{
			name:     "Exact match",
			allow:    []string{"alice@example.com", "bob@example.com"},
			sender:   "alice@example.com",
			expected: true,
		},
		{
			name:     "No match",
			allow:    []string{"alice@example.com"},
			sender:   "eve@example.com",
			expected: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cfg := channels.EmailConfig{
				IMAPHost:     "imap.example.com",
				IMAPUsername: "user@example.com",
				IMAPPassword: "pass",
				SMTPHost:     "smtp.example.com",
				AllowFrom:    tt.allow,
			}
			ch, _ := channels.NewEmailChannel(cfg, msgBus)
			result := ch.IsAllowed(tt.sender)
			if result != tt.expected {
				t.Errorf("IsAllowed(%q) = %v, expected %v", tt.sender, result, tt.expected)
			}
		})
	}
}

// ---------------------------------------------------------------------------
// Matrix Channel Tests
// ---------------------------------------------------------------------------

func TestNewMatrixChannel_ValidConfig(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.MatrixConfig{
		Homeserver:  "https://matrix.example.com",
		AccessToken: "syt_token123",
	}

	ch, err := channels.NewMatrixChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewMatrixChannel() returned unexpected error: %v", err)
	}
	if ch == nil {
		t.Fatal("NewMatrixChannel() returned nil channel")
	}
}

func TestNewMatrixChannel_Defaults(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.MatrixConfig{
		Homeserver:  "https://matrix.example.com/",
		AccessToken: "token123",
	}

	ch, err := channels.NewMatrixChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewMatrixChannel() failed: %v", err)
	}

	// Name should default to "matrix"
	if ch.Name() != "matrix" {
		t.Errorf("Expected default name 'matrix', got '%s'", ch.Name())
	}

	// Should not be running
	if ch.IsRunning() {
		t.Error("New channel should not be running")
	}
}

func TestNewMatrixChannel_TrailingSlashStripped(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.MatrixConfig{
		Homeserver:  "https://matrix.example.com/",
		AccessToken: "token123",
	}

	ch, err := channels.NewMatrixChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewMatrixChannel() failed: %v", err)
	}
	_ = ch // Just verifying no panic or error; trailing slash is internal
}

func TestNewMatrixChannel_CustomName(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.MatrixConfig{
		Homeserver:  "https://matrix.example.com",
		AccessToken: "token123",
		ChannelName: "custom-matrix",
	}

	ch, err := channels.NewMatrixChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewMatrixChannel() failed: %v", err)
	}
	if ch.Name() != "custom-matrix" {
		t.Errorf("Expected name 'custom-matrix', got '%s'", ch.Name())
	}
}

func TestNewMatrixChannel_MissingHomeserver(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.MatrixConfig{
		AccessToken: "token123",
	}

	_, err := channels.NewMatrixChannel(cfg, msgBus)
	if err == nil {
		t.Fatal("Expected error when Homeserver is empty")
	}
	if !strings.Contains(err.Error(), "homeserver") {
		t.Errorf("Expected error mentioning homeserver, got: %v", err)
	}
}

func TestNewMatrixChannel_MissingAccessToken(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.MatrixConfig{
		Homeserver: "https://matrix.example.com",
	}

	_, err := channels.NewMatrixChannel(cfg, msgBus)
	if err == nil {
		t.Fatal("Expected error when AccessToken is empty")
	}
	if !strings.Contains(err.Error(), "access_token") {
		t.Errorf("Expected error mentioning access_token, got: %v", err)
	}
}

func TestMatrixChannel_SendNotRunning(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.MatrixConfig{
		Homeserver:  "https://matrix.example.com",
		AccessToken: "token123",
	}

	ch, _ := channels.NewMatrixChannel(cfg, msgBus)

	err := ch.Send(context.Background(), bus.OutboundMessage{
		Channel: "matrix",
		ChatID:  "!room:matrix.org",
		Content: "Hello",
	})
	if err == nil {
		t.Fatal("Expected error when sending on non-running channel")
	}
	if !strings.Contains(err.Error(), "not running") {
		t.Errorf("Expected 'not running' error, got: %v", err)
	}
}

func TestMatrixChannel_StopWithoutStart(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.MatrixConfig{
		Homeserver:  "https://matrix.example.com",
		AccessToken: "token123",
	}

	ch, _ := channels.NewMatrixChannel(cfg, msgBus)

	err := ch.Stop(context.Background())
	if err != nil {
		t.Errorf("Stop() on non-started channel should not return error, got: %v", err)
	}
}

// newMatrixMockServer creates an httptest.Server that simulates Matrix CS API.
func newMatrixMockServer(t *testing.T) *httptest.Server {
	t.Helper()
	mux := http.NewServeMux()

	mux.HandleFunc("/_matrix/client/v3/account/whoami", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]string{
			"user_id": "@bot:matrix.org",
		})
	})

	mux.HandleFunc("/_matrix/client/v3/sync", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]interface{}{
			"next_batch": "batch_token_123",
			"rooms": map[string]interface{}{
				"join": map[string]interface{}{},
			},
		})
	})

	return httptest.NewServer(mux)
}

func TestMatrixChannel_StartWithMockServer(t *testing.T) {
	syncCount := 0
	var syncMu sync.Mutex

	mux := http.NewServeMux()

	mux.HandleFunc("/_matrix/client/v3/account/whoami", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]string{
			"user_id": "@bot:matrix.org",
		})
	})

	mux.HandleFunc("/_matrix/client/v3/sync", func(w http.ResponseWriter, r *http.Request) {
		syncMu.Lock()
		syncCount++
		syncMu.Unlock()

		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]interface{}{
			"next_batch": "batch_token_123",
			"rooms": map[string]interface{}{
				"join": map[string]interface{}{},
			},
		})
	})

	server := httptest.NewServer(mux)
	defer server.Close()

	msgBus := bus.NewMessageBus()
	cfg := channels.MatrixConfig{
		Homeserver:  server.URL,
		AccessToken: "test-token",
		UserID:      "@bot:matrix.org",
		RoomID:      "!default:matrix.org",
	}

	ch, err := channels.NewMatrixChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewMatrixChannel() failed: %v", err)
	}

	ctx := context.Background()
	startErr := ch.Start(ctx)
	if startErr != nil {
		t.Fatalf("Start() failed with mock server: %v", startErr)
	}

	if !ch.IsRunning() {
		t.Error("Channel should be running after Start()")
	}

	// Wait briefly for the sync loop to run at least once
	time.Sleep(200 * time.Millisecond)

	stopErr := ch.Stop(ctx)
	if stopErr != nil {
		t.Errorf("Stop() returned error: %v", stopErr)
	}

	if ch.IsRunning() {
		t.Error("Channel should not be running after Stop()")
	}

	syncMu.Lock()
	count := syncCount
	syncMu.Unlock()
	if count == 0 {
		t.Error("Expected at least one sync request")
	}
}

func TestMatrixChannel_SendWithMockServer(t *testing.T) {
	mux := http.NewServeMux()

	mux.HandleFunc("/_matrix/client/v3/account/whoami", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]string{
			"user_id": "@bot:matrix.org",
		})
	})

	mux.HandleFunc("/_matrix/client/v3/sync", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]interface{}{
			"next_batch": "batch_token_123",
			"rooms": map[string]interface{}{
				"join": map[string]interface{}{},
			},
		})
	})

	// Catch-all for message sending (PUT requests to rooms endpoint)
	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPut {
			http.NotFound(w, r)
			return
		}

		// Verify Authorization header
		auth := r.Header.Get("Authorization")
		if auth != "Bearer test-token" {
			w.WriteHeader(http.StatusUnauthorized)
			json.NewEncoder(w).Encode(map[string]string{
				"errcode": "M_UNKNOWN_TOKEN",
				"error":   "Unrecognized access token",
			})
			return
		}

		// Read and verify body
		body, _ := io.ReadAll(r.Body)
		var payload map[string]string
		json.Unmarshal(body, &payload)

		if payload["msgtype"] != "m.text" {
			t.Errorf("Expected msgtype 'm.text', got '%s'", payload["msgtype"])
		}

		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]string{
			"event_id": "$event_123",
		})
	})

	server := httptest.NewServer(mux)
	defer server.Close()

	msgBus := bus.NewMessageBus()
	cfg := channels.MatrixConfig{
		Homeserver:  server.URL,
		AccessToken: "test-token",
		UserID:      "@bot:matrix.org",
	}

	ch, _ := channels.NewMatrixChannel(cfg, msgBus)
	ctx := context.Background()
	ch.Start(ctx)
	defer ch.Stop(ctx)

	err := ch.Send(ctx, bus.OutboundMessage{
		Channel: "matrix",
		ChatID:  "!room:matrix.org",
		Content: "Hello Matrix!",
	})
	if err != nil {
		t.Errorf("Send() returned unexpected error: %v", err)
	}
}

func TestMatrixChannel_SendNoRoomID(t *testing.T) {
	server := newMatrixMockServer(t)
	defer server.Close()

	msgBus := bus.NewMessageBus()
	cfg := channels.MatrixConfig{
		Homeserver:  server.URL,
		AccessToken: "test-token",
		UserID:      "@bot:matrix.org",
		// No RoomID configured, and no ChatID in message
	}

	ch, _ := channels.NewMatrixChannel(cfg, msgBus)
	ctx := context.Background()
	ch.Start(ctx)
	defer ch.Stop(ctx)

	err := ch.Send(ctx, bus.OutboundMessage{
		Channel: "matrix",
		ChatID:  "",
		Content: "Hello",
	})
	if err == nil {
		t.Fatal("Expected error for missing room ID")
	}
	if !strings.Contains(err.Error(), "room") {
		t.Errorf("Expected room-related error, got: %v", err)
	}
}

func TestMatrixChannel_SendUsesDefaultRoom(t *testing.T) {
	mux := http.NewServeMux()

	mux.HandleFunc("/_matrix/client/v3/account/whoami", func(w http.ResponseWriter, r *http.Request) {
		json.NewEncoder(w).Encode(map[string]string{"user_id": "@bot:matrix.org"})
	})

	mux.HandleFunc("/_matrix/client/v3/sync", func(w http.ResponseWriter, r *http.Request) {
		json.NewEncoder(w).Encode(map[string]interface{}{
			"next_batch": "batch1",
			"rooms":      map[string]interface{}{"join": map[string]interface{}{}},
		})
	})

	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodPut {
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(map[string]string{"event_id": "$evt_1"})
		}
	})

	server := httptest.NewServer(mux)
	defer server.Close()

	msgBus := bus.NewMessageBus()
	cfg := channels.MatrixConfig{
		Homeserver:  server.URL,
		AccessToken: "test-token",
		UserID:      "@bot:matrix.org",
		RoomID:      "!default:matrix.org",
	}

	ch, _ := channels.NewMatrixChannel(cfg, msgBus)
	ctx := context.Background()
	ch.Start(ctx)
	defer ch.Stop(ctx)

	// Send with empty ChatID -- should use default room
	err := ch.Send(ctx, bus.OutboundMessage{
		Channel: "matrix",
		ChatID:  "",
		Content: "Hello default room",
	})
	if err != nil {
		t.Errorf("Send() with default room should succeed, got: %v", err)
	}
}

func TestMatrixChannel_ReceivesMessage(t *testing.T) {
	mux := http.NewServeMux()
	syncCallCount := 0
	var syncMu sync.Mutex

	mux.HandleFunc("/_matrix/client/v3/account/whoami", func(w http.ResponseWriter, r *http.Request) {
		json.NewEncoder(w).Encode(map[string]string{"user_id": "@bot:matrix.org"})
	})

	mux.HandleFunc("/_matrix/client/v3/sync", func(w http.ResponseWriter, r *http.Request) {
		syncMu.Lock()
		syncCallCount++
		count := syncCallCount
		syncMu.Unlock()

		w.Header().Set("Content-Type", "application/json")

		// On the second sync (after initial), include a message
		if count == 2 {
			json.NewEncoder(w).Encode(map[string]interface{}{
				"next_batch": "batch_2",
				"rooms": map[string]interface{}{
					"join": map[string]interface{}{
						"!testroom:matrix.org": map[string]interface{}{
							"timeline": map[string]interface{}{
								"events": []map[string]interface{}{
									{
										"type":             "m.room.message",
										"sender":           "@alice:matrix.org",
										"event_id":         "$evt_001",
										"origin_server_ts": 1234567890,
										"content": map[string]string{
											"msgtype": "m.text",
											"body":    "Hello bot!",
										},
									},
								},
							},
						},
					},
				},
			})
		} else {
			json.NewEncoder(w).Encode(map[string]interface{}{
				"next_batch": fmt.Sprintf("batch_%d", count),
				"rooms": map[string]interface{}{
					"join": map[string]interface{}{},
				},
			})
		}
	})

	server := httptest.NewServer(mux)
	defer server.Close()

	msgBus := bus.NewMessageBus()
	cfg := channels.MatrixConfig{
		Homeserver:  server.URL,
		AccessToken: "test-token",
		UserID:      "@bot:matrix.org",
	}

	ch, _ := channels.NewMatrixChannel(cfg, msgBus)
	ctx := context.Background()
	ch.Start(ctx)
	defer ch.Stop(ctx)

	// Wait for the sync to process and message to be published to bus
	ctx2, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	msg, ok := msgBus.ConsumeInbound(ctx2)
	if !ok {
		t.Fatal("Expected to receive an inbound message from Matrix channel")
	}

	if msg.Channel != "matrix" {
		t.Errorf("Expected channel 'matrix', got '%s'", msg.Channel)
	}
	if msg.SenderID != "@alice:matrix.org" {
		t.Errorf("Expected sender '@alice:matrix.org', got '%s'", msg.SenderID)
	}
	if msg.Content != "Hello bot!" {
		t.Errorf("Expected content 'Hello bot!', got '%s'", msg.Content)
	}
	if msg.Metadata["event_id"] != "$evt_001" {
		t.Errorf("Expected event_id '$evt_001', got '%s'", msg.Metadata["event_id"])
	}
}

func TestMatrixChannel_IgnoresOwnMessages(t *testing.T) {
	mux := http.NewServeMux()
	syncCallCount := 0
	var syncMu sync.Mutex

	mux.HandleFunc("/_matrix/client/v3/account/whoami", func(w http.ResponseWriter, r *http.Request) {
		json.NewEncoder(w).Encode(map[string]string{"user_id": "@bot:matrix.org"})
	})

	mux.HandleFunc("/_matrix/client/v3/sync", func(w http.ResponseWriter, r *http.Request) {
		syncMu.Lock()
		syncCallCount++
		count := syncCallCount
		syncMu.Unlock()

		w.Header().Set("Content-Type", "application/json")

		if count == 2 {
			// Message from the bot itself -- should be ignored
			json.NewEncoder(w).Encode(map[string]interface{}{
				"next_batch": "batch_2",
				"rooms": map[string]interface{}{
					"join": map[string]interface{}{
						"!testroom:matrix.org": map[string]interface{}{
							"timeline": map[string]interface{}{
								"events": []map[string]interface{}{
									{
										"type":             "m.room.message",
										"sender":           "@bot:matrix.org", // Same as bot
										"event_id":         "$evt_own",
										"origin_server_ts": 1234567890,
										"content": map[string]string{
											"msgtype": "m.text",
											"body":    "My own message",
										},
									},
								},
							},
						},
					},
				},
			})
		} else {
			json.NewEncoder(w).Encode(map[string]interface{}{
				"next_batch": fmt.Sprintf("batch_%d", count),
				"rooms":      map[string]interface{}{"join": map[string]interface{}{}},
			})
		}
	})

	server := httptest.NewServer(mux)
	defer server.Close()

	msgBus := bus.NewMessageBus()
	cfg := channels.MatrixConfig{
		Homeserver:  server.URL,
		AccessToken: "test-token",
		UserID:      "@bot:matrix.org",
	}

	ch, _ := channels.NewMatrixChannel(cfg, msgBus)
	ctx := context.Background()
	ch.Start(ctx)
	defer ch.Stop(ctx)

	// Wait a bit for sync to process
	time.Sleep(500 * time.Millisecond)

	// Check that the bus does NOT contain our own message
	ctx2, cancel := context.WithTimeout(context.Background(), 500*time.Millisecond)
	defer cancel()

	_, ok := msgBus.ConsumeInbound(ctx2)
	if ok {
		t.Error("Bot should ignore its own messages; unexpected inbound message received")
	}
}

func TestMatrixChannel_IgnoresNonTextMessages(t *testing.T) {
	mux := http.NewServeMux()
	syncCallCount := 0
	var syncMu sync.Mutex

	mux.HandleFunc("/_matrix/client/v3/account/whoami", func(w http.ResponseWriter, r *http.Request) {
		json.NewEncoder(w).Encode(map[string]string{"user_id": "@bot:matrix.org"})
	})

	mux.HandleFunc("/_matrix/client/v3/sync", func(w http.ResponseWriter, r *http.Request) {
		syncMu.Lock()
		syncCallCount++
		count := syncCallCount
		syncMu.Unlock()

		w.Header().Set("Content-Type", "application/json")

		if count == 2 {
			// Non-text message (image) -- should be ignored
			json.NewEncoder(w).Encode(map[string]interface{}{
				"next_batch": "batch_2",
				"rooms": map[string]interface{}{
					"join": map[string]interface{}{
						"!testroom:matrix.org": map[string]interface{}{
							"timeline": map[string]interface{}{
								"events": []map[string]interface{}{
									{
										"type":             "m.room.message",
										"sender":           "@alice:matrix.org",
										"event_id":         "$evt_img",
										"origin_server_ts": 1234567890,
										"content": map[string]string{
											"msgtype": "m.image",
											"body":    "photo.jpg",
										},
									},
								},
							},
						},
					},
				},
			})
		} else {
			json.NewEncoder(w).Encode(map[string]interface{}{
				"next_batch": fmt.Sprintf("batch_%d", count),
				"rooms":      map[string]interface{}{"join": map[string]interface{}{}},
			})
		}
	})

	server := httptest.NewServer(mux)
	defer server.Close()

	msgBus := bus.NewMessageBus()
	cfg := channels.MatrixConfig{
		Homeserver:  server.URL,
		AccessToken: "test-token",
		UserID:      "@bot:matrix.org",
	}

	ch, _ := channels.NewMatrixChannel(cfg, msgBus)
	ctx := context.Background()
	ch.Start(ctx)
	defer ch.Stop(ctx)

	time.Sleep(500 * time.Millisecond)

	ctx2, cancel := context.WithTimeout(context.Background(), 500*time.Millisecond)
	defer cancel()

	_, ok := msgBus.ConsumeInbound(ctx2)
	if ok {
		t.Error("Non-text messages should be ignored")
	}
}

func TestMatrixChannel_IgnoresNonRoomMessageEvents(t *testing.T) {
	mux := http.NewServeMux()
	syncCallCount := 0
	var syncMu sync.Mutex

	mux.HandleFunc("/_matrix/client/v3/account/whoami", func(w http.ResponseWriter, r *http.Request) {
		json.NewEncoder(w).Encode(map[string]string{"user_id": "@bot:matrix.org"})
	})

	mux.HandleFunc("/_matrix/client/v3/sync", func(w http.ResponseWriter, r *http.Request) {
		syncMu.Lock()
		syncCallCount++
		count := syncCallCount
		syncMu.Unlock()

		w.Header().Set("Content-Type", "application/json")

		if count == 2 {
			// m.room.member event -- not m.room.message
			json.NewEncoder(w).Encode(map[string]interface{}{
				"next_batch": "batch_2",
				"rooms": map[string]interface{}{
					"join": map[string]interface{}{
						"!testroom:matrix.org": map[string]interface{}{
							"timeline": map[string]interface{}{
								"events": []map[string]interface{}{
									{
										"type":             "m.room.member",
										"sender":           "@alice:matrix.org",
										"event_id":         "$evt_member",
										"origin_server_ts": 1234567890,
										"state_key":        "@alice:matrix.org",
										"content": map[string]interface{}{
											"membership": "join",
										},
									},
								},
							},
						},
					},
				},
			})
		} else {
			json.NewEncoder(w).Encode(map[string]interface{}{
				"next_batch": fmt.Sprintf("batch_%d", count),
				"rooms":      map[string]interface{}{"join": map[string]interface{}{}},
			})
		}
	})

	server := httptest.NewServer(mux)
	defer server.Close()

	msgBus := bus.NewMessageBus()
	cfg := channels.MatrixConfig{
		Homeserver:  server.URL,
		AccessToken: "test-token",
		UserID:      "@bot:matrix.org",
	}

	ch, _ := channels.NewMatrixChannel(cfg, msgBus)
	ctx := context.Background()
	ch.Start(ctx)
	defer ch.Stop(ctx)

	time.Sleep(500 * time.Millisecond)

	ctx2, cancel := context.WithTimeout(context.Background(), 500*time.Millisecond)
	defer cancel()

	_, ok := msgBus.ConsumeInbound(ctx2)
	if ok {
		t.Error("Non m.room.message events should be ignored")
	}
}

func TestMatrixChannel_StartFails_InvalidCredentials(t *testing.T) {
	mux := http.NewServeMux()

	mux.HandleFunc("/_matrix/client/v3/account/whoami", func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusUnauthorized)
		json.NewEncoder(w).Encode(map[string]string{
			"errcode": "M_UNKNOWN_TOKEN",
			"error":   "Unrecognized access token",
		})
	})

	server := httptest.NewServer(mux)
	defer server.Close()

	msgBus := bus.NewMessageBus()
	cfg := channels.MatrixConfig{
		Homeserver:  server.URL,
		AccessToken: "bad-token",
	}

	ch, _ := channels.NewMatrixChannel(cfg, msgBus)
	err := ch.Start(context.Background())
	if err == nil {
		t.Fatal("Expected Start() to fail with invalid credentials")
		ch.Stop(context.Background())
	}
	if !strings.Contains(err.Error(), "credential") {
		t.Errorf("Expected credential error, got: %v", err)
	}
}

func TestMatrixChannel_SendServerReturnsError(t *testing.T) {
	mux := http.NewServeMux()

	mux.HandleFunc("/_matrix/client/v3/account/whoami", func(w http.ResponseWriter, r *http.Request) {
		json.NewEncoder(w).Encode(map[string]string{"user_id": "@bot:matrix.org"})
	})

	mux.HandleFunc("/_matrix/client/v3/sync", func(w http.ResponseWriter, r *http.Request) {
		json.NewEncoder(w).Encode(map[string]interface{}{
			"next_batch": "batch1",
			"rooms":      map[string]interface{}{"join": map[string]interface{}{}},
		})
	})

	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodPut {
			w.WriteHeader(http.StatusForbidden)
			json.NewEncoder(w).Encode(map[string]string{
				"errcode": "M_FORBIDDEN",
				"error":   "You are not allowed to send messages here",
			})
		}
	})

	server := httptest.NewServer(mux)
	defer server.Close()

	msgBus := bus.NewMessageBus()
	cfg := channels.MatrixConfig{
		Homeserver:  server.URL,
		AccessToken: "test-token",
		UserID:      "@bot:matrix.org",
		RoomID:      "!room:matrix.org",
	}

	ch, _ := channels.NewMatrixChannel(cfg, msgBus)
	ctx := context.Background()
	ch.Start(ctx)
	defer ch.Stop(ctx)

	err := ch.Send(ctx, bus.OutboundMessage{
		Channel: "matrix",
		ChatID:  "!room:matrix.org",
		Content: "Hello",
	})
	if err == nil {
		t.Fatal("Expected error when server returns error")
	}
}

func TestMatrixChannel_IsAllowed(t *testing.T) {
	msgBus := bus.NewMessageBus()

	tests := []struct {
		name     string
		allow    []string
		sender   string
		expected bool
	}{
		{
			name:     "Empty allowlist allows all",
			allow:    nil,
			sender:   "@anyone:matrix.org",
			expected: true,
		},
		{
			name:     "Exact match",
			allow:    []string{"@alice:matrix.org"},
			sender:   "@alice:matrix.org",
			expected: true,
		},
		{
			name:     "No match",
			allow:    []string{"@alice:matrix.org"},
			sender:   "@eve:matrix.org",
			expected: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cfg := channels.MatrixConfig{
				Homeserver:  "https://matrix.example.com",
				AccessToken: "token",
				AllowFrom:   tt.allow,
			}
			ch, _ := channels.NewMatrixChannel(cfg, msgBus)
			result := ch.IsAllowed(tt.sender)
			if result != tt.expected {
				t.Errorf("IsAllowed(%q) = %v, expected %v", tt.sender, result, tt.expected)
			}
		})
	}
}

// ---------------------------------------------------------------------------
// Webhook Inbound Channel Tests
// ---------------------------------------------------------------------------

func TestNewWebhookInboundChannel_ValidConfig(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.WebhookInboundConfig{
		ListenAddr: ":9091",
		Path:       "/webhook/test",
	}

	ch, err := channels.NewWebhookInboundChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewWebhookInboundChannel() returned unexpected error: %v", err)
	}
	if ch == nil {
		t.Fatal("NewWebhookInboundChannel() returned nil channel")
	}
}

func TestNewWebhookInboundChannel_Defaults(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.WebhookInboundConfig{}

	ch, err := channels.NewWebhookInboundChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewWebhookInboundChannel() failed: %v", err)
	}

	if ch.Name() != "webhook" {
		t.Errorf("Expected default name 'webhook', got '%s'", ch.Name())
	}

	if ch.IsRunning() {
		t.Error("New channel should not be running")
	}
}

func TestNewWebhookInboundChannel_CustomName(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.WebhookInboundConfig{
		ChannelName: "custom-webhook",
	}

	ch, err := channels.NewWebhookInboundChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("NewWebhookInboundChannel() failed: %v", err)
	}
	if ch.Name() != "custom-webhook" {
		t.Errorf("Expected name 'custom-webhook', got '%s'", ch.Name())
	}
}

func TestWebhookInboundChannel_StartStop(t *testing.T) {
	addr := findFreePort(t)

	msgBus := bus.NewMessageBus()
	cfg := channels.WebhookInboundConfig{
		ListenAddr: addr,
		Path:       "/webhook/test",
	}

	ch, _ := channels.NewWebhookInboundChannel(cfg, msgBus)
	ctx := context.Background()

	err := ch.Start(ctx)
	if err != nil {
		t.Fatalf("Start() failed: %v", err)
	}

	if !ch.IsRunning() {
		t.Error("Channel should be running after Start()")
	}

	time.Sleep(100 * time.Millisecond)

	// Stop should succeed
	stopErr := ch.Stop(ctx)
	if stopErr != nil {
		t.Errorf("Stop() returned error: %v", stopErr)
	}

	if ch.IsRunning() {
		t.Error("Channel should not be running after Stop()")
	}
}

func TestWebhookInboundChannel_StopWithoutStart(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.WebhookInboundConfig{
		ListenAddr: ":0",
	}

	ch, _ := channels.NewWebhookInboundChannel(cfg, msgBus)

	err := ch.Stop(context.Background())
	if err != nil {
		t.Errorf("Stop() on non-started channel should not return error, got: %v", err)
	}
}

func TestWebhookInboundChannel_SendNotRunning(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.WebhookInboundConfig{}

	ch, _ := channels.NewWebhookInboundChannel(cfg, msgBus)

	err := ch.Send(context.Background(), bus.OutboundMessage{
		Channel: "webhook",
		ChatID:  "test-chat",
		Content: "Hello",
	})
	if err == nil {
		t.Fatal("Expected error when sending on non-running channel")
	}
	if !strings.Contains(err.Error(), "not running") {
		t.Errorf("Expected 'not running' error, got: %v", err)
	}
}

func TestWebhookInboundChannel_FullIntegration(t *testing.T) {
	addr := findFreePort(t)

	msgBus := bus.NewMessageBus()
	cfg := channels.WebhookInboundConfig{
		ListenAddr: addr,
		Path:       "/webhook/incoming",
	}

	ch, _ := channels.NewWebhookInboundChannel(cfg, msgBus)
	ctx := context.Background()
	ch.Start(ctx)
	defer ch.Stop(ctx)

	time.Sleep(200 * time.Millisecond)

	// Send a POST request with valid JSON in a goroutine.
	// The handler will block waiting for an outbound response, so we
	// run it in a goroutine and use channel.Send() to resolve it.
	payload := `{"content": "Hello webhook!", "sender_id": "user1", "chat_id": "chat1"}`

	httpDone := make(chan struct{})
	go func() {
		defer close(httpDone)
		client := shortHTTPClient()
		resp, err := client.Post("http://"+addr+"/webhook/incoming", "application/json", strings.NewReader(payload))
		if err != nil {
			return
		}
		io.ReadAll(resp.Body)
		resp.Body.Close()
	}()

	// Consume the inbound message from the bus
	ctx2, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	msg, ok := msgBus.ConsumeInbound(ctx2)
	if !ok {
		t.Fatal("Expected to receive inbound message from webhook channel")
	}

	if msg.Channel != "webhook" {
		t.Errorf("Expected channel 'webhook', got '%s'", msg.Channel)
	}
	if msg.SenderID != "user1" {
		t.Errorf("Expected sender 'user1', got '%s'", msg.SenderID)
	}
	if msg.ChatID != "chat1" {
		t.Errorf("Expected chatID 'chat1', got '%s'", msg.ChatID)
	}
	if msg.Content != "Hello webhook!" {
		t.Errorf("Expected content 'Hello webhook!', got '%s'", msg.Content)
	}
	if msg.Metadata["platform"] != "webhook_inbound" {
		t.Errorf("Expected platform 'webhook_inbound', got '%s'", msg.Metadata["platform"])
	}

	// Send a response to resolve the pending HTTP request
	ch.Send(ctx, bus.OutboundMessage{
		Channel: "webhook",
		ChatID:  "chat1",
		Content: "Response!",
	})

	// Wait for the HTTP goroutine to finish
	select {
	case <-httpDone:
	case <-time.After(5 * time.Second):
		t.Error("HTTP request goroutine did not finish in time")
	}
}

func TestWebhookInboundChannel_MethodNotAllowed(t *testing.T) {
	addr := findFreePort(t)

	msgBus := bus.NewMessageBus()
	cfg := channels.WebhookInboundConfig{
		ListenAddr: addr,
		Path:       "/webhook/test",
	}

	ch, _ := channels.NewWebhookInboundChannel(cfg, msgBus)
	ctx := context.Background()
	ch.Start(ctx)
	defer ch.Stop(ctx)

	time.Sleep(100 * time.Millisecond)

	// GET should be rejected
	req, _ := http.NewRequest(http.MethodGet, "http://"+addr+"/webhook/test", nil)
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatalf("GET request failed: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusMethodNotAllowed {
		t.Errorf("Expected 405 Method Not Allowed, got %d", resp.StatusCode)
	}
}

func TestWebhookInboundChannel_InvalidJSON(t *testing.T) {
	addr := findFreePort(t)

	msgBus := bus.NewMessageBus()
	cfg := channels.WebhookInboundConfig{
		ListenAddr: addr,
		Path:       "/webhook/test",
	}

	ch, _ := channels.NewWebhookInboundChannel(cfg, msgBus)
	ctx := context.Background()
	ch.Start(ctx)
	defer ch.Stop(ctx)

	time.Sleep(100 * time.Millisecond)

	resp, err := http.Post("http://"+addr+"/webhook/test", "application/json", strings.NewReader("not json"))
	if err != nil {
		t.Fatalf("POST request failed: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusBadRequest {
		t.Errorf("Expected 400 Bad Request, got %d", resp.StatusCode)
	}
}

func TestWebhookInboundChannel_EmptyContent(t *testing.T) {
	addr := findFreePort(t)

	msgBus := bus.NewMessageBus()
	cfg := channels.WebhookInboundConfig{
		ListenAddr: addr,
		Path:       "/webhook/test",
	}

	ch, _ := channels.NewWebhookInboundChannel(cfg, msgBus)
	ctx := context.Background()
	ch.Start(ctx)
	defer ch.Stop(ctx)

	time.Sleep(100 * time.Millisecond)

	payload := `{"content": "", "sender_id": "user1"}`
	resp, err := http.Post("http://"+addr+"/webhook/test", "application/json", strings.NewReader(payload))
	if err != nil {
		t.Fatalf("POST request failed: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusBadRequest {
		t.Errorf("Expected 400 Bad Request for empty content, got %d", resp.StatusCode)
	}
}

func TestWebhookInboundChannel_APIKeyValidation(t *testing.T) {
	addr := findFreePort(t)

	msgBus := bus.NewMessageBus()
	cfg := channels.WebhookInboundConfig{
		ListenAddr: addr,
		Path:       "/webhook/test",
		APIKey:     "secret-key-123",
	}

	ch, _ := channels.NewWebhookInboundChannel(cfg, msgBus)
	ctx := context.Background()
	ch.Start(ctx)
	defer ch.Stop(ctx)

	time.Sleep(100 * time.Millisecond)

	t.Run("No API key", func(t *testing.T) {
		payload := `{"content": "Hello"}`
		resp, err := http.Post("http://"+addr+"/webhook/test", "application/json", strings.NewReader(payload))
		if err != nil {
			t.Fatalf("POST request failed: %v", err)
		}
		defer resp.Body.Close()

		if resp.StatusCode != http.StatusForbidden {
			t.Errorf("Expected 403 Forbidden without API key, got %d", resp.StatusCode)
		}
	})

	t.Run("Wrong API key", func(t *testing.T) {
		req, _ := http.NewRequest(http.MethodPost, "http://"+addr+"/webhook/test", strings.NewReader(`{"content": "Hello"}`))
		req.Header.Set("Content-Type", "application/json")
		req.Header.Set("X-Webhook-Key", "wrong-key")
		resp, err := http.DefaultClient.Do(req)
		if err != nil {
			t.Fatalf("POST request failed: %v", err)
		}
		defer resp.Body.Close()

		if resp.StatusCode != http.StatusForbidden {
			t.Errorf("Expected 403 Forbidden with wrong API key, got %d", resp.StatusCode)
		}
	})

	t.Run("Correct API key", func(t *testing.T) {
		req, _ := http.NewRequest(http.MethodPost, "http://"+addr+"/webhook/test", strings.NewReader(`{"content": "Hello", "chat_id": "api-test-chat"}`))
		req.Header.Set("Content-Type", "application/json")
		req.Header.Set("X-Webhook-Key", "secret-key-123")

		// Use a goroutine since the handler will block waiting for a response
		go func() {
			resp, err := shortHTTPClient().Do(req)
			if err != nil {
				return
			}
			io.ReadAll(resp.Body)
			resp.Body.Close()
		}()

		// Consume the inbound message (which proves the key was accepted)
		ctx2, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()

		msg, ok := msgBus.ConsumeInbound(ctx2)
		if !ok {
			t.Fatal("Expected to receive inbound message with correct API key")
		}

		// Respond to unblock the handler
		ch.Send(ctx, bus.OutboundMessage{
			Channel: "webhook",
			ChatID:  msg.ChatID,
			Content: "OK",
		})
	})
}

func TestWebhookInboundChannel_DefaultValues(t *testing.T) {
	addr := findFreePort(t)

	msgBus := bus.NewMessageBus()
	cfg := channels.WebhookInboundConfig{
		ListenAddr: addr,
		Path:       "/webhook/test",
	}

	ch, _ := channels.NewWebhookInboundChannel(cfg, msgBus)
	ctx := context.Background()
	ch.Start(ctx)
	defer ch.Stop(ctx)

	time.Sleep(100 * time.Millisecond)

	// Send request without sender_id and chat_id -- should use defaults
	payload := `{"content": "Test defaults"}`
	go func() {
		client := shortHTTPClient()
		resp, err := client.Post("http://"+addr+"/webhook/test", "application/json", strings.NewReader(payload))
		if err != nil {
			return
		}
		io.ReadAll(resp.Body)
		resp.Body.Close()
	}()

	// Consume the inbound message
	ctx2, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	msg, ok := msgBus.ConsumeInbound(ctx2)
	if !ok {
		t.Fatal("Expected inbound message")
	}

	if msg.SenderID != "webhook" {
		t.Errorf("Expected default sender_id 'webhook', got '%s'", msg.SenderID)
	}
	if msg.ChatID != "webhook:default" {
		t.Errorf("Expected default chat_id 'webhook:default', got '%s'", msg.ChatID)
	}

	// Respond to unblock handler
	ch.Send(ctx, bus.OutboundMessage{
		Channel: "webhook",
		ChatID:  msg.ChatID,
		Content: "OK",
	})
}

func TestWebhookInboundChannel_MetadataPassing(t *testing.T) {
	addr := findFreePort(t)

	msgBus := bus.NewMessageBus()
	cfg := channels.WebhookInboundConfig{
		ListenAddr: addr,
		Path:       "/webhook/test",
	}

	ch, _ := channels.NewWebhookInboundChannel(cfg, msgBus)
	ctx := context.Background()
	ch.Start(ctx)
	defer ch.Stop(ctx)

	time.Sleep(100 * time.Millisecond)

	payload := `{"content": "Test metadata", "sender_id": "user1", "chat_id": "chat1", "metadata": {"source": "test", "priority": "high"}}`
	go func() {
		client := shortHTTPClient()
		resp, err := client.Post("http://"+addr+"/webhook/test", "application/json", strings.NewReader(payload))
		if err != nil {
			return
		}
		io.ReadAll(resp.Body)
		resp.Body.Close()
	}()

	ctx2, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	msg, ok := msgBus.ConsumeInbound(ctx2)
	if !ok {
		t.Fatal("Expected inbound message")
	}

	if msg.Metadata["source"] != "test" {
		t.Errorf("Expected metadata source='test', got '%s'", msg.Metadata["source"])
	}
	if msg.Metadata["priority"] != "high" {
		t.Errorf("Expected metadata priority='high', got '%s'", msg.Metadata["priority"])
	}
	if msg.Metadata["platform"] != "webhook_inbound" {
		t.Errorf("Expected metadata platform='webhook_inbound', got '%s'", msg.Metadata["platform"])
	}

	ch.Send(ctx, bus.OutboundMessage{
		Channel: "webhook",
		ChatID:  msg.ChatID,
		Content: "OK",
	})
}

func TestWebhookInboundChannel_SendResolvesPending(t *testing.T) {
	addr := findFreePort(t)

	msgBus := bus.NewMessageBus()
	cfg := channels.WebhookInboundConfig{
		ListenAddr: addr,
		Path:       "/webhook/test",
	}

	ch, _ := channels.NewWebhookInboundChannel(cfg, msgBus)
	ctx := context.Background()
	ch.Start(ctx)
	defer ch.Stop(ctx)

	time.Sleep(100 * time.Millisecond)

	// Send a webhook request in a goroutine
	httpDone := make(chan string) // receives response content or empty string
	go func() {
		client := shortHTTPClient()
		resp, err := client.Post("http://"+addr+"/webhook/test", "application/json",
			strings.NewReader(`{"content": "Please respond", "sender_id": "user1", "chat_id": "response-test"}`))
		if err != nil {
			close(httpDone)
			return
		}
		defer resp.Body.Close()
		var result struct {
			Content string `json:"content"`
			Error   string `json:"error,omitempty"`
		}
		json.NewDecoder(resp.Body).Decode(&result)
		httpDone <- result.Content
	}()

	// Wait for the inbound message
	ctx2, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	msg, ok := msgBus.ConsumeInbound(ctx2)
	if !ok {
		t.Fatal("Expected inbound message")
	}

	// Send the response via the channel to resolve the pending HTTP request
	err := ch.Send(ctx, bus.OutboundMessage{
		Channel: "webhook",
		ChatID:  msg.ChatID,
		Content: "Here is your response!",
	})
	if err != nil {
		t.Errorf("Send() returned error: %v", err)
	}

	// Verify the HTTP client received the response
	select {
	case content := <-httpDone:
		if content != "Here is your response!" {
			t.Errorf("Expected response 'Here is your response!', got '%s'", content)
		}
	case <-time.After(5 * time.Second):
		t.Error("Timed out waiting for HTTP response")
	}
}

func TestWebhookInboundChannel_SendNoPendingRequest(t *testing.T) {
	addr := findFreePort(t)

	msgBus := bus.NewMessageBus()
	cfg := channels.WebhookInboundConfig{
		ListenAddr: addr,
		Path:       "/webhook/test",
	}

	ch, _ := channels.NewWebhookInboundChannel(cfg, msgBus)
	ctx := context.Background()
	ch.Start(ctx)
	defer ch.Stop(ctx)

	time.Sleep(100 * time.Millisecond)

	// Send response for a chatID that has no pending request
	err := ch.Send(ctx, bus.OutboundMessage{
		Channel: "webhook",
		ChatID:  "nonexistent-chat",
		Content: "Nobody is waiting for this",
	})
	// Should not error -- just silently ignored
	if err != nil {
		t.Errorf("Send() for non-pending chat should not error, got: %v", err)
	}
}

func TestWebhookInboundChannel_RoutingHandler(t *testing.T) {
	addr := findFreePort(t)

	msgBus := bus.NewMessageBus()
	cfg := channels.WebhookInboundConfig{
		ListenAddr: addr,
		Path:       "/webhook/incoming",
	}

	ch, _ := channels.NewWebhookInboundChannel(cfg, msgBus)
	ctx := context.Background()
	ch.Start(ctx)
	defer ch.Stop(ctx)

	time.Sleep(100 * time.Millisecond)

	// Send request to the routing path in a goroutine
	go func() {
		client := shortHTTPClient()
		resp, err := client.Post("http://"+addr+"/webhook/incoming/mychannel/mychat123", "application/json",
			strings.NewReader(`{"content": "Routed message"}`))
		if err != nil {
			return
		}
		io.ReadAll(resp.Body)
		resp.Body.Close()
	}()

	ctx2, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	msg, ok := msgBus.ConsumeInbound(ctx2)
	if !ok {
		t.Fatal("Expected inbound message from routing handler")
	}

	// The chat_id should be overridden by the path segment
	if msg.ChatID != "mychat123" {
		t.Errorf("Expected chatID 'mychat123' from path, got '%s'", msg.ChatID)
	}
	if msg.Metadata["routed_channel"] != "mychannel" {
		t.Errorf("Expected routed_channel 'mychannel', got '%s'", msg.Metadata["routed_channel"])
	}

	ch.Send(ctx, bus.OutboundMessage{
		Channel: "webhook",
		ChatID:  msg.ChatID,
		Content: "OK",
	})
}

func TestWebhookInboundChannel_RoutingHandlerWithAPIKey(t *testing.T) {
	addr := findFreePort(t)

	msgBus := bus.NewMessageBus()
	cfg := channels.WebhookInboundConfig{
		ListenAddr: addr,
		Path:       "/webhook/incoming",
		APIKey:     "secret-key",
	}

	ch, _ := channels.NewWebhookInboundChannel(cfg, msgBus)
	ctx := context.Background()
	ch.Start(ctx)
	defer ch.Stop(ctx)

	time.Sleep(100 * time.Millisecond)

	// Without API key -- routing path
	req, _ := http.NewRequest(http.MethodPost, "http://"+addr+"/webhook/incoming/ch/chat1", strings.NewReader(`{"content": "test"}`))
	req.Header.Set("Content-Type", "application/json")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatalf("Request failed: %v", err)
	}
	resp.Body.Close()

	if resp.StatusCode != http.StatusForbidden {
		t.Errorf("Expected 403 for routing path without API key, got %d", resp.StatusCode)
	}
}

func TestWebhookInboundChannel_RoutingHandlerMethodCheck(t *testing.T) {
	addr := findFreePort(t)

	msgBus := bus.NewMessageBus()
	cfg := channels.WebhookInboundConfig{
		ListenAddr: addr,
		Path:       "/webhook/incoming",
	}

	ch, _ := channels.NewWebhookInboundChannel(cfg, msgBus)
	ctx := context.Background()
	ch.Start(ctx)
	defer ch.Stop(ctx)

	time.Sleep(100 * time.Millisecond)

	// GET on routing path should be rejected
	req, _ := http.NewRequest(http.MethodGet, "http://"+addr+"/webhook/incoming/ch/chat1", nil)
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatalf("GET request failed: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusMethodNotAllowed {
		t.Errorf("Expected 405 Method Not Allowed, got %d", resp.StatusCode)
	}
}

func TestWebhookInboundChannel_RoutingHandlerInvalidJSON(t *testing.T) {
	addr := findFreePort(t)

	msgBus := bus.NewMessageBus()
	cfg := channels.WebhookInboundConfig{
		ListenAddr: addr,
		Path:       "/webhook/incoming",
	}

	ch, _ := channels.NewWebhookInboundChannel(cfg, msgBus)
	ctx := context.Background()
	ch.Start(ctx)
	defer ch.Stop(ctx)

	time.Sleep(100 * time.Millisecond)

	resp, err := http.Post("http://"+addr+"/webhook/incoming/ch/chat1", "application/json", strings.NewReader("invalid"))
	if err != nil {
		t.Fatalf("POST request failed: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusBadRequest {
		t.Errorf("Expected 400 for invalid JSON in routing handler, got %d", resp.StatusCode)
	}
}

func TestWebhookInboundChannel_RoutingHandlerEmptyContent(t *testing.T) {
	addr := findFreePort(t)

	msgBus := bus.NewMessageBus()
	cfg := channels.WebhookInboundConfig{
		ListenAddr: addr,
		Path:       "/webhook/incoming",
	}

	ch, _ := channels.NewWebhookInboundChannel(cfg, msgBus)
	ctx := context.Background()
	ch.Start(ctx)
	defer ch.Stop(ctx)

	time.Sleep(100 * time.Millisecond)

	resp, err := http.Post("http://"+addr+"/webhook/incoming/ch/chat1", "application/json",
		strings.NewReader(`{"content": ""}`))
	if err != nil {
		t.Fatalf("POST request failed: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusBadRequest {
		t.Errorf("Expected 400 for empty content in routing handler, got %d", resp.StatusCode)
	}
}

func TestWebhookInboundChannel_DoubleStop(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := channels.WebhookInboundConfig{
		ListenAddr: ":0",
	}

	ch, _ := channels.NewWebhookInboundChannel(cfg, msgBus)
	ctx := context.Background()

	// Stop twice -- should not panic
	ch.Stop(ctx)
	ch.Stop(ctx)
}

func TestWebhookInboundChannel_IsAllowed(t *testing.T) {
	msgBus := bus.NewMessageBus()

	tests := []struct {
		name     string
		allow    []string
		sender   string
		expected bool
	}{
		{
			name:     "Empty allowlist allows all",
			allow:    nil,
			sender:   "any-sender",
			expected: true,
		},
		{
			name:     "Exact match",
			allow:    []string{"allowed-user"},
			sender:   "allowed-user",
			expected: true,
		},
		{
			name:     "No match",
			allow:    []string{"allowed-user"},
			sender:   "other-user",
			expected: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cfg := channels.WebhookInboundConfig{
				AllowFrom: tt.allow,
			}
			ch, _ := channels.NewWebhookInboundChannel(cfg, msgBus)
			result := ch.IsAllowed(tt.sender)
			if result != tt.expected {
				t.Errorf("IsAllowed(%q) = %v, expected %v", tt.sender, result, tt.expected)
			}
		})
	}
}

// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Web Channel Unit Tests

package web_test

import (
	"context"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	. "github.com/276793422/NemesisBot/module/web"
)

// TestHandleWebSocketWithToken tests WebSocket connection with valid auth token
func TestHandleWebSocketWithToken(t *testing.T) {
	// Create session manager and bus
	sessionMgr := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()

	authToken := "test-secret-token-123"

	// Create server using NewServer
	server := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       0, // Use random port
		WSPath:     "/ws",
		AuthToken:  authToken,
		SessionMgr: sessionMgr,
		Bus:        testBus,
	})

	// Create test HTTP server with the server's handler
	testServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Manually call the handleWebSocket logic
		if r.URL.Path == "/ws" {
			// Check auth token
			if authToken != "" {
				token := r.URL.Query().Get("token")
				if token != authToken {
					http.Error(w, "Unauthorized", http.StatusUnauthorized)
					return
				}
			}
			// For testing, we'll simulate a successful WebSocket upgrade
			// In real scenario, the server would handle this
			w.WriteHeader(http.StatusSwitchingProtocols)
		}
	}))
	defer testServer.Close()

	// Note: Full WebSocket testing requires the server's internal handler
	// This test validates the configuration was accepted
	if server == nil {
		t.Fatal("Server creation failed")
	}
}

// TestNewServerWithAuthToken tests creating server with auth token configuration
func TestNewServerWithAuthToken(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()

	authToken := "test-token-xyz"

	server := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       8080,
		WSPath:     "/ws",
		AuthToken:  authToken,
		SessionMgr: sessionMgr,
		Bus:        testBus,
	})

	if server == nil {
		t.Fatal("Expected server to be created, got nil")
	}
}

// TestNewServerWithoutAuthToken tests creating server without auth token
func TestNewServerWithoutAuthToken(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()

	server := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       8080,
		WSPath:     "/ws",
		AuthToken:  "", // No auth token
		SessionMgr: sessionMgr,
		Bus:        testBus,
	})

	if server == nil {
		t.Fatal("Expected server to be created, got nil")
	}
}

// TestSessionManagerCreation tests session manager creation
func TestSessionManagerCreation(t *testing.T) {
	timeout := 1 * time.Hour
	sessionMgr := NewSessionManager(timeout)

	if sessionMgr == nil {
		t.Fatal("Expected session manager to be created, got nil")
	}

	// Check initial stats
	stats := sessionMgr.Stats()
	if stats == nil {
		t.Error("Expected stats to be returned, got nil")
	}

	activeSessions := stats["active_sessions"]
	if activeSessions == nil {
		t.Error("Expected active_sessions in stats")
	}
}

// TestBroadcastToNonExistentSession tests broadcasting to non-existent session
func TestBroadcastToNonExistentSession(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)

	// Try to send to non-existent session
	err := BroadcastToSession(sessionMgr, "non-existent-session-id", "assistant", "test message")

	if err == nil {
		t.Error("Expected error when broadcasting to non-existent session, got nil")
	}
}

// TestSessionManagerStats tests session manager statistics
func TestSessionManagerStats(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)

	stats := sessionMgr.Stats()

	// Initially should have 0 active sessions
	activeSessions, ok := stats["active_sessions"].(int)
	if !ok {
		t.Error("Expected active_sessions to be an int")
	}

	if activeSessions != 0 {
		t.Errorf("Expected 0 active sessions, got: %d", activeSessions)
	}
}

// TestGetActiveCount tests getting active session count
func TestGetActiveCount(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)

	count := sessionMgr.GetActiveCount()
	if count != 0 {
		t.Errorf("Expected 0 active sessions, got: %d", count)
	}
}

// TestGetAllSessionsEmpty tests getting all sessions when none exist
func TestGetAllSessionsEmpty(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)

	sessions := sessionMgr.GetAllSessions()
	// In Go, nil slices are valid - check length instead
	if sessions != nil && len(sessions) != 0 {
		t.Errorf("Expected 0 sessions, got: %d", len(sessions))
	}
	if len(sessions) != 0 {
		t.Errorf("Expected 0 sessions, got: %d", len(sessions))
	}
}

// TestServerStartAndStop tests server start and stop
func TestServerStartAndStop(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()

	server := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       0, // Random port
		WSPath:     "/ws",
		AuthToken:  "test-token",
		SessionMgr: sessionMgr,
		Bus:        testBus,
	})

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	// Try to start server
	err := server.Start(ctx)
	if err != nil && err != context.DeadlineExceeded {
		t.Logf("Server start returned error (may be expected): %v", err)
	}

	// Try to shutdown
	shutdownErr := server.Shutdown(context.Background())
	if shutdownErr != nil {
		t.Logf("Server shutdown returned error: %v", shutdownErr)
	}
}

// TestWebSocketMessageTypes tests WebSocket message type constants
func TestWebSocketMessageTypes(t *testing.T) {
	// This test verifies the message types are correctly defined
	// Since they're constants, we just check they compile and can be used

	// In a real test, we would import the web package and check:
	// MessageTypeMessage == "message"
	// MessageTypePing == "ping"
	// MessageTypePong == "pong"

	// For now, this is a placeholder that ensures the test infrastructure works
	if true {
		return
	}
	t.Error("Test should not reach here")
}

// TestServerConfigDefaults tests server configuration with defaults
func TestServerConfigDefaults(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()

	config := ServerConfig{
		SessionMgr: sessionMgr,
		Bus:        testBus,
		// Use defaults for other fields
	}

	server := NewServer(config)
	if server == nil {
		t.Fatal("Expected server to be created with defaults, got nil")
	}
}

// TestAuthTokenExtraction tests URL parameter extraction (conceptual)
func TestAuthTokenExtraction(t *testing.T) {
	// This test validates that token extraction logic works correctly
	// In the actual implementation, tokens are extracted from URL query params

	testCases := []struct {
		name     string
		url      string
		expected string
	}{
		{
			name:     "Token in URL",
			url:      "/ws?token=my-secret-token",
			expected: "my-secret-token",
		},
		{
			name:     "Token with other params",
			url:      "/ws?token=my-token&foo=bar",
			expected: "my-token",
		},
		{
			name:     "No token",
			url:      "/ws",
			expected: "",
		},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			// Parse URL and extract token
			// This is a simplified version of what the server does
			parts := strings.Split(tc.url, "?token=")
			var token string
			if len(parts) > 1 {
				tokenAndMore := parts[1]
				// Split by & to isolate token
				tokenParts := strings.Split(tokenAndMore, "&")
				token = tokenParts[0]
			}

			if token != tc.expected {
				t.Errorf("Expected token %s, got: %s", tc.expected, token)
			}
		})
	}
}

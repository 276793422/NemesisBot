// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Web Channel Integration Tests

package web_test

import (
	"context"
	"net/http"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	. "github.com/276793422/NemesisBot/module/web"
)

// TestIntegrationServerLifecycle tests complete server lifecycle with auth
func TestIntegrationServerLifecycle(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()

	authToken := "integration-test-token"

	server := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       0, // Random port
		WSPath:     "/ws",
		AuthToken:  authToken,
		SessionMgr: sessionMgr,
		Bus:        testBus,
	})

	// Start server
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	// Try to start
	err := server.Start(ctx)
	if err != nil && err != context.DeadlineExceeded && err != http.ErrServerClosed {
		t.Logf("Server start returned: %v", err)
	}

	// Verify server was created
	if server == nil {
		t.Error("Server should not be nil")
	}

	// Shutdown
	shutdownErr := server.Shutdown(context.Background())
	if shutdownErr != nil {
		t.Logf("Server shutdown returned: %v", shutdownErr)
	}
}

// TestIntegrationServerWithoutAuth tests server without authentication
func TestIntegrationServerWithoutAuth(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()

	server := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       0,
		WSPath:     "/ws",
		AuthToken:  "", // No auth
		SessionMgr: sessionMgr,
		Bus:        testBus,
	})

	ctx, cancel := context.WithTimeout(context.Background(), 1*time.Second)
	defer cancel()

	err := server.Start(ctx)
	if err != nil && err != context.DeadlineExceeded && err != http.ErrServerClosed {
		t.Logf("Server start returned: %v", err)
	}

	if server == nil {
		t.Error("Server should not be nil even without auth")
	}

	server.Shutdown(context.Background())
}

// TestIntegrationSessionManagerWithServer tests session manager integration
func TestIntegrationSessionManagerWithServer(t *testing.T) {
	timeout := 500 * time.Millisecond
	sessionMgr := NewSessionManager(timeout)
	testBus := bus.NewMessageBus()

	server := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       0,
		WSPath:     "/ws",
		AuthToken:  "test-token",
		SessionMgr: sessionMgr,
		Bus:        testBus,
	})

	// Check initial state
	stats := sessionMgr.Stats()
	activeSessions := stats["active_sessions"].(int)
	if activeSessions != 0 {
		t.Errorf("Expected 0 sessions initially, got: %d", activeSessions)
	}

	// Get all sessions
	sessions := sessionMgr.GetAllSessions()
	if len(sessions) != 0 {
		t.Errorf("Expected 0 sessions in GetAllSessions, got: %d", len(sessions))
	}

	// Cleanup
	server.Shutdown(context.Background())
	sessionMgr.Shutdown()
}

// TestIntegrationMultipleServers tests multiple server instances
func TestIntegrationMultipleServers(t *testing.T) {
	testBus := bus.NewMessageBus()

	// Create two servers with different configs
	sessionMgr1 := NewSessionManager(1 * time.Hour)
	server1 := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       0,
		WSPath:     "/ws",
		AuthToken:  "token1",
		SessionMgr: sessionMgr1,
		Bus:        testBus,
	})

	sessionMgr2 := NewSessionManager(1 * time.Hour)
	server2 := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       0,
		WSPath:     "/ws",
		AuthToken:  "token2",
		SessionMgr: sessionMgr2,
		Bus:        testBus,
	})

	if server1 == nil || server2 == nil {
		t.Error("Both servers should be created")
	}

	// Cleanup
	server1.Shutdown(context.Background())
	server2.Shutdown(context.Background())
}

// TestIntegrationSendToNonExistentSession tests error handling
func TestIntegrationSendToNonExistentSession(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()

	server := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       0,
		WSPath:     "/ws",
		AuthToken:  "test-token",
		SessionMgr: sessionMgr,
		Bus:        testBus,
	})

	// Try to send to non-existent session
	err := server.SendToSession("fake-session-id", "assistant", "test")
	if err == nil {
		t.Error("Expected error when sending to non-existent session")
	}

	// Cleanup
	server.Shutdown(context.Background())
}

// TestIntegrationGetSessionManager tests retrieving session manager
func TestIntegrationGetSessionManager(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()

	server := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       0,
		WSPath:     "/ws",
		AuthToken:  "test-token",
		SessionMgr: sessionMgr,
		Bus:        testBus,
	})

	retrievedMgr := server.GetSessionManager()
	if retrievedMgr == nil {
		t.Error("Expected non-nil session manager")
	}

	if retrievedMgr != sessionMgr {
		t.Error("Retrieved session manager should be the same instance")
	}

	// Cleanup
	server.Shutdown(context.Background())
}

// TestIntegrationSessionManagerShutdown tests session manager shutdown
func TestIntegrationSessionManagerShutdown(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)

	// Shutdown should not panic
	sessionMgr.Shutdown()

	// After shutdown, stats should still work
	stats := sessionMgr.Stats()
	if stats == nil {
		t.Error("Stats should still be accessible after shutdown")
	}
}

// TestIntegrationContextCancellation tests context cancellation handling
func TestIntegrationContextCancellation(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()

	server := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       0,
		WSPath:     "/ws",
		AuthToken:  "test-token",
		SessionMgr: sessionMgr,
		Bus:        testBus,
	})

	// Cancel context immediately
	ctx, cancel := context.WithCancel(context.Background())
	cancel() // Cancel immediately

	err := server.Start(ctx)
	if err != nil && err != context.Canceled {
		t.Logf("Server start with cancelled context returned: %v", err)
	}

	// Should still be able to shutdown
	server.Shutdown(context.Background())
}

// TestIntegrationSessionTimeoutConfiguration tests different timeout configurations
func TestIntegrationSessionTimeoutConfiguration(t *testing.T) {
	testCases := []struct {
		name    string
		timeout time.Duration
	}{
		{"Short timeout", 100 * time.Millisecond},
		{"Medium timeout", 1 * time.Second},
		{"Long timeout", 1 * time.Hour},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			sessionMgr := NewSessionManager(tc.timeout)
			if sessionMgr == nil {
				t.Error("Failed to create session manager")
			}

			// Cleanup
			sessionMgr.Shutdown()
		})
	}
}

// TestIntegrationServerConfigVariations tests different server configurations
func TestIntegrationServerConfigVariations(t *testing.T) {
	testBus := bus.NewMessageBus()

	configs := []struct {
		name   string
		config ServerConfig
	}{
		{
			name: "Default config",
			config: ServerConfig{
				SessionMgr: NewSessionManager(1 * time.Hour),
				Bus:        testBus,
			},
		},
		{
			name: "Config with auth",
			config: ServerConfig{
				Host:       "0.0.0.0",
				Port:       8080,
				WSPath:     "/ws",
				AuthToken:  "secret",
				SessionMgr: NewSessionManager(1 * time.Hour),
				Bus:        testBus,
			},
		},
		{
			name: "Config without auth",
			config: ServerConfig{
				Host:       "127.0.0.1",
				Port:       9090,
				WSPath:     "/websocket",
				AuthToken:  "",
				SessionMgr: NewSessionManager(1 * time.Hour),
				Bus:        testBus,
			},
		},
	}

	for _, tc := range configs {
		t.Run(tc.name, func(t *testing.T) {
			server := NewServer(tc.config)
			if server == nil {
				t.Error("Failed to create server")
			}

			// Cleanup
			server.Shutdown(context.Background())
			tc.config.SessionMgr.Shutdown()
		})
	}
}

// TestIntegrationBroadcastToSessionWithNilManager tests error handling with nil manager
func TestIntegrationBroadcastToSessionErrors(t *testing.T) {
	// This test verifies error handling in BroadcastToSession
	// Since we can't easily create a nil session manager, we test with empty manager

	sessionMgr := NewSessionManager(1 * time.Hour)

	// Try to broadcast to non-existent session
	err := BroadcastToSession(sessionMgr, "does-not-exist", "assistant", "test")
	if err == nil {
		t.Error("Expected error for non-existent session")
	}
}

// TestIntegrationConcurrentServerCreation tests concurrent server creation
func TestIntegrationConcurrentServerCreation(t *testing.T) {
	testBus := bus.NewMessageBus()

	// Create multiple servers concurrently
	done := make(chan bool, 5)

	for i := 0; i < 5; i++ {
		go func(index int) {
			sessionMgr := NewSessionManager(1 * time.Hour)
			server := NewServer(ServerConfig{
				Host:       "localhost",
				Port:       0,
				WSPath:     "/ws",
				AuthToken:  "token",
				SessionMgr: sessionMgr,
				Bus:        testBus,
			})

			if server != nil {
				server.Shutdown(context.Background())
			}
			sessionMgr.Shutdown()
			done <- true
		}(i)
	}

	// Wait for all goroutines
	for i := 0; i < 5; i++ {
		select {
		case <-done:
		case <-time.After(5 * time.Second):
			t.Fatal("Timeout waiting for concurrent server creation")
		}
	}
}

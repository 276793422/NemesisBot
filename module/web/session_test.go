// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package web

import (
	"testing"
	"time"
)

func TestNewSessionManager(t *testing.T) {
	timeout := 1 * time.Hour
	sm := NewSessionManager(timeout)

	if sm == nil {
		t.Fatal("NewSessionManager returned nil")
	}

	if sm.timeout != timeout {
		t.Errorf("Expected timeout %v, got %v", timeout, sm.timeout)
	}

	// Should have no sessions initially
	if sm.GetActiveCount() != 0 {
		t.Errorf("Expected 0 sessions, got %d", sm.GetActiveCount())
	}
}

func TestSessionManager_GetActiveCount(t *testing.T) {
	sm := NewSessionManager(1 * time.Hour)

	// Initially empty
	count := sm.GetActiveCount()
	if count != 0 {
		t.Errorf("Expected 0 sessions, got %d", count)
	}
}

func TestSessionManager_GetAllSessions(t *testing.T) {
	sm := NewSessionManager(1 * time.Hour)

	// Initially empty
	sessions := sm.GetAllSessions()
	if len(sessions) != 0 {
		t.Errorf("Expected 0 sessions, got %d", len(sessions))
	}
}

func TestSessionManager_Stats(t *testing.T) {
	sm := NewSessionManager(1 * time.Hour)

	stats := sm.Stats()
	if stats == nil {
		t.Fatal("Stats should not be nil")
	}

	activeSessions, ok := stats["active_sessions"].(int)
	if !ok {
		t.Error("active_sessions should be an int")
	}

	if activeSessions != 0 {
		t.Errorf("Expected 0 active sessions, got %d", activeSessions)
	}
}

func TestSessionManager_RemoveSession_NonExistent(t *testing.T) {
	sm := NewSessionManager(1 * time.Hour)

	// Removing non-existent session should not panic
	sm.RemoveSession("non-existent-id")
}

func TestSessionManager_GetSession_NotFound(t *testing.T) {
	sm := NewSessionManager(1 * time.Hour)

	session, ok := sm.GetSession("non-existent-id")
	if ok {
		t.Error("Expected false for non-existent session")
	}
	if session != nil {
		t.Error("Expected nil session for non-existent ID")
	}
}

func TestSessionManager_Shutdown_Empty(t *testing.T) {
	sm := NewSessionManager(1 * time.Hour)

	// Should not panic on empty manager
	sm.Shutdown()

	// Should still be functional after shutdown
	stats := sm.Stats()
	if stats == nil {
		t.Error("Stats should still be available after shutdown")
	}
}

func TestSessionManager_GenerateSessionID(t *testing.T) {
	sm := NewSessionManager(1 * time.Hour)

	// Generate multiple IDs and check they're different
	ids := make(map[string]bool)
	for i := 0; i < 100; i++ {
		id := sm.generateSessionID()
		if id == "" {
			t.Error("Generated ID should not be empty")
		}
		if len(id) != 16 {
			t.Errorf("Expected ID length 16, got %d", len(id))
		}
		if ids[id] {
			t.Error("Generated duplicate ID")
		}
		ids[id] = true
	}
}

func TestSessionManager_StartCleanup(t *testing.T) {
	sm := NewSessionManager(100 * time.Millisecond)

	// Start cleanup goroutine
	sm.startCleanup()

	// Wait a bit to ensure cleanup goroutine started
	time.Sleep(200 * time.Millisecond)

	// Should not panic
	sm.Shutdown()
}

func TestSessionManager_CleanupInactiveSessions_Empty(t *testing.T) {
	sm := NewSessionManager(1 * time.Hour)

	// Should not panic on empty manager
	sm.cleanupInactiveSessions()
}

func TestNewServer(t *testing.T) {
	sm := NewSessionManager(1 * time.Hour)
	config := ServerConfig{
		Host:       "localhost",
		Port:       8080,
		WSPath:     "/ws",
		SessionMgr: sm,
	}

	server := NewServer(config)

	if server == nil {
		t.Fatal("NewServer returned nil")
	}

	if server.host != "localhost" {
		t.Errorf("Expected host 'localhost', got '%s'", server.host)
	}

	if server.port != 8080 {
		t.Errorf("Expected port 8080, got %d", server.port)
	}

	if server.wsPath != "/ws" {
		t.Errorf("Expected wsPath '/ws', got '%s'", server.wsPath)
	}

	if server.sessionMgr != sm {
		t.Error("Session manager not set correctly")
	}
}

func TestServer_IsRunning_Initial(t *testing.T) {
	sm := NewSessionManager(1 * time.Hour)
	server := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       8080,
		WSPath:     "/ws",
		SessionMgr: sm,
	})

	if server.IsRunning() {
		t.Error("Server should not be running initially")
	}
}

func TestServer_GetSessionManager(t *testing.T) {
	sm := NewSessionManager(1 * time.Hour)
	server := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       8080,
		WSPath:     "/ws",
		SessionMgr: sm,
	})

	retrieved := server.GetSessionManager()
	if retrieved != sm {
		t.Error("GetSessionManager should return the same manager")
	}
}

func TestServer_Shutdown_NotRunning(t *testing.T) {
	sm := NewSessionManager(1 * time.Hour)
	server := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       8080,
		WSPath:     "/ws",
		SessionMgr: sm,
	})

	// Should not error when shutting down non-running server
	err := server.Shutdown(nil)
	if err != nil {
		t.Errorf("Expected no error shutting down non-running server, got %v", err)
	}
}

func TestServer_Stop_NotRunning(t *testing.T) {
	sm := NewSessionManager(1 * time.Hour)
	server := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       8080,
		WSPath:     "/ws",
		SessionMgr: sm,
	})

	// Should not error when stopping non-running server
	err := server.Stop(nil)
	if err != nil {
		t.Errorf("Expected no error stopping non-running server, got %v", err)
	}
}

func TestBroadcastToSession(t *testing.T) {
	sm := NewSessionManager(1 * time.Hour)

	// Broadcasting to non-existent session should error
	err := BroadcastToSession(sm, "non-existent", "assistant", "test message")
	if err == nil {
		t.Error("Expected error broadcasting to non-existent session")
	}
}

func TestMin(t *testing.T) {
	tests := []struct {
		a, b     int
		expected int
	}{
		{1, 2, 1},
		{2, 1, 1},
		{5, 5, 5},
		{0, 100, 0},
		{-1, 1, -1},
	}

	for _, tt := range tests {
		result := min(tt.a, tt.b)
		if result != tt.expected {
			t.Errorf("min(%d, %d) = %d, expected %d", tt.a, tt.b, result, tt.expected)
		}
	}
}

func TestMessageTypeConstants(t *testing.T) {
	// Old MessageTypeMessage/MessageTypePing/MessageTypePong constants removed in Phase 3.
	// Protocol type/module/cmd string constants are now used directly.
	// ProtocolMessage tests are in test/unit/web/protocol_test.go
}

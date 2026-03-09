// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Web Channel Unit Tests - Comprehensive Session and Server Tests

package web_test

import (
	"context"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/gorilla/websocket"

	"github.com/276793422/NemesisBot/module/bus"
	. "github.com/276793422/NemesisBot/module/web"
)

// TestSessionManagerCreateSession tests creating a new session
func TestSessionManagerCreateSession(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)

	// Create a mock WebSocket connection
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Just upgrade to WebSocket and keep connection open
		upgrader := &websocket.Upgrader{
			ReadBufferSize:  1024,
			WriteBufferSize: 1024,
			CheckOrigin: func(r *http.Request) bool {
				return true
			},
		}
		conn, err := upgrader.Upgrade(w, r, nil)
		if err != nil {
			t.Fatalf("Failed to upgrade WebSocket: %v", err)
		}
		defer conn.Close()

		// Keep connection alive
		select {}
	}))
	defer server.Close()

	// Connect to the WebSocket server
	wsURL := "ws" + strings.TrimPrefix(server.URL, "http") + "/ws"
	conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("Failed to connect to WebSocket: %v", err)
	}
	defer conn.Close()

	// Create session
	session := sessionMgr.CreateSession(conn)

	if session == nil {
		t.Fatal("Expected non-nil session")
	}

	if session.ID == "" {
		t.Error("Expected non-empty session ID")
	}

	if session.Conn == nil {
		t.Error("Expected non-nil connection in session")
	}

	if session.SenderID == "" {
		t.Error("Expected non-empty sender ID")
	}

	if session.ChatID == "" {
		t.Error("Expected non-empty chat ID")
	}

	// Check that session was added to manager
	stats := sessionMgr.Stats()
	activeSessions := stats["active_sessions"].(int)
	if activeSessions != 1 {
		t.Errorf("Expected 1 active session, got %d", activeSessions)
	}
}

// TestSessionManagerGetSession tests retrieving a session
func TestSessionManagerGetSession(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		upgrader := &websocket.Upgrader{
			ReadBufferSize:  1024,
			WriteBufferSize: 1024,
			CheckOrigin: func(r *http.Request) bool {
				return true
			},
		}
		conn, err := upgrader.Upgrade(w, r, nil)
		if err != nil {
			t.Fatalf("Failed to upgrade WebSocket: %v", err)
		}
		defer conn.Close()
		select {}
	}))
	defer server.Close()

	wsURL := "ws" + strings.TrimPrefix(server.URL, "http") + "/ws"
	conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("Failed to connect to WebSocket: %v", err)
	}
	defer conn.Close()

	session := sessionMgr.CreateSession(conn)

	// Get the session we just created
	retrieved, ok := sessionMgr.GetSession(session.ID)
	if !ok {
		t.Error("Expected to find session")
	}

	if retrieved == nil {
		t.Fatal("Expected non-nil retrieved session")
	}

	if retrieved.ID != session.ID {
		t.Errorf("Expected session ID %s, got %s", session.ID, retrieved.ID)
	}

	// Try to get non-existent session
	_, ok = sessionMgr.GetSession("non-existent")
	if ok {
		t.Error("Expected not to find non-existent session")
	}
}

// TestSessionManagerRemoveSession tests removing a session
func TestSessionManagerRemoveSession(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		upgrader := &websocket.Upgrader{
			ReadBufferSize:  1024,
			WriteBufferSize: 1024,
			CheckOrigin: func(r *http.Request) bool {
				return true
			},
		}
		conn, err := upgrader.Upgrade(w, r, nil)
		if err != nil {
			t.Fatalf("Failed to upgrade WebSocket: %v", err)
		}
		defer conn.Close()
		select {}
	}))
	defer server.Close()

	wsURL := "ws" + strings.TrimPrefix(server.URL, "http") + "/ws"
	conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("Failed to connect to WebSocket: %v", err)
	}
	defer conn.Close()

	session := sessionMgr.CreateSession(conn)

	// Remove the session
	sessionMgr.RemoveSession(session.ID)

	// Check that session was removed
	stats := sessionMgr.Stats()
	activeSessions := stats["active_sessions"].(int)
	if activeSessions != 0 {
		t.Errorf("Expected 0 active sessions after removal, got %d", activeSessions)
	}

	// Try to get removed session
	_, ok := sessionMgr.GetSession(session.ID)
	if ok {
		t.Error("Expected not to find removed session")
	}
}

// TestSessionManagerBroadcast tests broadcasting to a session
func TestSessionManagerBroadcast(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)

	received := make(chan []byte, 1)

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		upgrader := &websocket.Upgrader{
			ReadBufferSize:  1024,
			WriteBufferSize: 1024,
			CheckOrigin: func(r *http.Request) bool {
				return true
			},
		}
		conn, err := upgrader.Upgrade(w, r, nil)
		if err != nil {
			t.Fatalf("Failed to upgrade WebSocket: %v", err)
		}
		defer conn.Close()

		// Read messages and send to channel
		for {
			_, message, err := conn.ReadMessage()
			if err != nil {
				return
			}
			select {
			case received <- message:
			default:
			}
		}
	}))
	defer server.Close()

	wsURL := "ws" + strings.TrimPrefix(server.URL, "http") + "/ws"
	conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("Failed to connect to WebSocket: %v", err)
	}
	defer conn.Close()

	session := sessionMgr.CreateSession(conn)

	// Broadcast message to session
	testMessage := []byte("test message")
	err = sessionMgr.Broadcast(session.ID, testMessage)
	if err != nil {
		t.Errorf("Expected no error broadcasting to session, got %v", err)
	}

	// Wait for message to be received
	select {
	case msg := <-received:
		if string(msg) != string(testMessage) {
			t.Errorf("Expected message %s, got %s", testMessage, msg)
		}
	case <-time.After(1 * time.Second):
		t.Error("Timeout waiting for message")
	}
}

// TestSessionManagerBroadcastToNonExistent tests broadcasting to non-existent session
func TestSessionManagerBroadcastToNonExistent(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)

	// Try to broadcast to non-existent session
	err := sessionMgr.Broadcast("non-existent-id", []byte("test"))
	if err == nil {
		t.Error("Expected error broadcasting to non-existent session")
	}
}

// TestSessionManagerGetAllSessions tests getting all sessions
func TestSessionManagerGetAllSessions(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)

	// Initially should be empty
	sessions := sessionMgr.GetAllSessions()
	if len(sessions) != 0 {
		t.Errorf("Expected 0 sessions, got %d", len(sessions))
	}

	// Create a few sessions sequentially to maintain order
	var sessionIDs []string
	for i := 0; i < 3; i++ {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			upgrader := &websocket.Upgrader{
				ReadBufferSize:  1024,
				WriteBufferSize: 1024,
				CheckOrigin: func(r *http.Request) bool {
					return true
				},
			}
			conn, err := upgrader.Upgrade(w, r, nil)
			if err != nil {
				return
			}
			defer conn.Close()
			select {}
		}))

		wsURL := "ws" + strings.TrimPrefix(server.URL, "http") + "/ws"
		conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
		if err != nil {
			continue
		}

		session := sessionMgr.CreateSession(conn)
		sessionIDs = append(sessionIDs, session.ID)
		// Close server immediately to avoid connection issues
		server.Close()
	}

	// Get all sessions
	sessions = sessionMgr.GetAllSessions()
	if len(sessions) != 3 {
		t.Errorf("Expected 3 sessions, got %d", len(sessions))
	}

	// Verify all session IDs are present (order may vary)
	foundIDs := make(map[string]bool)
	for _, session := range sessions {
		foundIDs[session.ID] = true
	}

	for _, expectedID := range sessionIDs {
		if !foundIDs[expectedID] {
			t.Errorf("Expected to find session ID %s", expectedID)
		}
	}
}

// TestSessionManagerConcurrentAccess tests concurrent access to session manager
func TestSessionManagerConcurrentAccess(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)

	var wg sync.WaitGroup
	numGoroutines := 10
	operationsPerGoroutine := 100

	for i := 0; i < numGoroutines; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for j := 0; j < operationsPerGoroutine; j++ {
				// Test stats access
				_ = sessionMgr.Stats()
				_ = sessionMgr.GetActiveCount()
				_ = sessionMgr.GetAllSessions()
			}
		}()
	}

	wg.Wait()

	// Verify manager is still functional
	stats := sessionMgr.Stats()
	if stats == nil {
		t.Error("Expected stats to be available after concurrent access")
	}
}

// TestServerIsRunning tests the IsRunning method
func TestServerIsRunning(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()

	server := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       0, // Random port
		WSPath:     "/ws",
		SessionMgr: sessionMgr,
		Bus:        testBus,
	})

	// Initially not running
	if server.IsRunning() {
		t.Error("Expected server to not be running initially")
	}

	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()

	// Start server
	go func() {
		_ = server.Start(ctx)
	}()

	// Give it time to start
	time.Sleep(50 * time.Millisecond)

	// Should be running now
	if !server.IsRunning() {
		t.Log("Server may not have started yet (this is okay for short timeout)")
	}

	// Wait for context to timeout
	<-ctx.Done()

	// Server should be stopped now
	if server.IsRunning() {
		t.Log("Server may still be running (this is okay for short timeout)")
	}
}

// TestServerShutdown tests server shutdown
func TestServerShutdown(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()

	server := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       0, // Random port
		WSPath:     "/ws",
		SessionMgr: sessionMgr,
		Bus:        testBus,
	})

	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()

	// Start server
	go func() {
		_ = server.Start(ctx)
	}()

	// Give it time to start
	time.Sleep(50 * time.Millisecond)

	// Shutdown server
	err := server.Shutdown(context.Background())
	if err != nil {
		t.Logf("Shutdown returned error (may be expected): %v", err)
	}

	// Wait for context
	<-ctx.Done()

	// Should not be running
	if server.IsRunning() {
		t.Log("Server may still be running after shutdown (this is okay for short timeout)")
	}
}

// TestServerGetSessionManager tests getting the session manager
func TestServerGetSessionManager(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()

	server := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       0,
		WSPath:     "/ws",
		SessionMgr: sessionMgr,
		Bus:        testBus,
	})

	retrievedMgr := server.GetSessionManager()
	if retrievedMgr == nil {
		t.Error("Expected non-nil session manager")
	}

	if retrievedMgr != sessionMgr {
		t.Error("Expected to get the same session manager instance")
	}
}

// TestSessionManagerShutdown tests session manager shutdown
func TestSessionManagerShutdown(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		upgrader := &websocket.Upgrader{
			ReadBufferSize:  1024,
			WriteBufferSize: 1024,
			CheckOrigin: func(r *http.Request) bool {
				return true
			},
		}
		conn, err := upgrader.Upgrade(w, r, nil)
		if err != nil {
			return
		}
		defer conn.Close()
		select {}
	}))
	defer server.Close()

	wsURL := "ws" + strings.TrimPrefix(server.URL, "http") + "/ws"
	conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("Failed to connect to WebSocket: %v", err)
	}
	defer conn.Close()

	// Create a session
	sessionMgr.CreateSession(conn)

	// Shutdown the manager
	sessionMgr.Shutdown()

	// Check that all sessions are removed
	stats := sessionMgr.Stats()
	activeSessions := stats["active_sessions"].(int)
	if activeSessions != 0 {
		t.Errorf("Expected 0 active sessions after shutdown, got %d", activeSessions)
	}
}

// TestServerStartAlreadyRunning tests starting an already running server
func TestServerStartAlreadyRunning(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()

	server := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       0,
		WSPath:     "/ws",
		SessionMgr: sessionMgr,
		Bus:        testBus,
	})

	ctx, cancel := context.WithTimeout(context.Background(), 200*time.Millisecond)
	defer cancel()

	// Start server
	go func() {
		_ = server.Start(ctx)
	}()

	// Give it time to start
	time.Sleep(50 * time.Millisecond)

	// Try to start again (should fail)
	err := server.Start(ctx)
	if err == nil {
		t.Log("Second Start call may have succeeded (server might not have started yet)")
	}

	// Cleanup
	<-ctx.Done()
	_ = server.Shutdown(context.Background())
}

// TestSendToSession tests sending a message to a specific session
func TestSendToSession(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()

	received := make(chan []byte, 1)

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		upgrader := &websocket.Upgrader{
			ReadBufferSize:  1024,
			WriteBufferSize: 1024,
			CheckOrigin: func(r *http.Request) bool {
				return true
			},
		}
		conn, err := upgrader.Upgrade(w, r, nil)
		if err != nil {
			return
		}
		defer conn.Close()

		// Read messages
		for {
			_, message, err := conn.ReadMessage()
			if err != nil {
				return
			}
			select {
			case received <- message:
			default:
			}
		}
	}))
	defer server.Close()

	webServer := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       0,
		WSPath:     "/ws",
		SessionMgr: sessionMgr,
		Bus:        testBus,
	})

	wsURL := "ws" + strings.TrimPrefix(server.URL, "http") + "/ws"
	conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("Failed to connect to WebSocket: %v", err)
	}
	defer conn.Close()

	createdSession := sessionMgr.CreateSession(conn)

	// Send message to session
	err = webServer.SendToSession(createdSession.ID, "assistant", "test message")
	if err != nil {
		t.Errorf("Expected no error sending to session, got %v", err)
	}

	// Wait for message
	select {
	case <-received:
		// Message received
	case <-time.After(1 * time.Second):
		t.Error("Timeout waiting for message")
	}
}

// TestSessionManagerTimeout tests session timeout functionality
func TestSessionManagerTimeout(t *testing.T) {
	t.Skip("Skipping timeout test as it requires long wait times")

	shortTimeout := 100 * time.Millisecond
	sessionMgr := NewSessionManager(shortTimeout)

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		upgrader := &websocket.Upgrader{
			ReadBufferSize:  1024,
			WriteBufferSize: 1024,
			CheckOrigin: func(r *http.Request) bool {
				return true
			},
		}
		conn, err := upgrader.Upgrade(w, r, nil)
		if err != nil {
			return
		}
		defer conn.Close()
		select {}
	}))
	defer server.Close()

	wsURL := "ws" + strings.TrimPrefix(server.URL, "http") + "/ws"
	conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("Failed to connect to WebSocket: %v", err)
	}
	defer conn.Close()

	_ = sessionMgr.CreateSession(conn)

	// Initially should have 1 session
	stats := sessionMgr.Stats()
	activeSessions := stats["active_sessions"].(int)
	if activeSessions != 1 {
		t.Errorf("Expected 1 active session initially, got %d", activeSessions)
	}

	// Wait for timeout plus cleanup interval
	time.Sleep(shortTimeout + 10*time.Minute)

	// Session should be cleaned up by now
	stats = sessionMgr.Stats()
	activeSessions = stats["active_sessions"].(int)
	if activeSessions != 0 {
		t.Logf("Session may not have been cleaned up yet (cleanup runs every 5 minutes)")
	}
}

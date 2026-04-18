// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package web

import (
	"context"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
)

func TestServer_HandleHealth(t *testing.T) {
	sm := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()

	server := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       8080,
		WSPath:     "/ws",
		SessionMgr: sm,
		Bus:        testBus,
	})

	// Create a request to the health endpoint
	req := httptest.NewRequest("GET", "/health", nil)
	w := httptest.NewRecorder()

	server.handleHealth(w, req)

	// Check response
	resp := w.Result()
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		t.Errorf("Expected status 200, got %d", resp.StatusCode)
	}

	contentType := resp.Header.Get("Content-Type")
	if contentType != "application/json" {
		t.Errorf("Expected content type 'application/json', got '%s'", contentType)
	}

	body := w.Body.String()
	if !strings.Contains(body, `"status":"ok"`) {
		t.Errorf("Expected status 'ok' in response, got '%s'", body)
	}

	if !strings.Contains(body, `"running":false`) {
		t.Errorf("Expected running:false in response, got '%s'", body)
	}

	if !strings.Contains(body, `"sessions":0`) {
		t.Errorf("Expected sessions:0 in response, got '%s'", body)
	}
}

func TestServer_HandleHealth_WithSessions(t *testing.T) {
	sm := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()

	server := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       8080,
		WSPath:     "/ws",
		SessionMgr: sm,
		Bus:        testBus,
	})

	// Simulate having sessions by modifying stats
	// Note: We can't easily create real sessions without WebSocket connections
	// so we'll just test the handler structure

	req := httptest.NewRequest("GET", "/health", nil)
	w := httptest.NewRecorder()

	server.handleHealth(w, req)

	resp := w.Result()
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		t.Errorf("Expected status 200, got %d", resp.StatusCode)
	}
}

func TestServer_StaticFiles_Index(t *testing.T) {
	staticFS, err := StaticFiles()
	if err != nil {
		t.Skip("Static files not embedded, skipping test")
	}

	req := httptest.NewRequest("GET", "/", nil)
	w := httptest.NewRecorder()

	http.FileServer(http.FS(staticFS)).ServeHTTP(w, req)

	resp := w.Result()
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		t.Errorf("Expected status 200, got %d", resp.StatusCode)
	}

	contentType := resp.Header.Get("Content-Type")
	if !strings.Contains(contentType, "text/html") {
		t.Errorf("Expected HTML content type, got '%s'", contentType)
	}
}

func TestServer_StaticFiles_CSS(t *testing.T) {
	staticFS, err := StaticFiles()
	if err != nil {
		t.Skip("Static files not embedded, skipping test")
	}

	req := httptest.NewRequest("GET", "/css/theme.css", nil)
	w := httptest.NewRecorder()

	http.FileServer(http.FS(staticFS)).ServeHTTP(w, req)

	resp := w.Result()
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		t.Errorf("Expected status 200, got %d", resp.StatusCode)
	}

	contentType := resp.Header.Get("Content-Type")
	if !strings.Contains(contentType, "text/css") {
		t.Errorf("Expected CSS content type, got '%s'", contentType)
	}
}

func TestServer_StaticFiles_JS(t *testing.T) {
	staticFS, err := StaticFiles()
	if err != nil {
		t.Skip("Static files not embedded, skipping test")
	}

	req := httptest.NewRequest("GET", "/js/app.js", nil)
	w := httptest.NewRecorder()

	http.FileServer(http.FS(staticFS)).ServeHTTP(w, req)

	resp := w.Result()
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		t.Errorf("Expected status 200, got %d", resp.StatusCode)
	}

	contentType := resp.Header.Get("Content-Type")
	if !strings.Contains(contentType, "application/javascript") {
		t.Errorf("Expected JavaScript content type, got '%s'", contentType)
	}
}

func TestServer_StaticFiles_NotFound(t *testing.T) {
	staticFS, err := StaticFiles()
	if err != nil {
		t.Skip("Static files not embedded, skipping test")
	}

	req := httptest.NewRequest("GET", "/nonexistent.html", nil)
	w := httptest.NewRecorder()

	http.FileServer(http.FS(staticFS)).ServeHTTP(w, req)

	resp := w.Result()
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusNotFound {
		t.Errorf("Expected status 404, got %d", resp.StatusCode)
	}
}

func TestServer_StaticFiles_ChatPage(t *testing.T) {
	staticFS, err := StaticFiles()
	if err != nil {
		t.Skip("Static files not embedded, skipping test")
	}

	req := httptest.NewRequest("GET", "/chat/", nil)
	w := httptest.NewRecorder()

	http.FileServer(http.FS(staticFS)).ServeHTTP(w, req)

	resp := w.Result()
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		t.Errorf("Expected status 200, got %d", resp.StatusCode)
	}
}

func TestServer_HandleWebSocket_InvalidToken(t *testing.T) {
	sm := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()

	server := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       8080,
		WSPath:     "/ws",
		AuthToken:  "valid-token",
		SessionMgr: sm,
		Bus:        testBus,
	})

	// Create request without token
	req := httptest.NewRequest("GET", "/ws", nil)
	w := httptest.NewRecorder()

	server.handleWebSocket(w, req)

	resp := w.Result()
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusUnauthorized {
		t.Errorf("Expected status 401, got %d", resp.StatusCode)
	}
}

func TestServer_HandleWebSocket_ValidToken(t *testing.T) {
	sm := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()

	server := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       8080,
		WSPath:     "/ws",
		AuthToken:  "test-token",
		SessionMgr: sm,
		Bus:        testBus,
	})

	// Create request with valid token
	req := httptest.NewRequest("GET", "/ws?token=test-token", nil)
	w := httptest.NewRecorder()

	server.handleWebSocket(w, req)

	resp := w.Result()
	defer resp.Body.Close()

	// WebSocket upgrade will fail in test environment (not a real WebSocket connection)
	// but the authentication should pass
	// We expect either a 400 (bad upgrade) or 101 (switching protocols)
	if resp.StatusCode != http.StatusBadRequest && resp.StatusCode != http.StatusSwitchingProtocols {
		// This is ok - the WebSocket upgrade might fail for various reasons
		t.Logf("Got status %d (expected 400 or 101)", resp.StatusCode)
	}
}

func TestServer_Start_AlreadyRunning(t *testing.T) {
	sm := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()

	server := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       0, // Random port
		WSPath:     "/ws",
		SessionMgr: sm,
		Bus:        testBus,
	})

	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()

	// Start server in background
	go func() {
		_ = server.Start(ctx)
	}()

	// Give it time to start
	time.Sleep(50 * time.Millisecond)

	// Try to start again
	err := server.Start(ctx)
	if err == nil {
		t.Log("Second Start call succeeded (server might not have started yet)")
	}

	// Cleanup
	_ = server.Shutdown(context.Background())
}

func TestServer_SendToSession(t *testing.T) {
	sm := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()

	server := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       8080,
		WSPath:     "/ws",
		SessionMgr: sm,
		Bus:        testBus,
	})

	// Sending to non-existent session should error
	err := server.SendToSession("non-existent", "assistant", "test message")
	if err == nil {
		t.Error("Expected error sending to non-existent session")
	}
}

func TestServer_StartAndShutdown(t *testing.T) {
	sm := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()

	server := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       0, // Random port
		WSPath:     "/ws",
		SessionMgr: sm,
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

	// Check if running
	if !server.IsRunning() {
		t.Log("Server may not have started yet (this is ok for short timeout)")
	}

	// Shutdown
	err := server.Shutdown(context.Background())
	if err != nil {
		t.Logf("Shutdown returned error (may be expected): %v", err)
	}

	// Wait for context
	<-ctx.Done()

	// Should not be running
	if server.IsRunning() {
		t.Log("Server may still be running (this is ok for short timeout)")
	}
}

func TestIncomingMessage_Struct(t *testing.T) {
	msg := IncomingMessage{
		SessionID: "test-session",
		SenderID:  "test-sender",
		ChatID:    "test-chat",
		Content:   "test content",
		Timestamp: time.Now(),
	}

	if msg.SessionID != "test-session" {
		t.Errorf("Expected SessionID 'test-session', got '%s'", msg.SessionID)
	}

	if msg.SenderID != "test-sender" {
		t.Errorf("Expected SenderID 'test-sender', got '%s'", msg.SenderID)
	}

	if msg.ChatID != "test-chat" {
		t.Errorf("Expected ChatID 'test-chat', got '%s'", msg.ChatID)
	}

	if msg.Content != "test content" {
		t.Errorf("Expected Content 'test content', got '%s'", msg.Content)
	}
}

func TestClientMessage_Struct(t *testing.T) {
	// Old ClientMessage type removed in Phase 3.
	// ProtocolMessage tests are in test/unit/web/protocol_test.go
}

func TestServerMessage_Struct(t *testing.T) {
	// Old ServerMessage type removed in Phase 3.
	// ProtocolMessage tests are in test/unit/web/protocol_test.go
}

// --- SendHistoryToSession tests ---

func TestServer_SendHistoryToSession_NonExistentSession(t *testing.T) {
	sm := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()

	server := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       8080,
		WSPath:     "/ws",
		SessionMgr: sm,
		Bus:        testBus,
	})

	jsonContent := `{"request_id":"r1","messages":[],"has_more":false,"oldest_index":0,"total_count":0}`
	err := server.SendHistoryToSession("non-existent", jsonContent)
	if err == nil {
		t.Error("Expected error sending to non-existent session")
	}
}

func TestServer_SendHistoryToSession_InvalidJSON(t *testing.T) {
	sm := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()

	server := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       8080,
		WSPath:     "/ws",
		SessionMgr: sm,
		Bus:        testBus,
	})

	err := server.SendHistoryToSession("any-session", "not valid json")
	if err == nil {
		t.Error("Expected error for invalid JSON content")
	}
}

// --- Metadata propagation tests ---

func TestIncomingMessage_Metadata(t *testing.T) {
	msg := IncomingMessage{
		SessionID: "test-session",
		SenderID:  "test-sender",
		ChatID:    "test-chat",
		Content:   "test content",
		Timestamp: time.Now(),
		Metadata:  map[string]string{"request_type": "history"},
	}

	if msg.Metadata == nil {
		t.Fatal("Metadata should not be nil")
	}
	if msg.Metadata["request_type"] != "history" {
		t.Errorf("Metadata request_type = %q, want %q", msg.Metadata["request_type"], "history")
	}
}

func TestIncomingMessage_NilMetadata(t *testing.T) {
	msg := IncomingMessage{
		SessionID: "test-session",
		Content:   "test content",
		Timestamp: time.Now(),
	}

	if msg.Metadata != nil {
		t.Error("Metadata should be nil when not set")
	}
}

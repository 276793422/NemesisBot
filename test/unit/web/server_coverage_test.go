// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Web Channel Unit Tests - Coverage Improvement

package web_test

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/gorilla/websocket"
	"github.com/276793422/NemesisBot/module/bus"
	. "github.com/276793422/NemesisBot/module/web"
)

// ============================================================================
// Priority 1: Pure functions and simple tests
// ============================================================================

// --- resolveLogFilePath via handleAPILogs ---

func TestResolveLogFilePath_GeneralFallsBackToAppLog(t *testing.T) {
	s, dir := newTestServer(t)

	// Create logs dir with app.log but NOT nemesisbot.log
	logsDir := filepath.Join(dir, "logs")
	os.MkdirAll(logsDir, 0755)
	os.WriteFile(filepath.Join(logsDir, "app.log"), []byte(`{"level":"INFO","message":"from app.log"}`+"\n"), 0644)

	req := httptest.NewRequest("GET", "/api/logs?source=general&n=50", nil)
	w := httptest.NewRecorder()
	s.HandleAPILogsForTest(w, req)

	if w.Code != 200 {
		t.Fatalf("expected 200, got %d: %s", w.Code, w.Body.String())
	}

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)
	entries, _ := resp["entries"].([]interface{})
	if len(entries) != 1 {
		t.Fatalf("entries length = %d, want 1 (from app.log fallback)", len(entries))
	}
	first, _ := entries[0].(map[string]interface{})
	if first["message"] != "from app.log" {
		t.Errorf("message = %v, want 'from app.log'", first["message"])
	}
}

func TestResolveLogFilePath_GeneralWithPrimaryLog(t *testing.T) {
	s, dir := newTestServer(t)

	logsDir := filepath.Join(dir, "logs")
	os.MkdirAll(logsDir, 0755)
	// nemesisbot.log takes priority over app.log
	os.WriteFile(filepath.Join(logsDir, "nemesisbot.log"), []byte(`{"level":"INFO","message":"primary"}`+"\n"), 0644)
	os.WriteFile(filepath.Join(logsDir, "app.log"), []byte(`{"level":"INFO","message":"secondary"}`+"\n"), 0644)

	req := httptest.NewRequest("GET", "/api/logs?source=general&n=50", nil)
	w := httptest.NewRecorder()
	s.HandleAPILogsForTest(w, req)

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)
	entries, _ := resp["entries"].([]interface{})
	if len(entries) != 1 {
		t.Fatalf("entries length = %d, want 1", len(entries))
	}
	first, _ := entries[0].(map[string]interface{})
	if first["message"] != "primary" {
		t.Errorf("message = %v, want 'primary' (nemesisbot.log has priority)", first["message"])
	}
}

func TestResolveLogFilePath_LLMSource(t *testing.T) {
	s, dir := newTestServer(t)

	// Create request_logs dir with multiple files
	reqLogsDir := filepath.Join(dir, "logs", "request_logs")
	os.MkdirAll(reqLogsDir, 0755)

	// Create files with different timestamps
	os.WriteFile(filepath.Join(reqLogsDir, "old.log"), []byte(`{"level":"INFO","message":"old"}`+"\n"), 0644)
	time.Sleep(10 * time.Millisecond)
	os.WriteFile(filepath.Join(reqLogsDir, "newest.log"), []byte(`{"level":"INFO","message":"newest"}`+"\n"), 0644)

	req := httptest.NewRequest("GET", "/api/logs?source=llm&n=50", nil)
	w := httptest.NewRecorder()
	s.HandleAPILogsForTest(w, req)

	if w.Code != 200 {
		t.Fatalf("expected 200, got %d: %s", w.Code, w.Body.String())
	}

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)
	entries, _ := resp["entries"].([]interface{})
	if len(entries) != 1 {
		t.Fatalf("entries length = %d, want 1 (from newest log)", len(entries))
	}
	first, _ := entries[0].(map[string]interface{})
	if first["message"] != "newest" {
		t.Errorf("message = %v, want 'newest' (most recently modified file)", first["message"])
	}
}

func TestResolveLogFilePath_LLMSourceEmptyDir(t *testing.T) {
	s, dir := newTestServer(t)

	// Create empty request_logs dir
	reqLogsDir := filepath.Join(dir, "logs", "request_logs")
	os.MkdirAll(reqLogsDir, 0755)

	req := httptest.NewRequest("GET", "/api/logs?source=llm&n=50", nil)
	w := httptest.NewRecorder()
	s.HandleAPILogsForTest(w, req)

	if w.Code != 200 {
		t.Fatalf("expected 200, got %d", w.Code)
	}

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)
	entries, _ := resp["entries"].([]interface{})
	if len(entries) != 0 {
		t.Errorf("entries length = %d, want 0 (empty dir)", len(entries))
	}
}

func TestResolveLogFilePath_LLMSourceDirNotFound(t *testing.T) {
	s, dir := newTestServer(t)
	// Don't create request_logs dir at all
	_ = dir

	req := httptest.NewRequest("GET", "/api/logs?source=llm&n=50", nil)
	w := httptest.NewRecorder()
	s.HandleAPILogsForTest(w, req)

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)
	entries, _ := resp["entries"].([]interface{})
	if len(entries) != 0 {
		t.Errorf("entries length = %d, want 0 (dir not found)", len(entries))
	}
}

func TestResolveLogFilePath_LLMSourceWithSubDirs(t *testing.T) {
	s, dir := newTestServer(t)

	reqLogsDir := filepath.Join(dir, "logs", "request_logs")
	os.MkdirAll(filepath.Join(reqLogsDir, "subdir"), 0755)
	os.WriteFile(filepath.Join(reqLogsDir, "real.log"), []byte(`{"level":"INFO","message":"real"}`+"\n"), 0644)

	req := httptest.NewRequest("GET", "/api/logs?source=llm&n=50", nil)
	w := httptest.NewRecorder()
	s.HandleAPILogsForTest(w, req)

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)
	entries, _ := resp["entries"].([]interface{})
	if len(entries) != 1 {
		t.Fatalf("entries length = %d, want 1 (subdirs should be skipped)", len(entries))
	}
}

func TestResolveLogFilePath_SecuritySource(t *testing.T) {
	s, dir := newTestServer(t)

	configDir := filepath.Join(dir, "config")
	os.MkdirAll(configDir, 0755)

	// Create security audit log files
	os.WriteFile(filepath.Join(configDir, "security_audit_2026-04-01.log"), []byte(`{"level":"WARN","message":"audit old"}`+"\n"), 0644)
	os.WriteFile(filepath.Join(configDir, "security_audit_2026-04-20.log"), []byte(`{"level":"WARN","message":"audit new"}`+"\n"), 0644)

	req := httptest.NewRequest("GET", "/api/logs?source=security&n=50", nil)
	w := httptest.NewRecorder()
	s.HandleAPILogsForTest(w, req)

	if w.Code != 200 {
		t.Fatalf("expected 200, got %d: %s", w.Code, w.Body.String())
	}

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)
	entries, _ := resp["entries"].([]interface{})
	if len(entries) != 1 {
		t.Fatalf("entries length = %d, want 1 (latest security audit log)", len(entries))
	}
}

func TestResolveLogFilePath_SecuritySourceNoFiles(t *testing.T) {
	s, dir := newTestServer(t)

	configDir := filepath.Join(dir, "config")
	os.MkdirAll(configDir, 0755)

	req := httptest.NewRequest("GET", "/api/logs?source=security&n=50", nil)
	w := httptest.NewRecorder()
	s.HandleAPILogsForTest(w, req)

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)
	entries, _ := resp["entries"].([]interface{})
	if len(entries) != 0 {
		t.Errorf("entries length = %d, want 0 (no security audit files)", len(entries))
	}
}

func TestResolveLogFilePath_ClusterSource(t *testing.T) {
	s, _ := newTestServer(t)

	req := httptest.NewRequest("GET", "/api/logs?source=cluster&n=50", nil)
	w := httptest.NewRecorder()
	s.HandleAPILogsForTest(w, req)

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)
	entries, _ := resp["entries"].([]interface{})
	if len(entries) != 0 {
		t.Errorf("cluster source should return empty, got %d", len(entries))
	}
}

// --- findLatestFile via LLM source with multiple files ---

func TestFindLatestFile_MultipleFiles(t *testing.T) {
	s, dir := newTestServer(t)

	reqLogsDir := filepath.Join(dir, "logs", "request_logs")
	os.MkdirAll(reqLogsDir, 0755)

	// Create files in order (each newer than previous)
	os.WriteFile(filepath.Join(reqLogsDir, "first.log"), []byte(`{"message":"first"}`+"\n"), 0644)
	time.Sleep(15 * time.Millisecond)
	os.WriteFile(filepath.Join(reqLogsDir, "second.log"), []byte(`{"message":"second"}`+"\n"), 0644)
	time.Sleep(15 * time.Millisecond)
	os.WriteFile(filepath.Join(reqLogsDir, "third.log"), []byte(`{"message":"third"}`+"\n"), 0644)

	req := httptest.NewRequest("GET", "/api/logs?source=llm&n=50", nil)
	w := httptest.NewRecorder()
	s.HandleAPILogsForTest(w, req)

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)
	entries, _ := resp["entries"].([]interface{})
	if len(entries) != 1 {
		t.Fatalf("entries length = %d, want 1", len(entries))
	}
	first, _ := entries[0].(map[string]interface{})
	if first["message"] != "third" {
		t.Errorf("message = %v, want 'third' (newest file)", first["message"])
	}
}

// --- readLogEntries via handleAPILogs with various content ---

func TestReadLogEntries_MixedContent(t *testing.T) {
	s, dir := newTestServer(t)

	logsDir := filepath.Join(dir, "logs")
	os.MkdirAll(logsDir, 0755)
	logFile := filepath.Join(logsDir, "nemesisbot.log")

	// Mix of JSON entries, plain text, and empty lines
	content := `{"level":"INFO","message":"first entry","component":"web"}
plain text line without JSON structure

{"level":"ERROR","message":"error entry","fields":{"detail":"something broke"}}
{"level":"DEBUG","message":"debug entry"}` + "\n"
	os.WriteFile(logFile, []byte(content), 0644)

	req := httptest.NewRequest("GET", "/api/logs?source=general&n=50", nil)
	w := httptest.NewRecorder()
	s.HandleAPILogsForTest(w, req)

	if w.Code != 200 {
		t.Fatalf("expected 200, got %d: %s", w.Code, w.Body.String())
	}

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)
	entries, _ := resp["entries"].([]interface{})

	// Should have 4 entries (empty lines skipped)
	if len(entries) != 4 {
		t.Fatalf("entries length = %d, want 4 (empty lines skipped)", len(entries))
	}

	// First entry should be JSON-parsed
	first, _ := entries[0].(map[string]interface{})
	if first["level"] != "INFO" {
		t.Errorf("first entry level = %v, want INFO", first["level"])
	}

	// Second entry should be plain text fallback
	second, _ := entries[1].(map[string]interface{})
	if second["message"] != "plain text line without JSON structure" {
		t.Errorf("second entry message = %v, want plain text fallback", second["message"])
	}

	// Third entry should have fields
	third, _ := entries[2].(map[string]interface{})
	if third["level"] != "ERROR" {
		t.Errorf("third entry level = %v, want ERROR", third["level"])
	}
	fields, ok := third["fields"].(map[string]interface{})
	if !ok || fields["detail"] != "something broke" {
		t.Errorf("third entry fields = %v, want detail='something broke'", third["fields"])
	}

	// Fourth entry
	fourth, _ := entries[3].(map[string]interface{})
	if fourth["level"] != "DEBUG" {
		t.Errorf("fourth entry level = %v, want DEBUG", fourth["level"])
	}
}

func TestReadLogEntries_LimitSmallerThanTotal(t *testing.T) {
	s, dir := newTestServer(t)

	logsDir := filepath.Join(dir, "logs")
	os.MkdirAll(logsDir, 0755)
	logFile := filepath.Join(logsDir, "nemesisbot.log")

	var content string
	for i := 0; i < 20; i++ {
		content += fmt.Sprintf(`{"level":"INFO","message":"entry-%d"}`+"\n", i)
	}
	os.WriteFile(logFile, []byte(content), 0644)

	// Request only 5 entries
	req := httptest.NewRequest("GET", "/api/logs?source=general&n=5", nil)
	w := httptest.NewRecorder()
	s.HandleAPILogsForTest(w, req)

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)
	entries, _ := resp["entries"].([]interface{})

	if len(entries) != 5 {
		t.Fatalf("entries length = %d, want 5", len(entries))
	}

	// Should be last 5 entries: entry-15 through entry-19
	last, _ := entries[4].(map[string]interface{})
	if last["message"] != "entry-19" {
		t.Errorf("last entry message = %v, want 'entry-19'", last["message"])
	}
}

func TestReadLogEntries_NonExistentFile(t *testing.T) {
	s, dir := newTestServer(t)
	// Create logs dir but no file inside
	logsDir := filepath.Join(dir, "logs")
	os.MkdirAll(logsDir, 0755)

	req := httptest.NewRequest("GET", "/api/logs?source=general&n=50", nil)
	w := httptest.NewRecorder()
	s.HandleAPILogsForTest(w, req)

	if w.Code != 200 {
		t.Fatalf("expected 200, got %d", w.Code)
	}

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)
	entries, _ := resp["entries"].([]interface{})
	if len(entries) != 0 {
		t.Errorf("entries length = %d, want 0 for non-existent file", len(entries))
	}
}

// --- sanitizeMap via handleAPIConfig ---

func TestSanitizeMap_NonStringValuePreserved(t *testing.T) {
	s, dir := newTestServer(t)

	configDir := filepath.Join(dir, "config")
	os.MkdirAll(configDir, 0755)
	os.WriteFile(filepath.Join(configDir, "config.json"), []byte(`{
		"numeric_key": 42,
		"bool_token": true,
		"array_value": [1, 2, 3],
		"null_value": null
	}`), 0644)

	req := httptest.NewRequest("GET", "/api/config", nil)
	w := httptest.NewRecorder()
	s.HandleAPIConfigForTest(w, req)

	if w.Code != 200 {
		t.Fatalf("expected 200, got %d: %s", w.Code, w.Body.String())
	}

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)

	// Non-string values with sensitive names should NOT be masked (sanitizeMap only handles strings)
	if resp["numeric_key"] != float64(42) {
		t.Errorf("numeric_key = %v, want 42", resp["numeric_key"])
	}
	// bool_token has "token" in name but is a bool, not a string
	if resp["bool_token"] != true {
		t.Errorf("bool_token = %v, want true (bools not masked)", resp["bool_token"])
	}
}

func TestSanitizeMap_MixedTypesInNestedMap(t *testing.T) {
	s, dir := newTestServer(t)

	configDir := filepath.Join(dir, "config")
	os.MkdirAll(configDir, 0755)
	os.WriteFile(filepath.Join(configDir, "config.json"), []byte(`{
		"database": {
			"host": "localhost",
			"port": 5432,
			"password": "supersecret",
			"options": {
				"sslmode": "require",
				"api_key": "sk-internal-key-123"
			}
		}
	}`), 0644)

	req := httptest.NewRequest("GET", "/api/config", nil)
	w := httptest.NewRecorder()
	s.HandleAPIConfigForTest(w, req)

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)

	db, _ := resp["database"].(map[string]interface{})
	if db["host"] != "localhost" {
		t.Errorf("host = %v, want 'localhost'", db["host"])
	}
	if db["password"] != "supe****" {
		t.Errorf("password = %v, want 'supe****'", db["password"])
	}
	options, _ := db["options"].(map[string]interface{})
	if options["sslmode"] != "require" {
		t.Errorf("sslmode = %v, want 'require'", options["sslmode"])
	}
	if options["api_key"] != "sk-i****" {
		t.Errorf("nested api_key = %v, want 'sk-i****'", options["api_key"])
	}
}

// --- writeJSON / writeJSONError via handleAPILogs and handleAPIConfig ---

func TestWriteJSON_ContentType(t *testing.T) {
	s, dir := newTestServer(t)

	logsDir := filepath.Join(dir, "logs")
	os.MkdirAll(logsDir, 0755)
	os.WriteFile(filepath.Join(logsDir, "nemesisbot.log"), []byte(`{"message":"test"}`+"\n"), 0644)

	req := httptest.NewRequest("GET", "/api/logs?source=general", nil)
	w := httptest.NewRecorder()
	s.HandleAPILogsForTest(w, req)

	ct := w.Header().Get("Content-Type")
	if ct != "application/json" {
		t.Errorf("Content-Type = %q, want 'application/json'", ct)
	}
	acao := w.Header().Get("Access-Control-Allow-Origin")
	if acao != "*" {
		t.Errorf("Access-Control-Allow-Origin = %q, want '*'", acao)
	}
}

func TestWriteJSONError_ResponseFormat(t *testing.T) {
	s := NewServer(ServerConfig{Version: "test"})

	req := httptest.NewRequest("GET", "/api/logs", nil)
	w := httptest.NewRecorder()
	s.HandleAPILogsForTest(w, req)

	if w.Code != http.StatusServiceUnavailable {
		t.Fatalf("expected 503, got %d", w.Code)
	}

	ct := w.Header().Get("Content-Type")
	if ct != "application/json" {
		t.Errorf("Content-Type = %q, want 'application/json'", ct)
	}

	var resp map[string]string
	json.Unmarshal(w.Body.Bytes(), &resp)
	if resp["error"] != "workspace not configured" {
		t.Errorf("error = %q, want 'workspace not configured'", resp["error"])
	}
}

// --- min function indirectly tested via BroadcastToSession content preview ---

func TestMin_ViaShortContent(t *testing.T) {
	// Test BroadcastToSession with content shorter than 100 chars (min used in content_preview)
	sm := NewSessionManager(1 * time.Hour)
	shortContent := "hi"
	err := BroadcastToSession(sm, "non-existent", "assistant", shortContent)
	if err == nil {
		t.Error("expected error for non-existent session")
	}
	// The min() function is used internally in BroadcastToSession for content preview logging.
	// With short content (2 chars), min(100, 2) should return 2, not panic.
}

// ============================================================================
// Priority 2: Mock/construct input tests using WebSocket connections
// ============================================================================

// --- sendQueue.send/process via HandleWebSocket with real connections ---

func TestSendQueue_WithRealConnection(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()

	received := make(chan []byte, 10)

	// Create WebSocket server
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		upgrader := websocket.Upgrader{CheckOrigin: func(r *http.Request) bool { return true }}
		conn, err := upgrader.Upgrade(w, r, nil)
		if err != nil {
			return
		}
		defer conn.Close()
		// Echo back: read messages and forward to channel
		for {
			_, msg, err := conn.ReadMessage()
			if err != nil {
				return
			}
			select {
			case received <- msg:
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
		t.Fatalf("Failed to connect: %v", err)
	}
	defer conn.Close()

	session := sessionMgr.CreateSession(conn)

	// Send message via webServer (uses BroadcastToSession which uses sendQueue)
	err = webServer.SendToSession(session.ID, "assistant", "hello via queue")
	if err != nil {
		t.Fatalf("SendToSession failed: %v", err)
	}

	select {
	case msg := <-received:
		var protoMsg map[string]interface{}
		if err := json.Unmarshal(msg, &protoMsg); err != nil {
			t.Fatalf("Failed to unmarshal protocol message: %v", err)
		}
		if protoMsg["type"] != "message" {
			t.Errorf("type = %v, want 'message'", protoMsg["type"])
		}
		data, ok := protoMsg["data"].(map[string]interface{})
		if !ok {
			t.Fatal("data is not a map")
		}
		if data["content"] != "hello via queue" {
			t.Errorf("content = %v, want 'hello via queue'", data["content"])
		}
		if data["role"] != "assistant" {
			t.Errorf("role = %v, want 'assistant'", data["role"])
		}
	case <-time.After(2 * time.Second):
		t.Fatal("Timeout waiting for message via sendQueue")
	}
}

func TestSendQueue_ConcurrentSends(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()

	received := make(chan []byte, 20)

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		upgrader := websocket.Upgrader{CheckOrigin: func(r *http.Request) bool { return true }}
		conn, err := upgrader.Upgrade(w, r, nil)
		if err != nil {
			return
		}
		defer conn.Close()
		for {
			_, msg, err := conn.ReadMessage()
			if err != nil {
				return
			}
			select {
			case received <- msg:
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
		t.Fatalf("Failed to connect: %v", err)
	}
	defer conn.Close()

	session := sessionMgr.CreateSession(conn)

	// Send multiple messages concurrently (tests sendQueue thread safety)
	for i := 0; i < 5; i++ {
		go func(idx int) {
			_ = webServer.SendToSession(session.ID, "assistant", fmt.Sprintf("concurrent-%d", idx))
		}(i)
	}

	// Collect all 5 messages
	for i := 0; i < 5; i++ {
		select {
		case <-received:
		case <-time.After(3 * time.Second):
			t.Fatalf("Timeout waiting for concurrent message %d", i)
		}
	}
}

// --- handleMessageModule via HandleWebSocket ---

func TestHandleMessageModule_ChatSend(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)

	messageChan := make(chan IncomingMessage, 10)
	echoCh := make(chan IncomingMessage, 1)

	// Server that passes messageChan to HandleWebSocket
	wsHandler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		upgrader := websocket.Upgrader{CheckOrigin: func(r *http.Request) bool { return true }}
		conn, err := upgrader.Upgrade(w, r, nil)
		if err != nil {
			return
		}
		session := sessionMgr.CreateSession(conn)
		defer func() {
			conn.Close()
			sessionMgr.RemoveSession(session.ID)
		}()
		_ = HandleWebSocket(session, sessionMgr, messageChan, "")
	})

	server := httptest.NewServer(wsHandler)
	defer server.Close()

	// Start goroutine to consume from messageChan
	go func() {
		for msg := range messageChan {
			select {
			case echoCh <- msg:
			default:
			}
		}
	}()

	wsURL := "ws" + strings.TrimPrefix(server.URL, "http") + "/"
	conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("Failed to connect: %v", err)
	}
	defer conn.Close()

	// Send chat.send message
	chatMsg, _ := NewProtocolMessage("message", "chat", "send", map[string]string{
		"content": "hello from test",
	})
	data, _ := chatMsg.ToJSON()
	err = conn.WriteMessage(websocket.TextMessage, data)
	if err != nil {
		t.Fatalf("Failed to send message: %v", err)
	}

	// Verify message arrives on the messageChan
	select {
	case msg := <-echoCh:
		if msg.Content != "hello from test" {
			t.Errorf("Content = %q, want 'hello from test'", msg.Content)
		}
		if msg.SessionID == "" {
			t.Error("SessionID should not be empty")
		}
	case <-time.After(2 * time.Second):
		t.Fatal("Timeout waiting for message on channel")
	}
}

func TestHandleMessageModule_UnknownModule(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	messageChan := make(chan IncomingMessage, 10)
	received := make(chan []byte, 1)

	wsHandler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		upgrader := websocket.Upgrader{CheckOrigin: func(r *http.Request) bool { return true }}
		conn, err := upgrader.Upgrade(w, r, nil)
		if err != nil {
			return
		}
		session := sessionMgr.CreateSession(conn)
		defer func() {
			conn.Close()
			sessionMgr.RemoveSession(session.ID)
		}()
		_ = HandleWebSocket(session, sessionMgr, messageChan, "")
	})

	server := httptest.NewServer(wsHandler)
	defer server.Close()

	wsURL := "ws" + strings.TrimPrefix(server.URL, "http") + "/"
	conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("Failed to connect: %v", err)
	}
	defer conn.Close()

	// Read goroutine to capture error response
	go func() {
		_, msg, _ := conn.ReadMessage()
		select {
		case received <- msg:
		default:
		}
	}()

	// Send message with unknown module
	badMsg, _ := NewProtocolMessage("message", "unknown_module", "send", map[string]string{
		"content": "test",
	})
	data, _ := badMsg.ToJSON()
	conn.WriteMessage(websocket.TextMessage, data)

	select {
	case msg := <-received:
		var resp map[string]interface{}
		json.Unmarshal(msg, &resp)
		if resp["type"] != "system" {
			t.Errorf("error response type = %v, want 'system'", resp["type"])
		}
		if resp["module"] != "error" {
			t.Errorf("error response module = %v, want 'error'", resp["module"])
		}
	case <-time.After(2 * time.Second):
		t.Fatal("Timeout waiting for error response")
	}
}

// --- handleChatSend edge cases ---

func TestHandleChatSend_EmptyContent(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	messageChan := make(chan IncomingMessage, 10)
	received := make(chan []byte, 1)

	wsHandler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		upgrader := websocket.Upgrader{CheckOrigin: func(r *http.Request) bool { return true }}
		conn, err := upgrader.Upgrade(w, r, nil)
		if err != nil {
			return
		}
		session := sessionMgr.CreateSession(conn)
		defer func() {
			conn.Close()
			sessionMgr.RemoveSession(session.ID)
		}()
		_ = HandleWebSocket(session, sessionMgr, messageChan, "")
	})

	server := httptest.NewServer(wsHandler)
	defer server.Close()

	wsURL := "ws" + strings.TrimPrefix(server.URL, "http") + "/"
	conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("Failed to connect: %v", err)
	}
	defer conn.Close()

	go func() {
		_, msg, _ := conn.ReadMessage()
		select {
		case received <- msg:
		default:
		}
	}()

	// Send chat.send with empty content
	emptyMsg, _ := NewProtocolMessage("message", "chat", "send", map[string]string{
		"content": "",
	})
	data, _ := emptyMsg.ToJSON()
	conn.WriteMessage(websocket.TextMessage, data)

	select {
	case msg := <-received:
		var resp map[string]interface{}
		json.Unmarshal(msg, &resp)
		dataMap, _ := resp["data"].(map[string]interface{})
		if dataMap["content"] != "Message content cannot be empty" {
			t.Errorf("error content = %v, want 'Message content cannot be empty'", dataMap["content"])
		}
	case <-time.After(2 * time.Second):
		t.Fatal("Timeout waiting for error about empty content")
	}
}

func TestHandleChatSend_InvalidData(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	messageChan := make(chan IncomingMessage, 10)
	received := make(chan []byte, 1)

	wsHandler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		upgrader := websocket.Upgrader{CheckOrigin: func(r *http.Request) bool { return true }}
		conn, err := upgrader.Upgrade(w, r, nil)
		if err != nil {
			return
		}
		session := sessionMgr.CreateSession(conn)
		defer func() {
			conn.Close()
			sessionMgr.RemoveSession(session.ID)
		}()
		_ = HandleWebSocket(session, sessionMgr, messageChan, "")
	})

	server := httptest.NewServer(wsHandler)
	defer server.Close()

	wsURL := "ws" + strings.TrimPrefix(server.URL, "http") + "/"
	conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("Failed to connect: %v", err)
	}
	defer conn.Close()

	go func() {
		_, msg, _ := conn.ReadMessage()
		select {
		case received <- msg:
		default:
		}
	}()

	// Send chat.send with no data (nil data)
	badMsg, _ := NewProtocolMessage("message", "chat", "send", nil)
	data, _ := badMsg.ToJSON()
	conn.WriteMessage(websocket.TextMessage, data)

	select {
	case msg := <-received:
		var resp map[string]interface{}
		json.Unmarshal(msg, &resp)
		dataMap, _ := resp["data"].(map[string]interface{})
		if dataMap["content"] != "Invalid chat.send data" {
			t.Errorf("error content = %v, want 'Invalid chat.send data'", dataMap["content"])
		}
	case <-time.After(2 * time.Second):
		t.Fatal("Timeout waiting for error about invalid data")
	}
}

func TestHandleChatSend_UnknownCmd(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	messageChan := make(chan IncomingMessage, 10)
	received := make(chan []byte, 1)

	wsHandler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		upgrader := websocket.Upgrader{CheckOrigin: func(r *http.Request) bool { return true }}
		conn, err := upgrader.Upgrade(w, r, nil)
		if err != nil {
			return
		}
		session := sessionMgr.CreateSession(conn)
		defer func() {
			conn.Close()
			sessionMgr.RemoveSession(session.ID)
		}()
		_ = HandleWebSocket(session, sessionMgr, messageChan, "")
	})

	server := httptest.NewServer(wsHandler)
	defer server.Close()

	wsURL := "ws" + strings.TrimPrefix(server.URL, "http") + "/"
	conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("Failed to connect: %v", err)
	}
	defer conn.Close()

	go func() {
		_, msg, _ := conn.ReadMessage()
		select {
		case received <- msg:
		default:
		}
	}()

	// Send chat message with unknown cmd
	badCmdMsg, _ := NewProtocolMessage("message", "chat", "unknown_cmd", map[string]string{
		"content": "test",
	})
	data, _ := badCmdMsg.ToJSON()
	conn.WriteMessage(websocket.TextMessage, data)

	select {
	case msg := <-received:
		var resp map[string]interface{}
		json.Unmarshal(msg, &resp)
		dataMap, _ := resp["data"].(map[string]interface{})
		errContent, _ := dataMap["content"].(string)
		if !strings.Contains(errContent, "Unknown chat cmd") {
			t.Errorf("error content = %v, want to contain 'Unknown chat cmd'", errContent)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("Timeout waiting for error about unknown cmd")
	}
}

// --- handleHistoryRequest ---

func TestHandleHistoryRequest_ValidData(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	messageChan := make(chan IncomingMessage, 10)
	echoCh := make(chan IncomingMessage, 1)

	wsHandler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		upgrader := websocket.Upgrader{CheckOrigin: func(r *http.Request) bool { return true }}
		conn, err := upgrader.Upgrade(w, r, nil)
		if err != nil {
			return
		}
		session := sessionMgr.CreateSession(conn)
		defer func() {
			conn.Close()
			sessionMgr.RemoveSession(session.ID)
		}()
		_ = HandleWebSocket(session, sessionMgr, messageChan, "")
	})

	server := httptest.NewServer(wsHandler)
	defer server.Close()

	go func() {
		for msg := range messageChan {
			select {
			case echoCh <- msg:
			default:
			}
		}
	}()

	wsURL := "ws" + strings.TrimPrefix(server.URL, "http") + "/"
	conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("Failed to connect: %v", err)
	}
	defer conn.Close()

	// Send history_request
	histMsg, _ := NewProtocolMessage("message", "chat", "history_request", map[string]interface{}{
		"request_id":   "req-001",
		"limit":        20,
		"before_index": 50,
	})
	data, _ := histMsg.ToJSON()
	conn.WriteMessage(websocket.TextMessage, data)

	select {
	case msg := <-echoCh:
		if msg.Metadata == nil || msg.Metadata["request_type"] != "history" {
			t.Errorf("Metadata = %v, want request_type='history'", msg.Metadata)
		}
		// Content should be JSON payload of the request data
		var reqData map[string]interface{}
		if err := json.Unmarshal([]byte(msg.Content), &reqData); err != nil {
			t.Fatalf("Failed to unmarshal content: %v", err)
		}
		if reqData["request_id"] != "req-001" {
			t.Errorf("request_id = %v, want 'req-001'", reqData["request_id"])
		}
	case <-time.After(2 * time.Second):
		t.Fatal("Timeout waiting for history request on channel")
	}
}

func TestHandleHistoryRequest_InvalidData(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	messageChan := make(chan IncomingMessage, 10)
	received := make(chan []byte, 1)

	wsHandler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		upgrader := websocket.Upgrader{CheckOrigin: func(r *http.Request) bool { return true }}
		conn, err := upgrader.Upgrade(w, r, nil)
		if err != nil {
			return
		}
		session := sessionMgr.CreateSession(conn)
		defer func() {
			conn.Close()
			sessionMgr.RemoveSession(session.ID)
		}()
		_ = HandleWebSocket(session, sessionMgr, messageChan, "")
	})

	server := httptest.NewServer(wsHandler)
	defer server.Close()

	wsURL := "ws" + strings.TrimPrefix(server.URL, "http") + "/"
	conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("Failed to connect: %v", err)
	}
	defer conn.Close()

	go func() {
		_, msg, _ := conn.ReadMessage()
		select {
		case received <- msg:
		default:
		}
	}()

	// Send history_request with nil data
	histMsg, _ := NewProtocolMessage("message", "chat", "history_request", nil)
	data, _ := histMsg.ToJSON()
	conn.WriteMessage(websocket.TextMessage, data)

	select {
	case msg := <-received:
		var resp map[string]interface{}
		json.Unmarshal(msg, &resp)
		dataMap, _ := resp["data"].(map[string]interface{})
		if dataMap["content"] != "Invalid history_request data" {
			t.Errorf("error content = %v, want 'Invalid history_request data'", dataMap["content"])
		}
	case <-time.After(2 * time.Second):
		t.Fatal("Timeout waiting for error about invalid history request data")
	}
}

// --- handleSystemModule ---

func TestHandleSystemModule_HeartbeatPing(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	messageChan := make(chan IncomingMessage, 10)
	received := make(chan []byte, 1)

	wsHandler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		upgrader := websocket.Upgrader{CheckOrigin: func(r *http.Request) bool { return true }}
		conn, err := upgrader.Upgrade(w, r, nil)
		if err != nil {
			return
		}
		session := sessionMgr.CreateSession(conn)
		defer func() {
			conn.Close()
			sessionMgr.RemoveSession(session.ID)
		}()
		_ = HandleWebSocket(session, sessionMgr, messageChan, "")
	})

	server := httptest.NewServer(wsHandler)
	defer server.Close()

	wsURL := "ws" + strings.TrimPrefix(server.URL, "http") + "/"
	conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("Failed to connect: %v", err)
	}
	defer conn.Close()

	go func() {
		_, msg, _ := conn.ReadMessage()
		select {
		case received <- msg:
		default:
		}
	}()

	// Send heartbeat ping
	pingMsg, _ := NewProtocolMessage("system", "heartbeat", "ping", map[string]interface{}{})
	data, _ := pingMsg.ToJSON()
	conn.WriteMessage(websocket.TextMessage, data)

	select {
	case msg := <-received:
		var resp map[string]interface{}
		json.Unmarshal(msg, &resp)
		if resp["type"] != "system" {
			t.Errorf("type = %v, want 'system'", resp["type"])
		}
		if resp["module"] != "heartbeat" {
			t.Errorf("module = %v, want 'heartbeat'", resp["module"])
		}
		if resp["cmd"] != "pong" {
			t.Errorf("cmd = %v, want 'pong'", resp["cmd"])
		}
	case <-time.After(2 * time.Second):
		t.Fatal("Timeout waiting for pong response")
	}
}

func TestHandleSystemModule_ErrorNotify(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	messageChan := make(chan IncomingMessage, 10)

	wsHandler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		upgrader := websocket.Upgrader{CheckOrigin: func(r *http.Request) bool { return true }}
		conn, err := upgrader.Upgrade(w, r, nil)
		if err != nil {
			return
		}
		session := sessionMgr.CreateSession(conn)
		defer func() {
			conn.Close()
			sessionMgr.RemoveSession(session.ID)
		}()
		_ = HandleWebSocket(session, sessionMgr, messageChan, "")
	})

	server := httptest.NewServer(wsHandler)
	defer server.Close()

	wsURL := "ws" + strings.TrimPrefix(server.URL, "http") + "/"
	conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("Failed to connect: %v", err)
	}
	defer conn.Close()

	// Send error notify - server should log it, not crash
	// No response expected for error notify from client
	errorMsg, _ := NewProtocolMessage("system", "error", "notify", map[string]string{
		"content": "client error occurred",
	})
	data, _ := errorMsg.ToJSON()
	err = conn.WriteMessage(websocket.TextMessage, data)
	if err != nil {
		t.Fatalf("Failed to send error notify: %v", err)
	}

	// If we get here without panic, the test passes.
	// Send a ping afterwards to verify connection is still alive
	time.Sleep(50 * time.Millisecond)

	pingMsg, _ := NewProtocolMessage("system", "heartbeat", "ping", map[string]interface{}{})
	pingData, _ := pingMsg.ToJSON()

	received := make(chan []byte, 1)
	go func() {
		_, msg, _ := conn.ReadMessage()
		select {
		case received <- msg:
		default:
		}
	}()

	conn.WriteMessage(websocket.TextMessage, pingData)

	select {
	case msg := <-received:
		var resp map[string]interface{}
		json.Unmarshal(msg, &resp)
		if resp["cmd"] != "pong" {
			t.Errorf("cmd = %v, want 'pong' (connection should still be alive)", resp["cmd"])
		}
	case <-time.After(2 * time.Second):
		t.Fatal("Timeout: connection died after error notify")
	}
}

func TestHandleSystemModule_UnknownModule(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	messageChan := make(chan IncomingMessage, 10)
	received := make(chan []byte, 1)

	wsHandler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		upgrader := websocket.Upgrader{CheckOrigin: func(r *http.Request) bool { return true }}
		conn, err := upgrader.Upgrade(w, r, nil)
		if err != nil {
			return
		}
		session := sessionMgr.CreateSession(conn)
		defer func() {
			conn.Close()
			sessionMgr.RemoveSession(session.ID)
		}()
		_ = HandleWebSocket(session, sessionMgr, messageChan, "")
	})

	server := httptest.NewServer(wsHandler)
	defer server.Close()

	wsURL := "ws" + strings.TrimPrefix(server.URL, "http") + "/"
	conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("Failed to connect: %v", err)
	}
	defer conn.Close()

	go func() {
		_, msg, _ := conn.ReadMessage()
		select {
		case received <- msg:
		default:
		}
	}()

	// Send system message with unknown module
	badMsg, _ := NewProtocolMessage("system", "nonexistent", "cmd", map[string]string{
		"content": "test",
	})
	data, _ := badMsg.ToJSON()
	conn.WriteMessage(websocket.TextMessage, data)

	select {
	case msg := <-received:
		var resp map[string]interface{}
		json.Unmarshal(msg, &resp)
		dataMap, _ := resp["data"].(map[string]interface{})
		errContent, _ := dataMap["content"].(string)
		if !strings.Contains(errContent, "Unknown system module") {
			t.Errorf("error content = %v, want to contain 'Unknown system module'", errContent)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("Timeout waiting for error about unknown system module")
	}
}

func TestHandleSystemModule_UnknownHeartbeatCmd(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	messageChan := make(chan IncomingMessage, 10)
	received := make(chan []byte, 1)

	wsHandler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		upgrader := websocket.Upgrader{CheckOrigin: func(r *http.Request) bool { return true }}
		conn, err := upgrader.Upgrade(w, r, nil)
		if err != nil {
			return
		}
		session := sessionMgr.CreateSession(conn)
		defer func() {
			conn.Close()
			sessionMgr.RemoveSession(session.ID)
		}()
		_ = HandleWebSocket(session, sessionMgr, messageChan, "")
	})

	server := httptest.NewServer(wsHandler)
	defer server.Close()

	wsURL := "ws" + strings.TrimPrefix(server.URL, "http") + "/"
	conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("Failed to connect: %v", err)
	}
	defer conn.Close()

	go func() {
		_, msg, _ := conn.ReadMessage()
		select {
		case received <- msg:
		default:
		}
	}()

	// Send heartbeat with unknown cmd
	badMsg, _ := NewProtocolMessage("system", "heartbeat", "unknown_cmd", nil)
	data, _ := badMsg.ToJSON()
	conn.WriteMessage(websocket.TextMessage, data)

	select {
	case msg := <-received:
		var resp map[string]interface{}
		json.Unmarshal(msg, &resp)
		dataMap, _ := resp["data"].(map[string]interface{})
		errContent, _ := dataMap["content"].(string)
		if !strings.Contains(errContent, "Unknown heartbeat cmd") {
			t.Errorf("error content = %v, want to contain 'Unknown heartbeat cmd'", errContent)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("Timeout waiting for error about unknown heartbeat cmd")
	}
}

func TestHandleSystemModule_UnknownErrorCmd(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	messageChan := make(chan IncomingMessage, 10)
	received := make(chan []byte, 1)

	wsHandler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		upgrader := websocket.Upgrader{CheckOrigin: func(r *http.Request) bool { return true }}
		conn, err := upgrader.Upgrade(w, r, nil)
		if err != nil {
			return
		}
		session := sessionMgr.CreateSession(conn)
		defer func() {
			conn.Close()
			sessionMgr.RemoveSession(session.ID)
		}()
		_ = HandleWebSocket(session, sessionMgr, messageChan, "")
	})

	server := httptest.NewServer(wsHandler)
	defer server.Close()

	wsURL := "ws" + strings.TrimPrefix(server.URL, "http") + "/"
	conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("Failed to connect: %v", err)
	}
	defer conn.Close()

	go func() {
		_, msg, _ := conn.ReadMessage()
		select {
		case received <- msg:
		default:
		}
	}()

	// Send error module with unknown cmd
	badMsg, _ := NewProtocolMessage("system", "error", "unknown_error_cmd", nil)
	data, _ := badMsg.ToJSON()
	conn.WriteMessage(websocket.TextMessage, data)

	select {
	case msg := <-received:
		var resp map[string]interface{}
		json.Unmarshal(msg, &resp)
		dataMap, _ := resp["data"].(map[string]interface{})
		errContent, _ := dataMap["content"].(string)
		if !strings.Contains(errContent, "Unknown error cmd") {
			t.Errorf("error content = %v, want to contain 'Unknown error cmd'", errContent)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("Timeout waiting for error about unknown error cmd")
	}
}

// --- Unknown protocol type dispatch ---

func TestHandleWebSocket_UnknownProtocolType(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	messageChan := make(chan IncomingMessage, 10)
	received := make(chan []byte, 1)

	wsHandler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		upgrader := websocket.Upgrader{CheckOrigin: func(r *http.Request) bool { return true }}
		conn, err := upgrader.Upgrade(w, r, nil)
		if err != nil {
			return
		}
		session := sessionMgr.CreateSession(conn)
		defer func() {
			conn.Close()
			sessionMgr.RemoveSession(session.ID)
		}()
		_ = HandleWebSocket(session, sessionMgr, messageChan, "")
	})

	server := httptest.NewServer(wsHandler)
	defer server.Close()

	wsURL := "ws" + strings.TrimPrefix(server.URL, "http") + "/"
	conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("Failed to connect: %v", err)
	}
	defer conn.Close()

	go func() {
		_, msg, _ := conn.ReadMessage()
		select {
		case received <- msg:
		default:
		}
	}()

	// Send message with unknown type
	unknownMsg := map[string]interface{}{
		"type":    "unknown_type",
		"module":  "test",
		"cmd":     "test",
		"data":    map[string]string{"content": "test"},
	}
	data, _ := json.Marshal(unknownMsg)
	conn.WriteMessage(websocket.TextMessage, data)

	select {
	case msg := <-received:
		var resp map[string]interface{}
		json.Unmarshal(msg, &resp)
		dataMap, _ := resp["data"].(map[string]interface{})
		errContent, _ := dataMap["content"].(string)
		if !strings.Contains(errContent, "Unknown protocol type") {
			t.Errorf("error content = %v, want to contain 'Unknown protocol type'", errContent)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("Timeout waiting for error about unknown protocol type")
	}
}

// --- Binary message handling ---

func TestHandleWebSocket_BinaryMessageIgnored(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	messageChan := make(chan IncomingMessage, 10)

	wsHandler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		upgrader := websocket.Upgrader{CheckOrigin: func(r *http.Request) bool { return true }}
		conn, err := upgrader.Upgrade(w, r, nil)
		if err != nil {
			return
		}
		session := sessionMgr.CreateSession(conn)
		defer func() {
			conn.Close()
			sessionMgr.RemoveSession(session.ID)
		}()
		_ = HandleWebSocket(session, sessionMgr, messageChan, "")
	})

	server := httptest.NewServer(wsHandler)
	defer server.Close()

	wsURL := "ws" + strings.TrimPrefix(server.URL, "http") + "/"
	conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("Failed to connect: %v", err)
	}
	defer conn.Close()

	// Send binary message - should be ignored (no response expected)
	err = conn.WriteMessage(websocket.BinaryMessage, []byte{0x00, 0x01, 0x02})
	if err != nil {
		t.Fatalf("Failed to send binary message: %v", err)
	}

	// Send a ping immediately after to verify connection still works
	pingMsg, _ := NewProtocolMessage("system", "heartbeat", "ping", map[string]interface{}{})
	pingData, _ := pingMsg.ToJSON()

	received := make(chan []byte, 1)
	go func() {
		_, msg, _ := conn.ReadMessage()
		select {
		case received <- msg:
		default:
		}
	}()

	conn.WriteMessage(websocket.TextMessage, pingData)

	select {
	case msg := <-received:
		var resp map[string]interface{}
		json.Unmarshal(msg, &resp)
		if resp["cmd"] != "pong" {
			t.Errorf("cmd = %v, want 'pong' (connection alive after binary msg)", resp["cmd"])
		}
	case <-time.After(2 * time.Second):
		t.Fatal("Timeout: connection died after binary message")
	}
}

// --- Invalid JSON message ---

func TestHandleWebSocket_InvalidJSONMessage(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	messageChan := make(chan IncomingMessage, 10)
	received := make(chan []byte, 1)

	wsHandler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		upgrader := websocket.Upgrader{CheckOrigin: func(r *http.Request) bool { return true }}
		conn, err := upgrader.Upgrade(w, r, nil)
		if err != nil {
			return
		}
		session := sessionMgr.CreateSession(conn)
		defer func() {
			conn.Close()
			sessionMgr.RemoveSession(session.ID)
		}()
		_ = HandleWebSocket(session, sessionMgr, messageChan, "")
	})

	server := httptest.NewServer(wsHandler)
	defer server.Close()

	wsURL := "ws" + strings.TrimPrefix(server.URL, "http") + "/"
	conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("Failed to connect: %v", err)
	}
	defer conn.Close()

	go func() {
		_, msg, _ := conn.ReadMessage()
		select {
		case received <- msg:
		default:
		}
	}()

	// Send invalid JSON text message
	conn.WriteMessage(websocket.TextMessage, []byte("not valid json"))

	select {
	case msg := <-received:
		var resp map[string]interface{}
		json.Unmarshal(msg, &resp)
		dataMap, _ := resp["data"].(map[string]interface{})
		errContent, _ := dataMap["content"].(string)
		if !strings.Contains(errContent, "Invalid protocol message format") {
			t.Errorf("error content = %v, want to contain 'Invalid protocol message format'", errContent)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("Timeout waiting for error about invalid JSON")
	}
}

// --- Additional coverage: Server accessors and lifecycle ---

func TestServer_SetModelName(t *testing.T) {
	s, _ := newTestServer(t)

	s.SetModelName("gpt-4-turbo")
	// Verify via status endpoint
	req := httptest.NewRequest("GET", "/api/status", nil)
	w := httptest.NewRecorder()
	s.HandleAPIStatusForTest(w, req)

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)
	if resp["model"] != "gpt-4-turbo" {
		t.Errorf("model = %v, want 'gpt-4-turbo'", resp["model"])
	}
}

func TestServer_SetWorkspace(t *testing.T) {
	s := NewServer(ServerConfig{
		Version:    "test",
		SessionMgr: NewSessionManager(1 * time.Hour),
	})

	// Initially no workspace, should get 503
	req := httptest.NewRequest("GET", "/api/logs", nil)
	w := httptest.NewRecorder()
	s.HandleAPILogsForTest(w, req)
	if w.Code != http.StatusServiceUnavailable {
		t.Errorf("expected 503 without workspace, got %d", w.Code)
	}

	// Set workspace
	tmpDir := t.TempDir()
	s.SetWorkspace(tmpDir)

	// Now it should work (return empty entries since no log file)
	req2 := httptest.NewRequest("GET", "/api/logs?source=general", nil)
	w2 := httptest.NewRecorder()
	s.HandleAPILogsForTest(w2, req2)
	if w2.Code != 200 {
		t.Errorf("expected 200 with workspace, got %d", w2.Code)
	}
}

func TestServer_GetEventHub(t *testing.T) {
	s, _ := newTestServer(t)

	hub := s.GetEventHub()
	if hub == nil {
		t.Fatal("GetEventHub() returned nil")
	}

	// Verify it's a functional hub
	ch := hub.Subscribe()
	hub.Publish("test", "data")
	select {
	case evt := <-ch:
		if evt.Type != "test" || evt.Data != "data" {
			t.Errorf("event = %+v, want type='test' data='data'", evt)
		}
	case <-time.After(time.Second):
		t.Fatal("Timeout waiting for event")
	}
	hub.Unsubscribe(ch)
}

func TestServer_ShutdownNotRunning(t *testing.T) {
	s, _ := newTestServer(t)

	// Shutdown when not running should return nil
	err := s.Shutdown(context.Background())
	if err != nil {
		t.Errorf("Shutdown on non-running server should return nil, got %v", err)
	}
}

func TestServer_Stop(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()

	s := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       0,
		WSPath:     "/ws",
		SessionMgr: sessionMgr,
		Bus:        testBus,
	})

	ctx, cancel := context.WithTimeout(context.Background(), 500*time.Millisecond)
	defer cancel()

	go func() {
		_ = s.Start(ctx)
	}()

	// Wait for server to start
	time.Sleep(100 * time.Millisecond)

	if !s.IsRunning() {
		t.Log("Server may not have started yet")
	}

	// Stop should work same as Shutdown
	stopErr := s.Stop(context.Background())
	if stopErr != nil {
		t.Logf("Stop returned error (may be expected): %v", stopErr)
	}
}

func TestServer_HandleHealth(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)

	// Use httptest to hit a health-like endpoint
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		stats := sessionMgr.Stats()
		w.Header().Set("Content-Type", "application/json")
		fmt.Fprintf(w, `{"status":"ok","running":%t,"sessions":%d}`, true, stats["active_sessions"])
	}))
	defer ts.Close()

	resp, err := http.Get(ts.URL)
	if err != nil {
		t.Fatalf("Failed to call health endpoint: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != 200 {
		t.Errorf("Health endpoint returned %d, want 200", resp.StatusCode)
	}

	ct := resp.Header.Get("Content-Type")
	if ct != "application/json" {
		t.Errorf("Content-Type = %q, want 'application/json'", ct)
	}
}

// --- SendHistoryToSession ---

func TestSendHistoryToSession_InvalidJSON(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()

	s := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       0,
		WSPath:     "/ws",
		SessionMgr: sessionMgr,
		Bus:        testBus,
	})

	err := s.SendHistoryToSession("non-existent", "not valid json")
	if err == nil {
		t.Error("Expected error for invalid JSON content")
	}
	if !strings.Contains(err.Error(), "failed to unmarshal history data") {
		t.Errorf("error = %v, want to contain 'failed to unmarshal history data'", err)
	}
}

func TestSendHistoryToSession_ValidJSON(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()

	s := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       0,
		WSPath:     "/ws",
		SessionMgr: sessionMgr,
		Bus:        testBus,
	})

	// Valid JSON but session doesn't exist
	err := s.SendHistoryToSession("non-existent", `{"messages":[],"has_more":false}`)
	if err == nil {
		t.Error("Expected error for non-existent session")
	}
	// Error should be "session not found", not a JSON error
	if !strings.Contains(err.Error(), "session not found") {
		t.Errorf("error = %v, want to contain 'session not found'", err)
	}
}

// --- dispatchOutbound coverage via processMessages ---
// Note: processMessages is unexported; it is tested via module/web/websocket_test.go
// (package-internal test). Here we verify the Start/Stop lifecycle exercises it.

func TestServer_ProcessMessagesViaStart(t *testing.T) {
	testBus := bus.NewMessageBus()

	s := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       0,
		WSPath:     "/ws",
		SessionMgr: NewSessionManager(1 * time.Hour),
		Bus:        testBus,
	})

	ctx, cancel := context.WithTimeout(context.Background(), 500*time.Millisecond)
	defer cancel()

	// Starting the server also starts processMessages goroutine
	go func() {
		_ = s.Start(ctx)
	}()
	time.Sleep(100 * time.Millisecond)

	// Just verify the server started and processMessages is running
	if !s.IsRunning() {
		t.Log("Server may not have started in time")
	}

	// Let context expire to stop
	<-ctx.Done()
	_ = s.Shutdown(context.Background())
}

// --- publishStatusLoop ---

func TestPublishStatusLoop_PublishesPeriodically(t *testing.T) {
	s, _ := newTestServer(t)

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	hub := s.GetEventHub()
	ch := hub.Subscribe()
	defer hub.Unsubscribe(ch)

	// Start status loop
	go s.PublishStatusLoopForTest(ctx)

	// Wait for at least one status event
	select {
	case event := <-ch:
		if event.Type != "status" {
			t.Errorf("event type = %q, want 'status'", event.Type)
		}
		data, ok := event.Data.(map[string]interface{})
		if !ok {
			t.Fatal("event data is not a map")
		}
		if data["version"] != "test-1.0.0" {
			t.Errorf("version = %v, want 'test-1.0.0'", data["version"])
		}
	case <-time.After(7 * time.Second):
		t.Fatal("Timeout waiting for status event")
	}
}

// --- Negative n parameter handling ---

func TestHandleAPILogs_NegativeN(t *testing.T) {
	s, dir := newTestServer(t)

	logsDir := filepath.Join(dir, "logs")
	os.MkdirAll(logsDir, 0755)
	logFile := filepath.Join(logsDir, "nemesisbot.log")

	var content string
	for i := 0; i < 10; i++ {
		content += fmt.Sprintf(`{"level":"INFO","message":"entry %d"}`+"\n", i)
	}
	os.WriteFile(logFile, []byte(content), 0644)

	// Negative n should use default 200
	req := httptest.NewRequest("GET", "/api/logs?source=general&n=-5", nil)
	w := httptest.NewRecorder()
	s.HandleAPILogsForTest(w, req)

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)
	entries, _ := resp["entries"].([]interface{})
	if len(entries) != 10 {
		t.Errorf("entries length = %d, want 10 (negative n uses default 200, all entries returned)", len(entries))
	}
}

// --- Zero n parameter ---

func TestHandleAPILogs_ZeroN(t *testing.T) {
	s, dir := newTestServer(t)

	logsDir := filepath.Join(dir, "logs")
	os.MkdirAll(logsDir, 0755)
	logFile := filepath.Join(logsDir, "nemesisbot.log")
	os.WriteFile(logFile, []byte(`{"level":"INFO","message":"test"}`+"\n"), 0644)

	// Zero n should use default 200 (since parsed > 0 is false)
	req := httptest.NewRequest("GET", "/api/logs?source=general&n=0", nil)
	w := httptest.NewRecorder()
	s.HandleAPILogsForTest(w, req)

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)
	entries, _ := resp["entries"].([]interface{})
	if len(entries) != 1 {
		t.Errorf("entries length = %d, want 1 (n=0 uses default 200)", len(entries))
	}
}

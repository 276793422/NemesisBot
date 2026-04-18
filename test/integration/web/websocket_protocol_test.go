// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// WebSocket Protocol Integration Tests

package web_test

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	. "github.com/276793422/NemesisBot/module/web"
	"github.com/gorilla/websocket"
)

// testUpgrader for test server
var integrationUpgrader = websocket.Upgrader{
	CheckOrigin: func(r *http.Request) bool { return true },
}

// setupIntegrationWS creates a test server + connected WebSocket pair.
func setupIntegrationWS(t *testing.T) (*websocket.Conn, chan IncomingMessage, func()) {
	t.Helper()

	sessionMgr := NewSessionManager(1 * time.Hour)
	messageChan := make(chan IncomingMessage, 100)

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		conn, err := integrationUpgrader.Upgrade(w, r, nil)
		if err != nil {
			t.Logf("Upgrade error: %v", err)
			return
		}

		sess := sessionMgr.CreateSession(conn)

		if err := HandleWebSocket(sess, sessionMgr, messageChan, ""); err != nil {
			t.Logf("HandleWebSocket error: %v", err)
		}
	}))

	wsURL := "ws" + strings.TrimPrefix(server.URL, "http") + "/ws"
	ws, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		server.Close()
		t.Fatalf("Failed to dial: %v", err)
	}

	cleanup := func() {
		ws.Close()
		server.Close()
	}

	time.Sleep(50 * time.Millisecond)
	return ws, messageChan, cleanup
}

// readWSJSON reads next JSON message from WebSocket with timeout
func readWSJSON(t *testing.T, ws *websocket.Conn, timeout time.Duration) map[string]interface{} {
	t.Helper()
	ws.SetReadDeadline(time.Now().Add(timeout))
	_, raw, err := ws.ReadMessage()
	if err != nil {
		t.Fatalf("ReadMessage error: %v", err)
	}
	var result map[string]interface{}
	if err := json.Unmarshal(raw, &result); err != nil {
		t.Fatalf("Unmarshal error: %v, raw: %s", err, string(raw))
	}
	return result
}

func mustRawMarshal(v interface{}) json.RawMessage {
	b, _ := json.Marshal(v)
	return b
}

func TestWSProtocol_ChatSend(t *testing.T) {
	ws, msgChan, cleanup := setupIntegrationWS(t)
	defer cleanup()

	msg := ProtocolMessage{
		Type:   "message",
		Module: "chat",
		Cmd:    "send",
		Data:   mustRawMarshal(map[string]string{"content": "hello world"}),
	}
	data, _ := json.Marshal(msg)
	if err := ws.WriteMessage(websocket.TextMessage, data); err != nil {
		t.Fatalf("Write error: %v", err)
	}

	select {
	case incoming := <-msgChan:
		if incoming.Content != "hello world" {
			t.Errorf("Content = %q, want %q", incoming.Content, "hello world")
		}
		if incoming.SessionID == "" {
			t.Error("SessionID should not be empty")
		}
		if incoming.SenderID == "" {
			t.Error("SenderID should not be empty")
		}
	case <-time.After(2 * time.Second):
		t.Fatal("Timeout waiting for message on channel")
	}
}

func TestWSProtocol_ChatSend_EmptyContent(t *testing.T) {
	ws, _, cleanup := setupIntegrationWS(t)
	defer cleanup()

	msg := ProtocolMessage{
		Type:   "message",
		Module: "chat",
		Cmd:    "send",
		Data:   mustRawMarshal(map[string]string{"content": ""}),
	}
	data, _ := json.Marshal(msg)
	ws.WriteMessage(websocket.TextMessage, data)

	resp := readWSJSON(t, ws, 2*time.Second)
	if resp["type"] != "system" || resp["module"] != "error" {
		t.Errorf("Expected error for empty content, got: %v", resp)
	}

	dataMap := resp["data"].(map[string]interface{})
	errMsg := dataMap["content"].(string)
	if !strings.Contains(errMsg, "empty") {
		t.Errorf("Error message should mention empty, got: %s", errMsg)
	}
}

func TestWSProtocol_ChatSend_MissingData(t *testing.T) {
	ws, _, cleanup := setupIntegrationWS(t)
	defer cleanup()

	msg := ProtocolMessage{
		Type:   "message",
		Module: "chat",
		Cmd:    "send",
	}
	data, _ := json.Marshal(msg)
	ws.WriteMessage(websocket.TextMessage, data)

	resp := readWSJSON(t, ws, 2*time.Second)
	if resp["type"] != "system" || resp["module"] != "error" {
		t.Errorf("Expected error for missing data, got: %v", resp)
	}
}

func TestWSProtocol_HeartbeatPingPong(t *testing.T) {
	ws, _, cleanup := setupIntegrationWS(t)
	defer cleanup()

	msg := ProtocolMessage{
		Type:   "system",
		Module: "heartbeat",
		Cmd:    "ping",
		Data:   json.RawMessage(`{}`),
	}
	data, _ := json.Marshal(msg)
	ws.WriteMessage(websocket.TextMessage, data)

	resp := readWSJSON(t, ws, 2*time.Second)
	if resp["type"] != "system" {
		t.Errorf("type = %v, want system", resp["type"])
	}
	if resp["module"] != "heartbeat" {
		t.Errorf("module = %v, want heartbeat", resp["module"])
	}
	if resp["cmd"] != "pong" {
		t.Errorf("cmd = %v, want pong", resp["cmd"])
	}
}

func TestWSProtocol_UnknownType(t *testing.T) {
	ws, _, cleanup := setupIntegrationWS(t)
	defer cleanup()

	msg := ProtocolMessage{Type: "unknown_type", Module: "test", Cmd: "test"}
	data, _ := json.Marshal(msg)
	ws.WriteMessage(websocket.TextMessage, data)

	resp := readWSJSON(t, ws, 2*time.Second)
	if resp["type"] != "system" || resp["module"] != "error" {
		t.Errorf("Expected error for unknown type, got: %v", resp)
	}
}

func TestWSProtocol_UnknownChatCmd(t *testing.T) {
	ws, _, cleanup := setupIntegrationWS(t)
	defer cleanup()

	msg := ProtocolMessage{Type: "message", Module: "chat", Cmd: "nonexistent"}
	data, _ := json.Marshal(msg)
	ws.WriteMessage(websocket.TextMessage, data)

	resp := readWSJSON(t, ws, 2*time.Second)
	if resp["type"] != "system" || resp["module"] != "error" {
		t.Errorf("Expected error for unknown chat cmd, got: %v", resp)
	}
}

func TestWSProtocol_UnknownMessageModule(t *testing.T) {
	ws, _, cleanup := setupIntegrationWS(t)
	defer cleanup()

	msg := ProtocolMessage{Type: "message", Module: "nonexistent", Cmd: "test"}
	data, _ := json.Marshal(msg)
	ws.WriteMessage(websocket.TextMessage, data)

	resp := readWSJSON(t, ws, 2*time.Second)
	if resp["type"] != "system" || resp["module"] != "error" {
		t.Errorf("Expected error for unknown module, got: %v", resp)
	}
}

func TestWSProtocol_InvalidJSON(t *testing.T) {
	ws, _, cleanup := setupIntegrationWS(t)
	defer cleanup()

	ws.WriteMessage(websocket.TextMessage, []byte(`not valid json`))

	resp := readWSJSON(t, ws, 2*time.Second)
	if resp["type"] != "system" || resp["module"] != "error" {
		t.Errorf("Expected error for invalid JSON, got: %v", resp)
	}
}

func TestWSProtocol_HistoryRequest_RoutedToChannel(t *testing.T) {
	ws, msgChan, cleanup := setupIntegrationWS(t)
	defer cleanup()

	msg := ProtocolMessage{
		Type:   "message",
		Module: "chat",
		Cmd:    "history_request",
		Data:   mustRawMarshal(HistoryRequestData{RequestID: "req-test", Limit: 10}),
	}
	data, _ := json.Marshal(msg)
	ws.WriteMessage(websocket.TextMessage, data)

	select {
	case incoming := <-msgChan:
		// Verify the message is routed through messageChan with proper metadata
		if incoming.Metadata == nil {
			t.Fatal("Metadata should not be nil for history request")
		}
		if incoming.Metadata["request_type"] != "history" {
			t.Errorf("Metadata request_type = %q, want %q", incoming.Metadata["request_type"], "history")
		}
		// Verify Content contains the JSON request data
		var reqData HistoryRequestData
		if err := json.Unmarshal([]byte(incoming.Content), &reqData); err != nil {
			t.Fatalf("Failed to parse Content as HistoryRequestData: %v", err)
		}
		if reqData.RequestID != "req-test" {
			t.Errorf("RequestID = %q, want %q", reqData.RequestID, "req-test")
		}
		if reqData.Limit != 10 {
			t.Errorf("Limit = %d, want 10", reqData.Limit)
		}
		if incoming.SessionID == "" {
			t.Error("SessionID should not be empty")
		}
	case <-time.After(2 * time.Second):
		t.Fatal("Timeout waiting for history request on channel")
	}
}

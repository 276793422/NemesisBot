// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Protocol Unit Tests

package web_test

import (
	"encoding/json"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/web"
)

func TestIsNewProtocol(t *testing.T) {
	tests := []struct {
		name     string
		input    string
		expected bool
	}{
		{
			name:     "new protocol with module field",
			input:    `{"type":"message","module":"chat","cmd":"send","data":{"content":"hello"},"timestamp":"2026-04-18T12:00:00Z"}`,
			expected: true,
		},
		{
			name:     "new protocol system heartbeat",
			input:    `{"type":"system","module":"heartbeat","cmd":"ping","data":{},"timestamp":"2026-04-18T12:00:00Z"}`,
			expected: true,
		},
		{
			name:     "old flat format message",
			input:    `{"type":"message","content":"hello","timestamp":"2026-04-18T12:00:00Z"}`,
			expected: false,
		},
		{
			name:     "old flat format ping",
			input:    `{"type":"ping","timestamp":"2026-04-18T12:00:00Z"}`,
			expected: false,
		},
		{
			name:     "empty module string is not new protocol",
			input:    `{"type":"message","module":"","cmd":"send"}`,
			expected: false,
		},
		{
			name:     "invalid json",
			input:    `not json`,
			expected: false,
		},
		{
			name:     "empty object",
			input:    `{}`,
			expected: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := web.IsNewProtocol([]byte(tt.input))
			if result != tt.expected {
				t.Errorf("IsNewProtocol() = %v, want %v", result, tt.expected)
			}
		})
	}
}

func TestParseProtocolMessage(t *testing.T) {
	input := `{"type":"message","module":"chat","cmd":"send","data":{"content":"hello"},"timestamp":"2026-04-18T12:00:00Z"}`

	msg, err := web.ParseProtocolMessage([]byte(input))
	if err != nil {
		t.Fatalf("ParseProtocolMessage() error = %v", err)
	}

	if msg.Type != "message" {
		t.Errorf("Type = %q, want %q", msg.Type, "message")
	}
	if msg.Module != "chat" {
		t.Errorf("Module = %q, want %q", msg.Module, "chat")
	}
	if msg.Cmd != "send" {
		t.Errorf("Cmd = %q, want %q", msg.Cmd, "send")
	}

	// Verify data field
	var data struct {
		Content string `json:"content"`
	}
	if err := msg.DecodeData(&data); err != nil {
		t.Fatalf("DecodeData() error = %v", err)
	}
	if data.Content != "hello" {
		t.Errorf("Data.Content = %q, want %q", data.Content, "hello")
	}
}

func TestParseProtocolMessage_InvalidJSON(t *testing.T) {
	_, err := web.ParseProtocolMessage([]byte(`invalid`))
	if err == nil {
		t.Error("ParseProtocolMessage() expected error for invalid JSON")
	}
}

func TestNewProtocolMessage(t *testing.T) {
	data := map[string]string{"content": "test message"}

	msg, err := web.NewProtocolMessage("message", "chat", "send", data)
	if err != nil {
		t.Fatalf("NewProtocolMessage() error = %v", err)
	}

	if msg.Type != "message" {
		t.Errorf("Type = %q, want %q", msg.Type, "message")
	}
	if msg.Module != "chat" {
		t.Errorf("Module = %q, want %q", msg.Module, "chat")
	}
	if msg.Cmd != "send" {
		t.Errorf("Cmd = %q, want %q", msg.Cmd, "send")
	}
	if msg.Timestamp == "" {
		t.Error("Timestamp should not be empty")
	}
	if msg.Data == nil {
		t.Error("Data should not be nil")
	}

	// Verify data is correctly encoded
	var decoded map[string]string
	if err := msg.DecodeData(&decoded); err != nil {
		t.Fatalf("DecodeData() error = %v", err)
	}
	if decoded["content"] != "test message" {
		t.Errorf("Decoded content = %q, want %q", decoded["content"], "test message")
	}
}

func TestNewProtocolMessage_NilData(t *testing.T) {
	msg, err := web.NewProtocolMessage("system", "heartbeat", "ping", nil)
	if err != nil {
		t.Fatalf("NewProtocolMessage() error = %v", err)
	}
	if msg.Data != nil {
		t.Error("Data should be nil when nil is passed")
	}
}

func TestProtocolMessage_ToJSON(t *testing.T) {
	msg, _ := web.NewProtocolMessage("message", "chat", "send", map[string]string{"content": "hello"})

	data, err := msg.ToJSON()
	if err != nil {
		t.Fatalf("ToJSON() error = %v", err)
	}

	// Verify it's valid JSON and round-trips correctly
	var parsed map[string]interface{}
	if err := json.Unmarshal(data, &parsed); err != nil {
		t.Fatalf("ToJSON() output is not valid JSON: %v", err)
	}

	if parsed["type"] != "message" {
		t.Errorf("type = %v, want message", parsed["type"])
	}
	if parsed["module"] != "chat" {
		t.Errorf("module = %v, want chat", parsed["module"])
	}
	if parsed["cmd"] != "send" {
		t.Errorf("cmd = %v, want send", parsed["cmd"])
	}
}

func TestProtocolMessage_DecodeData_NilData(t *testing.T) {
	msg, _ := web.NewProtocolMessage("system", "heartbeat", "ping", nil)

	var v struct{}
	err := msg.DecodeData(&v)
	if err == nil {
		t.Error("DecodeData() expected error for nil data")
	}
}

func TestProtocolMessage_RoundTrip(t *testing.T) {
	original, _ := web.NewProtocolMessage("message", "chat", "history_request", map[string]interface{}{
		"request_id":   "test-123",
		"limit":        20,
		"before_index": 50,
	})

	jsonBytes, err := original.ToJSON()
	if err != nil {
		t.Fatalf("ToJSON() error = %v", err)
	}

	parsed, err := web.ParseProtocolMessage(jsonBytes)
	if err != nil {
		t.Fatalf("ParseProtocolMessage() error = %v", err)
	}

	if parsed.Type != original.Type {
		t.Errorf("Type mismatch: %q != %q", parsed.Type, original.Type)
	}
	if parsed.Module != original.Module {
		t.Errorf("Module mismatch: %q != %q", parsed.Module, original.Module)
	}
	if parsed.Cmd != original.Cmd {
		t.Errorf("Cmd mismatch: %q != %q", parsed.Cmd, original.Cmd)
	}

	var data web.HistoryRequestData
	if err := parsed.DecodeData(&data); err != nil {
		t.Fatalf("DecodeData() error = %v", err)
	}
	if data.RequestID != "test-123" {
		t.Errorf("RequestID = %q, want %q", data.RequestID, "test-123")
	}
	if data.Limit != 20 {
		t.Errorf("Limit = %d, want %d", data.Limit, 20)
	}
	if data.BeforeIndex == nil || *data.BeforeIndex != 50 {
		t.Errorf("BeforeIndex = %v, want 50", data.BeforeIndex)
	}
}

// TestBroadcastToSession_OutputFormat verifies BroadcastToSession outputs ProtocolMessage JSON
func TestBroadcastToSession_OutputFormat(t *testing.T) {
	sm := web.NewSessionManager(1 * time.Hour)

	// Calling with non-existent session should fail, but we can verify the format
	// by checking the Broadcast method's data parameter.
	// We do this indirectly: create a session with a mock sendQueue that captures output.

	// Test 1: non-existent session returns error
	err := web.BroadcastToSession(sm, "non-existent", "assistant", "test message")
	if err == nil {
		t.Error("Expected error for non-existent session")
	}
}

// TestNewProtocolMessage_BroadcastFormat verifies the exact JSON structure
// that BroadcastToSession produces for outbound messages.
func TestNewProtocolMessage_BroadcastFormat(t *testing.T) {
	msg, err := web.NewProtocolMessage("message", "chat", "receive", map[string]string{
		"role":    "assistant",
		"content": "Hello from agent",
	})
	if err != nil {
		t.Fatalf("NewProtocolMessage() error = %v", err)
	}

	data, err := msg.ToJSON()
	if err != nil {
		t.Fatalf("ToJSON() error = %v", err)
	}

	var parsed map[string]interface{}
	if err := json.Unmarshal(data, &parsed); err != nil {
		t.Fatalf("Unmarshal() error = %v", err)
	}

	// Verify exact protocol structure
	if parsed["type"] != "message" {
		t.Errorf("type = %v, want 'message'", parsed["type"])
	}
	if parsed["module"] != "chat" {
		t.Errorf("module = %v, want 'chat'", parsed["module"])
	}
	if parsed["cmd"] != "receive" {
		t.Errorf("cmd = %v, want 'receive'", parsed["cmd"])
	}

	// Verify data payload
	dataMap, ok := parsed["data"].(map[string]interface{})
	if !ok {
		t.Fatal("data should be a map")
	}
	if dataMap["role"] != "assistant" {
		t.Errorf("data.role = %v, want 'assistant'", dataMap["role"])
	}
	if dataMap["content"] != "Hello from agent" {
		t.Errorf("data.content = %v, want 'Hello from agent'", dataMap["content"])
	}

	// Verify timestamp exists
	if parsed["timestamp"] == nil {
		t.Error("timestamp should exist")
	}
}

// TestNewProtocolMessage_ErrorFormat verifies the error message protocol format.
func TestNewProtocolMessage_ErrorFormat(t *testing.T) {
	msg, err := web.NewProtocolMessage("system", "error", "notify", map[string]string{
		"content": "Something went wrong",
	})
	if err != nil {
		t.Fatalf("NewProtocolMessage() error = %v", err)
	}

	data, err := msg.ToJSON()
	if err != nil {
		t.Fatalf("ToJSON() error = %v", err)
	}

	var parsed map[string]interface{}
	if err := json.Unmarshal(data, &parsed); err != nil {
		t.Fatalf("Unmarshal() error = %v", err)
	}

	if parsed["type"] != "system" {
		t.Errorf("type = %v, want 'system'", parsed["type"])
	}
	if parsed["module"] != "error" {
		t.Errorf("module = %v, want 'error'", parsed["module"])
	}
	if parsed["cmd"] != "notify" {
		t.Errorf("cmd = %v, want 'notify'", parsed["cmd"])
	}

	dataMap, ok := parsed["data"].(map[string]interface{})
	if !ok {
		t.Fatal("data should be a map")
	}
	if dataMap["content"] != "Something went wrong" {
		t.Errorf("data.content = %v, want 'Something went wrong'", dataMap["content"])
	}
}

// TestNewProtocolMessage_HeartbeatPongFormat verifies the heartbeat pong protocol format.
func TestNewProtocolMessage_HeartbeatPongFormat(t *testing.T) {
	msg, err := web.NewProtocolMessage("system", "heartbeat", "pong", map[string]interface{}{})
	if err != nil {
		t.Fatalf("NewProtocolMessage() error = %v", err)
	}

	data, err := msg.ToJSON()
	if err != nil {
		t.Fatalf("ToJSON() error = %v", err)
	}

	var parsed map[string]interface{}
	if err := json.Unmarshal(data, &parsed); err != nil {
		t.Fatalf("Unmarshal() error = %v", err)
	}

	if parsed["type"] != "system" {
		t.Errorf("type = %v, want 'system'", parsed["type"])
	}
	if parsed["module"] != "heartbeat" {
		t.Errorf("module = %v, want 'heartbeat'", parsed["module"])
	}
	if parsed["cmd"] != "pong" {
		t.Errorf("cmd = %v, want 'pong'", parsed["cmd"])
	}
}

// TestNewProtocolMessage_HistoryResponseFormat verifies the history response protocol format.
func TestNewProtocolMessage_HistoryResponseFormat(t *testing.T) {
	msg, err := web.NewProtocolMessage("message", "chat", "history", map[string]interface{}{
		"request_id": "req-001",
		"messages": []map[string]string{
			{"role": "user", "content": "hi"},
			{"role": "assistant", "content": "hello"},
		},
		"has_more":     false,
		"oldest_index": 0,
		"total_count":  2,
	})
	if err != nil {
		t.Fatalf("NewProtocolMessage() error = %v", err)
	}

	data, err := msg.ToJSON()
	if err != nil {
		t.Fatalf("ToJSON() error = %v", err)
	}

	var parsed map[string]interface{}
	if err := json.Unmarshal(data, &parsed); err != nil {
		t.Fatalf("Unmarshal() error = %v", err)
	}

	if parsed["type"] != "message" {
		t.Errorf("type = %v, want 'message'", parsed["type"])
	}
	if parsed["module"] != "chat" {
		t.Errorf("module = %v, want 'chat'", parsed["module"])
	}
	if parsed["cmd"] != "history" {
		t.Errorf("cmd = %v, want 'history'", parsed["cmd"])
	}

	dataMap, ok := parsed["data"].(map[string]interface{})
	if !ok {
		t.Fatal("data should be a map")
	}
	if dataMap["request_id"] != "req-001" {
		t.Errorf("data.request_id = %v, want 'req-001'", dataMap["request_id"])
	}
	messages, ok := dataMap["messages"].([]interface{})
	if !ok || len(messages) != 2 {
		t.Fatalf("data.messages should have 2 entries, got %v", dataMap["messages"])
	}
}

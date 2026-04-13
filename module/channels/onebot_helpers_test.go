// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package channels

import (
	"encoding/json"
	"testing"
)

func TestIsAPIResponse_ValidOk(t *testing.T) {
	raw := json.RawMessage(`"ok"`)
	if !isAPIResponse(raw) {
		t.Error("expected true for 'ok'")
	}
}

func TestIsAPIResponse_ValidFailed(t *testing.T) {
	raw := json.RawMessage(`"failed"`)
	if !isAPIResponse(raw) {
		t.Error("expected true for 'failed'")
	}
}

func TestIsAPIResponse_BotStatus(t *testing.T) {
	raw := json.RawMessage(`{"online": true, "good": true}`)
	if !isAPIResponse(raw) {
		t.Error("expected true for online bot status")
	}
}

func TestIsAPIResponse_BotStatusOffline(t *testing.T) {
	raw := json.RawMessage(`{"online": false, "good": false}`)
	if isAPIResponse(raw) {
		t.Error("expected false for offline bot status")
	}
}

func TestIsAPIResponse_Invalid(t *testing.T) {
	raw := json.RawMessage(`"something_else"`)
	if isAPIResponse(raw) {
		t.Error("expected false for unknown string")
	}
}

func TestIsAPIResponse_Empty(t *testing.T) {
	raw := json.RawMessage(``)
	if isAPIResponse(raw) {
		t.Error("expected false for empty raw")
	}
}

func TestIsAPIResponse_Nil(t *testing.T) {
	if isAPIResponse(nil) {
		t.Error("expected false for nil")
	}
}

func TestOneBotRawEventParsing(t *testing.T) {
	jsonStr := `{
		"post_type": "message",
		"message_type": "group",
		"sub_type": "normal",
		"message_id": 12345,
		"user_id": "67890",
		"group_id": 11111,
		"raw_message": "hello world",
		"message": "hello world",
		"sender": {"user_id": 67890, "nickname": "testuser"},
		"self_id": 99999,
		"time": 1700000000
	}`

	var raw oneBotRawEvent
	if err := json.Unmarshal([]byte(jsonStr), &raw); err != nil {
		t.Fatalf("failed to unmarshal: %v", err)
	}

	if raw.PostType != "message" {
		t.Errorf("post_type = %q, want 'message'", raw.PostType)
	}
	if raw.MessageType != "group" {
		t.Errorf("message_type = %q, want 'group'", raw.MessageType)
	}
	if raw.RawMessage != "hello world" {
		t.Errorf("raw_message = %q", raw.RawMessage)
	}
}

func TestOneBotMessageSegmentParsing(t *testing.T) {
	jsonStr := `{"type": "text", "data": {"text": "hello"}}`
	var seg oneBotMessageSegment
	if err := json.Unmarshal([]byte(jsonStr), &seg); err != nil {
		t.Fatalf("failed to unmarshal: %v", err)
	}
	if seg.Type != "text" {
		t.Errorf("type = %q, want 'text'", seg.Type)
	}
	if seg.Data["text"] != "hello" {
		t.Errorf("data.text = %v", seg.Data["text"])
	}
}

func TestOneBotAPIRequestSerialization(t *testing.T) {
	req := oneBotAPIRequest{
		Action: "send_private_msg",
		Params: map[string]interface{}{"user_id": 123, "message": "hi"},
		Echo:   "test_echo",
	}

	data, err := json.Marshal(req)
	if err != nil {
		t.Fatalf("failed to marshal: %v", err)
	}

	var parsed map[string]interface{}
	if err := json.Unmarshal(data, &parsed); err != nil {
		t.Fatalf("failed to unmarshal back: %v", err)
	}

	if parsed["action"] != "send_private_msg" {
		t.Errorf("action = %v", parsed["action"])
	}
	if parsed["echo"] != "test_echo" {
		t.Errorf("echo = %v", parsed["echo"])
	}
}

func TestOneBotAPIRequestNoEcho(t *testing.T) {
	req := oneBotAPIRequest{
		Action: "get_login_info",
		Params: nil,
	}
	data, err := json.Marshal(req)
	if err != nil {
		t.Fatalf("failed to marshal: %v", err)
	}

	var parsed map[string]interface{}
	if err := json.Unmarshal(data, &parsed); err != nil {
		t.Fatalf("failed to unmarshal back: %v", err)
	}

	if _, hasEcho := parsed["echo"]; hasEcho {
		t.Error("echo should be omitted when empty")
	}
}

func TestParseJSONInt64(t *testing.T) {
	tests := []struct {
		input    string
		expected int64
		hasErr   bool
	}{
		{`12345`, 12345, false},
		{`"67890"`, 67890, false},
		{`null`, 0, false},
		{`""`, 0, true},
		{`"abc"`, 0, true},
		{``, 0, false},
	}

	for _, tt := range tests {
		t.Run(tt.input, func(t *testing.T) {
			result, err := parseJSONInt64(json.RawMessage(tt.input))
			if tt.hasErr && err == nil {
				t.Error("expected error")
			}
			if !tt.hasErr && err != nil {
				t.Errorf("unexpected error: %v", err)
			}
			if result != tt.expected {
				t.Errorf("got %d, want %d", result, tt.expected)
			}
		})
	}
}

func TestParseJSONString(t *testing.T) {
	tests := []struct {
		input    string
		expected string
	}{
		{`"hello"`, "hello"},
		{`123`, "123"},
		{``, ""},
		{`null`, ""},
	}

	for _, tt := range tests {
		t.Run(tt.input, func(t *testing.T) {
			result := parseJSONString(json.RawMessage(tt.input))
			if result != tt.expected {
				t.Errorf("got %q, want %q", result, tt.expected)
			}
		})
	}
}

func TestTruncate(t *testing.T) {
	tests := []struct {
		input    string
		n        int
		expected string
	}{
		{"short", 10, "short"},
		{"exact length", 12, "exact length"},
		{"this is a long string", 10, "this is a ..."},
		{"unicode: 你好世界", 3, "uni..."},
	}

	for _, tt := range tests {
		t.Run(tt.input, func(t *testing.T) {
			result := truncate(tt.input, tt.n)
			if result != tt.expected {
				t.Errorf("truncate(%q, %d) = %q, want %q", tt.input, tt.n, result, tt.expected)
			}
		})
	}
}

func TestOneBotSenderParsing(t *testing.T) {
	jsonStr := `{"user_id": 12345, "nickname": "TestUser", "card": "CardName"}`
	var sender oneBotSender
	if err := json.Unmarshal([]byte(jsonStr), &sender); err != nil {
		t.Fatalf("failed to unmarshal: %v", err)
	}
	if sender.Nickname != "TestUser" {
		t.Errorf("nickname = %q", sender.Nickname)
	}
	if sender.Card != "CardName" {
		t.Errorf("card = %q", sender.Card)
	}
}

func TestBotStatusParsing(t *testing.T) {
	jsonStr := `{"online": true, "good": false}`
	var bs BotStatus
	if err := json.Unmarshal([]byte(jsonStr), &bs); err != nil {
		t.Fatalf("failed to unmarshal: %v", err)
	}
	if !bs.Online {
		t.Error("expected online = true")
	}
	if bs.Good {
		t.Error("expected good = false")
	}
}

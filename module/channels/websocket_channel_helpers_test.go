// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package channels

import (
	"encoding/json"
	"testing"
	"time"
)

func TestClientMessageJSON(t *testing.T) {
	msg := ClientMessage{
		Type:      MessageTypeMessage,
		Content:   "hello world",
		Timestamp: time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC),
	}

	data, err := json.Marshal(msg)
	if err != nil {
		t.Fatalf("failed to marshal: %v", err)
	}

	var parsed ClientMessage
	if err := json.Unmarshal(data, &parsed); err != nil {
		t.Fatalf("failed to unmarshal: %v", err)
	}

	if parsed.Type != MessageTypeMessage {
		t.Errorf("type = %q, want %q", parsed.Type, MessageTypeMessage)
	}
	if parsed.Content != "hello world" {
		t.Errorf("content = %q", parsed.Content)
	}
}

func TestServerMessageJSON(t *testing.T) {
	msg := ServerMessage{
		Type:      MessageTypeMessage,
		Role:      "assistant",
		Content:   "response text",
		Timestamp: time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC),
		Error:     "",
	}

	data, err := json.Marshal(msg)
	if err != nil {
		t.Fatalf("failed to marshal: %v", err)
	}

	var parsed ServerMessage
	if err := json.Unmarshal(data, &parsed); err != nil {
		t.Fatalf("failed to unmarshal: %v", err)
	}

	if parsed.Role != "assistant" {
		t.Errorf("role = %q", parsed.Role)
	}
	if parsed.Content != "response text" {
		t.Errorf("content = %q", parsed.Content)
	}
	if parsed.Error != "" {
		t.Errorf("error should be empty, got %q", parsed.Error)
	}
}

func TestServerMessageError(t *testing.T) {
	msg := ServerMessage{
		Type:    MessageTypeError,
		Error:   "something went wrong",
	}
	data, err := json.Marshal(msg)
	if err != nil {
		t.Fatalf("failed to marshal: %v", err)
	}

	var parsed ServerMessage
	if err := json.Unmarshal(data, &parsed); err != nil {
		t.Fatalf("failed to unmarshal: %v", err)
	}

	if parsed.Type != MessageTypeError {
		t.Errorf("type = %q", parsed.Type)
	}
	if parsed.Error != "something went wrong" {
		t.Errorf("error = %q", parsed.Error)
	}
}

func TestMessageTypeConstants(t *testing.T) {
	if MessageTypeMessage != "message" {
		t.Errorf("MessageTypeMessage = %q, want 'message'", MessageTypeMessage)
	}
	if MessageTypePing != "ping" {
		t.Errorf("MessageTypePing = %q, want 'ping'", MessageTypePing)
	}
	if MessageTypePong != "pong" {
		t.Errorf("MessageTypePong = %q, want 'pong'", MessageTypePong)
	}
	if MessageTypeError != "error" {
		t.Errorf("MessageTypeError = %q, want 'error'", MessageTypeError)
	}
}

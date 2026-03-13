// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package tools

import (
	"context"
	"errors"
	"strings"
	"testing"
)

func TestMessageTool_Execute_Success(t *testing.T) {
	tool := NewMessageTool()

	sent := false
	var sentChannel, sentChatID, sentContent string

	tool.SetSendCallback(func(channel, chatID, content string) error {
		sent = true
		sentChannel = channel
		sentChatID = chatID
		sentContent = content
		return nil
	})

	tool.SetContext("test_channel", "test_chat")

	ctx := context.Background()
	result := tool.Execute(ctx, map[string]interface{}{
		"content": "Hello, World!",
	})

	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}

	if !sent {
		t.Error("Callback should have been called")
	}

	if sentChannel != "test_channel" {
		t.Errorf("Expected channel 'test_channel', got '%s'", sentChannel)
	}

	if sentChatID != "test_chat" {
		t.Errorf("Expected chatID 'test_chat', got '%s'", sentChatID)
	}

	if sentContent != "Hello, World!" {
		t.Errorf("Expected content 'Hello, World!', got '%s'", sentContent)
	}

	if !result.Silent {
		t.Error("Result should be silent")
	}

	if !tool.HasSentInRound() {
		t.Error("HasSentInRound should return true")
	}
}

func TestMessageTool_Execute_RPCChannelWithCorrelationID(t *testing.T) {
	tool := NewMessageTool()

	var sentContent string
	tool.SetSendCallback(func(channel, chatID, content string) error {
		sentContent = content
		return nil
	})

	tool.SetContext("rpc", "test_chat")

	ctx := context.WithValue(context.Background(), "correlation_id", "test-correlation-123")

	result := tool.Execute(ctx, map[string]interface{}{
		"content": "Hello, RPC!",
	})

	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}

	// Check that correlation ID was added
	if !strings.Contains(sentContent, "[rpc:test-correlation-123]") {
		t.Errorf("Expected correlation ID prefix in content, got '%s'", sentContent)
	}

	if !strings.Contains(sentContent, "Hello, RPC!") {
		t.Errorf("Expected original message in content, got '%s'", sentContent)
	}
}

func TestMessageTool_Execute_RPCChannelNoCorrelationID(t *testing.T) {
	tool := NewMessageTool()

	tool.SetSendCallback(func(channel, chatID, content string) error {
		return nil
	})

	tool.SetContext("rpc", "test_chat")

	ctx := context.Background() // No correlation ID

	result := tool.Execute(ctx, map[string]interface{}{
		"content": "Hello, RPC!",
	})

	// Should still succeed, but with a warning logged
	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}
}

func TestMessageTool_Execute_CustomChannelAndChatID(t *testing.T) {
	tool := NewMessageTool()

	var sentChannel, sentChatID string
	tool.SetSendCallback(func(channel, chatID, content string) error {
		sentChannel = channel
		sentChatID = chatID
		return nil
	})

	tool.SetContext("default_channel", "default_chat")

	ctx := context.Background()
	result := tool.Execute(ctx, map[string]interface{}{
		"content": "Test message",
		"channel": "custom_channel",
		"chat_id": "custom_chat",
	})

	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}

	if sentChannel != "custom_channel" {
		t.Errorf("Expected channel 'custom_channel', got '%s'", sentChannel)
	}

	if sentChatID != "custom_chat" {
		t.Errorf("Expected chatID 'custom_chat', got '%s'", sentChatID)
	}
}

func TestMessageTool_Execute_MissingContent(t *testing.T) {
	tool := NewMessageTool()
	tool.SetSendCallback(func(channel, chatID, content string) error {
		return nil
	})

	ctx := context.Background()
	result := tool.Execute(ctx, map[string]interface{}{})

	if !result.IsError {
		t.Error("Expected error for missing content")
	}

	if !strings.Contains(result.ForLLM, "content is required") {
		t.Errorf("Expected 'content is required' error, got '%s'", result.ForLLM)
	}
}

func TestMessageTool_Execute_NoTargetChannel(t *testing.T) {
	tool := NewMessageTool()
	tool.SetSendCallback(func(channel, chatID, content string) error {
		return nil
	})

	// Don't set context - no default channel/chat

	ctx := context.Background()
	result := tool.Execute(ctx, map[string]interface{}{
		"content": "Test",
	})

	if !result.IsError {
		t.Error("Expected error when no target channel specified")
	}

	if !strings.Contains(result.ForLLM, "No target") {
		t.Errorf("Expected 'No target' error, got '%s'", result.ForLLM)
	}
}

func TestMessageTool_Execute_NoCallback(t *testing.T) {
	tool := NewMessageTool()
	// Don't set callback

	tool.SetContext("test_channel", "test_chat")

	ctx := context.Background()
	result := tool.Execute(ctx, map[string]interface{}{
		"content": "Test",
	})

	if !result.IsError {
		t.Error("Expected error when no callback configured")
	}

	if !strings.Contains(result.ForLLM, "not configured") {
		t.Errorf("Expected 'not configured' error, got '%s'", result.ForLLM)
	}
}

func TestMessageTool_Execute_CallbackError(t *testing.T) {
	tool := NewMessageTool()

	expectedErr := errors.New("send failed")
	tool.SetSendCallback(func(channel, chatID, content string) error {
		return expectedErr
	})

	tool.SetContext("test_channel", "test_chat")

	ctx := context.Background()
	result := tool.Execute(ctx, map[string]interface{}{
		"content": "Test",
	})

	if !result.IsError {
		t.Error("Expected error when callback fails")
	}

	if !strings.Contains(result.ForLLM, "sending message") {
		t.Errorf("Expected 'sending message' error, got '%s'", result.ForLLM)
	}

	if result.Err != expectedErr {
		t.Error("Error should be preserved in result")
	}
}

func TestMessageTool_SetContext(t *testing.T) {
	tool := NewMessageTool()

	tool.SetContext("channel1", "chat1")
	if tool.defaultChannel != "channel1" {
		t.Errorf("Expected defaultChannel 'channel1', got '%s'", tool.defaultChannel)
	}
	if tool.defaultChatID != "chat1" {
		t.Errorf("Expected defaultChatID 'chat1', got '%s'", tool.defaultChatID)
	}

	// Update context
	tool.SetContext("channel2", "chat2")
	if tool.defaultChannel != "channel2" {
		t.Errorf("Expected defaultChannel 'channel2', got '%s'", tool.defaultChannel)
	}
	if tool.defaultChatID != "chat2" {
		t.Errorf("Expected defaultChatID 'chat2', got '%s'", tool.defaultChatID)
	}
}

func TestMessageTool_HasSentInRound(t *testing.T) {
	tool := NewMessageTool()

	if tool.HasSentInRound() {
		t.Error("HasSentInRound should initially be false")
	}

	tool.SetSendCallback(func(channel, chatID, content string) error {
		return nil
	})

	tool.SetContext("test", "test")

	ctx := context.Background()
	tool.Execute(ctx, map[string]interface{}{"content": "test"})

	if !tool.HasSentInRound() {
		t.Error("HasSentInRound should be true after sending")
	}

	// Reset context should reset the flag
	tool.SetContext("test2", "test2")
	if tool.HasSentInRound() {
		t.Error("HasSentInRound should be false after context reset")
	}
}

func TestMessageTool_Name(t *testing.T) {
	tool := NewMessageTool()
	if tool.Name() != "message" {
		t.Errorf("Expected name 'message', got '%s'", tool.Name())
	}
}

func TestMessageTool_Description(t *testing.T) {
	tool := NewMessageTool()
	desc := tool.Description()
	if desc == "" {
		t.Error("Description should not be empty")
	}
	if !strings.Contains(desc, "Send a message") {
		t.Errorf("Description should mention sending message, got '%s'", desc)
	}
}

func TestMessageTool_Parameters(t *testing.T) {
	tool := NewMessageTool()
	params := tool.Parameters()

	if params == nil {
		t.Fatal("Parameters should not be nil")
	}

	if params["type"] != "object" {
		t.Errorf("Expected type 'object', got '%v'", params["type"])
	}

	props, ok := params["properties"].(map[string]interface{})
	if !ok {
		t.Fatal("Properties should be a map")
	}

	// Check required parameters
	required, ok := params["required"].([]string)
	if !ok {
		t.Fatal("Required should be a string slice")
	}

	if len(required) != 1 || required[0] != "content" {
		t.Errorf("Expected only 'content' to be required, got %v", required)
	}

	// Check that content, channel, and chat_id are in properties
	if _, ok := props["content"]; !ok {
		t.Error("content should be in properties")
	}
	if _, ok := props["channel"]; !ok {
		t.Error("channel should be in properties")
	}
	if _, ok := props["chat_id"]; !ok {
		t.Error("chat_id should be in properties")
	}
}

func TestGetCorrelationIDFromContext(t *testing.T) {
	tests := []struct {
		name     string
		ctx      context.Context
		expected string
	}{
		{
			name:     "nil context",
			ctx:      nil,
			expected: "",
		},
		{
			name:     "context without correlation ID",
			ctx:      context.Background(),
			expected: "",
		},
		{
			name:     "context with correlation ID",
			ctx:      context.WithValue(context.Background(), "correlation_id", "test-123"),
			expected: "test-123",
		},
		{
			name:     "context with wrong type",
			ctx:      context.WithValue(context.Background(), "correlation_id", 123),
			expected: "",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := getCorrelationIDFromContext(tt.ctx)
			if result != tt.expected {
				t.Errorf("Expected '%s', got '%s'", tt.expected, result)
			}
		})
	}
}

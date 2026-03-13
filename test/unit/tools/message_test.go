// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package tools_test

import (
	"context"
	"fmt"
	"testing"

	. "github.com/276793422/NemesisBot/module/tools"
)

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
		t.Errorf("Expected ['content'], got %v", required)
	}

	// Check content property
	content, ok := props["content"].(map[string]interface{})
	if !ok {
		t.Fatal("Content property should exist")
	}

	if content["type"] != "string" {
		t.Errorf("Content type should be string, got '%v'", content["type"])
	}
}

func TestMessageTool_Execute_MissingContent(t *testing.T) {
	tool := NewMessageTool()
	ctx := context.Background()
	args := map[string]interface{}{}

	result := tool.Execute(ctx, args)

	if !result.IsError {
		t.Error("Expected error when content is missing")
	}
}

func TestMessageTool_Execute_NoCallback(t *testing.T) {
	tool := NewMessageTool()
	ctx := context.Background()
	args := map[string]interface{}{
		"content": "test message",
	}

	result := tool.Execute(ctx, args)

	if !result.IsError {
		t.Error("Expected error when callback is not configured")
	}
}

func TestMessageTool_Execute_WithCallback(t *testing.T) {
	tool := NewMessageTool()

	var receivedChannel, receivedChatID, receivedContent string
	tool.SetSendCallback(func(channel, chatID, content string) error {
		receivedChannel = channel
		receivedChatID = chatID
		receivedContent = content
		return nil
	})

	tool.SetContext("test_channel", "test_chat")

	ctx := context.Background()
	args := map[string]interface{}{
		"content": "test message",
	}

	result := tool.Execute(ctx, args)

	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}

	if receivedChannel != "test_channel" {
		t.Errorf("Expected channel 'test_channel', got '%s'", receivedChannel)
	}

	if receivedChatID != "test_chat" {
		t.Errorf("Expected chatID 'test_chat', got '%s'", receivedChatID)
	}

	if receivedContent != "test message" {
		t.Errorf("Expected content 'test message', got '%s'", receivedContent)
	}

	// Check that result is silent (message sent directly to user)
	if !result.Silent {
		t.Error("Result should be silent (message sent directly)")
	}
}

func TestMessageTool_Execute_CallbackError(t *testing.T) {
	tool := NewMessageTool()

	tool.SetSendCallback(func(channel, chatID, content string) error {
		return fmt.Errorf("send failed")
	})

	tool.SetContext("test_channel", "test_chat")

	ctx := context.Background()
	args := map[string]interface{}{
		"content": "test message",
	}

	result := tool.Execute(ctx, args)

	if !result.IsError {
		t.Error("Expected error when callback fails")
	}
}

func TestMessageTool_Execute_OverrideChannelAndChatID(t *testing.T) {
	tool := NewMessageTool()

	// Set default context
	tool.SetContext("default_channel", "default_chat")

	var receivedChannel, receivedChatID string
	tool.SetSendCallback(func(channel, chatID, content string) error {
		receivedChannel = channel
		receivedChatID = chatID
		return nil
	})

	ctx := context.Background()
	args := map[string]interface{}{
		"content": "test message",
		"channel": "override_channel",
		"chat_id": "override_chat",
	}

	result := tool.Execute(ctx, args)

	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}

	if receivedChannel != "override_channel" {
		t.Errorf("Expected override channel, got '%s'", receivedChannel)
	}

	if receivedChatID != "override_chat" {
		t.Errorf("Expected override chatID, got '%s'", receivedChatID)
	}
}

func TestMessageTool_Execute_RPCChannelWithCorrelationID(t *testing.T) {
	tool := NewMessageTool()

	var receivedContent string
	tool.SetSendCallback(func(channel, chatID, content string) error {
		receivedContent = content
		return nil
	})

	tool.SetContext("rpc", "test_chat")

	ctx := context.Background()
	args := map[string]interface{}{
		"content": "test message",
	}

	result := tool.Execute(ctx, args)

	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}

	// Without correlation ID in context, content should be unchanged
	if receivedContent != "test message" {
		t.Errorf("Expected 'test message', got '%s'", receivedContent)
	}
}

func TestMessageTool_Execute_RPCChannelWithCorrelationIDInContext(t *testing.T) {
	tool := NewMessageTool()

	var receivedContent string
	tool.SetSendCallback(func(channel, chatID, content string) error {
		receivedContent = content
		return nil
	})

	tool.SetContext("rpc", "test_chat")

	// Add correlation ID to context
	ctx := context.WithValue(context.Background(), "correlation_id", "test-correlation-123")

	args := map[string]interface{}{
		"content": "test message",
	}

	result := tool.Execute(ctx, args)

	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}

	// With correlation ID, content should have prefix
	expectedPrefix := "[rpc:test-correlation-123]"
	if !contains(receivedContent, expectedPrefix) {
		t.Errorf("Expected content to start with '%s', got '%s'", expectedPrefix, receivedContent)
	}

	if !contains(receivedContent, "test message") {
		t.Errorf("Expected content to contain 'test message', got '%s'", receivedContent)
	}
}

func TestMessageTool_HasSentInRound(t *testing.T) {
	tool := NewMessageTool()

	// Initially should not have sent
	if tool.HasSentInRound() {
		t.Error("Should not have sent before SetContext")
	}

	tool.SetContext("test_channel", "test_chat")

	// Should reset after SetContext
	if tool.HasSentInRound() {
		t.Error("Should not have sent after SetContext reset")
	}

	tool.SetSendCallback(func(channel, chatID, content string) error {
		return nil
	})

	ctx := context.Background()
	args := map[string]interface{}{
		"content": "test",
	}

	tool.Execute(ctx, args)

	// Should have sent after successful execution
	if !tool.HasSentInRound() {
		t.Error("Should have sent after successful execution")
	}

	// SetContext should reset the flag
	tool.SetContext("test_channel", "test_chat")
	if tool.HasSentInRound() {
		t.Error("Should not have sent after SetContext reset")
	}
}

func TestMessageTool_Execute_NoChannel(t *testing.T) {
	tool := NewMessageTool()

	tool.SetSendCallback(func(channel, chatID, content string) error {
		return nil
	})

	// Don't set context
	ctx := context.Background()
	args := map[string]interface{}{
		"content": "test message",
	}

	result := tool.Execute(ctx, args)

	if !result.IsError {
		t.Error("Expected error when no channel is configured")
	}
}

func TestMessageTool_Execute_EmptyContent(t *testing.T) {
	tool := NewMessageTool()

	tool.SetSendCallback(func(channel, chatID, content string) error {
		return nil
	})

	tool.SetContext("test_channel", "test_chat")

	ctx := context.Background()
	args := map[string]interface{}{
		"content": "",
	}

	result := tool.Execute(ctx, args)

	// Empty content should still work
	if result.IsError {
		t.Errorf("Expected success with empty content, got error: %s", result.ForLLM)
	}
}

func TestMessageTool_Execute_NilContext(t *testing.T) {
	tool := NewMessageTool()

	var receivedContent string
	tool.SetSendCallback(func(channel, chatID, content string) error {
		receivedContent = content
		return nil
	})

	tool.SetContext("test_channel", "test_chat")

	// Pass nil context
	args := map[string]interface{}{
		"content": "test message",
	}

	result := tool.Execute(nil, args)

	if result.IsError {
		t.Errorf("Expected success with nil context, got error: %s", result.ForLLM)
	}

	// Content should be sent normally without correlation ID
	if receivedContent != "test message" {
		t.Errorf("Expected 'test message', got '%s'", receivedContent)
	}
}

func TestMessageTool_Execute_LongContent(t *testing.T) {
	tool := NewMessageTool()

	var receivedContent string
	tool.SetSendCallback(func(channel, chatID, content string) error {
		receivedContent = content
		return nil
	})

	tool.SetContext("test_channel", "test_chat")

	// Create long content
	longContent := ""
	for i := 0; i < 1000; i++ {
		longContent += "test "
	}

	ctx := context.Background()
	args := map[string]interface{}{
		"content": longContent,
	}

	result := tool.Execute(ctx, args)

	if result.IsError {
		t.Errorf("Expected success with long content, got error: %s", result.ForLLM)
	}

	if receivedContent != longContent {
		t.Error("Long content should be sent in full")
	}
}

func TestMessageTool_Execute_SpecialCharacters(t *testing.T) {
	tool := NewMessageTool()

	var receivedContent string
	tool.SetSendCallback(func(channel, chatID, content string) error {
		receivedContent = content
		return nil
	})

	tool.SetContext("test_channel", "test_chat")

	specialContent := "Test with \n newlines \t tabs \"quotes\" 'apostrophes'"

	ctx := context.Background()
	args := map[string]interface{}{
		"content": specialContent,
	}

	result := tool.Execute(ctx, args)

	if result.IsError {
		t.Errorf("Expected success with special characters, got error: %s", result.ForLLM)
	}

	if receivedContent != specialContent {
		t.Errorf("Special characters should be preserved: got '%s'", receivedContent)
	}
}

func TestMessageTool_Execute_UTF8(t *testing.T) {
	tool := NewMessageTool()

	var receivedContent string
	tool.SetSendCallback(func(channel, chatID, content string) error {
		receivedContent = content
		return nil
	})

	tool.SetContext("test_channel", "test_chat")

	utf8Content := "Test UTF-8: 你好 世界 🌍 Привет"

	ctx := context.Background()
	args := map[string]interface{}{
		"content": utf8Content,
	}

	result := tool.Execute(ctx, args)

	if result.IsError {
		t.Errorf("Expected success with UTF-8, got error: %s", result.ForLLM)
	}

	if receivedContent != utf8Content {
		t.Errorf("UTF-8 content should be preserved: got '%s'", receivedContent)
	}
}

func TestMessageTool_Execute_MultipleRPCMessages(t *testing.T) {
	tool := NewMessageTool()

	var receivedContents []string
	tool.SetSendCallback(func(channel, chatID, content string) error {
		receivedContents = append(receivedContents, content)
		return nil
	})

	tool.SetContext("rpc", "test_chat")

	ctx1 := context.WithValue(context.Background(), "correlation_id", "corr-1")
	ctx2 := context.WithValue(context.Background(), "correlation_id", "corr-2")

	// Send first message
	args1 := map[string]interface{}{
		"content": "message 1",
	}
	result1 := tool.Execute(ctx1, args1)

	if result1.IsError {
		t.Errorf("First message failed: %s", result1.ForLLM)
	}

	// Send second message
	args2 := map[string]interface{}{
		"content": "message 2",
	}
	result2 := tool.Execute(ctx2, args2)

	if result2.IsError {
		t.Errorf("Second message failed: %s", result2.ForLLM)
	}

	if len(receivedContents) != 2 {
		t.Fatalf("Expected 2 messages, got %d", len(receivedContents))
	}

	if !contains(receivedContents[0], "[rpc:corr-1]") {
		t.Errorf("First message should have corr-1 prefix, got: %s", receivedContents[0])
	}

	if !contains(receivedContents[1], "[rpc:corr-2]") {
		t.Errorf("Second message should have corr-2 prefix, got: %s", receivedContents[1])
	}
}

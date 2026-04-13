// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package agent

import (
	"testing"

	"github.com/276793422/NemesisBot/module/providers"
)

// --- extractPeer tests ---

func TestExtractPeer_NoMetadata(t *testing.T) {
	msg := createTestInboundMessage("test", "user1", "chat1", "hello")
	result := extractPeer(msg)
	if result != nil {
		t.Error("Expected nil when no peer metadata")
	}
}

func TestExtractPeer_DirectPeerWithID(t *testing.T) {
	msg := createTestInboundMessage("test", "user1", "chat1", "hello")
	msg.Metadata = map[string]string{
		"peer_kind": "direct",
		"peer_id":   "explicit-id",
	}
	result := extractPeer(msg)
	if result == nil {
		t.Fatal("Expected non-nil peer")
	}
	if result.Kind != "direct" || result.ID != "explicit-id" {
		t.Errorf("Expected kind=direct id=explicit-id, got kind=%s id=%s", result.Kind, result.ID)
	}
}

func TestExtractPeer_DirectPeerFallbackToSenderID(t *testing.T) {
	msg := createTestInboundMessage("test", "user1", "chat1", "hello")
	msg.Metadata = map[string]string{
		"peer_kind": "direct",
	}
	result := extractPeer(msg)
	if result == nil {
		t.Fatal("Expected non-nil peer")
	}
	if result.ID != "user1" {
		t.Errorf("Expected ID to fallback to SenderID 'user1', got '%s'", result.ID)
	}
}

func TestExtractPeer_GroupPeerFallbackToChatID(t *testing.T) {
	msg := createTestInboundMessage("test", "user1", "chat1", "hello")
	msg.Metadata = map[string]string{
		"peer_kind": "group",
	}
	result := extractPeer(msg)
	if result == nil {
		t.Fatal("Expected non-nil peer")
	}
	if result.ID != "chat1" {
		t.Errorf("Expected ID to fallback to ChatID 'chat1', got '%s'", result.ID)
	}
}

// --- extractParentPeer tests ---

func TestExtractParentPeer_NoMetadata(t *testing.T) {
	msg := createTestInboundMessage("test", "user1", "chat1", "hello")
	result := extractParentPeer(msg)
	if result != nil {
		t.Error("Expected nil when no parent peer metadata")
	}
}

func TestExtractParentPeer_PartialMetadata(t *testing.T) {
	msg := createTestInboundMessage("test", "user1", "chat1", "hello")
	msg.Metadata = map[string]string{
		"parent_peer_kind": "direct",
		// missing parent_peer_id
	}
	result := extractParentPeer(msg)
	if result != nil {
		t.Error("Expected nil when parent_peer_id is missing")
	}
}

func TestExtractParentPeer_CompleteMetadata(t *testing.T) {
	msg := createTestInboundMessage("test", "user1", "chat1", "hello")
	msg.Metadata = map[string]string{
		"parent_peer_kind": "group",
		"parent_peer_id":   "parent-chat-123",
	}
	result := extractParentPeer(msg)
	if result == nil {
		t.Fatal("Expected non-nil parent peer")
	}
	if result.Kind != "group" || result.ID != "parent-chat-123" {
		t.Errorf("Expected kind=group id=parent-chat-123, got kind=%s id=%s", result.Kind, result.ID)
	}
}

// --- formatMessagesForLog tests ---

func TestFormatMessagesForLog_Empty(t *testing.T) {
	result := formatMessagesForLog([]providers.Message{})
	if result != "[]" {
		t.Errorf("Expected '[]', got '%s'", result)
	}
}

func TestFormatMessagesForLog_SimpleMessage(t *testing.T) {
	messages := []providers.Message{
		{Role: "user", Content: "Hello"},
	}
	result := formatMessagesForLog(messages)
	if result == "[]" {
		t.Error("Expected non-empty formatted output")
	}
	if !helpersContains(result, "user") || !helpersContains(result, "Hello") {
		t.Errorf("Expected output to contain role 'user' and content 'Hello', got: %s", result)
	}
}

func TestFormatMessagesForLog_ToolCalls(t *testing.T) {
	messages := []providers.Message{
		{
			Role:    "assistant",
			Content: "",
			ToolCalls: []providers.ToolCall{
				{
					ID:   "tc-1",
					Type: "function",
					Name: "read_file",
					Function: &providers.FunctionCall{
						Name:      "read_file",
						Arguments: `{"path":"/test"}`,
					},
				},
			},
		},
	}
	result := formatMessagesForLog(messages)
	if !helpersContains(result, "ToolCalls") || !helpersContains(result, "read_file") {
		t.Errorf("Expected output to contain tool call info, got: %s", result)
	}
}

func TestFormatMessagesForLog_ToolCallID(t *testing.T) {
	messages := []providers.Message{
		{Role: "tool", Content: "result", ToolCallID: "tc-123"},
	}
	result := formatMessagesForLog(messages)
	if !helpersContains(result, "tc-123") {
		t.Errorf("Expected output to contain ToolCallID 'tc-123', got: %s", result)
	}
}

// --- formatToolsForLog tests ---

func TestFormatToolsForLog_Empty(t *testing.T) {
	result := formatToolsForLog([]providers.ToolDefinition{})
	if result != "[]" {
		t.Errorf("Expected '[]', got '%s'", result)
	}
}

func TestFormatToolsForLog_WithTools(t *testing.T) {
	tools := []providers.ToolDefinition{
		{
			Type: "function",
			Function: providers.ToolFunctionDefinition{
				Name:        "read_file",
				Description: "Read a file",
				Parameters:  map[string]interface{}{"path": "/test"},
			},
		},
	}
	result := formatToolsForLog(tools)
	if !helpersContains(result, "read_file") || !helpersContains(result, "Read a file") {
		t.Errorf("Expected output to contain tool name and description, got: %s", result)
	}
}

// --- handleCommand tests ---

func TestHandleCommand_NotACommand(t *testing.T) {
	loop := createTestAgentLoop(t)
	msg := createTestInboundMessage("test", "user1", "chat1", "just a normal message")
	result, handled := loop.handleCommand(nil, msg)
	if handled {
		t.Error("Expected handled=false for non-command message")
	}
	if result != "" {
		t.Error("Expected empty result for non-command message")
	}
}

func TestHandleCommand_ShowModel(t *testing.T) {
	loop := createTestAgentLoop(t)
	msg := createTestInboundMessage("test", "user1", "chat1", "/show model")
	result, handled := loop.handleCommand(nil, msg)
	if !handled {
		t.Error("Expected handled=true for /show command")
	}
	if !helpersContains(result, "test/model") {
		t.Errorf("Expected result to contain model name, got: %s", result)
	}
}

func TestHandleCommand_ShowChannel(t *testing.T) {
	loop := createTestAgentLoop(t)
	msg := createTestInboundMessage("test", "user1", "chat1", "/show channel")
	result, handled := loop.handleCommand(nil, msg)
	if !handled {
		t.Error("Expected handled=true for /show channel command")
	}
	if !helpersContains(result, "test") {
		t.Errorf("Expected result to contain channel name, got: %s", result)
	}
}

func TestHandleCommand_ShowAgents(t *testing.T) {
	loop := createTestAgentLoop(t)
	msg := createTestInboundMessage("test", "user1", "chat1", "/show agents")
	result, handled := loop.handleCommand(nil, msg)
	if !handled {
		t.Error("Expected handled=true for /show agents command")
	}
	if !helpersContains(result, "main") {
		t.Errorf("Expected result to contain agent IDs, got: %s", result)
	}
}

func TestHandleCommand_ShowUnknown(t *testing.T) {
	loop := createTestAgentLoop(t)
	msg := createTestInboundMessage("test", "user1", "chat1", "/show unknown")
	result, handled := loop.handleCommand(nil, msg)
	if !handled {
		t.Error("Expected handled=true for /show unknown command")
	}
	if !helpersContains(result, "Unknown") {
		t.Errorf("Expected 'Unknown' in result, got: %s", result)
	}
}

func TestHandleCommand_ShowNoArgs(t *testing.T) {
	loop := createTestAgentLoop(t)
	msg := createTestInboundMessage("test", "user1", "chat1", "/show")
	result, handled := loop.handleCommand(nil, msg)
	if !handled {
		t.Error("Expected handled=true")
	}
	if !helpersContains(result, "Usage") {
		t.Errorf("Expected usage message, got: %s", result)
	}
}

func TestHandleCommand_ListChannels_NoManager(t *testing.T) {
	loop := createTestAgentLoop(t)
	msg := createTestInboundMessage("test", "user1", "chat1", "/list channels")
	result, handled := loop.handleCommand(nil, msg)
	if !handled {
		t.Error("Expected handled=true")
	}
	if !helpersContains(result, "not initialized") {
		t.Errorf("Expected 'not initialized' message, got: %s", result)
	}
}

func TestHandleCommand_SwitchModel(t *testing.T) {
	loop := createTestAgentLoop(t)
	msg := createTestInboundMessage("test", "user1", "chat1", "/switch model to new-model")
	result, handled := loop.handleCommand(nil, msg)
	if !handled {
		t.Error("Expected handled=true for /switch command")
	}
	if !helpersContains(result, "Switched model") {
		t.Errorf("Expected 'Switched model' in result, got: %s", result)
	}
}

func TestHandleCommand_UnknownCommand(t *testing.T) {
	loop := createTestAgentLoop(t)
	msg := createTestInboundMessage("test", "user1", "chat1", "/unknown_cmd")
	result, handled := loop.handleCommand(nil, msg)
	if handled {
		t.Error("Expected handled=false for unknown command")
	}
	if result != "" {
		t.Errorf("Expected empty result for unknown command, got: %s", result)
	}
}

// --- GetStartupInfo tests ---

func TestGetStartupInfo_Basic(t *testing.T) {
	loop := createTestAgentLoop(t)
	info := loop.GetStartupInfo()

	if info == nil {
		t.Fatal("Expected non-nil info")
	}
	if _, ok := info["tools"]; !ok {
		t.Error("Expected 'tools' in startup info")
	}
	if _, ok := info["skills"]; !ok {
		t.Error("Expected 'skills' in startup info")
	}
	if _, ok := info["agents"]; !ok {
		t.Error("Expected 'agents' in startup info")
	}
}

// helper to check substring - use different name to avoid conflict with forcecompression_test.go
func helpersContains(s, substr string) bool {
	return len(s) >= len(substr) && (s == substr || helpersContainsMiddle(s, substr))
}

func helpersContainsMiddle(s, substr string) bool {
	for i := 0; i <= len(s)-len(substr); i++ {
		if s[i:i+len(substr)] == substr {
			return true
		}
	}
	return false
}

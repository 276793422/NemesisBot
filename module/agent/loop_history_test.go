// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Chat History Bus Architecture Tests

package agent

import (
	"context"
	"encoding/json"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/routing"
)

// --- handleHistoryRequest unit tests ---

func TestHandleHistoryRequest_BasicHistory(t *testing.T) {
	loop := createTestAgentLoop(t)
	agent := loop.registry.GetDefaultAgent()
	sessionKey := routing.BuildAgentMainSessionKey(agent.ID)

	// Pre-populate session history
	agent.Sessions.AddMessage(sessionKey, "system", "System prompt")
	agent.Sessions.AddMessage(sessionKey, "user", "Hello")
	agent.Sessions.AddMessage(sessionKey, "assistant", "Hi there!")
	agent.Sessions.AddMessage(sessionKey, "tool", "tool result") // should be filtered
	agent.Sessions.AddMessage(sessionKey, "user", "How are you?")

	// Send history request via processMessage
	content, _ := json.Marshal(map[string]interface{}{
		"request_id": "req-1",
		"limit":      20,
	})
	msg := bus.InboundMessage{
		Channel:  "web",
		SenderID: "user1",
		ChatID:   "web:session-1",
		Content:  string(content),
		Metadata: map[string]string{"request_type": "history"},
	}

	// Start a goroutine to read outbound
	outboundCh := loop.bus.OutboundChannel()

	// Process message
	_, response, err := loop.processMessage(context.Background(), msg)
	if err != nil {
		t.Fatalf("processMessage error: %v", err)
	}
	// processMessage should return empty (history handled directly via bus)
	if response != "" {
		t.Errorf("Expected empty response from processMessage for history request, got: %s", response)
	}

	// Read outbound message
	select {
	case outbound := <-outboundCh:
		if outbound.Type != "history" {
			t.Errorf("Outbound Type = %q, want %q", outbound.Type, "history")
		}
		if outbound.Channel != "web" {
			t.Errorf("Outbound Channel = %q, want %q", outbound.Channel, "web")
		}
		if outbound.ChatID != "web:session-1" {
			t.Errorf("Outbound ChatID = %q, want %q", outbound.ChatID, "web:session-1")
		}

		// Parse response content
		var data map[string]interface{}
		if err := json.Unmarshal([]byte(outbound.Content), &data); err != nil {
			t.Fatalf("Failed to parse outbound content: %v", err)
		}

		if data["request_id"] != "req-1" {
			t.Errorf("request_id = %v, want req-1", data["request_id"])
		}
		// Should have 3 user/assistant messages (system and tool filtered out)
		totalCount := int(data["total_count"].(float64))
		if totalCount != 3 {
			t.Errorf("total_count = %d, want 3 (filtered system+tool)", totalCount)
		}
		messages := data["messages"].([]interface{})
		if len(messages) != 3 {
			t.Errorf("messages count = %d, want 3", len(messages))
		}
	case <-time.After(2 * time.Second):
		t.Fatal("Timeout waiting for outbound history message")
	}
}

func TestHandleHistoryRequest_WithLimit(t *testing.T) {
	loop := createTestAgentLoop(t)
	agent := loop.registry.GetDefaultAgent()
	sessionKey := routing.BuildAgentMainSessionKey(agent.ID)

	// Add 10 messages (5 user + 5 assistant)
	for i := 0; i < 10; i++ {
		role := "user"
		if i%2 == 1 {
			role = "assistant"
		}
		agent.Sessions.AddMessage(sessionKey, role, "msg")
	}

	content, _ := json.Marshal(map[string]interface{}{
		"request_id": "req-2",
		"limit":      3,
	})
	msg := bus.InboundMessage{
		Channel:  "web",
		ChatID:   "web:session-2",
		Content:  string(content),
		Metadata: map[string]string{"request_type": "history"},
	}

	outboundCh := loop.bus.OutboundChannel()
	_, _, _ = loop.processMessage(context.Background(), msg)

	select {
	case outbound := <-outboundCh:
		var data map[string]interface{}
		json.Unmarshal([]byte(outbound.Content), &data)

		totalCount := int(data["total_count"].(float64))
		if totalCount != 10 {
			t.Errorf("total_count = %d, want 10", totalCount)
		}
		messages := data["messages"].([]interface{})
		if len(messages) != 3 {
			t.Errorf("messages count = %d, want 3", len(messages))
		}
		if !data["has_more"].(bool) {
			t.Error("has_more should be true when there are older messages")
		}
	case <-time.After(2 * time.Second):
		t.Fatal("Timeout")
	}
}

func TestHandleHistoryRequest_WithBeforeIndex(t *testing.T) {
	loop := createTestAgentLoop(t)
	agent := loop.registry.GetDefaultAgent()
	sessionKey := routing.BuildAgentMainSessionKey(agent.ID)

	// Add 10 messages
	for i := 0; i < 10; i++ {
		role := "user"
		if i%2 == 1 {
			role = "assistant"
		}
		agent.Sessions.AddMessage(sessionKey, role, "msg")
	}

	beforeIndex := 7
	content, _ := json.Marshal(map[string]interface{}{
		"request_id":   "req-3",
		"limit":        3,
		"before_index": beforeIndex,
	})
	msg := bus.InboundMessage{
		Channel:  "web",
		ChatID:   "web:session-3",
		Content:  string(content),
		Metadata: map[string]string{"request_type": "history"},
	}

	outboundCh := loop.bus.OutboundChannel()
	_, _, _ = loop.processMessage(context.Background(), msg)

	select {
	case outbound := <-outboundCh:
		var data map[string]interface{}
		json.Unmarshal([]byte(outbound.Content), &data)

		oldestIndex := int(data["oldest_index"].(float64))
		if oldestIndex != 4 {
			t.Errorf("oldest_index = %d, want 4", oldestIndex)
		}
		messages := data["messages"].([]interface{})
		if len(messages) != 3 {
			t.Errorf("messages count = %d, want 3", len(messages))
		}
		if !data["has_more"].(bool) {
			t.Error("has_more should be true")
		}
	case <-time.After(2 * time.Second):
		t.Fatal("Timeout")
	}
}

func TestHandleHistoryRequest_EmptySession(t *testing.T) {
	loop := createTestAgentLoop(t)

	content, _ := json.Marshal(map[string]interface{}{
		"request_id": "req-4",
		"limit":      20,
	})
	msg := bus.InboundMessage{
		Channel:  "web",
		ChatID:   "web:session-4",
		Content:  string(content),
		Metadata: map[string]string{"request_type": "history"},
	}

	outboundCh := loop.bus.OutboundChannel()
	_, _, _ = loop.processMessage(context.Background(), msg)

	select {
	case outbound := <-outboundCh:
		var data map[string]interface{}
		json.Unmarshal([]byte(outbound.Content), &data)

		totalCount := int(data["total_count"].(float64))
		if totalCount != 0 {
			t.Errorf("total_count = %d, want 0", totalCount)
		}
		messages := data["messages"].([]interface{})
		if len(messages) != 0 {
			t.Errorf("messages count = %d, want 0", len(messages))
		}
		if data["has_more"].(bool) {
			t.Error("has_more should be false for empty session")
		}
	case <-time.After(2 * time.Second):
		t.Fatal("Timeout")
	}
}

func TestHandleHistoryRequest_DefaultLimit(t *testing.T) {
	loop := createTestAgentLoop(t)
	agent := loop.registry.GetDefaultAgent()
	sessionKey := routing.BuildAgentMainSessionKey(agent.ID)

	// Add 30 messages
	for i := 0; i < 30; i++ {
		role := "user"
		if i%2 == 1 {
			role = "assistant"
		}
		agent.Sessions.AddMessage(sessionKey, role, "msg")
	}

	// No limit specified → default should be 20
	content, _ := json.Marshal(map[string]interface{}{
		"request_id": "req-5",
	})
	msg := bus.InboundMessage{
		Channel:  "web",
		ChatID:   "web:session-5",
		Content:  string(content),
		Metadata: map[string]string{"request_type": "history"},
	}

	outboundCh := loop.bus.OutboundChannel()
	_, _, _ = loop.processMessage(context.Background(), msg)

	select {
	case outbound := <-outboundCh:
		var data map[string]interface{}
		json.Unmarshal([]byte(outbound.Content), &data)

		messages := data["messages"].([]interface{})
		if len(messages) != 20 {
			t.Errorf("messages count = %d, want 20 (default limit)", len(messages))
		}
		if !data["has_more"].(bool) {
			t.Error("has_more should be true with 30 messages and limit 20")
		}
	case <-time.After(2 * time.Second):
		t.Fatal("Timeout")
	}
}

func TestHandleHistoryRequest_FiltersToolAndSystemMessages(t *testing.T) {
	loop := createTestAgentLoop(t)
	agent := loop.registry.GetDefaultAgent()
	sessionKey := routing.BuildAgentMainSessionKey(agent.ID)

	// Add messages with various roles
	agent.Sessions.AddMessage(sessionKey, "system", "system note")
	agent.Sessions.AddMessage(sessionKey, "user", "Hello")
	agent.Sessions.AddMessage(sessionKey, "assistant", "Let me check")
	agent.Sessions.AddMessage(sessionKey, "tool", "tool result data")
	agent.Sessions.AddMessage(sessionKey, "assistant", "Here's the answer")

	content, _ := json.Marshal(map[string]interface{}{
		"request_id": "req-6",
		"limit":      20,
	})
	msg := bus.InboundMessage{
		Channel:  "web",
		ChatID:   "web:session-6",
		Content:  string(content),
		Metadata: map[string]string{"request_type": "history"},
	}

	outboundCh := loop.bus.OutboundChannel()
	_, _, _ = loop.processMessage(context.Background(), msg)

	select {
	case outbound := <-outboundCh:
		var data map[string]interface{}
		json.Unmarshal([]byte(outbound.Content), &data)

		// Only user + assistant = 3 messages
		totalCount := int(data["total_count"].(float64))
		if totalCount != 3 {
			t.Errorf("total_count = %d, want 3 (system and tool filtered)", totalCount)
		}
		messages := data["messages"].([]interface{})
		for _, m := range messages {
			msgMap := m.(map[string]interface{})
			role := msgMap["role"].(string)
			if role != "user" && role != "assistant" {
				t.Errorf("Unexpected role in filtered results: %q", role)
			}
		}
	case <-time.After(2 * time.Second):
		t.Fatal("Timeout")
	}
}

func TestHandleHistoryRequest_InvalidJSON(t *testing.T) {
	loop := createTestAgentLoop(t)

	msg := bus.InboundMessage{
		Channel:  "web",
		ChatID:   "web:session-err",
		Content:  "not valid json",
		Metadata: map[string]string{"request_type": "history"},
	}

	outboundCh := loop.bus.OutboundChannel()
	_, _, _ = loop.processMessage(context.Background(), msg)

	select {
	case outbound := <-outboundCh:
		// Should still get a response (empty), not hang
		if outbound.Type != "history" {
			t.Errorf("Type = %q, want history", outbound.Type)
		}
		var data map[string]interface{}
		json.Unmarshal([]byte(outbound.Content), &data)
		if data["request_id"] != "" {
			t.Errorf("request_id should be empty for invalid JSON, got %v", data["request_id"])
		}
		totalCount := int(data["total_count"].(float64))
		if totalCount != 0 {
			t.Errorf("total_count = %d, want 0 for error case", totalCount)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("Timeout - should still respond even with invalid JSON")
	}
}

func TestHandleHistoryRequest_BeforeIndexZero(t *testing.T) {
	loop := createTestAgentLoop(t)
	agent := loop.registry.GetDefaultAgent()
	sessionKey := routing.BuildAgentMainSessionKey(agent.ID)

	for i := 0; i < 5; i++ {
		agent.Sessions.AddMessage(sessionKey, "user", "msg")
	}

	beforeIndex := 0
	content, _ := json.Marshal(map[string]interface{}{
		"request_id":   "req-7",
		"limit":        10,
		"before_index": beforeIndex,
	})
	msg := bus.InboundMessage{
		Channel:  "web",
		ChatID:   "web:session-7",
		Content:  string(content),
		Metadata: map[string]string{"request_type": "history"},
	}

	outboundCh := loop.bus.OutboundChannel()
	_, _, _ = loop.processMessage(context.Background(), msg)

	select {
	case outbound := <-outboundCh:
		var data map[string]interface{}
		json.Unmarshal([]byte(outbound.Content), &data)

		// Nothing before index 0
		messages := data["messages"].([]interface{})
		if len(messages) != 0 {
			t.Errorf("messages count = %d, want 0 (nothing before index 0)", len(messages))
		}
		if data["has_more"].(bool) {
			t.Error("has_more should be false")
		}
	case <-time.After(2 * time.Second):
		t.Fatal("Timeout")
	}
}

// --- processMessage integration: verify history bypasses LLM ---

func TestProcessMessage_HistoryRequest_BypassesLLM(t *testing.T) {
	loop := createTestAgentLoop(t)
	agent := loop.registry.GetDefaultAgent()
	sessionKey := routing.BuildAgentMainSessionKey(agent.ID)

	// Add some history
	agent.Sessions.AddMessage(sessionKey, "user", "test")

	content, _ := json.Marshal(map[string]interface{}{
		"request_id": "req-bypass",
		"limit":      10,
	})
	msg := bus.InboundMessage{
		Channel:  "web",
		ChatID:   "web:session-bypass",
		Content:  string(content),
		Metadata: map[string]string{"request_type": "history"},
	}

	agentID, response, err := loop.processMessage(context.Background(), msg)
	if err != nil {
		t.Fatalf("Unexpected error: %v", err)
	}
	// agentID and response should both be empty - history is handled entirely via bus
	if agentID != "" {
		t.Errorf("agentID = %q, want empty (history bypasses LLM)", agentID)
	}
	if response != "" {
		t.Errorf("response = %q, want empty (history sent via bus, not returned)", response)
	}
}

func TestProcessMessage_NonHistoryMessage_NotAffected(t *testing.T) {
	loop := createTestAgentLoop(t)

	// A normal message should still go through LLM processing
	msg := bus.InboundMessage{
		Channel:  "web",
		SenderID: "user1",
		ChatID:   "web:session-normal",
		Content:  "Hello world",
		Metadata: map[string]string{}, // no request_type
	}

	_, response, _ := loop.processMessage(context.Background(), msg)
	// Should get a response from the mock LLM
	if response == "" {
		t.Error("Expected non-empty response for normal message")
	}
}

func TestProcessMessage_HistoryRequest_NilMetadata(t *testing.T) {
	loop := createTestAgentLoop(t)

	// Metadata is nil → should not trigger history handling
	msg := bus.InboundMessage{
		Channel:  "web",
		ChatID:   "web:session-nilmeta",
		Content:  `{"request_id":"x"}`,
		Metadata: nil,
	}

	_, response, _ := loop.processMessage(context.Background(), msg)
	// Should go through normal LLM processing (not history)
	if response == "" {
		t.Error("Expected LLM response, not history handling, when Metadata is nil")
	}
}

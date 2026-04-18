// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// History Provider Unit Tests

package services_test

import (
	"testing"

	"github.com/276793422/NemesisBot/module/providers/protocoltypes"
	"github.com/276793422/NemesisBot/module/services"
	"github.com/276793422/NemesisBot/module/session"
)

func newTestSessionManager() *session.SessionManager {
	// Use empty storage path for in-memory only
	return session.NewSessionManager("")
}

func TestSessionHistoryProvider_GetHistory_Basic(t *testing.T) {
	sm := newTestSessionManager()
	sessionKey := "test:session:1"

	// Add some messages
	sm.AddMessage(sessionKey, "user", "Hello")
	sm.AddMessage(sessionKey, "assistant", "Hi there!")
	sm.AddMessage(sessionKey, "user", "How are you?")
	sm.AddMessage(sessionKey, "assistant", "I'm doing well!")

	provider := services.NewSessionHistoryProvider(sm, sessionKey)

	page, err := provider.GetHistory(10, nil)
	if err != nil {
		t.Fatalf("GetHistory() error = %v", err)
	}

	if page.TotalCount != 4 {
		t.Errorf("TotalCount = %d, want 4", page.TotalCount)
	}
	if len(page.Messages) != 4 {
		t.Errorf("Messages count = %d, want 4", len(page.Messages))
	}
	if page.HasMore {
		t.Error("HasMore should be false")
	}
	if page.OldestIndex != 0 {
		t.Errorf("OldestIndex = %d, want 0", page.OldestIndex)
	}

	// Verify message content
	if page.Messages[0].Role != "user" || page.Messages[0].Content != "Hello" {
		t.Errorf("Message[0] = %+v, want role=user content=Hello", page.Messages[0])
	}
	if page.Messages[3].Role != "assistant" || page.Messages[3].Content != "I'm doing well!" {
		t.Errorf("Message[3] = %+v, want role=assistant content=I'm doing well!", page.Messages[3])
	}
}

func TestSessionHistoryProvider_GetHistory_WithLimit(t *testing.T) {
	sm := newTestSessionManager()
	sessionKey := "test:session:2"

	// Add 10 messages
	for i := 0; i < 10; i++ {
		if i%2 == 0 {
			sm.AddMessage(sessionKey, "user", "msg")
		} else {
			sm.AddMessage(sessionKey, "assistant", "reply")
		}
	}

	provider := services.NewSessionHistoryProvider(sm, sessionKey)

	// Request only 3 messages (most recent)
	page, err := provider.GetHistory(3, nil)
	if err != nil {
		t.Fatalf("GetHistory() error = %v", err)
	}

	if page.TotalCount != 10 {
		t.Errorf("TotalCount = %d, want 10", page.TotalCount)
	}
	if len(page.Messages) != 3 {
		t.Errorf("Messages count = %d, want 3", len(page.Messages))
	}
	if !page.HasMore {
		t.Error("HasMore should be true")
	}
	if page.OldestIndex != 7 {
		t.Errorf("OldestIndex = %d, want 7", page.OldestIndex)
	}
}

func TestSessionHistoryProvider_GetHistory_WithBeforeIndex(t *testing.T) {
	sm := newTestSessionManager()
	sessionKey := "test:session:3"

	// Add 10 messages
	for i := 0; i < 10; i++ {
		if i%2 == 0 {
			sm.AddMessage(sessionKey, "user", "msg")
		} else {
			sm.AddMessage(sessionKey, "assistant", "reply")
		}
	}

	provider := services.NewSessionHistoryProvider(sm, sessionKey)

	// Request 3 messages before index 7 (should get indices 4,5,6)
	beforeIndex := 7
	page, err := provider.GetHistory(3, &beforeIndex)
	if err != nil {
		t.Fatalf("GetHistory() error = %v", err)
	}

	if len(page.Messages) != 3 {
		t.Errorf("Messages count = %d, want 3", len(page.Messages))
	}
	if page.OldestIndex != 4 {
		t.Errorf("OldestIndex = %d, want 4", page.OldestIndex)
	}
	if !page.HasMore {
		t.Error("HasMore should be true")
	}
}

func TestSessionHistoryProvider_GetHistory_EmptySession(t *testing.T) {
	sm := newTestSessionManager()
	sessionKey := "test:session:empty"

	provider := services.NewSessionHistoryProvider(sm, sessionKey)

	page, err := provider.GetHistory(20, nil)
	if err != nil {
		t.Fatalf("GetHistory() error = %v", err)
	}

	if page.TotalCount != 0 {
		t.Errorf("TotalCount = %d, want 0", page.TotalCount)
	}
	if len(page.Messages) != 0 {
		t.Errorf("Messages count = %d, want 0", len(page.Messages))
	}
	if page.HasMore {
		t.Error("HasMore should be false")
	}
}

func TestSessionHistoryProvider_GetHistory_FiltersToolMessages(t *testing.T) {
	sm := newTestSessionManager()
	sessionKey := "test:session:filter"

	// Add messages with various roles
	sm.AddFullMessage(sessionKey, protocoltypes.Message{Role: "user", Content: "Hello"})
	sm.AddFullMessage(sessionKey, protocoltypes.Message{Role: "assistant", Content: "Let me check", ToolCalls: []protocoltypes.ToolCall{{ID: "tc1"}}})
	sm.AddFullMessage(sessionKey, protocoltypes.Message{Role: "tool", Content: "result data", ToolCallID: "tc1"})
	sm.AddFullMessage(sessionKey, protocoltypes.Message{Role: "assistant", Content: "Here's the answer"})
	sm.AddFullMessage(sessionKey, protocoltypes.Message{Role: "system", Content: "system note"})

	provider := services.NewSessionHistoryProvider(sm, sessionKey)

	page, err := provider.GetHistory(20, nil)
	if err != nil {
		t.Fatalf("GetHistory() error = %v", err)
	}

	// Only user and assistant messages should be returned (3 out of 5)
	// Filtered out: 1 tool message, 1 system message
	if page.TotalCount != 3 {
		t.Errorf("TotalCount = %d, want 3", page.TotalCount)
	}
	for _, msg := range page.Messages {
		if msg.Role != "user" && msg.Role != "assistant" {
			t.Errorf("Unexpected role in filtered results: %q", msg.Role)
		}
	}
}

func TestSessionHistoryProvider_GetHistory_BeforeIndexExceedsTotal(t *testing.T) {
	sm := newTestSessionManager()
	sessionKey := "test:session:overflow"

	sm.AddMessage(sessionKey, "user", "Hello")
	sm.AddMessage(sessionKey, "assistant", "Hi")

	provider := services.NewSessionHistoryProvider(sm, sessionKey)

	// beforeIndex beyond total should return from beginning
	beforeIndex := 100
	page, err := provider.GetHistory(10, &beforeIndex)
	if err != nil {
		t.Fatalf("GetHistory() error = %v", err)
	}

	// All 2 messages should be returned since beforeIndex > total
	if len(page.Messages) != 2 {
		t.Errorf("Messages count = %d, want 2", len(page.Messages))
	}
}

func TestSessionHistoryProvider_GetHistory_NonexistentSession(t *testing.T) {
	sm := newTestSessionManager()
	provider := services.NewSessionHistoryProvider(sm, "nonexistent:key")

	page, err := provider.GetHistory(20, nil)
	if err != nil {
		t.Fatalf("GetHistory() error = %v", err)
	}

	if page.TotalCount != 0 {
		t.Errorf("TotalCount = %d, want 0", page.TotalCount)
	}
	if len(page.Messages) != 0 {
		t.Errorf("Messages count = %d, want 0", len(page.Messages))
	}
}

func TestSessionHistoryProvider_GetHistory_BeforeIndexZero(t *testing.T) {
	sm := newTestSessionManager()
	sessionKey := "test:session:before_zero"

	for i := 0; i < 5; i++ {
		sm.AddMessage(sessionKey, "user", "msg")
	}

	provider := services.NewSessionHistoryProvider(sm, sessionKey)

	// beforeIndex = 0 means "before the very first message" → should return empty
	beforeIndex := 0
	page, err := provider.GetHistory(10, &beforeIndex)
	if err != nil {
		t.Fatalf("GetHistory() error = %v", err)
	}

	if len(page.Messages) != 0 {
		t.Errorf("Messages count = %d, want 0 (nothing before index 0)", len(page.Messages))
	}
	if page.HasMore {
		t.Error("HasMore should be false")
	}
}

func TestSessionHistoryProvider_GetHistory_BeforeIndexEqualsTotal(t *testing.T) {
	sm := newTestSessionManager()
	sessionKey := "test:session:before_eq_total"

	for i := 0; i < 5; i++ {
		sm.AddMessage(sessionKey, "user", "msg")
	}

	provider := services.NewSessionHistoryProvider(sm, sessionKey)

	// beforeIndex = totalCount should behave same as null (return latest)
	beforeIndex := 5
	page, err := provider.GetHistory(3, &beforeIndex)
	if err != nil {
		t.Fatalf("GetHistory() error = %v", err)
	}

	if len(page.Messages) != 3 {
		t.Errorf("Messages count = %d, want 3", len(page.Messages))
	}
	if !page.HasMore {
		t.Error("HasMore should be true")
	}
	if page.OldestIndex != 2 {
		t.Errorf("OldestIndex = %d, want 2", page.OldestIndex)
	}
}

func TestSessionHistoryProvider_GetHistory_LimitLargerThanTotal(t *testing.T) {
	sm := newTestSessionManager()
	sessionKey := "test:session:large_limit"

	sm.AddMessage(sessionKey, "user", "msg1")
	sm.AddMessage(sessionKey, "assistant", "reply1")

	provider := services.NewSessionHistoryProvider(sm, sessionKey)

	// Request more messages than exist
	page, err := provider.GetHistory(100, nil)
	if err != nil {
		t.Fatalf("GetHistory() error = %v", err)
	}

	if len(page.Messages) != 2 {
		t.Errorf("Messages count = %d, want 2", len(page.Messages))
	}
	if page.HasMore {
		t.Error("HasMore should be false")
	}
	if page.OldestIndex != 0 {
		t.Errorf("OldestIndex = %d, want 0", page.OldestIndex)
	}
}

func TestSessionHistoryProvider_GetHistory_NegativeBeforeIndex(t *testing.T) {
	sm := newTestSessionManager()
	sessionKey := "test:session:negative_idx"

	for i := 0; i < 5; i++ {
		sm.AddMessage(sessionKey, "user", "msg")
	}

	provider := services.NewSessionHistoryProvider(sm, sessionKey)

	// Negative beforeIndex should be ignored (treated as null)
	beforeIndex := -1
	page, err := provider.GetHistory(3, &beforeIndex)
	if err != nil {
		t.Fatalf("GetHistory() error = %v", err)
	}

	// Should return latest 3 messages (same as null beforeIndex)
	if len(page.Messages) != 3 {
		t.Errorf("Messages count = %d, want 3", len(page.Messages))
	}
}

func TestSessionHistoryProvider_GetHistory_LimitOne(t *testing.T) {
	sm := newTestSessionManager()
	sessionKey := "test:session:limit_one"

	for i := 0; i < 5; i++ {
		sm.AddMessage(sessionKey, "user", "msg")
	}

	provider := services.NewSessionHistoryProvider(sm, sessionKey)

	page, err := provider.GetHistory(1, nil)
	if err != nil {
		t.Fatalf("GetHistory() error = %v", err)
	}

	if len(page.Messages) != 1 {
		t.Errorf("Messages count = %d, want 1", len(page.Messages))
	}
	if page.Messages[0].Content != "msg" {
		t.Errorf("Message content = %q, want %q", page.Messages[0].Content, "msg")
	}
	if !page.HasMore {
		t.Error("HasMore should be true")
	}
	if page.OldestIndex != 4 {
		t.Errorf("OldestIndex = %d, want 4", page.OldestIndex)
	}
}

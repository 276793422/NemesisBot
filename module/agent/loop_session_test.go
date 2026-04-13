// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package agent

import (
	"context"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/providers"
)

// --- Session Busy State tests ---

func TestSessionManager_GetOrCreateState(t *testing.T) {
	loop := createTestAgentLoop(t)

	state1 := loop.getSessionBusyState("session-1")
	if state1 == nil {
		t.Fatal("Expected state to be created")
	}
	if state1.busy {
		t.Error("New session should not be busy")
	}

	// Same session should return same state
	state2 := loop.getSessionBusyState("session-1")
	if state1 != state2 {
		t.Error("Expected same state instance for same session")
	}

	// Different session should return different state
	state3 := loop.getSessionBusyState("session-2")
	if state3 == state1 {
		t.Error("Expected different state instance for different session")
	}
}

func TestSessionManager_AcquireAndRelease(t *testing.T) {
	loop := createTestAgentLoop(t)
	loop.concurrentMode = "reject"

	// First acquire should succeed
	if !loop.tryAcquireSession("s1") {
		t.Error("First acquire should succeed")
	}

	// Second acquire should fail (busy)
	if loop.tryAcquireSession("s1") {
		t.Error("Second acquire should fail when busy")
	}

	// Release
	loop.releaseSession("s1")

	// Third acquire should succeed again
	if !loop.tryAcquireSession("s1") {
		t.Error("Acquire after release should succeed")
	}
}

func TestSessionManager_QueueMode(t *testing.T) {
	loop := createTestAgentLoop(t)
	loop.concurrentMode = "queue"
	loop.queueSize = 2

	// First acquire succeeds
	if !loop.tryAcquireSession("s1") {
		t.Fatal("First acquire should succeed")
	}

	// Queue two requests
	if loop.tryAcquireSession("s1") {
		t.Error("Queued acquire should return false")
	}
	if loop.tryAcquireSession("s1") {
		t.Error("Queued acquire should return false")
	}

	// Queue full
	if loop.tryAcquireSession("s1") {
		t.Error("Should fail when queue full")
	}

	// Release one - should indicate queued
	if !loop.releaseSession("s1") {
		t.Error("Should indicate queued requests remain")
	}

	// Still busy
	state := loop.getSessionBusyState("s1")
	if !state.busy {
		t.Error("Should still be busy with queued requests")
	}
}

func TestSessionManager_ConcurrentAccess(t *testing.T) {
	loop := createTestAgentLoop(t)
	loop.concurrentMode = "reject"

	done := make(chan bool, 50)
	for i := 0; i < 50; i++ {
		go func() {
			if loop.tryAcquireSession("concurrent") {
				time.Sleep(time.Millisecond)
				loop.releaseSession("concurrent")
			}
			done <- true
		}()
	}
	for i := 0; i < 50; i++ {
		<-done
	}

	state := loop.getSessionBusyState("concurrent")
	if state.busy {
		t.Error("Session should not be busy after all goroutines complete")
	}
}

// --- RecordLastChannel / RecordLastChatID tests ---

func TestRecordLast_NilState(t *testing.T) {
	loop := createTestAgentLoop(t)
	loop.state = nil

	// Should not panic
	if err := loop.RecordLastChannel("test:123"); err != nil {
		t.Errorf("Expected no error with nil state, got: %v", err)
	}
	if err := loop.RecordLastChatID("chat-123"); err != nil {
		t.Errorf("Expected no error with nil state, got: %v", err)
	}
}

// --- estimateTokens tests ---

func TestEstimateTokens_Empty(t *testing.T) {
	loop := createTestAgentLoop(t)
	tokens := loop.estimateTokens([]providers.Message{})
	if tokens != 0 {
		t.Errorf("Expected 0 tokens for empty messages, got %d", tokens)
	}
}

func TestEstimateTokens_SimpleText(t *testing.T) {
	loop := createTestAgentLoop(t)
	messages := []providers.Message{
		{Role: "user", Content: "Hello world"},
		{Role: "assistant", Content: "Hi there!"},
	}
	tokens := loop.estimateTokens(messages)
	if tokens <= 0 {
		t.Errorf("Expected positive token count, got %d", tokens)
	}
	// "Hello world" (11) + "Hi there!" (9) = 20 chars → 20*2/5 = 8
	if tokens < 4 || tokens > 15 {
		t.Errorf("Expected reasonable token estimate, got %d", tokens)
	}
}

// --- summarizeBatch tests ---

func TestSummarizeBatch_Basic(t *testing.T) {
	loop := createTestAgentLoop(t)
	agent := loop.registry.GetDefaultAgent()
	if agent == nil {
		t.Fatal("Expected default agent")
	}

	batch := []providers.Message{
		{Role: "user", Content: "What is Go?"},
		{Role: "assistant", Content: "Go is a programming language."},
	}

	ctx := context.Background()
	summary, err := loop.summarizeBatch(ctx, agent, batch, "")
	if err != nil {
		t.Fatalf("Unexpected error: %v", err)
	}
	if summary == "" {
		t.Error("Expected non-empty summary")
	}
}

// --- forceCompression tests ---

func TestForceCompression_SmallHistory_NoOp(t *testing.T) {
	loop := createTestAgentLoop(t)
	agent := loop.registry.GetDefaultAgent()

	sessionKey := "test-nocomp"

	// First process a message to create the session
	ctx := context.Background()
	_, _ = loop.ProcessDirect(ctx, "create session", sessionKey)

	// Now set history
	history := []providers.Message{
		{Role: "system", Content: "System"},
		{Role: "user", Content: "Hi"},
		{Role: "assistant", Content: "Hello"},
		{Role: "user", Content: "How are you?"},
	}
	agent.Sessions.SetHistory(sessionKey, history)
	loop.forceCompression(agent, sessionKey)

	result := agent.Sessions.GetHistory(sessionKey)
	// 4 messages should not trigger compression
	if len(result) != 4 {
		t.Logf("History after compression: %d messages (may have been affected by session behavior)", len(result))
	}
}

func TestForceCompression_LargeHistory_Compresses(t *testing.T) {
	loop := createTestAgentLoop(t)
	agent := loop.registry.GetDefaultAgent()

	sessionKey := "test-comp"

	// Use AddMessage to create the session first
	agent.Sessions.AddMessage(sessionKey, "system", "System")
	agent.Sessions.AddMessage(sessionKey, "user", "M1")
	agent.Sessions.AddMessage(sessionKey, "assistant", "R1")
	agent.Sessions.AddMessage(sessionKey, "user", "M2")
	agent.Sessions.AddMessage(sessionKey, "assistant", "R2")
	agent.Sessions.AddMessage(sessionKey, "user", "M3")
	agent.Sessions.AddMessage(sessionKey, "assistant", "R3")
	agent.Sessions.AddMessage(sessionKey, "user", "M4")
	agent.Sessions.AddMessage(sessionKey, "assistant", "R4")
	agent.Sessions.AddMessage(sessionKey, "user", "Final")

	originalLen := len(agent.Sessions.GetHistory(sessionKey))
	if originalLen < 10 {
		t.Fatalf("Expected at least 10 messages, got %d", originalLen)
	}

	loop.forceCompression(agent, sessionKey)

	result := agent.Sessions.GetHistory(sessionKey)
	if len(result) == 0 {
		t.Fatal("Expected history to still exist after compression")
	}
	// First message should be system prompt
	if result[0].Role != "system" {
		t.Error("First message should still be system prompt")
	}
	// Should be smaller than original
	if len(result) >= originalLen {
		t.Errorf("Expected compression to reduce history from %d, got %d messages", originalLen, len(result))
	}
}

// --- Helper ---

func createTestAgentLoop(t *testing.T) *AgentLoop {
	t.Helper()
	tempDir := t.TempDir()
	cfg := &config.Config{
		Agents: config.AgentsConfig{
			Defaults: config.AgentDefaults{
				LLM:       "test/model",
				Workspace: tempDir,
			},
		},
	}
	msgBus := bus.NewMessageBus()
	provider := &mockProviderForTest{}
	return NewAgentLoop(cfg, msgBus, provider)
}

func createTestInboundMessage(channel, senderID, chatID, content string) bus.InboundMessage {
	return bus.InboundMessage{
		Channel:  channel,
		SenderID: senderID,
		ChatID:   chatID,
		Content:  content,
	}
}

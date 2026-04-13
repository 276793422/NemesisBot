// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package agent

import (
	"testing"

	"github.com/276793422/NemesisBot/module/providers"
)

// --- Continuation Save/Load tests ---

func TestContinuation_SaveAndLoad(t *testing.T) {
	loop := createTestAgentLoop(t)

	messages := []providers.Message{
		{Role: "user", Content: "Hello"},
		{Role: "assistant", Content: "Hi there"},
	}

	// Save
	loop.saveContinuation("task-1", messages, "tc-123", "telegram", "chat-456")

	// Load from memory
	data := loop.loadContinuation("task-1")
	if data == nil {
		t.Fatal("Expected continuation data to be found in memory")
	}
	if data.toolCallID != "tc-123" {
		t.Errorf("Expected toolCallID 'tc-123', got '%s'", data.toolCallID)
	}
	if data.channel != "telegram" {
		t.Errorf("Expected channel 'telegram', got '%s'", data.channel)
	}
	if data.chatID != "chat-456" {
		t.Errorf("Expected chatID 'chat-456', got '%s'", data.chatID)
	}
	if len(data.messages) != 2 {
		t.Fatalf("Expected 2 messages, got %d", len(data.messages))
	}
	if data.messages[0].Content != "Hello" {
		t.Errorf("Expected first message 'Hello', got '%s'", data.messages[0].Content)
	}
}

func TestContinuation_LoadNonExistent(t *testing.T) {
	loop := createTestAgentLoop(t)

	data := loop.loadContinuation("nonexistent")
	if data != nil {
		t.Error("Expected nil for non-existent continuation")
	}
}

func TestContinuation_SaveIsDeepCopy(t *testing.T) {
	loop := createTestAgentLoop(t)

	messages := []providers.Message{
		{Role: "user", Content: "original"},
	}

	loop.saveContinuation("task-2", messages, "tc-1", "cli", "chat-1")

	// Modify original slice - should not affect saved data
	messages[0].Content = "modified"

	data := loop.loadContinuation("task-2")
	if data == nil {
		t.Fatal("Expected continuation data")
	}
	if data.messages[0].Content != "original" {
		t.Errorf("Expected 'original' (deep copy), got '%s'", data.messages[0].Content)
	}
}

func TestContinuation_MultipleTasks(t *testing.T) {
	loop := createTestAgentLoop(t)

	messages1 := []providers.Message{{Role: "user", Content: "msg1"}}
	messages2 := []providers.Message{{Role: "user", Content: "msg2"}}

	loop.saveContinuation("task-a", messages1, "tc-a", "cli", "chat-a")
	loop.saveContinuation("task-b", messages2, "tc-b", "cli", "chat-b")

	dataA := loop.loadContinuation("task-a")
	dataB := loop.loadContinuation("task-b")

	if dataA == nil || dataB == nil {
		t.Fatal("Both continuations should exist")
	}
	if dataA.messages[0].Content != "msg1" {
		t.Error("Task A data mismatch")
	}
	if dataB.messages[0].Content != "msg2" {
		t.Error("Task B data mismatch")
	}
}

// --- handleClusterContinuation edge cases ---

func TestHandleClusterContinuation_NoData(t *testing.T) {
	loop := createTestAgentLoop(t)
	// Should not panic when continuation data doesn't exist
	loop.handleClusterContinuation(nil, "nonexistent-task")
}

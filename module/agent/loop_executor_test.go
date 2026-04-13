// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package agent

import (
	"context"
	"testing"
)

// --- updateToolContexts tests ---

func TestUpdateToolContexts_NoPanic(t *testing.T) {
	loop := createTestAgentLoop(t)
	agent := loop.registry.GetDefaultAgent()
	if agent == nil {
		t.Fatal("Expected default agent")
	}

	// Should not panic even when tools don't exist
	loop.updateToolContexts(agent, "test-channel", "chat-123")
}

// --- runAgentLoop edge cases ---

func TestRunAgentLoop_EmptyUserMessage(t *testing.T) {
	loop := createTestAgentLoop(t)
	agent := loop.registry.GetDefaultAgent()

	ctx := context.Background()
	result, err := loop.runAgentLoop(ctx, agent, processOptions{
		SessionKey:      "test-empty",
		Channel:         "cli",
		ChatID:          "test",
		UserMessage:     "",
		DefaultResponse: "No response.",
		EnableSummary:   false,
		SendResponse:    false,
	})
	if err != nil {
		t.Fatalf("Unexpected error: %v", err)
	}
	// Should get default or mock response
	if result == "" {
		t.Error("Expected non-empty result")
	}
}

func TestRunAgentLoop_WithNoHistory(t *testing.T) {
	loop := createTestAgentLoop(t)
	agent := loop.registry.GetDefaultAgent()

	ctx := context.Background()
	result, err := loop.runAgentLoop(ctx, agent, processOptions{
		SessionKey:      "test-nohistory",
		Channel:         "cli",
		ChatID:          "test",
		UserMessage:     "Hello",
		DefaultResponse: "No response.",
		EnableSummary:   false,
		SendResponse:    false,
		NoHistory:       true,
	})
	if err != nil {
		t.Fatalf("Unexpected error: %v", err)
	}
	if result == "" {
		t.Error("Expected non-empty result")
	}
}

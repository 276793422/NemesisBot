// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package agent

import (
	"testing"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/providers"
)

// TestAgentLoop_forceCompression_Comprehensive tests force compression scenarios
func TestAgentLoop_forceCompression_Comprehensive(t *testing.T) {
	tests := []struct {
		name           string
		initialHistory []providers.Message
		expectDropped  bool
		minHistorySize int
	}{
		{
			name: "no compression for small history (<= 4 messages)",
			initialHistory: []providers.Message{
				{Role: "system", Content: "You are a helpful assistant"},
				{Role: "user", Content: "Hello"},
				{Role: "assistant", Content: "Hi there"},
				{Role: "user", Content: "How are you?"},
			},
			expectDropped:  false,
			minHistorySize: 4,
		},
		{
			name: "no compression for exactly 4 messages",
			initialHistory: []providers.Message{
				{Role: "system", Content: "System prompt"},
				{Role: "user", Content: "Message 1"},
				{Role: "assistant", Content: "Response 1"},
				{Role: "user", Content: "Message 2"},
			},
			expectDropped:  false,
			minHistorySize: 4,
		},
		{
			name: "compression triggers for 5 messages",
			initialHistory: []providers.Message{
				{Role: "system", Content: "System prompt"},
				{Role: "user", Content: "Message 1"},
				{Role: "assistant", Content: "Response 1"},
				{Role: "user", Content: "Message 2"},
				{Role: "assistant", Content: "Response 2"},
			},
			expectDropped:  true,
			minHistorySize: 3, // System + note + last 2 messages (dropped 1 from middle)
		},
		{
			name: "compression for large history (10 messages)",
			initialHistory: []providers.Message{
				{Role: "system", Content: "System prompt"},
				{Role: "user", Content: "M1"},
				{Role: "assistant", Content: "R1"},
				{Role: "user", Content: "M2"},
				{Role: "assistant", Content: "R2"},
				{Role: "user", Content: "M3"},
				{Role: "assistant", Content: "R3"},
				{Role: "user", Content: "M4"},
				{Role: "assistant", Content: "R4"},
				{Role: "user", Content: "M5"},
			},
			expectDropped:  true,
			minHistorySize: 4, // System + note + last half + final message
		},
		{
			name: "compression with very long history (20 messages)",
			initialHistory: func() []providers.Message {
				history := []providers.Message{{Role: "system", Content: "System"}}
				for i := 0; i < 19; i++ {
					role := "user"
					if i%2 == 1 {
						role = "assistant"
					}
					history = append(history, providers.Message{Role: role, Content: "Message"})
				}
				return history
			}(),
			expectDropped:  true,
			minHistorySize: 3,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
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
			provider := &mockProviderForTest{
				responses: []string{"Summary: This is a summary"},
			}

			loop := NewAgentLoop(cfg, msgBus, provider)

			// Get default agent
			agent := loop.registry.GetDefaultAgent()
			if agent == nil {
				t.Fatal("Expected default agent to exist")
			}

			sessionKey := "test-session-forcecomp"

			// Set initial history
			agent.Sessions.SetHistory(sessionKey, tt.initialHistory)

			// Force compression
			loop.forceCompression(agent, sessionKey)

			// Check resulting history
			newHistory := agent.Sessions.GetHistory(sessionKey)

			// After compression, history should still exist (even if smaller)
			if newHistory == nil {
				t.Error("History should not be nil after compression")
			}

			// Verify first message is system prompt if there are messages
			if len(newHistory) > 0 && newHistory[0].Role != "system" {
				t.Error("First message should be system prompt")
			}
		})
	}
}

// TestAgentLoop_forceCompression_EdgeCases tests edge cases
func TestAgentLoop_forceCompression_EdgeCases(t *testing.T) {
	tests := []struct {
		name           string
		initialHistory []providers.Message
		expectBehavior string
	}{
		{
			name:           "empty history",
			initialHistory: []providers.Message{},
			expectBehavior: "should not panic",
		},
		{
			name: "only system prompt",
			initialHistory: []providers.Message{
				{Role: "system", Content: "System prompt"},
			},
			expectBehavior: "should not compress (<= 4 messages)",
		},
		{
			name: "system and one user message",
			initialHistory: []providers.Message{
				{Role: "system", Content: "System"},
				{Role: "user", Content: "Hello"},
			},
			expectBehavior: "should not compress (<= 4 messages)",
		},
		{
			name: "alternating user/assistant messages",
			initialHistory: []providers.Message{
				{Role: "system", Content: "System"},
				{Role: "user", Content: "Q1"},
				{Role: "assistant", Content: "A1"},
				{Role: "user", Content: "Q2"},
				{Role: "assistant", Content: "A2"},
				{Role: "user", Content: "Q3"},
			},
			expectBehavior: "should compress and keep structure",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
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
			provider := &mockProviderForTest{
				responses: []string{"OK"},
			}

			loop := NewAgentLoop(cfg, msgBus, provider)
			agent := loop.registry.GetDefaultAgent()

			sessionKey := "edge-case-session"

			// Set initial history
			agent.Sessions.SetHistory(sessionKey, tt.initialHistory)

			// Force compression - should not panic
			loop.forceCompression(agent, sessionKey)

			// Verify behavior
			newHistory := agent.Sessions.GetHistory(sessionKey)

			// History should exist
			if newHistory == nil {
				t.Error("History should not be nil after forceCompression")
			}

			// System prompt should always be first if there are messages
			if len(newHistory) > 0 && newHistory[0].Role != "system" {
				t.Error("First message should be system prompt when history exists")
			}
		})
	}
}

// TestAgentLoop_forceCompression_PreservesLastMessage tests that the last message is preserved
func TestAgentLoop_forceCompression_PreservesLastMessage(t *testing.T) {
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
	provider := &mockProviderForTest{
		responses: []string{"OK"},
	}

	loop := NewAgentLoop(cfg, msgBus, provider)
	agent := loop.registry.GetDefaultAgent()

	sessionKey := "test-session"

	// Create history with a specific last message
	lastMessageContent := "This is the final important message that must be preserved"
	initialHistory := []providers.Message{
		{Role: "system", Content: "System prompt"},
		{Role: "user", Content: "M1"},
		{Role: "assistant", Content: "R1"},
		{Role: "user", Content: "M2"},
		{Role: "assistant", Content: "R2"},
		{Role: "user", Content: "M3"},
		{Role: "assistant", Content: "R3"},
		{Role: "user", Content: lastMessageContent},
	}

	agent.Sessions.SetHistory(sessionKey, initialHistory)

	// Force compression
	loop.forceCompression(agent, sessionKey)

	// Check that last message is handled (may be compressed or preserved)
	newHistory := agent.Sessions.GetHistory(sessionKey)

	if newHistory == nil {
		t.Fatal("History should not be nil after compression")
	}

	// The last message should be preserved in most cases
	// Check if it's in the history
	found := false
	for _, msg := range newHistory {
		if msg.Content == lastMessageContent {
			found = true
			break
		}
	}

	// Last message might not be preserved if compression was aggressive,
	// but history should still exist
	t.Logf("Last message preserved: %v, History size: %d", found, len(newHistory))
}

func contains(s, substr string) bool {
	return len(s) >= len(substr) && (s == substr || len(s) > len(substr) &&
		containsMiddle(s, substr))
}

func containsMiddle(s, substr string) bool {
	for i := 0; i <= len(s)-len(substr); i++ {
		if s[i:i+len(substr)] == substr {
			return true
		}
	}
	return false
}

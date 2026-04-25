// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package agent

import (
	"context"
	"fmt"
	"testing"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/observer"
	"github.com/276793422/NemesisBot/module/providers"
	"github.com/276793422/NemesisBot/module/providers/protocoltypes"
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

// mockErrorProvider returns errors for testing retry logic.
type mockErrorProvider struct {
	err       error
	callCount int
}

func (m *mockErrorProvider) Chat(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
	m.callCount++
	return nil, m.err
}

func (m *mockErrorProvider) GetDefaultModel() string {
	return "test-model"
}

// mockTokenErrorProvider returns token error on first call, then succeeds.
type mockTokenErrorProvider struct {
	callCount int
}

func (m *mockTokenErrorProvider) Chat(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
	m.callCount++
	if m.callCount == 1 {
		return nil, fmt.Errorf("token limit exceeded: context length")
	}
	return &providers.LLMResponse{
		Content:      "Success after retry",
		FinishReason: "stop",
		ToolCalls:    []protocoltypes.ToolCall{},
		Usage:        &providers.UsageInfo{PromptTokens: 50, CompletionTokens: 20, TotalTokens: 70},
	}, nil
}

func (m *mockTokenErrorProvider) GetDefaultModel() string {
	return "test-model"
}

// TestRunAgentLoop_LLMError tests handling of LLM errors.
func TestRunAgentLoop_LLMError(t *testing.T) {
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
	provider := &mockErrorProvider{err: fmt.Errorf("connection refused")}

	loop := NewAgentLoop(cfg, msgBus, provider)
	agent := loop.registry.GetDefaultAgent()

	ctx := context.Background()
	_, err := loop.runAgentLoop(ctx, agent, processOptions{
		SessionKey:  "error-test",
		Channel:     "cli",
		ChatID:      "test",
		UserMessage: "Hello",
	})
	if err == nil {
		t.Error("expected error from LLM failure")
	}
}

// TestRunAgentLoop_TokenErrorRetry tests token limit retry.
func TestRunAgentLoop_TokenErrorRetry(t *testing.T) {
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
	provider := &mockTokenErrorProvider{}

	loop := NewAgentLoop(cfg, msgBus, provider)
	agent := loop.registry.GetDefaultAgent()

	ctx := context.Background()
	result, err := loop.runAgentLoop(ctx, agent, processOptions{
		SessionKey:  "token-retry-test",
		Channel:     "cli",
		ChatID:      "test",
		UserMessage: "Hello",
	})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result != "Success after retry" {
		t.Errorf("expected 'Success after retry', got %q", result)
	}
	if provider.callCount != 2 {
		t.Errorf("expected 2 calls, got %d", provider.callCount)
	}
}

// TestRunAgentLoop_WithEnabledLogging tests processing with logging enabled.
func TestRunAgentLoop_WithEnabledLogging(t *testing.T) {
	tempDir := t.TempDir()
	logDir := t.TempDir()
	cfg := &config.Config{
		Agents: config.AgentsConfig{
			Defaults: config.AgentDefaults{
				LLM:       "test/model",
				Workspace: tempDir,
			},
		},
		Logging: &config.LoggingConfig{
			LLM: &config.LLMLogConfig{
				Enabled:     true,
				LogDir:      logDir,
				DetailLevel: "truncated",
			},
		},
	}
	msgBus := bus.NewMessageBus()
	provider := &mockProviderForTest{}

	loop := NewAgentLoop(cfg, msgBus, provider)
	agent := loop.registry.GetDefaultAgent()

	ctx := context.Background()
	result, err := loop.runAgentLoop(ctx, agent, processOptions{
		SessionKey:  "logged-test",
		Channel:     "cli",
		ChatID:      "test",
		UserMessage: "Hello with logging",
	})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result == "" {
		t.Error("expected non-empty result")
	}
}

// mockSummaryProvider supports different responses for chat vs summary calls.
type mockSummaryProvider struct {
	responses []string
	index     int
}

func (m *mockSummaryProvider) Chat(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
	resp := "Summary response"
	if m.responses != nil && m.index < len(m.responses) {
		resp = m.responses[m.index]
		m.index++
	}
	return &providers.LLMResponse{
		Content:      resp,
		FinishReason: "stop",
		ToolCalls:    []protocoltypes.ToolCall{},
		Usage:        &providers.UsageInfo{PromptTokens: 50, CompletionTokens: 20, TotalTokens: 70},
	}, nil
}

func (m *mockSummaryProvider) GetDefaultModel() string {
	return "test-model"
}

// TestSummarizeSession_ShortHistory tests summarizeSession with short history (no-op).
func TestSummarizeSession_ShortHistory(t *testing.T) {
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
	provider := &mockSummaryProvider{}
	loop := NewAgentLoop(cfg, msgBus, provider)
	agent := loop.registry.GetDefaultAgent()

	sessionKey := "summarize-short"
	// Don't add history - should be no-op
	loop.summarizeSession(agent, sessionKey)
	// No panic means success
}

// TestSummarizeSession_WithHistory tests summarizeSession with enough history.
func TestSummarizeSession_WithHistory(t *testing.T) {
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
	provider := &mockSummaryProvider{
		responses: []string{"This is a summary of the conversation."},
	}
	loop := NewAgentLoop(cfg, msgBus, provider)
	agent := loop.registry.GetDefaultAgent()

	sessionKey := "summarize-long"
	// Add enough history messages (more than 4, since summarizeSession keeps last 4)
	for i := 0; i < 8; i++ {
		agent.Sessions.AddMessage(sessionKey, "user", fmt.Sprintf("Message %d: This is a test message with some content", i))
		agent.Sessions.AddMessage(sessionKey, "assistant", fmt.Sprintf("Response %d: This is a test response with some content", i))
	}

	// Verify we have enough history
	history := agent.Sessions.GetHistory(sessionKey)
	if len(history) <= 4 {
		t.Fatalf("expected more than 4 history messages, got %d", len(history))
	}

	loop.summarizeSession(agent, sessionKey)
	// summarizeSession may or may not set a summary depending on provider behavior
	// Just verify no panic
}

// TestSummarizeSession_LargeHistory_MultiPart tests summarizeSession with large history (>10 messages).
func TestSummarizeSession_LargeHistory_MultiPart(t *testing.T) {
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
	provider := &mockSummaryProvider{
		responses: []string{
			"Part 1 summary.",
			"Part 2 summary.",
			"Merged summary of both parts.",
		},
	}
	loop := NewAgentLoop(cfg, msgBus, provider)
	agent := loop.registry.GetDefaultAgent()

	sessionKey := "summarize-large"
	// Add more than 10 messages to trigger multi-part summarization
	for i := 0; i < 16; i++ {
		agent.Sessions.AddMessage(sessionKey, "user", fmt.Sprintf("User message %d with enough content to be meaningful", i))
		agent.Sessions.AddMessage(sessionKey, "assistant", fmt.Sprintf("Assistant response %d with enough content to be meaningful", i))
	}

	loop.summarizeSession(agent, sessionKey)
	// Just verify no panic - summary may or may not be set depending on provider behavior
}

// TestRunAgentLoop_WithObserver tests running with observer manager.
func TestRunAgentLoop_WithObserver(t *testing.T) {
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
	loop := NewAgentLoop(cfg, msgBus, provider)

	// Create and set observer manager
	mgr := observer.NewManager()
	loop.SetObserverManager(mgr)

	agent := loop.registry.GetDefaultAgent()
	ctx := context.Background()

	result, err := loop.runAgentLoop(ctx, agent, processOptions{
		SessionKey:  "observer-test",
		Channel:     "cli",
		ChatID:      "test",
		UserMessage: "Hello with observer",
	})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result == "" {
		t.Error("expected non-empty result")
	}
}

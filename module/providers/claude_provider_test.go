// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package providers_test

import (
	"context"
	"fmt"
	"strings"
	"testing"

	"github.com/276793422/NemesisBot/module/providers"
)

// --- NewClaudeProvider ---

func TestNewClaudeProvider(t *testing.T) {
	p := providers.NewClaudeProvider("test-token")
	if p == nil {
		t.Fatal("expected non-nil provider")
	}
}

func TestNewClaudeProvider_EmptyToken(t *testing.T) {
	p := providers.NewClaudeProvider("")
	if p == nil {
		t.Fatal("expected non-nil provider even with empty token")
	}
}

// --- NewClaudeProviderWithBaseURL ---

func TestNewClaudeProviderWithBaseURL(t *testing.T) {
	p := providers.NewClaudeProviderWithBaseURL("test-token", "https://custom-api.example.com")
	if p == nil {
		t.Fatal("expected non-nil provider")
	}
}

func TestNewClaudeProviderWithBaseURL_EmptyBaseURL(t *testing.T) {
	p := providers.NewClaudeProviderWithBaseURL("test-token", "")
	if p == nil {
		t.Fatal("expected non-nil provider with empty base URL")
	}
}

// --- NewClaudeProviderWithTokenSource ---

func TestNewClaudeProviderWithTokenSource(t *testing.T) {
	tokenSource := func() (string, error) {
		return "refreshed-token", nil
	}
	p := providers.NewClaudeProviderWithTokenSource("initial-token", tokenSource)
	if p == nil {
		t.Fatal("expected non-nil provider")
	}
}

func TestNewClaudeProviderWithTokenSource_NilTokenSource(t *testing.T) {
	p := providers.NewClaudeProviderWithTokenSource("initial-token", nil)
	if p == nil {
		t.Fatal("expected non-nil provider even with nil token source")
	}
}

// --- NewClaudeProviderWithTokenSourceAndBaseURL ---

func TestNewClaudeProviderWithTokenSourceAndBaseURL(t *testing.T) {
	tokenSource := func() (string, error) {
		return "refreshed-token", nil
	}
	p := providers.NewClaudeProviderWithTokenSourceAndBaseURL("initial-token", tokenSource, "https://api.example.com")
	if p == nil {
		t.Fatal("expected non-nil provider")
	}
}

func TestNewClaudeProviderWithTokenSourceAndBaseURL_EmptyAll(t *testing.T) {
	p := providers.NewClaudeProviderWithTokenSourceAndBaseURL("", nil, "")
	if p == nil {
		t.Fatal("expected non-nil provider")
	}
}

// --- GetDefaultModel ---

func TestClaudeProvider_GetDefaultModel(t *testing.T) {
	p := providers.NewClaudeProvider("test-token")
	model := p.GetDefaultModel()
	if model == "" {
		t.Error("GetDefaultModel should return non-empty string")
	}
	// The default model should be from the anthropic provider
	// which returns "claude-sonnet-4-5-20250929"
	expectedModel := "claude-sonnet-4-5-20250929"
	if model != expectedModel {
		t.Errorf("expected default model '%s', got '%s'", expectedModel, model)
	}
}

// --- ClaudeProvider implements LLMProvider ---

func TestClaudeProvider_ImplementsLLMProvider(t *testing.T) {
	// Compile-time check
	var _ providers.LLMProvider = providers.NewClaudeProvider("token")
}

// --- Chat with cancelled context ---

func TestClaudeProvider_Chat_ContextCanceled(t *testing.T) {
	p := providers.NewClaudeProvider("test-token")
	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	messages := []providers.Message{
		{Role: "user", Content: "Hello"},
	}

	// This will fail because the context is cancelled, but it should not panic
	_, err := p.Chat(ctx, messages, nil, "claude-sonnet-4-5-20250929", nil)
	if err == nil {
		t.Error("expected error with cancelled context")
	}
}

func TestClaudeProvider_Chat_EmptyMessages(t *testing.T) {
	p := providers.NewClaudeProvider("test-token")
	ctx := context.Background()

	// This will fail because there's no real API, but should not panic
	_, err := p.Chat(ctx, nil, nil, "", nil)
	// We expect an error since there's no real API endpoint
	if err == nil {
		t.Log("Chat completed without error (unexpected but not necessarily wrong)")
	}
}

func TestClaudeProvider_Chat_WithTokenSource(t *testing.T) {
	tokenSource := func() (string, error) {
		return "refreshed-token", nil
	}
	p := providers.NewClaudeProviderWithTokenSource("initial-token", tokenSource)

	ctx := context.Background()

	// The token source should be called during Chat
	// This will fail because there's no real API, but should not panic
	_, err := p.Chat(ctx, []providers.Message{{Role: "user", Content: "test"}}, nil, "", nil)
	if err == nil {
		t.Log("Chat completed without error")
	}
}

func TestClaudeProvider_Chat_TokenSourceError(t *testing.T) {
	tokenSource := func() (string, error) {
		return "", fmt.Errorf("token refresh failed")
	}
	p := providers.NewClaudeProviderWithTokenSource("initial-token", tokenSource)

	ctx := context.Background()

	_, err := p.Chat(ctx, []providers.Message{{Role: "user", Content: "test"}}, nil, "", nil)
	if err == nil {
		t.Error("expected error when token source fails")
	}
	if err != nil && !strings.Contains(err.Error(), "refreshing token") {
		t.Errorf("expected error about refreshing token, got: %v", err)
	}
}

func TestClaudeProvider_Chat_WithTools(t *testing.T) {
	p := providers.NewClaudeProvider("test-token")
	ctx := context.Background()

	tools := []providers.ToolDefinition{
		{
			Type: "function",
			Function: providers.ToolFunctionDefinition{
				Name:        "test_tool",
				Description: "A test tool",
			},
		},
	}

	_, err := p.Chat(ctx, []providers.Message{{Role: "user", Content: "use tool"}}, tools, "", nil)
	if err == nil {
		t.Log("Chat completed without error")
	}
}

func TestClaudeProvider_Chat_WithSystemMessage(t *testing.T) {
	p := providers.NewClaudeProvider("test-token")
	ctx := context.Background()

	messages := []providers.Message{
		{Role: "system", Content: "You are helpful"},
		{Role: "user", Content: "Hello"},
	}

	_, err := p.Chat(ctx, messages, nil, "", nil)
	if err == nil {
		t.Log("Chat completed without error")
	}
}

func TestClaudeProvider_Chat_AllMessageRoles(t *testing.T) {
	p := providers.NewClaudeProvider("test-token")
	ctx := context.Background()

	messages := []providers.Message{
		{Role: "system", Content: "System prompt"},
		{Role: "user", Content: "User message"},
		{Role: "assistant", Content: "Assistant reply", ToolCalls: []providers.ToolCall{
			{ID: "call-1", Name: "tool1", Arguments: map[string]interface{}{"key": "value"}},
		}},
		{Role: "tool", Content: "tool result", ToolCallID: "call-1"},
		{Role: "user", Content: "Thanks"},
	}

	_, err := p.Chat(ctx, messages, nil, "", nil)
	if err == nil {
		t.Log("Chat completed without error")
	}
}

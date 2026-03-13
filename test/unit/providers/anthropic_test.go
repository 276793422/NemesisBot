// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package providers_test

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	anthropicprovider "github.com/276793422/NemesisBot/module/providers/anthropic"
	"github.com/276793422/NemesisBot/module/providers/protocoltypes"
)

// TestAnthropicProviderNewProvider tests the creation of a new Anthropic provider
func TestAnthropicProviderNewProvider(t *testing.T) {
	t.Run("NewProvider with token", func(t *testing.T) {
		p := anthropicprovider.NewProvider("test-token")
		if p == nil {
			t.Fatal("expected non-nil provider")
		}
		if p.BaseURL() != "https://api.anthropic.com" {
			t.Errorf("expected default base URL, got %s", p.BaseURL())
		}
	})

	t.Run("NewProviderWithBaseURL", func(t *testing.T) {
		customBase := "https://custom.anthropic.com"
		p := anthropicprovider.NewProviderWithBaseURL("test-token", customBase)
		if p == nil {
			t.Fatal("expected non-nil provider")
		}
		if p.BaseURL() != customBase {
			t.Errorf("expected custom base URL %s, got %s", customBase, p.BaseURL())
		}
	})

	t.Run("NewProviderWithBaseURL with /v1 suffix", func(t *testing.T) {
		p := anthropicprovider.NewProviderWithBaseURL("test-token", "https://api.example.com/v1")
		if p.BaseURL() != "https://api.example.com" {
			t.Errorf("expected base URL without /v1 suffix, got %s", p.BaseURL())
		}
	})

	t.Run("NewProviderWithBaseURL empty", func(t *testing.T) {
		p := anthropicprovider.NewProviderWithBaseURL("test-token", "")
		if p.BaseURL() != "https://api.anthropic.com" {
			t.Errorf("expected default base URL, got %s", p.BaseURL())
		}
	})

	t.Run("NewProviderWithTokenSource", func(t *testing.T) {
		tokenSource := func() (string, error) {
			return "dynamic-token", nil
		}
		p := anthropicprovider.NewProviderWithTokenSource("test-token", tokenSource)
		if p == nil {
			t.Fatal("expected non-nil provider")
		}
	})
}

// TestAnthropicProviderGetDefaultModel tests the GetDefaultModel method
func TestAnthropicProviderGetDefaultModel(t *testing.T) {
	p := anthropicprovider.NewProvider("test-token")
	model := p.GetDefaultModel()
	if model == "" {
		t.Error("expected non-empty default model")
	}
}

// TestAnthropicProviderChatWithMockServer tests the Chat method with a mock server
func TestAnthropicProviderChatWithMockServer(t *testing.T) {
	// Create a mock server that returns a valid Anthropic response
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Verify request
		if r.Method != "POST" {
			t.Errorf("expected POST request, got %s", r.Method)
		}
		if r.URL.Path != "/v1/messages" {
			t.Errorf("expected /v1/messages path, got %s", r.URL.Path)
		}

		// Return a mock response (Anthropic SDK uses x-api-key or Authorization)
		response := map[string]interface{}{
			"id":            "msg-123",
			"type":          "message",
			"role":          "assistant",
			"content":       []interface{}{map[string]string{"type": "text", "text": "Test response"}},
			"stop_reason":   "end_turn",
			"model":         "claude-3-5-sonnet-20241022",
			"stop_sequence": nil,
			"usage": map[string]interface{}{
				"input_tokens":  10,
				"output_tokens": 20,
			},
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(response)
	}))
	defer server.Close()

	// Create provider with mock server URL
	p := anthropicprovider.NewProviderWithBaseURL("test-token", server.URL)

	// Test Chat method
	ctx := context.Background()
	messages := []protocoltypes.Message{
		{Role: "user", Content: "Hello"},
	}

	response, err := p.Chat(ctx, messages, nil, "claude-3-5-sonnet-20241022", map[string]interface{}{
		"max_tokens": 4096,
	})

	if err != nil {
		t.Fatalf("Chat failed: %v", err)
	}

	if response.Content != "Test response" {
		t.Errorf("expected 'Test response', got '%s'", response.Content)
	}

	if response.FinishReason != "stop" {
		t.Errorf("expected finish_reason 'stop', got '%s'", response.FinishReason)
	}

	if response.Usage == nil {
		t.Error("expected usage info")
	} else {
		if response.Usage.PromptTokens != 10 {
			t.Errorf("expected 10 prompt tokens, got %d", response.Usage.PromptTokens)
		}
		if response.Usage.CompletionTokens != 20 {
			t.Errorf("expected 20 completion tokens, got %d", response.Usage.CompletionTokens)
		}
	}
}

// TestAnthropicProviderChatWithToolCalls tests the Chat method with tool calls
func TestAnthropicProviderChatWithToolCalls(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		response := map[string]interface{}{
			"id":   "msg-123",
			"type": "message",
			"role": "assistant",
			"content": []interface{}{
				map[string]string{"type": "text", "text": "Let me call the tool"},
				map[string]interface{}{
					"type":  "tool_use",
					"id":    "toolu-123",
					"name":  "test_tool",
					"input": map[string]string{"param": "value"},
				},
			},
			"stop_reason": "tool_use",
			"model":       "claude-3-5-sonnet-20241022",
			"usage": map[string]interface{}{
				"input_tokens":  15,
				"output_tokens": 25,
			},
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(response)
	}))
	defer server.Close()

	p := anthropicprovider.NewProviderWithBaseURL("test-token", server.URL)

	ctx := context.Background()
	messages := []protocoltypes.Message{
		{Role: "user", Content: "Use the tool"},
	}
	tools := []protocoltypes.ToolDefinition{
		{
			Type: "function",
			Function: protocoltypes.ToolFunctionDefinition{
				Name:        "test_tool",
				Description: "A test tool",
				Parameters: map[string]interface{}{
					"type":       "object",
					"properties": map[string]interface{}{"param": map[string]string{"type": "string"}},
					"required":   []string{"param"},
				},
			},
		},
	}

	response, err := p.Chat(ctx, messages, tools, "claude-3-5-sonnet-20241022", map[string]interface{}{
		"max_tokens": 4096,
	})

	if err != nil {
		t.Fatalf("Chat failed: %v", err)
	}

	if response.FinishReason != "tool_calls" {
		t.Errorf("expected finish_reason 'tool_calls', got '%s'", response.FinishReason)
	}

	if len(response.ToolCalls) != 1 {
		t.Fatalf("expected 1 tool call, got %d", len(response.ToolCalls))
	}

	if response.ToolCalls[0].Name != "test_tool" {
		t.Errorf("expected tool name 'test_tool', got '%s'", response.ToolCalls[0].Name)
	}
}

// TestAnthropicProviderChatWithTokenRefresh tests token refresh functionality
func TestAnthropicProviderChatWithTokenRefresh(t *testing.T) {
	callCount := 0
	tokenSource := func() (string, error) {
		callCount++
		return "dynamic-token", nil
	}

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		response := map[string]interface{}{
			"id":          "msg-123",
			"type":        "message",
			"role":        "assistant",
			"content":     []interface{}{map[string]string{"type": "text", "text": "Response"}},
			"stop_reason": "end_turn",
			"model":       "claude-3-5-sonnet-20241022",
			"usage":       map[string]interface{}{"input_tokens": 10, "output_tokens": 20},
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(response)
	}))
	defer server.Close()

	p := anthropicprovider.NewProviderWithTokenSourceAndBaseURL("initial-token", tokenSource, server.URL)

	ctx := context.Background()
	messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

	_, err := p.Chat(ctx, messages, nil, "claude-3-5-sonnet-20241022", map[string]interface{}{
		"max_tokens": 4096,
	})

	if err != nil {
		t.Fatalf("Chat failed: %v", err)
	}

	if callCount != 1 {
		t.Errorf("expected token source to be called once, got %d", callCount)
	}
}

// TestAnthropicProviderChatErrorHandling tests error handling
func TestAnthropicProviderChatErrorHandling(t *testing.T) {
	t.Run("HTTP error", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			w.WriteHeader(http.StatusUnauthorized)
			w.Write([]byte(`{"error":{"message":"Invalid token"}}`))
		}))
		defer server.Close()

		p := anthropicprovider.NewProviderWithBaseURL("test-token", server.URL)

		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		_, err := p.Chat(ctx, messages, nil, "claude-3-5-sonnet-20241022", map[string]interface{}{
			"max_tokens": 4096,
		})

		if err == nil {
			t.Error("expected error for unauthorized request")
		}
	})

	t.Run("Context cancellation", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			time.Sleep(100 * time.Millisecond)
			w.WriteHeader(http.StatusOK)
		}))
		defer server.Close()

		p := anthropicprovider.NewProviderWithBaseURL("test-token", server.URL)

		ctx, cancel := context.WithCancel(context.Background())
		cancel() // Cancel immediately

		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		_, err := p.Chat(ctx, messages, nil, "claude-3-5-sonnet-20241022", map[string]interface{}{
			"max_tokens": 4096,
		})

		if err == nil {
			t.Error("expected error for cancelled context")
		}
	})
}

// TestAnthropicProviderChatWithTemperature tests temperature parameter
func TestAnthropicProviderChatWithTemperature(t *testing.T) {
	var receivedTemp float64
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var requestBody map[string]interface{}
		json.NewDecoder(r.Body).Decode(&requestBody)
		receivedTemp = requestBody["temperature"].(float64)

		response := map[string]interface{}{
			"id":          "msg-123",
			"type":        "message",
			"role":        "assistant",
			"content":     []interface{}{map[string]string{"type": "text", "text": "Response"}},
			"stop_reason": "end_turn",
			"model":       "claude-3-5-sonnet-20241022",
			"usage":       map[string]interface{}{"input_tokens": 10, "output_tokens": 20},
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(response)
	}))
	defer server.Close()

	p := anthropicprovider.NewProviderWithBaseURL("test-token", server.URL)

	ctx := context.Background()
	messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

	_, err := p.Chat(ctx, messages, nil, "claude-3-5-sonnet-20241022", map[string]interface{}{
		"max_tokens":  4096,
		"temperature": 0.7,
	})

	if err != nil {
		t.Fatalf("Chat failed: %v", err)
	}

	if receivedTemp != 0.7 {
		t.Errorf("expected temperature 0.7, got %f", receivedTemp)
	}
}

// TestAnthropicProviderChatWithSystemMessage tests system message handling
func TestAnthropicProviderChatWithSystemMessage(t *testing.T) {
	var receivedSystem bool
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var requestBody map[string]interface{}
		json.NewDecoder(r.Body).Decode(&requestBody)
		_, receivedSystem = requestBody["system"]

		response := map[string]interface{}{
			"id":          "msg-123",
			"type":        "message",
			"role":        "assistant",
			"content":     []interface{}{map[string]string{"type": "text", "text": "Response"}},
			"stop_reason": "end_turn",
			"model":       "claude-3-5-sonnet-20241022",
			"usage":       map[string]interface{}{"input_tokens": 10, "output_tokens": 20},
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(response)
	}))
	defer server.Close()

	p := anthropicprovider.NewProviderWithBaseURL("test-token", server.URL)

	ctx := context.Background()
	messages := []protocoltypes.Message{
		{Role: "system", Content: "You are a helpful assistant"},
		{Role: "user", Content: "Hello"},
	}

	_, err := p.Chat(ctx, messages, nil, "claude-3-5-sonnet-20241022", map[string]interface{}{
		"max_tokens": 4096,
	})

	if err != nil {
		t.Fatalf("Chat failed: %v", err)
	}

	if !receivedSystem {
		t.Error("expected system message to be included in request")
	}
}

// TestAnthropicProviderChatWithToolResult tests tool result message handling
func TestAnthropicProviderChatWithToolResult(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var requestBody map[string]interface{}
		json.NewDecoder(r.Body).Decode(&requestBody)

		messages := requestBody["messages"].([]interface{})
		if len(messages) != 2 {
			t.Errorf("expected 2 messages, got %d", len(messages))
		}

		response := map[string]interface{}{
			"id":          "msg-123",
			"type":        "message",
			"role":        "assistant",
			"content":     []interface{}{map[string]string{"type": "text", "text": "Final response"}},
			"stop_reason": "end_turn",
			"model":       "claude-3-5-sonnet-20241022",
			"usage":       map[string]interface{}{"input_tokens": 10, "output_tokens": 20},
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(response)
	}))
	defer server.Close()

	p := anthropicprovider.NewProviderWithBaseURL("test-token", server.URL)

	ctx := context.Background()
	messages := []protocoltypes.Message{
		{Role: "user", Content: "Use the tool"},
		{
			Role:       "user",
			ToolCallID: "toolu-123",
			Content:    "Tool result",
		},
	}

	_, err := p.Chat(ctx, messages, nil, "claude-3-5-sonnet-20241022", map[string]interface{}{
		"max_tokens": 4096,
	})

	if err != nil {
		t.Fatalf("Chat failed: %v", err)
	}
}

// TestAnthropicProviderChatMaxTokens tests max_tokens parameter
func TestAnthropicProviderChatMaxTokens(t *testing.T) {
	var receivedMaxTokens float64
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var requestBody map[string]interface{}
		json.NewDecoder(r.Body).Decode(&requestBody)
		receivedMaxTokens = requestBody["max_tokens"].(float64)

		response := map[string]interface{}{
			"id":          "msg-123",
			"type":        "message",
			"role":        "assistant",
			"content":     []interface{}{map[string]string{"type": "text", "text": "Response"}},
			"stop_reason": "end_turn",
			"model":       "claude-3-5-sonnet-20241022",
			"usage":       map[string]interface{}{"input_tokens": 10, "output_tokens": 20},
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(response)
	}))
	defer server.Close()

	p := anthropicprovider.NewProviderWithBaseURL("test-token", server.URL)

	ctx := context.Background()
	messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

	_, err := p.Chat(ctx, messages, nil, "claude-3-5-sonnet-20241022", map[string]interface{}{
		"max_tokens": 8192,
	})

	if err != nil {
		t.Fatalf("Chat failed: %v", err)
	}

	if receivedMaxTokens != 8192 {
		t.Errorf("expected max_tokens 8192, got %f", receivedMaxTokens)
	}
}

// TestAnthropicProviderChatDefaultMaxTokens tests default max_tokens
func TestAnthropicProviderChatDefaultMaxTokens(t *testing.T) {
	var receivedMaxTokens float64
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var requestBody map[string]interface{}
		json.NewDecoder(r.Body).Decode(&requestBody)
		receivedMaxTokens = requestBody["max_tokens"].(float64)

		response := map[string]interface{}{
			"id":          "msg-123",
			"type":        "message",
			"role":        "assistant",
			"content":     []interface{}{map[string]string{"type": "text", "text": "Response"}},
			"stop_reason": "end_turn",
			"model":       "claude-3-5-sonnet-20241022",
			"usage":       map[string]interface{}{"input_tokens": 10, "output_tokens": 20},
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(response)
	}))
	defer server.Close()

	p := anthropicprovider.NewProviderWithBaseURL("test-token", server.URL)

	ctx := context.Background()
	messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

	// Don't pass max_tokens - should default to 4096
	_, err := p.Chat(ctx, messages, nil, "claude-3-5-sonnet-20241022", nil)

	if err != nil {
		t.Fatalf("Chat failed: %v", err)
	}

	if receivedMaxTokens != 4096 {
		t.Errorf("expected default max_tokens 4096, got %f", receivedMaxTokens)
	}
}

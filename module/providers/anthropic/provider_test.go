// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package anthropicprovider

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	anthropic "github.com/anthropics/anthropic-sdk-go"
	"github.com/276793422/NemesisBot/module/providers/protocoltypes"
)

// TestNewProvider tests provider creation
func TestNewProvider(t *testing.T) {
	t.Run("basic provider", func(t *testing.T) {
		p := NewProvider("test-token")
		if p == nil {
			t.Fatal("expected non-nil provider")
		}
		if p.BaseURL() != defaultBaseURL {
			t.Errorf("expected %s, got %s", defaultBaseURL, p.BaseURL())
		}
		if p.client == nil {
			t.Error("expected non-nil client")
		}
	})

	t.Run("empty token", func(t *testing.T) {
		p := NewProvider("")
		if p == nil {
			t.Fatal("expected non-nil provider even with empty token")
		}
	})
}

// TestNewProviderWithClient tests creating provider with existing client
func TestNewProviderWithClient(t *testing.T) {
	// Create a mock anthropic client
	mockClient := &anthropic.Client{}

	p := NewProviderWithClient(mockClient)
	if p == nil {
		t.Fatal("expected non-nil provider")
	}
	if p.client != mockClient {
		t.Error("expected client to be set to provided client")
	}
	if p.BaseURL() != defaultBaseURL {
		t.Errorf("expected %s, got %s", defaultBaseURL, p.BaseURL())
	}
}

// TestNewProviderWithBaseURL tests custom base URL
func TestNewProviderWithBaseURL(t *testing.T) {
	testCases := []struct {
		name         string
		apiBase      string
		expectedBase string
	}{
		{
			name:         "custom base URL",
			apiBase:      "https://custom.api.com",
			expectedBase: "https://custom.api.com",
		},
		{
			name:         "base URL with trailing slash",
			apiBase:      "https://custom.api.com/",
			expectedBase: "https://custom.api.com",
		},
		{
			name:         "base URL with /v1 suffix",
			apiBase:      "https://custom.api.com/v1",
			expectedBase: "https://custom.api.com",
		},
		{
			name:         "base URL with /v1/ suffix",
			apiBase:      "https://custom.api.com/v1/",
			expectedBase: "https://custom.api.com",
		},
		{
			name:         "empty base URL uses default",
			apiBase:      "",
			expectedBase: defaultBaseURL,
		},
		{
			name:         "whitespace only base URL",
			apiBase:      "   ",
			expectedBase: defaultBaseURL,
		},
		{
			name:         "base URL with multiple trailing slashes",
			apiBase:      "https://custom.api.com///",
			expectedBase: "https://custom.api.com",
		},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			p := NewProviderWithBaseURL("test-token", tc.apiBase)
			if p == nil {
				t.Fatal("expected non-nil provider")
			}
			if p.BaseURL() != tc.expectedBase {
				t.Errorf("expected %s, got %s", tc.expectedBase, p.BaseURL())
			}
		})
	}
}

// TestNewProviderWithTokenSource tests token source functionality
func TestNewProviderWithTokenSource(t *testing.T) {
	t.Run("with token source", func(t *testing.T) {
		tokenSource := func() (string, error) {
			return "dynamic-token", nil
		}
		p := NewProviderWithTokenSource("initial-token", tokenSource)
		if p == nil {
			t.Fatal("expected non-nil provider")
		}
		if p.tokenSource == nil {
			t.Error("expected token source to be set")
		}
	})

	t.Run("with token source and base URL", func(t *testing.T) {
		tokenSource := func() (string, error) {
			return "dynamic-token", nil
		}
		customBase := "https://custom.api.com"
		p := NewProviderWithTokenSourceAndBaseURL("initial-token", tokenSource, customBase)
		if p == nil {
			t.Fatal("expected non-nil provider")
		}
		if p.tokenSource == nil {
			t.Error("expected token source to be set")
		}
		if p.BaseURL() != customBase {
			t.Errorf("expected %s, got %s", customBase, p.BaseURL())
		}
	})
}

// TestGetDefaultModel tests default model retrieval
func TestGetDefaultModel(t *testing.T) {
	p := NewProvider("test-token")
	model := p.GetDefaultModel()
	if model == "" {
		t.Error("expected non-empty default model")
	}
	expectedModel := "claude-sonnet-4-5-20250929"
	if model != expectedModel {
		t.Errorf("expected %s, got %s", expectedModel, model)
	}
}

// TestChat tests the Chat method with various scenarios
func TestChat(t *testing.T) {
	t.Run("successful chat with text response", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			if r.Method != "POST" {
				t.Errorf("expected POST, got %s", r.Method)
			}
			if r.URL.Path != "/v1/messages" {
				t.Errorf("expected /v1/messages, got %s", r.URL.Path)
			}

			response := map[string]interface{}{
				"id":           "msg-123",
				"type":         "message",
				"role":         "assistant",
				"content":      []interface{}{map[string]string{"type": "text", "text": "Hello!"}},
				"stop_reason":  "end_turn",
				"model":        "claude-3-5-sonnet-20241022",
				"stop_sequence": nil,
				"usage": map[string]interface{}{
					"input_tokens":  10,
					"output_tokens": 5,
				},
			}
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(response)
		}))
		defer server.Close()

		p := NewProviderWithBaseURL("test-token", server.URL)
		ctx := context.Background()
		messages := []protocoltypes.Message{
			{Role: "user", Content: "Hello"},
		}

		resp, err := p.Chat(ctx, messages, nil, "claude-3-5-sonnet-20241022", nil)
		if err != nil {
			t.Fatalf("Chat failed: %v", err)
		}

		if resp.Content != "Hello!" {
			t.Errorf("expected 'Hello!', got '%s'", resp.Content)
		}
		if resp.FinishReason != "stop" {
			t.Errorf("expected 'stop', got '%s'", resp.FinishReason)
		}
		if resp.Usage == nil {
			t.Error("expected usage info")
		} else {
			if resp.Usage.PromptTokens != 10 {
				t.Errorf("expected 10 prompt tokens, got %d", resp.Usage.PromptTokens)
			}
			if resp.Usage.CompletionTokens != 5 {
				t.Errorf("expected 5 completion tokens, got %d", resp.Usage.CompletionTokens)
			}
		}
	})

	t.Run("chat with tool calls", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			response := map[string]interface{}{
				"id":   "msg-123",
				"type": "message",
				"role": "assistant",
				"content": []interface{}{
					map[string]string{"type": "text", "text": "I'll help you"},
					map[string]interface{}{
						"type":  "tool_use",
						"id":    "toolu-123",
						"name":  "search",
						"input": map[string]string{"query": "test"},
					},
				},
				"stop_reason": "tool_use",
				"model":       "claude-3-5-sonnet-20241022",
				"usage":       map[string]interface{}{"input_tokens": 15, "output_tokens": 25},
			}
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(response)
		}))
		defer server.Close()

		p := NewProviderWithBaseURL("test-token", server.URL)
		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Search for test"}}
		tools := []protocoltypes.ToolDefinition{
			{
				Type: "function",
				Function: protocoltypes.ToolFunctionDefinition{
					Name:        "search",
					Description: "Search",
					Parameters: map[string]interface{}{
						"type":       "object",
						"properties": map[string]interface{}{"query": map[string]string{"type": "string"}},
						"required":   []string{"query"},
					},
				},
			},
		}

		resp, err := p.Chat(ctx, messages, tools, "claude-3-5-sonnet-20241022", nil)
		if err != nil {
			t.Fatalf("Chat failed: %v", err)
		}

		if resp.FinishReason != "tool_calls" {
			t.Errorf("expected 'tool_calls', got '%s'", resp.FinishReason)
		}
		if len(resp.ToolCalls) != 1 {
			t.Fatalf("expected 1 tool call, got %d", len(resp.ToolCalls))
		}
		if resp.ToolCalls[0].Name != "search" {
			t.Errorf("expected 'search', got '%s'", resp.ToolCalls[0].Name)
		}
	})

	t.Run("chat with max_tokens", func(t *testing.T) {
		var receivedMaxTokens int64
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			var body map[string]interface{}
			json.NewDecoder(r.Body).Decode(&body)
			receivedMaxTokens = int64(body["max_tokens"].(float64))

			response := map[string]interface{}{
				"id":           "msg-123",
				"type":         "message",
				"role":         "assistant",
				"content":      []interface{}{map[string]string{"type": "text", "text": "Response"}},
				"stop_reason":  "end_turn",
				"model":        "claude-3-5-sonnet-20241022",
				"stop_sequence": nil,
				"usage":        map[string]interface{}{"input_tokens": 10, "output_tokens": 5},
			}
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(response)
		}))
		defer server.Close()

		p := NewProviderWithBaseURL("test-token", server.URL)
		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		_, err := p.Chat(ctx, messages, nil, "claude-3-5-sonnet-20241022", map[string]interface{}{
			"max_tokens": 8192,
		})
		if err != nil {
			t.Fatalf("Chat failed: %v", err)
		}

		if receivedMaxTokens != 8192 {
			t.Errorf("expected 8192, got %d", receivedMaxTokens)
		}
	})

	t.Run("chat with temperature", func(t *testing.T) {
		var receivedTemp float64
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			var body map[string]interface{}
			json.NewDecoder(r.Body).Decode(&body)
			receivedTemp = body["temperature"].(float64)

			response := map[string]interface{}{
				"id":           "msg-123",
				"type":         "message",
				"role":         "assistant",
				"content":      []interface{}{map[string]string{"type": "text", "text": "Response"}},
				"stop_reason":  "end_turn",
				"model":        "claude-3-5-sonnet-20241022",
				"stop_sequence": nil,
				"usage":        map[string]interface{}{"input_tokens": 10, "output_tokens": 5},
			}
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(response)
		}))
		defer server.Close()

		p := NewProviderWithBaseURL("test-token", server.URL)
		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		_, err := p.Chat(ctx, messages, nil, "claude-3-5-sonnet-20241022", map[string]interface{}{
			"temperature": 0.7,
		})
		if err != nil {
			t.Fatalf("Chat failed: %v", err)
		}

		if receivedTemp != 0.7 {
			t.Errorf("expected 0.7, got %f", receivedTemp)
		}
	})

	t.Run("chat with system message", func(t *testing.T) {
		var hasSystem bool
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			var body map[string]interface{}
			json.NewDecoder(r.Body).Decode(&body)
			_, hasSystem = body["system"]

			response := map[string]interface{}{
				"id":           "msg-123",
				"type":         "message",
				"role":         "assistant",
				"content":      []interface{}{map[string]string{"type": "text", "text": "Response"}},
				"stop_reason":  "end_turn",
				"model":        "claude-3-5-sonnet-20241022",
				"stop_sequence": nil,
				"usage":        map[string]interface{}{"input_tokens": 10, "output_tokens": 5},
			}
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(response)
		}))
		defer server.Close()

		p := NewProviderWithBaseURL("test-token", server.URL)
		ctx := context.Background()
		messages := []protocoltypes.Message{
			{Role: "system", Content: "You are helpful"},
			{Role: "user", Content: "Hello"},
		}

		_, err := p.Chat(ctx, messages, nil, "claude-3-5-sonnet-20241022", nil)
		if err != nil {
			t.Fatalf("Chat failed: %v", err)
		}

		if !hasSystem {
			t.Error("expected system message in request")
		}
	})

	t.Run("chat with tool result", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			var body map[string]interface{}
			json.NewDecoder(r.Body).Decode(&body)

			messages := body["messages"].([]interface{})
			if len(messages) < 2 {
				t.Errorf("expected at least 2 messages, got %d", len(messages))
			}

			response := map[string]interface{}{
				"id":           "msg-123",
				"type":         "message",
				"role":         "assistant",
				"content":      []interface{}{map[string]string{"type": "text", "text": "Final response"}},
				"stop_reason":  "end_turn",
				"model":        "claude-3-5-sonnet-20241022",
				"stop_sequence": nil,
				"usage":        map[string]interface{}{"input_tokens": 10, "output_tokens": 5},
			}
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(response)
		}))
		defer server.Close()

		p := NewProviderWithBaseURL("test-token", server.URL)
		ctx := context.Background()
		messages := []protocoltypes.Message{
			{Role: "user", Content: "Use tool"},
			{Role: "user", ToolCallID: "toolu-123", Content: "Tool result"},
		}

		_, err := p.Chat(ctx, messages, nil, "claude-3-5-sonnet-20241022", nil)
		if err != nil {
			t.Fatalf("Chat failed: %v", err)
		}
	})

	t.Run("chat with tool call in assistant message", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			var body map[string]interface{}
			json.NewDecoder(r.Body).Decode(&body)

			messages := body["messages"].([]interface{})
			if len(messages) < 1 {
				t.Error("expected at least 1 message")
			}

			response := map[string]interface{}{
				"id":           "msg-123",
				"type":         "message",
				"role":         "assistant",
				"content":      []interface{}{map[string]string{"type": "text", "text": "Response"}},
				"stop_reason":  "end_turn",
				"model":        "claude-3-5-sonnet-20241022",
				"stop_sequence": nil,
				"usage":        map[string]interface{}{"input_tokens": 10, "output_tokens": 5},
			}
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(response)
		}))
		defer server.Close()

		p := NewProviderWithBaseURL("test-token", server.URL)
		ctx := context.Background()
		messages := []protocoltypes.Message{
			{
				Role: "assistant",
				Content: "I'll use a tool",
				ToolCalls: []protocoltypes.ToolCall{
					{ID: "toolu-123", Name: "search", Arguments: map[string]interface{}{"query": "test"}},
				},
			},
			{Role: "user", ToolCallID: "toolu-123", Content: "Result"},
		}

		_, err := p.Chat(ctx, messages, nil, "claude-3-5-sonnet-20241022", nil)
		if err != nil {
			t.Fatalf("Chat failed: %v", err)
		}
	})
}

// TestChatErrorHandling tests error scenarios
func TestChatErrorHandling(t *testing.T) {
	t.Run("HTTP error", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			w.WriteHeader(http.StatusUnauthorized)
			w.Write([]byte(`{"error":{"message":"Invalid token"}}`))
		}))
		defer server.Close()

		p := NewProviderWithBaseURL("test-token", server.URL)
		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		_, err := p.Chat(ctx, messages, nil, "claude-3-5-sonnet-20241022", nil)
		if err == nil {
			t.Error("expected error for unauthorized request")
		}
	})

	t.Run("context cancellation", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			time.Sleep(100 * time.Millisecond)
			w.WriteHeader(http.StatusOK)
		}))
		defer server.Close()

		p := NewProviderWithBaseURL("test-token", server.URL)
		ctx, cancel := context.WithCancel(context.Background())
		cancel()

		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		_, err := p.Chat(ctx, messages, nil, "claude-3-5-sonnet-20241022", nil)
		if err == nil {
			t.Error("expected error for cancelled context")
		}
	})

	t.Run("token source error", func(t *testing.T) {
		tokenSource := func() (string, error) {
			return "", fmt.Errorf("token refresh failed")
		}

		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			response := map[string]interface{}{
				"id":           "msg-123",
				"type":         "message",
				"role":         "assistant",
				"content":      []interface{}{map[string]string{"type": "text", "text": "Response"}},
				"stop_reason":  "end_turn",
				"model":        "claude-3-5-sonnet-20241022",
				"stop_sequence": nil,
				"usage":        map[string]interface{}{"input_tokens": 10, "output_tokens": 5},
			}
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(response)
		}))
		defer server.Close()

		p := NewProviderWithTokenSourceAndBaseURL("initial-token", tokenSource, server.URL)
		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		_, err := p.Chat(ctx, messages, nil, "claude-3-5-sonnet-20241022", nil)
		if err == nil {
			t.Error("expected error from token source")
		}
	})

	t.Run("malformed tool call input", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			response := map[string]interface{}{
				"id":   "msg-123",
				"type": "message",
				"role": "assistant",
				"content": []interface{}{
					map[string]interface{}{
						"type":  "tool_use",
						"id":    "toolu-123",
						"name":  "test_tool",
						"input": []byte("{invalid json"),
					},
				},
				"stop_reason": "tool_use",
				"model":       "claude-3-5-sonnet-20241022",
				"usage":       map[string]interface{}{"input_tokens": 10, "output_tokens": 5},
			}
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(response)
		}))
		defer server.Close()

		p := NewProviderWithBaseURL("test-token", server.URL)
		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Use tool"}}

		resp, err := p.Chat(ctx, messages, nil, "claude-3-5-sonnet-20241022", nil)
		if err != nil {
			t.Fatalf("Chat failed: %v", err)
		}

		// Should have raw input when JSON parsing fails
		if len(resp.ToolCalls) != 1 {
			t.Fatalf("expected 1 tool call, got %d", len(resp.ToolCalls))
		}
		if _, hasRaw := resp.ToolCalls[0].Arguments["raw"]; !hasRaw {
			t.Error("expected raw arguments when JSON parsing fails")
		}
	})

	t.Run("network error", func(t *testing.T) {
		p := NewProviderWithBaseURL("test-token", "https://nonexistent.example.com")
		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		_, err := p.Chat(ctx, messages, nil, "claude-3-5-sonnet-20241022", nil)
		if err == nil {
			t.Error("expected error for network connection failure")
		}
	})

	t.Run("timeout error", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			time.Sleep(2 * time.Second)
			w.WriteHeader(http.StatusOK)
		}))
		defer server.Close()

		p := NewProviderWithBaseURL("test-token", server.URL)
		ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
		defer cancel()

		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		_, err := p.Chat(ctx, messages, nil, "claude-3-5-sonnet-20241022", nil)
		if err == nil {
			t.Error("expected error for timeout")
		}
	})

	t.Run("invalid JSON response", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			w.Header().Set("Content-Type", "application/json")
			w.Write([]byte(`{invalid json`))
		}))
		defer server.Close()

		p := NewProviderWithBaseURL("test-token", server.URL)
		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		_, err := p.Chat(ctx, messages, nil, "claude-3-5-sonnet-20241022", nil)
		if err == nil {
			t.Error("expected error for invalid JSON response")
		}
	})
}

// TestResponseParsing tests response parsing edge cases
func TestResponseParsing(t *testing.T) {
	t.Run("max_tokens stop reason", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			response := map[string]interface{}{
				"id":           "msg-123",
				"type":         "message",
				"role":         "assistant",
				"content":      []interface{}{map[string]string{"type": "text", "text": "Response"}},
				"stop_reason":  "max_tokens",
				"model":        "claude-3-5-sonnet-20241022",
				"stop_sequence": nil,
				"usage":        map[string]interface{}{"input_tokens": 10, "output_tokens": 5},
			}
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(response)
		}))
		defer server.Close()

		p := NewProviderWithBaseURL("test-token", server.URL)
		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		resp, err := p.Chat(ctx, messages, nil, "claude-3-5-sonnet-20241022", nil)
		if err != nil {
			t.Fatalf("Chat failed: %v", err)
		}

		if resp.FinishReason != "length" {
			t.Errorf("expected 'length', got '%s'", resp.FinishReason)
		}
	})

	t.Run("multiple content blocks", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			response := map[string]interface{}{
				"id":   "msg-123",
				"type": "message",
				"role": "assistant",
				"content": []interface{}{
					map[string]string{"type": "text", "text": "Hello "},
					map[string]string{"type": "text", "text": "world!"},
				},
				"stop_reason":  "end_turn",
				"model":        "claude-3-5-sonnet-20241022",
				"stop_sequence": nil,
				"usage":        map[string]interface{}{"input_tokens": 10, "output_tokens": 5},
			}
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(response)
		}))
		defer server.Close()

		p := NewProviderWithBaseURL("test-token", server.URL)
		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		resp, err := p.Chat(ctx, messages, nil, "claude-3-5-sonnet-20241022", nil)
		if err != nil {
			t.Fatalf("Chat failed: %v", err)
		}

		if resp.Content != "Hello world!" {
			t.Errorf("expected 'Hello world!', got '%s'", resp.Content)
		}
	})

	t.Run("empty content", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			response := map[string]interface{}{
				"id":           "msg-123",
				"type":         "message",
				"role":         "assistant",
				"content":      []interface{}{},
				"stop_reason":  "end_turn",
				"model":        "claude-3-5-sonnet-20241022",
				"stop_sequence": nil,
				"usage":        map[string]interface{}{"input_tokens": 10, "output_tokens": 0},
			}
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(response)
		}))
		defer server.Close()

		p := NewProviderWithBaseURL("test-token", server.URL)
		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		resp, err := p.Chat(ctx, messages, nil, "claude-3-5-sonnet-20241022", nil)
		if err != nil {
			t.Fatalf("Chat failed: %v", err)
		}

		if resp.Content != "" {
			t.Errorf("expected empty content, got '%s'", resp.Content)
		}
	})
}

// TestBuildParamsTests tests buildParams function indirectly through Chat
func TestBuildParamsIndirect(t *testing.T) {
	t.Run("default max_tokens when not specified", func(t *testing.T) {
		var receivedMaxTokens int64
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			var body map[string]interface{}
			json.NewDecoder(r.Body).Decode(&body)
			receivedMaxTokens = int64(body["max_tokens"].(float64))

			response := map[string]interface{}{
				"id":           "msg-123",
				"type":         "message",
				"role":         "assistant",
				"content":      []interface{}{map[string]string{"type": "text", "text": "Response"}},
				"stop_reason":  "end_turn",
				"model":        "claude-3-5-sonnet-20241022",
				"stop_sequence": nil,
				"usage":        map[string]interface{}{"input_tokens": 10, "output_tokens": 5},
			}
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(response)
		}))
		defer server.Close()

		p := NewProviderWithBaseURL("test-token", server.URL)
		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		// Don't pass max_tokens in options
		_, err := p.Chat(ctx, messages, nil, "claude-3-5-sonnet-20241022", nil)
		if err != nil {
			t.Fatalf("Chat failed: %v", err)
		}

		if receivedMaxTokens != 4096 {
			t.Errorf("expected default 4096, got %d", receivedMaxTokens)
		}
	})

	t.Run("tools are included", func(t *testing.T) {
		var hasTools bool
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			var body map[string]interface{}
			json.NewDecoder(r.Body).Decode(&body)
			_, hasTools = body["tools"]

			response := map[string]interface{}{
				"id":           "msg-123",
				"type":         "message",
				"role":         "assistant",
				"content":      []interface{}{map[string]string{"type": "text", "text": "Response"}},
				"stop_reason":  "end_turn",
				"model":        "claude-3-5-sonnet-20241022",
				"stop_sequence": nil,
				"usage":        map[string]interface{}{"input_tokens": 10, "output_tokens": 5},
			}
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(response)
		}))
		defer server.Close()

		p := NewProviderWithBaseURL("test-token", server.URL)
		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}
		tools := []protocoltypes.ToolDefinition{
			{
				Type: "function",
				Function: protocoltypes.ToolFunctionDefinition{
					Name:        "test",
					Description: "Test",
					Parameters: map[string]interface{}{
						"type":       "object",
						"properties": map[string]interface{}{},
					},
				},
			},
		}

		_, err := p.Chat(ctx, messages, tools, "claude-3-5-sonnet-20241022", nil)
		if err != nil {
			t.Fatalf("Chat failed: %v", err)
		}

		if !hasTools {
			t.Error("expected tools in request")
		}
	})
}

// TestBuildParamsDirect tests buildParams function directly
func TestBuildParamsDirect(t *testing.T) {
	t.Run("empty messages", func(t *testing.T) {
		params, err := buildParams([]protocoltypes.Message{}, nil, "claude-3-5-sonnet-20241022", nil)
		if err != nil {
			t.Fatalf("buildParams failed: %v", err)
		}
		if len(params.Messages) != 0 {
			t.Errorf("expected empty messages, got %d", len(params.Messages))
		}
		if params.MaxTokens != 4096 {
			t.Errorf("expected default max_tokens 4096, got %d", params.MaxTokens)
		}
	})

	t.Run("invalid model name", func(t *testing.T) {
		params, err := buildParams([]protocoltypes.Message{
			{Role: "user", Content: "Hello"},
		}, nil, "invalid_model", nil)
		if err != nil {
			t.Fatalf("buildParams failed: %v", err)
		}
		// Should still build params even with invalid model name
		if string(params.Model) != "invalid_model" {
			t.Errorf("expected model to be set to invalid_model, got %s", params.Model)
		}
	})

	t.Run("negative max_tokens", func(t *testing.T) {
		params, err := buildParams([]protocoltypes.Message{
			{Role: "user", Content: "Hello"},
		}, nil, "claude-3-5-sonnet-20241022", map[string]interface{}{
			"max_tokens": -1,
		})
		if err != nil {
			t.Fatalf("buildParams failed: %v", err)
		}
		// Should still set the max_tokens even if negative
		if params.MaxTokens != -1 {
			t.Errorf("expected max_tokens to be -1, got %d", params.MaxTokens)
		}
	})

	t.Run("negative temperature", func(t *testing.T) {
		params, err := buildParams([]protocoltypes.Message{
			{Role: "user", Content: "Hello"},
		}, nil, "claude-3-5-sonnet-20241022", map[string]interface{}{
			"temperature": float64(-0.5),
		})
		if err != nil {
			t.Fatalf("buildParams failed: %v", err)
		}
		if params.Temperature.Value != -0.5 {
			t.Errorf("expected temperature to be -0.5, got %f", params.Temperature.Value)
		}
	})

	t.Run("invalid temperature type", func(t *testing.T) {
		params, err := buildParams([]protocoltypes.Message{
			{Role: "user", Content: "Hello"},
		}, nil, "claude-3-5-sonnet-20241022", map[string]interface{}{
			"temperature": "not a float",
		})
		if err != nil {
			t.Fatalf("buildParams failed: %v", err)
		}
		// Should not set temperature if type is wrong
		if params.Temperature.Value != 0 {
			t.Errorf("expected temperature to be 0, got %f", params.Temperature.Value)
		}
	})

	t.Run("tool with invalid parameters", func(t *testing.T) {
		tools := []protocoltypes.ToolDefinition{
			{
				Type: "function",
				Function: protocoltypes.ToolFunctionDefinition{
					Name: "test_tool",
					Parameters: map[string]interface{}{
						"type": "object",
						"properties": map[string]interface{}{
							"invalid_prop": map[string]interface{}{"type": "invalid_type"},
						},
					},
				},
			},
		}
		params, err := buildParams([]protocoltypes.Message{
			{Role: "user", Content: "Hello"},
		}, tools, "claude-3-5-sonnet-20241022", nil)
		if err != nil {
			t.Fatalf("buildParams failed: %v", err)
		}
		if len(params.Tools) != 1 {
			t.Errorf("expected 1 tool, got %d", len(params.Tools))
		}
	})
}

// TestTranslateTools tests tool translation indirectly through Chat
func TestTranslateToolsIndirect(t *testing.T) {
	t.Run("tool with required parameters", func(t *testing.T) {
		var receivedTools []interface{}
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			var body map[string]interface{}
			json.NewDecoder(r.Body).Decode(&body)
			receivedTools = body["tools"].([]interface{})

			response := map[string]interface{}{
				"id":           "msg-123",
				"type":         "message",
				"role":         "assistant",
				"content":      []interface{}{map[string]string{"type": "text", "text": "Response"}},
				"stop_reason":  "end_turn",
				"model":        "claude-3-5-sonnet-20241022",
				"stop_sequence": nil,
				"usage":        map[string]interface{}{"input_tokens": 10, "output_tokens": 5},
			}
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(response)
		}))
		defer server.Close()

		p := NewProviderWithBaseURL("test-token", server.URL)
		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}
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

		_, err := p.Chat(ctx, messages, tools, "claude-3-5-sonnet-20241022", nil)
		if err != nil {
			t.Fatalf("Chat failed: %v", err)
		}

		if len(receivedTools) != 1 {
			t.Fatalf("expected 1 tool, got %d", len(receivedTools))
		}
	})

	t.Run("tool without description", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			response := map[string]interface{}{
				"id":           "msg-123",
				"type":         "message",
				"role":         "assistant",
				"content":      []interface{}{map[string]string{"type": "text", "text": "Response"}},
				"stop_reason":  "end_turn",
				"model":        "claude-3-5-sonnet-20241022",
				"stop_sequence": nil,
				"usage":        map[string]interface{}{"input_tokens": 10, "output_tokens": 5},
			}
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(response)
		}))
		defer server.Close()

		p := NewProviderWithBaseURL("test-token", server.URL)
		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}
		tools := []protocoltypes.ToolDefinition{
			{
				Type: "function",
				Function: protocoltypes.ToolFunctionDefinition{
					Name:       "test_tool",
					Parameters: map[string]interface{}{"type": "object", "properties": map[string]interface{}{}},
				},
			},
		}

		_, err := p.Chat(ctx, messages, tools, "claude-3-5-sonnet-20241022", nil)
		if err != nil {
			t.Fatalf("Chat failed: %v", err)
		}
	})
}

// TestTranslateTools tests tool translation directly
func TestTranslateTools(t *testing.T) {
	t.Run("basic tool", func(t *testing.T) {
		tools := []protocoltypes.ToolDefinition{
			{
				Type: "function",
				Function: protocoltypes.ToolFunctionDefinition{
					Name:        "test_tool",
					Description: "A test tool",
					Parameters: map[string]interface{}{
						"type":       "object",
						"properties": map[string]interface{}{"param": map[string]string{"type": "string"}},
					},
				},
			},
		}
		result := translateTools(tools)

		if len(result) != 1 {
			t.Fatalf("expected 1 tool, got %d", len(result))
		}

		if result[0].OfTool.Name != "test_tool" {
			t.Errorf("expected tool name 'test_tool', got %s", result[0].OfTool.Name)
		}

		if result[0].OfTool.Description.Value != "A test tool" {
			t.Errorf("expected description to be set to 'A test tool', got %v", result[0].OfTool.Description)
		}

		if len(result[0].OfTool.InputSchema.Required) != 0 {
			t.Errorf("expected 0 required parameters, got %d", len(result[0].OfTool.InputSchema.Required))
		}
	})

	t.Run("tool without description", func(t *testing.T) {
		tools := []protocoltypes.ToolDefinition{
			{
				Type: "function",
				Function: protocoltypes.ToolFunctionDefinition{
					Name: "test_tool",
					Parameters: map[string]interface{}{
						"type":       "object",
						"properties": map[string]interface{}{},
					},
				},
			},
		}
		result := translateTools(tools)

		if len(result) != 1 {
			t.Fatalf("expected 1 tool, got %d", len(result))
		}

		if result[0].OfTool.Description.Value != "" {
			t.Error("expected description to be empty when not provided")
		}
	})

	t.Run("tool with required parameters", func(t *testing.T) {
		tools := []protocoltypes.ToolDefinition{
			{
				Type: "function",
				Function: protocoltypes.ToolFunctionDefinition{
					Name: "test_tool",
					Parameters: map[string]interface{}{
						"type":       "object",
						"properties": map[string]interface{}{"param": map[string]string{"type": "string"}},
						"required":   []interface{}{"param"},
					},
				},
			},
		}
		result := translateTools(tools)

		if len(result) != 1 {
			t.Fatalf("expected 1 tool, got %d", len(result))
		}

		if len(result[0].OfTool.InputSchema.Required) != 1 {
			t.Errorf("expected 1 required parameter, got %d", len(result[0].OfTool.InputSchema.Required))
		}

		if result[0].OfTool.InputSchema.Required[0] != "param" {
			t.Errorf("expected 'param' as required, got %s", result[0].OfTool.InputSchema.Required[0])
		}
	})

	t.Run("tool with non-string required parameters", func(t *testing.T) {
		tools := []protocoltypes.ToolDefinition{
			{
				Type: "function",
				Function: protocoltypes.ToolFunctionDefinition{
					Name: "test_tool",
					Parameters: map[string]interface{}{
						"type":       "object",
						"properties": map[string]interface{}{},
						"required":   []interface{}{123, "valid_param"},
					},
				},
			},
		}
		result := translateTools(tools)

		if len(result) != 1 {
			t.Fatalf("expected 1 tool, got %d", len(result))
		}

		if len(result[0].OfTool.InputSchema.Required) != 1 {
			t.Errorf("expected 1 required parameter (only strings), got %d", len(result[0].OfTool.InputSchema.Required))
		}

		if result[0].OfTool.InputSchema.Required[0] != "valid_param" {
			t.Errorf("expected 'valid_param' as required, got %s", result[0].OfTool.InputSchema.Required[0])
		}
	})

	t.Run("tool with empty required array", func(t *testing.T) {
		tools := []protocoltypes.ToolDefinition{
			{
				Type: "function",
				Function: protocoltypes.ToolFunctionDefinition{
					Name: "test_tool",
					Parameters: map[string]interface{}{
						"type":       "object",
						"properties": map[string]interface{}{},
						"required":   []interface{}{},
					},
				},
			},
		}
		result := translateTools(tools)

		if len(result) != 1 {
			t.Fatalf("expected 1 tool, got %d", len(result))
		}

		if len(result[0].OfTool.InputSchema.Required) != 0 {
			t.Errorf("expected 0 required parameters, got %d", len(result[0].OfTool.InputSchema.Required))
		}
	})

	t.Run("tool with invalid required type", func(t *testing.T) {
		tools := []protocoltypes.ToolDefinition{
			{
				Type: "function",
				Function: protocoltypes.ToolFunctionDefinition{
					Name: "test_tool",
					Parameters: map[string]interface{}{
						"type":     "object",
						"required": "not an array",
					},
				},
			},
		}
		result := translateTools(tools)

		if len(result) != 1 {
			t.Fatalf("expected 1 tool, got %d", len(result))
		}

		if len(result[0].OfTool.InputSchema.Required) != 0 {
			t.Errorf("expected 0 required parameters when type is wrong, got %d", len(result[0].OfTool.InputSchema.Required))
		}
	})

	t.Run("multiple tools", func(t *testing.T) {
		tools := []protocoltypes.ToolDefinition{
			{
				Type: "function",
				Function: protocoltypes.ToolFunctionDefinition{
					Name:        "tool1",
					Description: "First tool",
					Parameters: map[string]interface{}{
						"type":       "object",
						"properties": map[string]interface{}{},
					},
				},
			},
			{
				Type: "function",
				Function: protocoltypes.ToolFunctionDefinition{
					Name: "tool2",
					Parameters: map[string]interface{}{
						"type":       "object",
						"properties": map[string]interface{}{},
					},
				},
			},
		}
		result := translateTools(tools)

		if len(result) != 2 {
			t.Fatalf("expected 2 tools, got %d", len(result))
		}

		if result[0].OfTool.Name != "tool1" {
			t.Errorf("expected first tool name 'tool1', got %s", result[0].OfTool.Name)
		}

		if result[1].OfTool.Name != "tool2" {
			t.Errorf("expected second tool name 'tool2', got %s", result[1].OfTool.Name)
		}
	})

	t.Run("empty tools array", func(t *testing.T) {
		result := translateTools([]protocoltypes.ToolDefinition{})
		if len(result) != 0 {
			t.Errorf("expected empty result, got %d tools", len(result))
		}
	})

	t.Run("tool with complex properties", func(t *testing.T) {
		tools := []protocoltypes.ToolDefinition{
			{
				Type: "function",
				Function: protocoltypes.ToolFunctionDefinition{
					Name: "complex_tool",
					Parameters: map[string]interface{}{
						"type": "object",
						"properties": map[string]interface{}{
							"name": map[string]interface{}{
								"type": "string",
							},
							"age": map[string]interface{}{
								"type": "integer",
							},
						},
					},
				},
			},
		}
		result := translateTools(tools)

		if len(result) != 1 {
			t.Fatalf("expected 1 tool, got %d", len(result))
		}

		// Just ensure the tool was created without errors
		if result[0].OfTool.Name != "complex_tool" {
			t.Errorf("expected tool name 'complex_tool', got %s", result[0].OfTool.Name)
		}
	})
}

// TestNormalizeBaseURL tests base URL normalization
func TestNormalizeBaseURL(t *testing.T) {
	testCases := []struct {
		name     string
		input    string
		expected string
	}{
		{"empty string", "", defaultBaseURL},
		{"whitespace", "   ", defaultBaseURL},
		{"simple URL", "https://api.example.com", "https://api.example.com"},
		{"with trailing slash", "https://api.example.com/", "https://api.example.com"},
		{"with multiple trailing slashes", "https://api.example.com///", "https://api.example.com"},
		{"with /v1 suffix", "https://api.example.com/v1", "https://api.example.com"},
		{"with /v1/ suffix", "https://api.example.com/v1/", "https://api.example.com"},
		{"with /v1 and trailing slash", "https://api.example.com/v1/", "https://api.example.com"},
		{"only /v1", "/v1", defaultBaseURL},
		{"only slash", "/", defaultBaseURL},
		{"with whitespace and URL", "  https://api.example.com  ", "https://api.example.com"},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			result := normalizeBaseURL(tc.input)
			if result != tc.expected {
				t.Errorf("normalizeBaseURL(%q) = %q, want %q", tc.input, result, tc.expected)
			}
		})
	}
}

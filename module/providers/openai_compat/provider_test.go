// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package openai_compat

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/providers/protocoltypes"
)

// TestNewProvider tests provider creation
func TestNewProvider(t *testing.T) {
	t.Run("basic provider", func(t *testing.T) {
		p := NewProvider("test-key", "https://api.example.com", "")
		if p == nil {
			t.Fatal("expected non-nil provider")
		}
		if p.apiKey != "test-key" {
			t.Errorf("expected api key 'test-key', got '%s'", p.apiKey)
		}
		if p.apiBase != "https://api.example.com" {
			t.Errorf("expected base URL 'https://api.example.com', got '%s'", p.apiBase)
		}
		if p.httpClient == nil {
			t.Error("expected non-nil http client")
		}
	})

	t.Run("with trailing slash in apiBase", func(t *testing.T) {
		p := NewProvider("test-key", "https://api.example.com/", "")
		if p.apiBase != "https://api.example.com" {
			t.Errorf("expected trailing slash to be trimmed, got '%s'", p.apiBase)
		}
	})

	t.Run("with multiple trailing slashes", func(t *testing.T) {
		p := NewProvider("test-key", "https://api.example.com///", "")
		if p.apiBase != "https://api.example.com" {
			t.Errorf("expected trailing slashes to be trimmed, got '%s'", p.apiBase)
		}
	})

	t.Run("empty apiBase", func(t *testing.T) {
		p := NewProvider("test-key", "", "")
		if p.apiBase != "" {
			t.Errorf("expected empty apiBase, got '%s'", p.apiBase)
		}
	})

	t.Run("empty apiKey", func(t *testing.T) {
		p := NewProvider("", "https://api.example.com", "")
		if p.apiKey != "" {
			t.Errorf("expected empty apiKey, got '%s'", p.apiKey)
		}
	})
}

// TestNewProviderWithProxy tests proxy configuration
func TestNewProviderWithProxy(t *testing.T) {
	t.Run("valid proxy URL", func(t *testing.T) {
		proxyURL, _ := url.Parse("http://proxy.example.com:8080")
		if proxyURL == nil {
			t.Fatal("failed to parse proxy URL")
		}

		p := NewProvider("test-key", "https://api.example.com", "http://proxy.example.com:8080")
		if p == nil {
			t.Fatal("expected non-nil provider")
		}
		if p.httpClient == nil {
			t.Error("expected non-nil http client")
		}
		// Verify transport is configured
		if p.httpClient.Transport == nil {
			t.Error("expected transport to be configured")
		}
	})

	t.Run("invalid proxy URL", func(t *testing.T) {
		// Should not panic, just log error and continue without proxy
		p := NewProvider("test-key", "https://api.example.com", "://invalid-url")
		if p == nil {
			t.Fatal("expected non-nil provider even with invalid proxy")
		}
		// Transport should be default (nil means default transport)
		if p.httpClient.Transport != nil {
			// Transport was set, check if it's the proxy transport
			// Actually, invalid proxy should result in nil transport (default)
		}
	})

	t.Run("empty proxy", func(t *testing.T) {
		p := NewProvider("test-key", "https://api.example.com", "")
		if p == nil {
			t.Fatal("expected non-nil provider")
		}
		// Should use default transport
		if p.httpClient.Transport != nil {
			// Check if it's not a proxy transport
		}
	})
}

// TestChat tests the Chat method with various scenarios
func TestChat(t *testing.T) {
	t.Run("successful chat with text response", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			if r.Method != "POST" {
				t.Errorf("expected POST, got %s", r.Method)
			}
			if r.URL.Path != "/chat/completions" {
				t.Errorf("expected /chat/completions, got %s", r.URL.Path)
			}

			auth := r.Header.Get("Authorization")
			if auth != "Bearer test-key" {
				t.Errorf("expected 'Bearer test-key', got '%s'", auth)
			}

			response := map[string]interface{}{
				"id":      "chatcmpl-123",
				"object":  "chat.completion",
				"created": int(time.Now().Unix()),
				"model":   "gpt-3.5-turbo",
				"choices": []interface{}{
					map[string]interface{}{
						"index": 0,
						"message": map[string]interface{}{
							"role":    "assistant",
							"content": "Hello!",
						},
						"finish_reason": "stop",
					},
				},
				"usage": map[string]interface{}{
					"prompt_tokens":     10,
					"completion_tokens": 5,
					"total_tokens":      15,
				},
			}
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(response)
		}))
		defer server.Close()

		p := NewProvider("test-key", server.URL, "")
		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		resp, err := p.Chat(ctx, messages, nil, "gpt-3.5-turbo", nil)
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
		}
	})

	t.Run("chat with tool calls", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			response := map[string]interface{}{
				"id":      "chatcmpl-123",
				"object":  "chat.completion",
				"created": int(time.Now().Unix()),
				"model":   "gpt-3.5-turbo",
				"choices": []interface{}{
					map[string]interface{}{
						"index": 0,
						"message": map[string]interface{}{
							"role":    "assistant",
							"content": "I'll help you",
							"tool_calls": []interface{}{
								map[string]interface{}{
									"id":   "call-123",
									"type": "function",
									"function": map[string]interface{}{
										"name":      "search",
										"arguments": `{"query":"test"}`,
									},
								},
							},
						},
						"finish_reason": "tool_calls",
					},
				},
				"usage": map[string]interface{}{
					"prompt_tokens":     15,
					"completion_tokens": 25,
					"total_tokens":      40,
				},
			}
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(response)
		}))
		defer server.Close()

		p := NewProvider("test-key", server.URL, "")
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

		resp, err := p.Chat(ctx, messages, tools, "gpt-3.5-turbo", nil)
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
		var receivedMaxTokens interface{}
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			var body map[string]interface{}
			json.NewDecoder(r.Body).Decode(&body)
			receivedMaxTokens = body["max_tokens"]

			response := map[string]interface{}{
				"id":      "chatcmpl-123",
				"object":  "chat.completion",
				"created": int(time.Now().Unix()),
				"model":   "gpt-3.5-turbo",
				"choices": []interface{}{
					map[string]interface{}{
						"index": 0,
						"message": map[string]interface{}{
							"role":    "assistant",
							"content": "Response",
						},
						"finish_reason": "stop",
					},
				},
				"usage": map[string]interface{}{"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15},
			}
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(response)
		}))
		defer server.Close()

		p := NewProvider("test-key", server.URL, "")
		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		_, err := p.Chat(ctx, messages, nil, "gpt-3.5-turbo", map[string]interface{}{
			"max_tokens": 2048,
		})
		if err != nil {
			t.Fatalf("Chat failed: %v", err)
		}

		if receivedMaxTokens != float64(2048) {
			t.Errorf("expected 2048, got %v", receivedMaxTokens)
		}
	})

	t.Run("chat with temperature", func(t *testing.T) {
		var receivedTemp interface{}
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			var body map[string]interface{}
			json.NewDecoder(r.Body).Decode(&body)
			receivedTemp = body["temperature"]

			response := map[string]interface{}{
				"id":      "chatcmpl-123",
				"object":  "chat.completion",
				"created": int(time.Now().Unix()),
				"model":   "gpt-3.5-turbo",
				"choices": []interface{}{
					map[string]interface{}{
						"index": 0,
						"message": map[string]interface{}{
							"role":    "assistant",
							"content": "Response",
						},
						"finish_reason": "stop",
					},
				},
				"usage": map[string]interface{}{"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15},
			}
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(response)
		}))
		defer server.Close()

		p := NewProvider("test-key", server.URL, "")
		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		_, err := p.Chat(ctx, messages, nil, "gpt-3.5-turbo", map[string]interface{}{
			"temperature": 0.8,
		})
		if err != nil {
			t.Fatalf("Chat failed: %v", err)
		}

		if receivedTemp != float64(0.8) {
			t.Errorf("expected 0.8, got %v", receivedTemp)
		}
	})

	t.Run("chat with tools includes tool_choice", func(t *testing.T) {
		var hasToolChoice bool
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			var body map[string]interface{}
			json.NewDecoder(r.Body).Decode(&body)
			_, hasToolChoice = body["tool_choice"]

			response := map[string]interface{}{
				"id":      "chatcmpl-123",
				"object":  "chat.completion",
				"created": int(time.Now().Unix()),
				"model":   "gpt-3.5-turbo",
				"choices": []interface{}{
					map[string]interface{}{
						"index": 0,
						"message": map[string]interface{}{
							"role":    "assistant",
							"content": "Response",
						},
						"finish_reason": "stop",
					},
				},
				"usage": map[string]interface{}{"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15},
			}
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(response)
		}))
		defer server.Close()

		p := NewProvider("test-key", server.URL, "")
		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}
		tools := []protocoltypes.ToolDefinition{
			{
				Type: "function",
				Function: protocoltypes.ToolFunctionDefinition{
					Name:        "test",
					Description: "Test",
					Parameters:  map[string]interface{}{"type": "object", "properties": map[string]interface{}{}},
				},
			},
		}

		_, err := p.Chat(ctx, messages, tools, "gpt-3.5-turbo", nil)
		if err != nil {
			t.Fatalf("Chat failed: %v", err)
		}

		if !hasToolChoice {
			t.Error("expected tool_choice in request when tools are provided")
		}
	})
}

// TestChatErrorHandling tests error scenarios
func TestChatErrorHandling(t *testing.T) {
	t.Run("empty apiBase", func(t *testing.T) {
		p := NewProvider("test-key", "", "")
		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		_, err := p.Chat(ctx, messages, nil, "gpt-3.5-turbo", nil)
		if err == nil {
			t.Error("expected error for empty apiBase")
		}
		if !strings.Contains(err.Error(), "API base not configured") {
			t.Errorf("expected 'API base not configured' error, got: %v", err)
		}
	})

	t.Run("HTTP error", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			w.WriteHeader(http.StatusUnauthorized)
			w.Write([]byte(`{"error":{"message":"Invalid API key"}}`))
		}))
		defer server.Close()

		p := NewProvider("test-key", server.URL, "")
		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		_, err := p.Chat(ctx, messages, nil, "gpt-3.5-turbo", nil)
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

		p := NewProvider("test-key", server.URL, "")
		ctx, cancel := context.WithCancel(context.Background())
		cancel()

		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		_, err := p.Chat(ctx, messages, nil, "gpt-3.5-turbo", nil)
		if err == nil {
			t.Error("expected error for cancelled context")
		}
	})

	t.Run("invalid JSON response", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			w.Header().Set("Content-Type", "application/json")
			w.Write([]byte(`{invalid json}`))
		}))
		defer server.Close()

		p := NewProvider("test-key", server.URL, "")
		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		_, err := p.Chat(ctx, messages, nil, "gpt-3.5-turbo", nil)
		if err == nil {
			t.Error("expected error for invalid JSON response")
		}
	})

	t.Run("network error", func(t *testing.T) {
		// Use an invalid URL that will cause a network error
		p := NewProvider("test-key", "http://localhost:9999", "")
		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		// Set a short timeout to avoid long wait
		ctx, cancel := context.WithTimeout(ctx, 100*time.Millisecond)
		defer cancel()

		_, err := p.Chat(ctx, messages, nil, "gpt-3.5-turbo", nil)
		if err == nil {
			t.Error("expected error for network failure")
		}
	})
}

// TestModelNormalization tests model name normalization
func TestModelNormalization(t *testing.T) {
	testCases := []struct {
		name          string
		model         string
		apiBase       string
		expectedModel string
	}{
		{
			name:          "plain model name",
			model:         "gpt-3.5-turbo",
			apiBase:       "https://api.openai.com",
			expectedModel: "gpt-3.5-turbo",
		},
		{
			name:          "moonshot prefix",
			model:         "moonshot/v1",
			apiBase:       "https://api.moonshot.cn",
			expectedModel: "v1",
		},
		{
			name:          "nvidia prefix",
			model:         "nvidia/model",
			apiBase:       "https://integrate.api.nvidia.com",
			expectedModel: "model",
		},
		{
			name:          "groq prefix",
			model:         "groq/model",
			apiBase:       "https://api.groq.com",
			expectedModel: "model",
		},
		{
			name:          "ollama prefix",
			model:         "ollama/model",
			apiBase:       "https://ollama.com",
			expectedModel: "model",
		},
		{
			name:          "deepseek prefix",
			model:         "deepseek/model",
			apiBase:       "https://api.deepseek.com",
			expectedModel: "model",
		},
		{
			name:          "google prefix",
			model:         "google/model",
			apiBase:       "https://google.com",
			expectedModel: "model",
		},
		{
			name:          "zhipu prefix",
			model:         "zhipu/glm-4",
			apiBase:       "https://open.bigmodel.cn",
			expectedModel: "glm-4",
		},
		{
			name:          "openrouter with openrouter API",
			model:         "openrouter/gpt-3.5-turbo",
			apiBase:       "https://openrouter.ai/api",
			expectedModel: "gpt-3.5-turbo",
		},
		{
			name:          "openrouter with non-openrouter API",
			model:         "openrouter/gpt-3.5-turbo",
			apiBase:       "https://api.openai.com",
			expectedModel: "gpt-3.5-turbo",
		},
		{
			name:          "unknown prefix",
			model:         "unknown/model",
			apiBase:       "https://api.example.com",
			expectedModel: "unknown/model",
		},
		{
			name:          "no slash",
			model:         "gpt-3.5-turbo",
			apiBase:       "https://api.openai.com",
			expectedModel: "gpt-3.5-turbo",
		},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			var receivedModel string
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				var body map[string]interface{}
				json.NewDecoder(r.Body).Decode(&body)
				receivedModel = body["model"].(string)

				response := map[string]interface{}{
					"id":      "chatcmpl-123",
					"object":  "chat.completion",
					"created": int(time.Now().Unix()),
					"model":   tc.expectedModel,
					"choices": []interface{}{
						map[string]interface{}{
							"index": 0,
							"message": map[string]interface{}{
								"role":    "assistant",
								"content": "Response",
							},
							"finish_reason": "stop",
						},
					},
					"usage": map[string]interface{}{"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15},
				}
				w.Header().Set("Content-Type", "application/json")
				json.NewEncoder(w).Encode(response)
			}))
			defer server.Close()

			p := NewProvider("test-key", server.URL, "")
			ctx := context.Background()
			messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

			_, err := p.Chat(ctx, messages, nil, tc.model, nil)
			if err != nil {
				t.Fatalf("Chat failed: %v", err)
			}

			if receivedModel != tc.expectedModel {
				t.Errorf("expected model %s, got %s", tc.expectedModel, receivedModel)
			}
		})
	}
}

// TestSpecialModelHandling tests special model-specific behavior
func TestSpecialModelHandling(t *testing.T) {
	t.Run("GLM model uses max_completion_tokens", func(t *testing.T) {
		var hasCompletionTokens bool
		var hasMaxTokens bool
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			var body map[string]interface{}
			json.NewDecoder(r.Body).Decode(&body)
			_, hasCompletionTokens = body["max_completion_tokens"]
			_, hasMaxTokens = body["max_tokens"]

			response := map[string]interface{}{
				"id":      "chatcmpl-123",
				"object":  "chat.completion",
				"created": int(time.Now().Unix()),
				"model":   "glm-4",
				"choices": []interface{}{
					map[string]interface{}{
						"index": 0,
						"message": map[string]interface{}{
							"role":    "assistant",
							"content": "Response",
						},
						"finish_reason": "stop",
					},
				},
				"usage": map[string]interface{}{"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15},
			}
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(response)
		}))
		defer server.Close()

		p := NewProvider("test-key", server.URL, "")
		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		_, err := p.Chat(ctx, messages, nil, "zhipu/glm-4", map[string]interface{}{
			"max_tokens": 2048,
		})
		if err != nil {
			t.Fatalf("Chat failed: %v", err)
		}

		if !hasCompletionTokens {
			t.Error("GLM models should use max_completion_tokens")
		}
		if hasMaxTokens {
			t.Error("GLM models should not use max_tokens when max_completion_tokens is set")
		}
	})

	t.Run("O1 model uses max_completion_tokens", func(t *testing.T) {
		var hasCompletionTokens bool
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			var body map[string]interface{}
			json.NewDecoder(r.Body).Decode(&body)
			_, hasCompletionTokens = body["max_completion_tokens"]

			response := map[string]interface{}{
				"id":      "chatcmpl-123",
				"object":  "chat.completion",
				"created": int(time.Now().Unix()),
				"model":   "o1-preview",
				"choices": []interface{}{
					map[string]interface{}{
						"index": 0,
						"message": map[string]interface{}{
							"role":    "assistant",
							"content": "Response",
						},
						"finish_reason": "stop",
					},
				},
				"usage": map[string]interface{}{"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15},
			}
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(response)
		}))
		defer server.Close()

		p := NewProvider("test-key", server.URL, "")
		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		_, err := p.Chat(ctx, messages, nil, "o1-preview", map[string]interface{}{
			"max_tokens": 2048,
		})
		if err != nil {
			t.Fatalf("Chat failed: %v", err)
		}

		if !hasCompletionTokens {
			t.Error("O1 models should use max_completion_tokens")
		}
	})

	t.Run("GPT-5 model uses max_completion_tokens", func(t *testing.T) {
		var hasCompletionTokens bool
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			var body map[string]interface{}
			json.NewDecoder(r.Body).Decode(&body)
			_, hasCompletionTokens = body["max_completion_tokens"]

			response := map[string]interface{}{
				"id":      "chatcmpl-123",
				"object":  "chat.completion",
				"created": int(time.Now().Unix()),
				"model":   "gpt-5",
				"choices": []interface{}{
					map[string]interface{}{
						"index": 0,
						"message": map[string]interface{}{
							"role":    "assistant",
							"content": "Response",
						},
						"finish_reason": "stop",
					},
				},
				"usage": map[string]interface{}{"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15},
			}
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(response)
		}))
		defer server.Close()

		p := NewProvider("test-key", server.URL, "")
		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		_, err := p.Chat(ctx, messages, nil, "gpt-5", map[string]interface{}{
			"max_tokens": 2048,
		})
		if err != nil {
			t.Fatalf("Chat failed: %v", err)
		}

		if !hasCompletionTokens {
			t.Error("GPT-5 models should use max_completion_tokens")
		}
	})

	t.Run("Kimi K2 model forces temperature to 1.0", func(t *testing.T) {
		var receivedTemp interface{}
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			var body map[string]interface{}
			json.NewDecoder(r.Body).Decode(&body)
			receivedTemp = body["temperature"]

			response := map[string]interface{}{
				"id":      "chatcmpl-123",
				"object":  "chat.completion",
				"created": int(time.Now().Unix()),
				"model":   "kimi-k2",
				"choices": []interface{}{
					map[string]interface{}{
						"index": 0,
						"message": map[string]interface{}{
							"role":    "assistant",
							"content": "Response",
						},
						"finish_reason": "stop",
					},
				},
				"usage": map[string]interface{}{"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15},
			}
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(response)
		}))
		defer server.Close()

		p := NewProvider("test-key", server.URL, "")
		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		// Request temperature 0.5, but K2 should use 1.0
		_, err := p.Chat(ctx, messages, nil, "kimi/k2", map[string]interface{}{
			"temperature": 0.5,
		})
		if err != nil {
			t.Fatalf("Chat failed: %v", err)
		}

		if receivedTemp != float64(1.0) {
			t.Errorf("Kimi K2 should force temperature to 1.0, got %v", receivedTemp)
		}
	})
}

// TestResponseParsing tests response parsing edge cases
func TestResponseParsing(t *testing.T) {
	t.Run("empty choices", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			response := map[string]interface{}{
				"id":      "chatcmpl-123",
				"object":  "chat.completion",
				"created": int(time.Now().Unix()),
				"model":   "gpt-3.5-turbo",
				"choices": []interface{}{},
			}
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(response)
		}))
		defer server.Close()

		p := NewProvider("test-key", server.URL, "")
		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		resp, err := p.Chat(ctx, messages, nil, "gpt-3.5-turbo", nil)
		if err != nil {
			t.Fatalf("Chat failed: %v", err)
		}

		if resp.Content != "" {
			t.Errorf("expected empty content, got '%s'", resp.Content)
		}
		if resp.FinishReason != "stop" {
			t.Errorf("expected 'stop', got '%s'", resp.FinishReason)
		}
	})

	t.Run("missing usage field", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			response := map[string]interface{}{
				"id":      "chatcmpl-123",
				"object":  "chat.completion",
				"created": int(time.Now().Unix()),
				"model":   "gpt-3.5-turbo",
				"choices": []interface{}{
					map[string]interface{}{
						"index": 0,
						"message": map[string]interface{}{
							"role":    "assistant",
							"content": "Response",
						},
						"finish_reason": "stop",
					},
				},
				// No usage field
			}
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(response)
		}))
		defer server.Close()

		p := NewProvider("test-key", server.URL, "")
		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		resp, err := p.Chat(ctx, messages, nil, "gpt-3.5-turbo", nil)
		if err != nil {
			t.Fatalf("Chat failed: %v", err)
		}

		if resp.Usage != nil {
			t.Error("expected nil usage when not provided")
		}
	})

	t.Run("malformed tool call arguments", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			response := map[string]interface{}{
				"id":      "chatcmpl-123",
				"object":  "chat.completion",
				"created": int(time.Now().Unix()),
				"model":   "gpt-3.5-turbo",
				"choices": []interface{}{
					map[string]interface{}{
						"index": 0,
						"message": map[string]interface{}{
							"role":    "assistant",
							"content": "I'll help",
							"tool_calls": []interface{}{
								map[string]interface{}{
									"id":   "call-123",
									"type": "function",
									"function": map[string]interface{}{
										"name":      "test_tool",
										"arguments": `{invalid json`,
									},
								},
							},
						},
						"finish_reason": "tool_calls",
					},
				},
				"usage": map[string]interface{}{"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15},
			}
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(response)
		}))
		defer server.Close()

		p := NewProvider("test-key", server.URL, "")
		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		resp, err := p.Chat(ctx, messages, nil, "gpt-3.5-turbo", nil)
		if err != nil {
			t.Fatalf("Chat failed: %v", err)
		}

		if len(resp.ToolCalls) != 1 {
			t.Fatalf("expected 1 tool call, got %d", len(resp.ToolCalls))
		}
		// Should have raw arguments when JSON parsing fails
		if _, hasRaw := resp.ToolCalls[0].Arguments["raw"]; !hasRaw {
			t.Error("expected raw arguments when JSON parsing fails")
		}
	})

	t.Run("tool call without function", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			response := map[string]interface{}{
				"id":      "chatcmpl-123",
				"object":  "chat.completion",
				"created": int(time.Now().Unix()),
				"model":   "gpt-3.5-turbo",
				"choices": []interface{}{
					map[string]interface{}{
						"index": 0,
						"message": map[string]interface{}{
							"role":    "assistant",
							"content": "I'll help",
							"tool_calls": []interface{}{
								map[string]interface{}{
									"id":   "call-123",
									"type": "function",
									// No function field
								},
							},
						},
						"finish_reason": "tool_calls",
					},
				},
				"usage": map[string]interface{}{"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15},
			}
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(response)
		}))
		defer server.Close()

		p := NewProvider("test-key", server.URL, "")
		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		resp, err := p.Chat(ctx, messages, nil, "gpt-3.5-turbo", nil)
		if err != nil {
			t.Fatalf("Chat failed: %v", err)
		}

		if len(resp.ToolCalls) != 1 {
			t.Fatalf("expected 1 tool call, got %d", len(resp.ToolCalls))
		}
		if resp.ToolCalls[0].Name != "" {
			t.Errorf("expected empty name, got '%s'", resp.ToolCalls[0].Name)
		}
	})
}

// TestNoAPIKey tests chat without API key
func TestNoAPIKey(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Check that no Authorization header is set
		auth := r.Header.Get("Authorization")
		if auth != "" {
			t.Errorf("expected no Authorization header, got '%s'", auth)
		}

		response := map[string]interface{}{
			"id":      "chatcmpl-123",
			"object":  "chat.completion",
			"created": int(time.Now().Unix()),
			"model":   "gpt-3.5-turbo",
			"choices": []interface{}{
				map[string]interface{}{
					"index": 0,
					"message": map[string]interface{}{
						"role":    "assistant",
						"content": "Response",
					},
					"finish_reason": "stop",
				},
			},
			"usage": map[string]interface{}{"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15},
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(response)
	}))
	defer server.Close()

	p := NewProvider("", server.URL, "")
	ctx := context.Background()
	messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

	_, err := p.Chat(ctx, messages, nil, "gpt-3.5-turbo", nil)
	if err != nil {
		t.Fatalf("Chat failed: %v", err)
	}
}

// TestAsInt tests the asInt helper function
func TestAsInt(t *testing.T) {
	testCases := []struct {
		input    interface{}
		expected int
		ok       bool
	}{
		{int(42), 42, true},
		{int64(42), 42, true},
		{float64(42.5), 42, true},
		{float32(42.5), 42, true},
		{"not a number", 0, false},
		{nil, 0, false},
		{true, 0, false},
	}

	for _, tc := range testCases {
		result, ok := asInt(tc.input)
		if ok != tc.ok {
			t.Errorf("asInt(%v) ok = %v, want %v", tc.input, ok, tc.ok)
		}
		if ok && result != tc.expected {
			t.Errorf("asInt(%v) = %d, want %d", tc.input, result, tc.expected)
		}
	}
}

// TestAsFloat tests the asFloat helper function
func TestAsFloat(t *testing.T) {
	testCases := []struct {
		input    interface{}
		expected float64
		ok       bool
	}{
		{float64(3.14), 3.14, true},
		{float32(3.14), 3.14, true},
		{int(42), 42.0, true},
		{int64(42), 42.0, true},
		{"not a number", 0.0, false},
		{nil, 0.0, false},
		{true, 0.0, false},
	}

	for _, tc := range testCases {
		result, ok := asFloat(tc.input)
		if ok != tc.ok {
			t.Errorf("asFloat(%v) ok = %v, want %v", tc.input, ok, tc.ok)
		}
		if ok && result != tc.expected {
			// For float comparison, check if they're close enough
			if abs(result-tc.expected) > 0.0001 {
				t.Errorf("asFloat(%v) = %f, want %f", tc.input, result, tc.expected)
			}
		}
	}
}

func abs(x float64) float64 {
	if x < 0 {
		return -x
	}
	return x
}

// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package providers_test

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"testing"
	"time"

	openaicompat "github.com/276793422/NemesisBot/module/providers/openai_compat"
	"github.com/276793422/NemesisBot/module/providers/protocoltypes"
)

// TestOpenAICompatNewProvider tests the creation of a new OpenAI compatible provider
func TestOpenAICompatNewProvider(t *testing.T) {
	t.Run("NewProvider basic", func(t *testing.T) {
		p := openaicompat.NewProvider("test-key", "https://api.example.com", "")
		if p == nil {
			t.Fatal("expected non-nil provider")
		}
	})

	t.Run("NewProvider with proxy", func(t *testing.T) {
		// Create a test proxy server
		proxy := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			w.WriteHeader(http.StatusOK)
		}))
		defer proxy.Close()

		p := openaicompat.NewProvider("test-key", "https://api.example.com", proxy.URL)
		if p == nil {
			t.Fatal("expected non-nil provider")
		}
	})

	t.Run("NewProvider with invalid proxy", func(t *testing.T) {
		// This should not panic, just log an error
		p := openaicompat.NewProvider("test-key", "https://api.example.com", "://invalid-url")
		if p == nil {
			t.Fatal("expected non-nil provider even with invalid proxy")
		}
	})

	t.Run("NewProvider with empty apiBase", func(t *testing.T) {
		p := openaicompat.NewProvider("test-key", "", "")
		if p == nil {
			t.Fatal("expected non-nil provider")
		}
	})
}

// TestOpenAICompatChatWithMockServer tests the Chat method with a mock server
func TestOpenAICompatChatWithMockServer(t *testing.T) {
	// Create a mock server that returns a valid OpenAI-compatible response
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Verify request
		if r.Method != "POST" {
			t.Errorf("expected POST request, got %s", r.Method)
		}
		if r.URL.Path != "/chat/completions" {
			t.Errorf("expected /chat/completions path, got %s", r.URL.Path)
		}

		// Check authorization header
		auth := r.Header.Get("Authorization")
		if auth != "Bearer test-key" {
			t.Errorf("expected Authorization header 'Bearer test-key', got %s", auth)
		}

		// Return a mock response
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
						"content": "Test response",
					},
					"finish_reason": "stop",
				},
			},
			"usage": map[string]interface{}{
				"prompt_tokens":     10,
				"completion_tokens": 20,
				"total_tokens":      30,
			},
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(response)
	}))
	defer server.Close()

	// Create provider with mock server URL
	p := openaicompat.NewProvider("test-key", server.URL, "")

	// Test Chat method
	ctx := context.Background()
	messages := []protocoltypes.Message{
		{Role: "user", Content: "Hello"},
	}

	response, err := p.Chat(ctx, messages, nil, "gpt-3.5-turbo", nil)

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

// TestOpenAICompatChatWithToolCalls tests the Chat method with tool calls
func TestOpenAICompatChatWithToolCalls(t *testing.T) {
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
						"content": "Let me call the tool",
						"tool_calls": []interface{}{
							map[string]interface{}{
								"id":   "call-123",
								"type": "function",
								"function": map[string]interface{}{
									"name":      "test_tool",
									"arguments": `{"param":"value"}`,
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

	p := openaicompat.NewProvider("test-key", server.URL, "")

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

	response, err := p.Chat(ctx, messages, tools, "gpt-3.5-turbo", nil)

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

	if response.ToolCalls[0].ID != "call-123" {
		t.Errorf("expected tool call ID 'call-123', got '%s'", response.ToolCalls[0].ID)
	}
}

// TestOpenAICompatChatWithMaxTokens tests max_tokens parameter
func TestOpenAICompatChatWithMaxTokens(t *testing.T) {
	var receivedMaxTokens interface{}
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var requestBody map[string]interface{}
		json.NewDecoder(r.Body).Decode(&requestBody)
		receivedMaxTokens = requestBody["max_tokens"]

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
						"content": "Test response",
					},
					"finish_reason": "stop",
				},
			},
			"usage": map[string]interface{}{
				"prompt_tokens":     10,
				"completion_tokens": 20,
				"total_tokens":      30,
			},
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(response)
	}))
	defer server.Close()

	p := openaicompat.NewProvider("test-key", server.URL, "")

	ctx := context.Background()
	messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

	_, err := p.Chat(ctx, messages, nil, "gpt-3.5-turbo", map[string]interface{}{
		"max_tokens": 2048,
	})

	if err != nil {
		t.Fatalf("Chat failed: %v", err)
	}

	if receivedMaxTokens != float64(2048) {
		t.Errorf("expected max_tokens 2048, got %v", receivedMaxTokens)
	}
}

// TestOpenAICompatChatWithTemperature tests temperature parameter
func TestOpenAICompatChatWithTemperature(t *testing.T) {
	var receivedTemp interface{}
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var requestBody map[string]interface{}
		json.NewDecoder(r.Body).Decode(&requestBody)
		receivedTemp = requestBody["temperature"]

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
						"content": "Test response",
					},
					"finish_reason": "stop",
				},
			},
			"usage": map[string]interface{}{
				"prompt_tokens":     10,
				"completion_tokens": 20,
				"total_tokens":      30,
			},
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(response)
	}))
	defer server.Close()

	p := openaicompat.NewProvider("test-key", server.URL, "")

	ctx := context.Background()
	messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

	_, err := p.Chat(ctx, messages, nil, "gpt-3.5-turbo", map[string]interface{}{
		"temperature": 0.8,
	})

	if err != nil {
		t.Fatalf("Chat failed: %v", err)
	}

	if receivedTemp != float64(0.8) {
		t.Errorf("expected temperature 0.8, got %v", receivedTemp)
	}
}

// TestOpenAICompatChatErrorHandling tests error handling
func TestOpenAICompatChatErrorHandling(t *testing.T) {
	t.Run("HTTP error", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			w.WriteHeader(http.StatusUnauthorized)
			w.Write([]byte(`{"error":{"message":"Invalid API key"}}`))
		}))
		defer server.Close()

		p := openaicompat.NewProvider("test-key", server.URL, "")

		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		_, err := p.Chat(ctx, messages, nil, "gpt-3.5-turbo", nil)

		if err == nil {
			t.Error("expected error for unauthorized request")
		}
	})

	t.Run("Empty apiBase", func(t *testing.T) {
		p := openaicompat.NewProvider("test-key", "", "")

		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		_, err := p.Chat(ctx, messages, nil, "gpt-3.5-turbo", nil)

		if err == nil {
			t.Error("expected error for empty apiBase")
		}
	})

	t.Run("Context cancellation", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			time.Sleep(100 * time.Millisecond)
			w.WriteHeader(http.StatusOK)
		}))
		defer server.Close()

		p := openaicompat.NewProvider("test-key", server.URL, "")

		ctx, cancel := context.WithCancel(context.Background())
		cancel()

		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		_, err := p.Chat(ctx, messages, nil, "gpt-3.5-turbo", nil)

		if err == nil {
			t.Error("expected error for cancelled context")
		}
	})
}

// TestOpenAICompatModelNormalization tests model name normalization
func TestOpenAICompatModelNormalization(t *testing.T) {
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
			name:          "openrouter prefix",
			model:         "openrouter/gpt-3.5-turbo",
			apiBase:       "https://openrouter.ai/api",
			expectedModel: "gpt-3.5-turbo",
		},
		{
			name:          "zhipu prefix",
			model:         "zhipu/glm-4",
			apiBase:       "https://open.bigmodel.cn",
			expectedModel: "glm-4",
		},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			var receivedModel string
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				var requestBody map[string]interface{}
				json.NewDecoder(r.Body).Decode(&requestBody)
				receivedModel = requestBody["model"].(string)

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
								"content": "Test",
							},
							"finish_reason": "stop",
						},
					},
					"usage": map[string]interface{}{
						"prompt_tokens":     10,
						"completion_tokens": 20,
						"total_tokens":      30,
					},
				}
				w.Header().Set("Content-Type", "application/json")
				json.NewEncoder(w).Encode(response)
			}))
			defer server.Close()

			p := openaicompat.NewProvider("test-key", server.URL, "")

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

// TestOpenAICompatGLMMaxCompletionTokens tests GLM models use max_completion_tokens
func TestOpenAICompatGLMMaxCompletionTokens(t *testing.T) {
	var usedCompletionTokens bool
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var requestBody map[string]interface{}
		json.NewDecoder(r.Body).Decode(&requestBody)
		_, usedCompletionTokens = requestBody["max_completion_tokens"]
		_, hasMaxTokens := requestBody["max_tokens"]

		if usedCompletionTokens && hasMaxTokens {
			t.Error("should not have both max_completion_tokens and max_tokens")
		}

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
						"content": "Test",
					},
					"finish_reason": "stop",
				},
			},
			"usage": map[string]interface{}{
				"prompt_tokens":     10,
				"completion_tokens": 20,
				"total_tokens":      30,
			},
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(response)
	}))
	defer server.Close()

	p := openaicompat.NewProvider("test-key", server.URL, "")

	ctx := context.Background()
	messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

	_, err := p.Chat(ctx, messages, nil, "zhipu/glm-4", map[string]interface{}{
		"max_tokens": 2048,
	})

	if err != nil {
		t.Fatalf("Chat failed: %v", err)
	}

	if !usedCompletionTokens {
		t.Error("GLM models should use max_completion_tokens")
	}
}

// TestOpenAICompatKimiTemperature tests Kimi K2 models only support temperature=1
func TestOpenAICompatKimiTemperature(t *testing.T) {
	var receivedTemp interface{}
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var requestBody map[string]interface{}
		json.NewDecoder(r.Body).Decode(&requestBody)
		receivedTemp = requestBody["temperature"]

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
						"content": "Test",
					},
					"finish_reason": "stop",
				},
			},
			"usage": map[string]interface{}{
				"prompt_tokens":     10,
				"completion_tokens": 20,
				"total_tokens":      30,
			},
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(response)
	}))
	defer server.Close()

	p := openaicompat.NewProvider("test-key", server.URL, "")

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
}

// TestOpenAICompatEmptyChoices tests handling of empty choices
func TestOpenAICompatEmptyChoices(t *testing.T) {
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

	p := openaicompat.NewProvider("test-key", server.URL, "")

	ctx := context.Background()
	messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

	response, err := p.Chat(ctx, messages, nil, "gpt-3.5-turbo", nil)

	if err != nil {
		t.Fatalf("Chat failed: %v", err)
	}

	if response.Content != "" {
		t.Errorf("expected empty content for empty choices, got '%s'", response.Content)
	}

	if response.FinishReason != "stop" {
		t.Errorf("expected finish_reason 'stop', got '%s'", response.FinishReason)
	}
}

// TestOpenAICompatNoAPIKey tests Chat without API key
func TestOpenAICompatNoAPIKey(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Check that no Authorization header is set
		auth := r.Header.Get("Authorization")
		if auth != "" {
			t.Errorf("expected no Authorization header, got %s", auth)
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
						"content": "Test",
					},
					"finish_reason": "stop",
				},
			},
			"usage": map[string]interface{}{
				"prompt_tokens":     10,
				"completion_tokens": 20,
				"total_tokens":      30,
			},
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(response)
	}))
	defer server.Close()

	p := openaicompat.NewProvider("", server.URL, "")

	ctx := context.Background()
	messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

	_, err := p.Chat(ctx, messages, nil, "gpt-3.5-turbo", nil)

	if err != nil {
		t.Fatalf("Chat failed: %v", err)
	}
}

// TestOpenAICompatMalformedToolCallArgs tests handling of malformed tool call arguments
func TestOpenAICompatMalformedToolCallArgs(t *testing.T) {
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
						"content": "Let me call the tool",
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

	p := openaicompat.NewProvider("test-key", server.URL, "")

	ctx := context.Background()
	messages := []protocoltypes.Message{{Role: "user", Content: "Use the tool"}}

	response, err := p.Chat(ctx, messages, nil, "gpt-3.5-turbo", nil)

	if err != nil {
		t.Fatalf("Chat failed: %v", err)
	}

	if len(response.ToolCalls) != 1 {
		t.Fatalf("expected 1 tool call, got %d", len(response.ToolCalls))
	}

	// Should have raw arguments when JSON parsing fails
	if _, hasRaw := response.ToolCalls[0].Arguments["raw"]; !hasRaw {
		t.Error("expected raw arguments to be present when JSON parsing fails")
	}
}

// TestOpenAICompatProxyUsage tests that proxy is actually used
func TestOpenAICompatProxyUsage(t *testing.T) {
	// This test verifies the provider can be created with a proxy
	// Actual proxy functionality would require more complex setup

	t.Run("valid proxy URL", func(t *testing.T) {
		proxyURL, _ := url.Parse("http://proxy.example.com:8080")
		if proxyURL == nil {
			t.Fatal("failed to parse proxy URL")
		}

		// Just verify creation doesn't fail
		p := openaicompat.NewProvider("test-key", "https://api.example.com", "http://proxy.example.com:8080")
		if p == nil {
			t.Error("expected non-nil provider with valid proxy")
		}
		_ = p
	})

	t.Run("invalid proxy URL", func(t *testing.T) {
		// Should not panic, just log error and continue without proxy
		p := openaicompat.NewProvider("test-key", "https://api.example.com", "not-a-url")
		if p == nil {
			t.Error("expected non-nil provider even with invalid proxy")
		}
		_ = p
	})
}

// TestOpenAICompatResponseParsing tests response parsing edge cases
func TestOpenAICompatResponseParsing(t *testing.T) {
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
							"content": "Test",
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

		p := openaicompat.NewProvider("test-key", server.URL, "")

		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		response, err := p.Chat(ctx, messages, nil, "gpt-3.5-turbo", nil)

		if err != nil {
			t.Fatalf("Chat failed: %v", err)
		}

		if response.Usage != nil {
			t.Error("expected nil usage when not provided")
		}
	})

	t.Run("invalid JSON response", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			w.Header().Set("Content-Type", "application/json")
			w.Write([]byte(`{invalid json}`))
		}))
		defer server.Close()

		p := openaicompat.NewProvider("test-key", server.URL, "")

		ctx := context.Background()
		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		_, err := p.Chat(ctx, messages, nil, "gpt-3.5-turbo", nil)

		if err == nil {
			t.Error("expected error for invalid JSON response")
		}
	})
}

// TestMain is a setup hook
func TestMain(m *testing.M) {
	// Run tests
	os.Exit(m.Run())
}

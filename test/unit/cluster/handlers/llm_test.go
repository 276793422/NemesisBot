// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package handlers_test

import (
	"context"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/channels"
	"github.com/276793422/NemesisBot/module/cluster/handlers"
)

// TestRegisterLLMHandlers tests that LLM handlers are registered
func TestRegisterLLMHandlers(t *testing.T) {
	mockCluster := &mockClusterForHandlers{
		nodeID:      "test-node-1",
		address:     "127.0.0.1:21950",
		capabilities: []string{"llm_forward"},
		logMessages: []string{},
	}

	// Create a mock RPCChannel
	mockMsgBus := bus.NewMessageBus()
	cfg := &channels.RPCChannelConfig{
		MessageBus:      mockMsgBus,
		RequestTimeout:  60 * time.Second,
		CleanupInterval: 30 * time.Second,
	}
	rpcCh, err := channels.NewRPCChannel(cfg)
	if err != nil {
		t.Fatalf("Failed to create RPC channel: %v", err)
	}

	// Start the RPC channel
	ctx := context.Background()
	if err := rpcCh.Start(ctx); err != nil {
		t.Fatalf("Failed to start RPC channel: %v", err)
	}
	defer rpcCh.Stop(ctx)

	registeredHandlers := make(map[string]bool)

	// Create a registrar that tracks registered handlers
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		registeredHandlers[action] = true
	}

	// Register LLM handlers
	handlers.RegisterLLMHandlers(mockCluster, rpcCh, func(rpcChannel *channels.RPCChannel) func(map[string]interface{}) (map[string]interface{}, error) {
		return func(payload map[string]interface{}) (map[string]interface{}, error) {
			// Mock handler that simulates LLMForwardHandler behavior
			chatID, _ := payload["chat_id"].(string)
			content, _ := payload["content"].(string)

			if chatID == "" {
				return map[string]interface{}{
					"success": false,
					"error":   "chat_id is required",
				}, nil
			}

			if content == "" {
				return map[string]interface{}{
					"success": false,
					"error":   "content is required",
				}, nil
			}

			return map[string]interface{}{
				"success": true,
				"content": "mock response",
			}, nil
		}
	}, registrar)

	// Verify llm_forward handler is registered
	if !registeredHandlers["llm_forward"] {
		t.Error("Handler 'llm_forward' was not registered")
	}

	// Verify log message was written
	if len(mockCluster.logMessages) == 0 {
		t.Error("Expected log message to be written")
	}

	// Verify the handler function is not nil
	if !registeredHandlers["llm_forward"] {
		t.Fatal("llm_forward handler was not registered")
	}
}

// TestLLMForwardHandlerExists tests that the LLM forward handler can be created
func TestLLMForwardHandlerExists(t *testing.T) {
	mockCluster := &mockClusterForHandlers{
		nodeID:      "test-node-1",
		address:     "127.0.0.1:21950",
		capabilities: []string{"llm_forward"},
		logMessages: []string{},
	}

	// Create a mock RPCChannel
	mockMsgBus := bus.NewMessageBus()
	cfg := &channels.RPCChannelConfig{
		MessageBus:      mockMsgBus,
		RequestTimeout:  60 * time.Second,
		CleanupInterval: 30 * time.Second,
	}
	rpcCh, err := channels.NewRPCChannel(cfg)
	if err != nil {
		t.Fatalf("Failed to create RPC channel: %v", err)
	}

	// Start the RPC channel
	ctx := context.Background()
	if err := rpcCh.Start(ctx); err != nil {
		t.Fatalf("Failed to start RPC channel: %v", err)
	}
	defer rpcCh.Stop(ctx)

	var llmForwardHandler func(map[string]interface{}) (map[string]interface{}, error)

	// Capture the llm_forward handler
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if action == "llm_forward" {
			llmForwardHandler = handler
		}
	}

	handlers.RegisterLLMHandlers(mockCluster, rpcCh, func(rpcChannel *channels.RPCChannel) func(map[string]interface{}) (map[string]interface{}, error) {
		return func(payload map[string]interface{}) (map[string]interface{}, error) {
			// Mock handler that simulates LLMForwardHandler behavior
			chatID, _ := payload["chat_id"].(string)
			content, _ := payload["content"].(string)

			if chatID == "" {
				return map[string]interface{}{
					"success": false,
					"error":   "chat_id is required",
				}, nil
			}

			if content == "" {
				return map[string]interface{}{
					"success": false,
					"error":   "content is required",
				}, nil
			}

			return map[string]interface{}{
				"success": true,
				"content": "mock response",
			}, nil
		}
	}, registrar)

	if llmForwardHandler == nil {
		t.Fatal("llm_forward handler was not registered")
	}

	// Verify the handler is a function
	if llmForwardHandler == nil {
		t.Error("llm_forward handler is nil")
	}
}

// TestLLMForwardHandlerBasicCall tests basic call to the handler (will timeout without LLM)
func TestLLMForwardHandlerBasicCall(t *testing.T) {
	mockCluster := &mockClusterForHandlers{
		nodeID:      "test-node-1",
		address:     "127.0.0.1:21950",
		capabilities: []string{"llm_forward"},
		logMessages: []string{},
	}

	// Create a mock RPCChannel
	mockMsgBus := bus.NewMessageBus()
	cfg := &channels.RPCChannelConfig{
		MessageBus:      mockMsgBus,
		RequestTimeout:  2 * time.Second, // Short timeout for test
		CleanupInterval: 30 * time.Second,
	}
	rpcCh, err := channels.NewRPCChannel(cfg)
	if err != nil {
		t.Fatalf("Failed to create RPC channel: %v", err)
	}

	// Start the RPC channel
	ctx := context.Background()
	if err := rpcCh.Start(ctx); err != nil {
		t.Fatalf("Failed to start RPC channel: %v", err)
	}
	defer rpcCh.Stop(ctx)

	var llmForwardHandler func(map[string]interface{}) (map[string]interface{}, error)

	// Capture the llm_forward handler
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if action == "llm_forward" {
			llmForwardHandler = handler
		}
	}

	handlers.RegisterLLMHandlers(mockCluster, rpcCh, func(rpcChannel *channels.RPCChannel) func(map[string]interface{}) (map[string]interface{}, error) {
		return func(payload map[string]interface{}) (map[string]interface{}, error) {
			// Mock handler that simulates LLMForwardHandler behavior
			chatID, _ := payload["chat_id"].(string)
			content, _ := payload["content"].(string)

			if chatID == "" {
				return map[string]interface{}{
					"success": false,
					"error":   "chat_id is required",
				}, nil
			}

			if content == "" {
				return map[string]interface{}{
					"success": false,
					"error":   "content is required",
				}, nil
			}

			return map[string]interface{}{
				"success": true,
				"content": "mock response",
			}, nil
		}
	}, registrar)

	if llmForwardHandler == nil {
		t.Fatal("llm_forward handler was not registered")
	}

	// Test with missing required fields
	tests := []struct {
		name        string
		payload     map[string]interface{}
		expectError bool
		checkKey    string
		checkValue  interface{}
	}{
		{
			name: "Missing chat_id",
			payload: map[string]interface{}{
				"content": "test message",
			},
			expectError: false, // Handler returns error in response, not as error
			checkKey:    "success",
			checkValue:  false,
		},
		{
			name: "Missing content",
			payload: map[string]interface{}{
				"chat_id": "test-chat-1",
			},
			expectError: false,
			checkKey:    "success",
			checkValue:  false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			response, err := llmForwardHandler(tt.payload)
			if err != nil && tt.expectError {
				return // Expected error
			}
			if err != nil {
				t.Errorf("Unexpected error: %v", err)
				return
			}

			// Check response has the expected key/value
			val, ok := response[tt.checkKey]
			if !ok {
				t.Errorf("Response missing key '%s'", tt.checkKey)
				return
			}

			if val != tt.checkValue {
				t.Errorf("Expected '%s' = %v, got %v", tt.checkKey, tt.checkValue, val)
			}
		})
	}
}

// TestLLMForwardHandlerCallStructure tests that the handler has the correct structure
func TestLLMForwardHandlerCallStructure(t *testing.T) {
	mockCluster := &mockClusterForHandlers{
		nodeID:       "test-node-1",
		address:      "127.0.0.1:21950",
		capabilities: []string{"llm_forward"},
		logMessages:  []string{},
	}

	// Create a mock RPCChannel
	mockMsgBus := bus.NewMessageBus()
	cfg := &channels.RPCChannelConfig{
		MessageBus:      mockMsgBus,
		RequestTimeout:  1 * time.Second,
		CleanupInterval: 30 * time.Second,
	}
	rpcCh, err := channels.NewRPCChannel(cfg)
	if err != nil {
		t.Fatalf("Failed to create RPC channel: %v", err)
	}

	// Start the RPC channel
	ctx := context.Background()
	if err := rpcCh.Start(ctx); err != nil {
		t.Fatalf("Failed to start RPC channel: %v", err)
	}
	defer rpcCh.Stop(ctx)

	var llmForwardHandler func(map[string]interface{}) (map[string]interface{}, error)

	// Capture the llm_forward handler
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if action == "llm_forward" {
			llmForwardHandler = handler
		}
	}

	handlers.RegisterLLMHandlers(mockCluster, rpcCh, func(rpcChannel *channels.RPCChannel) func(map[string]interface{}) (map[string]interface{}, error) {
		return func(payload map[string]interface{}) (map[string]interface{}, error) {
			// Mock handler that simulates LLMForwardHandler behavior
			chatID, _ := payload["chat_id"].(string)
			content, _ := payload["content"].(string)

			if chatID == "" {
				return map[string]interface{}{
					"success": false,
					"error":   "chat_id is required",
				}, nil
			}

			if content == "" {
				return map[string]interface{}{
					"success": false,
					"error":   "content is required",
				}, nil
			}

			return map[string]interface{}{
				"success": true,
				"content": "mock response",
			}, nil
		}
	}, registrar)

	if llmForwardHandler == nil {
		t.Fatal("llm_forward handler was not registered")
	}

	// Test that handler returns proper error structure for missing fields
	payload := map[string]interface{}{
		"chat_id": "", // Empty chat_id should cause error
		"content": "test message",
	}

	response, err := llmForwardHandler(payload)
	if err != nil {
		t.Errorf("Handler returned error instead of error response: %v", err)
	}

	if response == nil {
		t.Fatal("Handler returned nil response")
	}

	if response["success"] != false {
		t.Errorf("Expected success=false for missing chat_id, got %v", response["success"])
	}

	if _, ok := response["error"]; !ok {
		t.Error("Expected error field in response for missing chat_id")
	}
}

// TestRegisterLLMHandlersLogMessage tests that registration produces a log message
func TestRegisterLLMHandlersLogMessage(t *testing.T) {
	mockCluster := &mockClusterForHandlers{
		nodeID:       "test-node-1",
		address:      "127.0.0.1:21950",
		capabilities: []string{"llm_forward"},
		logMessages:  []string{},
	}

	// Create a mock RPCChannel
	mockMsgBus := bus.NewMessageBus()
	cfg := &channels.RPCChannelConfig{
		MessageBus:      mockMsgBus,
		RequestTimeout:  1 * time.Second,
		CleanupInterval: 30 * time.Second,
	}
	rpcCh, err := channels.NewRPCChannel(cfg)
	if err != nil {
		t.Fatalf("Failed to create RPC channel: %v", err)
	}

	ctx := context.Background()
	if err := rpcCh.Start(ctx); err != nil {
		t.Fatalf("Failed to start RPC channel: %v", err)
	}
	defer rpcCh.Stop(ctx)

	registeredHandlers := make(map[string]bool)

	// Create a registrar that tracks registered handlers
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		registeredHandlers[action] = true
	}

	// Register LLM handlers
	handlers.RegisterLLMHandlers(mockCluster, rpcCh, func(rpcChannel *channels.RPCChannel) func(map[string]interface{}) (map[string]interface{}, error) {
		return func(payload map[string]interface{}) (map[string]interface{}, error) {
			// Mock handler that simulates LLMForwardHandler behavior
			chatID, _ := payload["chat_id"].(string)
			content, _ := payload["content"].(string)

			if chatID == "" {
				return map[string]interface{}{
					"success": false,
					"error":   "chat_id is required",
				}, nil
			}

			if content == "" {
				return map[string]interface{}{
					"success": false,
					"error":   "content is required",
				}, nil
			}

			return map[string]interface{}{
				"success": true,
				"content": "mock response",
			}, nil
		}
	}, registrar)

	// Verify log message was written
	if len(mockCluster.logMessages) == 0 {
		t.Error("Expected log message to be written")
	}

	// Check log message contains expected text
	found := false
	for _, msg := range mockCluster.logMessages {
		if contains(msg, "Registered LLM handlers") {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("Expected log message to contain 'Registered LLM handlers', got: %v", mockCluster.logMessages)
	}
}

// Helper function to check if a string contains a substring
func contains(s, substr string) bool {
	return len(s) >= len(substr) && (s == substr || len(substr) == 0 ||
		(len(s) > 0 && len(substr) > 0 && indexOf(s, substr) >= 0))
}

func indexOf(s, substr string) int {
	for i := 0; i <= len(s)-len(substr); i++ {
		if s[i:i+len(substr)] == substr {
			return i
		}
	}
	return -1
}

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

// TestRegisterPeerChatHandlers tests that peer chat handlers are registered
func TestRegisterPeerChatHandlers(t *testing.T) {
	mockCluster := &mockClusterForHandlers{
		nodeID:       "test-node-1",
		address:      "127.0.0.1:21950",
		capabilities: []string{"peer_chat", "llm"},
		logMessages:  []string{},
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

	// Register peer chat handlers
	handlers.RegisterPeerChatHandlers(mockCluster, rpcCh, func(rpcChannel *channels.RPCChannel) func(map[string]interface{}) (map[string]interface{}, error) {
		return func(payload map[string]interface{}) (map[string]interface{}, error) {
			// Mock handler that simulates PeerChatHandler behavior
			content, _ := payload["content"].(string)

			if content == "" {
				return map[string]interface{}{
					"status":   "error",
					"response": "content is required",
				}, nil
			}

			return map[string]interface{}{
				"status":   "success",
				"response": "mock response",
			}, nil
		}
	}, registrar)

	// Verify peer_chat handler is registered
	if !registeredHandlers["peer_chat"] {
		t.Error("Handler 'peer_chat' was not registered")
	}

	// Verify log message was written
	if len(mockCluster.logMessages) == 0 {
		t.Error("Expected log message to be written")
	}

	// Verify the handler function is registered
	if !registeredHandlers["peer_chat"] {
		t.Fatal("peer_chat handler was not registered")
	}
}

// TestPeerChatHandlerExists tests that the peer chat handler can be created
func TestPeerChatHandlerExists(t *testing.T) {
	mockCluster := &mockClusterForHandlers{
		nodeID:       "test-node-1",
		address:      "127.0.0.1:21950",
		capabilities: []string{"peer_chat", "llm"},
		logMessages:  []string{},
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

	var peerChatHandler func(map[string]interface{}) (map[string]interface{}, error)

	// Capture the peer_chat handler
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if action == "peer_chat" {
			peerChatHandler = handler
		}
	}

	handlers.RegisterPeerChatHandlers(mockCluster, rpcCh, func(rpcChannel *channels.RPCChannel) func(map[string]interface{}) (map[string]interface{}, error) {
		return func(payload map[string]interface{}) (map[string]interface{}, error) {
			// Mock handler that simulates PeerChatHandler behavior
			content, _ := payload["content"].(string)

			if content == "" {
				return map[string]interface{}{
					"status":   "error",
					"response": "content is required",
				}, nil
			}

			return map[string]interface{}{
				"status":   "success",
				"response": "mock response",
			}, nil
		}
	}, registrar)

	if peerChatHandler == nil {
		t.Fatal("peer_chat handler was not registered")
	}
}

// TestPeerChatHandlerBasicCall tests basic call to the handler
func TestPeerChatHandlerBasicCall(t *testing.T) {
	mockCluster := &mockClusterForHandlers{
		nodeID:       "test-node-1",
		address:      "127.0.0.1:21950",
		capabilities: []string{"peer_chat", "llm"},
		logMessages:  []string{},
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

	var peerChatHandler func(map[string]interface{}) (map[string]interface{}, error)

	// Capture the peer_chat handler
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if action == "peer_chat" {
			peerChatHandler = handler
		}
	}

	handlers.RegisterPeerChatHandlers(mockCluster, rpcCh, func(rpcChannel *channels.RPCChannel) func(map[string]interface{}) (map[string]interface{}, error) {
		return func(payload map[string]interface{}) (map[string]interface{}, error) {
			// Mock handler that simulates PeerChatHandler behavior
			content, _ := payload["content"].(string)

			if content == "" {
				return map[string]interface{}{
					"status":   "error",
					"response": "content is required",
				}, nil
			}

			return map[string]interface{}{
				"status":   "success",
				"response": "mock response",
			}, nil
		}
	}, registrar)

	if peerChatHandler == nil {
		t.Fatal("peer_chat handler was not registered")
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
			name: "Missing content",
			payload: map[string]interface{}{
				"type": "chat",
			},
			expectError: false,
			checkKey:    "status",
			checkValue:  "error",
		},
		{
			name: "Valid payload",
			payload: map[string]interface{}{
				"type":    "chat",
				"content": "test message",
			},
			expectError: false,
			checkKey:    "status",
			checkValue:  "success",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			response, err := peerChatHandler(tt.payload)
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

// TestPeerChatHandlerCallStructure tests that the handler has the correct structure
func TestPeerChatHandlerCallStructure(t *testing.T) {
	mockCluster := &mockClusterForHandlers{
		nodeID:       "test-node-1",
		address:      "127.0.0.1:21950",
		capabilities: []string{"peer_chat", "llm"},
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

	var peerChatHandler func(map[string]interface{}) (map[string]interface{}, error)

	// Capture the peer_chat handler
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if action == "peer_chat" {
			peerChatHandler = handler
		}
	}

	handlers.RegisterPeerChatHandlers(mockCluster, rpcCh, func(rpcChannel *channels.RPCChannel) func(map[string]interface{}) (map[string]interface{}, error) {
		return func(payload map[string]interface{}) (map[string]interface{}, error) {
			// Mock handler that simulates PeerChatHandler behavior
			content, _ := payload["content"].(string)

			if content == "" {
				return map[string]interface{}{
					"status":   "error",
					"response": "content is required",
				}, nil
			}

			return map[string]interface{}{
				"status":   "success",
				"response": "mock response",
			}, nil
		}
	}, registrar)

	if peerChatHandler == nil {
		t.Fatal("peer_chat handler was not registered")
	}

	// Test that handler returns proper error structure for missing fields
	payload := map[string]interface{}{
		"type":    "chat",
		"content": "", // Empty content should cause error
	}

	response, err := peerChatHandler(payload)
	if err != nil {
		t.Errorf("Handler returned error instead of error response: %v", err)
	}

	if response == nil {
		t.Fatal("Handler returned nil response")
	}

	if response["status"] != "error" {
		t.Errorf("Expected status=error for missing content, got %v", response["status"])
	}

	if _, ok := response["response"]; !ok {
		t.Error("Expected response field in result for missing content")
	}
}

// TestRegisterPeerChatHandlersLogMessage tests that registration produces a log message
func TestRegisterPeerChatHandlersLogMessage(t *testing.T) {
	mockCluster := &mockClusterForHandlers{
		nodeID:       "test-node-1",
		address:      "127.0.0.1:21950",
		capabilities: []string{"peer_chat", "llm"},
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

	// Register peer chat handlers
	handlers.RegisterPeerChatHandlers(mockCluster, rpcCh, func(rpcChannel *channels.RPCChannel) func(map[string]interface{}) (map[string]interface{}, error) {
		return func(payload map[string]interface{}) (map[string]interface{}, error) {
			// Mock handler that simulates PeerChatHandler behavior
			content, _ := payload["content"].(string)

			if content == "" {
				return map[string]interface{}{
					"status":   "error",
					"response": "content is required",
				}, nil
			}

			return map[string]interface{}{
				"status":   "success",
				"response": "mock response",
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
		if contains(msg, "Registered peer chat handler: peer_chat") {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("Expected log message to contain 'Registered peer chat handler: peer_chat', got: %v", mockCluster.logMessages)
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

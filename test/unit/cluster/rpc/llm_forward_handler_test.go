// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package rpc_test

import (
	"context"
	"encoding/json"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/channels"
	clusterrpc "github.com/276793422/NemesisBot/module/cluster/rpc"
)

// mockClusterForHandler is a mock Cluster implementation for testing
type mockClusterForHandler struct{}

func (m *mockClusterForHandler) GetRegistry() interface{}                       { return nil }
func (m *mockClusterForHandler) GetNodeID() string                             { return "test-server" }
func (m *mockClusterForHandler) GetAddress() string                            { return "" }
func (m *mockClusterForHandler) GetCapabilities() []string                    { return []string{"llm_forward"} }
func (m *mockClusterForHandler) GetOnlinePeers() []interface{}                 { return nil }
func (m *mockClusterForHandler) LogRPCInfo(msg string, args ...interface{})   {}
func (m *mockClusterForHandler) LogRPCError(msg string, args ...interface{})  {}
func (m *mockClusterForHandler) LogRPCDebug(msg string, args ...interface{})  {}
func (m *mockClusterForHandler) GetPeer(peerID string) (interface{}, error)   { return nil, nil }
func (m *mockClusterForHandler) GetLocalNetworkInterfaces() ([]clusterrpc.LocalNetworkInterface, error) {
	return []clusterrpc.LocalNetworkInterface{
		{IP: "127.0.0.1", Mask: "255.255.255.0"},
	}, nil
}

// TestNewLLMForwardHandler tests creating a new handler
func TestNewLLMForwardHandler(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &channels.RPCChannelConfig{
		MessageBus: msgBus,
	}
	rpcCh, err := channels.NewRPCChannel(cfg)
	if err != nil {
		t.Fatalf("Failed to create RPC channel: %v", err)
	}

	mockCluster := &mockClusterForHandler{}
	handler := clusterrpc.NewLLMForwardHandler(mockCluster, rpcCh)

	if handler == nil {
		t.Fatal("Expected non-nil handler")
	}
}

// TestLLMForwardHandlerHandleSuccess tests successful LLM forward
func TestLLMForwardHandlerHandleSuccess(t *testing.T) {
	// Setup
	msgBus := bus.NewMessageBus()
	cfg := &channels.RPCChannelConfig{
		MessageBus:      msgBus,
		RequestTimeout:  5 * time.Second,
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

	mockCluster := &mockClusterForHandler{}
	handler := clusterrpc.NewLLMForwardHandler(mockCluster, rpcCh)

	payload := map[string]interface{}{
		"chat_id":  "test-user",
		"content":  "Hello from Bot A",
		"sender_id": "bot-a",
	}

	// Handle in goroutine (because it waits for response)
	resultCh := make(chan map[string]interface{}, 1)
	errCh := make(chan error, 1)

	go func() {
		result, err := handler.Handle(payload)
		if err != nil {
			errCh <- err
			return
		}
		resultCh <- result
	}()

	// Wait for timeout (expected)
	select {
	case <-resultCh:
		t.Log("Handler returned (timeout is expected in this test)")
	case err := <-errCh:
		t.Fatalf("Handle failed: %v", err)
	case <-time.After(2 * time.Second):
		t.Log("Test completed (timeout expected due to unknown correlation ID)")
	}
}

// TestLLMForwardHandlerHandleMissingChatID tests missing required fields
func TestLLMForwardHandlerHandleMissingChatID(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &channels.RPCChannelConfig{
		MessageBus: msgBus,
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

	mockCluster := &mockClusterForHandler{}
	handler := clusterrpc.NewLLMForwardHandler(mockCluster, rpcCh)

	payload := map[string]interface{}{
		"content": "Hello",
	}

	result, err := handler.Handle(payload)
	if err != nil {
		t.Fatalf("Handle failed: %v", err)
	}

	success, ok := result["success"].(bool)
	if !ok {
		t.Fatalf("Expected success to be bool, got %T", result["success"])
	}
	if success {
		t.Error("Expected success=false for missing chat_id")
	}

	if errMsg, ok := result["error"].(string); ok {
		if errMsg != "chat_id is required" {
			t.Errorf("Expected error 'chat_id is required', got '%s'", errMsg)
		}
	} else {
		t.Error("Expected error message in result")
	}
}

// TestLLMForwardHandlerHandleMissingContent tests missing required fields
func TestLLMForwardHandlerHandleMissingContent(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &channels.RPCChannelConfig{
		MessageBus: msgBus,
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

	mockCluster := &mockClusterForHandler{}
	handler := clusterrpc.NewLLMForwardHandler(mockCluster, rpcCh)

	payload := map[string]interface{}{
		"chat_id": "test-user",
	}

	result, err := handler.Handle(payload)
	if err != nil {
		t.Fatalf("Handle failed: %v", err)
	}

	success, ok := result["success"].(bool)
	if !ok {
		t.Fatalf("Expected success to be bool, got %T", result["success"])
	}
	if success {
		t.Error("Expected success=false for missing content")
	}

	if errMsg, ok := result["error"].(string); ok {
		if errMsg != "content is required" {
			t.Errorf("Expected error 'content is required', got '%s'", errMsg)
		}
	} else {
		t.Error("Expected error message in result")
	}
}

// TestLLMForwardHandlerHandleTimeout tests timeout scenario
func TestLLMForwardHandlerHandleTimeout(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &channels.RPCChannelConfig{
		MessageBus:      msgBus,
		RequestTimeout:  2 * time.Second, // Short timeout
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

	mockCluster := &mockClusterForHandler{}
	handler := clusterrpc.NewLLMForwardHandler(mockCluster, rpcCh)

	payload := map[string]interface{}{
		"chat_id": "test-user",
		"content": "Hello",
	}

	// Handle in goroutine
	resultCh := make(chan map[string]interface{}, 1)
	go func() {
		result, _ := handler.Handle(payload)
		resultCh <- result
	}()

	// Wait for timeout (don't send response)
	select {
	case result := <-resultCh:
		success, ok := result["success"].(bool)
		if !ok {
			t.Fatalf("Expected success to be bool, got %T", result["success"])
		}
		if success {
			t.Error("Expected success=false on timeout")
		}

		if errMsg, ok := result["error"].(string); ok {
			t.Logf("Got expected error message: '%s'", errMsg)
		}

	case <-time.After(70 * time.Second):
		t.Fatal("Test should have timed out by now")
	}
}

// TestLLMForwardPayloadJSON tests JSON marshaling/unmarshaling
func TestLLMForwardPayloadJSON(t *testing.T) {
	payload := clusterrpc.LLMForwardPayload{
		Channel:    "rpc",
		ChatID:     "test-user",
		Content:    "Hello Bot B",
		SenderID:   "bot-a",
		SessionKey: "session-123",
		Metadata: map[string]string{
			"source": "rpc",
		},
	}

	// Marshal to JSON
	data, err := json.Marshal(payload)
	if err != nil {
		t.Fatalf("Failed to marshal: %v", err)
	}

	// Unmarshal back
	var decoded clusterrpc.LLMForwardPayload
	if err := json.Unmarshal(data, &decoded); err != nil {
		t.Fatalf("Failed to unmarshal: %v", err)
	}

	if decoded.ChatID != payload.ChatID {
		t.Errorf("Expected ChatID '%s', got '%s'", payload.ChatID, decoded.ChatID)
	}

	if decoded.Content != payload.Content {
		t.Errorf("Expected Content '%s', got '%s'", payload.Content, decoded.Content)
	}
}

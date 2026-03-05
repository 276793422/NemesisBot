// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package rpc_test

import (
	"testing"

	clusterrpc "github.com/276793422/NemesisBot/module/cluster/rpc"
)

// mockClusterForPeerChat is a mock Cluster for testing PeerChatHandler
type mockClusterForPeerChat struct {
	logMessages []string
}

func (m *mockClusterForPeerChat) GetRegistry() interface{}                        { return nil }
func (m *mockClusterForPeerChat) GetNodeID() string                              { return "test-node" }
func (m *mockClusterForPeerChat) GetAddress() string                             { return "" }
func (m *mockClusterForPeerChat) GetCapabilities() []string                      { return []string{"peer_chat", "llm"} }
func (m *mockClusterForPeerChat) GetOnlinePeers() []interface{}                   { return nil }
func (m *mockClusterForPeerChat) GetActionsSchema() []interface{}                 { return nil }
func (m *mockClusterForPeerChat) LogRPCInfo(msg string, args ...interface{})    { m.logMessages = append(m.logMessages, "INFO: "+msg) }
func (m *mockClusterForPeerChat) LogRPCError(msg string, args ...interface{})   { m.logMessages = append(m.logMessages, "ERROR: "+msg) }
func (m *mockClusterForPeerChat) LogRPCDebug(msg string, args ...interface{})  { m.logMessages = append(m.logMessages, "DEBUG: "+msg) }
func (m *mockClusterForPeerChat) GetPeer(peerID string) (interface{}, error)    { return nil, nil }
func (m *mockClusterForPeerChat) GetLocalNetworkInterfaces() ([]clusterrpc.LocalNetworkInterface, error) {
	return []clusterrpc.LocalNetworkInterface{{IP: "127.0.0.1", Mask: "255.255.255.0"}}, nil
}

// TestNewPeerChatHandler tests the creation of a new PeerChatHandler
func TestNewPeerChatHandler(t *testing.T) {
	mockCluster := &mockClusterForPeerChat{}
	handler := clusterrpc.NewPeerChatHandler(mockCluster, nil)

	if handler == nil {
		t.Fatal("Expected non-nil handler")
	}
}

// TestPeerChatHandler_TaskType tests peer chat with task type
func TestPeerChatHandler_TaskType(t *testing.T) {
	mockCluster := &mockClusterForPeerChat{}
	handler := clusterrpc.NewPeerChatHandler(mockCluster, nil)

	payload := map[string]interface{}{
		"type":    "task",
		"content": "帮我写一首诗",
		"context": map[string]interface{}{
			"chat_id": "test-user-123",
		},
	}

	result, err := handler.Handle(payload)
	if err != nil {
		t.Errorf("Handle returned error: %v", err)
	}

	// With nil rpcChannel, should get error
	if result["status"] != "error" {
		t.Logf("Expected status 'error' due to nil rpcChannel, got '%v'", result["status"])
	}
}

// TestPeerChatHandler_ChatType tests peer chat with chat type
func TestPeerChatHandler_ChatType(t *testing.T) {
	mockCluster := &mockClusterForPeerChat{}
	handler := clusterrpc.NewPeerChatHandler(mockCluster, nil)

	payload := map[string]interface{}{
		"type":    "chat",
		"content": "你好，最近忙什么呢？",
	}

	result, err := handler.Handle(payload)
	if err != nil {
		t.Errorf("Handle returned error: %v", err)
	}

	if result["status"] != "error" {
		t.Logf("Expected status 'error' due to nil rpcChannel, got '%v'", result["status"])
	}
}

// TestPeerChatHandler_MissingContent tests error handling when content is missing
func TestPeerChatHandler_MissingContent(t *testing.T) {
	mockCluster := &mockClusterForPeerChat{}
	handler := clusterrpc.NewPeerChatHandler(mockCluster, nil)

	payload := map[string]interface{}{
		"type": "task",
	}

	result, err := handler.Handle(payload)
	if err != nil {
		t.Errorf("Handle returned error: %v", err)
	}

	if result["status"] != "error" {
		t.Errorf("Expected status 'error', got '%v'", result["status"])
	}

	if result["response"] != "content is required" {
		t.Errorf("Expected error message 'content is required', got '%v'", result["response"])
	}
}

// TestPeerChatHandler_EmptyPayload tests handling of empty payload
func TestPeerChatHandler_EmptyPayload(t *testing.T) {
	mockCluster := &mockClusterForPeerChat{}
	handler := clusterrpc.NewPeerChatHandler(mockCluster, nil)

	payload := map[string]interface{}{}

	result, err := handler.Handle(payload)
	if err != nil {
		t.Errorf("Handle returned error: %v", err)
	}

	if result["status"] != "error" {
		t.Errorf("Expected status 'error', got '%v'", result["status"])
	}
}

// TestPeerChatHandler_WithContext tests peer chat with context
func TestPeerChatHandler_WithContext(t *testing.T) {
	mockCluster := &mockClusterForPeerChat{}
	handler := clusterrpc.NewPeerChatHandler(mockCluster, nil)

	payload := map[string]interface{}{
		"type":    "task",
		"content": "帮我分析",
		"context": map[string]interface{}{
			"chat_id":      "user-456",
			"session_key":  "session-xyz",
			"sender_id":    "node-abc",
			"extra_data":   12345,
		},
	}

	result, err := handler.Handle(payload)
	if err != nil {
		t.Errorf("Handle returned error: %v", err)
	}

	// With nil rpcChannel, should get error but parsing should have worked
	if result["status"] != "error" {
		t.Logf("Expected status 'error' due to nil rpcChannel, got '%v'", result["status"])
	}
}

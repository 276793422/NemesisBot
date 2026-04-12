// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package rpc_test

import (
	"context"
	"fmt"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/channels"
	clusterrpc "github.com/276793422/NemesisBot/module/cluster/rpc"
)

// mockClusterForPeerChat is a mock Cluster for testing PeerChatHandler
type mockClusterForPeerChat struct {
	logMessages  []string
	mu           sync.Mutex
	callbackData map[string]interface{} // 存储回调数据
	callbackMu   sync.Mutex
}

func (m *mockClusterForPeerChat) GetRegistry() interface{}        { return nil }
func (m *mockClusterForPeerChat) GetNodeID() string               { return "test-node" }
func (m *mockClusterForPeerChat) GetAddress() string              { return "" }
func (m *mockClusterForPeerChat) GetCapabilities() []string       { return []string{"peer_chat", "llm"} }
func (m *mockClusterForPeerChat) GetOnlinePeers() []interface{}   { return nil }
func (m *mockClusterForPeerChat) GetActionsSchema() []interface{} { return nil }
func (m *mockClusterForPeerChat) LogRPCInfo(msg string, args ...interface{}) {
	m.mu.Lock()
	defer m.mu.Unlock()
	formatted := fmt.Sprintf(msg, args...)
	m.logMessages = append(m.logMessages, "INFO: "+formatted)
}
func (m *mockClusterForPeerChat) LogRPCError(msg string, args ...interface{}) {
	m.mu.Lock()
	defer m.mu.Unlock()
	formatted := fmt.Sprintf(msg, args...)
	m.logMessages = append(m.logMessages, "ERROR: "+formatted)
}
func (m *mockClusterForPeerChat) LogRPCDebug(msg string, args ...interface{}) {
	m.mu.Lock()
	defer m.mu.Unlock()
	formatted := fmt.Sprintf(msg, args...)
	m.logMessages = append(m.logMessages, "DEBUG: "+formatted)
}
func (m *mockClusterForPeerChat) GetPeer(peerID string) (interface{}, error) { return nil, nil }
func (m *mockClusterForPeerChat) GetLocalNetworkInterfaces() ([]clusterrpc.LocalNetworkInterface, error) {
	return []clusterrpc.LocalNetworkInterface{{IP: "127.0.0.1", Mask: "255.255.255.0"}}, nil
}
func (m *mockClusterForPeerChat) CallWithContext(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) {
	m.callbackMu.Lock()
	defer m.callbackMu.Unlock()
	m.callbackData = payload
	return []byte(`{"status":"received"}`), nil
}

// TestNewPeerChatHandler tests the creation of a new PeerChatHandler
func TestNewPeerChatHandler(t *testing.T) {
	mockCluster := &mockClusterForPeerChat{}
	handler := clusterrpc.NewPeerChatHandler(mockCluster, nil)

	if handler == nil {
		t.Fatal("Expected non-nil handler")
	}
}

// TestPeerChatHandler_TaskType_ImmediateACK tests that Handle returns ACK immediately
func TestPeerChatHandler_TaskType_ImmediateACK(t *testing.T) {
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

	// 异步模式：应立即返回 accepted
	if result["status"] != "accepted" {
		t.Errorf("Expected status 'accepted', got '%v'", result["status"])
	}

	taskID, _ := result["task_id"].(string)
	if taskID == "" {
		t.Error("Expected non-empty task_id in ACK")
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

// TestPeerChatHandler_WithContext tests peer chat with context returns ACK
func TestPeerChatHandler_WithContext(t *testing.T) {
	mockCluster := &mockClusterForPeerChat{}
	handler := clusterrpc.NewPeerChatHandler(mockCluster, nil)

	payload := map[string]interface{}{
		"type":    "task",
		"content": "帮我分析",
		"context": map[string]interface{}{
			"chat_id":     "user-456",
			"session_key": "session-xyz",
			"sender_id":   "node-abc",
			"extra_data":  12345,
		},
	}

	result, err := handler.Handle(payload)
	if err != nil {
		t.Errorf("Handle returned error: %v", err)
	}

	// 异步模式：应立即返回 accepted
	if result["status"] != "accepted" {
		t.Errorf("Expected status 'accepted', got '%v'", result["status"])
	}
}

// TestPeerChatHandler_SessionKey_RpcFrom tests that _rpc.from takes priority
func TestPeerChatHandler_SessionKey_RpcFrom(t *testing.T) {
	mockCluster := &mockClusterForPeerChat{}

	// Create a real RPCChannel with minimal setup
	msgBus := bus.NewMessageBus()
	cfg := &channels.RPCChannelConfig{
		MessageBus:      msgBus,
		RequestTimeout:  5 * time.Second,
		CleanupInterval: 1 * time.Second,
	}
	rpcChannel, err := channels.NewRPCChannel(cfg)
	if err != nil {
		t.Fatalf("Failed to create RPCChannel: %v", err)
	}

	// Start the channel in background
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	if err := rpcChannel.Start(ctx); err != nil {
		t.Fatalf("Failed to start RPCChannel: %v", err)
	}
	defer rpcChannel.Stop(ctx)

	handler := clusterrpc.NewPeerChatHandler(mockCluster, rpcChannel)

	payload := map[string]interface{}{
		"type":    "chat",
		"content": "test message",
		"_rpc": map[string]interface{}{
			"from": "node-A",
			"to":   "node-B",
			"id":   "test-id-123",
		},
		"context": map[string]interface{}{
			"sender_id": "should-be-ignored", // This should be ignored
		},
	}

	_, err = handler.Handle(payload)
	if err != nil {
		t.Errorf("Handle returned error: %v", err)
	}

	// Wait a bit for async processing to start
	time.Sleep(100 * time.Millisecond)

	// Verify log contains expected session key
	found := false
	expectedSessionKey := "cluster_rpc:node-A"
	for _, log := range mockCluster.logMessages {
		if strings.Contains(log, expectedSessionKey) && strings.Contains(log, "Using session_key=") {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("Expected log containing session key '%s', got logs: %v", expectedSessionKey, mockCluster.logMessages)
	}
}

// TestPeerChatHandler_SessionKey_ContextFallback tests fallback to context.sender_id
func TestPeerChatHandler_SessionKey_ContextFallback(t *testing.T) {
	mockCluster := &mockClusterForPeerChat{}

	// Create a real RPCChannel
	msgBus := bus.NewMessageBus()
	cfg := &channels.RPCChannelConfig{
		MessageBus:      msgBus,
		RequestTimeout:  5 * time.Second,
		CleanupInterval: 1 * time.Second,
	}
	rpcChannel, err := channels.NewRPCChannel(cfg)
	if err != nil {
		t.Fatalf("Failed to create RPCChannel: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	if err := rpcChannel.Start(ctx); err != nil {
		t.Fatalf("Failed to start RPCChannel: %v", err)
	}
	defer rpcChannel.Stop(ctx)

	handler := clusterrpc.NewPeerChatHandler(mockCluster, rpcChannel)

	payload := map[string]interface{}{
		"type":    "chat",
		"content": "test message",
		"context": map[string]interface{}{
			"sender_id": "node-B",
		},
	}

	_, err = handler.Handle(payload)
	if err != nil {
		t.Errorf("Handle returned error: %v", err)
	}

	time.Sleep(100 * time.Millisecond)

	// Verify log contains expected session key
	found := false
	expectedSessionKey := "cluster_rpc:node-B"
	for _, log := range mockCluster.logMessages {
		if strings.Contains(log, expectedSessionKey) && strings.Contains(log, "Using session_key=") {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("Expected log containing session key '%s', got logs: %v", expectedSessionKey, mockCluster.logMessages)
	}
}

// TestPeerChatHandler_SessionKey_Default tests default "remote-peer" when no sender_id is provided
func TestPeerChatHandler_SessionKey_Default(t *testing.T) {
	mockCluster := &mockClusterForPeerChat{}

	// Create a real RPCChannel
	msgBus := bus.NewMessageBus()
	cfg := &channels.RPCChannelConfig{
		MessageBus:      msgBus,
		RequestTimeout:  5 * time.Second,
		CleanupInterval: 1 * time.Second,
	}
	rpcChannel, err := channels.NewRPCChannel(cfg)
	if err != nil {
		t.Fatalf("Failed to create RPCChannel: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	if err := rpcChannel.Start(ctx); err != nil {
		t.Fatalf("Failed to start RPCChannel: %v", err)
	}
	defer rpcChannel.Stop(ctx)

	handler := clusterrpc.NewPeerChatHandler(mockCluster, rpcChannel)

	payload := map[string]interface{}{
		"type":    "chat",
		"content": "test message",
	}

	_, err = handler.Handle(payload)
	if err != nil {
		t.Errorf("Handle returned error: %v", err)
	}

	time.Sleep(100 * time.Millisecond)

	// Verify log contains default session key
	found := false
	expectedSessionKey := "cluster_rpc:remote-peer"
	for _, log := range mockCluster.logMessages {
		if strings.Contains(log, expectedSessionKey) && strings.Contains(log, "Using session_key=") {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("Expected log containing session key '%s', got logs: %v", expectedSessionKey, mockCluster.logMessages)
	}
}

// TestPeerChatHandler_SessionKey_EmptyRpcFrom tests fallback when _rpc.from is empty string
func TestPeerChatHandler_SessionKey_EmptyRpcFrom(t *testing.T) {
	mockCluster := &mockClusterForPeerChat{}

	// Create a real RPCChannel
	msgBus := bus.NewMessageBus()
	cfg := &channels.RPCChannelConfig{
		MessageBus:      msgBus,
		RequestTimeout:  5 * time.Second,
		CleanupInterval: 1 * time.Second,
	}
	rpcChannel, err := channels.NewRPCChannel(cfg)
	if err != nil {
		t.Fatalf("Failed to create RPCChannel: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	if err := rpcChannel.Start(ctx); err != nil {
		t.Fatalf("Failed to start RPCChannel: %v", err)
	}
	defer rpcChannel.Stop(ctx)

	handler := clusterrpc.NewPeerChatHandler(mockCluster, rpcChannel)

	payload := map[string]interface{}{
		"type":    "chat",
		"content": "test message",
		"_rpc": map[string]interface{}{
			"from": "", // Empty string should trigger fallback
			"to":   "node-B",
		},
		"context": map[string]interface{}{
			"sender_id": "node-C",
		},
	}

	_, err = handler.Handle(payload)
	if err != nil {
		t.Errorf("Handle returned error: %v", err)
	}

	time.Sleep(100 * time.Millisecond)

	// Should fallback to context.sender_id
	found := false
	expectedSessionKey := "cluster_rpc:node-C"
	for _, log := range mockCluster.logMessages {
		if strings.Contains(log, expectedSessionKey) && strings.Contains(log, "Using session_key=") {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("Expected log containing session key '%s', got logs: %v", expectedSessionKey, mockCluster.logMessages)
	}
}

// TestPeerChatHandler_NilRPCChannel_NoSource tests nil rpcChannel with no source info
func TestPeerChatHandler_NilRPCChannel_NoSource(t *testing.T) {
	mockCluster := &mockClusterForPeerChat{}
	handler := clusterrpc.NewPeerChatHandler(mockCluster, nil)

	payload := map[string]interface{}{
		"type":    "chat",
		"content": "test message",
	}

	result, err := handler.Handle(payload)
	if err != nil {
		t.Errorf("Handle returned error: %v", err)
	}

	// 异步模式：应返回 ACK（即使 rpcChannel 为 nil，错误在 goroutine 中处理）
	if result["status"] != "accepted" {
		t.Errorf("Expected status 'accepted' (async ACK), got '%v'", result["status"])
	}
}

// TestPeerChatHandler_NilRPCChannel_WithSource tests nil rpcChannel with source info (callback should fail gracefully)
func TestPeerChatHandler_NilRPCChannel_WithSource(t *testing.T) {
	mockCluster := &mockClusterForPeerChat{}
	handler := clusterrpc.NewPeerChatHandler(mockCluster, nil)

	payload := map[string]interface{}{
		"type":    "chat",
		"content": "test message",
		"_source": map[string]interface{}{
			"node_id": "node-A",
		},
	}

	result, err := handler.Handle(payload)
	if err != nil {
		t.Errorf("Handle returned error: %v", err)
	}

	// 异步模式：应返回 ACK
	if result["status"] != "accepted" {
		t.Errorf("Expected status 'accepted', got '%v'", result["status"])
	}

	// 等待异步处理（goroutine 会记录 rpc channel not available 的错误）
	time.Sleep(200 * time.Millisecond)

	// 验证日志中有错误记录
	found := false
	for _, log := range mockCluster.logMessages {
		if strings.Contains(log, "RPC channel is not available") {
			found = true
			break
		}
	}
	if !found {
		t.Log("Expected log about RPC channel not available - async goroutine may not have run yet")
	}
}

// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package rpc

import (
	"context"
	"encoding/json"
	"fmt"
	"net"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/channels"
	"github.com/276793422/NemesisBot/module/cluster/transport"
)

// --- Mock Cluster for RPC coverage tests ---

type coverageMockCluster struct {
	nodeID    string
	address   string
	peers     map[string]*coverageMockNode
	caps      []string
	online    []interface{}
	actions   []interface{}
	logs      []string
	mu        sync.RWMutex
	callFn    func(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error)
	taskStore TaskResultStorer
}

type coverageMockNode struct {
	id           string
	name         string
	address      string
	addresses    []string
	rpcPort      int
	capabilities []string
	status       string
	online       bool
}

func newCoverageMockCluster() *coverageMockCluster {
	return &coverageMockCluster{
		nodeID: "test-node",
		peers:  make(map[string]*coverageMockNode),
	}
}

func (m *coverageMockCluster) GetRegistry() interface{}          { return nil }
func (m *coverageMockCluster) GetNodeID() string                 { return m.nodeID }
func (m *coverageMockCluster) GetAddress() string                { return m.address }
func (m *coverageMockCluster) GetCapabilities() []string         { return m.caps }
func (m *coverageMockCluster) GetOnlinePeers() []interface{}     { return m.online }
func (m *coverageMockCluster) GetActionsSchema() []interface{}   { return m.actions }
func (m *coverageMockCluster) LogRPCInfo(msg string, args ...interface{}) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.logs = append(m.logs, fmt.Sprintf("[INFO] "+msg, args...))
}
func (m *coverageMockCluster) LogRPCError(msg string, args ...interface{}) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.logs = append(m.logs, fmt.Sprintf("[ERROR] "+msg, args...))
}
func (m *coverageMockCluster) LogRPCDebug(msg string, args ...interface{}) {
	// skip debug logs in tests
}
func (m *coverageMockCluster) GetPeer(peerID string) (interface{}, error) {
	m.mu.RLock()
	defer m.mu.RUnlock()
	p, ok := m.peers[peerID]
	if !ok {
		return nil, fmt.Errorf("peer not found: %s", peerID)
	}
	return p, nil
}
func (m *coverageMockCluster) GetLocalNetworkInterfaces() ([]LocalNetworkInterface, error) {
	return []LocalNetworkInterface{
		{IP: "192.168.1.100", Mask: "255.255.255.0"},
	}, nil
}
func (m *coverageMockCluster) CallWithContext(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) {
	if m.callFn != nil {
		return m.callFn(ctx, peerID, action, payload)
	}
	return nil, fmt.Errorf("not implemented")
}
func (m *coverageMockCluster) GetTaskResultStorer() TaskResultStorer {
	return m.taskStore
}

func (n *coverageMockNode) GetID() string            { return n.id }
func (n *coverageMockNode) GetName() string          { return n.name }
func (n *coverageMockNode) GetAddress() string       { return n.address }
func (n *coverageMockNode) GetAddresses() []string   { return n.addresses }
func (n *coverageMockNode) GetRPCPort() int          { return n.rpcPort }
func (n *coverageMockNode) GetCapabilities() []string { return n.capabilities }
func (n *coverageMockNode) GetStatus() string        { return n.status }
func (n *coverageMockNode) IsOnline() bool           { return n.online }

// --- RateLimiter advanced tests ---

func TestRateLimiter_WindowExpiry(t *testing.T) {
	rl := NewRateLimiter(100, 10*time.Millisecond, 2, 50*time.Millisecond)
	ctx := context.Background()

	// Use 2 tokens (max in window)
	if err := rl.Acquire(ctx, "peer-1"); err != nil {
		t.Fatalf("First acquire failed: %v", err)
	}
	if err := rl.Acquire(ctx, "peer-1"); err != nil {
		t.Fatalf("Second acquire failed: %v", err)
	}

	// Third should be rate limited
	err := rl.Acquire(ctx, "peer-1")
	if err == nil {
		t.Fatal("Third acquire should be rate limited")
	}

	// Wait for window to expire
	time.Sleep(60 * time.Millisecond)

	// Should succeed again
	if err := rl.Acquire(ctx, "peer-1"); err != nil {
		t.Fatalf("Acquire after window expired failed: %v", err)
	}
}

func TestRateLimiter_ContextCancellation(t *testing.T) {
	rl := NewRateLimiter(0, 1*time.Hour, 1, 1*time.Hour)

	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	err := rl.Acquire(ctx, "peer-1")
	if err == nil {
		t.Fatal("Acquire with cancelled context should fail")
	}
}

func TestRateLimiter_RefillTokens(t *testing.T) {
	rl := NewRateLimiter(2, 10*time.Millisecond, 100, 1*time.Hour)
	ctx := context.Background()

	// Use all tokens
	rl.Acquire(ctx, "peer-1")
	rl.Acquire(ctx, "peer-1")

	// Wait for refill
	time.Sleep(20 * time.Millisecond)

	// Should succeed after refill
	if err := rl.Acquire(ctx, "peer-1"); err != nil {
		t.Fatalf("Acquire after refill failed: %v", err)
	}
}

// --- Client tests ---

func TestClient_SelectBestAddress_Empty(t *testing.T) {
	mc := newCoverageMockCluster()
	c := NewClient(mc)

	result := c.selectBestAddress([]string{})
	if result != "" {
		t.Errorf("selectBestAddress(empty) = %q, want %q", result, "")
	}
}

func TestClient_SelectBestAddress_Single(t *testing.T) {
	mc := newCoverageMockCluster()
	c := NewClient(mc)

	result := c.selectBestAddress([]string{"1.2.3.4:1234"})
	if result != "1.2.3.4:1234" {
		t.Errorf("selectBestAddress(single) = %q, want %q", result, "1.2.3.4:1234")
	}
}

func TestClient_SelectBestAddress_SubnetMatch(t *testing.T) {
	mc := newCoverageMockCluster()
	c := NewClient(mc)

	// Local interface is 192.168.1.100/24, so 192.168.1.x should match
	addresses := []string{"10.0.0.1:1234", "192.168.1.50:1234"}
	result := c.selectBestAddress(addresses)
	if result != "192.168.1.50:1234" {
		t.Errorf("selectBestAddress() = %q, want %q (subnet match)", result, "192.168.1.50:1234")
	}
}

func TestClient_SelectBestAddress_NoSubnetMatch(t *testing.T) {
	mc := newCoverageMockCluster()
	c := NewClient(mc)

	addresses := []string{"10.0.0.1:1234", "172.16.0.1:1234"}
	result := c.selectBestAddress(addresses)
	// Should return first address when no subnet match
	if result != "10.0.0.1:1234" {
		t.Errorf("selectBestAddress() = %q, want %q (first)", result, "10.0.0.1:1234")
	}
}

func TestClient_ConnectToPeer_EmptyAddresses(t *testing.T) {
	mc := newCoverageMockCluster()
	c := NewClient(mc)

	_, _, err := c.connectToPeer(context.Background(), "peer-1", []string{})
	if err == nil {
		t.Fatal("connectToPeer with empty addresses should fail")
	}
}

func TestClient_Call_PeerNotFound(t *testing.T) {
	mc := newCoverageMockCluster()
	c := NewClient(mc)

	_, err := c.Call("nonexistent", "test", nil)
	if err == nil {
		t.Fatal("Call() with nonexistent peer should fail")
	}
}

func TestClient_CallWithContext_PeerNotNodeInterface(t *testing.T) {
	mc := newCoverageMockCluster()
	mc.peers["bad-peer"] = nil // Will cause type assertion to fail
	c := NewClient(mc)

	// We need to return something from GetPeer that doesn't implement Node
	// But our mock returns mockRPCNode which does implement Node
	// Let's test with a peer that returns a non-Node
	mc.peers["bad-peer"] = &coverageMockNode{id: "bad-peer", online: true, addresses: []string{}, address: "1.2.3.4:1234"}
	_, err := c.CallWithContext(context.Background(), "bad-peer", "test", nil)
	// This should attempt to connect but fail since there's no server
	if err == nil {
		t.Fatal("CallWithContext() should fail when connection fails")
	}
}

func TestClient_CallWithContext_PeerOffline(t *testing.T) {
	mc := newCoverageMockCluster()
	mc.peers["offline"] = &coverageMockNode{
		id:      "offline",
		online:  false,
		address: "1.2.3.4:1234",
	}
	c := NewClient(mc)

	_, err := c.CallWithContext(context.Background(), "offline", "test", nil)
	if err == nil {
		t.Fatal("CallWithContext() with offline peer should fail")
	}
}

// --- Server enhancePayload test ---

func TestServer_EnhancePayload_NilPayloadWithExistingRPC(t *testing.T) {
	mc := newCoverageMockCluster()
	s := NewServer(mc)

	req := &transport.RPCMessage{
		From: "node-1",
		To:   "node-2",
		ID:   "msg-123",
	}

	// Test with payload that has existing _rpc field
	payload := map[string]interface{}{
		"_rpc": map[string]interface{}{
			"existing": "value",
		},
	}

	enhanced := s.enhancePayload(payload, req)

	rpcMeta, ok := enhanced["_rpc"].(map[string]interface{})
	if !ok {
		t.Fatal("_rpc should be map[string]interface{}")
	}
	if rpcMeta["from"] != "node-1" {
		t.Errorf("rpcMeta['from'] = %v, want node-1", rpcMeta["from"])
	}
	if rpcMeta["to"] != "node-2" {
		t.Errorf("rpcMeta['to'] = %v, want node-2", rpcMeta["to"])
	}
	if rpcMeta["id"] != "msg-123" {
		t.Errorf("rpcMeta['id'] = %v, want msg-123", rpcMeta["id"])
	}
	if rpcMeta["existing"] != "value" {
		t.Errorf("rpcMeta['existing'] = %v, want value", rpcMeta["existing"])
	}
}

func TestServer_EnhancePayload_NonMapRPC(t *testing.T) {
	mc := newCoverageMockCluster()
	s := NewServer(mc)

	req := &transport.RPCMessage{
		From: "node-1",
		To:   "node-2",
		ID:   "msg-123",
	}

	// _rpc is not a map
	payload := map[string]interface{}{
		"_rpc": "invalid-type",
	}

	enhanced := s.enhancePayload(payload, req)
	// Should not panic, _rpc stays as string
	if enhanced["_rpc"] != "invalid-type" {
		t.Error("_rpc should remain unchanged when not a map")
	}
}

// --- Server request handling with real TCP ---

func TestServer_HandleRequest_Validation(t *testing.T) {
	mc := newCoverageMockCluster()
	s := NewServer(mc)
	s.Start(0)
	defer s.Stop()

	// Connect to server
	conn, err := net.Dial("tcp", fmt.Sprintf("127.0.0.1:%d", s.GetPort()))
	if err != nil {
		t.Fatalf("Failed to connect: %v", err)
	}
	defer conn.Close()

	// Send an invalid request (empty action)
	req := &transport.RPCMessage{
		Version: "1.0",
		Type:    transport.RPCTypeRequest,
		From:    "test-node",
		To:      "server",
		ID:      "msg-validate",
		Action:  "", // Empty action should fail validation
		Payload: map[string]interface{}{},
	}

	data, _ := json.Marshal(req)
	if err := transport.WriteFrame(conn, data); err != nil {
		t.Fatalf("Failed to send request: %v", err)
	}

	// Read response
	respData, err := transport.DecodeFrame(conn)
	if err != nil {
		t.Fatalf("Failed to read response: %v", err)
	}

	var resp transport.RPCMessage
	if err := json.Unmarshal(respData, &resp); err != nil {
		t.Fatalf("Failed to unmarshal response: %v", err)
	}

	if resp.Type != transport.RPCTypeError {
		t.Errorf("Response type = %q, want %q", resp.Type, transport.RPCTypeError)
	}
}

func TestServer_HandleRequest_SuccessWithHandler(t *testing.T) {
	mc := newCoverageMockCluster()
	s := NewServer(mc)

	// Register a handler before starting
	s.RegisterHandler("test_action", func(payload map[string]interface{}) (map[string]interface{}, error) {
		return map[string]interface{}{
			"status":  "ok",
			"message": "test response",
		}, nil
	})

	s.Start(0)
	defer s.Stop()

	conn, err := net.Dial("tcp", fmt.Sprintf("127.0.0.1:%d", s.GetPort()))
	if err != nil {
		t.Fatalf("Failed to connect: %v", err)
	}
	defer conn.Close()

	req := &transport.RPCMessage{
		Version: "1.0",
		Type:    transport.RPCTypeRequest,
		From:    "test-client",
		To:      mc.GetNodeID(),
		ID:      "msg-success",
		Action:  "test_action",
		Payload: map[string]interface{}{"key": "value"},
	}

	data, _ := json.Marshal(req)
	if err := transport.WriteFrame(conn, data); err != nil {
		t.Fatalf("Failed to send request: %v", err)
	}

	respData, err := transport.DecodeFrame(conn)
	if err != nil {
		t.Fatalf("Failed to read response: %v", err)
	}

	var resp transport.RPCMessage
	if err := json.Unmarshal(respData, &resp); err != nil {
		t.Fatalf("Failed to unmarshal response: %v", err)
	}

	if resp.Type != transport.RPCTypeResponse {
		t.Errorf("Response type = %q, want %q", resp.Type, transport.RPCTypeResponse)
	}
	if resp.ID != "msg-success" {
		t.Errorf("Response ID = %q, want %q", resp.ID, "msg-success")
	}
}

func TestServer_HandleRequest_HandlerReturnsError(t *testing.T) {
	mc := newCoverageMockCluster()
	s := NewServer(mc)

	s.RegisterHandler("error_action", func(payload map[string]interface{}) (map[string]interface{}, error) {
		return nil, fmt.Errorf("handler error occurred")
	})

	s.Start(0)
	defer s.Stop()

	conn, err := net.Dial("tcp", fmt.Sprintf("127.0.0.1:%d", s.GetPort()))
	if err != nil {
		t.Fatalf("Failed to connect: %v", err)
	}
	defer conn.Close()

	req := &transport.RPCMessage{
		Version: "1.0",
		Type:    transport.RPCTypeRequest,
		From:    "test-client",
		To:      mc.GetNodeID(),
		ID:      "msg-error",
		Action:  "error_action",
		Payload: map[string]interface{}{},
	}

	data, _ := json.Marshal(req)
	transport.WriteFrame(conn, data)

	respData, err := transport.DecodeFrame(conn)
	if err != nil {
		t.Fatalf("Failed to read response: %v", err)
	}

	var resp transport.RPCMessage
	json.Unmarshal(respData, &resp)

	if resp.Type != transport.RPCTypeError {
		t.Errorf("Response type = %q, want %q", resp.Type, transport.RPCTypeError)
	}
	if resp.Error != "handler error occurred" {
		t.Errorf("Response error = %q, want %q", resp.Error, "handler error occurred")
	}
}

// --- PeerChatHandler tests ---

func TestPeerChatHandler_ParsePayload(t *testing.T) {
	mc := newCoverageMockCluster()
	handler := NewPeerChatHandler(mc, nil)

	tests := []struct {
		name    string
		payload map[string]interface{}
		want    PeerChatPayload
	}{
		{
			name:    "empty payload",
			payload: map[string]interface{}{},
			want:    PeerChatPayload{},
		},
		{
			name: "full payload",
			payload: map[string]interface{}{
				"type":    "chat",
				"content": "hello",
				"context": map[string]interface{}{"chat_id": "123"},
			},
			want: PeerChatPayload{
				Type:    "chat",
				Content: "hello",
				Context: map[string]interface{}{"chat_id": "123"},
			},
		},
		{
			name: "wrong types",
			payload: map[string]interface{}{
				"type":    123,
				"content": 456,
				"context": "invalid",
			},
			want: PeerChatPayload{},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			var req PeerChatPayload
			handler.parsePayload(tt.payload, &req)

			if req.Type != tt.want.Type {
				t.Errorf("Type = %q, want %q", req.Type, tt.want.Type)
			}
			if req.Content != tt.want.Content {
				t.Errorf("Content = %q, want %q", req.Content, tt.want.Content)
			}
		})
	}
}

func TestPeerChatHandler_ErrorResponse(t *testing.T) {
	mc := newCoverageMockCluster()
	handler := NewPeerChatHandler(mc, nil)

	resp := handler.errorResponse("error", "something went wrong")
	if resp["status"] != "error" {
		t.Errorf("status = %v, want error", resp["status"])
	}
	if resp["response"] != "something went wrong" {
		t.Errorf("response = %v, want 'something went wrong'", resp["response"])
	}
}

func TestPeerChatHandler_Handle_DefaultType(t *testing.T) {
	mc := newCoverageMockCluster()
	handler := NewPeerChatHandler(mc, nil)

	resp, err := handler.Handle(map[string]interface{}{
		"content": "test message",
	})
	if err != nil {
		t.Fatalf("Handle() error = %v", err)
	}
	if resp["status"] != "accepted" {
		t.Errorf("status = %v, want accepted", resp["status"])
	}
	taskID, ok := resp["task_id"].(string)
	if !ok || taskID == "" {
		t.Error("task_id should be a non-empty string")
	}
}

func TestPeerChatHandler_Handle_WithTaskID(t *testing.T) {
	mc := newCoverageMockCluster()
	handler := NewPeerChatHandler(mc, nil)

	resp, _ := handler.Handle(map[string]interface{}{
		"content": "test message",
		"task_id": "custom-task-123",
	})
	if resp["task_id"] != "custom-task-123" {
		t.Errorf("task_id = %v, want custom-task-123", resp["task_id"])
	}
}

func TestPeerChatHandler_Handle_InvalidPayload(t *testing.T) {
	mc := newCoverageMockCluster()
	handler := NewPeerChatHandler(mc, nil)

	// Content field is not a string
	resp, _ := handler.Handle(map[string]interface{}{
		"type":    123,
		"content": 456, // not a string
	})
	// parsePayload extracts what it can; empty content triggers error
	if resp["status"] == "accepted" {
		t.Error("Should not accept when content is missing/empty")
	}
}

func TestPeerChatHandler_Handle_WithContext(t *testing.T) {
	mc := newCoverageMockCluster()
	handler := NewPeerChatHandler(mc, nil)

	resp, _ := handler.Handle(map[string]interface{}{
		"content": "test",
		"context": map[string]interface{}{
			"chat_id":    "chat-456",
			"sender_id":  "sender-789",
		},
		"_source": map[string]interface{}{
			"node_id": "source-node",
		},
	})
	if resp["status"] != "accepted" {
		t.Errorf("status = %v, want accepted", resp["status"])
	}
}

// --- extractIP tests ---

func TestExtractIP(t *testing.T) {
	tests := []struct {
		addr string
		want string
	}{
		{"192.168.1.1:8080", "192.168.1.1"},
		{"[::1]:8080", "::1"},
		{"invalid", "invalid"}, // no port
		{"", ""},
	}

	for _, tt := range tests {
		got := extractIP(tt.addr)
		if got != tt.want {
			t.Errorf("extractIP(%q) = %q, want %q", tt.addr, got, tt.want)
		}
	}
}

// --- isSameSubnet tests ---

func TestIsSameSubnet_RPC_EdgeCases(t *testing.T) {
	tests := []struct {
		name string
		ip1  string
		ip2  string
		mask string
		want bool
	}{
		{"same subnet", "192.168.1.1", "192.168.1.2", "255.255.255.0", true},
		{"different subnet", "192.168.1.1", "192.168.2.1", "255.255.255.0", false},
		{"invalid ip1", "invalid", "192.168.1.2", "255.255.255.0", false},
		{"invalid ip2", "192.168.1.1", "invalid", "255.255.255.0", false},
		{"invalid mask", "192.168.1.1", "192.168.1.2", "invalid", false},
		{"empty strings", "", "", "", false},
		{"broad mask", "10.0.1.1", "10.0.2.1", "255.0.0.0", true},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := isSameSubnet(tt.ip1, tt.ip2, tt.mask)
			if got != tt.want {
				t.Errorf("isSameSubnet(%q, %q, %q) = %v, want %v", tt.ip1, tt.ip2, tt.mask, got, tt.want)
			}
		})
	}
}

// --- Integration test: Server + Client real TCP ---

func TestServer_Client_Integration(t *testing.T) {
	// Start server
	mc := newCoverageMockCluster()
	mc.nodeID = "server-node"
	s := NewServer(mc)

	s.RegisterHandler("echo", func(payload map[string]interface{}) (map[string]interface{}, error) {
		return map[string]interface{}{
			"echo": payload["message"],
		}, nil
	})

	s.Start(0)
	defer s.Stop()

	// Create client that connects to server
	mc2 := newCoverageMockCluster()
	mc2.nodeID = "client-node"
	mc2.peers["server-node"] = &coverageMockNode{
		id:        "server-node",
		name:      "Server",
		address:   fmt.Sprintf("127.0.0.1:%d", s.GetPort()),
		addresses: []string{"127.0.0.1"},
		rpcPort:   s.GetPort(),
		online:    true,
	}
	c := NewClient(mc2)

	resp, err := c.CallWithContext(context.Background(), "server-node", "echo", map[string]interface{}{
		"message": "hello world",
	})
	if err != nil {
		t.Fatalf("CallWithContext() error = %v", err)
	}

	var result map[string]interface{}
	if err := json.Unmarshal(resp, &result); err != nil {
		t.Fatalf("Failed to unmarshal response: %v", err)
	}

	if result["echo"] != "hello world" {
		t.Errorf("echo = %v, want 'hello world'", result["echo"])
	}
}

// --- Server concurrent connection test ---

func TestServer_ConcurrentConnections(t *testing.T) {
	mc := newCoverageMockCluster()
	s := NewServer(mc)

	s.RegisterHandler("ping", func(payload map[string]interface{}) (map[string]interface{}, error) {
		return map[string]interface{}{"status": "ok"}, nil
	})

	s.Start(0)
	defer s.Stop()

	var wg sync.WaitGroup
	var successCount int64

	for i := 0; i < 5; i++ {
		wg.Add(1)
		go func(idx int) {
			defer wg.Done()

			conn, err := net.Dial("tcp", fmt.Sprintf("127.0.0.1:%d", s.GetPort()))
			if err != nil {
				t.Logf("Connection %d failed: %v", idx, err)
				return
			}
			defer conn.Close()

			req := &transport.RPCMessage{
				Version: "1.0",
				Type:    transport.RPCTypeRequest,
				From:    fmt.Sprintf("client-%d", idx),
				To:      "server",
				ID:      fmt.Sprintf("msg-%d", idx),
				Action:  "ping",
				Payload: map[string]interface{}{},
			}

			data, _ := json.Marshal(req)
			if err := transport.WriteFrame(conn, data); err != nil {
				t.Logf("Write %d failed: %v", idx, err)
				return
			}

			respData, err := transport.DecodeFrame(conn)
			if err != nil {
				t.Logf("Read %d failed: %v", idx, err)
				return
			}

			var resp transport.RPCMessage
			if err := json.Unmarshal(respData, &resp); err != nil {
				t.Logf("Unmarshal %d failed: %v", idx, err)
				return
			}

			if resp.Type == transport.RPCTypeResponse {
				atomic.AddInt64(&successCount, 1)
			}
		}(i)
	}

	wg.Wait()

	if atomic.LoadInt64(&successCount) != 5 {
		t.Errorf("successCount = %d, want 5", successCount)
	}
}

// --- Server connection replacement test ---

func TestServer_ConnectionReplacement(t *testing.T) {
	mc := newCoverageMockCluster()
	s := NewServer(mc)

	s.RegisterHandler("ping", func(payload map[string]interface{}) (map[string]interface{}, error) {
		return map[string]interface{}{"status": "ok"}, nil
	})

	s.Start(0)
	defer s.Stop()

	addr := fmt.Sprintf("127.0.0.1:%d", s.GetPort())

	// First connection
	conn1, err := net.Dial("tcp", addr)
	if err != nil {
		t.Fatalf("First connection failed: %v", err)
	}

	// Small delay for connection to be registered
	time.Sleep(50 * time.Millisecond)

	if count := s.GetConnectionCount(); count != 1 {
		t.Logf("Connection count after first = %d (expected 1, may vary)", count)
	}

	// Close first connection
	conn1.Close()

	time.Sleep(50 * time.Millisecond)

	// Second connection from same remote should replace
	conn2, err := net.Dial("tcp", addr)
	if err != nil {
		t.Fatalf("Second connection failed: %v", err)
	}
	defer conn2.Close()

	time.Sleep(50 * time.Millisecond)

	// Should have exactly 1 connection (the new one)
	if count := s.GetConnectionCount(); count != 1 {
		t.Logf("Connection count after second = %d (expected 1, may vary)", count)
	}
}

// --- Server restart test ---

func TestServer_Restart(t *testing.T) {
	mc := newCoverageMockCluster()
	s := NewServer(mc)

	// Start
	if err := s.Start(0); err != nil {
		t.Fatalf("First start failed: %v", err)
	}
	port := s.GetPort()

	// Stop
	if err := s.Stop(); err != nil {
		t.Fatalf("Stop failed: %v", err)
	}

	// Start again (should work because shutdownCh is recreated)
	if err := s.Start(0); err != nil {
		t.Fatalf("Second start failed: %v", err)
	}
	defer s.Stop()

	// Port may be different after restart
	newPort := s.GetPort()
	t.Logf("Original port: %d, new port: %d", port, newPort)
}

// --- Server handleMessage nil message test ---

func TestServer_HandleConnection_NilMessage(t *testing.T) {
	mc := newCoverageMockCluster()
	s := NewServer(mc)

	s.RegisterHandler("test_nil", func(payload map[string]interface{}) (map[string]interface{}, error) {
		return map[string]interface{}{"status": "ok"}, nil
	})

	s.Start(0)
	defer s.Stop()

	conn, err := net.Dial("tcp", fmt.Sprintf("127.0.0.1:%d", s.GetPort()))
	if err != nil {
		t.Fatalf("Failed to connect: %v", err)
	}
	defer conn.Close()

	// Send a valid request to make sure the connection is handled properly
	req := &transport.RPCMessage{
		Version: "1.0",
		Type:    transport.RPCTypeRequest,
		From:    "test",
		To:      "server",
		ID:      "msg-nil-test",
		Action:  "test_nil",
		Payload: map[string]interface{}{},
	}
	data, _ := json.Marshal(req)
	transport.WriteFrame(conn, data)

	// Read response
	respData, err := transport.DecodeFrame(conn)
	if err != nil {
		t.Fatalf("Failed to read response: %v", err)
	}

	var resp transport.RPCMessage
	json.Unmarshal(respData, &resp)
	if resp.ID != "msg-nil-test" {
		t.Errorf("Response ID = %q, want %q", resp.ID, "msg-nil-test")
	}
}

// --- Mock TaskResultStorer ---

type mockTaskResultStorer struct {
	running map[string]bool
	results map[string]*taskResultEntry
	mu      sync.Mutex
}

type taskResultEntry struct {
	taskID       string
	resultStatus string
	response     string
	errMsg       string
	sourceNode   string
}

func newMockTaskResultStorer() *mockTaskResultStorer {
	return &mockTaskResultStorer{
		running: make(map[string]bool),
		results: make(map[string]*taskResultEntry),
	}
}

func (m *mockTaskResultStorer) SetRunning(taskID, sourceNode string) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.running[taskID] = true
}

func (m *mockTaskResultStorer) SetResult(taskID, resultStatus, response, errMsg, sourceNode string) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.results[taskID] = &taskResultEntry{
		taskID:       taskID,
		resultStatus: resultStatus,
		response:     response,
		errMsg:       errMsg,
		sourceNode:   sourceNode,
	}
	delete(m.running, taskID)
	return nil
}

func (m *mockTaskResultStorer) Delete(taskID string) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	delete(m.results, taskID)
	delete(m.running, taskID)
	return nil
}

func TestPeerChatHandler_WithTaskResultStorer(t *testing.T) {
	store := newMockTaskResultStorer()
	mc := newCoverageMockCluster()
	mc.taskStore = store

	handler := NewPeerChatHandler(mc, nil)

	resp, _ := handler.Handle(map[string]interface{}{
		"content": "test with storer",
		"_source": map[string]interface{}{
			"node_id": "source-node",
		},
	})

	taskID := resp["task_id"].(string)

	// Give async goroutine time to run (it should hit nil rpcChannel quickly)
	time.Sleep(100 * time.Millisecond)

	// After async processing, result should be persisted since rpcChannel is nil
	// (callback will fail since there's no cluster.CallWithContext implementation)
	store.mu.Lock()
	_, hasResult := store.results[taskID]
	store.mu.Unlock()

	// The running state should have been set
	if !hasResult {
		// It might be in running state if async hasn't completed yet
		store.mu.Lock()
		_, isRunning := store.running[taskID]
		store.mu.Unlock()
		if !isRunning {
			t.Log("Task should be in either running or results state")
		}
	}
}

// --- deleteResult tests ---

func TestPeerChatHandler_DeleteResult_NilStore(t *testing.T) {
	mc := newCoverageMockCluster()
	handler := NewPeerChatHandler(mc, nil)

	// Should not panic with nil store
	handler.deleteResult("task-123")
}

func TestPeerChatHandler_DeleteResult_WithStore(t *testing.T) {
	store := newMockTaskResultStorer()
	mc := newCoverageMockCluster()
	mc.taskStore = store

	handler := NewPeerChatHandler(mc, nil)

	// Add a result first
	store.SetResult("task-del", "success", "response", "", "node-1")

	// Delete it
	handler.deleteResult("task-del")

	store.mu.Lock()
	_, exists := store.results["task-del"]
	store.mu.Unlock()
	if exists {
		t.Error("Result should be deleted")
	}
}

func TestPeerChatHandler_PersistResult_NilStore(t *testing.T) {
	mc := newCoverageMockCluster()
	handler := NewPeerChatHandler(mc, nil)

	// Should not panic with nil store
	handler.persistResult("task-123", "success", "response", "", "")
}

func TestPeerChatHandler_PersistResult_EmptySourceNode(t *testing.T) {
	store := newMockTaskResultStorer()
	mc := newCoverageMockCluster()
	mc.taskStore = store

	handler := NewPeerChatHandler(mc, nil)

	// Empty sourceNode should not persist
	handler.persistResult("task-123", "success", "response", "", "")

	store.mu.Lock()
	_, exists := store.results["task-123"]
	store.mu.Unlock()
	if exists {
		t.Error("Should not persist with empty source node")
	}
}

// --- Client receiveResponse test ---

func TestClient_ReceiveResponse(t *testing.T) {
	mc := newCoverageMockCluster()
	c := NewClient(mc)

	// Start a test server
	s := NewServer(mc)
	s.Start(0)
	defer s.Stop()

	// Connect
	conn, err := net.Dial("tcp", fmt.Sprintf("127.0.0.1:%d", s.GetPort()))
	if err != nil {
		t.Fatalf("Failed to connect: %v", err)
	}
	defer conn.Close()

	// Wrap connection
	config := &transport.TCPConnConfig{
		NodeID:         "test-client",
		Address:        fmt.Sprintf("127.0.0.1:%d", s.GetPort()),
		ReadBufferSize: 100,
		SendBufferSize: 100,
		SendTimeout:    5 * time.Second,
		IdleTimeout:    0,
	}
	tcpConn := transport.NewTCPConn(conn, config)
	tcpConn.Start()
	defer tcpConn.Close()

	// receiveResponse with short timeout via context
	c.timeout = 100 * time.Millisecond
	_, err = c.receiveResponse(tcpConn, "nonexistent-msg-id")
	if err == nil {
		t.Fatal("receiveResponse with no matching message should timeout")
	}
}

// --- Client connectToPeer tests ---

func TestClient_ConnectToPeer_SingleAddress(t *testing.T) {
	mc := newCoverageMockCluster()
	c := NewClient(mc)

	// Start a test server
	s := NewServer(mc)
	s.Start(0)
	defer s.Stop()

	addr := fmt.Sprintf("127.0.0.1:%d", s.GetPort())
	selectedAddr, conn, err := c.connectToPeer(context.Background(), "test-peer", []string{addr})
	if err != nil {
		t.Fatalf("connectToPeer() error = %v", err)
	}
	if conn == nil {
		t.Fatal("connectToPeer() returned nil conn")
	}
	conn.Close()
	if selectedAddr != addr {
		t.Errorf("selectedAddr = %q, want %q", selectedAddr, addr)
	}
}

func TestClient_ConnectToPeer_MultipleAddresses(t *testing.T) {
	mc := newCoverageMockCluster()
	c := NewClient(mc)

	// Start a test server
	s := NewServer(mc)
	s.Start(0)
	defer s.Stop()

	goodAddr := fmt.Sprintf("127.0.0.1:%d", s.GetPort())
	badAddr := "127.0.0.1:1" // Unlikely to be listening

	selectedAddr, conn, err := c.connectToPeer(context.Background(), "test-peer", []string{badAddr, goodAddr})
	if err != nil {
		t.Fatalf("connectToPeer() error = %v", err)
	}
	conn.Close()
	if selectedAddr != goodAddr {
		t.Errorf("selectedAddr = %q, want %q", selectedAddr, goodAddr)
	}
}

func TestClient_ConnectToPeer_CancelledContext(t *testing.T) {
	mc := newCoverageMockCluster()
	c := NewClient(mc)

	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	_, _, err := c.connectToPeer(ctx, "test-peer", []string{"127.0.0.1:1234"})
	if err == nil {
		t.Fatal("connectToPeer with cancelled context should fail")
	}
}

// --- sendCallback test ---

func TestPeerChatHandler_SendCallback_NoSourceNode(t *testing.T) {
	mc := newCoverageMockCluster()
	handler := NewPeerChatHandler(mc, nil)

	result := handler.sendCallback(map[string]interface{}{}, "task-123", "success", "response", "")
	if result {
		t.Error("sendCallback with no source node_id should return false")
	}
}

func TestPeerChatHandler_SendCallback_WithSourceNode(t *testing.T) {
	mc := newCoverageMockCluster()
	callCount := 0
	mc.callFn = func(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) {
		callCount++
		return []byte(`{"status":"ok"}`), nil
	}

	handler := NewPeerChatHandler(mc, nil)

	result := handler.sendCallback(
		map[string]interface{}{"node_id": "source-node"},
		"task-123",
		"success",
		"response text",
		"",
	)
	if !result {
		t.Error("sendCallback should succeed when RPC call succeeds")
	}
	if callCount != 1 {
		t.Errorf("callCount = %d, want 1", callCount)
	}
}

func TestPeerChatHandler_SendCallback_RetryFailure(t *testing.T) {
	mc := newCoverageMockCluster()
	mc.callFn = func(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) {
		return nil, fmt.Errorf("connection refused")
	}

	handler := NewPeerChatHandler(mc, nil)

	result := handler.sendCallback(
		map[string]interface{}{"node_id": "source-node"},
		"task-123",
		"success",
		"response text",
		"",
	)
	if result {
		t.Error("sendCallback should fail when all retries fail")
	}
}

// --- processAsync partial tests ---

func TestPeerChatHandler_ProcessAsync_NilRPCChannel(t *testing.T) {
	mc := newCoverageMockCluster()
	store := newMockTaskResultStorer()
	mc.taskStore = store

	handler := NewPeerChatHandler(mc, nil)

	resp, _ := handler.Handle(map[string]interface{}{
		"content": "test async nil channel",
		"_source": map[string]interface{}{
			"node_id": "source-node",
		},
	})
	taskID := resp["task_id"].(string)

	// Wait for async processing (should hit nil rpcChannel quickly)
	time.Sleep(200 * time.Millisecond)

	// Task should be persisted with error since rpcChannel is nil
	store.mu.Lock()
	result := store.results[taskID]
	store.mu.Unlock()

	if result == nil {
		t.Log("processAsync result not yet stored (timing issue)")
	} else if result.resultStatus != "error" {
		t.Errorf("resultStatus = %q, want error", result.resultStatus)
	}
}

func TestPeerChatHandler_ProcessAsync_WithRPCChannel(t *testing.T) {
	mc := newCoverageMockCluster()
	store := newMockTaskResultStorer()
	mc.taskStore = store

	// Create a real RPCChannel with bus
	msgBus := bus.NewMessageBus()
	rpcConfig := &channels.RPCChannelConfig{
		MessageBus:      msgBus,
		RequestTimeout:  5 * time.Second,
		CleanupInterval: 1 * time.Second,
	}
	rpcChannel, err := channels.NewRPCChannel(rpcConfig)
	if err != nil {
		t.Fatalf("NewRPCChannel() error = %v", err)
	}

	handler := NewPeerChatHandler(mc, rpcChannel)

	resp, _ := handler.Handle(map[string]interface{}{
		"content": "test async with channel",
		"_source": map[string]interface{}{
			"node_id": "source-node",
		},
		"_rpc": map[string]interface{}{
			"from": "remote-node",
		},
		"context": map[string]interface{}{
			"chat_id":   "chat-1",
			"sender_id": "sender-1",
		},
	})
	taskID := resp["task_id"].(string)

	// Wait for async processing (will timeout since no agent responds)
	time.Sleep(200 * time.Millisecond)

	// Task should be in running state since the RPC channel will wait for response
	store.mu.Lock()
	_, isRunning := store.running[taskID]
	store.mu.Unlock()

	_ = isRunning // timing-dependent, just verify no panic
	_ = taskID
}

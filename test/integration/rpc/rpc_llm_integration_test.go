// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package main

import (
	"context"
	"encoding/json"
	"fmt"
	"net"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/channels"
	clusterrpc "github.com/276793422/NemesisBot/module/cluster/rpc"
	"github.com/276793422/NemesisBot/module/cluster/transport"
	"github.com/276793422/NemesisBot/module/tools"
)

// TestBotToBotRPCIntegration tests the complete Bot-to-Bot RPC LLM call flow
// This demonstrates:
// 1. Bot A (client) sends RPC request to Bot B (server)
// 2. Bot B's RPC server receives request and forwards to local LLM via RPCChannel
// 3. Bot B's MessageTool adds CorrelationID to response
// 4. Bot B's response is delivered back to Bot A via RPC
func TestBotToBotRPCIntegration(t *testing.T) {
	t.Log("=== Bot-to-Bot RPC LLM Integration Test ===")

	// Setup: Create two bot instances
	botB := createTestBot("Bot-B", 21950) // Server bot
	botA := createTestBot("Bot-A", 21949) // Client bot

	// Start Bot B (server)
	if err := botB.Start(); err != nil {
		t.Fatalf("Failed to start Bot B: %v", err)
	}
	defer botB.Stop()

	t.Logf("Bot B started on port %d", botB.RPCPort)

	// Wait for Bot B to be ready
	time.Sleep(500 * time.Millisecond)

	// Bot A sends RPC request to Bot B
	t.Log("\n[Test] Bot A -> Bot B: Send RPC LLM forward request")

	requestPayload := map[string]interface{}{
		"chat_id":  "test-user-123",
		"content":  "Hello from Bot A! What is 2+2?",
		"sender_id": "bot-a",
	}

	// Send request from Bot A to Bot B
	response, err := botA.SendRPCRequest("127.0.0.1:21950", "llm_forward", requestPayload)
	if err != nil {
		t.Fatalf("Failed to send RPC request: %v", err)
	}

	t.Logf("Bot A received response: %s", response)

	// Verify response
	var result map[string]interface{}
	if err := json.Unmarshal([]byte(response), &result); err != nil {
		t.Fatalf("Failed to parse response: %v", err)
	}

	success, ok := result["success"].(bool)
	if !ok || !success {
		t.Errorf("Expected success=true, got: %v", result)
	}

	if content, ok := result["content"].(string); ok {
		t.Logf("LLM Response: %s", content)
	} else {
		t.Error("Expected content in response")
	}

	t.Log("\n=== Test PASSED ===")
}

// TestRPCChannelLLMForwarding tests RPC channel specifically
func TestRPCChannelLLMForwarding(t *testing.T) {
	t.Log("=== RPC Channel LLM Forwarding Test ===")

	// Create MessageBus
	msgBus := bus.NewMessageBus()

	// Create RPC Channel
	cfg := &channels.RPCChannelConfig{
		MessageBus:      msgBus,
		RequestTimeout:  10 * time.Second,
		CleanupInterval: 5 * time.Second,
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

	// Simulate incoming RPC request
	t.Log("\n[Test] Simulating RPC LLM forward request")

	// Step 1: RPC Server receives request and calls RPCChannel.Input()
	correlationID := "test-corr-" + fmt.Sprint(time.Now().UnixNano())
	inbound := &bus.InboundMessage{
		Channel:       "rpc",
		ChatID:        "test-user",
		Content:       "What is the capital of France?",
		CorrelationID: correlationID,
		SenderID:      "remote-bot",
		SessionKey:    "test-session",
	}

	respCh, err := rpcCh.Input(ctx, inbound)
	if err != nil {
		t.Fatalf("Failed to send to RPC channel: %v", err)
	}

	t.Logf("Request sent to MessageBus with CorrelationID: %s", correlationID)

	// Step 2: Simulate LLM processing (MessageTool with CorrelationID)
	// In real scenario, this would be processed by AgentLoop
	// Here we simulate the response with CorrelationID prefix
	simulatedLLMResponse := fmt.Sprintf("[rpc:%s] The capital of France is Paris.", correlationID)

	// Step 3: MessageTool sends response to MessageBus.OutboundChannel
	msgBus.PublishOutbound(bus.OutboundMessage{
		Channel: "rpc",
		ChatID:  "test-user",
		Content: simulatedLLMResponse,
	})

	t.Log("Simulated LLM response sent to MessageBus")

	// Step 4: RPC Channel delivers response to waiting RPC handler
	select {
	case response := <-respCh:
		// Response should have CorrelationID prefix removed
		expectedResponse := "The capital of France is Paris."
		if response != expectedResponse {
			t.Errorf("Expected response '%s', got '%s'", expectedResponse, response)
		} else {
			t.Logf("✅ Received correct response: %s", response)
		}

	case <-time.After(2 * time.Second):
		t.Fatal("Timeout waiting for RPC response")
	}

	t.Log("\n=== Test PASSED ===")
}

// TestMessageToolWithCorrelationID tests MessageTool with CorrelationID
func TestMessageToolWithCorrelationID(t *testing.T) {
	t.Log("=== MessageTool CorrelationID Test ===")

	// Create MessageTool
	tool := tools.NewMessageTool()

	// Test 1: Normal channel (no CorrelationID)
	ctx1 := context.Background()
	tool.SetContext("telegram", "user123")

	tool.SetSendCallback(func(channel, chatID, content string) error {
		// Just verify it was called
		return nil
	})

	result1 := tool.Execute(ctx1, map[string]interface{}{
		"content": "Hello",
	})

	if result1.IsError {
		t.Fatalf("Execute failed: %s", result1.ForLLM)
	}
	t.Logf("Normal channel result: %s", result1.ForLLM)

	// Test 2: RPC channel with CorrelationID in context
	testCorrelationID := "test-corr-123"
	ctx2 := context.WithValue(context.Background(), "correlation_id", testCorrelationID)
	tool.SetContext("rpc", "user456")

	// Track what was sent
	var sentContent2 string
	tool.SetSendCallback(func(channel, chatID, content string) error {
		sentContent2 = content
		return nil
	})

	result2 := tool.Execute(ctx2, map[string]interface{}{
		"content": "Hello from LLM",
	})

	if result2.IsError {
		t.Fatalf("Execute failed: %s", result2.ForLLM)
	}

	// Verify CorrelationID was added
	expectedContent := fmt.Sprintf("[rpc:%s] Hello from LLM", testCorrelationID)
	if sentContent2 != expectedContent {
		t.Errorf("Expected content '%s', got '%s'", expectedContent, sentContent2)
	} else {
		t.Logf("✅ CorrelationID correctly added: %s", sentContent2)
	}

	t.Log("\n=== Test PASSED ===")
}

// testBot represents a test bot instance
type testBot struct {
	Name    string
	RPCPort int
	rpcCh   *channels.RPCChannel
	msgBus  *bus.MessageBus
	rpcSrv  *clusterrpc.Server
}

// createTestBot creates a test bot instance
func createTestBot(name string, rpcPort int) *testBot {
	msgBus := bus.NewMessageBus()

	cfg := &channels.RPCChannelConfig{
		MessageBus:      msgBus,
		RequestTimeout:  10 * time.Second,
		CleanupInterval: 5 * time.Second,
	}

	rpcCh, _ := channels.NewRPCChannel(cfg)

	rpcSrv := clusterrpc.NewServer(&mockClusterForTest{
		name:   name,
		msgBus: msgBus,
	})

	return &testBot{
		Name:   name,
		RPCPort: rpcPort,
		rpcCh:  rpcCh,
		msgBus: msgBus,
		rpcSrv:  rpcSrv,
	}
}

// Start starts the test bot
func (b *testBot) Start() error {
	ctx := context.Background()

	// Start RPC Channel
	if err := b.rpcCh.Start(ctx); err != nil {
		return fmt.Errorf("failed to start RPC channel: %w", err)
	}

	// Register LLM Forward Handler
	handler := &testLLMForwardHandler{
		rpcCh:  b.rpcCh,
		msgBus: b.msgBus,
	}
	b.rpcSrv.RegisterHandler("llm_forward", handler.Handle)

	// Start RPC Server
	if err := b.rpcSrv.Start(b.RPCPort); err != nil {
		return fmt.Errorf("failed to start RPC server: %w", err)
	}

	return nil
}

// Stop stops the test bot
func (b *testBot) Stop() {
	ctx := context.Background()
	b.rpcCh.Stop(ctx)
	b.rpcSrv.Stop()
}

// SendRPCRequest sends an RPC request to another bot
func (b *testBot) SendRPCRequest(address, action string, payload map[string]interface{}) (string, error) {
	// Create TCP connection
	conn, err := net.DialTimeout("tcp", address, 5*time.Second)
	if err != nil {
		return "", fmt.Errorf("failed to dial: %w", err)
	}
	defer conn.Close()

	// Wrap in transport.TCPConn
	tcpConn := transport.NewTCPConn(conn, nil)
	tcpConn.Start()

	// Create RPC request
	req := transport.NewRequest("bot-a", "bot-b", action, payload)

	// Send request
	if err := tcpConn.Send(req); err != nil {
		return "", fmt.Errorf("failed to send: %w", err)
	}

	// Wait for response
	respCh := tcpConn.Receive()
	select {
	case msg := <-respCh:
		if msg.Type == transport.RPCTypeError {
			return "", fmt.Errorf("RPC error: %s", msg.Error)
		}
		if msg.Type == transport.RPCTypeResponse {
			responseBytes, _ := json.Marshal(msg.Payload)
			return string(responseBytes), nil
		}
		return "", fmt.Errorf("unexpected message type: %s", msg.Type)

	case <-time.After(10 * time.Second):
		return "", fmt.Errorf("timeout waiting for response")
	}
}

// mockClusterForTest is a mock Cluster for testing
type mockClusterForTest struct {
	name   string
	msgBus *bus.MessageBus
}

func (m *mockClusterForTest) GetRegistry() interface{}                        { return nil }
func (m *mockClusterForTest) GetNodeID() string                              { return m.name }
func (m *mockClusterForTest) GetAddress() string                             { return "" }
func (m *mockClusterForTest) GetCapabilities() []string                      { return []string{"llm_forward"} }
func (m *mockClusterForTest) GetOnlinePeers() []interface{}                   { return nil }
func (m *mockClusterForTest) LogRPCInfo(msg string, args ...interface{})    { fmt.Printf("[INFO] %s\n", msg) }
func (m *mockClusterForTest) LogRPCError(msg string, args ...interface{})   { fmt.Printf("[ERROR] %s\n", msg) }
func (m *mockClusterForTest) LogRPCDebug(msg string, args ...interface{})  { fmt.Printf("[DEBUG] %s\n", msg) }
func (m *mockClusterForTest) GetPeer(peerID string) (interface{}, error)    { return nil, nil }
func (m *mockClusterForTest) GetLocalNetworkInterfaces() ([]clusterrpc.LocalNetworkInterface, error) {
	return []clusterrpc.LocalNetworkInterface{
		{IP: "127.0.0.1", Mask: "255.255.255.0"},
	}, nil
}

// testLLMForwardHandler handles LLM forward requests in tests
type testLLMForwardHandler struct {
	rpcCh  *channels.RPCChannel
	msgBus *bus.MessageBus
}

func (h *testLLMForwardHandler) Handle(payload map[string]interface{}) (map[string]interface{}, error) {
	// Extract fields
	chatID, _ := payload["chat_id"].(string)
	content, _ := payload["content"].(string)

	if chatID == "" || content == "" {
		return map[string]interface{}{
			"success": false,
			"error":   "chat_id and content are required",
		}, nil
	}

	// Create inbound message
	correlationID := fmt.Sprintf("test-%d", time.Now().UnixNano())
	inbound := &bus.InboundMessage{
		Channel:       "rpc",
		ChatID:        chatID,
		Content:       content,
		CorrelationID: correlationID,
		SenderID:      "remote-bot",
		SessionKey:    "test-session",
	}

	// Send to RPC Channel
	ctx := context.Background()
	respCh, err := h.rpcCh.Input(ctx, inbound)
	if err != nil {
		return map[string]interface{}{
			"success": false,
			"error":   fmt.Sprintf("failed to process: %v", err),
		}, nil
	}

	// Simulate LLM response (in real scenario, AgentLoop would process this)
	// The response must include the CorrelationID prefix
	simulatedResponse := fmt.Sprintf("[rpc:%s] Test LLM response to: %s", correlationID, content)

	// Send response through MessageBus (simulating MessageTool)
	h.msgBus.PublishOutbound(bus.OutboundMessage{
		Channel: "rpc",
		ChatID:  chatID,
		Content: simulatedResponse,
	})

	// Wait for RPC Channel to deliver response
	select {
	case response := <-respCh:
		return map[string]interface{}{
			"success": true,
			"content": response,
		}, nil

	case <-ctx.Done():
		return map[string]interface{}{
			"success": false,
			"error":   "timeout",
		}, nil
	}
}

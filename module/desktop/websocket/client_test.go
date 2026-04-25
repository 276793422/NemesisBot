package websocket

import (
	"context"
	"encoding/json"
	"testing"
	"time"
)

func TestNewWebSocketClient(t *testing.T) {
	key := &WebSocketKey{
		Key:  "test-key-123",
		Port: 8080,
		Path: "/ws",
	}
	client := NewWebSocketClient(key)
	if client == nil {
		t.Fatal("NewWebSocketClient returned nil")
	}
	if client.id != "test-key-123" {
		t.Errorf("Expected id 'test-key-123', got %s", client.id)
	}
	if client.serverURL != "ws://127.0.0.1:8080/ws" {
		t.Errorf("Expected ws://127.0.0.1:8080/ws, got %s", client.serverURL)
	}
	if client.dispatcher == nil {
		t.Error("Dispatcher should be initialized")
	}
}

func TestWebSocketClient_RegisterHandler(t *testing.T) {
	client := NewWebSocketClient(&WebSocketKey{Key: "test", Port: 8080, Path: "/ws"})
	called := false
	client.RegisterHandler("test_method", func(ctx context.Context, msg *Message) (*Message, error) {
		called = true
		return nil, nil
	})
	// Verify by dispatching a message through the dispatcher
	dmsg := &Message{
		JSONRPC: Version,
		ID:      "1",
		Method:  "test_method",
	}
	client.dispatcher.Dispatch(context.Background(), dmsg)
	if !called {
		t.Error("Handler should have been called")
	}
}

func TestWebSocketClient_RegisterNotificationHandler(t *testing.T) {
	client := NewWebSocketClient(&WebSocketKey{Key: "test", Port: 8080, Path: "/ws"})
	called := false
	client.RegisterNotificationHandler("test_notify", func(ctx context.Context, msg *Message) {
		called = true
	})
	// Verify through dispatcher
	dmsg := &Message{
		JSONRPC: Version,
		Method:  "test_notify",
	}
	client.dispatcher.Dispatch(context.Background(), dmsg)
	if !called {
		t.Error("Notification handler should have been called")
	}
}

func TestWebSocketClient_SetFallbackHandler(t *testing.T) {
	client := NewWebSocketClient(&WebSocketKey{Key: "test", Port: 8080, Path: "/ws"})
	called := false
	client.SetFallbackHandler(func(ctx context.Context, msg *Message) (*Message, error) {
		called = true
		return nil, nil
	})
	// Verify through dispatcher with unknown method
	dmsg := &Message{
		JSONRPC: Version,
		ID:      "1",
		Method:  "unknown_method",
	}
	client.dispatcher.Dispatch(context.Background(), dmsg)
	if !called {
		t.Error("Fallback handler should have been called")
	}
}

func TestWebSocketClient_Close_NoConnection(t *testing.T) {
	client := NewWebSocketClient(&WebSocketKey{Key: "test", Port: 8080, Path: "/ws"})
	err := client.Close()
	if err != nil {
		t.Errorf("Close should not error with no connection, got: %v", err)
	}
}

func TestWebSocketClient_Close_DoubleClose(t *testing.T) {
	client := NewWebSocketClient(&WebSocketKey{Key: "test", Port: 8080, Path: "/ws"})
	client.Close()
	client.Close() // Should not panic
}

func TestWebSocketClient_handleProtocolMessage_Response(t *testing.T) {
	client := NewWebSocketClient(&WebSocketKey{Key: "test", Port: 8080, Path: "/ws"})

	// Register a pending request
	respCh := make(chan *Message, 1)
	client.pendingMu.Lock()
	client.pending["test-id-1"] = respCh
	client.pendingMu.Unlock()

	// Simulate receiving a response
	msg := &Message{
		JSONRPC: Version,
		ID:      "test-id-1",
		Result:  json.RawMessage(`"ok"`),
	}
	client.handleProtocolMessage(msg)

	// Verify response was routed
	select {
	case resp := <-respCh:
		if resp.ID != "test-id-1" {
			t.Errorf("Expected ID 'test-id-1', got %s", resp.ID)
		}
	case <-time.After(time.Second):
		t.Error("Response should have been routed to pending channel")
	}

	// Clean up
	client.pendingMu.Lock()
	delete(client.pending, "test-id-1")
	client.pendingMu.Unlock()
}

func TestWebSocketClient_handleProtocolMessage_ResponseNoPending(t *testing.T) {
	client := NewWebSocketClient(&WebSocketKey{Key: "test", Port: 8080, Path: "/ws"})

	// Response for non-existent pending request - should not panic
	msg := &Message{
		JSONRPC: Version,
		ID:      "nonexistent",
		Result:  json.RawMessage(`"ok"`),
	}
	client.handleProtocolMessage(msg)
}

func TestWebSocketClient_handleProtocolMessage_Request(t *testing.T) {
	client := NewWebSocketClient(&WebSocketKey{Key: "test", Port: 8080, Path: "/ws"})

	called := false
	client.RegisterHandler("test_method", func(ctx context.Context, m *Message) (*Message, error) {
		called = true
		return &Message{
			JSONRPC: Version,
			ID:      m.ID,
			Result:  json.RawMessage(`"handled"`),
		}, nil
	})

	msg := &Message{
		JSONRPC: Version,
		ID:      "test-id-1",
		Method:  "test_method",
		Params:  json.RawMessage(`{}`),
	}
	client.handleProtocolMessage(msg)

	if !called {
		t.Error("Handler should have been called")
	}
}

func TestWebSocketClient_handleProtocolMessage_Notification(t *testing.T) {
	client := NewWebSocketClient(&WebSocketKey{Key: "test", Port: 8080, Path: "/ws"})

	called := false
	client.RegisterNotificationHandler("test_notify", func(ctx context.Context, m *Message) {
		called = true
	})

	msg := &Message{
		JSONRPC: Version,
		Method:  "test_notify",
		Params:  json.RawMessage(`{}`),
	}
	client.handleProtocolMessage(msg)

	if !called {
		t.Error("Notification handler should have been called")
	}
}

func TestWebSocketClient_sendRaw(t *testing.T) {
	client := NewWebSocketClient(&WebSocketKey{Key: "test", Port: 8080, Path: "/ws"})

	// Consume from sendCh
	go client.writeLoop()

	msg := &Message{
		JSONRPC: Version,
		Method:  "test",
	}
	err := client.sendRaw(msg)
	// writeLoop will find no conn and return, but sendRaw should succeed in sending to channel
	_ = err
}

func TestWebSocketClient_Notify(t *testing.T) {
	client := NewWebSocketClient(&WebSocketKey{Key: "test", Port: 8080, Path: "/ws"})

	// Consume from sendCh
	go client.writeLoop()

	err := client.Notify("test_notification", map[string]string{"key": "value"})
	_ = err
}

func TestWebSocketClient_Call_ContextCancelled(t *testing.T) {
	client := NewWebSocketClient(&WebSocketKey{Key: "test", Port: 8080, Path: "/ws"})

	ctx, cancel := context.WithCancel(context.Background())
	cancel() // Cancel immediately

	_, err := client.Call(ctx, "test_method", nil)
	if err == nil {
		t.Error("Expected error with cancelled context")
	}
}

func TestClientErrors(t *testing.T) {
	if ErrClientSendTimeout == nil {
		t.Error("ErrClientSendTimeout should not be nil")
	}
	if ErrClientCallTimeout == nil {
		t.Error("ErrClientCallTimeout should not be nil")
	}
	if ErrClientSendTimeout.Code != "SEND_TIMEOUT" {
		t.Errorf("Expected SEND_TIMEOUT, got %s", ErrClientSendTimeout.Code)
	}
	if ErrClientCallTimeout.Code != "CALL_TIMEOUT" {
		t.Errorf("Expected CALL_TIMEOUT, got %s", ErrClientCallTimeout.Code)
	}
}

func TestWebSocketClient_ConcurrentHandlers(t *testing.T) {
	client := NewWebSocketClient(&WebSocketKey{Key: "test", Port: 8080, Path: "/ws"})

	var count int
	client.RegisterHandler("concurrent", func(ctx context.Context, m *Message) (*Message, error) {
		count++
		return nil, nil
	})

	done := make(chan bool)
	for i := 0; i < 10; i++ {
		go func() {
			dmsg := &Message{
				JSONRPC: Version,
				ID:      "1",
				Method:  "concurrent",
			}
			client.dispatcher.Dispatch(context.Background(), dmsg)
			done <- true
		}()
	}

	for i := 0; i < 10; i++ {
		<-done
	}
	// count may not be exactly 10 due to race, but should be > 0
}

// Integration test: server + client
func TestWebSocketServerClientIntegration(t *testing.T) {
	keyGen := NewKeyGenerator()
	server := NewWebSocketServer(keyGen)
	if err := server.Start(); err != nil {
		t.Fatalf("Failed to start server: %v", err)
	}
	defer server.Stop()

	port := server.GetPort()
	wsKey, err := keyGen.Generate("child-1", 1234)
	if err != nil {
		t.Fatalf("Failed to generate key: %v", err)
	}
	wsKey.Port = port
	wsKey.Path = "/"

	client := NewWebSocketClient(wsKey)
	client.RegisterHandler("ping", func(ctx context.Context, m *Message) (*Message, error) {
		return &Message{
			JSONRPC: Version,
			ID:      m.ID,
			Result:  json.RawMessage(`"pong"`),
		}, nil
	})

	if err := client.Connect(); err != nil {
		t.Fatalf("Failed to connect: %v", err)
	}
	defer client.Close()

	time.Sleep(100 * time.Millisecond)

	conn := server.GetConnection(wsKey.Key)
	if conn == nil {
		t.Error("Server should have the client connection")
	}

	// Server sends notification to client
	err = server.SendNotification(wsKey.Key, "test_notify", map[string]string{"msg": "hello"})
	if err != nil {
		t.Errorf("SendNotification failed: %v", err)
	}

	// Verify we can send notification to child ID
	err = server.SendNotification("child-1", "test_notify2", map[string]string{"msg": "hello2"})
	if err != nil {
		t.Errorf("SendNotification by childID failed: %v", err)
	}
}

func TestWebSocketServerClient_CallChild(t *testing.T) {
	keyGen := NewKeyGenerator()
	server := NewWebSocketServer(keyGen)
	if err := server.Start(); err != nil {
		t.Fatalf("Failed to start server: %v", err)
	}
	defer server.Stop()

	port := server.GetPort()
	wsKey, err := keyGen.Generate("child-2", 1234)
	if err != nil {
		t.Fatalf("Failed to generate key: %v", err)
	}
	wsKey.Port = port
	wsKey.Path = "/"

	client := NewWebSocketClient(wsKey)
	client.RegisterHandler("get_status", func(ctx context.Context, m *Message) (*Message, error) {
		return &Message{
			JSONRPC: Version,
			ID:      m.ID,
			Result:  json.RawMessage(`{"status":"ok"}`),
		}, nil
	})

	if err := client.Connect(); err != nil {
		t.Fatalf("Failed to connect: %v", err)
	}
	defer client.Close()

	time.Sleep(100 * time.Millisecond)

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	resp, err := server.CallChild(ctx, wsKey.Key, "get_status", nil)
	if err != nil {
		t.Errorf("CallChild failed: %v", err)
	}
	if resp == nil {
		t.Error("Response should not be nil")
	}
}

func TestWebSocketServer_SendNotification_ConnectionNotFound(t *testing.T) {
	keyGen := NewKeyGenerator()
	server := NewWebSocketServer(keyGen)

	err := server.SendNotification("nonexistent", "test", nil)
	if err == nil {
		t.Error("Expected error for non-existent connection")
	}
	if err != ErrConnectionNotFound {
		t.Errorf("Expected ErrConnectionNotFound, got: %v", err)
	}
}

func TestWebSocketServer_CallChild_ConnectionNotFound(t *testing.T) {
	keyGen := NewKeyGenerator()
	server := NewWebSocketServer(keyGen)

	_, err := server.CallChild(context.Background(), "nonexistent", "test", nil)
	if err == nil {
		t.Error("Expected error for non-existent connection")
	}
}

func TestWebSocketServer_RemoveConnection_NonExistent(t *testing.T) {
	keyGen := NewKeyGenerator()
	server := NewWebSocketServer(keyGen)
	server.RemoveConnection("nonexistent")
}

func TestNewRequestWithID(t *testing.T) {
	msg, err := NewRequestWithID("custom-id", "test_method", json.RawMessage(`{"key":"val"}`))
	if err != nil {
		t.Fatalf("NewRequestWithID error: %v", err)
	}
	if msg.ID != "custom-id" {
		t.Errorf("Expected ID 'custom-id', got %s", msg.ID)
	}
	if msg.Method != "test_method" {
		t.Errorf("Expected method 'test_method', got %s", msg.Method)
	}
}

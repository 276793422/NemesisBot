//go:build !cross_compile

package websocket

import (
	"context"
	"encoding/json"
	"errors"
	"testing"
	"time"
)

func TestNewWebSocketServer(t *testing.T) {
	kg := NewKeyGenerator()
	srv := NewWebSocketServer(kg)
	if srv == nil {
		t.Fatal("NewWebSocketServer returned nil")
	}
}

func TestWebSocketServerStartStop(t *testing.T) {
	kg := NewKeyGenerator()
	srv := NewWebSocketServer(kg)

	if err := srv.Start(); err != nil {
		t.Fatalf("Start failed: %v", err)
	}

	port := srv.GetPort()
	if port <= 0 {
		t.Errorf("Expected positive port after start, got %d", port)
	}

	if err := srv.Stop(); err != nil {
		t.Fatalf("Stop failed: %v", err)
	}
}

func TestWebSocketServerGetPortBeforeStart(t *testing.T) {
	kg := NewKeyGenerator()
	srv := NewWebSocketServer(kg)

	port := srv.GetPort()
	if port != 0 {
		t.Errorf("Expected port 0 before start, got %d", port)
	}
}

func TestWebSocketServerDoubleStop(t *testing.T) {
	kg := NewKeyGenerator()
	srv := NewWebSocketServer(kg)

	if err := srv.Start(); err != nil {
		t.Fatalf("Start failed: %v", err)
	}

	if err := srv.Stop(); err != nil {
		t.Fatalf("First Stop failed: %v", err)
	}

	// Second stop should not panic
	if err := srv.Stop(); err != nil {
		t.Fatalf("Second Stop failed: %v", err)
	}
}

func TestWebSocketServerGetConnectionNotFound(t *testing.T) {
	kg := NewKeyGenerator()
	srv := NewWebSocketServer(kg)

	conn := srv.GetConnection("nonexistent")
	if conn != nil {
		t.Error("Expected nil for unknown connection")
	}
}

func TestWebSocketServerSendNotificationNotFound(t *testing.T) {
	kg := NewKeyGenerator()
	srv := NewWebSocketServer(kg)

	err := srv.SendNotification("nonexistent", "window.bring_to_front", nil)
	if err == nil {
		t.Error("Expected error when sending notification to nonexistent child")
	}
	if !errors.Is(err, ErrConnectionNotFound) {
		t.Errorf("Expected ErrConnectionNotFound, got: %v", err)
	}
}

func TestWebSocketServerRemoveConnectionNotFound(t *testing.T) {
	kg := NewKeyGenerator()
	srv := NewWebSocketServer(kg)

	// Should not panic
	srv.RemoveConnection("nonexistent")
}

func TestWebSocketServerDynamicPort(t *testing.T) {
	kg := NewKeyGenerator()
	srv1 := NewWebSocketServer(kg)
	srv2 := NewWebSocketServer(kg)

	if err := srv1.Start(); err != nil {
		t.Fatalf("srv1 Start failed: %v", err)
	}
	defer srv1.Stop()

	if err := srv2.Start(); err != nil {
		t.Fatalf("srv2 Start failed: %v", err)
	}
	defer srv2.Stop()

	port1 := srv1.GetPort()
	port2 := srv2.GetPort()

	if port1 == port2 {
		t.Errorf("Two servers should get different ports, both got %d", port1)
	}
}

func TestWebSocketServerStartStopQuick(t *testing.T) {
	kg := NewKeyGenerator()
	srv := NewWebSocketServer(kg)

	if err := srv.Start(); err != nil {
		t.Fatalf("Start failed: %v", err)
	}

	time.Sleep(10 * time.Millisecond)

	if err := srv.Stop(); err != nil {
		t.Fatalf("Stop failed: %v", err)
	}
}

func TestWebSocketServerCallChildNotFound(t *testing.T) {
	kg := NewKeyGenerator()
	srv := NewWebSocketServer(kg)

	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()

	resp, err := srv.CallChild(ctx, "nonexistent", "system.ping", nil)
	if err == nil {
		t.Error("Expected error when calling nonexistent child")
	}
	if resp != nil {
		t.Error("Expected nil response for nonexistent child")
	}
	if !errors.Is(err, ErrConnectionNotFound) {
		t.Errorf("Expected ErrConnectionNotFound, got: %v", err)
	}
}

func TestWebSocketServerRegisterHandler(t *testing.T) {
	kg := NewKeyGenerator()
	srv := NewWebSocketServer(kg)

	srv.RegisterHandler("test.method", func(ctx context.Context, msg *Message) (*Message, error) {
		return NewResponse(msg.ID, map[string]string{"ok": "true"})
	})

	if len(srv.dispatcher.handlers) != 1 {
		t.Errorf("Expected 1 handler, got %d", len(srv.dispatcher.handlers))
	}
}

func TestWebSocketServerRegisterNotificationHandler(t *testing.T) {
	kg := NewKeyGenerator()
	srv := NewWebSocketServer(kg)

	srv.RegisterNotificationHandler("test.notify", func(ctx context.Context, msg *Message) {
	})

	if len(srv.dispatcher.notifHandlers) != 1 {
		t.Errorf("Expected 1 notification handler, got %d", len(srv.dispatcher.notifHandlers))
	}
}

func TestWebSocketServerSendNotificationToConnection(t *testing.T) {
	kg := NewKeyGenerator()
	srv := NewWebSocketServer(kg)

	conn := &ChildConnection{
		ID:     "child-1",
		Key:    "test-key",
		SendCh: make(chan []byte, 10),
		Meta:   make(map[string]string),
	}
	srv.mu.Lock()
	srv.connections["child-1"] = conn
	srv.mu.Unlock()

	err := srv.SendNotification("child-1", "window.bring_to_front", map[string]string{"window": "main"})
	if err != nil {
		t.Fatalf("SendNotification failed: %v", err)
	}

	select {
	case data := <-conn.SendCh:
		var msg Message
		if err := json.Unmarshal(data, &msg); err != nil {
			t.Fatalf("Failed to unmarshal sent message: %v", err)
		}
		if msg.JSONRPC != Version {
			t.Errorf("JSONRPC = %q, want %q", msg.JSONRPC, Version)
		}
		if msg.Method != "window.bring_to_front" {
			t.Errorf("Method = %q, want %q", msg.Method, "window.bring_to_front")
		}
		if msg.ID != "" {
			t.Error("Notification should have empty ID")
		}
	case <-time.After(time.Second):
		t.Fatal("Timeout waiting for message in SendCh")
	}
}

func TestWebSocketServerConnectionLevelDispatcher(t *testing.T) {
	kg := NewKeyGenerator()
	srv := NewWebSocketServer(kg)

	connCalled := false
	srvCalled := false

	srv.RegisterNotificationHandler("test.notify", func(ctx context.Context, msg *Message) {
		srvCalled = true
	})

	conn := &ChildConnection{
		ID:         "child-1",
		Key:        "test-key",
		SendCh:     make(chan []byte, 10),
		Meta:       make(map[string]string),
		Dispatcher: NewDispatcher(),
	}
	conn.Dispatcher.RegisterNotification("test.notify", func(ctx context.Context, msg *Message) {
		connCalled = true
	})

	srv.mu.Lock()
	srv.connections["child-1"] = conn
	srv.mu.Unlock()

	notif, _ := NewNotification("test.notify", nil)
	srv.handleServerProtocolMessage(conn, notif)

	if !connCalled {
		t.Error("Connection-level dispatcher handler was not called")
	}
	if srvCalled {
		t.Error("Server-level dispatcher should not be called when connection-level exists")
	}
}

func TestRemoveAllConnectionKeys(t *testing.T) {
	kg := NewKeyGenerator()
	srv := NewWebSocketServer(kg)

	conn := &ChildConnection{
		ID:     "uuid-123",
		SendCh: make(chan []byte, 10),
		Meta:   map[string]string{"child_id": "child-1"},
	}

	// 注册双键
	srv.mu.Lock()
	srv.connections["uuid-123"] = conn
	srv.connections["child-1"] = conn
	srv.mu.Unlock()

	if len(srv.connections) != 2 {
		t.Fatalf("Expected 2 keys, got %d", len(srv.connections))
	}

	// removeAllConnectionKeys 应该同时删除两个 key
	srv.removeAllConnectionKeys(conn)

	if len(srv.connections) != 0 {
		t.Errorf("Expected 0 keys after removal, got %d", len(srv.connections))
	}
}

func TestRemoveConnectionIdempotent(t *testing.T) {
	kg := NewKeyGenerator()
	srv := NewWebSocketServer(kg)

	conn := &ChildConnection{
		ID:     "uuid-456",
		SendCh: make(chan []byte, 10),
		Meta:   map[string]string{"child_id": "child-2"},
	}

	srv.mu.Lock()
	srv.connections["uuid-456"] = conn
	srv.connections["child-2"] = conn
	srv.mu.Unlock()

	// 第一次移除
	srv.RemoveConnection("child-2")

	// 第二次移除应该不 panic（conn 已被删除，查找不到直接返回）
	srv.RemoveConnection("child-2")
	srv.RemoveConnection("uuid-456")
}

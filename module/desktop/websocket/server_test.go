//go:build !cross_compile

package websocket

import (
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

func TestWebSocketServerSendToChildNotFound(t *testing.T) {
	kg := NewKeyGenerator()
	srv := NewWebSocketServer(kg)

	err := srv.SendToChild("nonexistent", map[string]interface{}{"test": true})
	if err == nil {
		t.Error("Expected error when sending to nonexistent child")
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

	// Immediately stop to test quick lifecycle
	time.Sleep(10 * time.Millisecond)

	if err := srv.Stop(); err != nil {
		t.Fatalf("Stop failed: %v", err)
	}
}

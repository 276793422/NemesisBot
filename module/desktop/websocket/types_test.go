//go:build !cross_compile

package websocket

import (
	"testing"
)

func TestChildConnection(t *testing.T) {
	conn := &ChildConnection{
		ID:       "conn-1",
		Key:      "test-key",
		SendCh:   make(chan []byte, 10),
		ChildPID: 1234,
		Meta:     map[string]string{"child_id": "child-1"},
	}

	if conn.ID != "conn-1" {
		t.Errorf("Expected ID 'conn-1', got '%s'", conn.ID)
	}
	if conn.Key != "test-key" {
		t.Errorf("Expected Key 'test-key', got '%s'", conn.Key)
	}
	if conn.ChildPID != 1234 {
		t.Errorf("Expected ChildPID 1234, got %d", conn.ChildPID)
	}
	if conn.Meta["child_id"] != "child-1" {
		t.Errorf("Expected Meta[child_id]='child-1', got '%s'", conn.Meta["child_id"])
	}

	// Test channel operations
	conn.SendCh <- []byte("test")
}

func TestChildConnectionCloseSendIdempotent(t *testing.T) {
	conn := &ChildConnection{
		ID:     "conn-close-test",
		SendCh: make(chan []byte, 10),
	}

	// Close twice should not panic (sync.Once protection)
	conn.CloseSend()
	conn.CloseSend()
}

func TestWebSocketError(t *testing.T) {
	err := &WebSocketError{Code: "TEST_ERROR", Message: "test error"}
	if err.Error() != "test error" {
		t.Errorf("Expected 'test error', got '%s'", err.Error())
	}
	if err.Code != "TEST_ERROR" {
		t.Errorf("Expected Code 'TEST_ERROR', got '%s'", err.Code)
	}
}

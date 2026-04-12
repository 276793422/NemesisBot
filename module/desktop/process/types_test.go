//go:build !cross_compile

package process

import (
	"encoding/json"
	"testing"
)

func TestProcessStatusConstants(t *testing.T) {
	statuses := []ProcessStatus{
		ProcessStatusStarting,
		ProcessStatusRunning,
		ProcessStatusHandshaking,
		ProcessStatusConnected,
		ProcessStatusTerminated,
		ProcessStatusFailed,
	}
	// Verify all statuses are distinct
	seen := make(map[ProcessStatus]bool)
	for _, s := range statuses {
		if seen[s] {
			t.Errorf("Duplicate status value: %d", s)
		}
		seen[s] = true
	}
	if len(seen) != 6 {
		t.Errorf("Expected 6 distinct statuses, got %d", len(seen))
	}
}

func TestPipeMessageJSON(t *testing.T) {
	msg := &PipeMessage{
		Type:    "handshake",
		Version: "1.0",
		Data: map[string]interface{}{
			"key":   "test-key",
			"count": float64(42),
		},
	}

	data, err := json.Marshal(msg)
	if err != nil {
		t.Fatalf("Failed to marshal PipeMessage: %v", err)
	}

	var decoded PipeMessage
	if err := json.Unmarshal(data, &decoded); err != nil {
		t.Fatalf("Failed to unmarshal PipeMessage: %v", err)
	}

	if decoded.Type != "handshake" {
		t.Errorf("Expected type 'handshake', got '%s'", decoded.Type)
	}
	if decoded.Version != "1.0" {
		t.Errorf("Expected version '1.0', got '%s'", decoded.Version)
	}
	if decoded.Data["key"] != "test-key" {
		t.Errorf("Expected key 'test-key', got '%v'", decoded.Data["key"])
	}
}

func TestChildResult(t *testing.T) {
	tests := []struct {
		name    string
		result  ChildResult
		success bool
		hasErr  bool
	}{
		{
			name:    "successful result",
			result:  ChildResult{Success: true, Data: "ok"},
			success: true,
			hasErr:  false,
		},
		{
			name:    "failed result",
			result:  ChildResult{Success: false, Error: ErrPopupNotSupported},
			success: false,
			hasErr:  true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if tt.result.Success != tt.success {
				t.Errorf("Expected Success=%v, got %v", tt.success, tt.result.Success)
			}
			if tt.hasErr && tt.result.Error == nil {
				t.Error("Expected non-nil error")
			}
			if !tt.hasErr && tt.result.Error != nil {
				t.Errorf("Expected nil error, got %v", tt.result.Error)
			}
		})
	}
}

func TestHandshakeResult(t *testing.T) {
	result := &HandshakeResult{
		Success:  true,
		WindowID: "win-1",
	}
	if !result.Success {
		t.Error("Expected Success to be true")
	}
	if result.WindowID != "win-1" {
		t.Errorf("Expected WindowID 'win-1', got '%s'", result.WindowID)
	}
}

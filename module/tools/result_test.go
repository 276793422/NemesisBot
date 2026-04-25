// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package tools

import (
	"encoding/json"
	"errors"
	"testing"
)

func TestNewToolResult_Coverage(t *testing.T) {
	result := NewToolResult("test content")
	if result.ForLLM != "test content" {
		t.Errorf("Expected ForLLM 'test content', got '%s'", result.ForLLM)
	}
	if result.Silent {
		t.Error("NewToolResult should not be silent")
	}
	if result.IsError {
		t.Error("NewToolResult should not be an error")
	}
	if result.Async {
		t.Error("NewToolResult should not be async")
	}
}

func TestSilentResult_Coverage(t *testing.T) {
	result := SilentResult("quiet update")
	if result.ForLLM != "quiet update" {
		t.Errorf("Expected ForLLM 'quiet update', got '%s'", result.ForLLM)
	}
	if !result.Silent {
		t.Error("SilentResult should be silent")
	}
	if result.IsError {
		t.Error("SilentResult should not be an error")
	}
	if result.Async {
		t.Error("SilentResult should not be async")
	}
}

func TestAsyncResult_Coverage(t *testing.T) {
	result := AsyncResult("async task started")
	if result.ForLLM != "async task started" {
		t.Errorf("Expected ForLLM 'async task started', got '%s'", result.ForLLM)
	}
	if result.Silent {
		t.Error("AsyncResult should not be silent")
	}
	if result.IsError {
		t.Error("AsyncResult should not be an error")
	}
	if !result.Async {
		t.Error("AsyncResult should be async")
	}
}

func TestErrorResult_Coverage(t *testing.T) {
	result := ErrorResult("something failed")
	if result.ForLLM != "something failed" {
		t.Errorf("Expected ForLLM 'something failed', got '%s'", result.ForLLM)
	}
	if result.Silent {
		t.Error("ErrorResult should not be silent")
	}
	if !result.IsError {
		t.Error("ErrorResult should be an error")
	}
	if result.Async {
		t.Error("ErrorResult should not be async")
	}
}

func TestUserResult_Coverage(t *testing.T) {
	result := UserResult("visible content")
	if result.ForLLM != "visible content" {
		t.Errorf("Expected ForLLM 'visible content', got '%s'", result.ForLLM)
	}
	if result.ForUser != "visible content" {
		t.Errorf("Expected ForUser 'visible content', got '%s'", result.ForUser)
	}
	if result.Silent {
		t.Error("UserResult should not be silent")
	}
	if result.IsError {
		t.Error("UserResult should not be an error")
	}
	if result.Async {
		t.Error("UserResult should not be async")
	}
}

func TestToolResult_MarshalJSON(t *testing.T) {
	tests := []struct {
		name    string
		result  *ToolResult
		wantErr bool
	}{
		{
			name:   "basic result",
			result: NewToolResult("hello"),
		},
		{
			name:   "error result",
			result: ErrorResult("failed"),
		},
		{
			name:   "silent result",
			result: SilentResult("quiet"),
		},
		{
			name:   "async result",
			result: AsyncResult("async"),
		},
		{
			name:   "user result",
			result: UserResult("user content"),
		},
		{
			name:   "result with error",
			result: ErrorResult("err").WithError(errors.New("underlying error")),
		},
		{
			name: "result with TaskID",
			result: &ToolResult{
				ForLLM: "async task",
				Async:  true,
				TaskID: "task-123",
			},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			data, err := json.Marshal(tt.result)
			if (err != nil) != tt.wantErr {
				t.Errorf("MarshalJSON() error = %v, wantErr %v", err, tt.wantErr)
				return
			}

			// Verify Err is NOT serialized (json:"-" tag)
			var parsed map[string]interface{}
			if err := json.Unmarshal(data, &parsed); err != nil {
				t.Fatalf("Failed to unmarshal: %v", err)
			}

			if _, hasErr := parsed["Err"]; hasErr {
				t.Error("Err field should not be in JSON output")
			}

			// Verify key fields are present
			if _, hasForLLM := parsed["for_llm"]; !hasForLLM {
				t.Error("for_llm field should be present in JSON output")
			}
		})
	}
}

func TestToolResult_WithError(t *testing.T) {
	err := errors.New("test error")
	result := ErrorResult("operation failed").WithError(err)

	if result.Err != err {
		t.Error("WithError should set Err field")
	}
	if !result.IsError {
		t.Error("Result should still be an error")
	}
	if result.ForLLM != "operation failed" {
		t.Errorf("ForLLM should be preserved, got '%s'", result.ForLLM)
	}

	// Test chaining with nil error
	result2 := NewToolResult("ok").WithError(nil)
	if result2.Err != nil {
		t.Error("Err should be nil")
	}
}

func TestToolResult_MarshalJSON_RoundTrip(t *testing.T) {
	original := &ToolResult{
		ForLLM:  "test content",
		ForUser: "user content",
		Silent:  true,
		IsError: false,
		Async:   false,
		TaskID:  "task-456",
	}

	data, err := json.Marshal(original)
	if err != nil {
		t.Fatalf("Marshal failed: %v", err)
	}

	var restored ToolResult
	if err := json.Unmarshal(data, &restored); err != nil {
		t.Fatalf("Unmarshal failed: %v", err)
	}

	if restored.ForLLM != original.ForLLM {
		t.Errorf("ForLLM mismatch: got '%s', want '%s'", restored.ForLLM, original.ForLLM)
	}
	if restored.ForUser != original.ForUser {
		t.Errorf("ForUser mismatch: got '%s', want '%s'", restored.ForUser, original.ForUser)
	}
	if restored.Silent != original.Silent {
		t.Errorf("Silent mismatch: got %v, want %v", restored.Silent, original.Silent)
	}
	if restored.TaskID != original.TaskID {
		t.Errorf("TaskID mismatch: got '%s', want '%s'", restored.TaskID, original.TaskID)
	}
}

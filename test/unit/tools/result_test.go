// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package tools_test

import (
	"encoding/json"
	"errors"
	"testing"

	. "github.com/276793422/NemesisBot/module/tools"
)

func TestNewToolResult(t *testing.T) {
	result := NewToolResult("test content")

	if result.ForLLM != "test content" {
		t.Errorf("Expected ForLLM 'test content', got '%s'", result.ForLLM)
	}

	if result.ForUser != "" {
		t.Errorf("Expected empty ForUser, got '%s'", result.ForUser)
	}

	if result.Silent {
		t.Error("Expected Silent to be false")
	}

	if result.IsError {
		t.Error("Expected IsError to be false")
	}

	if result.Async {
		t.Error("Expected Async to be false")
	}
}

func TestSilentResult(t *testing.T) {
	result := SilentResult("silent operation")

	if result.ForLLM != "silent operation" {
		t.Errorf("Expected ForLLM 'silent operation', got '%s'", result.ForLLM)
	}

	if !result.Silent {
		t.Error("Expected Silent to be true")
	}

	if result.IsError {
		t.Error("Expected IsError to be false")
	}

	if result.Async {
		t.Error("Expected Async to be false")
	}
}

func TestAsyncResult(t *testing.T) {
	result := AsyncResult("async operation started")

	if result.ForLLM != "async operation started" {
		t.Errorf("Expected ForLLM 'async operation started', got '%s'", result.ForLLM)
	}

	if !result.Async {
		t.Error("Expected Async to be true for async result")
	}

	if result.Silent {
		t.Error("Expected Silent to be false")
	}

	if result.IsError {
		t.Error("Expected IsError to be false")
	}
}

func TestErrorResult(t *testing.T) {
	result := ErrorResult("operation failed")

	if result.ForLLM != "operation failed" {
		t.Errorf("Expected ForLLM 'operation failed', got '%s'", result.ForLLM)
	}

	if !result.IsError {
		t.Error("Expected IsError to be true")
	}

	if result.Silent {
		t.Error("Expected Silent to be false")
	}

	if result.Async {
		t.Error("Expected Async to be false")
	}
}

func TestUserResult(t *testing.T) {
	content := "user visible result"
	result := UserResult(content)

	if result.ForLLM != content {
		t.Errorf("Expected ForLLM '%s', got '%s'", content, result.ForLLM)
	}

	if result.ForUser != content {
		t.Errorf("Expected ForUser '%s', got '%s'", content, result.ForUser)
	}

	if result.Silent {
		t.Error("Expected Silent to be false")
	}

	if result.IsError {
		t.Error("Expected IsError to be false")
	}

	if result.Async {
		t.Error("Expected Async to be false")
	}
}

func TestToolResult_WithError(t *testing.T) {
	err := errors.New("underlying error")
	result := ErrorResult("error message").WithError(err)

	if result.Err != err {
		t.Error("Expected error to be set")
	}

	if result.ForLLM != "error message" {
		t.Errorf("Expected ForLLM 'error message', got '%s'", result.ForLLM)
	}
}

func TestToolResult_WithError_Chaining(t *testing.T) {
	err := errors.New("test error")
	result := NewToolResult("success").WithError(err)

	if result.Err != err {
		t.Error("Error should be set even for non-error results")
	}

	if result.ForLLM != "success" {
		t.Errorf("ForLLM should not change: got '%s'", result.ForLLM)
	}
}

func TestToolResult_MarshalJSON(t *testing.T) {
	result := &ToolResult{
		ForLLM:  "content for LLM",
		ForUser: "content for user",
		Silent:  false,
		IsError: false,
		Async:   false,
		Err:     errors.New("internal error"),
	}

	// MarshalJSON should exclude the Err field (due to json:"-" tag)
	data, err := json.Marshal(result)
	if err != nil {
		t.Fatalf("Failed to marshal: %v", err)
	}

	var unmarshaled map[string]interface{}
	if err := json.Unmarshal(data, &unmarshaled); err != nil {
		t.Fatalf("Failed to unmarshal: %v", err)
	}

	// Check that Err field is not in JSON
	if _, exists := unmarshaled["Err"]; exists {
		t.Error("Err field should not be in JSON (has json:- tag)")
	}

	// Check other fields are present
	if unmarshaled["for_llm"] != "content for LLM" {
		t.Errorf("Expected for_llm 'content for LLM', got '%v'", unmarshaled["for_llm"])
	}

	if unmarshaled["for_user"] != "content for user" {
		t.Errorf("Expected for_user 'content for user', got '%v'", unmarshaled["for_user"])
	}
}

func TestToolResult_AllFieldsSet(t *testing.T) {
	result := &ToolResult{
		ForLLM:  "llm content",
		ForUser: "user content",
		Silent:  true,
		IsError: true,
		Async:   false,
	}

	// Verify all fields
	if result.ForLLM != "llm content" {
		t.Errorf("ForLLM mismatch: got '%s'", result.ForLLM)
	}

	if result.ForUser != "user content" {
		t.Errorf("ForUser mismatch: got '%s'", result.ForUser)
	}

	if !result.Silent {
		t.Error("Silent should be true")
	}

	if !result.IsError {
		t.Error("IsError should be true")
	}

	if result.Async {
		t.Error("Async should be false")
	}
}

func TestToolResult_SilentOverridesUser(t *testing.T) {
	// When Silent is true, ForUser should be ignored
	result := &ToolResult{
		ForLLM:  "content",
		ForUser: "this should be ignored",
		Silent:  true,
		IsError: false,
		Async:   false,
	}

	if !result.Silent {
		t.Error("Silent should be true")
	}

	// The ForUser field is set, but the Silent flag should take precedence
	// This is application logic, not enforced by the struct
}

func TestToolResult_ErrorCases(t *testing.T) {
	tests := []struct {
		name    string
		result  *ToolResult
		wantErr bool
	}{
		{
			name:    "success result",
			result:  NewToolResult("success"),
			wantErr: false,
		},
		{
			name:    "error result",
			result:  ErrorResult("error"),
			wantErr: true,
		},
		{
			name:    "async result",
			result:  AsyncResult("async"),
			wantErr: false,
		},
		{
			name:    "silent result",
			result:  SilentResult("silent"),
			wantErr: false,
		},
		{
			name:    "user result",
			result:  UserResult("user message"),
			wantErr: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if tt.result.IsError != tt.wantErr {
				t.Errorf("IsError = %v, want %v", tt.result.IsError, tt.wantErr)
			}
		})
	}
}

func TestToolResult_AsyncCases(t *testing.T) {
	tests := []struct {
		name      string
		result    *ToolResult
		wantAsync bool
	}{
		{
			name:      "normal result",
			result:    NewToolResult("normal"),
			wantAsync: false,
		},
		{
			name:      "async result",
			result:    AsyncResult("async"),
			wantAsync: true,
		},
		{
			name:      "error result",
			result:    ErrorResult("error"),
			wantAsync: false,
		},
		{
			name:      "silent result",
			result:    SilentResult("silent"),
			wantAsync: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if tt.result.Async != tt.wantAsync {
				t.Errorf("Async = %v, want %v", tt.result.Async, tt.wantAsync)
			}
		})
	}
}

func TestToolResult_SilentCases(t *testing.T) {
	tests := []struct {
		name       string
		result     *ToolResult
		wantSilent bool
	}{
		{
			name:       "normal result",
			result:     NewToolResult("normal"),
			wantSilent: false,
		},
		{
			name:       "silent result",
			result:     SilentResult("silent"),
			wantSilent: true,
		},
		{
			name:       "user result",
			result:     UserResult("user"),
			wantSilent: false,
		},
		{
			name:       "error result",
			result:     ErrorResult("error"),
			wantSilent: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if tt.result.Silent != tt.wantSilent {
				t.Errorf("Silent = %v, want %v", tt.result.Silent, tt.wantSilent)
			}
		})
	}
}

func TestToolResult_EmptyContent(t *testing.T) {
	result := NewToolResult("")

	if result.ForLLM != "" {
		t.Errorf("Expected empty ForLLM, got '%s'", result.ForLLM)
	}

	// Empty content is valid, no error
	if result.IsError {
		t.Error("Empty content should not be an error")
	}
}

func TestToolResult_WithNilError(t *testing.T) {
	result := NewToolResult("test").WithError(nil)

	if result.Err != nil {
		t.Error("Err should be nil when set to nil")
	}
}

func TestToolResult_MultipleWithError(t *testing.T) {
	result := NewToolResult("test")

	err1 := errors.New("error 1")
	result = result.WithError(err1)

	if result.Err != err1 {
		t.Error("First error should be set")
	}

	err2 := errors.New("error 2")
	result = result.WithError(err2)

	if result.Err != err2 {
		t.Error("Second error should replace first")
	}
}

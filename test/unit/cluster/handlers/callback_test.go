// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package handlers_test

import (
	"fmt"
	"sync"
	"testing"

	"github.com/276793422/NemesisBot/module/cluster/handlers"
)

// mockTaskCompleter records CompleteCallback calls
type mockTaskCompleter struct {
	completed map[string]string // taskID → status
	errors    map[string]string // taskID → error message
	mu        sync.Mutex
}

func newMockTaskCompleter() *mockTaskCompleter {
	return &mockTaskCompleter{
		completed: make(map[string]string),
		errors:    make(map[string]string),
	}
}

func (m *mockTaskCompleter) CompleteCallback(taskID string, status string, response string, errMsg string) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	if status == "error" && errMsg != "" {
		m.errors[taskID] = errMsg
	}
	m.completed[taskID] = status
	return nil
}

// mockFailingTaskCompleter always returns error
type mockFailingTaskCompleter struct{}

func (m *mockFailingTaskCompleter) CompleteCallback(taskID string, status string, response string, errMsg string) error {
	return fmt.Errorf("task not found: %s", taskID)
}

// mockCallbackLogger records log messages
type mockCallbackLogger struct {
	messages []string
	mu       sync.Mutex
}

func (m *mockCallbackLogger) LogRPCInfo(msg string, args ...interface{}) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.messages = append(m.messages, fmt.Sprintf("INFO: "+msg, args...))
}
func (m *mockCallbackLogger) LogRPCError(msg string, args ...interface{}) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.messages = append(m.messages, fmt.Sprintf("ERROR: "+msg, args...))
}
func (m *mockCallbackLogger) LogRPCDebug(msg string, args ...interface{}) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.messages = append(m.messages, fmt.Sprintf("DEBUG: "+msg, args...))
}

// TestCallbackHandler_Success tests successful callback
func TestCallbackHandler_Success(t *testing.T) {
	logger := &mockCallbackLogger{}
	completer := newMockTaskCompleter()

	var registeredHandler func(map[string]interface{}) (map[string]interface{}, error)
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if action == "peer_chat_callback" {
			registeredHandler = handler
		}
	}

	handlers.RegisterCallbackHandler(logger, completer, registrar)

	if registeredHandler == nil {
		t.Fatal("Handler was not registered")
	}

	// Simulate callback
	result, err := registeredHandler(map[string]interface{}{
		"task_id":  "task-123",
		"status":   "success",
		"response": "Hello from B!",
	})
	if err != nil {
		t.Fatalf("Handler returned error: %v", err)
	}

	if result["status"] != "received" {
		t.Errorf("Expected status 'received', got '%v'", result["status"])
	}
	if result["task_id"] != "task-123" {
		t.Errorf("Expected task_id 'task-123', got '%v'", result["task_id"])
	}

	// Verify completer was called
	completer.mu.Lock()
	if completer.completed["task-123"] != "success" {
		t.Errorf("Expected completer to record task-123 as success, got '%v'", completer.completed["task-123"])
	}
	completer.mu.Unlock()
}

// TestCallbackHandler_Error tests error callback
func TestCallbackHandler_Error(t *testing.T) {
	logger := &mockCallbackLogger{}
	completer := newMockTaskCompleter()

	var registeredHandler func(map[string]interface{}) (map[string]interface{}, error)
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if action == "peer_chat_callback" {
			registeredHandler = handler
		}
	}

	handlers.RegisterCallbackHandler(logger, completer, registrar)

	result, err := registeredHandler(map[string]interface{}{
		"task_id": "task-456",
		"status":  "error",
		"error":   "LLM processing failed",
	})
	if err != nil {
		t.Fatalf("Handler returned error: %v", err)
	}

	if result["status"] != "received" {
		t.Errorf("Expected status 'received', got '%v'", result["status"])
	}

	completer.mu.Lock()
	if completer.completed["task-456"] != "error" {
		t.Errorf("Expected completer to record task-456 as error, got '%v'", completer.completed["task-456"])
	}
	if completer.errors["task-456"] != "LLM processing failed" {
		t.Errorf("Expected error 'LLM processing failed', got '%v'", completer.errors["task-456"])
	}
	completer.mu.Unlock()
}

// TestCallbackHandler_MissingTaskID tests callback without task_id
func TestCallbackHandler_MissingTaskID(t *testing.T) {
	logger := &mockCallbackLogger{}
	completer := newMockTaskCompleter()

	var registeredHandler func(map[string]interface{}) (map[string]interface{}, error)
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if action == "peer_chat_callback" {
			registeredHandler = handler
		}
	}

	handlers.RegisterCallbackHandler(logger, completer, registrar)

	result, err := registeredHandler(map[string]interface{}{
		"status": "success",
	})
	if err != nil {
		t.Fatalf("Handler returned error: %v", err)
	}

	if result["status"] != "error" {
		t.Errorf("Expected status 'error', got '%v'", result["status"])
	}
}

// TestCallbackHandler_TaskNotFound tests callback for unknown task
func TestCallbackHandler_TaskNotFound(t *testing.T) {
	logger := &mockCallbackLogger{}
	completer := &mockFailingTaskCompleter{}

	var registeredHandler func(map[string]interface{}) (map[string]interface{}, error)
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if action == "peer_chat_callback" {
			registeredHandler = handler
		}
	}

	handlers.RegisterCallbackHandler(logger, completer, registrar)

	result, err := registeredHandler(map[string]interface{}{
		"task_id":  "unknown-task",
		"status":   "success",
		"response": "test",
	})
	if err != nil {
		t.Fatalf("Handler returned error: %v", err)
	}

	// Should return error status (task not found)
	if result["status"] != "error" {
		t.Errorf("Expected status 'error', got '%v'", result["status"])
	}
}

// TestCallbackHandler_MissingStatus tests callback with missing status defaults to error
func TestCallbackHandler_MissingStatus(t *testing.T) {
	logger := &mockCallbackLogger{}
	completer := newMockTaskCompleter()

	var registeredHandler func(map[string]interface{}) (map[string]interface{}, error)
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if action == "peer_chat_callback" {
			registeredHandler = handler
		}
	}

	handlers.RegisterCallbackHandler(logger, completer, registrar)

	result, err := registeredHandler(map[string]interface{}{
		"task_id":  "task-no-status",
		"response": "some response",
	})
	if err != nil {
		t.Fatalf("Handler returned error: %v", err)
	}

	// Should still succeed (defaults to error status)
	if result["status"] != "received" {
		t.Errorf("Expected status 'received', got '%v'", result["status"])
	}

	completer.mu.Lock()
	if completer.completed["task-no-status"] != "error" { // defaults to error
		t.Errorf("Expected completer to record task-no-status as error (default), got '%v'", completer.completed["task-no-status"])
	}
	completer.mu.Unlock()
}

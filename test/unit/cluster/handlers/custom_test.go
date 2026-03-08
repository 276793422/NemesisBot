// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package handlers_test

import (
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/cluster/handlers"
)

// TestRegisterCustomHandlers tests that all custom handlers are registered
func TestRegisterCustomHandlers(t *testing.T) {
	mockCluster := &mockClusterForHandlers{
		nodeID:       "test-node-1",
		address:      "127.0.0.1:21950",
		capabilities: []string{"hello"},
		logMessages:  []string{},
	}

	registeredHandlers := make(map[string]bool)

	// Create a registrar that tracks registered handlers
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		registeredHandlers[action] = true
	}

	// Register custom handlers
	handlers.RegisterCustomHandlers(mockCluster, mockCluster.GetNodeID, registrar)

	// Verify hello handler is registered
	if !registeredHandlers["hello"] {
		t.Error("Handler 'hello' was not registered")
	}

	// Verify log message was written
	if len(mockCluster.logMessages) == 0 {
		t.Error("Expected log message to be written")
	}
}

// TestHelloHandler tests the hello handler response
func TestHelloHandler(t *testing.T) {
	mockCluster := &mockClusterForHandlers{
		nodeID:       "test-node-1",
		address:      "127.0.0.1:21950",
		capabilities: []string{"hello"},
		logMessages:  []string{},
	}

	var helloHandler func(map[string]interface{}) (map[string]interface{}, error)

	// Capture the hello handler
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if action == "hello" {
			helloHandler = handler
		}
	}

	handlers.RegisterCustomHandlers(mockCluster, mockCluster.GetNodeID, registrar)

	if helloHandler == nil {
		t.Fatal("hello handler was not registered")
	}

	// Test hello handler with valid payload
	payload := map[string]interface{}{
		"from":      "remote-node-2",
		"timestamp": time.Now().Format(time.RFC3339),
	}

	response, err := helloHandler(payload)
	if err != nil {
		t.Errorf("hello handler returned error: %v", err)
	}

	// Verify response fields
	if response["status"] != "ok" {
		t.Errorf("Expected status 'ok', got '%s'", response["status"])
	}

	if response["node_id"] != "test-node-1" {
		t.Errorf("Expected node_id 'test-node-1', got '%s'", response["node_id"])
	}

	greeting, ok := response["greeting"].(string)
	if !ok {
		t.Error("Expected greeting to be a string")
	} else if greeting == "" {
		t.Error("Expected non-empty greeting")
	}

	timestamp, ok := response["timestamp"].(string)
	if !ok {
		t.Error("Expected timestamp to be a string")
	} else if timestamp == "" {
		t.Error("Expected non-empty timestamp")
	}
}

// TestHelloHandlerWithEmptyPayload tests hello handler with empty payload
func TestHelloHandlerWithEmptyPayload(t *testing.T) {
	mockCluster := &mockClusterForHandlers{
		nodeID:       "test-node-1",
		address:      "127.0.0.1:21950",
		capabilities: []string{"hello"},
		logMessages:  []string{},
	}

	var helloHandler func(map[string]interface{}) (map[string]interface{}, error)

	// Capture the hello handler
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if action == "hello" {
			helloHandler = handler
		}
	}

	handlers.RegisterCustomHandlers(mockCluster, mockCluster.GetNodeID, registrar)

	if helloHandler == nil {
		t.Fatal("hello handler was not registered")
	}

	// Test hello handler with empty payload
	response, err := helloHandler(map[string]interface{}{})
	if err != nil {
		t.Errorf("hello handler returned error: %v", err)
	}

	// Should still return valid response even with empty payload
	if response["status"] != "ok" {
		t.Errorf("Expected status 'ok', got '%s'", response["status"])
	}

	greeting, ok := response["greeting"].(string)
	if !ok {
		t.Error("Expected greeting to be a string")
	} else {
		// With empty from field, greeting should still be valid
		if greeting == "" {
			t.Error("Expected non-empty greeting even with empty payload")
		}
	}
}

// TestHelloHandlerLogsMessage tests that hello handler logs messages
func TestHelloHandlerLogsMessage(t *testing.T) {
	mockCluster := &mockClusterForHandlers{
		nodeID:       "test-node-1",
		address:      "127.0.0.1:21950",
		capabilities: []string{"hello"},
		logMessages:  []string{},
	}

	var helloHandler func(map[string]interface{}) (map[string]interface{}, error)

	// Capture the hello handler
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if action == "hello" {
			helloHandler = handler
		}
	}

	handlers.RegisterCustomHandlers(mockCluster, mockCluster.GetNodeID, registrar)

	if helloHandler == nil {
		t.Fatal("hello handler was not registered")
	}

	// Clear log messages from registration
	mockCluster.logMessages = []string{}

	// Call handler
	payload := map[string]interface{}{
		"from": "remote-node-2",
	}

	_, err := helloHandler(payload)
	if err != nil {
		t.Errorf("hello handler returned error: %v", err)
	}

	// Verify log messages were written
	if len(mockCluster.logMessages) < 2 {
		t.Errorf("Expected at least 2 log messages, got %d", len(mockCluster.logMessages))
	}

	// Check for expected log content
	foundReceived := false
	foundSending := false
	for _, msg := range mockCluster.logMessages {
		if contains(msg, "Received hello from") {
			foundReceived = true
		}
		if contains(msg, "Sending response to") {
			foundSending = true
		}
	}

	if !foundReceived {
		t.Error("Expected log message about receiving hello")
	}

	if !foundSending {
		t.Error("Expected log message about sending response")
	}
}

// TestHelloHandlerGreetingFormat tests the format of the greeting message
func TestHelloHandlerGreetingFormat(t *testing.T) {
	mockCluster := &mockClusterForHandlers{
		nodeID:      "test-node-1",
		logMessages: []string{},
	}

	var helloHandler func(map[string]interface{}) (map[string]interface{}, error)

	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if action == "hello" {
			helloHandler = handler
		}
	}

	handlers.RegisterCustomHandlers(mockCluster, mockCluster.GetNodeID, registrar)

	if helloHandler == nil {
		t.Fatal("hello handler was not registered")
	}

	tests := []struct {
		name           string
		from           string
		expectedSubstr string
	}{
		{
			name:           "Valid from field",
			from:           "remote-node-1",
			expectedSubstr: "remote-node-1",
		},
		{
			name:           "Another valid from field",
			from:           "bot-2",
			expectedSubstr: "bot-2",
		},
		{
			name:           "Empty from field",
			from:           "",
			expectedSubstr: "",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			payload := map[string]interface{}{
				"from": tt.from,
			}

			response, err := helloHandler(payload)
			if err != nil {
				t.Errorf("hello handler returned error: %v", err)
				return
			}

			greeting, ok := response["greeting"].(string)
			if !ok {
				t.Error("Expected greeting to be a string")
				return
			}

			if tt.expectedSubstr != "" && !contains(greeting, tt.expectedSubstr) {
				t.Errorf("Expected greeting to contain '%s', got '%s'", tt.expectedSubstr, greeting)
			}
		})
	}
}

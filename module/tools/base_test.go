// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package tools

import (
	"context"
	"testing"
)

func TestToolToSchema(t *testing.T) {
	mockTool := &MockToolForBase{
		name:        "test_tool",
		description: "A test tool",
		parameters: map[string]interface{}{
			"type": "object",
			"properties": map[string]interface{}{
				"input": map[string]interface{}{
					"type":        "string",
					"description": "Test input",
				},
			},
			"required": []string{"input"},
		},
	}

	schema := ToolToSchema(mockTool)

	if schema["type"] != "function" {
		t.Errorf("Expected type 'function', got '%v'", schema["type"])
	}

	fn, ok := schema["function"].(map[string]interface{})
	if !ok {
		t.Fatal("Function should be a map")
	}

	if fn["name"] != "test_tool" {
		t.Errorf("Expected name 'test_tool', got '%v'", fn["name"])
	}

	if fn["description"] != "A test tool" {
		t.Errorf("Expected description 'A test tool', got '%v'", fn["description"])
	}

	if fn["parameters"] == nil {
		t.Error("Parameters should not be nil")
	}
}

func TestContextualTool_SetContext(t *testing.T) {
	tool := &ContextualMockToolForBase{}

	channel := "test_channel"
	chatID := "test_chat"

	tool.SetContext(channel, chatID)

	if tool.channel != channel {
		t.Errorf("Expected channel '%s', got '%s'", channel, tool.channel)
	}

	if tool.chatID != chatID {
		t.Errorf("Expected chatID '%s', got '%s'", chatID, tool.chatID)
	}
}

func TestAsyncTool_SetCallback(t *testing.T) {
	tool := &AsyncMockToolForBase{}

	callbackCalled := false
	callback := func(ctx context.Context, result *ToolResult) {
		callbackCalled = true
	}

	tool.SetCallback(callback)

	if tool.callback == nil {
		t.Error("Callback should have been set")
	}

	// Test callback invocation
	ctx := context.Background()
	result := NewToolResult("test")
	tool.callback(ctx, result)

	if !callbackCalled {
		t.Error("Callback should have been called")
	}
}

// Mock implementations for testing

type MockToolForBase struct {
	name        string
	description string
	parameters  map[string]interface{}
}

func (m *MockToolForBase) Name() string {
	return m.name
}

func (m *MockToolForBase) Description() string {
	return m.description
}

func (m *MockToolForBase) Parameters() map[string]interface{} {
	return m.parameters
}

func (m *MockToolForBase) Execute(ctx context.Context, args map[string]interface{}) *ToolResult {
	return NewToolResult("mock result")
}

type ContextualMockToolForBase struct {
	*MockToolForBase
	channel string
	chatID  string
}

func (c *ContextualMockToolForBase) SetContext(channel, chatID string) {
	c.channel = channel
	c.chatID = chatID
}

type AsyncMockToolForBase struct {
	*MockToolForBase
	callback AsyncCallback
}

func (a *AsyncMockToolForBase) SetCallback(cb AsyncCallback) {
	a.callback = cb
}

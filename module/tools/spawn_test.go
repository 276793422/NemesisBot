// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package tools

import (
	"context"
	"testing"
)

func TestSpawnTool_Name(t *testing.T) {
	tool := &SpawnTool{} // Create without manager for basic tests

	if tool.Name() != "spawn" {
		t.Errorf("Expected name 'spawn', got '%s'", tool.Name())
	}
}

func TestSpawnTool_Description(t *testing.T) {
	tool := &SpawnTool{}

	desc := tool.Description()
	if desc == "" {
		t.Error("Description should not be empty")
	}
}

func TestSpawnTool_Parameters(t *testing.T) {
	tool := &SpawnTool{}

	params := tool.Parameters()

	if params == nil {
		t.Fatal("Parameters should not be nil")
	}

	if params["type"] != "object" {
		t.Errorf("Expected type 'object', got '%v'", params["type"])
	}

	props, ok := params["properties"].(map[string]interface{})
	if !ok {
		t.Fatal("Properties should be a map")
	}

	// Check required parameters
	required, ok := params["required"].([]string)
	if !ok {
		t.Fatal("Required should be a string slice")
	}

	if len(required) != 1 || required[0] != "task" {
		t.Errorf("Expected only 'task' to be required, got %v", required)
	}

	// Check properties
	if _, ok := props["task"]; !ok {
		t.Error("task should be in properties")
	}
	if _, ok := props["label"]; !ok {
		t.Error("label should be in properties")
	}
	if _, ok := props["agent_id"]; !ok {
		t.Error("agent_id should be in properties")
	}
}

func TestSpawnTool_Execute_MissingTask(t *testing.T) {
	tool := &SpawnTool{}
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"label": "Test",
	})

	if !result.IsError {
		t.Error("Expected error for missing task parameter")
	}

	if result.ForLLM != "task is required" {
		t.Errorf("Expected 'task is required' error, got '%s'", result.ForLLM)
	}
}

func TestSpawnTool_Execute_NilManager(t *testing.T) {
	tool := &SpawnTool{}
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"task": "Test task",
	})

	if !result.IsError {
		t.Error("Expected error when manager is nil")
	}

	if result.ForLLM != "Subagent manager not configured" {
		t.Errorf("Expected 'Subagent manager not configured' error, got '%s'", result.ForLLM)
	}
}

func TestSpawnTool_SetCallback(t *testing.T) {
	tool := &SpawnTool{}

	callbackCalled := false
	callback := func(ctx context.Context, result *ToolResult) {
		callbackCalled = true
	}

	tool.SetCallback(callback)

	if tool.callback == nil {
		t.Error("Callback should have been set")
	}

	// Test callback is set
	ctx := context.Background()
	result := NewToolResult("test")
	tool.callback(ctx, result)

	if !callbackCalled {
		t.Error("Callback should have been called")
	}
}

func TestSpawnTool_SetContext(t *testing.T) {
	tool := &SpawnTool{}

	tool.SetContext("test_channel", "test_chat")

	if tool.originChannel != "test_channel" {
		t.Errorf("Expected originChannel 'test_channel', got '%s'", tool.originChannel)
	}

	if tool.originChatID != "test_chat" {
		t.Errorf("Expected originChatID 'test_chat', got '%s'", tool.originChatID)
	}
}

func TestSpawnTool_SetAllowlistChecker(t *testing.T) {
	tool := &SpawnTool{}

	checker := func(agentID string) bool {
		return agentID == "allowed"
	}

	tool.SetAllowlistChecker(checker)

	if tool.allowlistCheck == nil {
		t.Error("Allowlist checker should have been set")
	}

	// Test the checker
	if !tool.allowlistCheck("allowed") {
		t.Error("Checker should return true for 'allowed'")
	}

	if tool.allowlistCheck("blocked") {
		t.Error("Checker should return false for 'blocked'")
	}
}

// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package tools_test

import (
	"context"
	"sync"
	"testing"

	. "github.com/276793422/NemesisBot/module/tools"
)

// MockTool is a simple test tool implementation
type MockTool struct {
	name        string
	description string
	parameters  map[string]interface{}
	executeFunc func(ctx context.Context, args map[string]interface{}) *ToolResult
}

func NewMockTool(name string) *MockTool {
	return &MockTool{
		name:        name,
		description: "Mock tool for testing",
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
		executeFunc: func(ctx context.Context, args map[string]interface{}) *ToolResult {
			return NewToolResult("mock result")
		},
	}
}

func (m *MockTool) Name() string {
	return m.name
}

func (m *MockTool) Description() string {
	return m.description
}

func (m *MockTool) Parameters() map[string]interface{} {
	return m.parameters
}

func (m *MockTool) Execute(ctx context.Context, args map[string]interface{}) *ToolResult {
	if m.executeFunc != nil {
		return m.executeFunc(ctx, args)
	}
	return NewToolResult("mock result")
}

// ContextualMockTool implements ContextualTool
type ContextualMockTool struct {
	*MockTool
	channel string
	chatID  string
}

func NewContextualMockTool(name string) *ContextualMockTool {
	return &ContextualMockTool{
		MockTool: NewMockTool(name),
	}
}

func (c *ContextualMockTool) SetContext(channel, chatID string) {
	c.channel = channel
	c.chatID = chatID
}

// AsyncMockTool implements AsyncTool
type AsyncMockTool struct {
	*MockTool
	callback AsyncCallback
}

func NewAsyncMockTool(name string) *AsyncMockTool {
	return &AsyncMockTool{
		MockTool: NewMockTool(name),
	}
}

func (a *AsyncMockTool) SetCallback(cb AsyncCallback) {
	a.callback = cb
}

func TestNewToolRegistry(t *testing.T) {
	registry := NewToolRegistry()

	if registry == nil {
		t.Fatal("NewToolRegistry returned nil")
	}

	if registry.Count() != 0 {
		t.Errorf("Expected empty registry, got %d tools", registry.Count())
	}
}

func TestToolRegistry_Register(t *testing.T) {
	registry := NewToolRegistry()
	tool := NewMockTool("test_tool")

	registry.Register(tool)

	if registry.Count() != 1 {
		t.Errorf("Expected 1 tool, got %d", registry.Count())
	}

	retrieved, ok := registry.Get("test_tool")
	if !ok {
		t.Fatal("Tool not found after registration")
	}

	if retrieved.Name() != "test_tool" {
		t.Errorf("Expected tool name 'test_tool', got '%s'", retrieved.Name())
	}
}

func TestToolRegistry_Register_Overwrite(t *testing.T) {
	registry := NewToolRegistry()
	tool1 := NewMockTool("test_tool")
	tool2 := NewMockTool("test_tool")

	tool1.description = "First tool"
	tool2.description = "Second tool"

	registry.Register(tool1)
	registry.Register(tool2)

	if registry.Count() != 1 {
		t.Errorf("Expected 1 tool after overwrite, got %d", registry.Count())
	}

	retrieved, _ := registry.Get("test_tool")
	if retrieved.Description() != "Second tool" {
		t.Errorf("Tool should have been overwritten, got description: %s", retrieved.Description())
	}
}

func TestToolRegistry_Get_NotFound(t *testing.T) {
	registry := NewToolRegistry()

	_, ok := registry.Get("nonexistent")
	if ok {
		t.Error("Expected false for non-existent tool")
	}
}

func TestToolRegistry_List(t *testing.T) {
	registry := NewToolRegistry()
	registry.Register(NewMockTool("tool1"))
	registry.Register(NewMockTool("tool2"))
	registry.Register(NewMockTool("tool3"))

	names := registry.List()

	if len(names) != 3 {
		t.Errorf("Expected 3 tool names, got %d", len(names))
	}

	// Check that all tools are present
	toolMap := make(map[string]bool)
	for _, name := range names {
		toolMap[name] = true
	}

	if !toolMap["tool1"] || !toolMap["tool2"] || !toolMap["tool3"] {
		t.Error("Not all tools were returned in List()")
	}
}

func TestToolRegistry_Count(t *testing.T) {
	registry := NewToolRegistry()

	if registry.Count() != 0 {
		t.Errorf("Expected 0 tools, got %d", registry.Count())
	}

	registry.Register(NewMockTool("tool1"))
	if registry.Count() != 1 {
		t.Errorf("Expected 1 tool, got %d", registry.Count())
	}

	registry.Register(NewMockTool("tool2"))
	registry.Register(NewMockTool("tool3"))
	if registry.Count() != 3 {
		t.Errorf("Expected 3 tools, got %d", registry.Count())
	}
}

func TestToolRegistry_GetSummaries(t *testing.T) {
	registry := NewToolRegistry()
	tool1 := NewMockTool("tool1")
	tool1.description = "Description 1"
	tool2 := NewMockTool("tool2")
	tool2.description = "Description 2"

	registry.Register(tool1)
	registry.Register(tool2)

	summaries := registry.GetSummaries()

	if len(summaries) != 2 {
		t.Errorf("Expected 2 summaries, got %d", len(summaries))
	}

	// Check format
	for _, summary := range summaries {
		if len(summary) == 0 {
			t.Error("Summary should not be empty")
		}
	}
}

func TestToolRegistry_Execute_Success(t *testing.T) {
	registry := NewToolRegistry()
	tool := NewMockTool("test_tool")
	tool.executeFunc = func(ctx context.Context, args map[string]interface{}) *ToolResult {
		return NewToolResult("execution successful")
	}

	registry.Register(tool)

	ctx := context.Background()
	result := registry.Execute(ctx, "test_tool", map[string]interface{}{"input": "test"})

	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}

	if result.ForLLM != "execution successful" {
		t.Errorf("Expected 'execution successful', got '%s'", result.ForLLM)
	}
}

func TestToolRegistry_Execute_ToolNotFound(t *testing.T) {
	registry := NewToolRegistry()

	ctx := context.Background()
	result := registry.Execute(ctx, "nonexistent", map[string]interface{}{})

	if !result.IsError {
		t.Error("Expected error for non-existent tool")
	}

	if result.ForLLM == "" {
		t.Error("Error message should not be empty")
	}
}

func TestToolRegistry_ExecuteWithContext_ContextualTool(t *testing.T) {
	registry := NewToolRegistry()
	tool := NewContextualMockTool("contextual_tool")

	registry.Register(tool)

	ctx := context.Background()
	result := registry.ExecuteWithContext(ctx, "contextual_tool", nil, "test_channel", "test_chat", nil)

	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}

	if tool.channel != "test_channel" {
		t.Errorf("Expected channel 'test_channel', got '%s'", tool.channel)
	}

	if tool.chatID != "test_chat" {
		t.Errorf("Expected chatID 'test_chat', got '%s'", tool.chatID)
	}
}

func TestToolRegistry_ExecuteWithContext_AsyncTool(t *testing.T) {
	registry := NewToolRegistry()
	tool := NewAsyncMockTool("async_tool")

	callbackCalled := false
	var callbackResult *ToolResult

	callback := func(ctx context.Context, result *ToolResult) {
		callbackCalled = true
		callbackResult = result
	}

	registry.Register(tool)

	ctx := context.Background()
	result := registry.ExecuteWithContext(ctx, "async_tool", nil, "", "", callback)

	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}

	if tool.callback == nil {
		t.Error("Callback should have been set on async tool")
	}

	// Note: callback is set but not called in this test since we're just verifying setup
	_ = callbackCalled
	_ = callbackResult
}

func TestToolRegistry_GetDefinitions(t *testing.T) {
	registry := NewToolRegistry()
	tool := NewMockTool("test_tool")

	registry.Register(tool)

	definitions := registry.GetDefinitions()

	if len(definitions) != 1 {
		t.Errorf("Expected 1 definition, got %d", len(definitions))
	}

	def := definitions[0]
	if def["type"] != "function" {
		t.Errorf("Expected type 'function', got '%v'", def["type"])
	}

	fn, ok := def["function"].(map[string]interface{})
	if !ok {
		t.Fatal("Function should be a map")
	}

	if fn["name"] != "test_tool" {
		t.Errorf("Expected name 'test_tool', got '%v'", fn["name"])
	}

	if fn["description"] != "Mock tool for testing" {
		t.Errorf("Unexpected description: %v", fn["description"])
	}

	if fn["parameters"] == nil {
		t.Error("Parameters should not be nil")
	}
}

func TestToolRegistry_ConcurrentAccess(t *testing.T) {
	registry := NewToolRegistry()
	const numGoroutines = 100
	const numTools = 50

	var wg sync.WaitGroup
	wg.Add(numGoroutines)

	// Concurrent registration
	for i := 0; i < numGoroutines/2; i++ {
		go func(idx int) {
			defer wg.Done()
			tool := NewMockTool("tool" + string(rune('0'+idx%10)))
			registry.Register(tool)
		}(i)
	}

	// Concurrent reads
	for i := 0; i < numGoroutines/2; i++ {
		go func(idx int) {
			defer wg.Done()
			registry.Get("tool" + string(rune('0'+idx%10)))
			registry.List()
			registry.Count()
		}(i)
	}

	wg.Wait()

	// Registry should still be consistent
	count := registry.Count()
	list := registry.List()

	if len(list) != count {
		t.Errorf("List length (%d) should match count (%d)", len(list), count)
	}
}

func TestToolRegistry_Execute_Concurrent(t *testing.T) {
	registry := NewToolRegistry()
	tool := NewMockTool("concurrent_tool")
	callCount := 0
	var mu sync.Mutex

	tool.executeFunc = func(ctx context.Context, args map[string]interface{}) *ToolResult {
		mu.Lock()
		callCount++
		mu.Unlock()
		return NewToolResult("concurrent result")
	}

	registry.Register(tool)

	const numGoroutines = 50
	var wg sync.WaitGroup
	wg.Add(numGoroutines)

	for i := 0; i < numGoroutines; i++ {
		go func() {
			defer wg.Done()
			ctx := context.Background()
			registry.Execute(ctx, "concurrent_tool", map[string]interface{}{})
		}()
	}

	wg.Wait()

	if callCount != numGoroutines {
		t.Errorf("Expected %d executions, got %d", numGoroutines, callCount)
	}
}

func TestToolToSchema(t *testing.T) {
	tool := NewMockTool("test_tool")
	tool.description = "Test description"

	schema := ToolToSchema(tool)

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

	if fn["description"] != "Test description" {
		t.Errorf("Expected 'Test description', got '%v'", fn["description"])
	}

	if fn["parameters"] == nil {
		t.Error("Parameters should not be nil")
	}
}

func TestToolRegistry_EmptyRegistry(t *testing.T) {
	registry := NewToolRegistry()

	// Test operations on empty registry
	if registry.Count() != 0 {
		t.Error("New registry should be empty")
	}

	names := registry.List()
	if len(names) != 0 {
		t.Error("List should be empty for new registry")
	}

	definitions := registry.GetDefinitions()
	if len(definitions) != 0 {
		t.Error("Definitions should be empty for new registry")
	}

	summaries := registry.GetSummaries()
	if len(summaries) != 0 {
		t.Error("Summaries should be empty for new registry")
	}
}

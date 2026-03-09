// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package tools

import (
	"context"
	"fmt"
	"testing"
	"time"
)

// MockTool is a mock implementation of the Tool interface for testing
type MockTool struct {
	name        string
	description string
	parameters  map[string]interface{}
	executeFunc func(ctx context.Context, args map[string]interface{}) *ToolResult
}

func (m *MockTool) Name() string {
	if m.name != "" {
		return m.name
	}
	return "mock_tool"
}

func (m *MockTool) Description() string {
	if m.description != "" {
		return m.description
	}
	return "A mock tool for testing"
}

func (m *MockTool) Parameters() map[string]interface{} {
	if m.parameters != nil {
		return m.parameters
	}
	return map[string]interface{}{
		"type": "object",
		"properties": map[string]interface{}{
			"input": map[string]interface{}{
				"type":        "string",
				"description": "Input parameter",
			},
		},
	}
}

func (m *MockTool) Execute(ctx context.Context, args map[string]interface{}) *ToolResult {
	if m.executeFunc != nil {
		return m.executeFunc(ctx, args)
	}
	return NewToolResult("mock result")
}

// MockContextualTool implements both Tool and ContextualTool
type MockContextualTool struct {
	MockTool
	channel string
	chatID  string
}

func (m *MockContextualTool) SetContext(channel, chatID string) {
	m.channel = channel
	m.chatID = chatID
}

func (m *MockContextualTool) GetContext() (string, string) {
	return m.channel, m.chatID
}

// MockAsyncTool implements both Tool and AsyncTool
type MockAsyncTool struct {
	MockTool
	callback AsyncCallback
}

func (m *MockAsyncTool) SetCallback(callback AsyncCallback) {
	m.callback = callback
}

func (m *MockAsyncTool) GetCallback() AsyncCallback {
	return m.callback
}

func TestNewToolRegistry(t *testing.T) {
	registry := NewToolRegistry()
	if registry == nil {
		t.Fatal("NewToolRegistry() returned nil")
	}
	if registry.tools == nil {
		t.Error("NewToolRegistry() tools map is nil")
	}
	if registry.Count() != 0 {
		t.Errorf("NewToolRegistry() count = %d, want 0", registry.Count())
	}
}

func TestToolRegistry_Register(t *testing.T) {
	registry := NewToolRegistry()
	tool := &MockTool{name: "test_tool"}

	registry.Register(tool)

	if registry.Count() != 1 {
		t.Errorf("Count() = %d, want 1", registry.Count())
	}

	retrieved, ok := registry.Get("test_tool")
	if !ok {
		t.Error("Get() did not find registered tool")
	}
	if retrieved.Name() != "test_tool" {
		t.Errorf("Retrieved tool name = %v, want test_tool", retrieved.Name())
	}
}

func TestToolRegistry_Register_Overwrite(t *testing.T) {
	registry := NewToolRegistry()
	tool1 := &MockTool{name: "test_tool", description: "First"}
	tool2 := &MockTool{name: "test_tool", description: "Second"}

	registry.Register(tool1)
	registry.Register(tool2)

	if registry.Count() != 1 {
		t.Errorf("Count() = %d, want 1 (should overwrite)", registry.Count())
	}

	retrieved, _ := registry.Get("test_tool")
	if retrieved.Description() != "Second" {
		t.Errorf("Retrieved tool description = %v, want Second", retrieved.Description())
	}
}

func TestToolRegistry_Get_NotFound(t *testing.T) {
	registry := NewToolRegistry()
	_, ok := registry.Get("nonexistent")
	if ok {
		t.Error("Get() should return false for nonexistent tool")
	}
}

func TestToolRegistry_Execute_Success(t *testing.T) {
	registry := NewToolRegistry()
	tool := &MockTool{
		name: "test_tool",
		executeFunc: func(ctx context.Context, args map[string]interface{}) *ToolResult {
			return NewToolResult("executed successfully")
		},
	}
	registry.Register(tool)

	ctx := context.Background()
	result := registry.Execute(ctx, "test_tool", map[string]interface{}{"input": "test"})

	if result.IsError {
		t.Errorf("Execute() returned error: %s", result.ForLLM)
	}
	if result.ForLLM != "executed successfully" {
		t.Errorf("Execute() result = %v, want 'executed successfully'", result.ForLLM)
	}
}

func TestToolRegistry_Execute_ToolNotFound(t *testing.T) {
	registry := NewToolRegistry()
	ctx := context.Background()

	result := registry.Execute(ctx, "nonexistent", map[string]interface{}{})

	if !result.IsError {
		t.Error("Execute() should return error for nonexistent tool")
	}
	if result.ForLLM == "" {
		t.Error("Execute() error message should not be empty")
	}
}

func TestToolRegistry_ExecuteWithContext_ContextualTool(t *testing.T) {
	registry := NewToolRegistry()
	tool := &MockContextualTool{
		MockTool: MockTool{name: "contextual_tool"},
	}
	registry.Register(tool)

	ctx := context.Background()
	result := registry.ExecuteWithContext(ctx, "contextual_tool", nil, "test-channel", "chat-123", nil)

	if result.IsError {
		t.Errorf("ExecuteWithContext() returned error: %s", result.ForLLM)
	}

	channel, chatID := tool.GetContext()
	if channel != "test-channel" {
		t.Errorf("Tool channel = %v, want 'test-channel'", channel)
	}
	if chatID != "chat-123" {
		t.Errorf("Tool chatID = %v, want 'chat-123'", chatID)
	}
}

func TestToolRegistry_ExecuteWithContext_AsyncTool(t *testing.T) {
	registry := NewToolRegistry()
	callback := func(ctx context.Context, result *ToolResult) {
		// Callback received
	}

	tool := &MockAsyncTool{
		MockTool: MockTool{
			name: "async_tool",
			executeFunc: func(ctx context.Context, args map[string]interface{}) *ToolResult {
				return AsyncResult("async operation started")
			},
		},
	}
	registry.Register(tool)

	ctx := context.Background()
	result := registry.ExecuteWithContext(ctx, "async_tool", nil, "", "", callback)

	if result.IsError {
		t.Errorf("ExecuteWithContext() returned error: %s", result.ForLLM)
	}
	if !result.Async {
		t.Error("ExecuteWithContext() should return async result")
	}
	if tool.GetCallback() == nil {
		t.Error("AsyncTool callback should be set")
	}
}

func TestToolRegistry_Execute_Error(t *testing.T) {
	registry := NewToolRegistry()
	tool := &MockTool{
		name: "error_tool",
		executeFunc: func(ctx context.Context, args map[string]interface{}) *ToolResult {
			return ErrorResult("execution failed")
		},
	}
	registry.Register(tool)

	ctx := context.Background()
	result := registry.Execute(ctx, "error_tool", nil)

	if !result.IsError {
		t.Error("Execute() should return error result")
	}
}

func TestToolRegistry_Execute_Async(t *testing.T) {
	registry := NewToolRegistry()
	tool := &MockTool{
		name: "async_tool",
		executeFunc: func(ctx context.Context, args map[string]interface{}) *ToolResult {
			return AsyncResult("async started")
		},
	}
	registry.Register(tool)

	ctx := context.Background()
	result := registry.Execute(ctx, "async_tool", nil)

	if result.IsError {
		t.Errorf("Execute() returned error: %s", result.ForLLM)
	}
	if !result.Async {
		t.Error("Execute() should return async result")
	}
}

func TestToolRegistry_GetDefinitions(t *testing.T) {
	registry := NewToolRegistry()
	tool := &MockTool{
		name:        "test_tool",
		description: "Test tool description",
		parameters: map[string]interface{}{
			"type": "object",
			"properties": map[string]interface{}{
				"param1": map[string]interface{}{
					"type": "string",
				},
			},
		},
	}
	registry.Register(tool)

	definitions := registry.GetDefinitions()

	if len(definitions) != 1 {
		t.Errorf("GetDefinitions() returned %d definitions, want 1", len(definitions))
	}

	def := definitions[0]
	if fn, ok := def["function"].(map[string]interface{}); ok {
		if fn["name"] != "test_tool" {
			t.Errorf("Definition name = %v, want 'test_tool'", fn["name"])
		}
		if fn["description"] != "Test tool description" {
			t.Errorf("Definition description = %v, want 'Test tool description'", fn["description"])
		}
	} else {
		t.Error("Definition should have function field")
	}
}

func TestToolRegistry_GetDefinitions_Empty(t *testing.T) {
	registry := NewToolRegistry()
	definitions := registry.GetDefinitions()

	if len(definitions) != 0 {
		t.Errorf("GetDefinitions() returned %d definitions, want 0", len(definitions))
	}
}

func TestToolRegistry_ToProviderDefs(t *testing.T) {
	registry := NewToolRegistry()
	tool := &MockTool{
		name:        "test_tool",
		description: "Test tool",
		parameters: map[string]interface{}{
			"type": "object",
		},
	}
	registry.Register(tool)

	defs := registry.ToProviderDefs()

	if len(defs) != 1 {
		t.Errorf("ToProviderDefs() returned %d definitions, want 1", len(defs))
	}

	def := defs[0]
	if def.Type != "function" {
		t.Errorf("Definition type = %v, want 'function'", def.Type)
	}
	if def.Function.Name != "test_tool" {
		t.Errorf("Definition name = %v, want 'test_tool'", def.Function.Name)
	}
	if def.Function.Description != "Test tool" {
		t.Errorf("Definition description = %v, want 'Test tool'", def.Function.Description)
	}
}

func TestToolRegistry_List(t *testing.T) {
	registry := NewToolRegistry()
	registry.Register(&MockTool{name: "tool1"})
	registry.Register(&MockTool{name: "tool2"})
	registry.Register(&MockTool{name: "tool3"})

	names := registry.List()

	if len(names) != 3 {
		t.Errorf("List() returned %d names, want 3", len(names))
	}

	// Check that all names are present
	nameMap := make(map[string]bool)
	for _, name := range names {
		nameMap[name] = true
	}

	for _, expected := range []string{"tool1", "tool2", "tool3"} {
		if !nameMap[expected] {
			t.Errorf("List() missing tool: %s", expected)
		}
	}
}

func TestToolRegistry_List_Empty(t *testing.T) {
	registry := NewToolRegistry()
	names := registry.List()

	if len(names) != 0 {
		t.Errorf("List() returned %d names, want 0", len(names))
	}
}

func TestToolRegistry_Count(t *testing.T) {
	registry := NewToolRegistry()

	if registry.Count() != 0 {
		t.Errorf("Count() = %d, want 0", registry.Count())
	}

	registry.Register(&MockTool{name: "tool1"})
	if registry.Count() != 1 {
		t.Errorf("Count() = %d, want 1", registry.Count())
	}

	registry.Register(&MockTool{name: "tool2"})
	if registry.Count() != 2 {
		t.Errorf("Count() = %d, want 2", registry.Count())
	}
}

func TestToolRegistry_GetSummaries(t *testing.T) {
	registry := NewToolRegistry()
	registry.Register(&MockTool{
		name:        "tool1",
		description: "First tool",
	})
	registry.Register(&MockTool{
		name:        "tool2",
		description: "Second tool",
	})

	summaries := registry.GetSummaries()

	if len(summaries) != 2 {
		t.Errorf("GetSummaries() returned %d summaries, want 2", len(summaries))
	}

	// Check format
	for _, summary := range summaries {
		if len(summary) == 0 {
			t.Error("Summary should not be empty")
		}
	}
}

func TestToolRegistry_ConcurrentAccess(t *testing.T) {
	registry := NewToolRegistry()
	ctx := context.Background()

	done := make(chan bool)

	// Concurrent registers
	for i := 0; i < 10; i++ {
		go func(idx int) {
			registry.Register(&MockTool{name: fmt.Sprintf("tool%d", idx)})
			done <- true
		}(i)
	}

	// Concurrent reads
	for i := 0; i < 10; i++ {
		go func() {
			registry.List()
			registry.Count()
			registry.GetDefinitions()
			done <- true
		}()
	}

	// Concurrent executes
	for i := 0; i < 10; i++ {
		go func() {
			registry.Execute(ctx, "tool1", nil)
			done <- true
		}()
	}

	// Wait for all goroutines
	for i := 0; i < 30; i++ {
		<-done
	}

	// Verify final state
	if registry.Count() != 10 {
		t.Errorf("Final count = %d, want 10", registry.Count())
	}
}

func TestToolRegistry_ExecuteTiming(t *testing.T) {
	registry := NewToolRegistry()
	executionTime := 50 * time.Millisecond

	tool := &MockTool{
		name: "slow_tool",
		executeFunc: func(ctx context.Context, args map[string]interface{}) *ToolResult {
			time.Sleep(executionTime)
			return NewToolResult("done")
		},
	}
	registry.Register(tool)

	ctx := context.Background()
	start := time.Now()
	result := registry.Execute(ctx, "slow_tool", nil)
	elapsed := time.Since(start)

	if result.IsError {
		t.Errorf("Execute() returned error: %s", result.ForLLM)
	}

	if elapsed < executionTime {
		t.Errorf("Execute() completed too quickly: %v < %v", elapsed, executionTime)
	}
}

func TestPluginableTool_Name(t *testing.T) {
	tool := &MockTool{name: "test_tool"}
	pluginable := &PluginableTool{Tool: tool}

	if pluginable.Name() != "test_tool" {
		t.Errorf("Name() = %v, want 'test_tool'", pluginable.Name())
	}
}

func TestPluginableTool_Description(t *testing.T) {
	tool := &MockTool{description: "test description"}
	pluginable := &PluginableTool{Tool: tool}

	if pluginable.Description() != "test description" {
		t.Errorf("Description() = %v, want 'test description'", pluginable.Description())
	}
}

func TestPluginableTool_Parameters(t *testing.T) {
	params := map[string]interface{}{
		"type": "object",
		"properties": map[string]interface{}{
			"test": map[string]interface{}{
				"type": "string",
			},
		},
	}
	tool := &MockTool{parameters: params}
	pluginable := &PluginableTool{Tool: tool}

	result := pluginable.Parameters()
	if len(result) != len(params) {
		t.Errorf("Parameters() length = %d, want %d", len(result), len(params))
	}
}

func TestToolRegistry_RegisterWithPlugin(t *testing.T) {
	registry := NewToolRegistry()
	tool := &MockTool{name: "test_tool"}

	// Register without plugin manager (will wrap but plugin operations will be no-op)
	registry.RegisterWithPlugin(tool, nil, "user", "source", "workspace")

	retrieved, ok := registry.Get("test_tool")
	if !ok {
		t.Fatal("RegisterWithPlugin() did not register tool")
	}

	// Should be wrapped as PluginableTool
	if retrieved.Name() != "test_tool" {
		t.Errorf("Retrieved tool name = %v, want 'test_tool'", retrieved.Name())
	}
}

func TestToolRegistry_MultipleTools(t *testing.T) {
	registry := NewToolRegistry()
	tools := []*MockTool{
		{name: "tool1", description: "First"},
		{name: "tool2", description: "Second"},
		{name: "tool3", description: "Third"},
	}

	for _, tool := range tools {
		registry.Register(tool)
	}

	if registry.Count() != 3 {
		t.Errorf("Count() = %d, want 3", registry.Count())
	}

	for _, tool := range tools {
		retrieved, ok := registry.Get(tool.Name())
		if !ok {
			t.Errorf("Tool %s not found", tool.Name())
		}
		if retrieved.Description() != tool.Description() {
			t.Errorf("Tool %s description mismatch", tool.Name())
		}
	}
}

func TestToolRegistry_ExecuteWithArgs(t *testing.T) {
	registry := NewToolRegistry()
	var receivedArgs map[string]interface{}

	tool := &MockTool{
		name: "args_tool",
		executeFunc: func(ctx context.Context, args map[string]interface{}) *ToolResult {
			receivedArgs = args
			return NewToolResult("args received")
		},
	}
	registry.Register(tool)

	ctx := context.Background()
	testArgs := map[string]interface{}{
		"param1": "value1",
		"param2": 42,
		"param3": true,
	}
	result := registry.Execute(ctx, "args_tool", testArgs)

	if result.IsError {
		t.Errorf("Execute() returned error: %s", result.ForLLM)
	}

	if receivedArgs == nil {
		t.Fatal("Execute() did not pass args to tool")
	}

	if receivedArgs["param1"] != "value1" {
		t.Errorf("param1 = %v, want 'value1'", receivedArgs["param1"])
	}
	if receivedArgs["param2"] != 42 {
		t.Errorf("param2 = %v, want 42", receivedArgs["param2"])
	}
	if receivedArgs["param3"] != true {
		t.Errorf("param3 = %v, want true", receivedArgs["param3"])
	}
}

func TestToolRegistry_ContextCancellation(t *testing.T) {
	registry := NewToolRegistry()
	blockingTool := &MockTool{
		name: "blocking_tool",
		executeFunc: func(ctx context.Context, args map[string]interface{}) *ToolResult {
			<-ctx.Done()
			return ErrorResult("cancelled")
		},
	}
	registry.Register(blockingTool)

	ctx, cancel := context.WithCancel(context.Background())
	go func() {
		time.Sleep(10 * time.Millisecond)
		cancel()
	}()

	result := registry.Execute(ctx, "blocking_tool", nil)

	// Result should be error due to cancellation
	if !result.IsError {
		t.Error("Execute() should return error when context is cancelled")
	}
}

// Benchmark tests
func BenchmarkToolRegistry_Register(b *testing.B) {
	registry := NewToolRegistry()
	tool := &MockTool{name: "bench_tool"}

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		registry.Register(tool)
	}
}

func BenchmarkToolRegistry_Get(b *testing.B) {
	registry := NewToolRegistry()
	tool := &MockTool{name: "bench_tool"}
	registry.Register(tool)

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		registry.Get("bench_tool")
	}
}

func BenchmarkToolRegistry_Execute(b *testing.B) {
	registry := NewToolRegistry()
	tool := &MockTool{
		name: "bench_tool",
		executeFunc: func(ctx context.Context, args map[string]interface{}) *ToolResult {
			return NewToolResult("result")
		},
	}
	registry.Register(tool)
	ctx := context.Background()

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		registry.Execute(ctx, "bench_tool", nil)
	}
}

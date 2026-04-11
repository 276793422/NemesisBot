// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package tools

import (
	"context"
	"fmt"
	"sync"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/providers"
)

// MockLLMProvider is a mock implementation of providers.LLMProvider
type MockLLMProvider struct {
	responses []MockLLMResponse
	callCount int
	mu        sync.Mutex
}

type MockLLMResponse struct {
	Response *providers.LLMResponse
	Error    error
	Delay    time.Duration
}

func (m *MockLLMProvider) Chat(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
	m.mu.Lock()
	defer m.mu.Unlock()

	if m.callCount >= len(m.responses) {
		return &providers.LLMResponse{
			Content: "No more mock responses configured",
		}, nil
	}

	resp := m.responses[m.callCount]
	m.callCount++

	// Apply delay if configured
	if resp.Delay > 0 {
		select {
		case <-time.After(resp.Delay):
		case <-ctx.Done():
			return nil, ctx.Err()
		}
	}

	if resp.Error != nil {
		return nil, resp.Error
	}

	return resp.Response, nil
}

func (m *MockLLMProvider) GetDefaultModel() string {
	return "mock-model"
}

func (m *MockLLMProvider) Reset() {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.callCount = 0
}

func (m *MockLLMProvider) SetCallCount(count int) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.callCount = count
}

// Test NewSubagentManager
func TestNewSubagentManager(t *testing.T) {
	provider := &MockLLMProvider{}
	workspace := "/test/workspace"
	msgBus := bus.NewMessageBus()

	manager := NewSubagentManager(provider, "test-model", workspace, msgBus)

	if manager == nil {
		t.Fatal("Expected non-nil manager")
	}

	if manager.provider != provider {
		t.Error("Provider not set correctly")
	}

	if manager.defaultModel != "test-model" {
		t.Error("Default model not set correctly")
	}

	if manager.workspace != workspace {
		t.Error("Workspace not set correctly")
	}

	if manager.bus != msgBus {
		t.Error("Bus not set correctly")
	}

	if manager.maxIterations != 10 {
		t.Errorf("Expected max iterations 10, got %d", manager.maxIterations)
	}

	if manager.tools == nil {
		t.Error("Tools registry should be initialized")
	}

	if manager.tasks == nil {
		t.Error("Tasks map should be initialized")
	}
}

// Test SetTools
func TestSubagentManager_SetTools(t *testing.T) {
	manager := NewSubagentManager(nil, "", "", nil)

	newTools := NewToolRegistry()
	newTools.Register(&MockToolForBase{name: "test_tool"})

	manager.SetTools(newTools)

	if manager.tools != newTools {
		t.Error("Tools not set correctly")
	}
}

// Test RegisterTool
func TestSubagentManager_RegisterTool(t *testing.T) {
	manager := NewSubagentManager(nil, "", "", nil)

	tool := &MockToolForBase{name: "test_tool"}
	manager.RegisterTool(tool)

	retrieved, ok := manager.tools.Get("test_tool")
	if !ok || retrieved == nil {
		t.Error("Tool not registered")
	}
}

// Test Spawn basic functionality
func TestSubagentManager_Spawn_Basic(t *testing.T) {
	provider := &MockLLMProvider{
		responses: []MockLLMResponse{
			{
				Response: &providers.LLMResponse{
					Content: "Task completed successfully",
				},
			},
		},
	}

	manager := NewSubagentManager(provider, "test-model", "/workspace", nil)

	ctx := context.Background()
	taskID, err := manager.Spawn(ctx, "test task", "test-label", "agent-1", "channel-1", "chat-1", nil)

	if err != nil {
		t.Fatalf("Expected no error, got %v", err)
	}

	if taskID == "" {
		t.Fatal("Expected non-empty task ID")
	}

	// Wait a bit for the task to start
	time.Sleep(100 * time.Millisecond)

	// Check task was created
	task, ok := manager.GetTask(taskID)
	if !ok {
		t.Fatal("Task not found")
	}

	if task.Task != "test task" {
		t.Errorf("Expected task 'test task', got '%s'", task.Task)
	}

	if task.Label != "test-label" {
		t.Errorf("Expected label 'test-label', got '%s'", task.Label)
	}

	if task.AgentID != "agent-1" {
		t.Errorf("Expected agent ID 'agent-1', got '%s'", task.AgentID)
	}

	if task.OriginChannel != "channel-1" {
		t.Errorf("Expected origin channel 'channel-1', got '%s'", task.OriginChannel)
	}

	if task.OriginChatID != "chat-1" {
		t.Errorf("Expected origin chat ID 'chat-1', got '%s'", task.OriginChatID)
	}

	// Wait for task completion
	time.Sleep(500 * time.Millisecond)

	task, _ = manager.GetTask(taskID)
	if task.Status != "completed" {
		t.Errorf("Expected status 'completed', got '%s'", task.Status)
	}
}

// Test Spawn with callback
func TestSubagentManager_Spawn_WithCallback(t *testing.T) {
	provider := &MockLLMProvider{
		responses: []MockLLMResponse{
			{
				Response: &providers.LLMResponse{
					Content: "Callback test result",
				},
			},
		},
	}

	manager := NewSubagentManager(provider, "test-model", "/workspace", nil)

	ctx := context.Background()

	callbackCalled := false
	var callbackResult *ToolResult
	callback := func(ctx context.Context, result *ToolResult) {
		callbackCalled = true
		callbackResult = result
	}

	_, err := manager.Spawn(ctx, "test task", "label", "agent-1", "channel-1", "chat-1", callback)
	if err != nil {
		t.Fatalf("Expected no error, got %v", err)
	}

	// Wait for callback
	time.Sleep(500 * time.Millisecond)

	if !callbackCalled {
		t.Error("Callback was not called")
	}

	if callbackResult == nil {
		t.Fatal("Callback result is nil")
	}

	if callbackResult.ForUser != "Callback test result" {
		t.Errorf("Expected result 'Callback test result', got '%s'", callbackResult.ForUser)
	}
}

// Test Spawn with context cancellation
func TestSubagentManager_Spawn_ContextCancellation(t *testing.T) {
	provider := &MockLLMProvider{
		responses: []MockLLMResponse{
			{
				Response: &providers.LLMResponse{
					Content: "Should not reach here",
				},
				Delay: 2 * time.Second,
			},
		},
	}

	manager := NewSubagentManager(provider, "test-model", "/workspace", nil)

	// Create a context that will be cancelled quickly
	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()

	// Spawn returns a message string, not the taskID
	// We need to get the task ID differently - by listing tasks or by storing the expected ID
	// Since this is an implementation detail, let's modify the test to check the state

	// First, get initial task count
	manager.mu.RLock()
	initialCount := len(manager.tasks)
	manager.mu.RUnlock()

	_, err := manager.Spawn(ctx, "test task", "label", "agent-1", "channel-1", "chat-1", nil)
	if err != nil {
		t.Fatalf("Expected no error, got %v", err)
	}

	// Wait for cancellation to be processed
	time.Sleep(300 * time.Millisecond)

	// Check that a task was created and then cancelled
	manager.mu.RLock()
	finalCount := len(manager.tasks)
	manager.mu.RUnlock()

	if finalCount <= initialCount {
		t.Fatal("Expected task to be created and remain in tasks map")
	}

	// Find the cancelled task
	var foundTask *SubagentTask
	var found bool
	manager.mu.RLock()
	for _, task := range manager.tasks {
		if task.Status == "cancelled" {
			foundTask = task
			found = true
			break
		}
	}
	manager.mu.RUnlock()

	if !found {
		t.Fatal("No cancelled task found")
	}

	if foundTask == nil {
		t.Fatal("Found cancelled task is nil")
	}

	// The task should be cancelled with appropriate message
	if foundTask.Status != "cancelled" {
		t.Errorf("Expected status 'cancelled', got '%s'", foundTask.Status)
	}
}

// Test Spawn with LLM error
func TestSubagentManager_Spawn_LLMError(t *testing.T) {
	provider := &MockLLMProvider{
		responses: []MockLLMResponse{
			{
				Error: fmt.Errorf("LLM connection failed"),
			},
		},
	}

	manager := NewSubagentManager(provider, "test-model", "/workspace", nil)

	ctx := context.Background()
	taskID, err := manager.Spawn(ctx, "test task", "label", "agent-1", "channel-1", "chat-1", nil)

	if err != nil {
		t.Fatalf("Expected no error, got %v", err)
	}

	// Wait for task completion - give it more time
	time.Sleep(500 * time.Millisecond)

	task, ok := manager.GetTask(taskID)
	if !ok {
		t.Fatal("Task not found")
	}

	if task.Status != "completed" && task.Status != "failed" {
		t.Errorf("Expected status 'completed' or 'failed', got '%s'", task.Status)
	}
}

// Test GetTask
func TestSubagentManager_GetTask(t *testing.T) {
	manager := NewSubagentManager(nil, "", "", nil)

	// Test non-existent task
	_, ok := manager.GetTask("non-existent")
	if ok {
		t.Error("Expected false for non-existent task")
	}

	// Create a task directly
	task := &SubagentTask{
		ID:            "test-task-1",
		Task:          "test task",
		Label:         "label",
		AgentID:       "agent-1",
		OriginChannel: "channel-1",
		OriginChatID:  "chat-1",
		Status:        "running",
		Created:       time.Now().UnixMilli(),
	}
	manager.tasks["test-task-1"] = task

	// Test existing task
	retrieved, ok := manager.GetTask("test-task-1")
	if !ok {
		t.Fatal("Expected true for existing task")
	}

	if retrieved.ID != "test-task-1" {
		t.Errorf("Expected ID 'test-task-1', got '%s'", retrieved.ID)
	}
}

// Test ListTasks
func TestSubagentManager_ListTasks(t *testing.T) {
	manager := NewSubagentManager(nil, "", "", nil)

	// Initially empty
	tasks := manager.ListTasks()
	if len(tasks) != 0 {
		t.Errorf("Expected 0 tasks, got %d", len(tasks))
	}

	// Add some tasks
	for i := 1; i <= 3; i++ {
		manager.tasks[fmt.Sprintf("task-%d", i)] = &SubagentTask{
			ID:     fmt.Sprintf("task-%d", i),
			Task:   fmt.Sprintf("task %d", i),
			Status: "running",
		}
	}

	tasks = manager.ListTasks()
	if len(tasks) != 3 {
		t.Errorf("Expected 3 tasks, got %d", len(tasks))
	}

	// Verify all tasks are present
	taskIDs := make(map[string]bool)
	for _, task := range tasks {
		taskIDs[task.ID] = true
	}

	for i := 1; i <= 3; i++ {
		if !taskIDs[fmt.Sprintf("task-%d", i)] {
			t.Errorf("Task task-%d not found in list", i)
		}
	}
}

// Test concurrent Spawn operations
func TestSubagentManager_ConcurrentSpawns(t *testing.T) {
	provider := &MockLLMProvider{
		responses: make([]MockLLMResponse, 10),
	}

	// Fill responses
	for i := 0; i < 10; i++ {
		provider.responses[i] = MockLLMResponse{
			Response: &providers.LLMResponse{
				Content: fmt.Sprintf("Task %d completed", i),
			},
		}
	}

	manager := NewSubagentManager(provider, "test-model", "/workspace", nil)

	ctx := context.Background()
	var wg sync.WaitGroup
	taskIDs := make([]string, 10)
	mu := sync.Mutex{}

	// Launch concurrent tasks
	for i := 0; i < 10; i++ {
		wg.Add(1)
		go func(index int) {
			defer wg.Done()
			taskID, err := manager.Spawn(ctx, fmt.Sprintf("task %d", index), "", "agent-1", "channel-1", "chat-1", nil)
			if err != nil {
				t.Errorf("Task %d failed: %v", index, err)
				return
			}
			mu.Lock()
			taskIDs[index] = taskID
			mu.Unlock()
		}(i)
	}

	wg.Wait()

	// Verify all tasks were created
	for i, taskID := range taskIDs {
		if taskID == "" {
			t.Errorf("Task %d did not get a task ID", i)
			continue
		}

		task, ok := manager.GetTask(taskID)
		if !ok {
			t.Errorf("Task %d not found", i)
			continue
		}

		if task.Status != "completed" && task.Status != "running" {
			t.Errorf("Task %d has unexpected status: %s", i, task.Status)
		}
	}
}

// Test SubagentTool
func TestSubagentTool_New(t *testing.T) {
	manager := NewSubagentManager(nil, "", "", nil)
	tool := NewSubagentTool(manager)

	if tool == nil {
		t.Fatal("Expected non-nil tool")
	}

	if tool.manager != manager {
		t.Error("Manager not set correctly")
	}

	if tool.originChannel != "cli" {
		t.Errorf("Expected origin channel 'cli', got '%s'", tool.originChannel)
	}

	if tool.originChatID != "direct" {
		t.Errorf("Expected origin chat ID 'direct', got '%s'", tool.originChatID)
	}
}

func TestSubagentTool_Name(t *testing.T) {
	tool := &SubagentTool{}
	if tool.Name() != "subagent" {
		t.Errorf("Expected name 'subagent', got '%s'", tool.Name())
	}
}

func TestSubagentTool_Description(t *testing.T) {
	tool := &SubagentTool{}
	desc := tool.Description()
	if desc == "" {
		t.Error("Description should not be empty")
	}
}

func TestSubagentTool_Parameters(t *testing.T) {
	tool := &SubagentTool{}
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

	if _, ok := props["task"]; !ok {
		t.Error("Task parameter missing")
	}

	if _, ok := props["label"]; !ok {
		t.Error("Label parameter missing")
	}

	required, ok := params["required"].([]string)
	if !ok {
		t.Fatal("Required should be a string slice")
	}

	if len(required) != 1 || required[0] != "task" {
		t.Errorf("Expected required ['task'], got %v", required)
	}
}

func TestSubagentTool_SetContext(t *testing.T) {
	tool := &SubagentTool{}
	tool.SetContext("test-channel", "test-chat")

	if tool.originChannel != "test-channel" {
		t.Errorf("Expected channel 'test-channel', got '%s'", tool.originChannel)
	}

	if tool.originChatID != "test-chat" {
		t.Errorf("Expected chat ID 'test-chat', got '%s'", tool.originChatID)
	}
}

func TestSubagentTool_Execute_MissingTask(t *testing.T) {
	tool := &SubagentTool{}
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{})

	if !result.IsError {
		t.Error("Expected error result")
	}

	if result.ForLLM == "" {
		t.Error("Expected error message")
	}
}

func TestSubagentTool_Execute_NilManager(t *testing.T) {
	tool := &SubagentTool{}
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"task": "test task",
	})

	if !result.IsError {
		t.Error("Expected error result")
	}

	if result.ForLLM == "" {
		t.Error("Expected error message")
	}
}

func TestSubagentTool_Execute_Success(t *testing.T) {
	provider := &MockLLMProvider{
		responses: []MockLLMResponse{
			{
				Response: &providers.LLMResponse{
					Content: "Subagent task completed successfully",
				},
			},
		},
	}

	manager := NewSubagentManager(provider, "test-model", "/workspace", nil)
	tool := NewSubagentTool(manager)
	tool.SetContext("test-channel", "test-chat")

	ctx := context.Background()
	result := tool.Execute(ctx, map[string]interface{}{
		"task":  "test task",
		"label": "test-label",
	})

	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}

	if result.ForUser == "" {
		t.Error("Expected user content")
	}

	if result.ForLLM == "" {
		t.Error("Expected LLM content")
	}

	// Verify LLM content contains details
	if !contains(result.ForLLM, "test-label") {
		t.Error("LLM content should contain label")
	}

	if !contains(result.ForLLM, "Subagent task completed") {
		t.Error("LLM content should contain result")
	}
}

func TestSubagentTool_Execute_LongContentTruncation(t *testing.T) {
	longContent := string(make([]byte, 1000))
	for i := range longContent {
		longContent = longContent[:i] + "A" + longContent[i+1:]
	}

	provider := &MockLLMProvider{
		responses: []MockLLMResponse{
			{
				Response: &providers.LLMResponse{
					Content: longContent,
				},
			},
		},
	}

	manager := NewSubagentManager(provider, "test-model", "/workspace", nil)
	tool := NewSubagentTool(manager)

	ctx := context.Background()
	result := tool.Execute(ctx, map[string]interface{}{
		"task": "test task",
	})

	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}

	// User content should be truncated
	if len(result.ForUser) > 503 { // 500 + "..."
		t.Errorf("Expected truncated user content (max ~503 chars), got %d", len(result.ForUser))
	}

	// LLM content should be full
	if len(result.ForLLM) < len(longContent) {
		t.Errorf("LLM content should contain full result, got %d chars (content was %d)", len(result.ForLLM), len(longContent))
	}
}

func TestSubagentTool_Execute_UnnamedLabel(t *testing.T) {
	provider := &MockLLMProvider{
		responses: []MockLLMResponse{
			{
				Response: &providers.LLMResponse{
					Content: "Task done",
				},
			},
		},
	}

	manager := NewSubagentManager(provider, "test-model", "/workspace", nil)
	tool := NewSubagentTool(manager)

	ctx := context.Background()
	result := tool.Execute(ctx, map[string]interface{}{
		"task": "test task",
	})

	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}

	// Should show (unnamed) for label
	if !contains(result.ForLLM, "(unnamed)") {
		t.Error("LLM content should show (unnamed) for empty label")
	}
}

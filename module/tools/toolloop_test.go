// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package tools

import (
	"context"
	"fmt"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/providers"
)

// MockToolForLoop is a simple mock tool for testing
type MockToolForLoop struct {
	name        string
	shouldError bool
	delay       time.Duration
	callCount   int
	lastArgs    map[string]interface{}
}

func (m *MockToolForLoop) Name() string {
	return m.name
}

func (m *MockToolForLoop) Description() string {
	return fmt.Sprintf("Mock tool %s", m.name)
}

func (m *MockToolForLoop) Parameters() map[string]interface{} {
	return map[string]interface{}{
		"type": "object",
		"properties": map[string]interface{}{
			"input": map[string]interface{}{
				"type":        "string",
				"description": "Test input",
			},
		},
	}
}

func (m *MockToolForLoop) Execute(ctx context.Context, args map[string]interface{}) *ToolResult {
	m.callCount++
	m.lastArgs = args

	if m.delay > 0 {
		select {
		case <-time.After(m.delay):
		case <-ctx.Done():
			return ErrorResult("Tool execution cancelled").WithError(ctx.Err())
		}
	}

	if m.shouldError {
		return ErrorResult("Tool error").WithError(fmt.Errorf("mock tool error"))
	}

	return NewToolResult(fmt.Sprintf("Tool %s executed successfully", m.name))
}

// Test RunToolLoop with direct response (no tools)
func TestRunToolLoop_DirectResponse(t *testing.T) {
	provider := &MockLLMProvider{
		responses: []MockLLMResponse{
			{
				Response: &providers.LLMResponse{
					Content:      "Direct answer without tools",
					ToolCalls:    []providers.ToolCall{},
					FinishReason: "stop",
				},
			},
		},
	}

	config := ToolLoopConfig{
		Provider:      provider,
		Model:         "test-model",
		Tools:         nil,
		MaxIterations: 10,
		LLMOptions:    nil,
	}

	messages := []providers.Message{
		{
			Role:    "system",
			Content: "You are a helpful assistant",
		},
		{
			Role:    "user",
			Content: "Simple question",
		},
	}

	ctx := context.Background()
	result, err := RunToolLoop(ctx, config, messages, "test-channel", "test-chat")

	if err != nil {
		t.Fatalf("Expected no error, got %v", err)
	}

	if result == nil {
		t.Fatal("Expected non-nil result")
	}

	if result.Content != "Direct answer without tools" {
		t.Errorf("Expected content 'Direct answer without tools', got '%s'", result.Content)
	}

	if result.Iterations != 1 {
		t.Errorf("Expected 1 iteration, got %d", result.Iterations)
	}

	if provider.callCount != 1 {
		t.Errorf("Expected 1 provider call, got %d", provider.callCount)
	}
}

// Test RunToolLoop with single tool call
func TestRunToolLoop_SingleToolCall(t *testing.T) {
	mockTool := &MockToolForLoop{name: "test_tool"}

	registry := NewToolRegistry()
	registry.Register(mockTool)

	provider := &MockLLMProvider{
		responses: []MockLLMResponse{
			{
				Response: &providers.LLMResponse{
					Content: "I'll use a tool",
					ToolCalls: []providers.ToolCall{
						{
							ID:   "call-1",
							Name: "test_tool",
							Arguments: map[string]interface{}{
								"input": "test input",
							},
						},
					},
				},
			},
			{
				Response: &providers.LLMResponse{
					Content:      "Tool execution complete",
					ToolCalls:    []providers.ToolCall{},
					FinishReason: "stop",
				},
			},
		},
	}

	config := ToolLoopConfig{
		Provider:      provider,
		Model:         "test-model",
		Tools:         registry,
		MaxIterations: 10,
	}

	messages := []providers.Message{
		{
			Role:    "user",
			Content: "Use a tool",
		},
	}

	ctx := context.Background()
	result, err := RunToolLoop(ctx, config, messages, "test-channel", "test-chat")

	if err != nil {
		t.Fatalf("Expected no error, got %v", err)
	}

	if result.Content != "Tool execution complete" {
		t.Errorf("Expected content 'Tool execution complete', got '%s'", result.Content)
	}

	if result.Iterations != 2 {
		t.Errorf("Expected 2 iterations, got %d", result.Iterations)
	}

	if mockTool.callCount != 1 {
		t.Errorf("Expected tool to be called once, got %d", mockTool.callCount)
	}

	if provider.callCount != 2 {
		t.Errorf("Expected 2 provider calls, got %d", provider.callCount)
	}
}

// Test RunToolLoop with multiple tool calls
func TestRunToolLoop_MultipleToolCalls(t *testing.T) {
	mockTool1 := &MockToolForLoop{name: "tool1"}
	mockTool2 := &MockToolForLoop{name: "tool2"}

	registry := NewToolRegistry()
	registry.Register(mockTool1)
	registry.Register(mockTool2)

	provider := &MockLLMProvider{
		responses: []MockLLMResponse{
			{
				Response: &providers.LLMResponse{
					Content: "I'll use tool1",
					ToolCalls: []providers.ToolCall{
						{
							ID:   "call-1",
							Name: "tool1",
							Arguments: map[string]interface{}{
								"input": "input1",
							},
						},
					},
				},
			},
			{
				Response: &providers.LLMResponse{
					Content: "Now I'll use tool2",
					ToolCalls: []providers.ToolCall{
						{
							ID:   "call-2",
							Name: "tool2",
							Arguments: map[string]interface{}{
								"input": "input2",
							},
						},
					},
				},
			},
			{
				Response: &providers.LLMResponse{
					Content:      "All tools executed",
					ToolCalls:    []providers.ToolCall{},
					FinishReason: "stop",
				},
			},
		},
	}

	config := ToolLoopConfig{
		Provider:      provider,
		Model:         "test-model",
		Tools:         registry,
		MaxIterations: 10,
	}

	messages := []providers.Message{
		{
			Role:    "user",
			Content: "Use multiple tools",
		},
	}

	ctx := context.Background()
	result, err := RunToolLoop(ctx, config, messages, "test-channel", "test-chat")

	if err != nil {
		t.Fatalf("Expected no error, got %v", err)
	}

	if result.Content != "All tools executed" {
		t.Errorf("Expected content 'All tools executed', got '%s'", result.Content)
	}

	if result.Iterations != 3 {
		t.Errorf("Expected 3 iterations, got %d", result.Iterations)
	}

	if mockTool1.callCount != 1 {
		t.Errorf("Expected tool1 to be called once, got %d", mockTool1.callCount)
	}

	if mockTool2.callCount != 1 {
		t.Errorf("Expected tool2 to be called once, got %d", mockTool2.callCount)
	}
}

// Test RunToolLoop with max iterations
func TestRunToolLoop_MaxIterations(t *testing.T) {
	mockTool := &MockToolForLoop{name: "test_tool"}

	registry := NewToolRegistry()
	registry.Register(mockTool)

	// Create responses that will keep calling tools
	responses := make([]MockLLMResponse, 15)
	for i := 0; i < 15; i++ {
		responses[i] = MockLLMResponse{
			Response: &providers.LLMResponse{
				Content: fmt.Sprintf("Iteration %d", i+1),
				ToolCalls: []providers.ToolCall{
					{
						ID:   fmt.Sprintf("call-%d", i+1),
						Name: "test_tool",
						Arguments: map[string]interface{}{
							"input": fmt.Sprintf("input-%d", i+1),
						},
					},
				},
			},
		}
	}

	provider := &MockLLMProvider{
		responses: responses,
	}

	config := ToolLoopConfig{
		Provider:      provider,
		Model:         "test-model",
		Tools:         registry,
		MaxIterations: 10,
	}

	messages := []providers.Message{
		{
			Role:    "user",
			Content: "Keep calling tools",
		},
	}

	ctx := context.Background()
	result, err := RunToolLoop(ctx, config, messages, "test-channel", "test-chat")

	if err != nil {
		t.Fatalf("Expected no error, got %v", err)
	}

	// Should stop at max iterations
	if result.Iterations != 10 {
		t.Errorf("Expected 10 iterations (max), got %d", result.Iterations)
	}

	if mockTool.callCount != 10 {
		t.Errorf("Expected tool to be called 10 times, got %d", mockTool.callCount)
	}
}

// Test RunToolLoop with LLM error
func TestRunToolLoop_LLMError(t *testing.T) {
	provider := &MockLLMProvider{
		responses: []MockLLMResponse{
			{
				Error: fmt.Errorf("LLM connection failed"),
			},
		},
	}

	config := ToolLoopConfig{
		Provider:      provider,
		Model:         "test-model",
		Tools:         nil,
		MaxIterations: 10,
	}

	messages := []providers.Message{
		{
			Role:    "user",
			Content: "Test",
		},
	}

	ctx := context.Background()
	result, err := RunToolLoop(ctx, config, messages, "test-channel", "test-chat")

	if err == nil {
		t.Fatal("Expected error, got nil")
	}

	if result != nil {
		t.Error("Expected nil result on error")
	}

	if !contains(err.Error(), "LLM call failed") {
		t.Errorf("Expected 'LLM call failed' in error, got: %v", err)
	}
}

// Test RunToolLoop with tool error
func TestRunToolLoop_ToolError(t *testing.T) {
	mockTool := &MockToolForLoop{
		name:        "error_tool",
		shouldError: true,
	}

	registry := NewToolRegistry()
	registry.Register(mockTool)

	provider := &MockLLMProvider{
		responses: []MockLLMResponse{
			{
				Response: &providers.LLMResponse{
					Content: "I'll use the error tool",
					ToolCalls: []providers.ToolCall{
						{
							ID:   "call-1",
							Name: "error_tool",
							Arguments: map[string]interface{}{
								"input": "test",
							},
						},
					},
				},
			},
			{
				Response: &providers.LLMResponse{
					Content:      "Tool failed, but I'll continue",
					ToolCalls:    []providers.ToolCall{},
					FinishReason: "stop",
				},
			},
		},
	}

	config := ToolLoopConfig{
		Provider:      provider,
		Model:         "test-model",
		Tools:         registry,
		MaxIterations: 10,
	}

	messages := []providers.Message{
		{
			Role:    "user",
			Content: "Test tool error",
		},
	}

	ctx := context.Background()
	result, err := RunToolLoop(ctx, config, messages, "test-channel", "test-chat")

	if err != nil {
		t.Fatalf("Expected no error, got %v", err)
	}

	// Loop should continue after tool error
	if result.Iterations != 2 {
		t.Errorf("Expected 2 iterations, got %d", result.Iterations)
	}

	if mockTool.callCount != 1 {
		t.Errorf("Expected tool to be called once, got %d", mockTool.callCount)
	}
}

// Test RunToolLoop with context cancellation
func TestRunToolLoop_ContextCancellation(t *testing.T) {
	mockTool := &MockToolForLoop{
		name:  "slow_tool",
		delay: 2 * time.Second,
	}

	registry := NewToolRegistry()
	registry.Register(mockTool)

	provider := &MockLLMProvider{
		responses: []MockLLMResponse{
			{
				Response: &providers.LLMResponse{
					Content: "I'll use a slow tool",
					ToolCalls: []providers.ToolCall{
						{
							ID:   "call-1",
							Name: "slow_tool",
							Arguments: map[string]interface{}{
								"input": "test",
							},
						},
					},
				},
			},
		},
	}

	config := ToolLoopConfig{
		Provider:      provider,
		Model:         "test-model",
		Tools:         registry,
		MaxIterations: 10,
	}

	messages := []providers.Message{
		{
			Role:    "user",
			Content: "Test cancellation",
		},
	}

	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()

	result, err := RunToolLoop(ctx, config, messages, "test-channel", "test-chat")

	// Should get an error due to context cancellation
	if err == nil {
		t.Fatal("Expected error due to context cancellation, got nil")
	}

	if result != nil {
		t.Error("Expected nil result on error")
	}
}

// Test RunToolLoop with no tools available
func TestRunToolLoop_NoToolsAvailable(t *testing.T) {
	provider := &MockLLMProvider{
		responses: []MockLLMResponse{
			{
				Response: &providers.LLMResponse{
					Content: "I'll try to use a tool",
					ToolCalls: []providers.ToolCall{
						{
							ID:   "call-1",
							Name: "nonexistent_tool",
							Arguments: map[string]interface{}{
								"input": "test",
							},
						},
					},
				},
			},
			{
				Response: &providers.LLMResponse{
					Content:      "Tool not available",
					ToolCalls:    []providers.ToolCall{},
					FinishReason: "stop",
				},
			},
		},
	}

	config := ToolLoopConfig{
		Provider:      provider,
		Model:         "test-model",
		Tools:         NewToolRegistry(), // Empty registry
		MaxIterations: 10,
	}

	messages := []providers.Message{
		{
			Role:    "user",
			Content: "Test with no tools",
		},
	}

	ctx := context.Background()
	result, err := RunToolLoop(ctx, config, messages, "test-channel", "test-chat")

	if err != nil {
		t.Fatalf("Expected no error, got %v", err)
	}

	// Should continue and return final response
	if result.Iterations != 2 {
		t.Errorf("Expected 2 iterations, got %d", result.Iterations)
	}
}

// Test RunToolLoop with default LLM options
func TestRunToolLoop_DefaultLLMOptions(t *testing.T) {
	provider := &MockLLMProvider{
		responses: []MockLLMResponse{
			{
				Response: &providers.LLMResponse{
					Content:      "Direct response",
					ToolCalls:    []providers.ToolCall{},
					FinishReason: "stop",
				},
			},
		},
	}

	config := ToolLoopConfig{
		Provider:      provider,
		Model:         "test-model",
		Tools:         nil,
		MaxIterations: 10,
		LLMOptions:    nil, // Should use defaults
	}

	messages := []providers.Message{
		{
			Role:    "user",
			Content: "Test",
		},
	}

	ctx := context.Background()
	result, err := RunToolLoop(ctx, config, messages, "test-channel", "test-chat")

	if err != nil {
		t.Fatalf("Expected no error, got %v", err)
	}

	if result.Content != "Direct response" {
		t.Errorf("Unexpected content: %s", result.Content)
	}
}

// Test RunToolLoop with custom LLM options
func TestRunToolLoop_CustomLLMOptions(t *testing.T) {
	provider := &MockLLMProvider{
		responses: []MockLLMResponse{
			{
				Response: &providers.LLMResponse{
					Content:      "Direct response",
					ToolCalls:    []providers.ToolCall{},
					FinishReason: "stop",
				},
			},
		},
	}

	config := ToolLoopConfig{
		Provider:      provider,
		Model:         "test-model",
		Tools:         nil,
		MaxIterations: 10,
		LLMOptions: map[string]any{
			"max_tokens":  2048,
			"temperature": 0.5,
		},
	}

	messages := []providers.Message{
		{
			Role:    "user",
			Content: "Test",
		},
	}

	ctx := context.Background()
	result, err := RunToolLoop(ctx, config, messages, "test-channel", "test-chat")

	if err != nil {
		t.Fatalf("Expected no error, got %v", err)
	}

	if result.Content != "Direct response" {
		t.Errorf("Unexpected content: %s", result.Content)
	}
}

// Test RunToolLoop with multiple tools in single iteration
func TestRunToolLoop_MultipleToolsSingleIteration(t *testing.T) {
	mockTool1 := &MockToolForLoop{name: "tool1"}
	mockTool2 := &MockToolForLoop{name: "tool2"}

	registry := NewToolRegistry()
	registry.Register(mockTool1)
	registry.Register(mockTool2)

	provider := &MockLLMProvider{
		responses: []MockLLMResponse{
			{
				Response: &providers.LLMResponse{
					Content: "I'll use both tools",
					ToolCalls: []providers.ToolCall{
						{
							ID:   "call-1",
							Name: "tool1",
							Arguments: map[string]interface{}{
								"input": "input1",
							},
						},
						{
							ID:   "call-2",
							Name: "tool2",
							Arguments: map[string]interface{}{
								"input": "input2",
							},
						},
					},
				},
			},
			{
				Response: &providers.LLMResponse{
					Content:      "Both tools executed",
					ToolCalls:    []providers.ToolCall{},
					FinishReason: "stop",
				},
			},
		},
	}

	config := ToolLoopConfig{
		Provider:      provider,
		Model:         "test-model",
		Tools:         registry,
		MaxIterations: 10,
	}

	messages := []providers.Message{
		{
			Role:    "user",
			Content: "Use both tools",
		},
	}

	ctx := context.Background()
	result, err := RunToolLoop(ctx, config, messages, "test-channel", "test-chat")

	if err != nil {
		t.Fatalf("Expected no error, got %v", err)
	}

	if result.Iterations != 2 {
		t.Errorf("Expected 2 iterations, got %d", result.Iterations)
	}

	if mockTool1.callCount != 1 {
		t.Errorf("Expected tool1 to be called once, got %d", mockTool1.callCount)
	}

	if mockTool2.callCount != 1 {
		t.Errorf("Expected tool2 to be called once, got %d", mockTool2.callCount)
	}
}

// Test RunToolLoop tool result propagation
func TestRunToolLoop_ToolResultPropagation(t *testing.T) {
	mockTool := &MockToolForLoop{name: "test_tool"}

	registry := NewToolRegistry()
	registry.Register(mockTool)

	provider := &MockLLMProvider{
		responses: []MockLLMResponse{
			{
				Response: &providers.LLMResponse{
					Content: "Using tool",
					ToolCalls: []providers.ToolCall{
						{
							ID:   "call-1",
							Name: "test_tool",
							Arguments: map[string]interface{}{
								"input": "specific_input",
							},
						},
					},
				},
			},
			{
				Response: &providers.LLMResponse{
					Content:      "Done",
					ToolCalls:    []providers.ToolCall{},
					FinishReason: "stop",
				},
			},
		},
	}

	config := ToolLoopConfig{
		Provider:      provider,
		Model:         "test-model",
		Tools:         registry,
		MaxIterations: 10,
	}

	messages := []providers.Message{
		{
			Role:    "user",
			Content: "Test",
		},
	}

	ctx := context.Background()
	_, err := RunToolLoop(ctx, config, messages, "test-channel", "test-chat")

	if err != nil {
		t.Fatalf("Expected no error, got %v", err)
	}

	// Verify tool received correct arguments
	if mockTool.lastArgs == nil {
		t.Fatal("Tool should have been called with arguments")
	}

	input, ok := mockTool.lastArgs["input"].(string)
	if !ok {
		t.Fatal("Input argument should be a string")
	}

	if input != "specific_input" {
		t.Errorf("Expected input 'specific_input', got '%s'", input)
	}
}

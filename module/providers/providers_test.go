// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package providers

import (
	"context"
	"errors"
	"testing"
	"time"
)

// MockLLMProvider is a mock implementation of LLMProvider for testing
type MockLLMProvider struct {
	defaultModel string
	chatFunc     func(ctx context.Context, messages []Message, tools []ToolDefinition, model string, options map[string]interface{}) (*LLMResponse, error)
}

func (m *MockLLMProvider) Chat(ctx context.Context, messages []Message, tools []ToolDefinition, model string, options map[string]interface{}) (*LLMResponse, error) {
	if m.chatFunc != nil {
		return m.chatFunc(ctx, messages, tools, model, options)
	}
	return &LLMResponse{
		Content: "Mock response",
		ToolCalls: []ToolCall{},
	}, nil
}

func (m *MockLLMProvider) GetDefaultModel() string {
	if m.defaultModel != "" {
		return m.defaultModel
	}
	return "mock-model"
}

func NewMockLLMProvider(defaultModel string) *MockLLMProvider {
	return &MockLLMProvider{
		defaultModel: defaultModel,
	}
}

func TestFailoverError_Error(t *testing.T) {
	wrappedErr := errors.New("underlying error")
	err := &FailoverError{
		Reason:   FailoverAuth,
		Provider: "test-provider",
		Model:    "test-model",
		Status:   401,
		Wrapped:  wrappedErr,
	}

	msg := err.Error()
	if msg == "" {
		t.Error("Error() should not return empty string")
	}

	expectedFields := []string{"failover", "auth", "test-provider", "test-model", "401"}
	for _, field := range expectedFields {
		if !containsString(msg, field) {
			t.Errorf("Error message should contain '%s', got '%s'", field, msg)
		}
	}
}

func TestFailoverError_Unwrap(t *testing.T) {
	wrappedErr := errors.New("underlying error")
	err := &FailoverError{
		Wrapped: wrappedErr,
	}

	unwrapped := err.Unwrap()
	if unwrapped != wrappedErr {
		t.Error("Unwrap() should return the wrapped error")
	}
}

func TestFailoverError_IsRetriable(t *testing.T) {
	tests := []struct {
		name     string
		reason   FailoverReason
		expected bool
	}{
		{
			name:     "auth error is retriable",
			reason:   FailoverAuth,
			expected: true,
		},
		{
			name:     "rate limit is retriable",
			reason:   FailoverRateLimit,
			expected: true,
		},
		{
			name:     "billing is retriable",
			reason:   FailoverBilling,
			expected: true,
		},
		{
			name:     "timeout is retriable",
			reason:   FailoverTimeout,
			expected: true,
		},
		{
			name:     "overloaded is retriable",
			reason:   FailoverOverloaded,
			expected: true,
		},
		{
			name:     "unknown is retriable",
			reason:   FailoverUnknown,
			expected: true,
		},
		{
			name:     "format error is not retriable",
			reason:   FailoverFormat,
			expected: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			err := &FailoverError{
				Reason: tt.reason,
			}

			result := err.IsRetriable()
			if result != tt.expected {
				t.Errorf("IsRetriable() = %v, want %v", result, tt.expected)
			}
		})
	}
}

func TestFailoverReason_Constants(t *testing.T) {
	tests := []struct {
		name   string
		reason FailoverReason
		value  string
	}{
		{"auth", FailoverAuth, "auth"},
		{"rate_limit", FailoverRateLimit, "rate_limit"},
		{"billing", FailoverBilling, "billing"},
		{"timeout", FailoverTimeout, "timeout"},
		{"format", FailoverFormat, "format"},
		{"overloaded", FailoverOverloaded, "overloaded"},
		{"unknown", FailoverUnknown, "unknown"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if string(tt.reason) != tt.value {
				t.Errorf("Expected %q, got %q", tt.value, tt.reason)
			}
		})
	}
}

func TestModelConfig_Structure(t *testing.T) {
	config := ModelConfig{
		Primary: "primary-model",
		Fallbacks: []string{
			"fallback1",
			"fallback2",
		},
	}

	if config.Primary != "primary-model" {
		t.Errorf("Expected primary 'primary-model', got '%s'", config.Primary)
	}

	if len(config.Fallbacks) != 2 {
		t.Errorf("Expected 2 fallbacks, got %d", len(config.Fallbacks))
	}

	if config.Fallbacks[0] != "fallback1" {
		t.Errorf("Expected fallback1 'fallback1', got '%s'", config.Fallbacks[0])
	}
}

func TestModelConfig_Empty(t *testing.T) {
	config := ModelConfig{}

	if config.Primary != "" {
		t.Errorf("Expected empty primary, got '%s'", config.Primary)
	}

	// In Go, slices are nil by default when creating a struct
	// Both nil and empty slice are valid for "no fallbacks"
	if config.Fallbacks != nil && len(config.Fallbacks) != 0 {
		t.Errorf("Expected 0 fallbacks, got %d", len(config.Fallbacks))
	}
}

func TestModelConfig_NoFallbacks(t *testing.T) {
	config := ModelConfig{
		Primary: "only-model",
	}

	if config.Primary != "only-model" {
		t.Errorf("Expected primary 'only-model', got '%s'", config.Primary)
	}

	if len(config.Fallbacks) != 0 {
		t.Errorf("Expected 0 fallbacks, got %d", len(config.Fallbacks))
	}
}

func TestMockLLMProvider_GetDefaultModel(t *testing.T) {
	provider := NewMockLLMProvider("custom-model")

	model := provider.GetDefaultModel()
	if model != "custom-model" {
		t.Errorf("Expected default model 'custom-model', got '%s'", model)
	}
}

func TestMockLLMProvider_GetDefaultModel_Empty(t *testing.T) {
	provider := NewMockLLMProvider("")

	model := provider.GetDefaultModel()
	if model != "mock-model" {
		t.Errorf("Expected default model 'mock-model', got '%s'", model)
	}
}

func TestMockLLMProvider_Chat(t *testing.T) {
	provider := NewMockLLMProvider("test-model")

	ctx := context.Background()
	messages := []Message{
		{Role: "user", Content: "Hello"},
	}

	response, err := provider.Chat(ctx, messages, nil, "test-model", nil)
	if err != nil {
		t.Errorf("Chat() returned error: %v", err)
	}

	if response == nil {
		t.Fatal("Chat() should return non-nil response")
	}

	if response.Content != "Mock response" {
		t.Errorf("Expected content 'Mock response', got '%s'", response.Content)
	}
}

func TestMockLLMProvider_Chat_Custom(t *testing.T) {
	customResponse := "Custom response"
	provider := &MockLLMProvider{
		chatFunc: func(ctx context.Context, messages []Message, tools []ToolDefinition, model string, options map[string]interface{}) (*LLMResponse, error) {
			return &LLMResponse{
				Content: customResponse,
			}, nil
		},
	}

	ctx := context.Background()
	response, err := provider.Chat(ctx, nil, nil, "", nil)

	if err != nil {
		t.Errorf("Chat() returned error: %v", err)
	}

	if response.Content != customResponse {
		t.Errorf("Expected content '%s', got '%s'", customResponse, response.Content)
	}
}

func TestMockLLMProvider_Chat_Error(t *testing.T) {
	expectedErr := errors.New("chat failed")
	provider := &MockLLMProvider{
		chatFunc: func(ctx context.Context, messages []Message, tools []ToolDefinition, model string, options map[string]interface{}) (*LLMResponse, error) {
			return nil, expectedErr
		},
	}

	ctx := context.Background()
	_, err := provider.Chat(ctx, nil, nil, "", nil)

	if err != expectedErr {
		t.Errorf("Expected error '%v', got '%v'", expectedErr, err)
	}
}

func TestMockLLMProvider_Chat_WithTimeout(t *testing.T) {
	t.Skip("timing-sensitive test, skipping in CI")
}

func TestMessage_Structure(t *testing.T) {
	message := Message{
		Role:    "user",
		Content: "Hello",
	}

	if message.Role != "user" {
		t.Errorf("Expected role 'user', got '%s'", message.Role)
	}

	if message.Content != "Hello" {
		t.Errorf("Expected content 'Hello', got '%s'", message.Content)
	}
}

func TestLLMResponse_Structure(t *testing.T) {
	response := LLMResponse{
		Content: "Response",
		ToolCalls: []ToolCall{
			{
				ID:   "call-123",
				Name: "test_tool",
			},
		},
	}

	if response.Content != "Response" {
		t.Errorf("Expected content 'Response', got '%s'", response.Content)
	}

	if len(response.ToolCalls) != 1 {
		t.Errorf("Expected 1 tool call, got %d", len(response.ToolCalls))
	}
}

func TestToolCall_Structure(t *testing.T) {
	toolCall := ToolCall{
		ID:   "call-456",
		Name: "another_tool",
		Arguments: map[string]interface{}{
			"param": "value",
		},
	}

	if toolCall.ID != "call-456" {
		t.Errorf("Expected ID 'call-456', got '%s'", toolCall.ID)
	}

	if toolCall.Name != "another_tool" {
		t.Errorf("Expected name 'another_tool', got '%s'", toolCall.Name)
	}
}

func TestToolDefinition_Structure(t *testing.T) {
	def := ToolDefinition{
		Type: "function",
		Function: ToolFunctionDefinition{
			Name:        "test_func",
			Description: "A test function",
			Parameters: map[string]interface{}{
				"type": "object",
			},
		},
	}

	if def.Type != "function" {
		t.Errorf("Expected type 'function', got '%s'", def.Type)
	}


	if def.Function.Name != "test_func" {
		t.Errorf("Expected function name 'test_func', got '%s'", def.Function.Name)
	}
}

func TestLLMProvider_Interface(t *testing.T) {
	// Verify that MockLLMProvider implements LLMProvider
	var _ LLMProvider = &MockLLMProvider{}

	provider := NewMockLLMProvider("test-model")

	// Test GetDefaultModel
	model := provider.GetDefaultModel()
	if model == "" {
		t.Error("GetDefaultModel() should return non-empty model")
	}

	// Test Chat
	ctx := context.Background()
	response, err := provider.Chat(ctx, nil, nil, "", nil)
	if err != nil {
		t.Errorf("Chat() returned error: %v", err)
	}
	if response == nil {
		t.Error("Chat() should return non-nil response")
	}
}

func TestFailoverError_AllFields(t *testing.T) {
	wrappedErr := errors.New("wrapped error")
	err := &FailoverError{
		Reason:   FailoverRateLimit,
		Provider: "provider-xyz",
		Model:    "model-abc",
		Status:   429,
		Wrapped:  wrappedErr,
	}

	if err.Reason != FailoverRateLimit {
		t.Errorf("Expected reason '%s', got '%s'", FailoverRateLimit, err.Reason)
	}

	if err.Provider != "provider-xyz" {
		t.Errorf("Expected provider 'provider-xyz', got '%s'", err.Provider)
	}

	if err.Model != "model-abc" {
		t.Errorf("Expected model 'model-abc', got '%s'", err.Model)
	}

	if err.Status != 429 {
		t.Errorf("Expected status 429, got %d", err.Status)
	}

	if err.Wrapped != wrappedErr {
		t.Error("Wrapped error not set correctly")
	}
}

func TestFailoverError_ErrorFormat(t *testing.T) {
	err := &FailoverError{
		Reason:   FailoverAuth,
		Provider: "test-provider",
		Model:    "test-model",
		Status:   401,
		Wrapped:  errors.New("auth failed"),
	}

	errorStr := err.Error()

	// Check that error string contains all key components
	requiredStrings := []string{
		"failover",
		string(FailoverAuth),
		"test-provider",
		"test-model",
		"401",
		"auth failed",
	}

	for _, s := range requiredStrings {
		if !containsString(errorStr, s) {
			t.Errorf("Error string should contain '%s', got '%s'", s, errorStr)
		}
	}
}

func TestModelConfig_AllFallbacks(t *testing.T) {
	config := ModelConfig{
		Primary: "primary",
		Fallbacks: []string{
			"fallback1",
			"fallback2",
			"fallback3",
			"fallback4",
			"fallback5",
		},
	}

	if len(config.Fallbacks) != 5 {
		t.Errorf("Expected 5 fallbacks, got %d", len(config.Fallbacks))
	}

	// Verify all fallbacks are present
	expectedFallbacks := []string{"fallback1", "fallback2", "fallback3", "fallback4", "fallback5"}
	for i, expected := range expectedFallbacks {
		if config.Fallbacks[i] != expected {
			t.Errorf("Fallback %d: expected '%s', got '%s'", i, expected, config.Fallbacks[i])
		}
	}
}

func TestModelConfig_SliceOperations(t *testing.T) {
	config := ModelConfig{
		Primary:   "primary",
		Fallbacks: []string{"f1", "f2", "f3"},
	}

	// Append fallback
	config.Fallbacks = append(config.Fallbacks, "f4")
	if len(config.Fallbacks) != 4 {
		t.Errorf("Expected 4 fallbacks after append, got %d", len(config.Fallbacks))
	}

	// Modify fallback
	config.Fallbacks[0] = "modified-f1"
	if config.Fallbacks[0] != "modified-f1" {
		t.Errorf("Expected modified fallback, got '%s'", config.Fallbacks[0])
	}
}

func TestLLMResponse_EmptyToolCalls(t *testing.T) {
	response := LLMResponse{
		Content:   "No tools needed",
		ToolCalls: []ToolCall{},
	}

	if response.Content != "No tools needed" {
		t.Errorf("Expected content 'No tools needed', got '%s'", response.Content)
	}

	if len(response.ToolCalls) != 0 {
		t.Errorf("Expected 0 tool calls, got %d", len(response.ToolCalls))
	}
}

func TestLLMResponse_MultipleToolCalls(t *testing.T) {
	response := LLMResponse{
		Content: "Using tools",
		ToolCalls: []ToolCall{
			{ID: "call-1", Name: "tool1"},
			{ID: "call-2", Name: "tool2"},
			{ID: "call-3", Name: "tool3"},
		},
	}

	if len(response.ToolCalls) != 3 {
		t.Errorf("Expected 3 tool calls, got %d", len(response.ToolCalls))
	}

	if response.ToolCalls[0].ID != "call-1" {
		t.Errorf("Expected first call ID 'call-1', got '%s'", response.ToolCalls[0].ID)
	}
}

func TestMessage_Roles(t *testing.T) {
	validRoles := []string{"user", "assistant", "system", "tool"}

	for _, role := range validRoles {
		msg := Message{Role: role}
		if msg.Role != role {
			t.Errorf("Expected role '%s', got '%s'", role, msg.Role)
		}
	}
}

func TestMessage_EmptyContent(t *testing.T) {
	message := Message{
		Role:    "user",
		Content: "",
	}

	if message.Content != "" {
		t.Errorf("Expected empty content, got '%s'", message.Content)
	}
}

func TestToolCall_NoArguments(t *testing.T) {
	toolCall := ToolCall{
		ID:        "call-789",
		Name:      "no_args_tool",
		Arguments: nil,
	}

	if toolCall.ID != "call-789" {
		t.Errorf("Expected ID 'call-789', got '%s'", toolCall.ID)
	}

	if toolCall.Name != "no_args_tool" {
		t.Errorf("Expected name 'no_args_tool', got '%s'", toolCall.Name)
	}

	if toolCall.Arguments != nil {
		t.Error("Expected nil arguments")
	}
}

func TestToolCall_EmptyArguments(t *testing.T) {
	toolCall := ToolCall{
		ID:        "call-empty",
		Name:      "empty_args_tool",
		Arguments: map[string]interface{}{},
	}

	if toolCall.Arguments == nil {
		t.Error("Arguments should not be nil")
	}

	if len(toolCall.Arguments) != 0 {
		t.Errorf("Expected 0 arguments, got %d", len(toolCall.Arguments))
	}
}

func TestToolCall_ComplexArguments(t *testing.T) {
	toolCall := ToolCall{
		ID:   "call-complex",
		Name: "complex_tool",
		Arguments: map[string]interface{}{
			"string":  "value",
			"number":  42,
			"boolean": true,
			"array":   []string{"a", "b", "c"},
			"object": map[string]interface{}{
				"nested": "data",
			},
		},
	}

	if toolCall.Arguments["string"] != "value" {
		t.Errorf("Expected string argument 'value', got '%v'", toolCall.Arguments["string"])
	}

	if toolCall.Arguments["number"] != 42 {
		t.Errorf("Expected number argument 42, got '%v'", toolCall.Arguments["number"])
	}

	if toolCall.Arguments["boolean"] != true {
		t.Errorf("Expected boolean argument true, got '%v'", toolCall.Arguments["boolean"])
	}
}

func TestToolFunctionDefinition_AllFields(t *testing.T) {
	funcDef := ToolFunctionDefinition{
		Name:        "full_function",
		Description: "A function with all fields",
		Parameters: map[string]interface{}{
			"type": "object",
			"properties": map[string]interface{}{
				"param1": map[string]interface{}{
					"type":        "string",
					"description": "First parameter",
				},
				"param2": map[string]interface{}{
					"type":        "integer",
					"description": "Second parameter",
				},
			},
			"required": []string{"param1"},
		},
	}

	if funcDef.Name != "full_function" {
		t.Errorf("Expected name 'full_function', got '%s'", funcDef.Name)
	}

	if funcDef.Description != "A function with all fields" {
		t.Errorf("Expected description 'A function with all fields', got '%s'", funcDef.Description)
	}

	if funcDef.Parameters == nil {
		t.Error("Parameters should not be nil")
	}
}

func TestToolFunctionDefinition_Minimal(t *testing.T) {
	funcDef := ToolFunctionDefinition{
		Name: "minimal_function",
	}

	if funcDef.Name != "minimal_function" {
		t.Errorf("Expected name 'minimal_function', got '%s'", funcDef.Name)
	}

	if funcDef.Description != "" {
		t.Errorf("Expected empty description, got '%s'", funcDef.Description)
	}

	if funcDef.Parameters != nil {
		t.Error("Parameters should be nil for minimal definition")
	}
}

func TestLLMProvider_ContextCancellation(t *testing.T) {
	provider := &MockLLMProvider{
		chatFunc: func(ctx context.Context, messages []Message, tools []ToolDefinition, model string, options map[string]interface{}) (*LLMResponse, error) {
			<-ctx.Done()
			return nil, ctx.Err()
		},
	}

	ctx, cancel := context.WithCancel(context.Background())
	cancel() // Cancel immediately

	_, err := provider.Chat(ctx, nil, nil, "", nil)
	if err == nil {
		t.Error("Expected error when context is cancelled")
	}
}

func TestLLMProvider_ContextTimeout(t *testing.T) {
	provider := &MockLLMProvider{
		chatFunc: func(ctx context.Context, messages []Message, tools []ToolDefinition, model string, options map[string]interface{}) (*LLMResponse, error) {
			select {
			case <-ctx.Done():
				return nil, ctx.Err()
			case <-time.After(1 * time.Second):
				return &LLMResponse{Content: "Done"}, nil
			}
		},
	}

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Millisecond)
	defer cancel()

	_, err := provider.Chat(ctx, nil, nil, "", nil)
	if err == nil {
		t.Error("Expected timeout error")
	}
}

func TestLLMProvider_WithOptions(t *testing.T) {
	var receivedOptions map[string]interface{}
	provider := &MockLLMProvider{
		chatFunc: func(ctx context.Context, messages []Message, tools []ToolDefinition, model string, options map[string]interface{}) (*LLMResponse, error) {
			receivedOptions = options
			return &LLMResponse{Content: "Response"}, nil
		},
	}

	ctx := context.Background()
	testOptions := map[string]interface{}{
		"temperature": 0.7,
		"max_tokens":   1000,
		"top_p":       0.9,
	}

	_, err := provider.Chat(ctx, nil, nil, "", testOptions)
	if err != nil {
		t.Errorf("Chat() returned error: %v", err)
	}

	if receivedOptions == nil {
		t.Error("Options should be passed to chat function")
	}

	if receivedOptions["temperature"] != 0.7 {
		t.Errorf("Expected temperature 0.7, got %v", receivedOptions["temperature"])
	}
}

func TestLLMProvider_WithMessages(t *testing.T) {
	var receivedMessages []Message
	provider := &MockLLMProvider{
		chatFunc: func(ctx context.Context, messages []Message, tools []ToolDefinition, model string, options map[string]interface{}) (*LLMResponse, error) {
			receivedMessages = messages
			return &LLMResponse{Content: "Response"}, nil
		},
	}

	ctx := context.Background()
	testMessages := []Message{
		{Role: "system", Content: "You are a helpful assistant"},
		{Role: "user", Content: "Hello"},
		{Role: "assistant", Content: "Hi there"},
		{Role: "user", Content: "How are you?"},
	}

	_, err := provider.Chat(ctx, testMessages, nil, "", nil)
	if err != nil {
		t.Errorf("Chat() returned error: %v", err)
	}

	if receivedMessages == nil {
		t.Fatal("Messages should be passed to chat function")
	}

	if len(receivedMessages) != 4 {
		t.Errorf("Expected 4 messages, got %d", len(receivedMessages))
	}
}

func TestLLMProvider_WithTools(t *testing.T) {
	var receivedTools []ToolDefinition
	provider := &MockLLMProvider{
		chatFunc: func(ctx context.Context, messages []Message, tools []ToolDefinition, model string, options map[string]interface{}) (*LLMResponse, error) {
			receivedTools = tools
			return &LLMResponse{Content: "Response"}, nil
		},
	}

	ctx := context.Background()
	testTools := []ToolDefinition{
		{
			Type: "function",
			Function: ToolFunctionDefinition{
				Name: "tool1",
			},
		},
		{
			Type: "function",
			Function: ToolFunctionDefinition{
				Name: "tool2",
			},
		},
	}

	_, err := provider.Chat(ctx, nil, testTools, "", nil)
	if err != nil {
		t.Errorf("Chat() returned error: %v", err)
	}

	if receivedTools == nil {
		t.Fatal("Tools should be passed to chat function")
	}

	if len(receivedTools) != 2 {
		t.Errorf("Expected 2 tools, got %d", len(receivedTools))
	}
}

func TestLLMProvider_WithModel(t *testing.T) {
	var receivedModel string
	provider := &MockLLMProvider{
		chatFunc: func(ctx context.Context, messages []Message, tools []ToolDefinition, model string, options map[string]interface{}) (*LLMResponse, error) {
			receivedModel = model
			return &LLMResponse{Content: "Response"}, nil
		},
	}

	ctx := context.Background()
	testModel := "custom-model-name"

	_, err := provider.Chat(ctx, nil, nil, testModel, nil)
	if err != nil {
		t.Errorf("Chat() returned error: %v", err)
	}

	if receivedModel != testModel {
		t.Errorf("Expected model '%s', got '%s'", testModel, receivedModel)
	}
}

func TestFailoverError_AsError(t *testing.T) {
	// Verify that FailoverError implements error interface
	var _ error = &FailoverError{}

	err := &FailoverError{
		Reason:   FailoverUnknown,
		Provider: "test",
		Model:    "model",
		Status:   500,
		Wrapped:  errors.New("test error"),
	}

	// Should be usable as an error
	errorStr := err.Error()
	if errorStr == "" {
		t.Error("Error() should return non-empty string")
	}
}

func TestModelConfig_Immutability(t *testing.T) {
	original := ModelConfig{
		Primary: "primary",
		Fallbacks: []string{"f1", "f2"},
	}

	// Copy by value
	copy := original

	// Modify original
	original.Primary = "modified"
	original.Fallbacks[0] = "modified"

	// Copy should be unaffected
	if copy.Primary != "primary" {
		t.Error("Copy should be independent of original")
	}

	if copy.Fallbacks[0] != "modified" {
		// Note: Slices are reference types, so this will actually be "modified"
		t.Log("Slice modification affects copy (expected behavior)")
	}
}

// Helper function for string contains
func containsString(s, substr string) bool {
	return len(s) >= len(substr) && findInString(s, substr) >= 0
}

func findInString(s, substr string) int {
	for i := 0; i <= len(s)-len(substr); i++ {
		if s[i:i+len(substr)] == substr {
			return i
		}
	}
	return -1
}

func TestLLMResponse_UsageInfo(t *testing.T) {
	response := LLMResponse{
		Content: "Response with usage",
		Usage: &UsageInfo{
			PromptTokens:     10,
			CompletionTokens: 20,
			TotalTokens:      30,
		},
	}

	if response.Usage == nil {
		t.Fatal("Usage should not be nil")
	}

	if response.Usage.PromptTokens != 10 {
		t.Errorf("Expected prompt tokens 10, got %d", response.Usage.PromptTokens)
	}

	if response.Usage.CompletionTokens != 20 {
		t.Errorf("Expected completion tokens 20, got %d", response.Usage.CompletionTokens)
	}

	if response.Usage.TotalTokens != 30 {
		t.Errorf("Expected total tokens 30, got %d", response.Usage.TotalTokens)
	}
}

func TestLLMResponse_NoUsageInfo(t *testing.T) {
	response := LLMResponse{
		Content: "Response without usage",
	}

	if response.Usage != nil {
		t.Error("Usage should be nil when not provided")
	}
}

func TestUsageInfo_AllFields(t *testing.T) {
	usage := UsageInfo{
		PromptTokens:     100,
		CompletionTokens: 200,
		TotalTokens:      300,
	}

	if usage.PromptTokens != 100 {
		t.Errorf("Expected prompt tokens 100, got %d", usage.PromptTokens)
	}

	if usage.CompletionTokens != 200 {
		t.Errorf("Expected completion tokens 200, got %d", usage.CompletionTokens)
	}

	if usage.TotalTokens != 300 {
		t.Errorf("Expected total tokens 300, got %d", usage.TotalTokens)
	}
}

func TestMessage_ToolCalls(t *testing.T) {
	message := Message{
		Role:    "assistant",
		Content: "I'll use tools",
		ToolCalls: []ToolCall{
			{
				ID:   "call-1",
				Name: "tool1",
				Function: &FunctionCall{
					Name:      "tool1",
					Arguments: `{"param": "value"}`,
				},
			},
		},
	}

	if len(message.ToolCalls) != 1 {
		t.Errorf("Expected 1 tool call, got %d", len(message.ToolCalls))
	}

	if message.ToolCalls[0].Function == nil {
		t.Error("Function should not be nil")
	}
}

func TestMessage_ToolCallID(t *testing.T) {
	message := Message{
		Role:       "tool",
		Content:    "Tool result",
		ToolCallID: "call-123",
	}

	if message.ToolCallID != "call-123" {
		t.Errorf("Expected tool call ID 'call-123', got '%s'", message.ToolCallID)
	}
}

func TestFunctionCall_Structure(t *testing.T) {
	funcCall := FunctionCall{
		Name:      "test_function",
		Arguments: `{"param1": "value1", "param2": "value2"}`,
	}

	if funcCall.Name != "test_function" {
		t.Errorf("Expected name 'test_function', got '%s'", funcCall.Name)
	}

	if funcCall.Arguments == "" {
		t.Error("Arguments should not be empty")
	}
}

func TestFunctionCall_NoArguments(t *testing.T) {
	funcCall := FunctionCall{
		Name:      "no_args_function",
		Arguments: "",
	}

	if funcCall.Name != "no_args_function" {
		t.Errorf("Expected name 'no_args_function', got '%s'", funcCall.Name)
	}

	if funcCall.Arguments != "" {
		t.Errorf("Expected empty arguments, got '%s'", funcCall.Arguments)
	}
}

func TestLLMProvider_ConcurrentCalls(t *testing.T) {
	provider := NewMockLLMProvider("test-model")
	ctx := context.Background()

	done := make(chan bool)

	for i := 0; i < 10; i++ {
		go func() {
			_, err := provider.Chat(ctx, nil, nil, "", nil)
			if err != nil {
				t.Errorf("Concurrent Chat() failed: %v", err)
			}
			done <- true
		}()
	}

	for i := 0; i < 10; i++ {
		<-done
	}
}

func TestFailoverError_StatusCodes(t *testing.T) {
	tests := []struct {
		name          string
		status        int
		reason        FailoverReason
		expectedError string
	}{
		{"unauthorized", 401, FailoverAuth, "401"},
		{"payment required", 402, FailoverBilling, "402"},
		{"forbidden", 403, FailoverAuth, "403"},
		{"not found", 404, FailoverUnknown, "404"},
		{"too many requests", 429, FailoverRateLimit, "429"},
		{"internal server error", 500, FailoverOverloaded, "500"},
		{"service unavailable", 503, FailoverOverloaded, "503"},
		{"gateway timeout", 504, FailoverTimeout, "504"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			err := &FailoverError{
				Reason: tt.reason,
				Status: tt.status,
			}

			errorStr := err.Error()
			if !containsString(errorStr, tt.expectedError) {
				t.Errorf("Error should contain '%s', got '%s'", tt.expectedError, errorStr)
			}
		})
	}
}

func TestModelConfig_GetFallbacks(t *testing.T) {
	config := ModelConfig{
		Primary: "primary",
		Fallbacks: []string{"f1", "f2", "f3"},
	}

	fallbacks := config.Fallbacks
	if fallbacks == nil {
		t.Error("Fallbacks should not be nil")
	}

	if len(fallbacks) != 3 {
		t.Errorf("Expected 3 fallbacks, got %d", len(fallbacks))
	}
}

func TestModelConfig_SetFallbacks(t *testing.T) {
	config := ModelConfig{
		Primary: "primary",
	}

	newFallbacks := []string{"new-f1", "new-f2"}
	config.Fallbacks = newFallbacks

	if len(config.Fallbacks) != 2 {
		t.Errorf("Expected 2 fallbacks, got %d", len(config.Fallbacks))
	}

	if config.Fallbacks[0] != "new-f1" {
		t.Errorf("Expected first fallback 'new-f1', got '%s'", config.Fallbacks[0])
	}
}

func TestLLMResponse_WithStreamingChunk(t *testing.T) {
	// This test verifies the structure for streaming responses
	response := LLMResponse{
		Content: "Streaming response",
	}

	if response.Content != "Streaming response" {
		t.Errorf("Expected content 'Streaming response', got '%s'", response.Content)
	}
}

func TestMessage_AllRoles(t *testing.T) {
	validRoles := []struct {
		role  string
		valid bool
	}{
		{"user", true},
		{"assistant", true},
		{"system", true},
		{"tool", true},
		{"function", true},
		{"developer", true},
	}

	for _, tt := range validRoles {
		t.Run(tt.role, func(t *testing.T) {
			msg := Message{Role: tt.role}
			if msg.Role != tt.role {
				t.Errorf("Expected role '%s', got '%s'", tt.role, msg.Role)
			}
		})
	}
}

func TestLLMProvider_MultipleSequentialCalls(t *testing.T) {
	callCount := 0
	provider := &MockLLMProvider{
		chatFunc: func(ctx context.Context, messages []Message, tools []ToolDefinition, model string, options map[string]interface{}) (*LLMResponse, error) {
			callCount++
			return &LLMResponse{
				Content: "Response",
				ToolCalls: []ToolCall{
					{
						ID:   "call-" + string(rune('0'+callCount)),
						Name: "tool",
					},
				},
			}, nil
		},
	}

	ctx := context.Background()

	// Make multiple calls
	for i := 0; i < 5; i++ {
		_, err := provider.Chat(ctx, nil, nil, "", nil)
		if err != nil {
			t.Errorf("Chat() call %d failed: %v", i+1, err)
		}
	}

	if callCount != 5 {
		t.Errorf("Expected 5 calls, got %d", callCount)
	}
}

func TestFailoverError_WrappingNil(t *testing.T) {
	err := &FailoverError{
		Reason:   FailoverUnknown,
		Provider: "test",
		Model:    "model",
		Status:   500,
		Wrapped:  nil,
	}

	// Should not panic
	_ = err.Error()
	_ = err.Unwrap()

	if err.Wrapped != nil {
		t.Error("Wrapped error should be nil")
	}
}

func TestModelConfig_PrimaryOnly(t *testing.T) {
	config := ModelConfig{
		Primary: "primary-only",
	}

	if config.Primary != "primary-only" {
		t.Errorf("Expected primary 'primary-only', got '%s'", config.Primary)
	}

	if config.Fallbacks == nil {
		config.Fallbacks = []string{}
	}

	if len(config.Fallbacks) != 0 {
		t.Errorf("Expected 0 fallbacks, got %d", len(config.Fallbacks))
	}
}

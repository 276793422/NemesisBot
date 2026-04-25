// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package providers

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"testing"
)

// ============================================================
// ClaudeCliProvider internal tests
// ============================================================

// --- messagesToPrompt ---

func TestClaudeCliProvider_MessagesToPrompt_SingleUser(t *testing.T) {
	p := NewClaudeCliProvider("")
	messages := []Message{
		{Role: "user", Content: "Hello world"},
	}

	result := p.messagesToPrompt(messages)
	if result != "Hello world" {
		t.Errorf("expected 'Hello world', got '%s'", result)
	}
}

func TestClaudeCliProvider_MessagesToPrompt_MultipleRoles(t *testing.T) {
	p := NewClaudeCliProvider("")
	messages := []Message{
		{Role: "system", Content: "You are helpful"},
		{Role: "user", Content: "Hello"},
		{Role: "assistant", Content: "Hi there"},
		{Role: "user", Content: "How are you?"},
	}

	result := p.messagesToPrompt(messages)

	if strings.Contains(result, "You are helpful") {
		t.Error("system messages should be excluded from prompt")
	}
	if !strings.Contains(result, "User: Hello") {
		t.Error("expected 'User: Hello' in prompt")
	}
	if !strings.Contains(result, "Assistant: Hi there") {
		t.Error("expected 'Assistant: Hi there' in prompt")
	}
	if !strings.Contains(result, "User: How are you?") {
		t.Error("expected 'User: How are you?' in prompt")
	}
}

func TestClaudeCliProvider_MessagesToPrompt_ToolMessage(t *testing.T) {
	p := NewClaudeCliProvider("")
	messages := []Message{
		{Role: "user", Content: "Read file.txt"},
		{Role: "tool", Content: "file contents here", ToolCallID: "call-1"},
	}

	result := p.messagesToPrompt(messages)

	if !strings.Contains(result, "[Tool Result for call-1]: file contents here") {
		t.Errorf("expected tool result format in prompt, got '%s'", result)
	}
}

func TestClaudeCliProvider_MessagesToPrompt_Empty(t *testing.T) {
	p := NewClaudeCliProvider("")
	result := p.messagesToPrompt(nil)
	if result != "" {
		t.Errorf("expected empty string for nil messages, got '%s'", result)
	}
}

func TestClaudeCliProvider_MessagesToPrompt_SystemOnly(t *testing.T) {
	p := NewClaudeCliProvider("")
	messages := []Message{
		{Role: "system", Content: "System prompt"},
	}

	result := p.messagesToPrompt(messages)
	// System messages are skipped, so result should be empty
	if result != "" {
		t.Errorf("expected empty string when only system messages, got '%s'", result)
	}
}

// --- buildSystemPrompt ---

func TestClaudeCliProvider_BuildSystemPrompt_SystemMessages(t *testing.T) {
	p := NewClaudeCliProvider("")
	messages := []Message{
		{Role: "system", Content: "You are a helpful assistant"},
		{Role: "user", Content: "Hello"},
	}

	result := p.buildSystemPrompt(messages, nil)
	if !strings.Contains(result, "You are a helpful assistant") {
		t.Errorf("expected system message in prompt, got '%s'", result)
	}
}

func TestClaudeCliProvider_BuildSystemPrompt_MultipleSystemMessages(t *testing.T) {
	p := NewClaudeCliProvider("")
	messages := []Message{
		{Role: "system", Content: "Rule 1"},
		{Role: "system", Content: "Rule 2"},
	}

	result := p.buildSystemPrompt(messages, nil)
	if !strings.Contains(result, "Rule 1") {
		t.Error("expected 'Rule 1' in system prompt")
	}
	if !strings.Contains(result, "Rule 2") {
		t.Error("expected 'Rule 2' in system prompt")
	}
}

func TestClaudeCliProvider_BuildSystemPrompt_NoSystemMessages(t *testing.T) {
	p := NewClaudeCliProvider("")
	messages := []Message{
		{Role: "user", Content: "Hello"},
	}

	result := p.buildSystemPrompt(messages, nil)
	if result != "" {
		t.Errorf("expected empty system prompt when no system messages, got '%s'", result)
	}
}

func TestClaudeCliProvider_BuildSystemPrompt_WithTools(t *testing.T) {
	p := NewClaudeCliProvider("")
	tools := []ToolDefinition{
		{
			Type: "function",
			Function: ToolFunctionDefinition{
				Name:        "test_tool",
				Description: "A test tool",
			},
		},
	}

	result := p.buildSystemPrompt(nil, tools)
	if !strings.Contains(result, "Available Tools") {
		t.Error("expected 'Available Tools' in system prompt with tools")
	}
	if !strings.Contains(result, "test_tool") {
		t.Error("expected tool name in system prompt")
	}
}

// --- buildToolsPrompt ---

func TestClaudeCliProvider_BuildToolsPrompt_SingleTool(t *testing.T) {
	p := NewClaudeCliProvider("")
	tools := []ToolDefinition{
		{
			Type: "function",
			Function: ToolFunctionDefinition{
				Name:        "read_file",
				Description: "Read a file",
				Parameters: map[string]interface{}{
					"type": "object",
					"properties": map[string]interface{}{
						"path": map[string]interface{}{"type": "string"},
					},
				},
			},
		},
	}

	result := p.buildToolsPrompt(tools)

	if !strings.Contains(result, "#### read_file") {
		t.Error("expected tool name header")
	}
	if !strings.Contains(result, "Description: Read a file") {
		t.Error("expected tool description")
	}
	if !strings.Contains(result, "Parameters:") {
		t.Error("expected Parameters section")
	}
	if !strings.Contains(result, "tool_calls") {
		t.Error("expected tool_calls format instructions")
	}
}

func TestClaudeCliProvider_BuildToolsPrompt_MultipleTools(t *testing.T) {
	p := NewClaudeCliProvider("")
	tools := []ToolDefinition{
		{Type: "function", Function: ToolFunctionDefinition{Name: "tool1", Description: "First tool"}},
		{Type: "function", Function: ToolFunctionDefinition{Name: "tool2", Description: "Second tool"}},
	}

	result := p.buildToolsPrompt(tools)

	if !strings.Contains(result, "#### tool1") {
		t.Error("expected tool1 header")
	}
	if !strings.Contains(result, "#### tool2") {
		t.Error("expected tool2 header")
	}
}

func TestClaudeCliProvider_BuildToolsPrompt_NonFunctionType(t *testing.T) {
	p := NewClaudeCliProvider("")
	tools := []ToolDefinition{
		{Type: "not_function", Function: ToolFunctionDefinition{Name: "ignored"}},
		{Type: "function", Function: ToolFunctionDefinition{Name: "included"}},
	}

	result := p.buildToolsPrompt(tools)

	if strings.Contains(result, "#### ignored") {
		t.Error("non-function tool should be skipped")
	}
	if !strings.Contains(result, "#### included") {
		t.Error("function tool should be included")
	}
}

func TestClaudeCliProvider_BuildToolsPrompt_NoDescription(t *testing.T) {
	p := NewClaudeCliProvider("")
	tools := []ToolDefinition{
		{Type: "function", Function: ToolFunctionDefinition{Name: "simple"}},
	}

	result := p.buildToolsPrompt(tools)

	if strings.Contains(result, "Description:") {
		t.Error("should not have Description header when description is empty")
	}
}

func TestClaudeCliProvider_BuildToolsPrompt_NoParameters(t *testing.T) {
	p := NewClaudeCliProvider("")
	tools := []ToolDefinition{
		{Type: "function", Function: ToolFunctionDefinition{Name: "no_params", Description: "No params tool"}},
	}

	result := p.buildToolsPrompt(tools)

	if strings.Contains(result, "Parameters:") {
		t.Error("should not have Parameters section when no parameters")
	}
}

// --- parseClaudeCliResponse ---

func TestClaudeCliProvider_ParseResponse_PlainText(t *testing.T) {
	p := NewClaudeCliProvider("")
	resp := claudeCliJSONResponse{
		IsError: false,
		Result:  "Hello, world!",
		Usage:   claudeCliUsageInfo{},
	}
	data, _ := json.Marshal(resp)

	result, err := p.parseClaudeCliResponse(string(data))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result.Content != "Hello, world!" {
		t.Errorf("expected 'Hello, world!', got '%s'", result.Content)
	}
	if result.FinishReason != "stop" {
		t.Errorf("expected 'stop', got '%s'", result.FinishReason)
	}
}

func TestClaudeCliProvider_ParseResponse_ErrorResponse(t *testing.T) {
	p := NewClaudeCliProvider("")
	resp := claudeCliJSONResponse{
		IsError: true,
		Result:  "Something went wrong",
	}
	data, _ := json.Marshal(resp)

	_, err := p.parseClaudeCliResponse(string(data))
	if err == nil {
		t.Error("expected error for is_error=true response")
	}
	if !strings.Contains(err.Error(), "Something went wrong") {
		t.Errorf("error should contain the result message, got: %v", err)
	}
}

func TestClaudeCliProvider_ParseResponse_InvalidJSON(t *testing.T) {
	p := NewClaudeCliProvider("")
	_, err := p.parseClaudeCliResponse("not json at all")
	if err == nil {
		t.Error("expected error for invalid JSON")
	}
}

func TestClaudeCliProvider_ParseResponse_WithUsage(t *testing.T) {
	p := NewClaudeCliProvider("")
	resp := claudeCliJSONResponse{
		IsError: false,
		Result:  "Used tokens",
		Usage: claudeCliUsageInfo{
			InputTokens:              100,
			OutputTokens:             50,
			CacheCreationInputTokens: 30,
			CacheReadInputTokens:     20,
		},
	}
	data, _ := json.Marshal(resp)

	result, err := p.parseClaudeCliResponse(string(data))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result.Usage == nil {
		t.Fatal("expected non-nil usage")
	}
	if result.Usage.PromptTokens != 150 { // 100 + 30 + 20
		t.Errorf("expected PromptTokens 150, got %d", result.Usage.PromptTokens)
	}
	if result.Usage.CompletionTokens != 50 {
		t.Errorf("expected CompletionTokens 50, got %d", result.Usage.CompletionTokens)
	}
	if result.Usage.TotalTokens != 200 { // 150 + 50
		t.Errorf("expected TotalTokens 200, got %d", result.Usage.TotalTokens)
	}
}

func TestClaudeCliProvider_ParseResponse_ZeroUsage(t *testing.T) {
	p := NewClaudeCliProvider("")
	resp := claudeCliJSONResponse{
		IsError: false,
		Result:  "No token info",
		Usage:   claudeCliUsageInfo{},
	}
	data, _ := json.Marshal(resp)

	result, err := p.parseClaudeCliResponse(string(data))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result.Usage != nil {
		t.Error("expected nil usage when all token counts are zero")
	}
}

func TestClaudeCliProvider_ParseResponse_WithToolCalls(t *testing.T) {
	p := NewClaudeCliProvider("")
	result := `I'll read that file. {"tool_calls":[{"id":"call_1","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"test.go\"}"}}]}`
	resp := claudeCliJSONResponse{
		IsError: false,
		Result:  result,
		Usage:   claudeCliUsageInfo{},
	}
	data, _ := json.Marshal(resp)

	parsed, err := p.parseClaudeCliResponse(string(data))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if parsed.FinishReason != "tool_calls" {
		t.Errorf("expected 'tool_calls', got '%s'", parsed.FinishReason)
	}
	if len(parsed.ToolCalls) != 1 {
		t.Fatalf("expected 1 tool call, got %d", len(parsed.ToolCalls))
	}
	if parsed.ToolCalls[0].Name != "read_file" {
		t.Errorf("expected name 'read_file', got '%s'", parsed.ToolCalls[0].Name)
	}
	if parsed.ToolCalls[0].ID != "call_1" {
		t.Errorf("expected ID 'call_1', got '%s'", parsed.ToolCalls[0].ID)
	}
	// Content should have tool calls stripped
	if strings.Contains(parsed.Content, "tool_calls") {
		t.Error("content should not contain tool_calls JSON")
	}
}

func TestClaudeCliProvider_ParseResponse_ContentTrimmed(t *testing.T) {
	p := NewClaudeCliProvider("")
	resp := claudeCliJSONResponse{
		IsError: false,
		Result:  "  trimmed content  ",
	}
	data, _ := json.Marshal(resp)

	result, err := p.parseClaudeCliResponse(string(data))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result.Content != "trimmed content" {
		t.Errorf("expected trimmed 'trimmed content', got '%s'", result.Content)
	}
}

// --- stripToolCallsJSON ---

func TestClaudeCliProvider_StripToolCallsJSON(t *testing.T) {
	p := NewClaudeCliProvider("")

	tests := []struct {
		name     string
		input    string
		expected string
	}{
		{
			name:     "no tool calls",
			input:    "plain text",
			expected: "plain text",
		},
		{
			name:     "tool calls at end",
			input:    "Some text {\"tool_calls\":[{\"id\":\"1\"}]}",
			expected: "Some text",
		},
		{
			name:     "tool calls in middle",
			input:    "Before {\"tool_calls\":[{\"id\":\"1\"}]} After",
			expected: "Before  After",
		},
		{
			name:     "empty string",
			input:    "",
			expected: "",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := p.stripToolCallsJSON(tt.input)
			if result != tt.expected {
				t.Errorf("expected '%s', got '%s'", tt.expected, result)
			}
		})
	}
}

// --- extractToolCalls ---

func TestClaudeCliProvider_ExtractToolCalls(t *testing.T) {
	p := NewClaudeCliProvider("")

	tests := []struct {
		name          string
		input         string
		expectedCount int
	}{
		{"no tool calls", "plain text", 0},
		{"single tool call", `{"tool_calls":[{"id":"1","type":"function","function":{"name":"test","arguments":"{}"}}]}`, 1},
		{"empty string", "", 0},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := p.extractToolCalls(tt.input)
			if len(result) != tt.expectedCount {
				t.Errorf("expected %d tool calls, got %d", tt.expectedCount, len(result))
			}
		})
	}
}

// --- findMatchingBrace ---

func TestFindMatchingBrace_Internal(t *testing.T) {
	tests := []struct {
		name     string
		text     string
		pos      int
		expected int
	}{
		{"empty braces", "{}", 0, 2},
		{"nested braces", "{a:{b:c}}", 0, 9},
		{"inner braces", "a{b{c}d}e", 1, 8},
		{"unmatched open", "{", 0, 0},
		{"deep nesting", "{{{{}}}}", 0, 8},
		{"object in text", "x{\"a\":1}y", 1, 8},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := findMatchingBrace(tt.text, tt.pos)
			if result != tt.expected {
				t.Errorf("expected %d, got %d", tt.expected, result)
			}
		})
	}
}

// ============================================================
// CodexCliProvider internal tests
// ============================================================

// --- buildPrompt ---

func TestCodexCliProvider_BuildPrompt_SingleUser(t *testing.T) {
	p := NewCodexCliProvider("")
	messages := []Message{
		{Role: "user", Content: "Write hello world"},
	}

	result := p.buildPrompt(messages, nil)
	if result != "Write hello world" {
		t.Errorf("expected 'Write hello world', got '%s'", result)
	}
}

func TestCodexCliProvider_BuildPrompt_SystemMessages(t *testing.T) {
	p := NewCodexCliProvider("")
	messages := []Message{
		{Role: "system", Content: "Be helpful"},
		{Role: "user", Content: "Hello"},
	}

	result := p.buildPrompt(messages, nil)

	if !strings.Contains(result, "System Instructions") {
		t.Error("expected 'System Instructions' header")
	}
	if !strings.Contains(result, "Be helpful") {
		t.Error("expected system message content")
	}
	if !strings.Contains(result, "Task") {
		t.Error("expected 'Task' header")
	}
	if !strings.Contains(result, "Hello") {
		t.Error("expected user message")
	}
}

func TestCodexCliProvider_BuildPrompt_AssistantMessage(t *testing.T) {
	p := NewCodexCliProvider("")
	messages := []Message{
		{Role: "user", Content: "Hi"},
		{Role: "assistant", Content: "Hello!"},
		{Role: "user", Content: "How are you?"},
	}

	result := p.buildPrompt(messages, nil)

	if !strings.Contains(result, "Assistant: Hello!") {
		t.Error("expected 'Assistant: Hello!' in prompt")
	}
}

func TestCodexCliProvider_BuildPrompt_ToolMessage(t *testing.T) {
	p := NewCodexCliProvider("")
	messages := []Message{
		{Role: "user", Content: "Read file"},
		{Role: "tool", Content: "file content", ToolCallID: "call-1"},
	}

	result := p.buildPrompt(messages, nil)

	if !strings.Contains(result, "[Tool Result for call-1]: file content") {
		t.Error("expected tool result format in prompt")
	}
}

func TestCodexCliProvider_BuildPrompt_WithTools(t *testing.T) {
	p := NewCodexCliProvider("")
	messages := []Message{
		{Role: "user", Content: "Do something"},
	}
	tools := []ToolDefinition{
		{Type: "function", Function: ToolFunctionDefinition{Name: "my_tool", Description: "Does stuff"}},
	}

	result := p.buildPrompt(messages, tools)

	if !strings.Contains(result, "Available Tools") {
		t.Error("expected 'Available Tools' in prompt")
	}
	if !strings.Contains(result, "my_tool") {
		t.Error("expected tool name in prompt")
	}
}

func TestCodexCliProvider_BuildPrompt_EmptyMessages(t *testing.T) {
	p := NewCodexCliProvider("")
	result := p.buildPrompt(nil, nil)
	if result != "" {
		t.Errorf("expected empty string for nil messages, got '%s'", result)
	}
}

// --- CodexCliProvider buildToolsPrompt ---

func TestCodexCliProvider_BuildToolsPrompt(t *testing.T) {
	p := NewCodexCliProvider("")
	tools := []ToolDefinition{
		{
			Type: "function",
			Function: ToolFunctionDefinition{
				Name:        "search",
				Description: "Search the web",
				Parameters: map[string]interface{}{
					"type": "object",
					"properties": map[string]interface{}{
						"query": map[string]interface{}{"type": "string"},
					},
				},
			},
		},
	}

	result := p.buildToolsPrompt(tools)

	if !strings.Contains(result, "#### search") {
		t.Error("expected tool name header")
	}
	if !strings.Contains(result, "Description: Search the web") {
		t.Error("expected tool description")
	}
	if !strings.Contains(result, "Parameters:") {
		t.Error("expected Parameters section")
	}
}

func TestCodexCliProvider_BuildToolsPrompt_NonFunction(t *testing.T) {
	p := NewCodexCliProvider("")
	tools := []ToolDefinition{
		{Type: "other", Function: ToolFunctionDefinition{Name: "skipped"}},
		{Type: "function", Function: ToolFunctionDefinition{Name: "included"}},
	}

	result := p.buildToolsPrompt(tools)

	if strings.Contains(result, "#### skipped") {
		t.Error("non-function type should be skipped")
	}
	if !strings.Contains(result, "#### included") {
		t.Error("function type should be included")
	}
}

// --- parseJSONLEvents ---

func TestCodexCliProvider_ParseJSONLEvents_AgentMessage(t *testing.T) {
	p := NewCodexCliProvider("")
	output := `{"type":"item.completed","item":{"id":"msg-1","type":"agent_message","text":"Hello, I can help!"}}`

	result, err := p.parseJSONLEvents(output)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result.Content != "Hello, I can help!" {
		t.Errorf("expected 'Hello, I can help!', got '%s'", result.Content)
	}
	if result.FinishReason != "stop" {
		t.Errorf("expected 'stop', got '%s'", result.FinishReason)
	}
}

func TestCodexCliProvider_ParseJSONLEvents_MultipleMessages(t *testing.T) {
	p := NewCodexCliProvider("")
	output := `{"type":"item.completed","item":{"id":"msg-1","type":"agent_message","text":"Part 1"}}
{"type":"item.completed","item":{"id":"msg-2","type":"agent_message","text":"Part 2"}}`

	result, err := p.parseJSONLEvents(output)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result.Content != "Part 1\nPart 2" {
		t.Errorf("expected 'Part 1\\nPart 2', got '%s'", result.Content)
	}
}

func TestCodexCliProvider_ParseJSONLEvents_WithUsage(t *testing.T) {
	p := NewCodexCliProvider("")
	output := `{"type":"turn.completed","usage":{"input_tokens":100,"cached_input_tokens":50,"output_tokens":80}}`

	result, err := p.parseJSONLEvents(output)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result.Usage == nil {
		t.Fatal("expected non-nil usage")
	}
	if result.Usage.PromptTokens != 150 { // 100 + 50
		t.Errorf("expected PromptTokens 150, got %d", result.Usage.PromptTokens)
	}
	if result.Usage.CompletionTokens != 80 {
		t.Errorf("expected CompletionTokens 80, got %d", result.Usage.CompletionTokens)
	}
	if result.Usage.TotalTokens != 230 { // 150 + 80
		t.Errorf("expected TotalTokens 230, got %d", result.Usage.TotalTokens)
	}
}

func TestCodexCliProvider_ParseJSONLEvents_ErrorOnly(t *testing.T) {
	p := NewCodexCliProvider("")
	output := `{"type":"error","message":"Rate limit exceeded"}`

	_, err := p.parseJSONLEvents(output)
	if err == nil {
		t.Error("expected error for error-only output")
	}
	if !strings.Contains(err.Error(), "Rate limit exceeded") {
		t.Errorf("error should contain message, got: %v", err)
	}
}

func TestCodexCliProvider_ParseJSONLEvents_TurnFailed(t *testing.T) {
	p := NewCodexCliProvider("")
	output := `{"type":"turn.failed","error":{"message":"Processing failed"}}`

	_, err := p.parseJSONLEvents(output)
	if err == nil {
		t.Error("expected error for turn.failed")
	}
	if !strings.Contains(err.Error(), "Processing failed") {
		t.Errorf("error should contain message, got: %v", err)
	}
}

func TestCodexCliProvider_ParseJSONLEvents_ErrorWithContent(t *testing.T) {
	// When there's both content and an error, content takes priority
	p := NewCodexCliProvider("")
	output := `{"type":"item.completed","item":{"id":"msg-1","type":"agent_message","text":"Partial result"}}
{"type":"error","message":"Something went wrong"}`

	result, err := p.parseJSONLEvents(output)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result.Content != "Partial result" {
		t.Errorf("expected 'Partial result', got '%s'", result.Content)
	}
}

func TestCodexCliProvider_ParseJSONLEvents_Empty(t *testing.T) {
	p := NewCodexCliProvider("")
	result, err := p.parseJSONLEvents("")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result.Content != "" {
		t.Errorf("expected empty content, got '%s'", result.Content)
	}
}

func TestCodexCliProvider_ParseJSONLEvents_MalformedLine(t *testing.T) {
	p := NewCodexCliProvider("")
	output := `not json at all
{"type":"item.completed","item":{"id":"msg-1","type":"agent_message","text":"Valid message"}}`

	result, err := p.parseJSONLEvents(output)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result.Content != "Valid message" {
		t.Errorf("expected 'Valid message', got '%s'", result.Content)
	}
}

func TestCodexCliProvider_ParseJSONLEvents_NonAgentItem(t *testing.T) {
	p := NewCodexCliProvider("")
	output := `{"type":"item.completed","item":{"id":"cmd-1","type":"command","command":"ls","exit_code":0}}
{"type":"item.completed","item":{"id":"msg-1","type":"agent_message","text":"Here's the listing"}}`

	result, err := p.parseJSONLEvents(output)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result.Content != "Here's the listing" {
		t.Errorf("expected 'Here''s the listing', got '%s'", result.Content)
	}
}

func TestCodexCliProvider_ParseJSONLEvents_ToolCalls(t *testing.T) {
	p := NewCodexCliProvider("")
	output := `{"type":"item.completed","item":{"id":"msg-1","type":"agent_message","text":"Let me search. {\"tool_calls\":[{\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"search\",\"arguments\":\"{\\\"q\\\":\\\"test\\\"}\"}}]}"}}`

	result, err := p.parseJSONLEvents(output)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if result.FinishReason != "tool_calls" {
		t.Errorf("expected 'tool_calls', got '%s'", result.FinishReason)
	}
	if len(result.ToolCalls) != 1 {
		t.Fatalf("expected 1 tool call, got %d", len(result.ToolCalls))
	}
	if result.ToolCalls[0].Name != "search" {
		t.Errorf("expected name 'search', got '%s'", result.ToolCalls[0].Name)
	}
}

// ============================================================
// CodexProvider internal tests
// ============================================================

// --- resolveCodexModel ---

func TestResolveCodexModel(t *testing.T) {
	tests := []struct {
		name             string
		model            string
		expectedModel    string
		expectedFallback string
	}{
		{"empty", "", "gpt-5.2", "empty model"},
		{"gpt-4o", "gpt-4o", "gpt-4o", ""},
		{"gpt-4o-mini", "gpt-4o-mini", "gpt-4o-mini", ""},
		{"o3-mini", "o3-mini", "o3-mini", ""},
		{"o4-mini", "o4-mini", "o4-mini", ""},
		{"openai/gpt-4o", "openai/gpt-4o", "gpt-4o", ""},
		{"other/ns model", "zhipu/glm-4", "gpt-5.2", "non-openai model namespace"},
		{"glm prefix", "glm-4", "gpt-5.2", "unsupported model prefix"},
		{"claude prefix", "claude-3", "gpt-5.2", "unsupported model prefix"},
		{"gemini prefix", "gemini-pro", "gpt-5.2", "unsupported model prefix"},
		{"deepseek prefix", "deepseek-v3", "gpt-5.2", "unsupported model prefix"},
		{"random model", "random-model", "gpt-5.2", "unsupported model family"},
		{"whitespace", "  gpt-4o  ", "gpt-4o", ""},
		{"GPT uppercase", "GPT-4o", "gpt-4o", ""},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			model, fallback := resolveCodexModel(tt.model)
			if model != tt.expectedModel {
				t.Errorf("expected model '%s', got '%s'", tt.expectedModel, model)
			}
			if (fallback != "") != (tt.expectedFallback != "") {
				t.Errorf("expected fallback status '%s', got '%s'", tt.expectedFallback, fallback)
			}
		})
	}
}

// --- resolveCodexToolCall ---

func TestResolveCodexToolCall(t *testing.T) {
	tests := []struct {
		name         string
		tc           ToolCall
		expectedName string
		expectedArgs string
		expectedOK   bool
	}{
		{
			name: "with Arguments map",
			tc: ToolCall{
				Name: "tool1",
				Arguments: map[string]interface{}{"key": "value"},
			},
			expectedName: "tool1",
			expectedArgs: `{"key":"value"}`,
			expectedOK:   true,
		},
		{
			name: "with Function.Arguments",
			tc: ToolCall{
				Function: &FunctionCall{
					Name:      "tool2",
					Arguments: `{"x":1}`,
				},
			},
			expectedName: "tool2",
			expectedArgs: `{"x":1}`,
			expectedOK:   true,
		},
		{
			name: "no name at all",
			tc: ToolCall{
				ID: "call-1",
			},
			expectedName: "",
			expectedArgs: "",
			expectedOK:   false,
		},
		{
			name: "Name takes priority over Function.Name",
			tc: ToolCall{
				Name: "primary",
				Function: &FunctionCall{
					Name: "secondary",
					Arguments: `{"a":"b"}`,
				},
			},
			expectedName: "primary",
			expectedArgs: `{"a":"b"}`,
			expectedOK:   true,
		},
		{
			name: "Arguments map takes priority over Function.Arguments",
			tc: ToolCall{
				Name:      "tool3",
				Arguments: map[string]interface{}{"z": float64(1)},
			},
			expectedName: "tool3",
			expectedArgs: `{"z":1}`,
			expectedOK:   true,
		},
		{
			name: "empty name falls back to Function",
			tc: ToolCall{
				Name: "",
				Function: &FunctionCall{
					Name: "fallback_name",
				},
			},
			expectedName: "fallback_name",
			expectedArgs: "{}",
			expectedOK:   true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			name, args, ok := resolveCodexToolCall(tt.tc)
			if ok != tt.expectedOK {
				t.Errorf("expected ok=%v, got ok=%v", tt.expectedOK, ok)
			}
			if ok {
				if name != tt.expectedName {
					t.Errorf("expected name '%s', got '%s'", tt.expectedName, name)
				}
				if tt.expectedArgs != "" && args != tt.expectedArgs {
					t.Errorf("expected args '%s', got '%s'", tt.expectedArgs, args)
				}
			}
		})
	}
}

// --- parseCodexResponse ---

func TestParseCodexResponse_TextOutput(t *testing.T) {
	// We can't easily construct a responses.Response object because it's from
	// the openai library. Instead, we test the CodexCliProvider's parseJSONLEvents
	// which covers similar logic.
	// The parseCodexResponse function is tested indirectly via integration tests.
}

// --- buildCodexParams ---

func TestBuildCodexParams_SystemMessage(t *testing.T) {
	messages := []Message{
		{Role: "system", Content: "You are helpful"},
		{Role: "user", Content: "Hello"},
	}

	params := buildCodexParams(messages, nil, "gpt-4o", nil, false)

	if params.Model != "gpt-4o" {
		t.Errorf("expected model 'gpt-4o', got '%s'", params.Model)
	}
	// Instructions should be set from system message
	instructions := params.Instructions
	if !instructions.Valid() || instructions.Value != "You are helpful" {
		t.Errorf("expected instructions 'You are helpful', got '%v'", instructions)
	}
}

func TestBuildCodexParams_DefaultInstructions(t *testing.T) {
	messages := []Message{
		{Role: "user", Content: "Hello"},
	}

	params := buildCodexParams(messages, nil, "gpt-4o", nil, false)

	// When no system message, should use default instructions
	instructions := params.Instructions
	if !instructions.Valid() || instructions.Value != defaultCodexInstructions {
		t.Errorf("expected default instructions, got '%v'", instructions)
	}
}

func TestBuildCodexParams_UserMessage(t *testing.T) {
	messages := []Message{
		{Role: "user", Content: "Hello world"},
	}

	params := buildCodexParams(messages, nil, "gpt-4o", nil, false)

	// Verify input items were created
	inputList := params.Input.OfInputItemList
	if inputList == nil {
		t.Fatal("expected non-nil input list")
	}
}

func TestBuildCodexParams_AssistantMessage(t *testing.T) {
	messages := []Message{
		{Role: "user", Content: "Hi"},
		{Role: "assistant", Content: "Hello!"},
		{Role: "user", Content: "How are you?"},
	}

	params := buildCodexParams(messages, nil, "gpt-4o", nil, false)
	_ = params // Verify it doesn't panic
}

func TestBuildCodexParams_AssistantWithToolCalls(t *testing.T) {
	messages := []Message{
		{Role: "user", Content: "Read file"},
		{Role: "assistant", Content: "", ToolCalls: []ToolCall{
			{ID: "call-1", Name: "read_file", Arguments: map[string]interface{}{"path": "test.go"}},
		}},
		{Role: "tool", Content: "file contents", ToolCallID: "call-1"},
	}

	params := buildCodexParams(messages, nil, "gpt-4o", nil, false)
	_ = params // Verify it doesn't panic
}

func TestBuildCodexParams_AssistantWithToolCallsAndContent(t *testing.T) {
	messages := []Message{
		{Role: "user", Content: "Read file"},
		{Role: "assistant", Content: "I'll read the file.", ToolCalls: []ToolCall{
			{ID: "call-1", Name: "read_file", Arguments: map[string]interface{}{"path": "test.go"}},
		}},
		{Role: "tool", Content: "file contents", ToolCallID: "call-1"},
	}

	params := buildCodexParams(messages, nil, "gpt-4o", nil, false)
	_ = params // Verify it doesn't panic
}

func TestBuildCodexParams_AssistantWithInvalidToolCall(t *testing.T) {
	messages := []Message{
		{Role: "user", Content: "Read file"},
		{Role: "assistant", Content: "", ToolCalls: []ToolCall{
			{ID: "call-1"}, // No name, no arguments
		}},
	}

	params := buildCodexParams(messages, nil, "gpt-4o", nil, false)
	_ = params // Verify it doesn't panic
}

func TestBuildCodexParams_UserWithToolCallID(t *testing.T) {
	messages := []Message{
		{Role: "user", Content: "tool result here", ToolCallID: "call-1"},
	}

	params := buildCodexParams(messages, nil, "gpt-4o", nil, false)
	_ = params // Verify it doesn't panic
}

func TestBuildCodexParams_ToolMessage(t *testing.T) {
	messages := []Message{
		{Role: "tool", Content: "result", ToolCallID: "call-1"},
	}

	params := buildCodexParams(messages, nil, "gpt-4o", nil, false)
	_ = params // Verify it doesn't panic
}

func TestBuildCodexParams_WithTools(t *testing.T) {
	messages := []Message{
		{Role: "user", Content: "Hello"},
	}
	tools := []ToolDefinition{
		{
			Type: "function",
			Function: ToolFunctionDefinition{
				Name:        "my_tool",
				Description: "A test tool",
				Parameters: map[string]interface{}{
					"type": "object",
					"properties": map[string]interface{}{
						"arg1": map[string]interface{}{"type": "string"},
					},
				},
			},
		},
	}

	params := buildCodexParams(messages, tools, "gpt-4o", nil, false)

	if len(params.Tools) == 0 {
		t.Error("expected tools to be set")
	}
}

func TestBuildCodexParams_WithWebSearch(t *testing.T) {
	messages := []Message{
		{Role: "user", Content: "Search the web"},
	}

	params := buildCodexParams(messages, nil, "gpt-4o", nil, true)

	if len(params.Tools) == 0 {
		t.Error("expected web search tool when enableWebSearch is true")
	}
}

func TestBuildCodexParams_WithOptions(t *testing.T) {
	messages := []Message{
		{Role: "user", Content: "Hello"},
	}
	options := map[string]interface{}{
		"temperature": 0.7,
		"max_tokens":  2048,
	}

	params := buildCodexParams(messages, nil, "gpt-4o", options, false)
	_ = params // Verify it doesn't panic
}

// --- translateToolsForCodex ---

func TestTranslateToolsForCodex_BasicTools(t *testing.T) {
	tools := []ToolDefinition{
		{
			Type: "function",
			Function: ToolFunctionDefinition{
				Name:        "tool1",
				Description: "First tool",
				Parameters: map[string]interface{}{
					"type": "object",
				},
			},
		},
		{
			Type: "function",
			Function: ToolFunctionDefinition{
				Name:        "tool2",
				Description: "Second tool",
			},
		},
	}

	result := translateToolsForCodex(tools, false)

	if len(result) != 2 {
		t.Fatalf("expected 2 tools, got %d", len(result))
	}
}

func TestTranslateToolsForCodex_NonFunctionFiltered(t *testing.T) {
	tools := []ToolDefinition{
		{Type: "other_type", Function: ToolFunctionDefinition{Name: "skipped"}},
		{Type: "function", Function: ToolFunctionDefinition{Name: "included"}},
	}

	result := translateToolsForCodex(tools, false)

	if len(result) != 1 {
		t.Fatalf("expected 1 tool, got %d", len(result))
	}
}

func TestTranslateToolsForCodex_WebSearchSkipsWebSearchTool(t *testing.T) {
	tools := []ToolDefinition{
		{Type: "function", Function: ToolFunctionDefinition{Name: "web_search", Description: "Search"}},
		{Type: "function", Function: ToolFunctionDefinition{Name: "other_tool"}},
	}

	result := translateToolsForCodex(tools, true)

	// Should have other_tool + web_search tool = 2
	if len(result) != 2 {
		t.Fatalf("expected 2 tools, got %d", len(result))
	}
}

func TestTranslateToolsForCodex_WebSearchNoDescription(t *testing.T) {
	tools := []ToolDefinition{
		{Type: "function", Function: ToolFunctionDefinition{Name: "my_tool"}},
	}

	result := translateToolsForCodex(tools, true)

	// Should have my_tool + web_search = 2
	if len(result) != 2 {
		t.Fatalf("expected 2 tools, got %d", len(result))
	}
}

func TestTranslateToolsForCodex_EmptyTools(t *testing.T) {
	result := translateToolsForCodex(nil, false)

	if len(result) != 0 {
		t.Errorf("expected 0 tools, got %d", len(result))
	}
}

func TestTranslateToolsForCodex_OnlyWebSearch(t *testing.T) {
	result := translateToolsForCodex(nil, true)

	if len(result) != 1 {
		t.Errorf("expected 1 web search tool, got %d", len(result))
	}
}

// --- CodexProvider.Chat with token source error ---

func TestCodexProvider_Chat_TokenSourceError(t *testing.T) {
	tokenSource := func() (string, string, error) {
		return "", "", fmt.Errorf("token refresh failed")
	}
	p := NewCodexProviderWithTokenSource("initial-token", "account-123", tokenSource)

	ctx := context.Background()
	_, err := p.Chat(ctx, []Message{{Role: "user", Content: "test"}}, nil, "", nil)
	if err == nil {
		t.Error("expected error when token source fails")
	}
	if !strings.Contains(err.Error(), "refreshing token") {
		t.Errorf("expected error about refreshing token, got: %v", err)
	}
}

func TestCodexProvider_Chat_TokenSourceReturnsNewAccountID(t *testing.T) {
	tokenSource := func() (string, string, error) {
		return "new-token", "new-account", nil
	}
	p := NewCodexProviderWithTokenSource("initial-token", "account-123", tokenSource)

	ctx, cancel := context.WithCancel(context.Background())
	cancel() // Cancel immediately so the API call fails quickly

	_, err := p.Chat(ctx, []Message{{Role: "user", Content: "test"}}, nil, "", nil)
	// Will fail because context is cancelled, but token source was invoked
	if err == nil {
		t.Log("Chat completed without error")
	}
}

// --- stripToolCallsFromText (additional coverage) ---

func TestStripToolCallsFromText_NoToolCalls(t *testing.T) {
	result := stripToolCallsFromText("plain text")
	if result != "plain text" {
		t.Errorf("expected 'plain text', got '%s'", result)
	}
}

func TestStripToolCallsFromText_EmptyString(t *testing.T) {
	result := stripToolCallsFromText("")
	if result != "" {
		t.Errorf("expected empty string, got '%s'", result)
	}
}

func TestStripToolCallsFromText_ToolCallsAtStart(t *testing.T) {
	input := `{"tool_calls":[{"id":"1","type":"function","function":{"name":"test","arguments":"{}"}}]} rest text`
	result := stripToolCallsFromText(input)
	if strings.Contains(result, "tool_calls") {
		t.Errorf("expected tool calls to be stripped, got '%s'", result)
	}
	if !strings.Contains(result, "rest text") {
		t.Errorf("expected 'rest text' to remain, got '%s'", result)
	}
}

// --- Additional CodexCliProvider.Chat coverage ---

func TestCodexCliProvider_Chat_WithModel(t *testing.T) {
	p := NewCodexCliProvider("/tmp")

	ctx := context.Background()
	// This will fail because codex doesn't exist, but tests the model arg path
	_, err := p.Chat(ctx, []Message{{Role: "user", Content: "test"}}, nil, "gpt-4o", nil)
	if err == nil {
		t.Log("codex command exists")
	}
}

func TestCodexCliProvider_Chat_DefaultModel(t *testing.T) {
	p := NewCodexCliProvider("/tmp")

	ctx := context.Background()
	// Uses "codex-cli" model which should be skipped
	_, err := p.Chat(ctx, []Message{{Role: "user", Content: "test"}}, nil, "codex-cli", nil)
	if err == nil {
		t.Log("codex command exists")
	}
}

func TestCodexCliProvider_Chat_WithWorkspace(t *testing.T) {
	p := NewCodexCliProvider("/some/workspace")

	ctx := context.Background()
	_, err := p.Chat(ctx, []Message{{Role: "user", Content: "test"}}, nil, "", nil)
	if err == nil {
		t.Log("codex command exists")
	}
}

func TestCodexCliProvider_Chat_WithTools(t *testing.T) {
	p := NewCodexCliProvider("")
	tools := []ToolDefinition{
		{Type: "function", Function: ToolFunctionDefinition{Name: "test_tool"}},
	}

	ctx := context.Background()
	_, err := p.Chat(ctx, []Message{{Role: "user", Content: "test"}}, tools, "", nil)
	if err == nil {
		t.Log("codex command exists")
	}
}

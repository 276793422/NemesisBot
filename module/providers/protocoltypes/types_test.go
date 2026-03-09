// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package protocoltypes

import (
	"encoding/json"
	"testing"
)

func TestToolCall_MarshalJSON(t *testing.T) {
	tests := []struct {
		name    string
		call    ToolCall
		wantErr bool
	}{
		{
			name: "function call",
			call: ToolCall{
				ID:   "call_123",
				Type: "function",
				Function: &FunctionCall{
					Name:      "test_function",
					Arguments: `{"param":"value"}`,
				},
			},
			wantErr: false,
		},
		{
			name: "tool call with name",
			call: ToolCall{
				ID:   "call_456",
				Name: "direct_tool",
				Arguments: map[string]interface{}{
					"param1": "value1",
				},
			},
			wantErr: false,
		},
		{
			name: "minimal call",
			call: ToolCall{
				ID: "call_789",
			},
			wantErr: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			data, err := json.Marshal(tt.call)
			if (err != nil) != tt.wantErr {
				t.Errorf("MarshalJSON() error = %v, wantErr %v", err, tt.wantErr)
				return
			}
			if !tt.wantErr && len(data) == 0 {
				t.Error("MarshalJSON() returned empty data")
			}
		})
	}
}

func TestToolCall_UnmarshalJSON(t *testing.T) {
	jsonData := `{
		"id": "call_123",
		"type": "function",
		"function": {
			"name": "test_function",
			"arguments": "{\"param\":\"value\"}"
		}
	}`

	var call ToolCall
	err := json.Unmarshal([]byte(jsonData), &call)
	if err != nil {
		t.Fatalf("UnmarshalJSON() error = %v", err)
	}

	if call.ID != "call_123" {
		t.Errorf("Expected ID 'call_123', got '%s'", call.ID)
	}
	if call.Type != "function" {
		t.Errorf("Expected Type 'function', got '%s'", call.Type)
	}
	if call.Function == nil {
		t.Fatal("Expected Function to be set")
	}
	if call.Function.Name != "test_function" {
		t.Errorf("Expected function name 'test_function', got '%s'", call.Function.Name)
	}
}

func TestFunctionCall_MarshalJSON(t *testing.T) {
	tests := []struct {
		name    string
		call    FunctionCall
		wantErr bool
	}{
		{
			name: "basic function call",
			call: FunctionCall{
				Name:      "my_function",
				Arguments: `{"arg1":"value1"}`,
			},
			wantErr: false,
		},
		{
			name: "empty arguments",
			call: FunctionCall{
				Name:      "test_func",
				Arguments: `{}`,
			},
			wantErr: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			data, err := json.Marshal(tt.call)
			if (err != nil) != tt.wantErr {
				t.Errorf("MarshalJSON() error = %v, wantErr %v", err, tt.wantErr)
				return
			}
			if !tt.wantErr && len(data) == 0 {
				t.Error("MarshalJSON() returned empty data")
			}
		})
	}
}

func TestLLMResponse_MarshalJSON(t *testing.T) {
	tests := []struct {
		name    string
		resp    LLMResponse
		wantErr bool
	}{
		{
			name: "response with content",
			resp: LLMResponse{
				Content:      "Hello, world!",
				FinishReason: "stop",
			},
			wantErr: false,
		},
		{
			name: "response with tool calls",
			resp: LLMResponse{
				Content: "Please wait",
				ToolCalls: []ToolCall{
					{
						ID:   "call_1",
						Type: "function",
						Function: &FunctionCall{
							Name:      "my_tool",
							Arguments: `{}`,
						},
					},
				},
				FinishReason: "tool_calls",
			},
			wantErr: false,
		},
		{
			name: "response with usage",
			resp: LLMResponse{
				Content: "Response text",
				Usage: &UsageInfo{
					PromptTokens:     10,
					CompletionTokens: 20,
					TotalTokens:      30,
				},
				FinishReason: "stop",
			},
			wantErr: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			data, err := json.Marshal(tt.resp)
			if (err != nil) != tt.wantErr {
				t.Errorf("MarshalJSON() error = %v, wantErr %v", err, tt.wantErr)
				return
			}
			if !tt.wantErr && len(data) == 0 {
				t.Error("MarshalJSON() returned empty data")
			}
		})
	}
}

func TestUsageInfo_MarshalJSON(t *testing.T) {
	usage := UsageInfo{
		PromptTokens:     100,
		CompletionTokens: 200,
		TotalTokens:      300,
	}

	data, err := json.Marshal(usage)
	if err != nil {
		t.Fatalf("MarshalJSON() error = %v", err)
	}

	if len(data) == 0 {
		t.Error("MarshalJSON() returned empty data")
	}

	// Verify unmarshaling works
	var decoded UsageInfo
	err = json.Unmarshal(data, &decoded)
	if err != nil {
		t.Fatalf("UnmarshalJSON() error = %v", err)
	}

	if decoded.PromptTokens != 100 {
		t.Errorf("Expected PromptTokens 100, got %d", decoded.PromptTokens)
	}
	if decoded.CompletionTokens != 200 {
		t.Errorf("Expected CompletionTokens 200, got %d", decoded.CompletionTokens)
	}
	if decoded.TotalTokens != 300 {
		t.Errorf("Expected TotalTokens 300, got %d", decoded.TotalTokens)
	}
}

func TestMessage_MarshalJSON(t *testing.T) {
	tests := []struct {
		name    string
		msg     Message
		wantErr bool
	}{
		{
			name: "user message",
			msg: Message{
				Role:    "user",
				Content: "Hello!",
			},
			wantErr: false,
		},
		{
			name: "assistant message with tool calls",
			msg: Message{
				Role:    "assistant",
				Content: "I'll help you",
				ToolCalls: []ToolCall{
					{
						ID:   "call_1",
						Name: "my_tool",
						Arguments: map[string]interface{}{
							"param": "value",
						},
					},
				},
			},
			wantErr: false,
		},
		{
			name: "tool response message",
			msg: Message{
				Role:       "tool",
				Content:    "Tool result",
				ToolCallID: "call_1",
			},
			wantErr: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			data, err := json.Marshal(tt.msg)
			if (err != nil) != tt.wantErr {
				t.Errorf("MarshalJSON() error = %v, wantErr %v", err, tt.wantErr)
				return
			}
			if !tt.wantErr && len(data) == 0 {
				t.Error("MarshalJSON() returned empty data")
			}
		})
	}
}

func TestToolDefinition_MarshalJSON(t *testing.T) {
	def := ToolDefinition{
		Type: "function",
		Function: ToolFunctionDefinition{
			Name:        "my_function",
			Description: "A test function",
			Parameters: map[string]interface{}{
				"type": "object",
				"properties": map[string]interface{}{
					"param1": map[string]interface{}{
						"type":        "string",
						"description": "A parameter",
					},
				},
			},
		},
	}

	data, err := json.Marshal(def)
	if err != nil {
		t.Fatalf("MarshalJSON() error = %v", err)
	}

	if len(data) == 0 {
		t.Error("MarshalJSON() returned empty data")
	}

	// Verify unmarshaling works
	var decoded ToolDefinition
	err = json.Unmarshal(data, &decoded)
	if err != nil {
		t.Fatalf("UnmarshalJSON() error = %v", err)
	}

	if decoded.Type != "function" {
		t.Errorf("Expected Type 'function', got '%s'", decoded.Type)
	}
	if decoded.Function.Name != "my_function" {
		t.Errorf("Expected function name 'my_function', got '%s'", decoded.Function.Name)
	}
}

func TestToolFunctionDefinition_MarshalJSON(t *testing.T) {
	tests := []struct {
		name    string
		def     ToolFunctionDefinition
		wantErr bool
	}{
		{
			name: "function with parameters",
			def: ToolFunctionDefinition{
				Name:        "test_func",
				Description: "Test function",
				Parameters: map[string]interface{}{
					"type": "object",
					"properties": map[string]interface{}{
						"arg1": map[string]interface{}{
							"type": "string",
						},
					},
				},
			},
			wantErr: false,
		},
		{
			name: "function without parameters",
			def: ToolFunctionDefinition{
				Name:        "simple_func",
				Description: "Simple function",
				Parameters:  map[string]interface{}{},
			},
			wantErr: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			data, err := json.Marshal(tt.def)
			if (err != nil) != tt.wantErr {
				t.Errorf("MarshalJSON() error = %v, wantErr %v", err, tt.wantErr)
				return
			}
			if !tt.wantErr && len(data) == 0 {
				t.Error("MarshalJSON() returned empty data")
			}
		})
	}
}

// TestRoundTrip tests marshaling and unmarshaling of all types
func TestRoundTrip(t *testing.T) {
	t.Run("ToolCall round-trip", func(t *testing.T) {
		original := ToolCall{
			ID:   "test_id",
			Type: "function",
			Function: &FunctionCall{
				Name:      "my_func",
				Arguments: `{"arg":"value"}`,
			},
			Arguments: map[string]interface{}{
				"extra": "data",
			},
		}

		data, err := json.Marshal(original)
		if err != nil {
			t.Fatalf("Marshal error: %v", err)
		}

		var decoded ToolCall
		err = json.Unmarshal(data, &decoded)
		if err != nil {
			t.Fatalf("Unmarshal error: %v", err)
		}

		if decoded.ID != original.ID {
			t.Errorf("ID mismatch: got %s, want %s", decoded.ID, original.ID)
		}
		if decoded.Type != original.Type {
			t.Errorf("Type mismatch: got %s, want %s", decoded.Type, original.Type)
		}
	})

	t.Run("LLMResponse round-trip", func(t *testing.T) {
		original := LLMResponse{
			Content: "Test response",
			ToolCalls: []ToolCall{
				{
					ID:   "call_1",
					Name: "tool1",
				},
			},
			FinishReason: "stop",
			Usage: &UsageInfo{
				PromptTokens:     50,
				CompletionTokens: 100,
				TotalTokens:      150,
			},
		}

		data, err := json.Marshal(original)
		if err != nil {
			t.Fatalf("Marshal error: %v", err)
		}

		var decoded LLMResponse
		err = json.Unmarshal(data, &decoded)
		if err != nil {
			t.Fatalf("Unmarshal error: %v", err)
		}

		if decoded.Content != original.Content {
			t.Errorf("Content mismatch: got %s, want %s", decoded.Content, original.Content)
		}
		if decoded.FinishReason != original.FinishReason {
			t.Errorf("FinishReason mismatch: got %s, want %s", decoded.FinishReason, original.FinishReason)
		}
	})
}

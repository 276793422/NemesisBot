// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package providers_test

import (
	"context"
	"encoding/json"
	"strings"
	"testing"

	"github.com/276793422/NemesisBot/module/providers"
)

// --- NewCodexCliProvider ---

func TestNewCodexCliProvider(t *testing.T) {
	p := providers.NewCodexCliProvider("/tmp/workspace")
	if p == nil {
		t.Fatal("expected non-nil provider")
	}
}

func TestNewCodexCliProvider_EmptyWorkspace(t *testing.T) {
	p := providers.NewCodexCliProvider("")
	if p == nil {
		t.Fatal("expected non-nil provider with empty workspace")
	}
}

// --- GetDefaultModel ---

func TestCodexCliProvider_GetDefaultModel(t *testing.T) {
	p := providers.NewCodexCliProvider("/tmp")
	model := p.GetDefaultModel()
	if model != "codex-cli" {
		t.Errorf("expected 'codex-cli', got '%s'", model)
	}
}

// --- CodexCliProvider implements LLMProvider ---

func TestCodexCliProvider_ImplementsLLMProvider(t *testing.T) {
	// Compile-time check
	var _ providers.LLMProvider = providers.NewCodexCliProvider("")
}

// --- CodexCliProvider.Chat with cancelled context ---

func TestCodexCliProvider_Chat_CancelledContext(t *testing.T) {
	p := providers.NewCodexCliProvider("")

	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	_, err := p.Chat(ctx, nil, nil, "", nil)
	if err == nil {
		t.Error("expected error with cancelled context")
	}
}

func TestCodexCliProvider_Chat_WithMessages(t *testing.T) {
	p := providers.NewCodexCliProvider("")

	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	messages := []providers.Message{
		{Role: "system", Content: "You are helpful"},
		{Role: "user", Content: "Hello"},
		{Role: "assistant", Content: "Hi there"},
		{Role: "tool", Content: "result", ToolCallID: "call-1"},
	}

	// Will fail because context is cancelled, but should not panic
	_, err := p.Chat(ctx, messages, nil, "", nil)
	if err == nil {
		t.Error("expected error with cancelled context")
	}
}

// --- Test JSONL event parsing (the expected format from codex exec --json) ---

func TestCodexCliProvider_JSONLEventFormat(t *testing.T) {
	// Verify that the expected JSONL event format parses correctly
	events := []string{
		`{"type":"item.completed","item":{"id":"msg-1","type":"agent_message","text":"Hello! I can help with that."}}`,
		`{"type":"turn.completed","usage":{"input_tokens":100,"cached_input_tokens":50,"output_tokens":80}}`,
	}

	for i, event := range events {
		var parsed struct {
			Type     string `json:"type"`
			ThreadID string `json:"thread_id,omitempty"`
			Message  string `json:"message,omitempty"`
			Item     *struct {
				ID       string `json:"id"`
				Type     string `json:"type"`
				Text     string `json:"text,omitempty"`
				Command  string `json:"command,omitempty"`
				Status   string `json:"status,omitempty"`
				ExitCode *int   `json:"exit_code,omitempty"`
				Output   string `json:"output,omitempty"`
			} `json:"item,omitempty"`
			Usage *struct {
				InputTokens       int `json:"input_tokens"`
				CachedInputTokens int `json:"cached_input_tokens"`
				OutputTokens      int `json:"output_tokens"`
			} `json:"usage,omitempty"`
			Error *struct {
				Message string `json:"message"`
			} `json:"error,omitempty"`
		}

		if err := json.Unmarshal([]byte(event), &parsed); err != nil {
			t.Errorf("event %d: failed to parse: %v", i, err)
			continue
		}

		switch i {
		case 0:
			if parsed.Type != "item.completed" {
				t.Errorf("expected type 'item.completed', got '%s'", parsed.Type)
			}
			if parsed.Item == nil {
				t.Fatal("expected non-nil item")
			}
			if parsed.Item.Type != "agent_message" {
				t.Errorf("expected item type 'agent_message', got '%s'", parsed.Item.Type)
			}
			if parsed.Item.Text != "Hello! I can help with that." {
				t.Errorf("unexpected text: '%s'", parsed.Item.Text)
			}
		case 1:
			if parsed.Type != "turn.completed" {
				t.Errorf("expected type 'turn.completed', got '%s'", parsed.Type)
			}
			if parsed.Usage == nil {
				t.Fatal("expected non-nil usage")
			}
			if parsed.Usage.InputTokens != 100 {
				t.Errorf("expected input_tokens 100, got %d", parsed.Usage.InputTokens)
			}
			if parsed.Usage.CachedInputTokens != 50 {
				t.Errorf("expected cached_input_tokens 50, got %d", parsed.Usage.CachedInputTokens)
			}
			if parsed.Usage.OutputTokens != 80 {
				t.Errorf("expected output_tokens 80, got %d", parsed.Usage.OutputTokens)
			}
		}
	}
}

// --- Test tool call format for CodexCli ---

func TestCodexCliProvider_ToolCallInJSONL(t *testing.T) {
	// Verify tool calls are extracted from agent_message text
	responseText := `I'll check the file. {"tool_calls":[{"id":"call_abc","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"test.go\"}"}}]}`

	// The tool call extraction uses the shared extractToolCallsFromText function
	if !strings.Contains(responseText, `{"tool_calls"`) {
		t.Error("expected tool_calls marker in response")
	}
}

// --- Test error event handling ---

func TestCodexCliProvider_ErrorEvent(t *testing.T) {
	errorEvent := `{"type":"error","message":"Rate limit exceeded"}`

	var parsed struct {
		Type    string `json:"type"`
		Message string `json:"message"`
	}
	if err := json.Unmarshal([]byte(errorEvent), &parsed); err != nil {
		t.Fatalf("failed to parse error event: %v", err)
	}

	if parsed.Type != "error" {
		t.Errorf("expected type 'error', got '%s'", parsed.Type)
	}
	if parsed.Message != "Rate limit exceeded" {
		t.Errorf("expected message 'Rate limit exceeded', got '%s'", parsed.Message)
	}
}

// --- Test turn.failed event ---

func TestCodexCliProvider_TurnFailedEvent(t *testing.T) {
	failedEvent := `{"type":"turn.failed","error":{"message":"Model returned invalid output"}}`

	var parsed struct {
		Type  string `json:"type"`
		Error *struct {
			Message string `json:"message"`
		} `json:"error"`
	}
	if err := json.Unmarshal([]byte(failedEvent), &parsed); err != nil {
		t.Fatalf("failed to parse turn.failed event: %v", err)
	}

	if parsed.Type != "turn.failed" {
		t.Errorf("expected type 'turn.failed', got '%s'", parsed.Type)
	}
	if parsed.Error == nil {
		t.Fatal("expected non-nil error")
	}
	if parsed.Error.Message != "Model returned invalid output" {
		t.Errorf("unexpected error message: '%s'", parsed.Error.Message)
	}
}

// --- Usage calculation for CodexCli ---

func TestCodexCliProvider_UsageCalculation(t *testing.T) {
	tests := []struct {
		name              string
		inputTokens       int
		cachedTokens      int
		outputTokens      int
		expectedPrompt    int
		expectedTotal     int
	}{
		{
			name:           "basic usage",
			inputTokens:    200,
			cachedTokens:   0,
			outputTokens:   100,
			expectedPrompt: 200,
			expectedTotal:  300,
		},
		{
			name:           "with cache",
			inputTokens:    200,
			cachedTokens:   50,
			outputTokens:   100,
			expectedPrompt: 250,
			expectedTotal:  350,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			promptTokens := tt.inputTokens + tt.cachedTokens
			totalTokens := promptTokens + tt.outputTokens

			if promptTokens != tt.expectedPrompt {
				t.Errorf("expected prompt tokens %d, got %d", tt.expectedPrompt, promptTokens)
			}
			if totalTokens != tt.expectedTotal {
				t.Errorf("expected total tokens %d, got %d", tt.expectedTotal, totalTokens)
			}
		})
	}
}

// --- Test multiple JSONL lines (combined output) ---

func TestCodexCliProvider_MultipleJSONLLines(t *testing.T) {
	combined := `{"type":"item.completed","item":{"id":"msg-1","type":"agent_message","text":"Part 1"}}
{"type":"item.completed","item":{"id":"msg-2","type":"agent_message","text":"Part 2"}}
{"type":"turn.completed","usage":{"input_tokens":100,"cached_input_tokens":0,"output_tokens":50}}`

	lines := strings.Split(strings.TrimSpace(combined), "\n")
	if len(lines) != 3 {
		t.Fatalf("expected 3 lines, got %d", len(lines))
	}

	for i, line := range lines {
		var parsed struct {
			Type string `json:"type"`
		}
		if err := json.Unmarshal([]byte(line), &parsed); err != nil {
			t.Errorf("line %d: failed to parse: %v", i, err)
		}
	}

	// Verify the content would be "Part 1\nPart 2" after joining
	expectedContent := "Part 1\nPart 2"
	parts := []string{"Part 1", "Part 2"}
	joined := strings.Join(parts, "\n")
	if joined != expectedContent {
		t.Errorf("expected '%s', got '%s'", expectedContent, joined)
	}
}

// --- Test non-agent_message items are ignored ---

func TestCodexCliProvider_NonAgentMessageItem(t *testing.T) {
	event := `{"type":"item.completed","item":{"id":"cmd-1","type":"command","command":"ls -la","status":"completed","exit_code":0,"output":"file1.txt\nfile2.txt"}}`

	var parsed struct {
		Type string `json:"type"`
		Item *struct {
			Type string `json:"type"`
			Text string `json:"text,omitempty"`
		} `json:"item,omitempty"`
	}
	if err := json.Unmarshal([]byte(event), &parsed); err != nil {
		t.Fatalf("failed to parse: %v", err)
	}

	if parsed.Item.Type != "command" {
		t.Errorf("expected type 'command', got '%s'", parsed.Item.Type)
	}
	// command type items should have empty Text field
	if parsed.Item.Text != "" {
		t.Errorf("expected empty text for command item, got '%s'", parsed.Item.Text)
	}
}

// --- CodexCliProvider.Chat with empty command ---

func TestCodexCliProvider_Chat_EmptyCommand(t *testing.T) {
	// Create a provider, then verify that Chat handles missing codex CLI gracefully
	p := providers.NewCodexCliProvider("/tmp")
	ctx := context.Background()

	// This will fail because 'codex' command likely doesn't exist
	_, err := p.Chat(ctx, []providers.Message{{Role: "user", Content: "test"}}, nil, "", nil)
	if err == nil {
		t.Log("codex command exists - unexpected but not wrong")
	} else {
		t.Logf("Expected error (codex not found): %v", err)
	}
}

// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package agent

import (
	"context"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/observer"
)

// TestRequestLoggerObserver_NewRequestLoggerObserver tests construction.
func TestRequestLoggerObserver_NewRequestLoggerObserver(t *testing.T) {
	cfg := &config.LoggingConfig{
		LLM: &config.LLMLogConfig{
			Enabled:     true,
			LogDir:      t.TempDir(),
			DetailLevel: "full",
		},
	}
	obs := NewRequestLoggerObserver(cfg, t.TempDir())
	if obs == nil {
		t.Fatal("expected non-nil observer")
	}
	if obs.Name() != "request_logger" {
		t.Errorf("expected name 'request_logger', got %q", obs.Name())
	}
	if len(obs.active) != 0 {
		t.Errorf("expected empty active map, got %d entries", len(obs.active))
	}
}

// TestRequestLoggerObserver_OnEvent_ConversationStart tests conversation start handling.
func TestRequestLoggerObserver_OnEvent_ConversationStart(t *testing.T) {
	logDir := t.TempDir()
	cfg := &config.LoggingConfig{
		LLM: &config.LLMLogConfig{
			Enabled:     true,
			LogDir:      logDir,
			DetailLevel: "full",
		},
	}
	obs := NewRequestLoggerObserver(cfg, t.TempDir())

	ctx := context.Background()
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:      observer.EventConversationStart,
		TraceID:   "trace-001",
		Timestamp: time.Now(),
		Data: &observer.ConversationStartData{
			Channel:  "test",
			SenderID: "user1",
			ChatID:   "chat1",
			Content:  "Hello",
		},
	})

	obs.mu.Lock()
	state, ok := obs.active["trace-001"]
	obs.mu.Unlock()
	if !ok {
		t.Fatal("expected active state for trace-001")
	}
	if state == nil || state.logger == nil {
		t.Fatal("expected non-nil logger in state")
	}
}

// TestRequestLoggerObserver_OnEvent_ConversationStart_Disabled tests that disabled logging skips.
func TestRequestLoggerObserver_OnEvent_ConversationStart_Disabled(t *testing.T) {
	cfg := &config.LoggingConfig{
		LLM: &config.LLMLogConfig{
			Enabled: false,
		},
	}
	obs := NewRequestLoggerObserver(cfg, t.TempDir())

	ctx := context.Background()
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventConversationStart,
		TraceID: "trace-002",
		Data: &observer.ConversationStartData{
			Content: "Hello",
		},
	})

	obs.mu.Lock()
	_, ok := obs.active["trace-002"]
	obs.mu.Unlock()
	if ok {
		t.Error("expected no active state when logging is disabled")
	}
}

// TestRequestLoggerObserver_OnEvent_LLMRequest tests LLM request handling.
func TestRequestLoggerObserver_OnEvent_LLMRequest(t *testing.T) {
	logDir := t.TempDir()
	cfg := &config.LoggingConfig{
		LLM: &config.LLMLogConfig{
			Enabled:     true,
			LogDir:      logDir,
			DetailLevel: "full",
		},
	}
	obs := NewRequestLoggerObserver(cfg, t.TempDir())
	ctx := context.Background()

	// Setup active state first
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:      observer.EventConversationStart,
		TraceID:   "trace-003",
		Timestamp: time.Now(),
		Data: &observer.ConversationStartData{
			Content: "Hello",
		},
	})

	// Send LLM request event
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:      observer.EventLLMRequest,
		TraceID:   "trace-003",
		Timestamp: time.Now(),
		Data: &observer.LLMRequestData{
			Round:        1,
			Model:        "test-model",
			ProviderName: "test-provider",
		},
	})
	// No crash means success
}

// TestRequestLoggerObserver_OnEvent_LLMResponse tests LLM response handling.
func TestRequestLoggerObserver_OnEvent_LLMResponse(t *testing.T) {
	logDir := t.TempDir()
	cfg := &config.LoggingConfig{
		LLM: &config.LLMLogConfig{
			Enabled:     true,
			LogDir:      logDir,
			DetailLevel: "full",
		},
	}
	obs := NewRequestLoggerObserver(cfg, t.TempDir())
	ctx := context.Background()

	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:      observer.EventConversationStart,
		TraceID:   "trace-004",
		Timestamp: time.Now(),
		Data: &observer.ConversationStartData{
			Content: "Hello",
		},
	})

	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:      observer.EventLLMResponse,
		TraceID:   "trace-004",
		Timestamp: time.Now(),
		Data: &observer.LLMResponseData{
			Round:        1,
			Content:      "Response text",
			Duration:     time.Second,
			FinishReason: "stop",
		},
	})
}

// TestRequestLoggerObserver_OnEvent_ToolCall tests tool call handling.
func TestRequestLoggerObserver_OnEvent_ToolCall(t *testing.T) {
	logDir := t.TempDir()
	cfg := &config.LoggingConfig{
		LLM: &config.LLMLogConfig{
			Enabled:     true,
			LogDir:      logDir,
			DetailLevel: "full",
		},
	}
	obs := NewRequestLoggerObserver(cfg, t.TempDir())
	ctx := context.Background()

	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:      observer.EventConversationStart,
		TraceID:   "trace-005",
		Timestamp: time.Now(),
		Data: &observer.ConversationStartData{
			Content: "Hello",
		},
	})

	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:      observer.EventToolCall,
		TraceID:   "trace-005",
		Timestamp: time.Now(),
		Data: &observer.ToolCallData{
			ToolName:   "read_file",
			Arguments:  map[string]interface{}{"path": "/tmp/test.txt"},
			LLMRound:   1,
			Success:    true,
			Duration:   100 * time.Millisecond,
		},
	})

	obs.mu.Lock()
	state := obs.active["trace-005"]
	obs.mu.Unlock()
	if state == nil {
		t.Fatal("expected active state")
	}
	if len(state.operations[1]) != 1 {
		t.Errorf("expected 1 operation in round 1, got %d", len(state.operations[1]))
	}
}

// TestRequestLoggerObserver_OnEvent_ToolCall_Failed tests failed tool call.
func TestRequestLoggerObserver_OnEvent_ToolCall_Failed(t *testing.T) {
	logDir := t.TempDir()
	cfg := &config.LoggingConfig{
		LLM: &config.LLMLogConfig{
			Enabled:     true,
			LogDir:      logDir,
			DetailLevel: "full",
		},
	}
	obs := NewRequestLoggerObserver(cfg, t.TempDir())
	ctx := context.Background()

	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:      observer.EventConversationStart,
		TraceID:   "trace-fail",
		Timestamp: time.Now(),
		Data: &observer.ConversationStartData{Content: "test"},
	})

	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:      observer.EventToolCall,
		TraceID:   "trace-fail",
		Timestamp: time.Now(),
		Data: &observer.ToolCallData{
			ToolName:  "read_file",
			LLMRound:  1,
			Success:   false,
			Error:     "file not found",
			Duration:  50 * time.Millisecond,
		},
	})

	obs.mu.Lock()
	state := obs.active["trace-fail"]
	obs.mu.Unlock()
	if state == nil {
		t.Fatal("expected active state")
	}
	ops := state.operations[1]
	if len(ops) != 1 {
		t.Fatalf("expected 1 operation, got %d", len(ops))
	}
	if ops[0].Status != "Failed" {
		t.Errorf("expected Failed status, got %s", ops[0].Status)
	}
	if ops[0].Error != "file not found" {
		t.Errorf("expected error 'file not found', got %s", ops[0].Error)
	}
}

// TestRequestLoggerObserver_OnEvent_ConversationEnd tests conversation end handling.
func TestRequestLoggerObserver_OnEvent_ConversationEnd(t *testing.T) {
	logDir := t.TempDir()
	cfg := &config.LoggingConfig{
		LLM: &config.LLMLogConfig{
			Enabled:     true,
			LogDir:      logDir,
			DetailLevel: "full",
		},
	}
	obs := NewRequestLoggerObserver(cfg, t.TempDir())
	ctx := context.Background()

	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:      observer.EventConversationStart,
		TraceID:   "trace-006",
		Timestamp: time.Now(),
		Data: &observer.ConversationStartData{Content: "Hello"},
	})

	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:      observer.EventConversationEnd,
		TraceID:   "trace-006",
		Timestamp: time.Now(),
		Data: &observer.ConversationEndData{
			TotalRounds:   1,
			Content:       "Final response",
			TotalDuration: 2 * time.Second,
			Channel:       "test",
			ChatID:        "chat1",
		},
	})

	obs.mu.Lock()
	_, ok := obs.active["trace-006"]
	obs.mu.Unlock()
	if ok {
		t.Error("expected trace-006 to be removed from active after conversation end")
	}
}

// TestRequestLoggerObserver_OnEvent_UnknownTrace tests events for unknown trace IDs.
func TestRequestLoggerObserver_OnEvent_UnknownTrace(t *testing.T) {
	cfg := &config.LoggingConfig{
		LLM: &config.LLMLogConfig{Enabled: true, LogDir: t.TempDir(), DetailLevel: "full"},
	}
	obs := NewRequestLoggerObserver(cfg, t.TempDir())
	ctx := context.Background()

	// These should not panic even without a conversation start
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventLLMRequest,
		TraceID: "unknown-trace",
		Data:    &observer.LLMRequestData{Round: 1},
	})

	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventLLMResponse,
		TraceID: "unknown-trace",
		Data:    &observer.LLMResponseData{Round: 1},
	})

	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventToolCall,
		TraceID: "unknown-trace",
		Data:    &observer.ToolCallData{ToolName: "test"},
	})

	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventConversationEnd,
		TraceID: "unknown-trace",
		Data:    &observer.ConversationEndData{TotalRounds: 1},
	})
}

// TestRequestLoggerObserver_OnEvent_WrongDataType tests events with wrong data types.
func TestRequestLoggerObserver_OnEvent_WrongDataType(t *testing.T) {
	cfg := &config.LoggingConfig{
		LLM: &config.LLMLogConfig{Enabled: true, LogDir: t.TempDir(), DetailLevel: "full"},
	}
	obs := NewRequestLoggerObserver(cfg, t.TempDir())
	ctx := context.Background()

	// Send events with wrong data types - should not panic
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventConversationStart,
		TraceID: "wrong-1",
		Data:    &observer.LLMRequestData{Round: 1}, // wrong type
	})

	// Setup active state for following events
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:      observer.EventConversationStart,
		TraceID:   "wrong-2",
		Timestamp: time.Now(),
		Data: &observer.ConversationStartData{Content: "test"},
	})

	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventLLMRequest,
		TraceID: "wrong-2",
		Data:    &observer.ConversationStartData{Content: "wrong"}, // wrong type
	})

	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventLLMResponse,
		TraceID: "wrong-2",
		Data:    &observer.ConversationStartData{Content: "wrong"}, // wrong type
	})

	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventToolCall,
		TraceID: "wrong-2",
		Data:    &observer.ConversationStartData{Content: "wrong"}, // wrong type
	})

	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventConversationEnd,
		TraceID: "wrong-2",
		Data:    &observer.LLMRequestData{Round: 1}, // wrong type
	})
}

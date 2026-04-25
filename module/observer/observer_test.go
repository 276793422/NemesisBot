package observer_test

import (
	"context"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/observer"
	"github.com/276793422/NemesisBot/module/providers"
)

// ---------------------------------------------------------------------------
// mockObserver implements observer.Observer for testing
// ---------------------------------------------------------------------------

type mockObserver struct {
	name     string
	events   []observer.ConversationEvent
	mu       sync.Mutex
	onEvent  func(ctx context.Context, event observer.ConversationEvent) // optional hook
}

func (m *mockObserver) Name() string { return m.name }

func (m *mockObserver) OnEvent(ctx context.Context, event observer.ConversationEvent) {
	m.mu.Lock()
	m.events = append(m.events, event)
	m.mu.Unlock()
	if m.onEvent != nil {
		m.onEvent(ctx, event)
	}
}

func (m *mockObserver) Events() []observer.ConversationEvent {
	m.mu.Lock()
	defer m.mu.Unlock()
	result := make([]observer.ConversationEvent, len(m.events))
	copy(result, m.events)
	return result
}

// ---------------------------------------------------------------------------
// EventType constants
// ---------------------------------------------------------------------------

func TestEventTypeConstants(t *testing.T) {
	tests := []struct {
		eventType observer.EventType
		expected  string
	}{
		{observer.EventConversationStart, "conversation_start"},
		{observer.EventConversationEnd, "conversation_end"},
		{observer.EventLLMRequest, "llm_request"},
		{observer.EventLLMResponse, "llm_response"},
		{observer.EventToolCall, "tool_call"},
	}
	for _, tt := range tests {
		if string(tt.eventType) != tt.expected {
			t.Errorf("EventType %q = %q, want %q", tt.eventType, string(tt.eventType), tt.expected)
		}
	}
}

// ---------------------------------------------------------------------------
// ConversationEvent and data types
// ---------------------------------------------------------------------------

func TestConversationEvent_Start(t *testing.T) {
	now := time.Now()
	event := observer.ConversationEvent{
		Type:      observer.EventConversationStart,
		TraceID:   "trace-001",
		Timestamp: now,
		Data: &observer.ConversationStartData{
			SessionKey: "session-1",
			Channel:    "web",
			ChatID:     "chat-123",
			SenderID:   "user-456",
			Content:    "Hello",
		},
	}

	if event.Type != observer.EventConversationStart {
		t.Error("Type mismatch")
	}
	if event.TraceID != "trace-001" {
		t.Error("TraceID mismatch")
	}
	if event.Timestamp != now {
		t.Error("Timestamp mismatch")
	}

	data, ok := event.Data.(*observer.ConversationStartData)
	if !ok {
		t.Fatal("Data should be ConversationStartData")
	}
	if data.SessionKey != "session-1" {
		t.Errorf("SessionKey = %q, want %q", data.SessionKey, "session-1")
	}
	if data.Channel != "web" {
		t.Errorf("Channel = %q, want %q", data.Channel, "web")
	}
}

func TestConversationEvent_End(t *testing.T) {
	event := observer.ConversationEvent{
		Type:    observer.EventConversationEnd,
		TraceID: "trace-002",
		Data: &observer.ConversationEndData{
			SessionKey:    "session-1",
			TotalRounds:   3,
			TotalDuration: 5 * time.Second,
		},
	}

	data, ok := event.Data.(*observer.ConversationEndData)
	if !ok {
		t.Fatal("Data should be ConversationEndData")
	}
	if data.TotalRounds != 3 {
		t.Errorf("TotalRounds = %d, want 3", data.TotalRounds)
	}
	if data.TotalDuration != 5*time.Second {
		t.Errorf("TotalDuration = %v, want 5s", data.TotalDuration)
	}
}

func TestConversationEvent_LLMRequest(t *testing.T) {
	event := observer.ConversationEvent{
		Type:    observer.EventLLMRequest,
		TraceID: "trace-003",
		Data: &observer.LLMRequestData{
			Round:        1,
			Model:        "gpt-4",
			ProviderName: "openai",
			Messages: []providers.Message{
				{Role: "user", Content: "Hello"},
			},
		},
	}

	data, ok := event.Data.(*observer.LLMRequestData)
	if !ok {
		t.Fatal("Data should be LLMRequestData")
	}
	if data.Model != "gpt-4" {
		t.Errorf("Model = %q, want %q", data.Model, "gpt-4")
	}
	if len(data.Messages) != 1 {
		t.Errorf("Messages count = %d, want 1", len(data.Messages))
	}
}

func TestConversationEvent_LLMResponse(t *testing.T) {
	event := observer.ConversationEvent{
		Type:    observer.EventLLMResponse,
		TraceID: "trace-004",
		Data: &observer.LLMResponseData{
			Round:        1,
			Duration:     2 * time.Second,
			Content:      "Hi there!",
			FinishReason: "stop",
		},
	}

	data, ok := event.Data.(*observer.LLMResponseData)
	if !ok {
		t.Fatal("Data should be LLMResponseData")
	}
	if data.Content != "Hi there!" {
		t.Errorf("Content = %q, want %q", data.Content, "Hi there!")
	}
	if data.Duration != 2*time.Second {
		t.Errorf("Duration = %v, want 2s", data.Duration)
	}
}

func TestConversationEvent_ToolCall(t *testing.T) {
	event := observer.ConversationEvent{
		Type:    observer.EventToolCall,
		TraceID: "trace-005",
		Data: &observer.ToolCallData{
			ToolName:  "file_read",
			Arguments: map[string]interface{}{"path": "/tmp/test.txt"},
			Success:   true,
			Duration:  100 * time.Millisecond,
			LLMRound:  1,
			ChainPos:  0,
		},
	}

	data, ok := event.Data.(*observer.ToolCallData)
	if !ok {
		t.Fatal("Data should be ToolCallData")
	}
	if data.ToolName != "file_read" {
		t.Errorf("ToolName = %q, want %q", data.ToolName, "file_read")
	}
	if !data.Success {
		t.Error("Success should be true")
	}
}

func TestConversationEvent_ToolCallWithError(t *testing.T) {
	event := observer.ConversationEvent{
		Type: observer.EventToolCall,
		Data: &observer.ToolCallData{
			ToolName: "bad_tool",
			Success:  false,
			Error:    "file not found",
		},
	}

	data, ok := event.Data.(*observer.ToolCallData)
	if !ok {
		t.Fatal("Data should be ToolCallData")
	}
	if data.Success {
		t.Error("Success should be false")
	}
	if data.Error != "file not found" {
		t.Errorf("Error = %q, want %q", data.Error, "file not found")
	}
}

// ---------------------------------------------------------------------------
// Observer interface compliance
// ---------------------------------------------------------------------------

func TestMockObserverImplementsInterface(t *testing.T) {
	var _ observer.Observer = &mockObserver{}
}

// ---------------------------------------------------------------------------
// NewManager
// ---------------------------------------------------------------------------

func TestNewManager(t *testing.T) {
	mgr := observer.NewManager()
	if mgr == nil {
		t.Fatal("NewManager should return non-nil")
	}
	if mgr.HasObservers() {
		t.Error("New manager should have no observers")
	}
}

// ---------------------------------------------------------------------------
// Manager.Register and Unregister
// ---------------------------------------------------------------------------

func TestManager_Register(t *testing.T) {
	mgr := observer.NewManager()
	obs := &mockObserver{name: "test-observer"}

	mgr.Register(obs)

	if !mgr.HasObservers() {
		t.Error("Manager should have observers after Register")
	}
}

func TestManager_RegisterMultiple(t *testing.T) {
	mgr := observer.NewManager()
	mgr.Register(&mockObserver{name: "obs1"})
	mgr.Register(&mockObserver{name: "obs2"})
	mgr.Register(&mockObserver{name: "obs3"})

	if !mgr.HasObservers() {
		t.Error("Manager should have observers")
	}
}

func TestManager_Unregister(t *testing.T) {
	mgr := observer.NewManager()
	mgr.Register(&mockObserver{name: "to-remove"})
	mgr.Register(&mockObserver{name: "to-keep"})

	mgr.Unregister("to-remove")

	if !mgr.HasObservers() {
		t.Error("Manager should still have observers after removing one")
	}
}

func TestManager_UnregisterNonExistent(t *testing.T) {
	mgr := observer.NewManager()
	mgr.Register(&mockObserver{name: "only-one"})

	// Removing non-existent should not panic or error
	mgr.Unregister("nonexistent")
	if !mgr.HasObservers() {
		t.Error("Manager should still have the registered observer")
	}
}

func TestManager_UnregisterAll(t *testing.T) {
	mgr := observer.NewManager()
	mgr.Register(&mockObserver{name: "a"})
	mgr.Register(&mockObserver{name: "b"})

	mgr.Unregister("a")
	mgr.Unregister("b")

	if mgr.HasObservers() {
		t.Error("Manager should have no observers after removing all")
	}
}

// ---------------------------------------------------------------------------
// Manager.Emit (async)
// ---------------------------------------------------------------------------

func TestManager_Emit(t *testing.T) {
	mgr := observer.NewManager()
	obs := &mockObserver{name: "async-obs"}
	mgr.Register(obs)

	event := observer.ConversationEvent{
		Type:    observer.EventConversationStart,
		TraceID: "trace-emit",
		Data:    &observer.ConversationStartData{SessionKey: "s1"},
	}

	mgr.Emit(context.Background(), event)

	// Wait for async delivery
	time.Sleep(100 * time.Millisecond)

	events := obs.Events()
	if len(events) != 1 {
		t.Fatalf("Expected 1 event, got %d", len(events))
	}
	if events[0].TraceID != "trace-emit" {
		t.Errorf("TraceID = %q, want %q", events[0].TraceID, "trace-emit")
	}
}

func TestManager_Emit_MultipleObservers(t *testing.T) {
	mgr := observer.NewManager()
	obs1 := &mockObserver{name: "obs1"}
	obs2 := &mockObserver{name: "obs2"}
	obs3 := &mockObserver{name: "obs3"}
	mgr.Register(obs1)
	mgr.Register(obs2)
	mgr.Register(obs3)

	event := observer.ConversationEvent{
		Type:    observer.EventToolCall,
		TraceID: "multi-emit",
	}
	mgr.Emit(context.Background(), event)

	time.Sleep(100 * time.Millisecond)

	if len(obs1.Events()) != 1 {
		t.Errorf("obs1: expected 1 event, got %d", len(obs1.Events()))
	}
	if len(obs2.Events()) != 1 {
		t.Errorf("obs2: expected 1 event, got %d", len(obs2.Events()))
	}
	if len(obs3.Events()) != 1 {
		t.Errorf("obs3: expected 1 event, got %d", len(obs3.Events()))
	}
}

func TestManager_Emit_NoObservers(t *testing.T) {
	mgr := observer.NewManager()

	// Should not panic
	event := observer.ConversationEvent{
		Type: observer.EventConversationEnd,
	}
	mgr.Emit(context.Background(), event)
}

// ---------------------------------------------------------------------------
// Manager.EmitSync (sync)
// ---------------------------------------------------------------------------

func TestManager_EmitSync(t *testing.T) {
	mgr := observer.NewManager()
	obs := &mockObserver{name: "sync-obs"}
	mgr.Register(obs)

	event := observer.ConversationEvent{
		Type:    observer.EventConversationEnd,
		TraceID: "sync-trace",
	}

	mgr.EmitSync(context.Background(), event)

	// Sync should be immediate, no sleep needed
	events := obs.Events()
	if len(events) != 1 {
		t.Fatalf("Expected 1 event, got %d", len(events))
	}
	if events[0].TraceID != "sync-trace" {
		t.Errorf("TraceID = %q, want %q", events[0].TraceID, "sync-trace")
	}
}

func TestManager_EmitSync_MultipleObservers(t *testing.T) {
	mgr := observer.NewManager()
	obs1 := &mockObserver{name: "sync1"}
	obs2 := &mockObserver{name: "sync2"}
	mgr.Register(obs1)
	mgr.Register(obs2)

	event := observer.ConversationEvent{
		Type:    observer.EventLLMResponse,
		TraceID: "sync-multi",
	}

	mgr.EmitSync(context.Background(), event)

	if len(obs1.Events()) != 1 {
		t.Errorf("obs1: expected 1, got %d", len(obs1.Events()))
	}
	if len(obs2.Events()) != 1 {
		t.Errorf("obs2: expected 1, got %d", len(obs2.Events()))
	}
}

func TestManager_EmitSync_NoObservers(t *testing.T) {
	mgr := observer.NewManager()

	// Should not panic
	event := observer.ConversationEvent{Type: observer.EventConversationStart}
	mgr.EmitSync(context.Background(), event)
}

// ---------------------------------------------------------------------------
// Manager error handling — observer panic recovery
// ---------------------------------------------------------------------------

func TestManager_Emit_ObserverPanic(t *testing.T) {
	mgr := observer.NewManager()

	panicObs := &mockObserver{
		name: "panic-obs",
		onEvent: func(ctx context.Context, event observer.ConversationEvent) {
			panic("deliberate panic")
		},
	}
	normalObs := &mockObserver{name: "normal-obs"}
	mgr.Register(panicObs)
	mgr.Register(normalObs)

	// Emit should not propagate the panic
	event := observer.ConversationEvent{Type: observer.EventToolCall}
	mgr.Emit(context.Background(), event)

	time.Sleep(100 * time.Millisecond)

	// Normal observer should still receive the event
	if len(normalObs.Events()) != 1 {
		t.Errorf("Normal observer should still receive event, got %d", len(normalObs.Events()))
	}
}

func TestManager_EmitSync_ObserverPanic(t *testing.T) {
	mgr := observer.NewManager()

	panicObs := &mockObserver{
		name: "sync-panic-obs",
		onEvent: func(ctx context.Context, event observer.ConversationEvent) {
			panic("deliberate sync panic")
		},
	}
	normalObs := &mockObserver{name: "sync-normal-obs"}
	mgr.Register(panicObs)
	mgr.Register(normalObs)

	// EmitSync should not propagate the panic
	event := observer.ConversationEvent{Type: observer.EventToolCall}
	mgr.EmitSync(context.Background(), event)

	// Normal observer should still receive the event
	if len(normalObs.Events()) != 1 {
		t.Errorf("Normal observer should still receive event, got %d", len(normalObs.Events()))
	}
}

// ---------------------------------------------------------------------------
// Edge cases: concurrent emit
// ---------------------------------------------------------------------------

func TestManager_ConcurrentEmit(t *testing.T) {
	mgr := observer.NewManager()
	var eventCount int64

	obs := &mockObserver{
		name: "concurrent-obs",
		onEvent: func(ctx context.Context, event observer.ConversationEvent) {
			atomic.AddInt64(&eventCount, 1)
		},
	}
	mgr.Register(obs)

	var wg sync.WaitGroup
	for i := 0; i < 100; i++ {
		wg.Add(1)
		go func(idx int) {
			defer wg.Done()
			mgr.Emit(context.Background(), observer.ConversationEvent{
				Type:    observer.EventToolCall,
				TraceID: "concurrent",
			})
		}(i)
	}

	wg.Wait()
	time.Sleep(500 * time.Millisecond) // Wait for all goroutines

	count := atomic.LoadInt64(&eventCount)
	if count != 100 {
		t.Errorf("Expected 100 events, got %d", count)
	}
}

func TestManager_ConcurrentEmitSync(t *testing.T) {
	mgr := observer.NewManager()
	var eventCount int64

	obs := &mockObserver{
		name: "sync-concurrent-obs",
		onEvent: func(ctx context.Context, event observer.ConversationEvent) {
			atomic.AddInt64(&eventCount, 1)
		},
	}
	mgr.Register(obs)

	var wg sync.WaitGroup
	for i := 0; i < 50; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			mgr.EmitSync(context.Background(), observer.ConversationEvent{
				Type: observer.EventConversationEnd,
			})
		}()
	}

	wg.Wait()

	count := atomic.LoadInt64(&eventCount)
	if count != 50 {
		t.Errorf("Expected 50 events, got %d", count)
	}
}

// ---------------------------------------------------------------------------
// Register and Unregister concurrent with Emit
// ---------------------------------------------------------------------------

func TestManager_ConcurrentRegisterUnregister(t *testing.T) {
	mgr := observer.NewManager()

	var wg sync.WaitGroup

	// Concurrently register, emit, unregister
	for i := 0; i < 20; i++ {
		wg.Add(3)
		go func() {
			defer wg.Done()
			mgr.Register(&mockObserver{name: "dynamic"})
		}()
		go func() {
			defer wg.Done()
			mgr.Emit(context.Background(), observer.ConversationEvent{Type: observer.EventToolCall})
		}()
		go func() {
			defer wg.Done()
			mgr.Unregister("dynamic")
		}()
	}

	wg.Wait()
	time.Sleep(200 * time.Millisecond)
}

// ---------------------------------------------------------------------------
// HasObservers
// ---------------------------------------------------------------------------

func TestManager_HasObservers(t *testing.T) {
	mgr := observer.NewManager()

	if mgr.HasObservers() {
		t.Error("New manager should have no observers")
	}

	mgr.Register(&mockObserver{name: "a"})
	if !mgr.HasObservers() {
		t.Error("Should have observers after register")
	}

	mgr.Unregister("a")
	if mgr.HasObservers() {
		t.Error("Should have no observers after unregister")
	}
}

// ---------------------------------------------------------------------------
// Verify all event data types can be created and accessed
// ---------------------------------------------------------------------------

func TestAllEventDataTypes(t *testing.T) {
	// ConversationStartData
	startData := &observer.ConversationStartData{
		SessionKey: "sk",
		Channel:    "ch",
		ChatID:     "cid",
		SenderID:   "sid",
		Content:    "hello",
	}
	if startData.SessionKey != "sk" {
		t.Error("ConversationStartData field mismatch")
	}

	// ConversationEndData
	endData := &observer.ConversationEndData{
		SessionKey:    "sk",
		Channel:       "ch",
		ChatID:        "cid",
		TotalRounds:   5,
		TotalDuration: 10 * time.Second,
		Content:       "bye",
		Error:         nil,
	}
	if endData.TotalRounds != 5 {
		t.Error("ConversationEndData field mismatch")
	}

	// LLMRequestData
	llmReqData := &observer.LLMRequestData{
		Round:        1,
		Model:        "model",
		ProviderName: "provider",
		APIKey:       "key",
		APIBase:      "http://localhost",
		HTTPHeaders:  map[string]string{"Authorization": "Bearer key"},
		FullConfig:   map[string]interface{}{"temperature": 0.7},
		Messages:     []providers.Message{{Role: "user", Content: "hi"}},
		Tools:        []providers.ToolDefinition{},
	}
	if llmReqData.Round != 1 {
		t.Error("LLMRequestData field mismatch")
	}

	// LLMResponseData
	llmRespData := &observer.LLMResponseData{
		Round:        2,
		Duration:     3 * time.Second,
		Content:      "response",
		ToolCalls:    []providers.ToolCall{},
		Usage:        &providers.UsageInfo{PromptTokens: 100, CompletionTokens: 50, TotalTokens: 150},
		FinishReason: "stop",
	}
	if llmRespData.Usage.TotalTokens != 150 {
		t.Error("LLMResponseData field mismatch")
	}

	// ToolCallData
	toolCallData := &observer.ToolCallData{
		ToolName:  "tool",
		Arguments: map[string]interface{}{"arg": "val"},
		Success:   true,
		Duration:  50 * time.Millisecond,
		Error:     "",
		LLMRound:  1,
		ChainPos:  0,
	}
	if toolCallData.ToolName != "tool" {
		t.Error("ToolCallData field mismatch")
	}
}

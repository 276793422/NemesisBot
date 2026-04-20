package observer_test

import (
	"context"
	"sync/atomic"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/observer"
)

// mockObserver records received events for test assertions
type mockObserver struct {
	name    string
	events  []observer.ConversationEvent
	count   int32
	eventCh chan observer.ConversationEvent
}

func newMockObserver(name string) *mockObserver {
	return &mockObserver{
		name:    name,
		eventCh: make(chan observer.ConversationEvent, 100),
	}
}

func (m *mockObserver) Name() string { return m.name }

func (m *mockObserver) OnEvent(ctx context.Context, event observer.ConversationEvent) {
	atomic.AddInt32(&m.count, 1)
	m.events = append(m.events, event)
	select {
	case m.eventCh <- event:
	default:
	}
}

func (m *mockObserver) WaitCount(n int32) bool {
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		if atomic.LoadInt32(&m.count) >= n {
			return true
		}
		time.Sleep(10 * time.Millisecond)
	}
	return false
}

func TestManagerNew(t *testing.T) {
	mgr := observer.NewManager()
	if mgr == nil {
		t.Fatal("NewManager returned nil")
	}
	if mgr.HasObservers() {
		t.Fatal("new manager should have no observers")
	}
}

func TestManagerRegister(t *testing.T) {
	mgr := observer.NewManager()
	obs := newMockObserver("test")
	mgr.Register(obs)

	if !mgr.HasObservers() {
		t.Fatal("manager should have observers after register")
	}
}

func TestManagerUnregister(t *testing.T) {
	mgr := observer.NewManager()
	obs := newMockObserver("test")
	mgr.Register(obs)
	mgr.Unregister("test")

	if mgr.HasObservers() {
		t.Fatal("manager should have no observers after unregister")
	}
}

func TestManagerEmitAsync(t *testing.T) {
	mgr := observer.NewManager()
	obs := newMockObserver("test")
	mgr.Register(obs)

	mgr.Emit(context.Background(), observer.ConversationEvent{
		Type:      observer.EventConversationStart,
		TraceID:   "trace-1",
		Timestamp: time.Now(),
		Data: &observer.ConversationStartData{
			SessionKey: "session1",
			Channel:    "web",
			ChatID:     "chat1",
			Content:    "hello",
		},
	})

	if !obs.WaitCount(1) {
		t.Fatal("observer should have received 1 event")
	}
	if obs.events[0].Type != observer.EventConversationStart {
		t.Fatalf("expected conversation_start, got %s", obs.events[0].Type)
	}
}

func TestManagerEmitSync(t *testing.T) {
	mgr := observer.NewManager()
	obs := newMockObserver("test")
	mgr.Register(obs)

	mgr.EmitSync(context.Background(), observer.ConversationEvent{
		Type:    observer.EventConversationEnd,
		TraceID: "trace-1",
		Data: &observer.ConversationEndData{
			TotalRounds: 3,
		},
	})

	if atomic.LoadInt32(&obs.count) != 1 {
		t.Fatalf("expected 1 event, got %d", obs.count)
	}
}

func TestManagerMultipleObservers(t *testing.T) {
	mgr := observer.NewManager()
	obs1 := newMockObserver("obs1")
	obs2 := newMockObserver("obs2")
	mgr.Register(obs1)
	mgr.Register(obs2)

	mgr.Emit(context.Background(), observer.ConversationEvent{
		Type:    observer.EventToolCall,
		TraceID: "trace-1",
		Data: &observer.ToolCallData{
			ToolName: "read_file",
			Success:  true,
		},
	})

	if !obs1.WaitCount(1) || !obs2.WaitCount(1) {
		t.Fatal("both observers should have received the event")
	}
}

func TestEventTypes(t *testing.T) {
	types := []observer.EventType{
		observer.EventConversationStart,
		observer.EventConversationEnd,
		observer.EventLLMRequest,
		observer.EventLLMResponse,
		observer.EventToolCall,
	}
	expected := []string{
		"conversation_start",
		"conversation_end",
		"llm_request",
		"llm_response",
		"tool_call",
	}
	for i, et := range types {
		if string(et) != expected[i] {
			t.Errorf("event type %d: expected %s, got %s", i, expected[i], et)
		}
	}
}

func TestConversationStartDataFields(t *testing.T) {
	mgr := observer.NewManager()
	obs := newMockObserver("test")
	mgr.Register(obs)

	mgr.EmitSync(context.Background(), observer.ConversationEvent{
		Type:    observer.EventConversationStart,
		TraceID: "trace-1",
		Data: &observer.ConversationStartData{
			SessionKey: "key1",
			Channel:    "web",
			ChatID:     "chat1",
			SenderID:   "user1",
			Content:    "hello world",
		},
	})

	if len(obs.events) != 1 {
		t.Fatalf("expected 1 event, got %d", len(obs.events))
	}
	data, ok := obs.events[0].Data.(*observer.ConversationStartData)
	if !ok {
		t.Fatal("expected ConversationStartData")
	}
	if data.SessionKey != "key1" || data.Channel != "web" || data.Content != "hello world" {
		t.Fatalf("unexpected data: %+v", data)
	}
}

func TestToolCallDataFields(t *testing.T) {
	mgr := observer.NewManager()
	obs := newMockObserver("test")
	mgr.Register(obs)

	mgr.EmitSync(context.Background(), observer.ConversationEvent{
		Type:    observer.EventToolCall,
		TraceID: "trace-1",
		Data: &observer.ToolCallData{
			ToolName:  "exec",
			Arguments: map[string]interface{}{"cmd": "ls"},
			Success:   false,
			Duration:  100 * time.Millisecond,
			Error:     "exit code 1",
			LLMRound:  2,
			ChainPos:  1,
		},
	})

	data := obs.events[0].Data.(*observer.ToolCallData)
	if data.ToolName != "exec" || data.Success != false || data.Error != "exit code 1" {
		t.Fatalf("unexpected data: %+v", data)
	}
	if data.LLMRound != 2 || data.ChainPos != 1 {
		t.Fatalf("unexpected round/chain: round=%d chain=%d", data.LLMRound, data.ChainPos)
	}
}

func TestUnregisterNonExistent(t *testing.T) {
	mgr := observer.NewManager()
	// Should not panic
	mgr.Unregister("nonexistent")
}

func TestEmitWithNoObservers(t *testing.T) {
	mgr := observer.NewManager()
	// Should not panic
	mgr.Emit(context.Background(), observer.ConversationEvent{
		Type: observer.EventConversationStart,
	})
	mgr.EmitSync(context.Background(), observer.ConversationEvent{
		Type: observer.EventConversationEnd,
	})
}

func TestPanicRecovery(t *testing.T) {
	mgr := observer.NewManager()
	panicObs := &panicObserver{}
	mgr.Register(panicObs)

	// Should not panic
	mgr.Emit(context.Background(), observer.ConversationEvent{
		Type: observer.EventConversationStart,
	})
	mgr.EmitSync(context.Background(), observer.ConversationEvent{
		Type: observer.EventConversationStart,
	})
}

type panicObserver struct{}

func (p *panicObserver) Name() string { return "panic" }
func (p *panicObserver) OnEvent(ctx context.Context, event observer.ConversationEvent) {
	panic("test panic")
}

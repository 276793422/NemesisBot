//go:build !cross_compile

package websocket

import (
	"context"
	"sync/atomic"
	"testing"
)

func TestDispatcherRegisterAndDispatchRequest(t *testing.T) {
	d := NewDispatcher()
	called := false

	d.Register("test.method", func(ctx context.Context, msg *Message) (*Message, error) {
		called = true
		return NewResponse(msg.ID, map[string]string{"echo": "ok"})
	})

	req, _ := NewRequest("test.method", map[string]string{"input": "hello"})
	resp, err := d.Dispatch(context.Background(), req)
	if err != nil {
		t.Fatalf("Dispatch: %v", err)
	}
	if !called {
		t.Error("handler was not called")
	}
	if resp == nil {
		t.Fatal("response should not be nil")
	}
	if !resp.IsSuccessResponse() {
		t.Error("response should be success")
	}

	var result map[string]string
	if err := resp.DecodeResult(&result); err != nil {
		t.Fatalf("DecodeResult: %v", err)
	}
	if result["echo"] != "ok" {
		t.Errorf("result[\"echo\"] = %q, want %q", result["echo"], "ok")
	}
}

func TestDispatcherNotification(t *testing.T) {
	d := NewDispatcher()
	var called atomic.Int32

	d.RegisterNotification("test.notify", func(ctx context.Context, msg *Message) {
		called.Add(1)
	})

	notif, _ := NewNotification("test.notify", nil)
	resp, err := d.Dispatch(context.Background(), notif)
	if err != nil {
		t.Fatalf("Dispatch: %v", err)
	}
	if resp != nil {
		t.Error("notification dispatch should return nil")
	}
	if called.Load() != 1 {
		t.Errorf("notification handler called %d times, want 1", called.Load())
	}
}

func TestDispatcherMethodNotFound(t *testing.T) {
	d := NewDispatcher()

	req, _ := NewRequest("unknown.method", nil)
	resp, err := d.Dispatch(context.Background(), req)
	if err != nil {
		t.Fatalf("Dispatch: %v", err)
	}
	if resp == nil {
		t.Fatal("should return error response")
	}
	if !resp.IsErrorResponse() {
		t.Error("response should be error")
	}
	if resp.Error.Code != ErrMethodNotFound {
		t.Errorf("error code = %d, want %d", resp.Error.Code, ErrMethodNotFound)
	}
}

func TestDispatcherFallback(t *testing.T) {
	d := NewDispatcher()
	fallbackCalled := false

	d.SetFallback(func(ctx context.Context, msg *Message) (*Message, error) {
		fallbackCalled = true
		return NewResponse(msg.ID, map[string]string{"fallback": "true"})
	})

	req, _ := NewRequest("unknown.method", nil)
	resp, err := d.Dispatch(context.Background(), req)
	if err != nil {
		t.Fatalf("Dispatch: %v", err)
	}
	if !fallbackCalled {
		t.Error("fallback handler was not called")
	}
	if resp == nil {
		t.Fatal("response should not be nil")
	}

	var result map[string]string
	if err := resp.DecodeResult(&result); err != nil {
		t.Fatalf("DecodeResult: %v", err)
	}
	if result["fallback"] != "true" {
		t.Errorf("result[\"fallback\"] = %q, want %q", result["fallback"], "true")
	}
}

func TestDispatcherFallbackNotUsedForRegisteredMethod(t *testing.T) {
	d := NewDispatcher()
	handlerCalled := false
	fallbackCalled := false

	d.Register("test.method", func(ctx context.Context, msg *Message) (*Message, error) {
		handlerCalled = true
		return NewResponse(msg.ID, nil)
	})
	d.SetFallback(func(ctx context.Context, msg *Message) (*Message, error) {
		fallbackCalled = true
		return NewResponse(msg.ID, nil)
	})

	req, _ := NewRequest("test.method", nil)
	_, err := d.Dispatch(context.Background(), req)
	if err != nil {
		t.Fatalf("Dispatch: %v", err)
	}
	if !handlerCalled {
		t.Error("registered handler was not called")
	}
	if fallbackCalled {
		t.Error("fallback should not be called when handler is registered")
	}
}

func TestDispatcherUnknownNotificationIgnored(t *testing.T) {
	d := NewDispatcher()

	notif, _ := NewNotification("unknown.notify", nil)
	resp, err := d.Dispatch(context.Background(), notif)
	if err != nil {
		t.Fatalf("Dispatch: %v", err)
	}
	if resp != nil {
		t.Error("unknown notification should return nil response")
	}
}

func TestDispatcherInvalidMessage(t *testing.T) {
	d := NewDispatcher()

	// A response message (has ID, no method) is neither request nor notification
	msg := &Message{JSONRPC: Version, ID: "some-id"}
	_, err := d.Dispatch(context.Background(), msg)
	if err == nil {
		t.Error("expected error for message that is neither request nor notification")
	}
}

func TestDispatcherMultipleHandlers(t *testing.T) {
	d := NewDispatcher()
	var a, b int32

	d.Register("method.a", func(ctx context.Context, msg *Message) (*Message, error) {
		atomic.AddInt32(&a, 1)
		return NewResponse(msg.ID, nil)
	})
	d.Register("method.b", func(ctx context.Context, msg *Message) (*Message, error) {
		atomic.AddInt32(&b, 1)
		return NewResponse(msg.ID, nil)
	})

	reqA, _ := NewRequest("method.a", nil)
	reqB, _ := NewRequest("method.b", nil)

	d.Dispatch(context.Background(), reqA)
	d.Dispatch(context.Background(), reqB)

	if atomic.LoadInt32(&a) != 1 {
		t.Errorf("method.a called %d times, want 1", a)
	}
	if atomic.LoadInt32(&b) != 1 {
		t.Errorf("method.b called %d times, want 1", b)
	}
}

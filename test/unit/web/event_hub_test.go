// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Web Channel Unit Tests - SSE EventHub

package web_test

import (
	"sync"
	"testing"
	"time"

	. "github.com/276793422/NemesisBot/module/web"
)

func TestEventHub_New(t *testing.T) {
	hub := NewEventHub()
	if hub == nil {
		t.Fatal("NewEventHub returned nil")
	}
	if hub.SubscriberCount() != 0 {
		t.Errorf("new hub subscriber count = %d, want 0", hub.SubscriberCount())
	}
}

func TestEventHub_Subscribe(t *testing.T) {
	hub := NewEventHub()
	ch := hub.Subscribe()

	if ch == nil {
		t.Fatal("Subscribe returned nil channel")
	}
	if hub.SubscriberCount() != 1 {
		t.Errorf("subscriber count = %d, want 1", hub.SubscriberCount())
	}
	hub.Unsubscribe(ch)
}

func TestEventHub_Unsubscribe(t *testing.T) {
	hub := NewEventHub()
	ch := hub.Subscribe()

	hub.Unsubscribe(ch)

	if hub.SubscriberCount() != 0 {
		t.Errorf("subscriber count after unsubscribe = %d, want 0", hub.SubscriberCount())
	}

	// Channel should be closed
	_, ok := <-ch
	if ok {
		t.Error("channel should be closed after unsubscribe")
	}
}

func TestEventHub_Publish_DeliversEvent(t *testing.T) {
	hub := NewEventHub()
	ch := hub.Subscribe()
	defer hub.Unsubscribe(ch)

	hub.Publish("test-type", map[string]interface{}{"key": "value"})

	select {
	case event := <-ch:
		if event.Type != "test-type" {
			t.Errorf("event type = %q, want 'test-type'", event.Type)
		}
		data, ok := event.Data.(map[string]interface{})
		if !ok {
			t.Fatal("event data is not a map")
		}
		if data["key"] != "value" {
			t.Errorf("event data key = %v, want 'value'", data["key"])
		}
	case <-time.After(time.Second):
		t.Fatal("timed out waiting for event")
	}
}

func TestEventHub_Publish_MultipleSubscribers(t *testing.T) {
	hub := NewEventHub()
	ch1 := hub.Subscribe()
	ch2 := hub.Subscribe()
	defer hub.Unsubscribe(ch1)
	defer hub.Unsubscribe(ch2)

	hub.Publish("broadcast", "hello")

	// Both subscribers should receive the event
	for i, ch := range []chan Event{ch1, ch2} {
		select {
		case event := <-ch:
			if event.Type != "broadcast" {
				t.Errorf("subscriber %d: event type = %q, want 'broadcast'", i, event.Type)
			}
			if event.Data != "hello" {
				t.Errorf("subscriber %d: event data = %v, want 'hello'", i, event.Data)
			}
		case <-time.After(time.Second):
			t.Fatalf("subscriber %d timed out waiting for event", i)
		}
	}
}

func TestEventHub_Publish_ChannelOverflow(t *testing.T) {
	hub := NewEventHub()
	ch := hub.Subscribe()
	defer hub.Unsubscribe(ch)

	// Publish more than buffer size (32) - should not block
	for i := 0; i < 40; i++ {
		hub.Publish("overflow", i)
	}

	// Should be able to read at least buffer size events
	received := 0
	timeout := time.After(time.Second)
	for {
		select {
		case <-ch:
			received++
		case <-timeout:
			goto done
		}
	}
done:
	if received != 32 {
		t.Errorf("received %d events, want 32 (buffer size)", received)
	}
}

func TestEventHub_SubscriberCount_Multiple(t *testing.T) {
	hub := NewEventHub()

	channels := make([]chan Event, 5)
	for i := 0; i < 5; i++ {
		channels[i] = hub.Subscribe()
	}

	if hub.SubscriberCount() != 5 {
		t.Errorf("subscriber count = %d, want 5", hub.SubscriberCount())
	}

	// Unsubscribe 2
	hub.Unsubscribe(channels[0])
	hub.Unsubscribe(channels[1])

	if hub.SubscriberCount() != 3 {
		t.Errorf("subscriber count after unsubscribe = %d, want 3", hub.SubscriberCount())
	}

	// Clean up remaining
	for i := 2; i < 5; i++ {
		hub.Unsubscribe(channels[i])
	}
}

func TestEventHub_ConcurrentPublish(t *testing.T) {
	hub := NewEventHub()
	ch := hub.Subscribe()
	defer hub.Unsubscribe(ch)

	var wg sync.WaitGroup
	for i := 0; i < 10; i++ {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			hub.Publish("concurrent", i)
		}(i)
	}
	wg.Wait()

	// Collect all events
	received := 0
	timeout := time.After(time.Second)
	for {
		select {
		case <-ch:
			received++
		case <-timeout:
			goto done2
		}
	}
done2:
	if received != 10 {
		t.Errorf("received %d events, want 10", received)
	}
}

func TestEventHub_ConcurrentSubscribeUnsubscribe(t *testing.T) {
	hub := NewEventHub()
	var wg sync.WaitGroup

	// Concurrent subscribe/unsubscribe should not race
	for i := 0; i < 20; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			ch := hub.Subscribe()
			hub.Publish("test", nil)
			hub.Unsubscribe(ch)
		}()
	}
	wg.Wait()

	// All should be unsubscribed
	if hub.SubscriberCount() != 0 {
		t.Errorf("subscriber count = %d, want 0 after all unsubscribe", hub.SubscriberCount())
	}
}

func TestEventHub_Publish_NilData(t *testing.T) {
	hub := NewEventHub()
	ch := hub.Subscribe()
	defer hub.Unsubscribe(ch)

	hub.Publish("nil-data", nil)

	select {
	case event := <-ch:
		if event.Data != nil {
			t.Errorf("event data = %v, want nil", event.Data)
		}
	case <-time.After(time.Second):
		t.Fatal("timed out waiting for event")
	}
}

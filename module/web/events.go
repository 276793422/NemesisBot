// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Web Channel Module - SSE Event Hub

package web

import "sync"

// Event represents a server-sent event
type Event struct {
	Type string      // Event type: log, status, security-alert, scanner-progress, cluster-event, heartbeat
	Data interface{} // Event payload
}

// EventHub manages SSE subscribers and broadcasts events
type EventHub struct {
	mu          sync.RWMutex
	subscribers map[chan Event]bool
}

// NewEventHub creates a new EventHub
func NewEventHub() *EventHub {
	return &EventHub{
		subscribers: make(map[chan Event]bool),
	}
}

// Subscribe creates and returns a new event channel
func (h *EventHub) Subscribe() chan Event {
	ch := make(chan Event, 32)
	h.mu.Lock()
	h.subscribers[ch] = true
	h.mu.Unlock()
	return ch
}

// Unsubscribe removes a subscriber channel
func (h *EventHub) Unsubscribe(ch chan Event) {
	h.mu.Lock()
	delete(h.subscribers, ch)
	h.mu.Unlock()
	close(ch)
}

// Publish sends an event to all subscribers
func (h *EventHub) Publish(eventType string, data interface{}) {
	event := Event{
		Type: eventType,
		Data: data,
	}
	h.mu.RLock()
	defer h.mu.RUnlock()
	for ch := range h.subscribers {
		select {
		case ch <- event:
		default:
			// Drop event if subscriber channel is full
		}
	}
}

// SubscriberCount returns the number of active subscribers
func (h *EventHub) SubscriberCount() int {
	h.mu.RLock()
	defer h.mu.RUnlock()
	return len(h.subscribers)
}

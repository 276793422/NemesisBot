package client

import (
	"sync"
	"time"
)

// MessageQueue is a thread-safe FIFO queue with blocking dequeue and timeout support.
type MessageQueue struct {
	mu      sync.Mutex
	cond    *sync.Cond
	items   [][]byte
	closed  bool
}

// NewMessageQueue creates a new empty message queue.
func NewMessageQueue() *MessageQueue {
	q := &MessageQueue{}
	q.cond = sync.NewCond(&q.mu)
	return q
}

// Enqueue adds a message to the queue and wakes one waiting consumer.
func (q *MessageQueue) Enqueue(data []byte) {
	q.mu.Lock()
	defer q.mu.Unlock()
	if q.closed {
		return
	}
	q.items = append(q.items, data)
	q.cond.Signal()
}

// Dequeue waits for a message up to timeoutMs milliseconds.
// timeoutMs <= 0 means non-blocking (return immediately if empty).
// Returns (data, true) on success, (nil, false) on timeout or closed queue.
func (q *MessageQueue) Dequeue(timeoutMs int) ([]byte, bool) {
	q.mu.Lock()
	defer q.mu.Unlock()

	if len(q.items) > 0 {
		return q.pop(), true
	}

	if q.closed {
		return nil, false
	}

	if timeoutMs == 0 {
		// Non-blocking
		return nil, false
	}

	// Wait with timeout
	var deadline time.Time
	if timeoutMs > 0 {
		deadline = time.Now().Add(time.Duration(timeoutMs) * time.Millisecond)
	}

	for len(q.items) == 0 && !q.closed {
		if timeoutMs > 0 {
			remaining := time.Until(deadline)
			if remaining <= 0 {
				return nil, false
			}
			// Use a goroutine + timer to wake the cond after timeout
			// since sync.Cond doesn't support timed wait natively.
			go func() {
				time.Sleep(remaining)
				q.cond.Broadcast()
			}()
			q.cond.Wait()
			if len(q.items) > 0 {
				return q.pop(), true
			}
			if q.closed {
				return nil, false
			}
			// Timed out
			if time.Now().After(deadline) {
				return nil, false
			}
		} else {
			// Infinite wait (timeoutMs < 0)
			q.cond.Wait()
			if len(q.items) > 0 {
				return q.pop(), true
			}
			if q.closed {
				return nil, false
			}
		}
	}

	return nil, false
}

// Close closes the queue and wakes all waiting consumers.
func (q *MessageQueue) Close() {
	q.mu.Lock()
	defer q.mu.Unlock()
	q.closed = true
	q.cond.Broadcast()
}

func (q *MessageQueue) pop() []byte {
	item := q.items[0]
	q.items[0] = nil // help GC
	q.items = q.items[1:]
	return item
}

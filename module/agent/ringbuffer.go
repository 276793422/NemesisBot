// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package agent

import "sync"

// RingBuffer is a thread-safe, generic circular buffer.
// When the buffer is full, new Push calls overwrite the oldest entry.
// Items are returned in insertion order (oldest first).
type RingBuffer[T any] struct {
	buf   []T
	size  int
	head  int // index of next write position
	count int // number of items currently in the buffer
	mu    sync.RWMutex
}

// NewRingBuffer creates a new RingBuffer with the given fixed capacity.
// Capacity must be positive; a capacity of 0 is clamped to 1.
func NewRingBuffer[T any](size int) *RingBuffer[T] {
	if size <= 0 {
		size = 1
	}
	return &RingBuffer[T]{
		buf:  make([]T, size),
		size: size,
	}
}

// Push adds an item to the ring buffer. If the buffer is full,
// the oldest entry is overwritten.
func (rb *RingBuffer[T]) Push(item T) {
	rb.mu.Lock()
	defer rb.mu.Unlock()

	rb.buf[rb.head] = item
	rb.head = (rb.head + 1) % rb.size
	if rb.count < rb.size {
		rb.count++
	}
}

// GetAll returns all items in insertion order (oldest first).
// The returned slice is a copy; modifications do not affect the buffer.
func (rb *RingBuffer[T]) GetAll() []T {
	rb.mu.RLock()
	defer rb.mu.RUnlock()

	if rb.count == 0 {
		return nil
	}

	result := make([]T, rb.count)
	for i := 0; i < rb.count; i++ {
		// Calculate the index of the i-th oldest item.
		// When the buffer is full, the oldest item is at rb.head.
		// Otherwise, it starts at index 0.
		idx := (rb.head - rb.count + i + rb.size) % rb.size
		result[i] = rb.buf[idx]
	}
	return result
}

// GetLast returns the last n items in insertion order (oldest first).
// If n is greater than the buffer length, all items are returned.
// If n <= 0, returns nil.
func (rb *RingBuffer[T]) GetLast(n int) []T {
	if n <= 0 {
		return nil
	}

	rb.mu.RLock()
	defer rb.mu.RUnlock()

	if rb.count == 0 {
		return nil
	}

	// Clamp n to available count
	if n > rb.count {
		n = rb.count
	}

	result := make([]T, n)
	startIdx := rb.count - n
	for i := 0; i < n; i++ {
		idx := (rb.head - rb.count + startIdx + i + rb.size) % rb.size
		result[i] = rb.buf[idx]
	}
	return result
}

// Len returns the number of items currently in the buffer.
func (rb *RingBuffer[T]) Len() int {
	rb.mu.RLock()
	defer rb.mu.RUnlock()
	return rb.count
}

// Clear removes all items from the buffer.
func (rb *RingBuffer[T]) Clear() {
	rb.mu.Lock()
	defer rb.mu.Unlock()

	// Zero out entries to help GC for reference types
	var zero T
	for i := range rb.buf {
		rb.buf[i] = zero
	}
	rb.head = 0
	rb.count = 0
}

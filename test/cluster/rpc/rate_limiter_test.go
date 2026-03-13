// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package rpc_test

import (
	"context"
	"sync"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/cluster/rpc"
)

func TestNewRateLimiter(t *testing.T) {
	limiter := rpc.NewRateLimiter(10, 1*time.Second, 30, 10*time.Second)
	if limiter == nil {
		t.Fatal("NewRateLimiter() returned nil")
	}
}

func TestRateLimiterAcquireSuccess(t *testing.T) {
	limiter := rpc.NewRateLimiter(5, 1*time.Second, 10, 1*time.Second)

	// Should be able to acquire 5 tokens immediately
	for i := 0; i < 5; i++ {
		err := limiter.Acquire(context.Background(), "peer-1")
		if err != nil {
			t.Errorf("Acquire() failed at iteration %d: %v", i, err)
		}
	}
}

func TestRateLimiterAcquireFail(t *testing.T) {
	// Use a very short refill rate to ensure tokens don't refill during test
	limiter := rpc.NewRateLimiter(2, 10*time.Second, 10, 1*time.Second)

	// Acquire all tokens (2 tokens available)
	for i := 0; i < 2; i++ {
		err := limiter.Acquire(context.Background(), "peer-1")
		if err != nil {
			t.Fatalf("Acquire() %d failed: %v", i+1, err)
		}
	}

	// Create a context with short timeout to avoid waiting for refill
	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()

	// Should fail to acquire more (will wait and timeout before refill)
	err := limiter.Acquire(ctx, "peer-1")
	if err == nil {
		t.Error("Expected rate limit error, got none")
	}
}

func TestRateLimiterRelease(t *testing.T) {
	limiter := rpc.NewRateLimiter(5, 1*time.Second, 10, 1*time.Second)

	// Acquire some tokens
	limiter.Acquire(context.Background(), "peer-1")
	limiter.Acquire(context.Background(), "peer-1")

	// Release one
	limiter.Release("peer-1")

	// Should be able to acquire one more
	err := limiter.Acquire(context.Background(), "peer-1")
	if err != nil {
		t.Errorf("Acquire() failed after release: %v", err)
	}
}

func TestRateLimiterTimeout(t *testing.T) {
	limiter := rpc.NewRateLimiter(1, 100*time.Millisecond, 2, 100*time.Millisecond)

	// Acquire the only token
	err := limiter.Acquire(context.Background(), "peer-1")
	if err != nil {
		t.Fatalf("First Acquire() failed: %v", err)
	}

	// Create context that will timeout before refill
	ctx, cancel := context.WithTimeout(context.Background(), 50*time.Millisecond)
	defer cancel()

	// Should fail due to timeout
	err = limiter.Acquire(ctx, "peer-1")
	if err == nil {
		t.Error("Expected timeout error, got none")
	}
}

func TestRateLimiterRefill(t *testing.T) {
	limiter := rpc.NewRateLimiter(1, 100*time.Millisecond, 2, 1*time.Second)

	// Acquire the only token
	err := limiter.Acquire(context.Background(), "peer-1")
	if err != nil {
		t.Fatalf("First Acquire() failed: %v", err)
	}

	// Wait for refill
	time.Sleep(150 * time.Millisecond)

	// Should be able to acquire again
	err = limiter.Acquire(context.Background(), "peer-1")
	if err != nil {
		t.Errorf("Acquire() after refill failed: %v", err)
	}
}

func TestRateLimiterDifferentPeers(t *testing.T) {
	limiter := rpc.NewRateLimiter(2, 1*time.Second, 5, 1*time.Second)

	// Should be able to acquire for different peers independently
	err := limiter.Acquire(context.Background(), "peer-1")
	if err != nil {
		t.Errorf("Acquire() for peer-1 failed: %v", err)
	}

	err = limiter.Acquire(context.Background(), "peer-2")
	if err != nil {
		t.Errorf("Acquire() for peer-2 failed: %v", err)
	}

	// Should be able to acquire more for each peer
	err = limiter.Acquire(context.Background(), "peer-1")
	if err != nil {
		t.Errorf("Acquire() second for peer-1 failed: %v", err)
	}

	err = limiter.Acquire(context.Background(), "peer-2")
	if err != nil {
		t.Errorf("Acquire() second for peer-2 failed: %v", err)
	}
}

func TestRateLimiterBurstLimit(t *testing.T) {
	limiter := rpc.NewRateLimiter(10, 1*time.Second, 3, 1*time.Second)

	// Should be able to burst 3 requests
	for i := 0; i < 3; i++ {
		err := limiter.Acquire(context.Background(), "peer-1")
		if err != nil {
			t.Errorf("Acquire() burst %d failed: %v", i, err)
		}
	}

	// Fourth should fail due to burst limit
	err := limiter.Acquire(context.Background(), "peer-1")
	if err == nil {
		t.Error("Expected burst limit error, got none")
	}
}

func TestRateLimiterConcurrent(t *testing.T) {
	limiter := rpc.NewRateLimiter(10, 1*time.Second, 20, 1*time.Second)

	// Test concurrent access
	const numGoroutines = 5
	const numAcquires = 2
	var wg sync.WaitGroup
	errors := make(chan error, numGoroutines*numAcquires)

	for i := 0; i < numGoroutines; i++ {
		wg.Add(1)
		go func(goroutineID int) {
			defer wg.Done()

			for j := 0; j < numAcquires; j++ {
				err := limiter.Acquire(context.Background(), "peer-1")
				if err != nil {
					errors <- err
				}
				time.Sleep(10 * time.Millisecond) // Small delay
				limiter.Release("peer-1")
			}
		}(i)
	}

	wg.Wait()
	close(errors)

	// Check for errors
	for err := range errors {
		t.Errorf("Concurrent Acquire/Release failed: %v", err)
	}
}

func TestRateLimiterWindowCleanup(t *testing.T) {
	limiter := rpc.NewRateLimiter(10, 1*time.Second, 3, 200*time.Millisecond)

	// Make some requests to populate the window
	for i := 0; i < 3; i++ {
		limiter.Acquire(context.Background(), "peer-1")
		limiter.Release("peer-1")
		time.Sleep(50 * time.Millisecond)
	}

	// Wait for window to expire
	time.Sleep(250 * time.Millisecond)

	// Should be able to make new requests
	err := limiter.Acquire(context.Background(), "peer-1")
	if err != nil {
		t.Errorf("Acquire() after window cleanup failed: %v", err)
	}
}

func TestRateLimiterInitialState(t *testing.T) {
	// Use a very short refill rate to ensure tokens don't refill during test
	limiter := rpc.NewRateLimiter(1, 10*time.Second, 30, 10*time.Second)

	// Check that new peer starts with max tokens (1 in this case)
	err := limiter.Acquire(context.Background(), "new-peer")
	if err != nil {
		t.Errorf("Acquire() for new peer failed: %v", err)
	}

	// Create a context with short timeout to avoid waiting for refill
	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()

	// Try to acquire again (should fail since we only have 1 token)
	err = limiter.Acquire(ctx, "new-peer")
	if err == nil {
		t.Error("Expected immediate rate limit for new peer, got none")
	}
}

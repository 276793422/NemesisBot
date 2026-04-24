package services

import (
	"context"
	"errors"
	"testing"
	"time"
)

func TestParallelInit_AllSuccess(t *testing.T) {
	count := make(chan struct{}, 3)

	err := parallelInit(context.Background(),
		func() error { count <- struct{}{}; return nil },
		func() error { count <- struct{}{}; return nil },
		func() error { count <- struct{}{}; return nil },
	)

	if err != nil {
		t.Fatalf("expected no error, got: %v", err)
	}
	if len(count) != 3 {
		t.Fatalf("expected count=3, got %d", len(count))
	}
}

func TestParallelInit_FirstError(t *testing.T) {
	testErr := errors.New("init failed")

	err := parallelInit(context.Background(),
		func() error { return testErr },
		func() error { return nil },
	)

	if err == nil {
		t.Fatal("expected error, got nil")
	}
	if !errors.Is(err, testErr) {
		t.Fatalf("expected testErr, got: %v", err)
	}
}

func TestParallelInit_ContextCancellation(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel() // cancel immediately

	err := parallelInit(ctx,
		func() error { return nil },
	)

	if err == nil {
		t.Fatal("expected context cancelled error, got nil")
	}
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("expected context.Canceled, got: %v", err)
	}
}

func TestParallelInit_Empty(t *testing.T) {
	err := parallelInit(context.Background())
	if err != nil {
		t.Fatalf("expected no error for empty inits, got: %v", err)
	}
}

func TestParallelInit_ConcurrentExecution(t *testing.T) {
	// Verify that init functions actually run concurrently by using a sync
	// pattern: goroutine A starts and blocks, goroutine B must be able to
	// start while A is blocked (proving concurrency).
	//
	// We use a two-phase approach: first goroutine signals it started and
	// waits, second goroutine signals it started and completes. If both
	// started, we know they ran concurrently.
	started := make(chan int, 3)
	release := make(chan struct{})

	errCh := make(chan error, 1)
	go func() {
		errCh <- parallelInit(context.Background(),
			func() error {
				started <- 1
				<-release // block until released
				return nil
			},
			func() error {
				started <- 2
				<-release // block until released
				return nil
			},
			func() error {
				started <- 3
				<-release // block until released
				return nil
			},
		)
	}()

	// Wait for all 3 goroutines to signal they started
	gotStarted := 0
	timeout := time.After(5 * time.Second)
	for gotStarted < 3 {
		select {
		case <-started:
			gotStarted++
		case <-timeout:
			t.Fatalf("timed out waiting for goroutines to start (got %d/3)", gotStarted)
		}
	}

	// All 3 started concurrently while blocking on release
	close(release)

	// Wait for parallelInit to complete
	if err := <-errCh; err != nil {
		t.Fatalf("expected no error, got: %v", err)
	}
}

func TestSequentialInit_AllSuccess(t *testing.T) {
	var order []int

	err := sequentialInit(
		func() error { order = append(order, 1); return nil },
		func() error { order = append(order, 2); return nil },
		func() error { order = append(order, 3); return nil },
	)

	if err != nil {
		t.Fatalf("expected no error, got: %v", err)
	}
	if len(order) != 3 || order[0] != 1 || order[1] != 2 || order[2] != 3 {
		t.Fatalf("expected order [1,2,3], got %v", order)
	}
}

func TestSequentialInit_StopsOnError(t *testing.T) {
	testErr := errors.New("step 2 failed")
	var order []int

	err := sequentialInit(
		func() error { order = append(order, 1); return nil },
		func() error { order = append(order, 2); return testErr },
		func() error { order = append(order, 3); return nil },
	)

	if !errors.Is(err, testErr) {
		t.Fatalf("expected testErr, got: %v", err)
	}
	if len(order) != 2 || order[0] != 1 || order[1] != 2 {
		t.Fatalf("expected order [1,2], got %v", order)
	}
}

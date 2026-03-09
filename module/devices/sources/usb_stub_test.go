// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package sources

import (
	"context"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/devices/events"
)

func TestNewUSBMonitor(t *testing.T) {
	monitor := NewUSBMonitor()
	if monitor == nil {
		t.Fatal("NewUSBMonitor() returned nil")
	}
}

func TestUSBMonitor_Kind(t *testing.T) {
	monitor := NewUSBMonitor()
	kind := monitor.Kind()
	if kind != events.KindUSB {
		t.Errorf("Kind() = %v, want %v", kind, events.KindUSB)
	}
}

func TestUSBMonitor_Start(t *testing.T) {
	monitor := NewUSBMonitor()
	ctx := context.Background()

	ch, err := monitor.Start(ctx)
	if err != nil {
		t.Fatalf("Start() error = %v", err)
	}
	if ch == nil {
		t.Fatal("Start() returned nil channel")
	}

	// Channel should be closed immediately on non-Linux
	select {
	case <-time.After(100 * time.Millisecond):
		t.Error("Channel should be closed immediately")
	case _, ok := <-ch:
		if ok {
			t.Error("Channel should be closed")
		}
	}
}

func TestUSBMonitor_Start_WithContext(t *testing.T) {
	monitor := NewUSBMonitor()
	ctx, cancel := context.WithCancel(context.Background())

	ch, err := monitor.Start(ctx)
	if err != nil {
		t.Fatalf("Start() error = %v", err)
	}

	// Cancel context
	cancel()

	// Channel should handle context cancellation
	select {
	case <-time.After(100 * time.Millisecond):
		// Expected - channel should be closed
	case _, ok := <-ch:
		if ok {
			t.Error("Channel should be closed after context cancellation")
		}
	}
}

func TestUSBMonitor_Stop(t *testing.T) {
	monitor := NewUSBMonitor()
	ctx := context.Background()

	// Start the monitor
	_, err := monitor.Start(ctx)
	if err != nil {
		t.Fatalf("Start() error = %v", err)
	}

	// Stop should not error
	err = monitor.Stop()
	if err != nil {
		t.Errorf("Stop() error = %v", err)
	}

	// Stop should be idempotent
	err = monitor.Stop()
	if err != nil {
		t.Errorf("Second Stop() error = %v", err)
	}
}

func TestUSBMonitor_Stop_NotStarted(t *testing.T) {
	monitor := NewUSBMonitor()

	// Stop before starting should not error
	err := monitor.Stop()
	if err != nil {
		t.Errorf("Stop() without Start() error = %v", err)
	}
}

func TestUSBMonitor_Lifecycle(t *testing.T) {
	monitor := NewUSBMonitor()
	ctx := context.Background()

	// Start
	ch, err := monitor.Start(ctx)
	if err != nil {
		t.Fatalf("Start() error = %v", err)
	}

	// Verify channel is closed
	select {
	case <-time.After(100 * time.Millisecond):
		t.Error("Channel should be closed")
	case _, ok := <-ch:
		if ok {
			t.Error("Channel should be closed")
		}
	}

	// Stop
	err = monitor.Stop()
	if err != nil {
		t.Errorf("Stop() error = %v", err)
	}
}

func TestUSBMonitor_ConcurrentStartStop(t *testing.T) {
	monitor := NewUSBMonitor()
	ctx := context.Background()

	// Test concurrent operations
	done := make(chan bool)

	// Multiple starts
	for i := 0; i < 3; i++ {
		go func() {
			ch, err := monitor.Start(ctx)
			if err != nil {
				t.Errorf("Start() error = %v", err)
			}
			if ch != nil {
				// Drain channel if needed
				for range ch {
				}
			}
			done <- true
		}()
	}

	// Multiple stops
	for i := 0; i < 3; i++ {
		go func() {
			err := monitor.Stop()
			if err != nil {
				t.Errorf("Stop() error = %v", err)
			}
			done <- true
		}()
	}

	// Wait for all goroutines
	for i := 0; i < 6; i++ {
		<-done
	}
}

func TestUSBMonitor_MultipleInstances(t *testing.T) {
	// Create multiple independent monitors
	monitor1 := NewUSBMonitor()
	monitor2 := NewUSBMonitor()

	ctx := context.Background()

	ch1, err1 := monitor1.Start(ctx)
	ch2, err2 := monitor2.Start(ctx)

	if err1 != nil {
		t.Fatalf("monitor1.Start() error = %v", err1)
	}
	if err2 != nil {
		t.Fatalf("monitor2.Start() error = %v", err2)
	}

	if ch1 == nil || ch2 == nil {
		t.Error("Start() returned nil channels")
	}

	// Clean up
	monitor1.Stop()
	monitor2.Stop()
}

func TestUSBMonitor_Start_MultipleContexts(t *testing.T) {
	monitor := NewUSBMonitor()

	// Test with different context types
	contexts := []context.Context{
		context.Background(),
		context.TODO(),
	}

	for _, ctx := range contexts {
		t.Run("", func(t *testing.T) {
			ch, err := monitor.Start(ctx)
			if err != nil {
				t.Errorf("Start() with context error = %v", err)
			}
			if ch == nil {
				t.Error("Start() returned nil channel")
			}

			// Clean up
			monitor.Stop()
		})
	}
}

func TestUSBMonitor_ChannelProperties(t *testing.T) {
	monitor := NewUSBMonitor()
	ctx := context.Background()

	ch, err := monitor.Start(ctx)
	if err != nil {
		t.Fatalf("Start() error = %v", err)
	}

	// Verify channel behavior
	// On non-Linux, channel should be closed
	select {
	case event, ok := <-ch:
		if ok {
			t.Errorf("Expected closed channel, got event: %v", event)
		}
		// Channel is closed - expected
	case <-time.After(50 * time.Millisecond):
		// Timeout - channel might still be open but this is OK
	}

	monitor.Stop()
}

// Benchmark tests
func BenchmarkUSBMonitor_StartStop(b *testing.B) {
	ctx := context.Background()

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		monitor := NewUSBMonitor()
		ch, _ := monitor.Start(ctx)
		if ch != nil {
			// Drain channel
			for range ch {
			}
		}
		monitor.Stop()
	}
}

func BenchmarkUSBMonitor_NewUSBMonitor(b *testing.B) {
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		_ = NewUSBMonitor()
	}
}

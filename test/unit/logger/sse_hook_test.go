// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Logger Unit Tests - SSE LogHook

package logger_test

import (
	"sync"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/logger"
)

func TestSetLogHook_CalledOnLog(t *testing.T) {
	var mu sync.Mutex
	var captured *logger.LogEntry
	logger.SetLogHook(func(entry logger.LogEntry) {
		mu.Lock()
		captured = &entry
		mu.Unlock()
	})
	defer logger.SetLogHook(nil)

	logger.InfoC("test-component", "test message for hook")

	// Wait for async goroutine
	time.Sleep(100 * time.Millisecond)

	mu.Lock()
	defer mu.Unlock()
	if captured == nil {
		t.Fatal("logHook was not called")
	}
	if captured.Component != "test-component" {
		t.Errorf("component = %q, want 'test-component'", captured.Component)
	}
	if captured.Message != "test message for hook" {
		t.Errorf("message = %q, want 'test message for hook'", captured.Message)
	}
	if captured.Level != "INFO" {
		t.Errorf("level = %q, want 'INFO'", captured.Level)
	}
}

func TestSetLogHook_Nil(t *testing.T) {
	// Should not panic when hook is nil
	logger.SetLogHook(nil)
	logger.InfoC("test", "this should not panic")
}

func TestSetLogHook_ConcurrentSafety(t *testing.T) {
	var mu sync.Mutex
	count := 0
	logger.SetLogHook(func(entry logger.LogEntry) {
		mu.Lock()
		count++
		mu.Unlock()
	})
	defer logger.SetLogHook(nil)

	var wg sync.WaitGroup
	for i := 0; i < 10; i++ {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			logger.InfoC("concurrent-test", "message")
		}(i)
	}
	wg.Wait()

	// Wait for async goroutines
	time.Sleep(200 * time.Millisecond)

	mu.Lock()
	defer mu.Unlock()
	if count != 10 {
		t.Errorf("hook called %d times, want 10", count)
	}
}

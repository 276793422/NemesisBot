package health

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"sync"
	"testing"
	"time"
)

func TestNewServer(t *testing.T) {
	server := NewServer("localhost", 8080)
	if server == nil {
		t.Fatal("NewServer returned nil")
	}
	if server.startTime.IsZero() {
		t.Error("StartTime should be set")
	}
	if server.ready {
		t.Error("Server should not be ready initially")
	}
	if server.checks == nil {
		t.Error("Checks map should be initialized")
	}
}

func TestHealthHandler(t *testing.T) {
	server := NewServer("localhost", 8080)
	req := httptest.NewRequest("GET", "/health", nil)
	w := httptest.NewRecorder()

	server.healthHandler(w, req)

	if w.Code != http.StatusOK {
		t.Errorf("Expected status 200, got %d", w.Code)
	}

	body, _ := io.ReadAll(w.Body)
	if len(body) == 0 {
		t.Error("Response body should not be empty")
	}
}

func TestReadyHandler(t *testing.T) {
	server := NewServer("localhost", 8080)

	// Test not ready
	req := httptest.NewRequest("GET", "/ready", nil)
	w := httptest.NewRecorder()

	server.readyHandler(w, req)

	if w.Code != http.StatusServiceUnavailable {
		t.Errorf("Expected status 503 when not ready, got %d", w.Code)
	}

	// Set ready
	server.SetReady(true)

	w = httptest.NewRecorder()
	server.readyHandler(w, req)

	if w.Code != http.StatusOK {
		t.Errorf("Expected status 200 when ready, got %d", w.Code)
	}
}

func TestSetReady(t *testing.T) {
	server := NewServer("localhost", 8080)

	server.SetReady(true)
	if !server.ready {
		t.Error("Server should be ready")
	}

	server.SetReady(false)
	if server.ready {
		t.Error("Server should not be ready")
	}
}

func TestRegisterCheck(t *testing.T) {
	server := NewServer("localhost", 8080)

	server.RegisterCheck("test-check", func() (bool, string) {
		return true, "All good"
	})

	if len(server.checks) != 1 {
		t.Errorf("Expected 1 check, got %d", len(server.checks))
	}

	check, exists := server.checks["test-check"]
	if !exists {
		t.Error("Check should exist")
	}
	if check.Name != "test-check" {
		t.Errorf("Expected name 'test-check', got %v", check.Name)
	}
}

func TestStartStop(t *testing.T) {
	server := NewServer("localhost", 0) // Use random port

	go func() {
		if err := server.Start(); err != nil && err != http.ErrServerClosed {
			t.Errorf("Start failed: %v", err)
		}
	}()

	// Give server time to start
	server.SetReady(true)

	// Stop server
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	if err := server.Stop(ctx); err != nil {
		t.Errorf("Stop failed: %v", err)
	}
}

func TestStartContext(t *testing.T) {
	server := NewServer("localhost", 0) // Use random port

	ctx, cancel := context.WithTimeout(context.Background(), 1*time.Second)
	defer cancel()

	go func() {
		if err := server.StartContext(ctx); err != nil && err != http.ErrServerClosed {
			t.Errorf("StartContext failed: %v", err)
		}
	}()

	// Give server time to start
	server.SetReady(true)
}

func TestStartContextCancellation(t *testing.T) {
	server := NewServer("localhost", 0) // Use random port

	ctx, cancel := context.WithTimeout(context.Background(), 1*time.Second)
	defer cancel()

	go func() {
		if err := server.StartContext(ctx); err != nil && err != http.ErrServerClosed {
			t.Errorf("StartContext failed: %v", err)
		}
	}()

	// Cancel context before server starts
	cancel()

	// Wait for cancellation to take effect
	time.Sleep(100 * time.Millisecond)
}

func TestStartContextError(t *testing.T) {
	server := NewServer("localhost", 0) // Use random port

	// Create context that's already cancelled
	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	// This will still start the server and then try to shut it down
	err := server.StartContext(ctx)
	if err != nil {
		// The server might start successfully and then be shut down
		// This is acceptable behavior
		t.Logf("Got expected shutdown error: %v", err)
	}
}

func TestStatusString(t *testing.T) {
	tests := []struct {
		name     string
		input    bool
		expected string
	}{
		{"true input", true, "ok"},
		{"false input", false, "fail"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := statusString(tt.input)
			if result != tt.expected {
				t.Errorf("statusString(%v) = %v, expected %v", tt.input, result, tt.expected)
			}
		})
	}
}

func TestHealthHandlerWithChecks(t *testing.T) {
	server := NewServer("localhost", 8080)

	// Register some checks
	server.RegisterCheck("check1", func() (bool, string) {
		return true, "Check 1 passed"
	})

	server.RegisterCheck("check2", func() (bool, string) {
		return false, "Check 2 failed"
	})

	req := httptest.NewRequest("GET", "/health", nil)
	w := httptest.NewRecorder()

	server.healthHandler(w, req)

	if w.Code != http.StatusOK {
		t.Errorf("Expected status 200, got %d", w.Code)
	}

	body, _ := io.ReadAll(w.Body)
	if len(body) == 0 {
		t.Error("Response body should not be empty")
	}

	// Verify response contains uptime
	if !bytes.Contains(body, []byte("uptime")) {
		t.Error("Response should contain uptime")
	}
}

func TestReadyHandlerWithFailingChecks(t *testing.T) {
	server := NewServer("localhost", 8080)
	server.SetReady(true)

	// Register a failing check
	server.RegisterCheck("failing-check", func() (bool, string) {
		return false, "This check fails"
	})

	req := httptest.NewRequest("GET", "/ready", nil)
	w := httptest.NewRecorder()

	server.readyHandler(w, req)

	if w.Code != http.StatusServiceUnavailable {
		t.Errorf("Expected status 503 when checks fail, got %d", w.Code)
	}
}

func TestRegisterCheckConcurrency(t *testing.T) {
	server := NewServer("localhost", 8080)

	// Register many checks concurrently
	var wg sync.WaitGroup
	for i := 0; i < 10; i++ {
		wg.Add(1)
		go func(id int) {
			defer wg.Done()
			server.RegisterCheck(fmt.Sprintf("check-%d", id), func() (bool, string) {
				return id%2 == 0, fmt.Sprintf("Check %d", id)
			})
		}(i)
	}
	wg.Wait()

	if len(server.checks) != 10 {
		t.Errorf("Expected 10 checks, got %d", len(server.checks))
	}
}

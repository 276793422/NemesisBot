// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package utils

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"
)

// TestShouldRetry tests the shouldRetry function
func TestShouldRetry(t *testing.T) {
	tests := []struct {
		name       string
		statusCode int
		want       bool
	}{
		{
			name:       "200 OK",
			statusCode: http.StatusOK,
			want:       false,
		},
		{
			name:       "201 Created",
			statusCode: http.StatusCreated,
			want:       false,
		},
		{
			name:       "204 No Content",
			statusCode: http.StatusNoContent,
			want:       false,
		},
		{
			name:       "400 Bad Request",
			statusCode: http.StatusBadRequest,
			want:       false,
		},
		{
			name:       "401 Unauthorized",
			statusCode: http.StatusUnauthorized,
			want:       false,
		},
		{
			name:       "403 Forbidden",
			statusCode: http.StatusForbidden,
			want:       false,
		},
		{
			name:       "404 Not Found",
			statusCode: http.StatusNotFound,
			want:       false,
		},
		{
			name:       "429 Too Many Requests",
			statusCode: http.StatusTooManyRequests,
			want:       true,
		},
		{
			name:       "500 Internal Server Error",
			statusCode: http.StatusInternalServerError,
			want:       true,
		},
		{
			name:       "502 Bad Gateway",
			statusCode: http.StatusBadGateway,
			want:       true,
		},
		{
			name:       "503 Service Unavailable",
			statusCode: http.StatusServiceUnavailable,
			want:       true,
		},
		{
			name:       "504 Gateway Timeout",
			statusCode: http.StatusGatewayTimeout,
			want:       true,
		},
		{
			name:       "418 I'm a teapot",
			statusCode: 418,
			want:       false,
		},
		{
			name:       "599 Unknown error",
			statusCode: 599,
			want:       true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := shouldRetry(tt.statusCode)
			if got != tt.want {
				t.Errorf("shouldRetry(%d) = %v, want %v", tt.statusCode, got, tt.want)
			}
		})
	}
}

// TestDoRequestWithRetry_Success tests successful request without retries
func TestDoRequestWithRetry_Success(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
		w.Write([]byte("success"))
	}))
	defer server.Close()

	client := &http.Client{}
	req, err := http.NewRequest("GET", server.URL, nil)
	if err != nil {
		t.Fatalf("Failed to create request: %v", err)
	}

	resp, err := DoRequestWithRetry(client, req)
	if err != nil {
		t.Fatalf("DoRequestWithRetry failed: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		t.Errorf("Expected status 200, got %d", resp.StatusCode)
	}
}

// TestDoRequestWithRetry_RetryOn500 tests retry on 500 errors
func TestDoRequestWithRetry_RetryOn500(t *testing.T) {
	attempts := 0
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		attempts++
		if attempts < 3 {
			w.WriteHeader(http.StatusInternalServerError)
			return
		}
		w.WriteHeader(http.StatusOK)
		w.Write([]byte("success after retries"))
	}))
	defer server.Close()

	client := &http.Client{}
	req, err := http.NewRequest("GET", server.URL, nil)
	if err != nil {
		t.Fatalf("Failed to create request: %v", err)
	}

	resp, err := DoRequestWithRetry(client, req)
	if err != nil {
		t.Fatalf("DoRequestWithRetry failed: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		t.Errorf("Expected status 200, got %d", resp.StatusCode)
	}

	if attempts != 3 {
		t.Errorf("Expected 3 attempts, got %d", attempts)
	}
}

// TestDoRequestWithRetry_RetryOn429 tests retry on 429 errors
func TestDoRequestWithRetry_RetryOn429(t *testing.T) {
	attempts := 0
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		attempts++
		if attempts < 2 {
			w.Header().Set("Retry-After", "1")
			w.WriteHeader(http.StatusTooManyRequests)
			return
		}
		w.WriteHeader(http.StatusOK)
		w.Write([]byte("success after rate limit"))
	}))
	defer server.Close()

	client := &http.Client{}
	req, err := http.NewRequest("GET", server.URL, nil)
	if err != nil {
		t.Fatalf("Failed to create request: %v", err)
	}

	resp, err := DoRequestWithRetry(client, req)
	if err != nil {
		t.Fatalf("DoRequestWithRetry failed: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		t.Errorf("Expected status 200, got %d", resp.StatusCode)
	}

	if attempts != 2 {
		t.Errorf("Expected 2 attempts, got %d", attempts)
	}
}

// TestDoRequestWithRetry_MaxRetriesExceeded tests max retries
func TestDoRequestWithRetry_MaxRetriesExceeded(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusInternalServerError)
	}))
	defer server.Close()

	client := &http.Client{}
	req, err := http.NewRequest("GET", server.URL, nil)
	if err != nil {
		t.Fatalf("Failed to create request: %v", err)
	}

	resp, err := DoRequestWithRetry(client, req)
	if err != nil {
		t.Fatalf("DoRequestWithRetry failed: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusInternalServerError {
		t.Errorf("Expected status 500 after max retries, got %d", resp.StatusCode)
	}
}

// TestDoRequestWithRetry_NoRetryOn4xx tests no retry on 4xx errors (except 429)
func TestDoRequestWithRetry_NoRetryOn4xx(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusBadRequest)
	}))
	defer server.Close()

	client := &http.Client{}
	req, err := http.NewRequest("GET", server.URL, nil)
	if err != nil {
		t.Fatalf("Failed to create request: %v", err)
	}

	resp, err := DoRequestWithRetry(client, req)
	if err != nil {
		t.Fatalf("DoRequestWithRetry failed: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusBadRequest {
		t.Errorf("Expected status 400, got %d", resp.StatusCode)
	}
}

// TestDoRequestWithRetry_ContextCancellation tests context cancellation
func TestDoRequestWithRetry_ContextCancellation(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusInternalServerError)
	}))
	defer server.Close()

	client := &http.Client{}
	ctx, cancel := context.WithCancel(context.Background())

	// Cancel immediately
	cancel()

	req, err := http.NewRequestWithContext(ctx, "GET", server.URL, nil)
	if err != nil {
		t.Fatalf("Failed to create request: %v", err)
	}

	// The function should return an error due to context cancellation
	_, err = DoRequestWithRetry(client, req)
	if err == nil {
		t.Error("Expected error due to context cancellation, got nil")
	}
	if err != nil && !errors.Is(err, context.Canceled) {
		t.Logf("Got error (may be context.Canceled): %v", err)
	}
}

// TestDoRequestWithRetry_ContextTimeout tests context timeout
func TestDoRequestWithRetry_ContextTimeout(t *testing.T) {
	attempts := 0
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		attempts++
		w.WriteHeader(http.StatusInternalServerError)
	}))
	defer server.Close()

	client := &http.Client{}
	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()

	req, err := http.NewRequestWithContext(ctx, "GET", server.URL, nil)
	if err != nil {
		t.Fatalf("Failed to create request: %v", err)
	}

	// The function should return an error due to timeout
	_, err = DoRequestWithRetry(client, req)
	if err == nil {
		t.Error("Expected error due to timeout, got nil")
	}
	if err != nil && !errors.Is(err, context.DeadlineExceeded) {
		t.Logf("Got error (may be timeout related): %v", err)
	}
}

// TestDoRequestWithRetry_NetworkError tests retry on network errors
func TestDoRequestWithRetry_NetworkError(t *testing.T) {
	// Use an invalid URL that will cause a network error
	client := &http.Client{}
	req, err := http.NewRequest("GET", "http://invalid-url-that-does-not-exist.local", nil)
	if err != nil {
		t.Fatalf("Failed to create request: %v", err)
	}

	// Should retry and eventually fail
	resp, err := DoRequestWithRetry(client, req)
	if err == nil {
		resp.Body.Close()
		t.Error("Expected error for invalid URL, got nil")
	}
}

// TestSleepWithCtx tests the sleepWithCtx function
func TestSleepWithCtx(t *testing.T) {
	tests := []struct {
		name      string
		duration  time.Duration
		cancelCtx bool
		wantErr   bool
	}{
		{
			name:      "normal sleep",
			duration:  10 * time.Millisecond,
			cancelCtx: false,
			wantErr:   false,
		},
		{
			name:      "cancelled context",
			duration:  100 * time.Millisecond,
			cancelCtx: true,
			wantErr:   true,
		},
		{
			name:      "zero duration",
			duration:  0,
			cancelCtx: false,
			wantErr:   false,
		},
		{
			name:      "short duration",
			duration:  1 * time.Millisecond,
			cancelCtx: false,
			wantErr:   false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			ctx := context.Background()
			if tt.cancelCtx {
				var cancel context.CancelFunc
				ctx, cancel = context.WithCancel(ctx)
				go func() {
					time.Sleep(10 * time.Millisecond)
					cancel()
				}()
			}

			start := time.Now()
			err := sleepWithCtx(ctx, tt.duration)
			elapsed := time.Since(start)

			if (err != nil) != tt.wantErr {
				t.Errorf("sleepWithCtx() error = %v, wantErr %v", err, tt.wantErr)
			}

			if !tt.cancelCtx && elapsed < tt.duration {
				t.Errorf("sleepWithCtx() returned early: elapsed %v, duration %v", elapsed, tt.duration)
			}
		})
	}
}

// TestDoRequestWithRetry_ExponentialBackoff tests exponential backoff timing
func TestDoRequestWithRetry_ExponentialBackoff(t *testing.T) {
	var attemptTimes []time.Time
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		attemptTimes = append(attemptTimes, time.Now())
		w.WriteHeader(http.StatusInternalServerError)
	}))
	defer server.Close()

	client := &http.Client{Timeout: 100 * time.Millisecond}
	req, err := http.NewRequest("GET", server.URL, nil)
	if err != nil {
		t.Fatalf("Failed to create request: %v", err)
	}

	_, err = DoRequestWithRetry(client, req)
	if err != nil {
		t.Logf("Request failed (expected): %v", err)
	}

	// Verify we made multiple attempts with delays
	if len(attemptTimes) < 2 {
		t.Errorf("Expected at least 2 attempts, got %d", len(attemptTimes))
		return
	}

	// Check that there are delays between attempts (exponential backoff)
	for i := 1; i < len(attemptTimes); i++ {
		delay := attemptTimes[i].Sub(attemptTimes[i-1])
		expectedMin := time.Duration(i) * retryDelayUnit
		if delay < expectedMin {
			t.Errorf("Attempt %d delay %v is less than expected minimum %v", i, delay, expectedMin)
		}
	}
}

// TestDoRequestWithRetry_POSTRequest tests retry with POST request
func TestDoRequestWithRetry_POSTRequest(t *testing.T) {
	attempts := 0
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		attempts++
		if r.Method != http.MethodPost {
			t.Errorf("Expected POST request, got %s", r.Method)
		}
		if attempts < 2 {
			w.WriteHeader(http.StatusInternalServerError)
			return
		}
		w.WriteHeader(http.StatusCreated)
	}))
	defer server.Close()

	client := &http.Client{}
	req, err := http.NewRequest("POST", server.URL, nil)
	if err != nil {
		t.Fatalf("Failed to create request: %v", err)
	}

	resp, err := DoRequestWithRetry(client, req)
	if err != nil {
		t.Fatalf("DoRequestWithRetry failed: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusCreated {
		t.Errorf("Expected status 201, got %d", resp.StatusCode)
	}
}

// TestDoRequestWithRetry_WithHeaders tests retry preserves headers
func TestDoRequestWithRetry_WithHeaders(t *testing.T) {
	attempts := 0
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		attempts++
		auth := r.Header.Get("Authorization")
		if auth != "Bearer token123" {
			t.Errorf("Expected Authorization header, got %q", auth)
		}
		if attempts < 2 {
			w.WriteHeader(http.StatusInternalServerError)
			return
		}
		w.WriteHeader(http.StatusOK)
	}))
	defer server.Close()

	client := &http.Client{}
	req, err := http.NewRequest("GET", server.URL, nil)
	if err != nil {
		t.Fatalf("Failed to create request: %v", err)
	}
	req.Header.Set("Authorization", "Bearer token123")

	resp, err := DoRequestWithRetry(client, req)
	if err != nil {
		t.Fatalf("DoRequestWithRetry failed: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		t.Errorf("Expected status 200, got %d", resp.StatusCode)
	}
}

// TestDoRequestWithRetry_IntermittentSuccess tests success after intermittent failures
func TestDoRequestWithRetry_IntermittentSuccess(t *testing.T) {
	attempts := 0
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		attempts++
		// Fail on attempts 1 and 3, succeed on 2
		if attempts == 1 || attempts == 3 {
			w.WriteHeader(http.StatusInternalServerError)
			return
		}
		if attempts == 2 {
			w.WriteHeader(http.StatusOK)
			w.Write([]byte("success"))
			return
		}
		w.WriteHeader(http.StatusInternalServerError)
	}))
	defer server.Close()

	client := &http.Client{}
	req, err := http.NewRequest("GET", server.URL, nil)
	if err != nil {
		t.Fatalf("Failed to create request: %v", err)
	}

	resp, err := DoRequestWithRetry(client, req)
	if err != nil {
		t.Fatalf("DoRequestWithRetry failed: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		t.Errorf("Expected status 200, got %d", resp.StatusCode)
	}

	if attempts != 2 {
		t.Errorf("Expected 2 attempts, got %d", attempts)
	}
}

// TestDoRequestWithRetry_Multiple5xxErrors tests retry on different 5xx errors
func TestDoRequestWithRetry_Multiple5xxErrors(t *testing.T) {
	errorCodes := []int{
		http.StatusInternalServerError,
		http.StatusBadGateway,
		http.StatusServiceUnavailable,
		http.StatusGatewayTimeout,
	}

	for _, code := range errorCodes {
		t.Run(http.StatusText(code), func(t *testing.T) {
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				w.WriteHeader(code)
			}))
			defer server.Close()

			client := &http.Client{}
			req, err := http.NewRequest("GET", server.URL, nil)
			if err != nil {
				t.Fatalf("Failed to create request: %v", err)
			}

			resp, err := DoRequestWithRetry(client, req)
			if err != nil {
				t.Fatalf("DoRequestWithRetry failed: %v", err)
			}
			defer resp.Body.Close()

			if resp.StatusCode != code {
				t.Errorf("Expected status %d, got %d", code, resp.StatusCode)
			}
		})
	}
}

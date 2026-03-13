// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package providers

import (
	"context"
	"errors"
	"strings"
	"testing"
)

// TestClassifyError_NilError tests classifying a nil error
func TestClassifyError_NilError(t *testing.T) {
	result := ClassifyError(nil, "test-provider", "test-model")
	if result != nil {
		t.Error("expected nil for nil error")
	}
}

// TestClassifyError_ContextCanceled tests context cancellation
func TestClassifyError_ContextCanceled(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	err := ctx.Err()

	result := ClassifyError(err, "test-provider", "test-model")
	if result != nil {
		t.Error("expected nil for context.Canceled (user abort)")
	}
}

// TestClassifyError_ContextDeadlineExceeded tests context deadline exceeded
func TestClassifyError_ContextDeadlineExceeded(t *testing.T) {
	err := context.DeadlineExceeded
	result := ClassifyError(err, "test-provider", "test-model")

	if result == nil {
		t.Fatal("expected non-nil result for deadline exceeded")
	}

	if result.Reason != FailoverTimeout {
		t.Errorf("expected reason FailoverTimeout, got %v", result.Reason)
	}

	if result.Provider != "test-provider" {
		t.Errorf("expected provider 'test-provider', got '%s'", result.Provider)
	}

	if result.Model != "test-model" {
		t.Errorf("expected model 'test-model', got '%s'", result.Model)
	}

	if result.Wrapped != err {
		t.Error("wrapped error should match original error")
	}
}

// TestClassifyError_RateLimitPatterns tests rate limit error patterns
func TestClassifyError_RateLimitPatterns(t *testing.T) {
	tests := []struct {
		name string
		err  error
	}{
		{"rate limit", errors.New("rate limit exceeded")},
		{"rate_limit", errors.New("rate_limit exceeded")},
		{"too many requests", errors.New("too many requests")},
		{"429", errors.New("HTTP 429")},
		{"quota exceeded", errors.New("exceeded your current quota")},
		{"resource exhausted", errors.New("resource has been exhausted")},
		{"resource_exhausted", errors.New("resource_exhausted error")},
		{"usage limit", errors.New("usage limit reached")},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := ClassifyError(tt.err, "test-provider", "test-model")
			if result == nil {
				t.Fatal("expected non-nil result")
			}

			if result.Reason != FailoverRateLimit {
				t.Errorf("expected reason FailoverRateLimit, got %v", result.Reason)
			}

			if !result.IsRetriable() {
				t.Error("rate limit errors should be retriable")
			}
		})
	}
}

// TestClassifyError_OverloadedPatterns tests overloaded error patterns
func TestClassifyError_OverloadedPatterns(t *testing.T) {
	tests := []struct {
		name string
		err  error
	}{
		{"overloaded_error", errors.New(`"type": "overloaded_error"`)},
		{"overloaded", errors.New("service is overloaded")},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := ClassifyError(tt.err, "test-provider", "test-model")
			if result == nil {
				t.Fatal("expected non-nil result")
			}

			if result.Reason != FailoverRateLimit {
				t.Errorf("expected reason FailoverRateLimit (overloaded treated as rate_limit), got %v", result.Reason)
			}
		})
	}
}

// TestClassifyError_TimeoutPatterns tests timeout error patterns
func TestClassifyError_TimeoutPatterns(t *testing.T) {
	tests := []struct {
		name string
		err  error
	}{
		{"timeout", errors.New("request timeout")},
		{"timed out", errors.New("connection timed out")},
		{"deadline exceeded", errors.New("context deadline exceeded")},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := ClassifyError(tt.err, "test-provider", "test-model")
			if result == nil {
				t.Fatal("expected non-nil result")
			}

			if result.Reason != FailoverTimeout {
				t.Errorf("expected reason FailoverTimeout, got %v", result.Reason)
			}

			if !result.IsRetriable() {
				t.Error("timeout errors should be retriable")
			}
		})
	}
}

// TestClassifyError_BillingPatterns tests billing error patterns
func TestClassifyError_BillingPatterns(t *testing.T) {
	tests := []struct {
		name string
		err  error
	}{
		{"402", errors.New("HTTP 402 Payment Required")},
		{"payment required", errors.New("payment required")},
		{"insufficient credits", errors.New("insufficient credits")},
		{"credit balance", errors.New("credit balance is low")},
		{"plans & billing", errors.New("please check plans & billing")},
		{"insufficient balance", errors.New("insufficient balance")},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := ClassifyError(tt.err, "test-provider", "test-model")
			if result == nil {
				t.Fatal("expected non-nil result")
			}

			if result.Reason != FailoverBilling {
				t.Errorf("expected reason FailoverBilling, got %v", result.Reason)
			}

			if !result.IsRetriable() {
				t.Error("billing errors should be retriable")
			}
		})
	}
}

// TestClassifyError_AuthPatterns tests authentication error patterns
func TestClassifyError_AuthPatterns(t *testing.T) {
	tests := []struct {
		name string
		err  error
	}{
		{"invalid api key", errors.New("invalid api key")},
		{"invalid_api_key", errors.New("invalid_api_key provided")},
		{"incorrect api key", errors.New("incorrect api key")},
		{"invalid token", errors.New("invalid token")},
		{"authentication", errors.New("authentication failed")},
		{"re-authenticate", errors.New("please re-authenticate")},
		{"oauth token refresh failed", errors.New("oauth token refresh failed")},
		{"unauthorized", errors.New("unauthorized access")},
		{"forbidden", errors.New("forbidden")},
		{"access denied", errors.New("access denied")},
		{"expired", errors.New("token has expired")},
		{"401", errors.New("HTTP 401 Unauthorized")},
		{"403", errors.New("HTTP 403 Forbidden")},
		{"no credentials found", errors.New("no credentials found")},
		{"no api key found", errors.New("no api key found")},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := ClassifyError(tt.err, "test-provider", "test-model")
			if result == nil {
				t.Fatal("expected non-nil result")
			}

			if result.Reason != FailoverAuth {
				t.Errorf("expected reason FailoverAuth, got %v", result.Reason)
			}

			if !result.IsRetriable() {
				t.Error("auth errors should be retriable")
			}
		})
	}
}

// TestClassifyError_FormatPatterns tests format error patterns (non-retriable)
func TestClassifyError_FormatPatterns(t *testing.T) {
	tests := []struct {
		name string
		err  error
	}{
		{"string should match pattern", errors.New("string should match pattern")},
		{"tool_use.id", errors.New("tool_use.id is required")},
		{"tool_use_id", errors.New("tool_use_id error")},
		{"messages.1.content.1.tool_use.id", errors.New("messages.1.content.1.tool_use.id invalid")},
		{"invalid request format", errors.New("invalid request format")},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := ClassifyError(tt.err, "test-provider", "test-model")
			if result == nil {
				t.Fatal("expected non-nil result")
			}

			if result.Reason != FailoverFormat {
				t.Errorf("expected reason FailoverFormat, got %v", result.Reason)
			}

			if result.IsRetriable() {
				t.Error("format errors should not be retriable")
			}
		})
	}
}

// TestClassifyError_ImageDimensionError tests image dimension error patterns
func TestClassifyError_ImageDimensionError(t *testing.T) {
	err := errors.New("image dimensions exceed max size")
	result := ClassifyError(err, "test-provider", "test-model")

	if result == nil {
		t.Fatal("expected non-nil result")
	}

	if result.Reason != FailoverFormat {
		t.Errorf("expected reason FailoverFormat, got %v", result.Reason)
	}

	if result.IsRetriable() {
		t.Error("image dimension errors should not be retriable")
	}
}

// TestClassifyError_ImageSizeError tests image size error patterns
func TestClassifyError_ImageSizeError(t *testing.T) {
	err := errors.New("image exceeds 10MB limit")
	result := ClassifyError(err, "test-provider", "test-model")

	if result == nil {
		t.Fatal("expected non-nil result")
	}

	if result.Reason != FailoverFormat {
		t.Errorf("expected reason FailoverFormat, got %v", result.Reason)
	}

	if result.IsRetriable() {
		t.Error("image size errors should not be retriable")
	}
}

// TestClassifyError_HTTPStatusCodes tests HTTP status code classification
func TestClassifyError_HTTPStatusCodes(t *testing.T) {
	tests := []struct {
		name           string
		err            error
		expectedReason FailoverReason
		expectedStatus int
	}{
		{"401", errors.New("status: 401"), FailoverAuth, 401},
		{"403", errors.New("status 403"), FailoverAuth, 403},
		{"408", errors.New("status 408"), FailoverTimeout, 408},
		{"429", errors.New("status: 429"), FailoverRateLimit, 429},
		{"500", errors.New("status: 500"), FailoverTimeout, 500},
		{"502", errors.New("status 502"), FailoverTimeout, 502},
		{"521", errors.New("status 521"), FailoverTimeout, 521},
		{"522", errors.New("status 522"), FailoverTimeout, 522},
		{"523", errors.New("status 523"), FailoverTimeout, 523},
		{"524", errors.New("status 524"), FailoverTimeout, 524},
		{"529", errors.New("status 529"), FailoverTimeout, 529},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := ClassifyError(tt.err, "test-provider", "test-model")
			if result == nil {
				t.Fatal("expected non-nil result")
			}

			if result.Reason != tt.expectedReason {
				t.Errorf("expected reason %v, got %v", tt.expectedReason, result.Reason)
			}

			if result.Status != tt.expectedStatus {
				t.Errorf("expected status %d, got %d", tt.expectedStatus, result.Status)
			}
		})
	}
}

// TestClassifyError_UnknownError tests unknown errors (not classified)
func TestClassifyError_UnknownError(t *testing.T) {
	err := errors.New("some unknown error that doesn't match any pattern")
	result := ClassifyError(err, "test-provider", "test-model")

	if result != nil {
		t.Error("expected nil for unknown error (should not trigger fallback)")
	}
}

// TestClassifyError_PriorityOrder tests that status codes take priority over message patterns
func TestClassifyError_PriorityOrder(t *testing.T) {
	// Error has both status code and message pattern
	// Status code should take priority
	err := errors.New("status: 429 - timeout error")
	result := ClassifyError(err, "test-provider", "test-model")

	if result == nil {
		t.Fatal("expected non-nil result")
	}

	// Should classify by status code (429 = rate_limit) not by message (timeout)
	if result.Reason != FailoverRateLimit {
		t.Errorf("expected reason FailoverRateLimit (from status code), got %v", result.Reason)
	}

	if result.Status != 429 {
		t.Errorf("expected status 429, got %d", result.Status)
	}
}

// TestClassifyError_MessagePatternPriority tests message pattern priority order
func TestClassifyError_MessagePatternPriority(t *testing.T) {
	tests := []struct {
		name           string
		err            error
		expectedReason FailoverReason
	}{
		// Rate limit patterns have highest priority
		{"rate limit before auth", errors.New("rate limit - unauthorized"), FailoverRateLimit},
		{"overloaded before timeout", errors.New("overloaded - timeout"), FailoverRateLimit},
		{"billing before timeout", errors.New("payment required - timeout"), FailoverBilling},
		{"timeout before auth", errors.New("timeout - unauthorized"), FailoverTimeout},
		{"auth before format", errors.New("authentication - invalid format"), FailoverAuth},
		{"format error", errors.New("string should match pattern"), FailoverFormat},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := ClassifyError(tt.err, "test-provider", "test-model")
			if result == nil {
				t.Fatal("expected non-nil result")
			}

			if result.Reason != tt.expectedReason {
				t.Errorf("expected reason %v, got %v", tt.expectedReason, result.Reason)
			}
		})
	}
}

// TestClassifyError_CaseInsensitive tests case-insensitive pattern matching
func TestClassifyError_CaseInsensitive(t *testing.T) {
	tests := []struct {
		name           string
		err            error
		expectedReason FailoverReason
	}{
		{"RATE LIMIT", errors.New("RATE LIMIT EXCEEDED"), FailoverRateLimit},
		{"Rate_Limit", errors.New("Rate_Limit Error"), FailoverRateLimit},
		{"TIMEOUT", errors.New("CONNECTION TIMEOUT"), FailoverTimeout},
		{"Unauthorized", errors.New("Unauthorized Access"), FailoverAuth},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := ClassifyError(tt.err, "test-provider", "test-model")
			if result == nil {
				t.Fatal("expected non-nil result")
			}

			if result.Reason != tt.expectedReason {
				t.Errorf("expected reason %v, got %v", tt.expectedReason, result.Reason)
			}
		})
	}
}

// TestExtractHTTPStatus tests HTTP status code extraction
func TestExtractHTTPStatus(t *testing.T) {
	tests := []struct {
		name     string
		msg      string
		expected int
	}{
		{"status: 429", "status: 429", 429},
		{"status 401", "status 401", 401},
		{"no status", "some error message", 0},
		{"invalid status", "status: abc", 0},
		{"multiple status codes", "status: 401 and 403", 401}, // First match
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := extractHTTPStatus(strings.ToLower(tt.msg))
			if result != tt.expected {
				t.Errorf("expected status %d, got %d", tt.expected, result)
			}
		})
	}
}

// TestIsImageDimensionError tests image dimension error detection
func TestIsImageDimensionError(t *testing.T) {
	tests := []struct {
		name     string
		msg      string
		expected bool
	}{
		{"dimension error", "image dimensions exceed max size", true},
		{"not dimension error", "image size too large", false},
		{"empty message", "", false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := IsImageDimensionError(strings.ToLower(tt.msg))
			if result != tt.expected {
				t.Errorf("expected %v, got %v", tt.expected, result)
			}
		})
	}
}

// TestIsImageSizeError tests image size error detection
func TestIsImageSizeError(t *testing.T) {
	tests := []struct {
		name     string
		msg      string
		expected bool
	}{
		{"size error", "image exceeds 10MB limit", true},
		{"size error with number", "image exceeds 5MB", true},
		{"not size error", "image dimensions too large", false},
		{"empty message", "", false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := IsImageSizeError(strings.ToLower(tt.msg))
			if result != tt.expected {
				t.Errorf("expected %v, got %v", tt.expected, result)
			}
		})
	}
}

// TestClassifyByStatus tests status code classification
func TestClassifyByStatus(t *testing.T) {
	tests := []struct {
		status   int
		expected FailoverReason
	}{
		{401, FailoverAuth},
		{403, FailoverAuth},
		{402, FailoverBilling},
		{408, FailoverTimeout},
		{429, FailoverRateLimit},
		{400, FailoverFormat},
		{500, FailoverTimeout},
		{502, FailoverTimeout},
		{503, FailoverTimeout},
		{521, FailoverTimeout},
		{522, FailoverTimeout},
		{523, FailoverTimeout},
		{524, FailoverTimeout},
		{529, FailoverTimeout},
		{200, ""},
		{204, ""},
		{404, ""},
	}

	for _, tt := range tests {
		t.Run(string(rune(tt.status)), func(t *testing.T) {
			result := classifyByStatus(tt.status)
			if result != tt.expected {
				t.Errorf("status %d: expected reason %v, got %v", tt.status, tt.expected, result)
			}
		})
	}
}

// TestClassifyByMessage tests message pattern classification
func TestClassifyByMessage(t *testing.T) {
	tests := []struct {
		name           string
		msg            string
		expectedReason FailoverReason
	}{
		{"rate limit", "rate limit exceeded", FailoverRateLimit},
		{"overloaded", "service overloaded", FailoverRateLimit},
		{"billing", "payment required", FailoverBilling},
		{"timeout", "request timeout", FailoverTimeout},
		{"auth", "unauthorized", FailoverAuth},
		{"format", "string should match pattern", FailoverFormat},
		{"unknown", "some random error", ""},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := classifyByMessage(strings.ToLower(tt.msg))
			if result != tt.expectedReason {
				t.Errorf("expected reason %v, got %v", tt.expectedReason, result)
			}
		})
	}
}

// TestMatchesAny tests pattern matching
func TestMatchesAny(t *testing.T) {
	patterns := []errorPattern{
		substr("rate limit"),
		rxp(`\b401\b`),
	}

	tests := []struct {
		name     string
		msg      string
		expected bool
	}{
		{"substring match", "rate limit exceeded", true},
		{"regex match", "HTTP 401 Unauthorized", true},
		{"no match", "some other error", false},
		{"case insensitive substring", "RATE LIMIT", true},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := matchesAny(strings.ToLower(tt.msg), patterns)
			if result != tt.expected {
				t.Errorf("expected %v, got %v", tt.expected, result)
			}
		})
	}
}

// TestParseDigits tests digit parsing
func TestParseDigits(t *testing.T) {
	tests := []struct {
		input    string
		expected int
	}{
		{"123", 123},
		{"401", 401},
		{"0", 0},
		{"abc", 0},
		{"12abc34", 1234},
		{"", 0},
	}

	for _, tt := range tests {
		t.Run(tt.input, func(t *testing.T) {
			result := parseDigits(tt.input)
			if result != tt.expected {
				t.Errorf("expected %d, got %d", tt.expected, result)
			}
		})
	}
}

// TestFailoverError_ErrorUnwrap tests error unwrapping
func TestFailoverError_ErrorUnwrap(t *testing.T) {
	originalErr := errors.New("original error")
	failErr := &FailoverError{
		Reason:   FailoverAuth,
		Provider: "test-provider",
		Model:    "test-model",
		Status:   401,
		Wrapped:  originalErr,
	}

	unwrapped := failErr.Unwrap()
	if unwrapped != originalErr {
		t.Error("unwrapped error should match original error")
	}

	// Test with nil wrapped error
	failErr.Wrapped = nil
	unwrapped = failErr.Unwrap()
	if unwrapped != nil {
		t.Error("unwrapped should be nil when wrapped error is nil")
	}
}

// TestFailoverError_ErrorString tests error string formatting
func TestFailoverError_ErrorString(t *testing.T) {
	originalErr := errors.New("original error")
	failErr := &FailoverError{
		Reason:   FailoverAuth,
		Provider: "anthropic",
		Model:    "claude-3-5",
		Status:   401,
		Wrapped:  originalErr,
	}

	errStr := failErr.Error()

	// Check that error string contains all key components
	requiredStrings := []string{
		"failover",
		string(FailoverAuth),
		"anthropic",
		"claude-3-5",
		"401",
		"original error",
	}

	for _, s := range requiredStrings {
		if !strings.Contains(errStr, s) {
			t.Errorf("error string should contain '%s': %s", s, errStr)
		}
	}
}

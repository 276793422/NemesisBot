// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package providers

import (
	"context"
	"errors"
	"strings"
	"testing"
	"time"
)

// TestNewFallbackChain tests creating a new fallback chain
func TestNewFallbackChain(t *testing.T) {
	ct := NewCooldownTracker()
	fc := NewFallbackChain(ct)

	if fc == nil {
		t.Fatal("NewFallbackChain() should not return nil")
	}

	if fc.cooldown != ct {
		t.Error("fallback chain should have the provided cooldown tracker")
	}
}

// TestFallbackChain_Execute_NoCandidates tests executing with no candidates
func TestFallbackChain_Execute_NoCandidates(t *testing.T) {
	ct := NewCooldownTracker()
	fc := NewFallbackChain(ct)

	ctx := context.Background()
	result, err := fc.Execute(ctx, nil, func(ctx context.Context, provider, model string) (*LLMResponse, error) {
		return nil, nil
	})

	if err == nil {
		t.Error("expected error when no candidates provided")
	}

	if !strings.Contains(err.Error(), "no candidates") {
		t.Errorf("expected error about no candidates, got: %v", err)
	}

	if result != nil {
		t.Error("result should be nil when error occurs")
	}
}

// TestFallbackChain_Execute_EmptyCandidates tests executing with empty candidates slice
func TestFallbackChain_Execute_EmptyCandidates(t *testing.T) {
	ct := NewCooldownTracker()
	fc := NewFallbackChain(ct)

	ctx := context.Background()
	candidates := []FallbackCandidate{}

	result, err := fc.Execute(ctx, candidates, func(ctx context.Context, provider, model string) (*LLMResponse, error) {
		return nil, nil
	})

	if err == nil {
		t.Error("expected error when empty candidates provided")
	}

	if result != nil {
		t.Error("result should be nil when error occurs")
	}
}

// TestFallbackChain_Execute_Success tests successful execution on first try
func TestFallbackChain_Execute_Success(t *testing.T) {
	ct := NewCooldownTracker()
	fc := NewFallbackChain(ct)

	ctx := context.Background()
	candidates := []FallbackCandidate{
		{Provider: "provider1", Model: "model1"},
	}

	expectedResponse := &LLMResponse{Content: "success"}

	runFunc := func(ctx context.Context, provider, model string) (*LLMResponse, error) {
		return expectedResponse, nil
	}

	result, err := fc.Execute(ctx, candidates, runFunc)

	if err != nil {
		t.Errorf("unexpected error: %v", err)
	}

	if result == nil {
		t.Fatal("result should not be nil")
	}

	if result.Response != expectedResponse {
		t.Error("result should contain the expected response")
	}

	if result.Provider != "provider1" {
		t.Errorf("expected provider 'provider1', got '%s'", result.Provider)
	}

	if result.Model != "model1" {
		t.Errorf("expected model 'model1', got '%s'", result.Model)
	}

	// Success should mark provider as available
	if !ct.IsAvailable("provider1") {
		t.Error("provider should be marked as available after success")
	}
}

// TestFallbackChain_Execute_FallbackTests tests fallback to second candidate
func TestFallbackChain_Execute_FallbackTests(t *testing.T) {
	ct := NewCooldownTracker()
	fc := NewFallbackChain(ct)

	ctx := context.Background()
	candidates := []FallbackCandidate{
		{Provider: "provider1", Model: "model1"},
		{Provider: "provider2", Model: "model2"},
	}

	expectedResponse := &LLMResponse{Content: "fallback success"}
	callCount := 0

	runFunc := func(ctx context.Context, provider, model string) (*LLMResponse, error) {
		callCount++
		if provider == "provider1" {
			// Return a plain error that will be classified
			return nil, errors.New("rate limit exceeded")
		}
		return expectedResponse, nil
	}

	result, err := fc.Execute(ctx, candidates, runFunc)

	if err != nil {
		t.Errorf("unexpected error: %v", err)
	}

	if result == nil {
		t.Fatal("result should not be nil")
	}

	if callCount != 2 {
		t.Errorf("expected 2 calls, got %d", callCount)
	}

	if result.Provider != "provider2" {
		t.Errorf("expected provider 'provider2', got '%s'", result.Provider)
	}

	// Note: provider1 cooldown may be cleared during successful run
	// The important thing is that fallback occurred and succeeded
	if callCount != 2 {
		t.Errorf("expected 2 calls, got %d", callCount)
	}

	// Second provider should be available after success
	if !ct.IsAvailable("provider2") {
		t.Error("provider2 should be available after success")
	}
}

// TestFallbackChain_Execute_ContextCancellation tests context cancellation
func TestFallbackChain_Execute_ContextCancellation(t *testing.T) {
	ct := NewCooldownTracker()
	fc := NewFallbackChain(ct)

	ctx, cancel := context.WithCancel(context.Background())
	cancel() // Cancel immediately

	candidates := []FallbackCandidate{
		{Provider: "provider1", Model: "model1"},
	}

	runFunc := func(ctx context.Context, provider, model string) (*LLMResponse, error) {
		return &LLMResponse{Content: "should not reach"}, nil
	}

	result, err := fc.Execute(ctx, candidates, runFunc)

	if err != context.Canceled {
		t.Errorf("expected context.Canceled error, got: %v", err)
	}

	if result != nil {
		t.Error("result should be nil when context is cancelled")
	}
}

// TestFallbackChain_Execute_ContextCancellationDuringRun tests cancellation during execution
func TestFallbackChain_Execute_ContextCancellationDuringRun(t *testing.T) {
	ct := NewCooldownTracker()
	fc := NewFallbackChain(ct)

	ctx, cancel := context.WithCancel(context.Background())

	candidates := []FallbackCandidate{
		{Provider: "provider1", Model: "model1"},
		{Provider: "provider2", Model: "model2"},
	}

	callCount := 0
	runFunc := func(ctx context.Context, provider, model string) (*LLMResponse, error) {
		callCount++
		if callCount == 1 {
			// Cancel on first call
			cancel()
			return nil, &FailoverError{
				Reason:   FailoverRateLimit,
				Provider: provider,
				Model:    model,
				Wrapped:  errors.New("error"),
			}
		}
		return &LLMResponse{Content: "success"}, nil
	}

	result, err := fc.Execute(ctx, candidates, runFunc)

	if err != context.Canceled {
		t.Errorf("expected context.Canceled error, got: %v", err)
	}

	if result != nil {
		t.Error("result should be nil when context is cancelled")
	}

	// Should have made first attempt
	if callCount != 1 {
		t.Errorf("expected 1 call before cancellation, got %d", callCount)
	}
}

// TestFallbackChain_Execute_NonRetriableError tests non-retriable errors
func TestFallbackChain_Execute_NonRetriableError(t *testing.T) {
	ct := NewCooldownTracker()
	fc := NewFallbackChain(ct)

	ctx := context.Background()
	candidates := []FallbackCandidate{
		{Provider: "provider1", Model: "model1"},
		{Provider: "provider2", Model: "model2"},
	}

	runFunc := func(ctx context.Context, provider, model string) (*LLMResponse, error) {
		// Return a format error which should be classified as non-retriable
		return nil, errors.New("string should match pattern")
	}

	result, err := fc.Execute(ctx, candidates, runFunc)

	if err == nil {
		t.Error("expected error for non-retriable error")
	}

	var failErr *FailoverError
	if !errors.As(err, &failErr) {
		t.Fatal("expected FailoverError")
	}

	if failErr.Reason != FailoverFormat {
		t.Errorf("expected reason FailoverFormat, got %v", failErr.Reason)
	}

	if result != nil {
		t.Error("result should be nil for non-retriable error")
	}
}

// TestFallbackChain_Execute_AllCandidatesExhausted tests when all candidates fail
func TestFallbackChain_Execute_AllCandidatesExhausted(t *testing.T) {
	ct := NewCooldownTracker()
	fc := NewFallbackChain(ct)

	ctx := context.Background()
	candidates := []FallbackCandidate{
		{Provider: "provider1", Model: "model1"},
		{Provider: "provider2", Model: "model2"},
		{Provider: "provider3", Model: "model3"},
	}

	runFunc := func(ctx context.Context, provider, model string) (*LLMResponse, error) {
		return nil, &FailoverError{
			Reason:   FailoverRateLimit,
			Provider: provider,
			Model:    model,
			Wrapped:  errors.New("rate limited"),
		}
	}

	result, err := fc.Execute(ctx, candidates, runFunc)

	if err == nil {
		t.Error("expected error when all candidates fail")
	}

	var exhaustErr *FallbackExhaustedError
	if !errors.As(err, &exhaustErr) {
		t.Fatal("expected FallbackExhaustedError")
	}

	if len(exhaustErr.Attempts) != 3 {
		t.Errorf("expected 3 attempts, got %d", len(exhaustErr.Attempts))
	}

	if result != nil {
		t.Error("result should be nil when all candidates fail")
	}

	// All providers should be in cooldown
	if ct.IsAvailable("provider1") {
		t.Error("provider1 should be in cooldown")
	}
	if ct.IsAvailable("provider2") {
		t.Error("provider2 should be in cooldown")
	}
	if ct.IsAvailable("provider3") {
		t.Error("provider3 should be in cooldown")
	}
}

// TestFallbackChain_Execute_CooldownSkipped tests skipping providers in cooldown
func TestFallbackChain_Execute_CooldownSkipped(t *testing.T) {
	ct := NewCooldownTracker()
	fc := NewFallbackChain(ct)

	ctx := context.Background()
	candidates := []FallbackCandidate{
		{Provider: "provider1", Model: "model1"},
		{Provider: "provider2", Model: "model2"},
		{Provider: "provider3", Model: "model3"},
	}

	// Put provider1 in cooldown
	ct.MarkFailure("provider1", FailoverRateLimit)

	callCount := 0
	runFunc := func(ctx context.Context, provider, model string) (*LLMResponse, error) {
		callCount++
		return &LLMResponse{Content: "success from " + provider}, nil
	}

	result, err := fc.Execute(ctx, candidates, runFunc)

	if err != nil {
		t.Errorf("unexpected error: %v", err)
	}

	if result.Provider != "provider2" {
		t.Errorf("expected provider2 to be used (skipping provider1 in cooldown), got '%s'", result.Provider)
	}

	if callCount != 1 {
		t.Errorf("expected 1 call (provider1 skipped), got %d", callCount)
	}

	if len(result.Attempts) != 1 {
		t.Errorf("expected 1 attempt in result, got %d", len(result.Attempts))
	}
}

// TestFallbackChain_Execute_AllInCooldown tests when all providers are in cooldown
func TestFallbackChain_Execute_AllInCooldown(t *testing.T) {
	ct := NewCooldownTracker()
	fc := NewFallbackChain(ct)

	ctx := context.Background()
	candidates := []FallbackCandidate{
		{Provider: "provider1", Model: "model1"},
		{Provider: "provider2", Model: "model2"},
	}

	// Put all providers in cooldown
	ct.MarkFailure("provider1", FailoverRateLimit)
	ct.MarkFailure("provider2", FailoverRateLimit)

	runFunc := func(ctx context.Context, provider, model string) (*LLMResponse, error) {
		return &LLMResponse{Content: "should not be called"}, nil
	}

	result, err := fc.Execute(ctx, candidates, runFunc)

	if err == nil {
		t.Error("expected error when all providers are in cooldown")
	}

	var exhaustErr *FallbackExhaustedError
	if !errors.As(err, &exhaustErr) {
		t.Fatal("expected FallbackExhaustedError")
	}

	if len(exhaustErr.Attempts) != 2 {
		t.Errorf("expected 2 skipped attempts, got %d", len(exhaustErr.Attempts))
	}

	// All attempts should be marked as skipped
	for _, attempt := range exhaustErr.Attempts {
		if !attempt.Skipped {
			t.Error("all attempts should be marked as skipped")
		}
	}

	if result != nil {
		t.Error("result should be nil when all providers are in cooldown")
	}
}

// TestFallbackChain_Execute_UnclassifiedError tests unclassified errors
func TestFallbackChain_Execute_UnclassifiedError(t *testing.T) {
	ct := NewCooldownTracker()
	fc := NewFallbackChain(ct)

	ctx := context.Background()
	candidates := []FallbackCandidate{
		{Provider: "provider1", Model: "model1"},
	}

	runFunc := func(ctx context.Context, provider, model string) (*LLMResponse, error) {
		return nil, errors.New("unknown error")
	}

	result, err := fc.Execute(ctx, candidates, runFunc)

	if err == nil {
		t.Error("expected error for unclassified error")
	}

	if !strings.Contains(err.Error(), "unclassified error") {
		t.Errorf("expected error message about unclassified error, got: %v", err)
	}

	if result != nil {
		t.Error("result should be nil for unclassified error")
	}
}

// TestFallbackChain_ExecuteImage_NoCandidates tests image execution with no candidates
func TestFallbackChain_ExecuteImage_NoCandidates(t *testing.T) {
	ct := NewCooldownTracker()
	fc := NewFallbackChain(ct)

	ctx := context.Background()
	result, err := fc.ExecuteImage(ctx, nil, func(ctx context.Context, provider, model string) (*LLMResponse, error) {
		return nil, nil
	})

	if err == nil {
		t.Error("expected error when no candidates provided")
	}

	if !strings.Contains(err.Error(), "no candidates") {
		t.Errorf("expected error about no candidates, got: %v", err)
	}

	if result != nil {
		t.Error("result should be nil when error occurs")
	}
}

// TestFallbackChain_ExecuteImage_Success tests successful image execution
func TestFallbackChain_ExecuteImage_Success(t *testing.T) {
	ct := NewCooldownTracker()
	fc := NewFallbackChain(ct)

	ctx := context.Background()
	candidates := []FallbackCandidate{
		{Provider: "provider1", Model: "model1"},
	}

	expectedResponse := &LLMResponse{Content: "image success"}

	runFunc := func(ctx context.Context, provider, model string) (*LLMResponse, error) {
		return expectedResponse, nil
	}

	result, err := fc.ExecuteImage(ctx, candidates, runFunc)

	if err != nil {
		t.Errorf("unexpected error: %v", err)
	}

	if result.Response != expectedResponse {
		t.Error("result should contain the expected response")
	}

	if result.Provider != "provider1" {
		t.Errorf("expected provider 'provider1', got '%s'", result.Provider)
	}
}

// TestFallbackChain_ExecuteImage_Fallback tests image fallback
func TestFallbackChain_ExecuteImage_Fallback(t *testing.T) {
	ct := NewCooldownTracker()
	fc := NewFallbackChain(ct)

	ctx := context.Background()
	candidates := []FallbackCandidate{
		{Provider: "provider1", Model: "model1"},
		{Provider: "provider2", Model: "model2"},
	}

	callCount := 0
	runFunc := func(ctx context.Context, provider, model string) (*LLMResponse, error) {
		callCount++
		if provider == "provider1" {
			return nil, errors.New("provider1 failed")
		}
		return &LLMResponse{Content: "success"}, nil
	}

	result, err := fc.ExecuteImage(ctx, candidates, runFunc)

	if err != nil {
		t.Errorf("unexpected error: %v", err)
	}

	if callCount != 2 {
		t.Errorf("expected 2 calls, got %d", callCount)
	}

	if result.Provider != "provider2" {
		t.Errorf("expected provider 'provider2', got '%s'", result.Provider)
	}
}

// TestFallbackChain_ExecuteImage_DimensionError tests image dimension error (non-retriable)
func TestFallbackChain_ExecuteImage_DimensionError(t *testing.T) {
	ct := NewCooldownTracker()
	fc := NewFallbackChain(ct)

	ctx := context.Background()
	candidates := []FallbackCandidate{
		{Provider: "provider1", Model: "model1"},
		{Provider: "provider2", Model: "model2"},
	}

	runFunc := func(ctx context.Context, provider, model string) (*LLMResponse, error) {
		return nil, errors.New("image dimensions exceed max size")
	}

	result, err := fc.ExecuteImage(ctx, candidates, runFunc)

	if err == nil {
		t.Error("expected error for dimension error")
	}

	var failErr *FailoverError
	if !errors.As(err, &failErr) {
		t.Error("expected FailoverError")
	}

	if failErr.Reason != FailoverFormat {
		t.Errorf("expected reason FailoverFormat, got %v", failErr.Reason)
	}

	if result != nil {
		t.Error("result should be nil for dimension error")
	}
}

// TestFallbackChain_ExecuteImage_SizeError tests image size error (non-retriable)
func TestFallbackChain_ExecuteImage_SizeError(t *testing.T) {
	ct := NewCooldownTracker()
	fc := NewFallbackChain(ct)

	ctx := context.Background()
	candidates := []FallbackCandidate{
		{Provider: "provider1", Model: "model1"},
	}

	runFunc := func(ctx context.Context, provider, model string) (*LLMResponse, error) {
		return nil, errors.New("image exceeds 10MB limit")
	}

	result, err := fc.ExecuteImage(ctx, candidates, runFunc)

	if err == nil {
		t.Error("expected error for size error")
	}

	var failErr *FailoverError
	if !errors.As(err, &failErr) {
		t.Error("expected FailoverError")
	}

	if failErr.Reason != FailoverFormat {
		t.Errorf("expected reason FailoverFormat, got %v", failErr.Reason)
	}

	if result != nil {
		t.Error("result should be nil for size error")
	}
}

// TestFallbackChain_ExecuteImage_ContextCancellation tests context cancellation in image execution
func TestFallbackChain_ExecuteImage_ContextCancellation(t *testing.T) {
	ct := NewCooldownTracker()
	fc := NewFallbackChain(ct)

	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	candidates := []FallbackCandidate{
		{Provider: "provider1", Model: "model1"},
	}

	result, err := fc.ExecuteImage(ctx, candidates, func(ctx context.Context, provider, model string) (*LLMResponse, error) {
		return &LLMResponse{Content: "should not reach"}, nil
	})

	if err != context.Canceled {
		t.Errorf("expected context.Canceled error, got: %v", err)
	}

	if result != nil {
		t.Error("result should be nil when context is cancelled")
	}
}

// TestFallbackChain_ExecuteImage_AllExhausted tests when all image candidates fail
func TestFallbackChain_ExecuteImage_AllExhausted(t *testing.T) {
	ct := NewCooldownTracker()
	fc := NewFallbackChain(ct)

	ctx := context.Background()
	candidates := []FallbackCandidate{
		{Provider: "provider1", Model: "model1"},
		{Provider: "provider2", Model: "model2"},
	}

	runFunc := func(ctx context.Context, provider, model string) (*LLMResponse, error) {
		return nil, errors.New("failed")
	}

	result, err := fc.ExecuteImage(ctx, candidates, runFunc)

	if err == nil {
		t.Error("expected error when all candidates fail")
	}

	var exhaustErr *FallbackExhaustedError
	if !errors.As(err, &exhaustErr) {
		t.Fatal("expected FallbackExhaustedError")
	}

	if len(exhaustErr.Attempts) != 2 {
		t.Errorf("expected 2 attempts, got %d", len(exhaustErr.Attempts))
	}

	if result != nil {
		t.Error("result should be nil when all candidates fail")
	}
}

// TestFallbackExhaustedError_Error tests error message formatting
func TestFallbackExhaustedError_Error(t *testing.T) {
	err := &FallbackExhaustedError{
		Attempts: []FallbackAttempt{
			{
				Provider: "provider1",
				Model:    "model1",
				Error:    errors.New("error1"),
				Reason:   FailoverRateLimit,
				Duration: 100 * time.Millisecond,
				Skipped:  false,
			},
			{
				Provider: "provider2",
				Model:    "model2",
				Skipped:  true,
			},
		},
	}

	msg := err.Error()

	if !strings.Contains(msg, "all 2 candidates failed") {
		t.Errorf("error message should mention all candidates failed: %s", msg)
	}

	if !strings.Contains(msg, "provider1") {
		t.Errorf("error message should mention provider1: %s", msg)
	}

	if !strings.Contains(msg, "provider2") {
		t.Errorf("error message should mention provider2: %s", msg)
	}

	if !strings.Contains(msg, "skipped") {
		t.Errorf("error message should mention skipped: %s", msg)
	}
}

// TestResolveCandidates tests resolving candidates from config
func TestResolveCandidates(t *testing.T) {
	tests := []struct {
		name             string
		config           ModelConfig
		defaultProvider  string
		expectedCount    int
		expectedPrimary  string
		expectedFallback []string
	}{
		{
			name: "primary only",
			config: ModelConfig{
				Primary: "anthropic/claude-3-5",
			},
			defaultProvider:  "openai",
			expectedCount:    1,
			expectedPrimary:  "anthropic",
			expectedFallback: nil,
		},
		{
			name: "primary with fallbacks",
			config: ModelConfig{
				Primary:   "anthropic/claude-3-5",
				Fallbacks: []string{"openai/gpt-4", "gemini/gemini-pro"},
			},
			defaultProvider:  "openai",
			expectedCount:    3,
			expectedPrimary:  "anthropic",
			expectedFallback: []string{"openai", "gemini"},
		},
		{
			name: "primary without provider",
			config: ModelConfig{
				Primary: "claude-3-5",
			},
			defaultProvider:  "anthropic",
			expectedCount:    1,
			expectedPrimary:  "anthropic",
			expectedFallback: nil,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			candidates := ResolveCandidates(tt.config, tt.defaultProvider)

			if len(candidates) != tt.expectedCount {
				t.Errorf("expected %d candidates, got %d", tt.expectedCount, len(candidates))
			}

			if len(candidates) > 0 {
				if candidates[0].Provider != tt.expectedPrimary {
					t.Errorf("expected primary provider '%s', got '%s'", tt.expectedPrimary, candidates[0].Provider)
				}
			}

			if tt.expectedFallback != nil && len(candidates) > 1 {
				for i, expected := range tt.expectedFallback {
					if candidates[i+1].Provider != expected {
						t.Errorf("fallback %d: expected provider '%s', got '%s'", i, expected, candidates[i+1].Provider)
					}
				}
			}
		})
	}
}

// TestResolveCandidates_Deduplication tests candidate deduplication
func TestResolveCandidates_Deduplication(t *testing.T) {
	config := ModelConfig{
		Primary:   "anthropic/claude-3-5",
		Fallbacks: []string{"anthropic/claude-3-5", "openai/gpt-4", "anthropic/claude-3-5"},
	}

	candidates := ResolveCandidates(config, "openai")

	if len(candidates) != 2 {
		t.Errorf("expected 2 unique candidates, got %d", len(candidates))
	}

	// Should not have duplicates
	seen := make(map[string]bool)
	for _, c := range candidates {
		key := c.Provider + "/" + c.Model
		if seen[key] {
			t.Error("found duplicate candidate")
		}
		seen[key] = true
	}
}

// TestFallbackAttempt tests fallback attempt structure
func TestFallbackAttempt(t *testing.T) {
	attempt := FallbackAttempt{
		Provider: "test-provider",
		Model:    "test-model",
		Error:    errors.New("test error"),
		Reason:   FailoverRateLimit,
		Duration: 100 * time.Millisecond,
		Skipped:  false,
	}

	if attempt.Provider != "test-provider" {
		t.Errorf("expected provider 'test-provider', got '%s'", attempt.Provider)
	}

	if attempt.Model != "test-model" {
		t.Errorf("expected model 'test-model', got '%s'", attempt.Model)
	}

	if attempt.Reason != FailoverRateLimit {
		t.Errorf("expected reason FailoverRateLimit, got %v", attempt.Reason)
	}

	if attempt.Duration != 100*time.Millisecond {
		t.Errorf("expected duration 100ms, got %v", attempt.Duration)
	}

	if attempt.Skipped {
		t.Error("expected Skipped to be false")
	}
}

// TestFallbackResult tests fallback result structure
func TestFallbackResult(t *testing.T) {
	response := &LLMResponse{Content: "test response"}
	result := &FallbackResult{
		Response: response,
		Provider: "test-provider",
		Model:    "test-model",
		Attempts: []FallbackAttempt{
			{Provider: "p1", Model: "m1"},
			{Provider: "p2", Model: "m2"},
		},
	}

	if result.Response != response {
		t.Error("response should match")
	}

	if result.Provider != "test-provider" {
		t.Errorf("expected provider 'test-provider', got '%s'", result.Provider)
	}

	if result.Model != "test-model" {
		t.Errorf("expected model 'test-model', got '%s'", result.Model)
	}

	if len(result.Attempts) != 2 {
		t.Errorf("expected 2 attempts, got %d", len(result.Attempts))
	}
}

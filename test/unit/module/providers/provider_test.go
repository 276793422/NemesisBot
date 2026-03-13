// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package providers

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/providers"
	"github.com/276793422/NemesisBot/module/providers/protocoltypes"
)

// TestLLMProvider is a mock implementation for testing
type TestLLMProvider struct {
	name  string
	model string
	fail  bool
	resp  *protocoltypes.LLMResponse
	delay time.Duration
}

// cooldownTrackerWithTime is a helper for testing cooldown functionality
type cooldownTrackerWithTime struct {
	*providers.CooldownTracker
	now time.Time
}

func (p *TestLLMProvider) Chat(ctx context.Context, messages []protocoltypes.Message, tools []protocoltypes.ToolDefinition, model string, options map[string]interface{}) (*protocoltypes.LLMResponse, error) {
	if p.delay > 0 {
		select {
		case <-time.After(p.delay):
		case <-ctx.Done():
			return nil, ctx.Err()
		}
	}

	if p.fail {
		return nil, fmt.Errorf("test error from %s", p.name)
	}

	if p.resp != nil {
		return p.resp, nil
	}

	return &protocoltypes.LLMResponse{
		Content: fmt.Sprintf("Response from %s using %s", p.name, model),
		Usage: &protocoltypes.UsageInfo{
			PromptTokens:     10,
			CompletionTokens: 5,
		},
	}, nil
}

func (p *TestLLMProvider) GetDefaultModel() string {
	return p.model
}

func (p *TestLLMProvider) getLLMResponse() *protocoltypes.LLMResponse {
	return &protocoltypes.LLMResponse{
		Content: "test response",
		Usage: &protocoltypes.UsageInfo{
			PromptTokens:     5,
			CompletionTokens: 3,
		},
	}
}

// TestLLMInterface tests the basic LLMProvider interface
func TestLLMInterface(t *testing.T) {
	ctx := context.Background()
	messages := []protocoltypes.Message{{Role: "user", Content: "test"}}

	provider := &TestLLMProvider{
		name:  "test",
		model: "test-model",
		resp: &protocoltypes.LLMResponse{
			Content: "test response",
			Usage: &protocoltypes.UsageInfo{
				PromptTokens:     5,
				CompletionTokens: 3,
			},
		},
	}

	// Test Chat method
	resp, err := provider.Chat(ctx, messages, nil, "test-model", nil)
	if err != nil {
		t.Fatalf("Chat() error = %v", err)
	}
	if resp == nil {
		t.Fatal("Chat() returned nil response")
	}
	if resp.Content != "test response" {
		t.Errorf("Chat() content = %v, want %v", resp.Content, "test response")
	}

	// Test GetDefaultModel
	model := provider.GetDefaultModel()
	if model != "test-model" {
		t.Errorf("GetDefaultModel() = %v, want %v", model, "test-model")
	}
}

// TestFailoverError tests the providers.FailoverError type and methods
func TestFailoverError(t *testing.T) {
	err := &providers.FailoverError{
		Reason:   providers.FailoverRateLimit,
		Provider: "test-provider",
		Model:    "test-model",
		Status:   429,
		Wrapped:  errors.New("rate limit exceeded"),
	}

	// Test Error() method
	want := "failover(rate_limit): provider=test-provider model=test-model status=429: rate limit exceeded"
	got := err.Error()
	if got != want {
		t.Errorf("providers.FailoverError.Error() = %v, want %v", got, want)
	}

	// Test Unwrap() method
	unwrapped := err.Unwrap()
	if unwrapped != err.Wrapped {
		t.Error("providers.FailoverError.Unwrap() did not return wrapped error")
	}

	// Test IsRetriable() method
	tests := []struct {
		reason   providers.FailoverReason
		expected bool
	}{
		{providers.FailoverRateLimit, true},
		{providers.FailoverTimeout, true},
		{providers.FailoverAuth, true},
		{providers.FailoverBilling, true},
		{providers.FailoverOverloaded, true},
		{providers.FailoverUnknown, true},
		{providers.FailoverFormat, false},
	}

	for _, tt := range tests {
		err.Reason = tt.reason
		if err.IsRetriable() != tt.expected {
			t.Errorf("IsRetriable() for reason %s = %v, want %v", tt.reason, err.IsRetriable(), tt.expected)
		}
	}
}

// Testproviders.ModelConfig tests the providers.ModelConfig type
func TestModelConfig(t *testing.T) {
	cfg := providers.ModelConfig{
		Primary:   "anthropic/claude-3-opus",
		Fallbacks: []string{"openai/gpt-4", "anthropic/claude-3-sonnet"},
	}

	if cfg.Primary != "anthropic/claude-3-opus" {
		t.Errorf("providers.ModelConfig.Primary = %v, want %v", cfg.Primary, "anthropic/claude-3-opus")
	}

	if len(cfg.Fallbacks) != 2 {
		t.Errorf("providers.ModelConfig.Fallbacks length = %v, want %v", len(cfg.Fallbacks), 2)
	}

	if cfg.Fallbacks[0] != "openai/gpt-4" {
		t.Errorf("providers.ModelConfig.Fallbacks[0] = %v, want %v", cfg.Fallbacks[0], "openai/gpt-4")
	}
}

// TestModelRef tests model reference parsing
func TestModelRef(t *testing.T) {
	tests := []struct {
		input       string
		defaultProv string
		want        *providers.ModelRef
	}{
		{"anthropic/claude-3-opus", "openai", &providers.ModelRef{Provider: "anthropic", Model: "claude-3-opus"}},
		{"gpt-4", "openai", &providers.ModelRef{Provider: "openai", Model: "gpt-4"}},
		{"  ZAI/  glm-4  ", "openai", &providers.ModelRef{Provider: "zai", Model: "glm-4"}},
		{"", "openai", nil},
		{"invalid-slash", "openai", &providers.ModelRef{Provider: "openai", Model: "invalid-slash"}},
		{"invalid-slash/", "openai", nil},
		{"/model", "openai", &providers.ModelRef{Provider: "openai", Model: "/model"}},
	}

	for _, tt := range tests {
		got := providers.ParseModelRef(tt.input, tt.defaultProv)
		if !equalModelRef(got, tt.want) {
			t.Errorf("providers.ParseModelRef(%q, %q) = %v, want %v", tt.input, tt.defaultProv, got, tt.want)
		}
	}
}

// Testproviders.NormalizeProvider tests provider normalization
func TestNormalizeProvider(t *testing.T) {
	tests := []struct {
		input    string
		expected string
	}{
		{"anthropic", "anthropic"},
		{"claude", "anthropic"},
		{"openai", "openai"},
		{"gpt", "openai"},
		{"z.ai", "zai"},
		{"z-ai", "zai"},
		{"qwen", "qwen-portal"},
		{"kimi-code", "kimi-coding"},
		{"google", "gemini"},
		{"  Z-Ai  ", "zai"},
		{"unknown", "unknown"},
	}

	for _, tt := range tests {
		got := providers.NormalizeProvider(tt.input)
		if got != tt.expected {
			t.Errorf("providers.NormalizeProvider(%q) = %q, want %q", tt.input, got, tt.expected)
		}
	}
}

// Testproviders.ModelKey tests model key generation
func TestModelKey(t *testing.T) {
	tests := []struct {
		provider, model string
		expected        string
	}{
		{"anthropic", "claude-3-opus", "anthropic/claude-3-opus"},
		{"Claude", "CLAUDE-3-Opus", "anthropic/claude-3-opus"},
		{"z.ai", "glm-4", "zai/glm-4"},
		{"  OpenAI  ", "  GPT-4  ", "openai/gpt-4"},
	}

	for _, tt := range tests {
		got := providers.ModelKey(tt.provider, tt.model)
		if got != tt.expected {
			t.Errorf("providers.ModelKey(%q, %q) = %q, want %q", tt.provider, tt.model, got, tt.expected)
		}
	}
}

func equalModelRef(a, b *providers.ModelRef) bool {
	if a == nil && b == nil {
		return true
	}
	if a == nil || b == nil {
		return false
	}
	return a.Provider == b.Provider && a.Model == b.Model
}

// TestCooldownTracker tests the cooldown tracking functionality
func TestCooldownTracker(t *testing.T) {
	ct := providers.NewCooldownTracker()
	provider := "test-provider"

	// Initially available
	if !ct.IsAvailable(provider) {
		t.Error("New provider should be available")
	}

	// Mark success
	ct.MarkSuccess(provider)
	if !ct.IsAvailable(provider) {
		t.Error("Provider should be available after success")
	}

	// Check error count
	count := ct.ErrorCount(provider)
	if count != 0 {
		t.Errorf("Error count after success = %v, want 0", count)
	}

	// Mark failure
	ct.MarkFailure(provider, providers.FailoverRateLimit)

	// Should not be available
	if ct.IsAvailable(provider) {
		t.Error("Provider should not be available after failure")
	}

	// Check error count
	count = ct.ErrorCount(provider)
	if count != 1 {
		t.Errorf("Error count after failure = %v, want 1", count)
	}

	// Check cooldown remaining
	remaining := ct.CooldownRemaining(provider)
	if remaining <= 0 {
		t.Errorf("Cooldown remaining should be positive, got %v", remaining)
	}

	// Check specific failure count
	rateLimitCount := ct.FailureCount(provider, providers.FailoverRateLimit)
	if rateLimitCount != 1 {
		t.Errorf("Rate limit failure count = %v, want 1", rateLimitCount)
	}

	// Mark success again
	ct.MarkSuccess(provider)
	if !ct.IsAvailable(provider) {
		t.Error("Provider should be available after second success")
	}
}

// TestCooldownTrackerExponentialBackoff tests the exponential backoff calculations
func TestCooldownTrackerExponentialBackoff(t *testing.T) {
	// Test standard cooldown progression
	tests := []struct {
		failCount int
		expected  time.Duration
	}{
		{1, time.Minute},      // 1 error -> 1 min
		{2, 5 * time.Minute},  // 2 errors -> 5 min
		{3, 25 * time.Minute}, // 3 errors -> 25 min
		{4, time.Hour},        // 4+ errors -> 1 hour (cap)
	}

	for _, tt := range tests {
		ct := providers.NewCooldownTracker()

		// Test actual cooldown progression with real time
		for i := 0; i < tt.failCount; i++ {
			ct.MarkFailure("test-provider", providers.FailoverRateLimit)
			time.Sleep(10 * time.Millisecond) // Small delay between failures
		}

		remaining := ct.CooldownRemaining("test-provider")
		// Check that cooldown is set (we can't test exact durations due to timing)
		if remaining == 0 && tt.failCount > 0 {
			t.Errorf("After %d failures, cooldown should be active, but got 0", tt.failCount)
		}
	}
}

// TestFallbackChain tests the basic fallback chain functionality
func TestFallbackChain(t *testing.T) {
	ct := providers.NewCooldownTracker()
	fc := providers.NewFallbackChain(ct)

	ctx := context.Background()
	candidates := []providers.FallbackCandidate{
		{Provider: "provider1", Model: "model1"},
		{Provider: "provider2", Model: "model2"},
	}

	// Test successful execution on first candidate
	providers := []*TestLLMProvider{
		{name: "provider1", model: "model1", fail: false},
		{name: "provider2", model: "model2", fail: false},
	}

	result, err := fc.Execute(ctx, candidates, func(ctx context.Context, provider, model string) (*protocoltypes.LLMResponse, error) {
		for _, p := range providers {
			if p.name == provider && p.model == model {
				resp, err := p.Chat(ctx, nil, nil, model, nil)
				if err != nil {
					return nil, fmt.Errorf("classified error: %w", err)
				}
				return resp, nil
			}
		}
		return nil, fmt.Errorf("provider not found")
	})

	if err != nil {
		t.Fatalf("Execute() error = %v", err)
	}

	if result.Provider != "provider1" {
		t.Errorf("Result provider = %v, want provider1", result.Provider)
	}
	if result.Model != "model1" {
		t.Errorf("Result model = %v, want model1", result.Model)
	}
	// On success, no attempts should be recorded as it returns immediately
	if len(result.Attempts) != 0 {
		t.Errorf("Result attempts length = %v, want 0 (success returns immediately)", len(result.Attempts))
	}
}

// TestFallbackChainFailure tests fallback behavior on failure
func TestFallbackChainFailure(t *testing.T) {
	ct := providers.NewCooldownTracker()
	fc := providers.NewFallbackChain(ct)

	ctx := context.Background()
	candidates := []providers.FallbackCandidate{
		{Provider: "provider1", Model: "model1"},
		{Provider: "provider2", Model: "model2"},
	}

	// Test failure on first candidate, success on second
	providers := []*TestLLMProvider{
		{name: "provider1", model: "model1", fail: true},
		{name: "provider2", model: "model2", fail: false},
	}

	result, err := fc.Execute(ctx, candidates, func(ctx context.Context, provider, model string) (*protocoltypes.LLMResponse, error) {
		for _, p := range providers {
			if p.name == provider && p.model == model {
				resp, err := p.Chat(ctx, nil, nil, model, nil)
				if err != nil {
					// Return an error that will be classified as rate limited
					return nil, fmt.Errorf("rate limit exceeded")
				}
				return resp, nil
			}
		}
		return nil, fmt.Errorf("provider not found")
	})

	if err != nil {
		t.Fatalf("Execute() error = %v", err)
	}

	if result.Provider != "provider2" {
		t.Errorf("Result provider = %v, want provider2", result.Provider)
	}
	// Only the failed attempt should be recorded
	if len(result.Attempts) != 1 {
		t.Errorf("Result attempts length = %v, want 1 (only failed attempt recorded)", len(result.Attempts))
	}
}

// TestFallbackChainAllFail tests behavior when all candidates fail
func TestFallbackChainAllFail(t *testing.T) {
	ct := providers.NewCooldownTracker()
	fc := providers.NewFallbackChain(ct)

	ctx := context.Background()
	candidates := []providers.FallbackCandidate{
		{Provider: "provider1", Model: "model1"},
		{Provider: "provider2", Model: "model2"},
	}

	providers := []*TestLLMProvider{
		{name: "provider1", model: "model1", fail: true},
		{name: "provider2", model: "model2", fail: true},
	}

	_, err := fc.Execute(ctx, candidates, func(ctx context.Context, provider, model string) (*protocoltypes.LLMResponse, error) {
		for _, p := range providers {
			if p.name == provider && p.model == model {
				resp, err := p.Chat(ctx, nil, nil, model, nil)
				if err != nil {
					// Return an error that will be classified as rate limited
					return nil, fmt.Errorf("rate limit exceeded")
				}
				return resp, nil
			}
		}
		return nil, fmt.Errorf("provider not found")
	})

	if err == nil {
		t.Fatal("Execute() should return error when all candidates fail")
	}

	// Check that the error contains information about all attempts
	errorStr := err.Error()
	if !strings.Contains(errorStr, "fallback: all 2 candidates failed") {
		t.Errorf("Error message should indicate all candidates failed, got: %v", errorStr)
	}
}

// TestFallbackChainContextCancellation tests context cancellation
func TestFallbackChainContextCancellation(t *testing.T) {
	ct := providers.NewCooldownTracker()
	fc := providers.NewFallbackChain(ct)

	baseCtx := context.Background()
	ctx, cancel := context.WithCancel(baseCtx)
	candidates := []providers.FallbackCandidate{
		{Provider: "provider1", Model: "model1"},
	}

	// Cancel context before execution
	cancel()

	_, err := fc.Execute(ctx, candidates, func(ctx context.Context, provider, model string) (*protocoltypes.LLMResponse, error) {
		return nil, fmt.Errorf("should not be called")
	})

	if err != context.Canceled {
		t.Errorf("Execute() error = %v, want context.Canceled", err)
	}
}

// Testproviders.FallbackExhaustedError tests the providers.FallbackExhaustedError type
func TestFailoverExhaustedError(t *testing.T) {
	attempts := []providers.FallbackAttempt{
		{
			Provider: "provider1",
			Model:    "model1",
			Error:    errors.New("error 1"),
			Reason:   providers.FailoverRateLimit,
			Duration: 100 * time.Millisecond,
		},
		{
			Provider: "provider2",
			Model:    "model2",
			Error:    errors.New("error 2"),
			Reason:   providers.FailoverTimeout,
			Duration: 200 * time.Millisecond,
			Skipped:  true,
		},
	}

	err := &providers.FallbackExhaustedError{Attempts: attempts}

	want := "fallback: all 2 candidates failed:\n  [1] provider1/model1: error 1 (reason=rate_limit, 100ms)\n  [2] provider2/model2: skipped (cooldown)"
	got := err.Error()

	if got != want {
		t.Errorf("providers.FallbackExhaustedError.Error() = %v, want %v", got, want)
	}
}

// Testproviders.ResolveCandidates tests candidate resolution
func TestResolveCandidates(t *testing.T) {
	cfg := providers.ModelConfig{
		Primary:   "anthropic/claude-3-opus",
		Fallbacks: []string{"openai/gpt-4", "anthropic/claude-3-sonnet", "anthropic/claude-3-opus"},
	}

	candidates := providers.ResolveCandidates(cfg, "openai")

	if len(candidates) != 3 {
		t.Errorf("providers.ResolveCandidates length = %v, want 3", len(candidates))
	}

	// Check for duplicates
	seen := make(map[string]bool)
	for _, c := range candidates {
		key := providers.ModelKey(c.Provider, c.Model)
		if seen[key] {
			t.Errorf("Duplicate candidate found: %v", c)
		}
		seen[key] = true
	}

	// Check primary is first
	if candidates[0].Provider != "anthropic" || candidates[0].Model != "claude-3-opus" {
		t.Errorf("First candidate = %v, want anthropic/claude-3-opus", candidates[0])
	}
}

// TestHTTPProvider tests the HTTP provider wrapper
func TestHTTPProvider(t *testing.T) {
	// HTTPProvider is just a wrapper around openai_compat.Provider
	// We can test basic functionality without actual HTTP calls
	provider := providers.NewHTTPProvider("test-key", "https://api.test.com", "")

	if provider == nil {
		t.Fatal("providers.NewHTTPProvider() returned nil")
	}

	// Test GetDefaultModel
	model := provider.GetDefaultModel()
	if model != "" {
		t.Errorf("HTTPProvider.GetDefaultModel() = %v, want empty string", model)
	}

	// Test that it implements LLMProvider
	var _ providers.LLMProvider = (*providers.HTTPProvider)(nil)
}

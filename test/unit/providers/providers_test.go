// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package providers_test

import (
	"context"
	"errors"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/providers"
	"github.com/276793422/NemesisBot/module/providers/protocoltypes"
)

// testError is a simple error type for testing
type testError struct{}

func (e *testError) Error() string {
	return "test error"
}

// TestFailoverError tests the FailoverError type
func TestFailoverError(t *testing.T) {
	t.Run("Error method", func(t *testing.T) {
		err := &providers.FailoverError{
			Reason:   providers.FailoverAuth,
			Provider: "test-provider",
			Model:    "test-model",
			Status:   401,
			Wrapped:  &testError{},
		}

		errStr := err.Error()
		if errStr == "" {
			t.Error("expected non-empty error string")
		}
	})

	t.Run("Unwrap method", func(t *testing.T) {
		wrapped := &testError{}
		err := &providers.FailoverError{
			Wrapped: wrapped,
		}

		if err.Unwrap() != wrapped {
			t.Error("Unwrap should return the wrapped error")
		}
	})

	t.Run("IsRetriable for format errors", func(t *testing.T) {
		err := &providers.FailoverError{
			Reason: providers.FailoverFormat,
		}

		if err.IsRetriable() {
			t.Error("Format errors should not be retriable")
		}
	})

	t.Run("IsRetriable for other errors", func(t *testing.T) {
		reasons := []providers.FailoverReason{
			providers.FailoverAuth,
			providers.FailoverRateLimit,
			providers.FailoverBilling,
			providers.FailoverTimeout,
			providers.FailoverOverloaded,
			providers.FailoverUnknown,
		}

		for _, reason := range reasons {
			err := &providers.FailoverError{
				Reason: reason,
			}

			if !err.IsRetriable() {
				t.Errorf("%s errors should be retriable", reason)
			}
		}
	})
}

// TestModelConfig tests the ModelConfig type
func TestModelConfig(t *testing.T) {
	t.Run("Primary model only", func(t *testing.T) {
		cfg := providers.ModelConfig{
			Primary: "gpt-4",
		}

		if cfg.Primary != "gpt-4" {
			t.Errorf("expected primary 'gpt-4', got %s", cfg.Primary)
		}

		if len(cfg.Fallbacks) != 0 {
			t.Errorf("expected no fallbacks, got %d", len(cfg.Fallbacks))
		}
	})

	t.Run("Primary with fallbacks", func(t *testing.T) {
		cfg := providers.ModelConfig{
			Primary: "gpt-4",
			Fallbacks: []string{
				"gpt-3.5-turbo",
				"claude-3-sonnet",
			},
		}

		if cfg.Primary != "gpt-4" {
			t.Errorf("expected primary 'gpt-4', got %s", cfg.Primary)
		}

		if len(cfg.Fallbacks) != 2 {
			t.Errorf("expected 2 fallbacks, got %d", len(cfg.Fallbacks))
		}
	})
}

// TestHTTPProvider tests the HTTP provider
func TestHTTPProvider(t *testing.T) {
	t.Run("NewHTTPProvider basic", func(t *testing.T) {
		p := providers.NewHTTPProvider("test-key", "https://api.example.com", "")
		if p == nil {
			t.Fatal("expected non-nil provider")
		}
	})

	t.Run("NewHTTPProvider with proxy", func(t *testing.T) {
		p := providers.NewHTTPProvider("test-key", "https://api.example.com", "http://proxy:8080")
		if p == nil {
			t.Fatal("expected non-nil provider")
		}
	})
}

// TestCreateProviderConfigErrors tests configuration error handling
func TestCreateProviderConfigErrors(t *testing.T) {
	t.Run("empty model reference", func(t *testing.T) {
		cfg := &config.Config{}

		_, err := providers.CreateProvider(cfg)
		if err == nil {
			t.Error("expected error for empty model reference")
		}
	})
}

// TestProviderInterface tests the LLMProvider interface
func TestProviderInterface(t *testing.T) {
	// Test that HTTP provider satisfies the interface
	p := providers.NewHTTPProvider("test-key", "https://api.example.com", "")

	// Verify it implements LLMProvider by checking method signatures
	var _ providers.LLMProvider = p

	// We can't fully test Chat without a real server,
	// but we can verify the interface is satisfied
	_ = p
}

// TestFailoverReasonConstants tests the failover reason constants
func TestFailoverReasonConstants(t *testing.T) {
	reasons := []providers.FailoverReason{
		providers.FailoverAuth,
		providers.FailoverRateLimit,
		providers.FailoverBilling,
		providers.FailoverTimeout,
		providers.FailoverFormat,
		providers.FailoverOverloaded,
		providers.FailoverUnknown,
	}

	for _, reason := range reasons {
		if reason == "" {
			t.Errorf("failover reason should not be empty: %v", reason)
		}
	}
}

// TestProviderTimeout tests provider timeout behavior
func TestProviderTimeout(t *testing.T) {
	// This test verifies timeout handling
	// Actual timeout testing would require a slow server

	t.Run("context timeout", func(t *testing.T) {
		p := providers.NewHTTPProvider("test-key", "https://api.example.com", "")

		ctx, cancel := context.WithTimeout(context.Background(), 1*time.Nanosecond)
		defer cancel()

		// Give time for context to timeout
		time.Sleep(10 * time.Millisecond)

		messages := []protocoltypes.Message{{Role: "user", Content: "Hello"}}

		// This should fail due to context timeout
		_, err := p.Chat(ctx, messages, nil, "gpt-3.5-turbo", nil)

		// The error might be context.DeadlineExceeded or a connection error
		if err == nil {
			t.Error("expected error for timed out context")
		}
	})
}

// TestFailoverErrorWrapping tests that FailoverError properly wraps errors
func TestFailoverErrorWrapping(t *testing.T) {
	t.Run("errors.Is works correctly", func(t *testing.T) {
		originalErr := errors.New("original error")
		failoverErr := &providers.FailoverError{
			Reason:  providers.FailoverAuth,
			Wrapped: originalErr,
		}

		if !errors.Is(failoverErr, originalErr) {
			t.Error("errors.Is should find the wrapped error")
		}
	})
}

// TestProviderGetDefaultModel tests the GetDefaultModel method
func TestProviderGetDefaultModel(t *testing.T) {
	// Most providers should have a default model
	p := providers.NewHTTPProvider("test-key", "https://api.example.com", "")

	// The HTTP provider doesn't have a GetDefaultModel method exposed
	// This is just to show the test pattern
	_ = p
}

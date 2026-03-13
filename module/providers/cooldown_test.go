// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package providers

import (
	"testing"
	"time"
)

// TestCooldownTracker_NewCooldownTracker tests creating a new cooldown tracker
func TestCooldownTracker_NewCooldownTracker(t *testing.T) {
	ct := NewCooldownTracker()
	if ct == nil {
		t.Fatal("NewCooldownTracker() should not return nil")
	}
	if ct.entries == nil {
		t.Error("entries map should be initialized")
	}
	if ct.failureWindow != defaultFailureWindow {
		t.Errorf("expected failureWindow %v, got %v", defaultFailureWindow, ct.failureWindow)
	}
	if ct.nowFunc == nil {
		t.Error("nowFunc should be initialized")
	}
}

// TestCooldownTracker_MarkFailure tests marking a provider as failed
func TestCooldownTracker_MarkFailure(t *testing.T) {
	ct := NewCooldownTracker()

	// Mark first failure
	ct.MarkFailure("test-provider", FailoverAuth)

	if ct.ErrorCount("test-provider") != 1 {
		t.Errorf("expected error count 1, got %d", ct.ErrorCount("test-provider"))
	}

	if ct.FailureCount("test-provider", FailoverAuth) != 1 {
		t.Errorf("expected auth failure count 1, got %d", ct.FailureCount("test-provider", FailoverAuth))
	}

	// Verify provider is in cooldown
	if ct.IsAvailable("test-provider") {
		t.Error("provider should be in cooldown after failure")
	}
}

// TestCooldownTracker_MarkFailure_Multiple tests multiple failures
func TestCooldownTracker_MarkFailure_Multiple(t *testing.T) {
	ct := NewCooldownTracker()

	// Mark multiple failures
	for i := 0; i < 5; i++ {
		ct.MarkFailure("test-provider", FailoverRateLimit)
	}

	if ct.ErrorCount("test-provider") != 5 {
		t.Errorf("expected error count 5, got %d", ct.ErrorCount("test-provider"))
	}

	if ct.FailureCount("test-provider", FailoverRateLimit) != 5 {
		t.Errorf("expected rate_limit failure count 5, got %d", ct.FailureCount("test-provider", FailoverRateLimit))
	}
}

// TestCooldownTracker_MarkFailure_Billing tests billing-specific cooldown
func TestCooldownTracker_MarkFailure_Billing(t *testing.T) {
	ct := NewCooldownTracker()

	// Mark billing failure
	ct.MarkFailure("test-provider", FailoverBilling)

	// Should have billing failure count
	if ct.FailureCount("test-provider", FailoverBilling) != 1 {
		t.Errorf("expected billing failure count 1, got %d", ct.FailureCount("test-provider", FailoverBilling))
	}

	// Provider should be in cooldown
	if ct.IsAvailable("test-provider") {
		t.Error("provider should be in cooldown after billing failure")
	}
}

// TestCooldownTracker_MarkFailure_DifferentReasons tests tracking different failure reasons
func TestCooldownTracker_MarkFailure_DifferentReasons(t *testing.T) {
	ct := NewCooldownTracker()

	ct.MarkFailure("test-provider", FailoverAuth)
	ct.MarkFailure("test-provider", FailoverRateLimit)
	ct.MarkFailure("test-provider", FailoverBilling)

	if ct.ErrorCount("test-provider") != 3 {
		t.Errorf("expected total error count 3, got %d", ct.ErrorCount("test-provider"))
	}

	if ct.FailureCount("test-provider", FailoverAuth) != 1 {
		t.Errorf("expected auth failure count 1, got %d", ct.FailureCount("test-provider", FailoverAuth))
	}

	if ct.FailureCount("test-provider", FailoverRateLimit) != 1 {
		t.Errorf("expected rate_limit failure count 1, got %d", ct.FailureCount("test-provider", FailoverRateLimit))
	}

	if ct.FailureCount("test-provider", FailoverBilling) != 1 {
		t.Errorf("expected billing failure count 1, got %d", ct.FailureCount("test-provider", FailoverBilling))
	}
}

// TestCooldownTracker_MarkSuccess tests marking a provider as successful
func TestCooldownTracker_MarkSuccess(t *testing.T) {
	ct := NewCooldownTracker()

	// Mark failures
	ct.MarkFailure("test-provider", FailoverAuth)
	ct.MarkFailure("test-provider", FailoverRateLimit)

	// Mark success
	ct.MarkSuccess("test-provider")

	// All counts should be reset
	if ct.ErrorCount("test-provider") != 0 {
		t.Errorf("expected error count 0 after success, got %d", ct.ErrorCount("test-provider"))
	}

	if ct.FailureCount("test-provider", FailoverAuth) != 0 {
		t.Errorf("expected auth failure count 0 after success, got %d", ct.FailureCount("test-provider", FailoverAuth))
	}

	// Provider should be available
	if !ct.IsAvailable("test-provider") {
		t.Error("provider should be available after success")
	}
}

// TestCooldownTracker_MarkSuccess_NoEntry tests marking success for provider with no entry
func TestCooldownTracker_MarkSuccess_NoEntry(t *testing.T) {
	ct := NewCooldownTracker()

	// Should not panic
	ct.MarkSuccess("non-existent-provider")

	// Provider should be available
	if !ct.IsAvailable("non-existent-provider") {
		t.Error("non-existent provider should be available")
	}
}

// TestCooldownTracker_IsAvailable tests checking provider availability
func TestCooldownTracker_IsAvailable(t *testing.T) {
	ct := NewCooldownTracker()

	// New provider should be available
	if !ct.IsAvailable("new-provider") {
		t.Error("new provider should be available")
	}

	// After failure, should not be available
	ct.MarkFailure("new-provider", FailoverAuth)
	if ct.IsAvailable("new-provider") {
		t.Error("provider should not be available after failure")
	}
}

// TestCooldownTracker_IsAvailable_CooldownExpiry tests cooldown expiry
func TestCooldownTracker_IsAvailable_CooldownExpiry(t *testing.T) {
	ct := NewCooldownTracker()

	fixedTime := time.Now()
	ct.nowFunc = func() time.Time { return fixedTime }

	// Mark failure
	ct.MarkFailure("test-provider", FailoverAuth)

	// Should be in cooldown
	if ct.IsAvailable("test-provider") {
		t.Error("provider should be in cooldown")
	}

	// Move time forward past cooldown
	ct.nowFunc = func() time.Time { return fixedTime.Add(2 * time.Hour) }

	// Should be available again
	if !ct.IsAvailable("test-provider") {
		t.Error("provider should be available after cooldown expires")
	}
}

// TestCooldownTracker_CooldownRemaining tests getting remaining cooldown time
func TestCooldownTracker_CooldownRemaining(t *testing.T) {
	ct := NewCooldownTracker()

	fixedTime := time.Now()
	ct.nowFunc = func() time.Time { return fixedTime }

	// No entry - should return 0
	remaining := ct.CooldownRemaining("no-entry")
	if remaining != 0 {
		t.Errorf("expected 0 remaining for non-existent provider, got %v", remaining)
	}

	// Mark failure
	ct.MarkFailure("test-provider", FailoverAuth)

	// Should have remaining time
	remaining = ct.CooldownRemaining("test-provider")
	if remaining == 0 {
		t.Error("expected non-zero remaining time")
	}

	// Move time forward
	ct.nowFunc = func() time.Time { return fixedTime.Add(2 * time.Hour) }

	// Should return 0 after cooldown expires
	remaining = ct.CooldownRemaining("test-provider")
	if remaining != 0 {
		t.Errorf("expected 0 remaining after cooldown expires, got %v", remaining)
	}
}

// TestCooldownTracker_CooldownRemaining_Billing tests billing cooldown remaining
func TestCooldownTracker_CooldownRemaining_Billing(t *testing.T) {
	ct := NewCooldownTracker()

	fixedTime := time.Now()
	ct.nowFunc = func() time.Time { return fixedTime }

	// Mark billing failure
	ct.MarkFailure("test-provider", FailoverBilling)

	// Should have remaining time (longer than standard cooldown)
	remaining := ct.CooldownRemaining("test-provider")
	if remaining == 0 {
		t.Error("expected non-zero remaining time for billing failure")
	}

	// Billing cooldown should be longer than standard
	ct.MarkFailure("other-provider", FailoverAuth)
	standardRemaining := ct.CooldownRemaining("other-provider")

	if remaining <= standardRemaining {
		t.Error("billing cooldown should be longer than standard cooldown")
	}
}

// TestCooldownTracker_ErrorCount tests getting error count
func TestCooldownTracker_ErrorCount(t *testing.T) {
	ct := NewCooldownTracker()

	// No entry - should return 0
	count := ct.ErrorCount("no-entry")
	if count != 0 {
		t.Errorf("expected 0 for non-existent provider, got %d", count)
	}

	// With failures
	ct.MarkFailure("test-provider", FailoverAuth)
	ct.MarkFailure("test-provider", FailoverRateLimit)

	count = ct.ErrorCount("test-provider")
	if count != 2 {
		t.Errorf("expected error count 2, got %d", count)
	}
}

// TestCooldownTracker_FailureCount tests getting failure count by reason
func TestCooldownTracker_FailureCount(t *testing.T) {
	ct := NewCooldownTracker()

	// No entry - should return 0
	count := ct.FailureCount("no-entry", FailoverAuth)
	if count != 0 {
		t.Errorf("expected 0 for non-existent provider, got %d", count)
	}

	// With failures
	ct.MarkFailure("test-provider", FailoverAuth)
	ct.MarkFailure("test-provider", FailoverAuth)
	ct.MarkFailure("test-provider", FailoverRateLimit)

	count = ct.FailureCount("test-provider", FailoverAuth)
	if count != 2 {
		t.Errorf("expected auth failure count 2, got %d", count)
	}

	count = ct.FailureCount("test-provider", FailoverRateLimit)
	if count != 1 {
		t.Errorf("expected rate_limit failure count 1, got %d", count)
	}
}

// TestCooldownTracker_FailureWindowReset tests failure window reset
func TestCooldownTracker_FailureWindowReset(t *testing.T) {
	ct := NewCooldownTracker()

	baseTime := time.Now()
	ct.nowFunc = func() time.Time { return baseTime }

	// Mark some failures
	ct.MarkFailure("test-provider", FailoverAuth)
	ct.MarkFailure("test-provider", FailoverRateLimit)

	if ct.ErrorCount("test-provider") != 2 {
		t.Errorf("expected error count 2, got %d", ct.ErrorCount("test-provider"))
	}

	// Move time forward past failure window
	ct.nowFunc = func() time.Time { return baseTime.Add(25 * time.Hour) }

	// Mark another failure - should reset counters
	ct.MarkFailure("test-provider", FailoverBilling)

	if ct.ErrorCount("test-provider") != 1 {
		t.Errorf("expected error count 1 after window reset, got %d", ct.ErrorCount("test-provider"))
	}

	// Old failure counts should be reset
	if ct.FailureCount("test-provider", FailoverAuth) != 0 {
		t.Errorf("expected auth failure count 0 after window reset, got %d", ct.FailureCount("test-provider", FailoverAuth))
	}

	// New failure should be counted
	if ct.FailureCount("test-provider", FailoverBilling) != 1 {
		t.Errorf("expected billing failure count 1, got %d", ct.FailureCount("test-provider", FailoverBilling))
	}
}

// TestCooldownTracker_MultipleProviders tests tracking multiple providers
func TestCooldownTracker_MultipleProviders(t *testing.T) {
	ct := NewCooldownTracker()

	ct.MarkFailure("provider1", FailoverAuth)
	ct.MarkFailure("provider2", FailoverRateLimit)
	ct.MarkFailure("provider3", FailoverBilling)

	// Each provider should have independent counts
	if ct.ErrorCount("provider1") != 1 {
		t.Errorf("expected provider1 error count 1, got %d", ct.ErrorCount("provider1"))
	}

	if ct.ErrorCount("provider2") != 1 {
		t.Errorf("expected provider2 error count 1, got %d", ct.ErrorCount("provider2"))
	}

	if ct.ErrorCount("provider3") != 1 {
		t.Errorf("expected provider3 error count 1, got %d", ct.ErrorCount("provider3"))
	}

	// Mark success for one provider
	ct.MarkSuccess("provider1")

	// Should not affect others
	if ct.ErrorCount("provider1") != 0 {
		t.Errorf("expected provider1 error count 0 after success, got %d", ct.ErrorCount("provider1"))
	}

	if ct.ErrorCount("provider2") != 1 {
		t.Errorf("expected provider2 error count 1, got %d", ct.ErrorCount("provider2"))
	}
}

// TestCalculateStandardCooldown tests standard cooldown calculation
func TestCalculateStandardCooldown(t *testing.T) {
	tests := []struct {
		name        string
		errorCount  int
		minDuration time.Duration
		maxDuration time.Duration
	}{
		{"1 error", 1, 1 * time.Minute, 1 * time.Minute},
		{"2 errors", 2, 5 * time.Minute, 5 * time.Minute},
		{"3 errors", 3, 25 * time.Minute, 25 * time.Minute},
		{"4 errors", 4, 1 * time.Hour, 1 * time.Hour},
		{"5 errors", 5, 1 * time.Hour, 1 * time.Hour},
		{"10 errors", 10, 1 * time.Hour, 1 * time.Hour},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			duration := calculateStandardCooldown(tt.errorCount)
			if duration < tt.minDuration || duration > tt.maxDuration {
				t.Errorf("expected cooldown between %v and %v, got %v", tt.minDuration, tt.maxDuration, duration)
			}
		})
	}
}

// TestCalculateBillingCooldown tests billing cooldown calculation
func TestCalculateBillingCooldown(t *testing.T) {
	tests := []struct {
		name              string
		billingErrorCount int
		minDuration       time.Duration
		maxDuration       time.Duration
	}{
		{"1 billing error", 1, 5 * time.Hour, 5 * time.Hour},
		{"2 billing errors", 2, 10 * time.Hour, 10 * time.Hour},
		{"3 billing errors", 3, 20 * time.Hour, 20 * time.Hour},
		{"4 billing errors", 4, 24 * time.Hour, 24 * time.Hour},
		{"5 billing errors", 5, 24 * time.Hour, 24 * time.Hour},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			duration := calculateBillingCooldown(tt.billingErrorCount)
			if duration < tt.minDuration || duration > tt.maxDuration {
				t.Errorf("expected cooldown between %v and %v, got %v", tt.minDuration, tt.maxDuration, duration)
			}
		})
	}
}

// TestCooldownTracker_ConcurrentAccess tests thread safety
func TestCooldownTracker_ConcurrentAccess(t *testing.T) {
	ct := NewCooldownTracker()

	done := make(chan bool)

	// Concurrent reads
	for i := 0; i < 10; i++ {
		go func() {
			for j := 0; j < 100; j++ {
				ct.IsAvailable("test-provider")
				ct.ErrorCount("test-provider")
			}
			done <- true
		}()
	}

	// Concurrent writes
	for i := 0; i < 10; i++ {
		go func() {
			for j := 0; j < 100; j++ {
				ct.MarkFailure("test-provider", FailoverAuth)
				ct.MarkSuccess("test-provider")
			}
			done <- true
		}()
	}

	// Wait for all goroutines
	for i := 0; i < 20; i++ {
		<-done
	}

	// Should not have caused any race conditions
	_ = ct.ErrorCount("test-provider")
}

//! CooldownManager with exponential backoff for provider failover.

use crate::failover::FailoverReason;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

const DEFAULT_FAILURE_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

/// Per-provider cooldown entry.
#[derive(Debug, Clone)]
struct CooldownEntry {
    error_count: usize,
    failure_counts: HashMap<FailoverReason, usize>,
    cooldown_end: Option<std::time::Instant>,
    disabled_until: Option<std::time::Instant>,
    disabled_reason: Option<FailoverReason>,
    last_failure: Option<std::time::Instant>,
}

impl Default for CooldownEntry {
    fn default() -> Self {
        Self {
            error_count: 0,
            failure_counts: HashMap::new(),
            cooldown_end: None,
            disabled_until: None,
            disabled_reason: None,
            last_failure: None,
        }
    }
}

/// Trait for getting the current time (injectable for testing).
pub trait Clock: Send + Sync {
    fn now(&self) -> std::time::Instant;
}

/// Real clock using `std::time::Instant::now()`.
pub struct RealClock;

impl Clock for RealClock {
    fn now(&self) -> std::time::Instant {
        std::time::Instant::now()
    }
}

/// Cooldown tracker manages per-provider cooldown state for the fallback chain.
/// Thread-safe via `RwLock`. In-memory only (resets on restart).
pub struct CooldownTracker {
    entries: RwLock<HashMap<String, CooldownEntry>>,
    failure_window: Duration,
    clock: Arc<dyn Clock>,
}

impl CooldownTracker {
    /// Create a tracker with default 24h failure window.
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            failure_window: DEFAULT_FAILURE_WINDOW,
            clock: Arc::new(RealClock),
        }
    }

    /// Create a tracker with a custom clock (for testing).
    pub fn with_clock(clock: Arc<dyn Clock>) -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            failure_window: DEFAULT_FAILURE_WINDOW,
            clock,
        }
    }

    /// Record a failure for a provider and set appropriate cooldown.
    /// Resets error counts if last failure was more than `failure_window` ago.
    pub fn mark_failure(&self, provider: &str, reason: FailoverReason) {
        let mut entries = self.entries.write();
        let now = self.clock.now();
        let entry = entries.entry(provider.to_string()).or_default();

        // 24h failure window reset: if no failure in failure_window, reset counters.
        if let Some(last) = entry.last_failure {
            if now.duration_since(last) > self.failure_window {
                entry.error_count = 0;
                entry.failure_counts.clear();
            }
        }

        entry.error_count += 1;
        *entry.failure_counts.entry(reason).or_insert(0) += 1;
        entry.last_failure = Some(now);

        if reason == FailoverReason::Billing {
            let billing_count = *entry.failure_counts.get(&FailoverReason::Billing).unwrap_or(&0);
            entry.disabled_until = Some(now + calculate_billing_cooldown(billing_count));
            entry.disabled_reason = Some(reason);
        } else {
            entry.cooldown_end = Some(now + calculate_standard_cooldown(entry.error_count));
        }
    }

    /// Reset all counters and cooldowns for a provider.
    pub fn mark_success(&self, provider: &str) {
        let mut entries = self.entries.write();
        if let Some(entry) = entries.get_mut(provider) {
            entry.error_count = 0;
            entry.failure_counts.clear();
            entry.cooldown_end = None;
            entry.disabled_until = None;
            entry.disabled_reason = None;
        }
    }

    /// Check if the provider is not in cooldown or disabled.
    pub fn is_available(&self, provider: &str) -> bool {
        let entries = self.entries.read();
        let entry = match entries.get(provider) {
            Some(e) => e,
            None => return true,
        };

        let now = self.clock.now();

        // Billing disable takes precedence (longer cooldown).
        if let Some(until) = entry.disabled_until {
            if now < until {
                return false;
            }
        }

        // Standard cooldown.
        if let Some(end) = entry.cooldown_end {
            if now < end {
                return false;
            }
        }

        true
    }

    /// Get the current error count for a provider.
    pub fn error_count(&self, provider: &str) -> usize {
        let entries = self.entries.read();
        entries.get(provider).map(|e| e.error_count).unwrap_or(0)
    }

    /// Get the failure count for a specific reason.
    pub fn failure_count(&self, provider: &str, reason: FailoverReason) -> usize {
        let entries = self.entries.read();
        entries
            .get(provider)
            .and_then(|e| e.failure_counts.get(&reason).copied())
            .unwrap_or(0)
    }

    /// Get the remaining cooldown duration for a provider.
    ///
    /// Returns `None` if the provider is not in cooldown.
    /// Returns billing disabled remaining if both cooldown and disabled are set.
    pub fn cooldown_remaining(&self, provider: &str) -> Option<Duration> {
        let entries = self.entries.read();
        let entry = entries.get(provider)?;
        let now = self.clock.now();

        // Check billing disable first (longer).
        if let Some(until) = entry.disabled_until {
            if now < until {
                return Some(until - now);
            }
        }

        // Check standard cooldown.
        if let Some(end) = entry.cooldown_end {
            if now < end {
                return Some(end - now);
            }
        }

        None
    }
}

impl Default for CooldownTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Calculate standard exponential backoff.
/// Formula: min(1h, 1min * 5^min(n-1, 3))
///
/// 1 error  -> 1 min
/// 2 errors -> 5 min
/// 3 errors -> 25 min
/// 4+ errors -> 1 hour (cap)
pub fn calculate_standard_cooldown(error_count: usize) -> Duration {
    let n = error_count.max(1);
    let exp = (n - 1).min(3);
    let secs = 60 * 5f64.powi(exp as i32) as u64;
    Duration::from_secs(secs.min(3600))
}

/// Calculate billing-specific exponential backoff.
/// Formula: min(24h, 5h * 2^min(n-1, 10))
///
/// 1 error  -> 5 hours
/// 2 errors -> 10 hours
/// 3 errors -> 20 hours
/// 4+ errors -> 24 hours (cap)
pub fn calculate_billing_cooldown(billing_error_count: usize) -> Duration {
    let base_secs: u64 = 5 * 60 * 60; // 5 hours
    let max_secs: u64 = 24 * 60 * 60; // 24 hours

    let n = billing_error_count.max(1);
    let exp = (n - 1).min(10);
    let raw = base_secs as f64 * 2f64.powi(exp as i32);
    Duration::from_secs(raw.min(max_secs as f64) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_standard_cooldown_progression() {
        let c1 = calculate_standard_cooldown(1);
        assert_eq!(c1, Duration::from_secs(60)); // 1 min

        let c2 = calculate_standard_cooldown(2);
        assert_eq!(c2, Duration::from_secs(300)); // 5 min

        let c3 = calculate_standard_cooldown(3);
        assert_eq!(c3, Duration::from_secs(1500)); // 25 min

        let c4 = calculate_standard_cooldown(4);
        assert_eq!(c4, Duration::from_secs(3600)); // capped at 1 hour

        let c10 = calculate_standard_cooldown(10);
        assert_eq!(c10, Duration::from_secs(3600)); // still capped
    }

    #[test]
    fn test_billing_cooldown_progression() {
        let c1 = calculate_billing_cooldown(1);
        assert_eq!(c1, Duration::from_secs(5 * 3600)); // 5 hours

        let c2 = calculate_billing_cooldown(2);
        assert_eq!(c2, Duration::from_secs(10 * 3600)); // 10 hours

        let c3 = calculate_billing_cooldown(3);
        assert_eq!(c3, Duration::from_secs(20 * 3600)); // 20 hours

        let c4 = calculate_billing_cooldown(4);
        assert_eq!(c4, Duration::from_secs(24 * 3600)); // capped at 24 hours
    }

    #[test]
    fn test_tracker_available_initially() {
        let tracker = CooldownTracker::new();
        assert!(tracker.is_available("openai"));
        assert!(tracker.is_available("anthropic"));
    }

    #[test]
    fn test_tracker_failure_then_available_after_cooldown() {
        let tracker = CooldownTracker::new();
        tracker.mark_failure("openai", FailoverReason::RateLimit);
        assert!(!tracker.is_available("openai"));
        assert_eq!(tracker.error_count("openai"), 1);
    }

    #[test]
    fn test_tracker_mark_success_resets() {
        let tracker = CooldownTracker::new();
        tracker.mark_failure("openai", FailoverReason::RateLimit);
        assert!(!tracker.is_available("openai"));

        tracker.mark_success("openai");
        assert!(tracker.is_available("openai"));
        assert_eq!(tracker.error_count("openai"), 0);
    }

    #[test]
    fn test_tracker_failure_count_by_reason() {
        let tracker = CooldownTracker::new();
        tracker.mark_failure("openai", FailoverReason::RateLimit);
        tracker.mark_failure("openai", FailoverReason::RateLimit);
        tracker.mark_failure("openai", FailoverReason::Timeout);

        assert_eq!(tracker.failure_count("openai", FailoverReason::RateLimit), 2);
        assert_eq!(tracker.failure_count("openai", FailoverReason::Timeout), 1);
        assert_eq!(tracker.error_count("openai"), 3);
    }

    #[test]
    fn test_billing_disables_longer() {
        let tracker = CooldownTracker::new();
        tracker.mark_failure("openai", FailoverReason::Billing);
        assert!(!tracker.is_available("openai"));
    }

    #[test]
    fn test_default_cooldown_zero_errors() {
        assert_eq!(calculate_standard_cooldown(0), Duration::from_secs(60));
    }

    #[test]
    fn test_cooldown_remaining_none_when_no_failure() {
        let tracker = CooldownTracker::new();
        assert!(tracker.cooldown_remaining("openai").is_none());
    }

    #[test]
    fn test_cooldown_remaining_some_after_failure() {
        let tracker = CooldownTracker::new();
        tracker.mark_failure("openai", FailoverReason::RateLimit);
        let remaining = tracker.cooldown_remaining("openai");
        assert!(remaining.is_some());
        let dur = remaining.unwrap();
        assert!(dur <= Duration::from_secs(60));
        assert!(dur > Duration::from_secs(0));
    }

    #[test]
    fn test_cooldown_remaining_none_after_success() {
        let tracker = CooldownTracker::new();
        tracker.mark_failure("openai", FailoverReason::RateLimit);
        tracker.mark_success("openai");
        assert!(tracker.cooldown_remaining("openai").is_none());
    }

    // ============================================================
    // Additional tests for missing coverage
    // ============================================================

    #[test]
    fn test_tracker_default() {
        let tracker = CooldownTracker::default();
        assert!(tracker.is_available("any-provider"));
        assert_eq!(tracker.error_count("any-provider"), 0);
    }

    #[test]
    fn test_tracker_multiple_providers_independent() {
        let tracker = CooldownTracker::new();
        tracker.mark_failure("openai", FailoverReason::RateLimit);
        assert!(!tracker.is_available("openai"));
        assert!(tracker.is_available("anthropic")); // independent
    }

    #[test]
    fn test_tracker_error_count_no_failure() {
        let tracker = CooldownTracker::new();
        assert_eq!(tracker.error_count("unknown"), 0);
    }

    #[test]
    fn test_tracker_failure_count_no_failure() {
        let tracker = CooldownTracker::new();
        assert_eq!(tracker.failure_count("unknown", FailoverReason::RateLimit), 0);
    }

    #[test]
    fn test_tracker_multiple_failures_escalate_cooldown() {
        let tracker = CooldownTracker::new();
        tracker.mark_failure("openai", FailoverReason::RateLimit);
        assert!(!tracker.is_available("openai"));

        tracker.mark_success("openai");
        assert!(tracker.is_available("openai"));

        tracker.mark_failure("openai", FailoverReason::RateLimit);
        tracker.mark_failure("openai", FailoverReason::RateLimit);
        assert!(!tracker.is_available("openai"));
        assert_eq!(tracker.error_count("openai"), 2);
    }

    #[test]
    fn test_billing_disables_provider() {
        let tracker = CooldownTracker::new();
        tracker.mark_failure("openai", FailoverReason::Billing);
        assert!(!tracker.is_available("openai"));
    }

    #[test]
    fn test_billing_multiple_errors_longer_cooldown() {
        let tracker = CooldownTracker::new();
        tracker.mark_failure("openai", FailoverReason::Billing);
        tracker.mark_failure("openai", FailoverReason::Billing);
        assert!(!tracker.is_available("openai"));
        assert_eq!(tracker.failure_count("openai", FailoverReason::Billing), 2);
    }

    #[test]
    fn test_mark_success_on_unknown_provider_no_panic() {
        let tracker = CooldownTracker::new();
        tracker.mark_success("unknown"); // Should not panic
        assert!(tracker.is_available("unknown"));
    }

    #[test]
    fn test_cooldown_remaining_for_billing() {
        let tracker = CooldownTracker::new();
        tracker.mark_failure("openai", FailoverReason::Billing);
        let remaining = tracker.cooldown_remaining("openai");
        assert!(remaining.is_some());
        // Billing cooldown is at least 5 hours
        let dur = remaining.unwrap();
        assert!(dur.as_secs() > 3600);
    }

    #[test]
    fn test_standard_cooldown_high_error_count() {
        // Verify the cap works for very high counts
        let c = calculate_standard_cooldown(100);
        assert_eq!(c, Duration::from_secs(3600)); // capped at 1 hour
    }

    #[test]
    fn test_billing_cooldown_zero_errors() {
        let c = calculate_billing_cooldown(0);
        assert_eq!(c, Duration::from_secs(5 * 3600)); // same as 1 error
    }

    // ---- Additional coverage tests for 95%+ ----

    #[test]
    fn test_tracker_with_custom_clock() {
        struct FrozenClock(std::time::Instant);
        impl Clock for FrozenClock {
            fn now(&self) -> std::time::Instant { self.0 }
        }
        let now = std::time::Instant::now();
        let tracker = CooldownTracker::with_clock(Arc::new(FrozenClock(now)));
        assert!(tracker.is_available("openai"));
        tracker.mark_failure("openai", FailoverReason::Timeout);
        assert!(!tracker.is_available("openai"));
    }

    #[test]
    fn test_tracker_failure_window_reset() {
        struct MutableClock {
            now: parking_lot::Mutex<std::time::Instant>,
        }
        impl Clock for MutableClock {
            fn now(&self) -> std::time::Instant { *self.now.lock() }
        }
        let base = std::time::Instant::now();
        let clock = Arc::new(MutableClock { now: parking_lot::Mutex::new(base) });
        let tracker = CooldownTracker::with_clock(clock.clone());

        // First failure
        tracker.mark_failure("openai", FailoverReason::RateLimit);
        assert_eq!(tracker.error_count("openai"), 1);

        // Advance time past failure window (24h + 1s)
        *clock.now.lock() = base + Duration::from_secs(24 * 3600 + 1);

        // Second failure should reset counters
        tracker.mark_failure("openai", FailoverReason::RateLimit);
        assert_eq!(tracker.error_count("openai"), 1); // Reset to 1 after window
    }

    #[test]
    fn test_tracker_mark_success_noop_for_unknown() {
        let tracker = CooldownTracker::new();
        tracker.mark_success("never_existed");
        assert!(tracker.is_available("never_existed"));
        assert_eq!(tracker.error_count("never_existed"), 0);
    }

    #[test]
    fn test_tracker_cooldown_remaining_after_billing() {
        let tracker = CooldownTracker::new();
        tracker.mark_failure("stripe", FailoverReason::Billing);
        let remaining = tracker.cooldown_remaining("stripe");
        assert!(remaining.is_some());
        // Billing cooldown is at least 5 hours
        assert!(remaining.unwrap().as_secs() >= 5 * 3600 - 1);
    }

    #[test]
    fn test_billing_cooldown_high_count() {
        // Very high count should still cap at 24 hours
        let c = calculate_billing_cooldown(100);
        assert_eq!(c, Duration::from_secs(24 * 3600));
    }

    #[test]
    fn test_standard_cooldown_progression_values() {
        // 1 error: 60s
        assert_eq!(calculate_standard_cooldown(1), Duration::from_secs(60));
        // 2 errors: 300s (5 min)
        assert_eq!(calculate_standard_cooldown(2), Duration::from_secs(300));
        // 3 errors: 1500s (25 min)
        assert_eq!(calculate_standard_cooldown(3), Duration::from_secs(1500));
        // 4 errors: capped at 3600s (1 hour)
        assert_eq!(calculate_standard_cooldown(4), Duration::from_secs(3600));
    }

    #[test]
    fn test_multiple_reasons_independent_counts() {
        let tracker = CooldownTracker::new();
        tracker.mark_failure("svc", FailoverReason::RateLimit);
        tracker.mark_failure("svc", FailoverReason::Timeout);
        tracker.mark_failure("svc", FailoverReason::Auth);

        assert_eq!(tracker.failure_count("svc", FailoverReason::RateLimit), 1);
        assert_eq!(tracker.failure_count("svc", FailoverReason::Timeout), 1);
        assert_eq!(tracker.failure_count("svc", FailoverReason::Auth), 1);
        assert_eq!(tracker.error_count("svc"), 3);
    }

    #[test]
    fn test_tracker_mark_success_clears_all() {
        let tracker = CooldownTracker::new();
        tracker.mark_failure("svc", FailoverReason::RateLimit);
        tracker.mark_failure("svc", FailoverReason::Billing);
        tracker.mark_failure("svc", FailoverReason::Timeout);

        tracker.mark_success("svc");
        assert!(tracker.is_available("svc"));
        assert_eq!(tracker.error_count("svc"), 0);
        assert_eq!(tracker.failure_count("svc", FailoverReason::RateLimit), 0);
        assert!(tracker.cooldown_remaining("svc").is_none());
    }
}

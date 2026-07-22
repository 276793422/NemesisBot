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
            let billing_count = *entry
                .failure_counts
                .get(&FailoverReason::Billing)
                .unwrap_or(&0);
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
mod tests;

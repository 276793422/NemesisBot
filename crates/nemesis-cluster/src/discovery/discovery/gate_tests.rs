//! G3: [`AnnounceWarnGate`] 限频闸门单测。

use super::{AnnounceWarnGate, DRIFT_WARN_THRESHOLD_SECS};
use std::time::Duration;

#[test]
fn test_first_admit_always_allowed() {
    let gate = AnnounceWarnGate::new();
    assert!(gate.admit("node-a"));
}

#[test]
fn test_second_admit_within_cooldown_blocked() {
    let gate = AnnounceWarnGate::new();
    assert!(gate.admit("node-a"));
    assert!(!gate.admit("node-a"), "同一 key 在 cooldown 窗口内应被拦");
}

#[test]
fn test_different_keys_independent() {
    let gate = AnnounceWarnGate::new();
    assert!(gate.admit("node-a"));
    assert!(gate.admit("node-b"), "不同 key 互不影响");
}

#[test]
fn test_custom_short_cooldown_allows_after_expiry() {
    let gate = AnnounceWarnGate::with_cooldown(Duration::from_millis(50));
    assert!(gate.admit("node-a"));
    assert!(!gate.admit("node-a"));
    std::thread::sleep(Duration::from_millis(80));
    assert!(gate.admit("node-a"), "cooldown 过后应重新放行");
}

#[test]
fn test_default_impl() {
    let _: AnnounceWarnGate = AnnounceWarnGate::default();
}

#[test]
fn test_drift_threshold_value() {
    assert_eq!(DRIFT_WARN_THRESHOLD_SECS, 60);
}

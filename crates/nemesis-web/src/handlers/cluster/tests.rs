use super::*;
use std::time::Duration;

#[test]
fn format_duration_zero() {
    assert_eq!(format_duration(Duration::from_secs(0)), "0s");
}

#[test]
fn format_duration_seconds_only() {
    assert_eq!(format_duration(Duration::from_secs(45)), "45s");
}

#[test]
fn format_duration_minutes() {
    assert_eq!(format_duration(Duration::from_secs(125)), "2m 5s");
}

#[test]
fn format_duration_hours() {
    assert_eq!(format_duration(Duration::from_secs(3725)), "1h 2m");
}

#[test]
fn format_duration_days() {
    assert_eq!(format_duration(Duration::from_secs(90061)), "1d 1h");
}

#[test]
fn format_ago_branches() {
    assert_eq!(format_ago(Duration::from_secs(5)), "5s ago");
    assert_eq!(format_ago(Duration::from_secs(120)), "2m ago");
    assert_eq!(format_ago(Duration::from_secs(7200)), "2h ago");
    assert_eq!(format_ago(Duration::from_secs(86400 * 3)), "3d ago");
}

#[test]
fn format_bytes_units() {
    assert_eq!(format_bytes(512), "512B");
    assert_eq!(format_bytes(2048), "2KB");
    assert_eq!(format_bytes(1024 * 1024 * 5), "5.0MB");
}

#[test]
fn truncate_str_no_truncation_when_short_enough() {
    assert_eq!(truncate_str("hello", 10), "hello");
    assert_eq!(truncate_str("hello", 5), "hello");
}

#[test]
fn truncate_str_ascii_truncates_with_ellipsis() {
    assert_eq!(truncate_str("hello world", 5), "hello...");
}

#[test]
fn truncate_str_utf8_lands_on_char_boundary() {
    // Each emoji is 4 bytes; max_len=6 must not split a multi-byte char.
    let out = truncate_str("🎯🎯🎯🎯", 6);
    assert!(out.ends_with("..."));
    // Should contain exactly one full emoji before the ellipsis (4 bytes ≤ 6).
    assert_eq!(out, "🎯...");
}

#[test]
fn test_broadcast_flag_passes_on_any_platform() {
    // Binding 0.0.0.0:0 always succeeds; setting SO_BROADCAST is universally
    // supported. This verifies the success-shape of the diagnostic helper.
    let r = test_broadcast_flag();
    assert_eq!(r["name"], "broadcast_flag");
    assert_eq!(r["pass"], true);
}

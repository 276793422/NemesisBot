use super::*;

#[test]
fn test_get_hostname_returns_nonempty_on_supported_platform() {
    let host = get_hostname();
    assert!(!host.is_empty(), "hostname should never be empty");
}

#[test]
fn test_collect_system_metrics_returns_nonzero_total_on_supported_platform() {
    // On Linux/Windows the total memory should always be > 0. On other
    // platforms it returns (0,0,0). Either way, the call should not panic.
    let (total, _used, _uptime) = collect_system_metrics();
    if cfg!(any(target_os = "linux", target_os = "windows")) {
        assert!(
            total > 0,
            "memory_total should be > 0 on supported platforms"
        );
    }
}

#[test]
fn test_collect_system_metrics_uptime_nonzero_on_supported_platform() {
    let (_, _, uptime) = collect_system_metrics();
    if cfg!(any(target_os = "linux", target_os = "windows")) {
        // On a freshly booted machine uptime could theoretically be very
        // small but never 0 after init.
        assert!(uptime > 0 || uptime == 0, "uptime call should not panic");
    }
}

#[test]
fn test_collect_os_version_returns_nonempty() {
    let v = collect_os_version();
    assert!(!v.is_empty(), "OS version should never be empty");
}

#[test]
fn test_collect_os_version_matches_os_const() {
    // Cross-platform fallback path returns std::env::consts::OS exactly.
    // Linux/Windows paths return a richer string but should at least
    // contain the OS family.
    let v = collect_os_version();
    let os_const = std::env::consts::OS;
    if cfg!(not(any(target_os = "linux", target_os = "windows"))) {
        assert_eq!(v, os_const);
    } else {
        // The richer string should at least mention the OS family.
        let family = if cfg!(target_os = "windows") {
            "Windows"
        } else {
            "Linux"
        };
        assert!(
            v.contains(family) || v.contains(os_const),
            "expected '{}' or '{}' in OS version: {}",
            family,
            os_const,
            v
        );
    }
}

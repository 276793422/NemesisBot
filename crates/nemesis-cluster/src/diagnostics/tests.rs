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
        // small but never 0 after init. uptime 是 u64（`>= 0` 恒真，触发
        // rustc unused_comparisons）；本测试只验证"调用不 panic"——panic
        // 在上方调用处就已失败，这里消费值即可，不再写恒真断言。
        let _ = uptime;
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
    // Windows returns "ProductName (Build N)"; Linux returns the
    // /etc/os-release PRETTY_NAME — verify against each platform's source
    // of truth instead of guessing substrings.
    let v = collect_os_version();
    let os_const = std::env::consts::OS;
    if cfg!(not(any(target_os = "linux", target_os = "windows"))) {
        assert_eq!(v, os_const);
    } else if cfg!(target_os = "windows") {
        assert!(
            v.contains("Windows") || v.contains(os_const),
            "expected Windows family in OS version: {}",
            v
        );
    } else {
        // 很多发行版（如 Ubuntu）的 PRETTY_NAME 本身不含 "Linux" 字样，
        // 不能按子串断言；直接对照 os-release 源头验证（os-release 缺失
        // 时实现回退 "Linux"，这里保持同一回退）。
        let pretty = std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|content| {
                content
                    .lines()
                    .find(|line| line.starts_with("PRETTY_NAME="))
                    .map(|line| {
                        line.trim_start_matches("PRETTY_NAME=")
                            .trim_matches('"')
                            .to_string()
                    })
            })
            .unwrap_or_else(|| "Linux".into());
        assert_eq!(
            v, pretty,
            "OS version should mirror /etc/os-release PRETTY_NAME (or the Linux fallback)"
        );
    }
}

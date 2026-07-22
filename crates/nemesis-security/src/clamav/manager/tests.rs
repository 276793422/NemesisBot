use super::*;
use std::time::Duration;

#[test]
fn test_parse_duration_24h() {
    assert_eq!(parse_duration_string("24h"), Duration::from_secs(24 * 3600));
}

#[test]
fn test_parse_duration_1h30m() {
    assert_eq!(parse_duration_string("1h30m"), Duration::from_secs(90 * 60));
}

#[test]
fn test_parse_duration_30m() {
    assert_eq!(parse_duration_string("30m"), Duration::from_secs(30 * 60));
}

#[test]
fn test_parse_duration_1d() {
    assert_eq!(parse_duration_string("1d"), Duration::from_secs(86400));
}

#[test]
fn test_parse_duration_seconds() {
    assert_eq!(parse_duration_string("45s"), Duration::from_secs(45));
}

#[test]
fn test_parse_duration_composite() {
    assert_eq!(
        parse_duration_string("1d2h30m15s"),
        Duration::from_secs(86400 + 7200 + 1800 + 15)
    );
}

#[test]
fn test_parse_duration_empty() {
    assert_eq!(parse_duration_string(""), Duration::ZERO);
}

#[test]
fn test_parse_duration_invalid() {
    assert_eq!(parse_duration_string("abc"), Duration::ZERO);
}

#[test]
fn test_parse_duration_invalid_mixed() {
    assert_eq!(parse_duration_string("1x"), Duration::ZERO);
}

#[test]
fn test_manager_new() {
    let config = ManagerConfig {
        enabled: false,
        clamav_path: String::new(),
        data_dir: String::new(),
        address: String::new(),
        scanner: None,
        update_interval: String::new(),
    };
    let manager = Manager::new(config);
    assert!(!manager.is_running());
    assert!(manager.hook().is_none());
    assert!(manager.scanner().is_none());
}

#[tokio::test]
async fn test_manager_get_stats_not_started() {
    let config = ManagerConfig {
        enabled: false,
        clamav_path: String::new(),
        data_dir: String::new(),
        address: String::new(),
        scanner: None,
        update_interval: String::new(),
    };
    let manager = Manager::new(config);
    let stats = manager.get_stats().await;
    assert_eq!(stats["enabled"], false);
    assert_eq!(stats["started"], false);
    assert!(stats.get("scanner").is_none());
}

#[tokio::test]
async fn test_manager_stop_when_not_started() {
    let config = ManagerConfig {
        enabled: false,
        clamav_path: String::new(),
        data_dir: String::new(),
        address: String::new(),
        scanner: None,
        update_interval: String::new(),
    };
    let manager = Manager::new(config);
    // Should succeed without error even when not started
    let result = manager.stop().await;
    assert!(result.is_ok());
}

#[test]
fn test_manager_config_debug() {
    let config = ManagerConfig {
        enabled: true,
        clamav_path: "/usr/bin".to_string(),
        data_dir: "/tmp/clamav".to_string(),
        address: "127.0.0.1:3310".to_string(),
        scanner: None,
        update_interval: "24h".to_string(),
    };
    let debug = format!("{:?}", config);
    assert!(debug.contains("enabled"));
    assert!(debug.contains("/usr/bin"));
}

#[tokio::test]
async fn test_manager_start_disabled() {
    let mut manager = Manager::new(ManagerConfig {
        enabled: false,
        clamav_path: String::new(),
        data_dir: String::new(),
        address: String::new(),
        scanner: None,
        update_interval: String::new(),
    });
    let result = manager.start().await;
    assert!(result.is_ok());
    assert!(!manager.is_running());
}

#[tokio::test]
async fn test_manager_start_already_started() {
    let mut manager = Manager::new(ManagerConfig {
        enabled: false,
        clamav_path: String::new(),
        data_dir: String::new(),
        address: String::new(),
        scanner: None,
        update_interval: String::new(),
    });
    manager.started.store(true, Ordering::SeqCst);
    let result = manager.start().await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("already started"));
}

#[tokio::test]
async fn test_manager_start_missing_clamav() {
    let mut manager = Manager::new(ManagerConfig {
        enabled: true,
        clamav_path: "/nonexistent/path".to_string(),
        data_dir: String::new(),
        address: String::new(),
        scanner: None,
        update_interval: String::new(),
    });
    // This will fail because the path doesn't exist
    let result = manager.start().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_manager_get_stats_with_updater() {
    let mut manager = Manager::new(ManagerConfig {
        enabled: false,
        clamav_path: String::new(),
        data_dir: String::new(),
        address: String::new(),
        scanner: None,
        update_interval: String::new(),
    });
    // Manually inject an updater with a recent last_update
    let updater = Arc::new(Updater::new(UpdaterConfig {
        clamav_path: String::new(),
        database_dir: String::new(),
        config_file: String::new(),
        update_interval: Duration::from_secs(3600),
        mirror_urls: Vec::new(),
    }));
    manager.updater = Some(updater);
    let stats = manager.get_stats().await;
    assert_eq!(stats["enabled"], false);
    // last_update_secs_ago should not be present since last_update is None
}

#[tokio::test]
async fn test_manager_hook_and_scanner_none_before_start() {
    let manager = Manager::new(ManagerConfig {
        enabled: false,
        clamav_path: String::new(),
        data_dir: String::new(),
        address: String::new(),
        scanner: None,
        update_interval: String::new(),
    });
    assert!(manager.hook().is_none());
    assert!(manager.scanner().is_none());
}

// ============================================================
// Additional coverage tests
// ============================================================

#[test]
fn test_parse_duration_2h() {
    assert_eq!(parse_duration_string("2h"), Duration::from_secs(7200));
}

#[test]
fn test_parse_duration_15m() {
    assert_eq!(parse_duration_string("15m"), Duration::from_secs(900));
}

#[test]
fn test_parse_duration_90s() {
    assert_eq!(parse_duration_string("90s"), Duration::from_secs(90));
}

#[test]
fn test_parse_duration_7d() {
    assert_eq!(parse_duration_string("7d"), Duration::from_secs(7 * 86400));
}

#[test]
fn test_parse_duration_1d12h() {
    assert_eq!(
        parse_duration_string("1d12h"),
        Duration::from_secs(86400 + 43200)
    );
}

#[test]
fn test_parse_duration_zero() {
    assert_eq!(parse_duration_string("0s"), Duration::from_secs(0));
}

#[test]
fn test_parse_duration_only_digits() {
    // Just digits without unit suffix -> current_num stays non-zero but is never added
    // After loop, total_secs remains 0, returns Duration::ZERO
    assert_eq!(parse_duration_string("123"), Duration::from_secs(0));
}

#[test]
fn test_manager_config_custom_scanner() {
    let scanner_cfg = ScannerConfig {
        enabled: true,
        address: "127.0.0.1:3310".to_string(),
        scan_on_write: false,
        scan_on_download: true,
        scan_on_exec: true,
        max_file_size: 100 * 1024 * 1024,
        timeout: Duration::from_secs(120),
    };
    let config = ManagerConfig {
        enabled: true,
        clamav_path: "/opt/clamav".to_string(),
        data_dir: "/tmp/clamav-data".to_string(),
        address: "127.0.0.1:3310".to_string(),
        scanner: Some(scanner_cfg),
        update_interval: "12h".to_string(),
    };
    let manager = Manager::new(config);
    assert!(!manager.is_running());
    assert!(manager.hook().is_none());
    assert!(manager.scanner().is_none());
}

#[tokio::test]
async fn test_manager_stop_when_disabled() {
    let manager = Manager::new(ManagerConfig {
        enabled: false,
        clamav_path: String::new(),
        data_dir: String::new(),
        address: String::new(),
        scanner: None,
        update_interval: String::new(),
    });
    // Stop should succeed when disabled and not started
    let result = manager.stop().await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_manager_get_stats_structure() {
    let config = ManagerConfig {
        enabled: true,
        clamav_path: String::new(),
        data_dir: String::new(),
        address: String::new(),
        scanner: None,
        update_interval: String::new(),
    };
    let manager = Manager::new(config);
    let stats = manager.get_stats().await;

    // Verify JSON structure
    assert!(stats.is_object());
    assert_eq!(stats["enabled"], true);
    assert_eq!(stats["started"], false);
    // No scanner -> no scanner stats
    assert!(stats.get("scanner").is_none());
}

#[test]
fn test_manager_new_with_enabled() {
    let config = ManagerConfig {
        enabled: true,
        clamav_path: String::new(),
        data_dir: String::new(),
        address: "127.0.0.1:9999".to_string(),
        scanner: None,
        update_interval: "6h".to_string(),
    };
    let manager = Manager::new(config);
    assert!(!manager.is_running());
}

#[tokio::test]
async fn test_manager_start_disabled_returns_ok() {
    let mut manager = Manager::new(ManagerConfig {
        enabled: false,
        clamav_path: String::new(),
        data_dir: String::new(),
        address: String::new(),
        scanner: None,
        update_interval: String::new(),
    });
    let result = manager.start().await;
    assert!(result.is_ok());
    // Even though start succeeded, is_running is still false because disabled
    assert!(!manager.is_running());
}

#[test]
fn test_parse_duration_complex_composite() {
    // Test 2d8h45m30s
    let expected = 2 * 86400 + 8 * 3600 + 45 * 60 + 30;
    assert_eq!(
        parse_duration_string("2d8h45m30s"),
        Duration::from_secs(expected)
    );
}

#[test]
fn test_parse_duration_invalid_char_in_middle() {
    assert_eq!(parse_duration_string("10x5m"), Duration::ZERO);
}

#[test]
fn test_parse_duration_only_days() {
    assert_eq!(parse_duration_string("3d"), Duration::from_secs(3 * 86400));
}

#[test]
fn test_parse_duration_only_hours() {
    assert_eq!(parse_duration_string("12h"), Duration::from_secs(12 * 3600));
}

#[test]
fn test_parse_duration_only_minutes() {
    assert_eq!(parse_duration_string("45m"), Duration::from_secs(45 * 60));
}

#[test]
fn test_parse_duration_only_seconds() {
    assert_eq!(parse_duration_string("30s"), Duration::from_secs(30));
}

#[test]
fn test_parse_duration_hours_and_minutes() {
    let expected = 2 * 3600 + 30 * 60;
    assert_eq!(
        parse_duration_string("2h30m"),
        Duration::from_secs(expected)
    );
}

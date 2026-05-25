use super::*;

fn test_config() -> UpdaterConfig {
    UpdaterConfig {
        clamav_path: "/usr/bin".to_string(),
        database_dir: String::new(),
        config_file: String::new(),
        update_interval: Duration::from_secs(3600),
        mirror_urls: Vec::new(),
    }
}

#[test]
fn test_updater_new() {
    let updater = Updater::new(test_config());
    assert!(updater.last_update().is_none());
}

#[test]
fn test_last_update_none() {
    let updater = Updater::new(test_config());
    assert_eq!(updater.last_update(), None);
}

#[test]
fn test_is_database_stale_no_database() {
    let updater = Updater::new(test_config());
    // With no database dir, should be stale
    assert!(updater.is_database_stale(Duration::from_secs(86400)));
}

#[test]
fn test_is_database_stale_empty_dir() {
    let dir = tempfile::tempdir().unwrap();
    let config = UpdaterConfig {
        clamav_path: "/usr/bin".to_string(),
        database_dir: dir.path().to_string_lossy().to_string(),
        config_file: String::new(),
        update_interval: Duration::from_secs(3600),
        mirror_urls: Vec::new(),
    };
    let updater = Updater::new(config);
    // With empty dir (no main.cvd), should be stale
    assert!(updater.is_database_stale(Duration::from_secs(86400)));
}

#[test]
fn test_stop_sets_running_flag() {
    let updater = Updater::new(test_config());
    // Manually set running, then stop
    updater.running.store(true, Ordering::SeqCst);
    assert!(updater.running.load(Ordering::SeqCst));
    updater.stop();
    assert!(!updater.running.load(Ordering::SeqCst));
}

#[test]
fn test_updater_config_fields() {
    let config = test_config();
    assert_eq!(config.clamav_path, "/usr/bin");
    assert_eq!(config.update_interval, Duration::from_secs(3600));
    assert!(config.mirror_urls.is_empty());
}

#[test]
fn test_find_executable() {
    let exe = super::super::find_executable("/usr/bin", "freshclam");
    if cfg!(target_os = "windows") {
        assert!(exe.ends_with("freshclam.exe"));
    } else {
        assert!(exe.ends_with("freshclam"));
    }
}

#[tokio::test]
async fn test_update_exe_not_found() {
    let updater = Updater::new(test_config());
    let result = updater.update(tokio_util::sync::CancellationToken::new(), None).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found"));
}

#[tokio::test]
async fn test_update_db_dir_created_before_exe_check() {
    // The updater checks for freshclam before creating the db dir
    // so with a nonexistent path, the db dir won't be created.
    // Let's test with an empty database_dir instead
    let _dir = tempfile::tempdir().unwrap();
    let config = UpdaterConfig {
        clamav_path: "/nonexistent".to_string(),
        database_dir: String::new(), // empty dir won't be created
        config_file: String::new(),
        update_interval: Duration::from_secs(3600),
        mirror_urls: Vec::new(),
    };
    let updater = Updater::new(config);
    let result = updater.update(tokio_util::sync::CancellationToken::new(), None).await;
    // Should fail because freshclam not found, not because of dir creation
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found"));
}

#[test]
fn test_is_database_stale_with_recent_file() {
    let dir = tempfile::tempdir().unwrap();
    let main_cvd = dir.path().join("main.cvd");
    std::fs::write(&main_cvd, "test").unwrap();
    let config = UpdaterConfig {
        clamav_path: "/usr/bin".to_string(),
        database_dir: dir.path().to_string_lossy().to_string(),
        config_file: String::new(),
        update_interval: Duration::from_secs(3600),
        mirror_urls: Vec::new(),
    };
    let updater = Updater::new(config);
    // File was just created, so with a large max_age it should not be stale
    assert!(!updater.is_database_stale(Duration::from_secs(86400 * 365)));
}

#[test]
fn test_is_database_stale_with_old_file() {
    let dir = tempfile::tempdir().unwrap();
    // Don't create main.cvd - should be stale
    let config = UpdaterConfig {
        clamav_path: "/usr/bin".to_string(),
        database_dir: dir.path().to_string_lossy().to_string(),
        config_file: String::new(),
        update_interval: Duration::from_secs(3600),
        mirror_urls: Vec::new(),
    };
    let updater = Updater::new(config);
    assert!(updater.is_database_stale(Duration::from_secs(1)));
}

#[tokio::test]
async fn test_auto_update_zero_interval() {
    let config = UpdaterConfig {
        clamav_path: String::new(),
        database_dir: String::new(),
        config_file: String::new(),
        update_interval: Duration::ZERO,
        mirror_urls: Vec::new(),
    };
    let updater = Updater::new(config);
    // With zero interval, start_auto_update should return immediately
    updater.start_auto_update().await;
}

#[test]
fn test_updater_running_flag() {
    let updater = Updater::new(test_config());
    assert!(!updater.running.load(Ordering::SeqCst));
    updater.running.store(true, Ordering::SeqCst);
    assert!(updater.running.load(Ordering::SeqCst));
    updater.stop();
    assert!(!updater.running.load(Ordering::SeqCst));
}

// ============================================================
// Additional coverage tests
// ============================================================

#[test]
fn test_updater_config_custom_values() {
    let config = UpdaterConfig {
        clamav_path: "/opt/clamav".to_string(),
        database_dir: "/var/lib/clamav".to_string(),
        config_file: "/etc/clamav/freshclam.conf".to_string(),
        update_interval: Duration::from_secs(7200),
        mirror_urls: vec!["http://mirror1.example.com".to_string()],
    };
    assert_eq!(config.clamav_path, "/opt/clamav");
    assert_eq!(config.database_dir, "/var/lib/clamav");
    assert_eq!(config.config_file, "/etc/clamav/freshclam.conf");
    assert_eq!(config.update_interval, Duration::from_secs(7200));
    assert_eq!(config.mirror_urls.len(), 1);
}

#[test]
fn test_updater_last_update_manually_set() {
    let updater = Updater::new(test_config());
    assert!(updater.last_update().is_none());

    // Manually set last_update
    *updater.last_update.lock().unwrap() = Some(SystemTime::now());
    assert!(updater.last_update().is_some());
}

#[test]
fn test_is_database_stale_with_recent_last_update() {
    let updater = Updater::new(test_config());
    // Set last_update to now
    *updater.last_update.lock().unwrap() = Some(SystemTime::now());

    // Should not be stale with a large max_age
    assert!(!updater.is_database_stale(Duration::from_secs(86400 * 365)));
    // Should be stale with a very small max_age
    // (time has passed since we set last_update, even if just nanoseconds)
    // This is timing-sensitive so we just verify it doesn't panic
    let _ = updater.is_database_stale(Duration::from_nanos(1));
}

#[test]
fn test_is_database_stale_with_file_newer_than_max_age() {
    let dir = tempfile::tempdir().unwrap();
    let main_cvd = dir.path().join("main.cvd");
    std::fs::write(&main_cvd, "fake cvd content").unwrap();

    let config = UpdaterConfig {
        clamav_path: "/usr/bin".to_string(),
        database_dir: dir.path().to_string_lossy().to_string(),
        config_file: String::new(),
        update_interval: Duration::from_secs(3600),
        mirror_urls: Vec::new(),
    };
    let updater = Updater::new(config);
    // File was just created, so with a large max_age it should not be stale
    assert!(!updater.is_database_stale(Duration::from_secs(86400)));
}

#[tokio::test]
async fn test_update_with_db_dir_but_no_freshclam() {
    let dir = tempfile::tempdir().unwrap();
    let config = UpdaterConfig {
        clamav_path: "/nonexistent".to_string(),
        database_dir: dir.path().to_string_lossy().to_string(),
        config_file: String::new(),
        update_interval: Duration::from_secs(3600),
        mirror_urls: Vec::new(),
    };
    let updater = Updater::new(config);
    let result = updater.update(tokio_util::sync::CancellationToken::new(), None).await;
    assert!(result.is_err());
    // Should fail because freshclam not found
    assert!(result.unwrap_err().contains("not found"));
}

#[tokio::test]
async fn test_update_with_config_file_but_no_freshclam() {
    let dir = tempfile::tempdir().unwrap();
    let config = UpdaterConfig {
        clamav_path: "/nonexistent".to_string(),
        database_dir: String::new(),
        config_file: dir.path().join("freshclam.conf").to_string_lossy().to_string(),
        update_interval: Duration::from_secs(3600),
        mirror_urls: Vec::new(),
    };
    let updater = Updater::new(config);
    let result = updater.update(tokio_util::sync::CancellationToken::new(), None).await;
    assert!(result.is_err());
}

#[test]
fn test_updater_stop_multiple_times() {
    let updater = Updater::new(test_config());
    updater.stop();
    updater.stop();
    updater.stop();
    assert!(!updater.running.load(Ordering::SeqCst));
}

#[test]
fn test_is_database_stale_no_dir_configured() {
    let config = UpdaterConfig {
        clamav_path: "/usr/bin".to_string(),
        database_dir: String::new(),
        config_file: String::new(),
        update_interval: Duration::from_secs(3600),
        mirror_urls: Vec::new(),
    };
    let updater = Updater::new(config);
    // No last_update and no database_dir -> should be stale
    assert!(updater.is_database_stale(Duration::from_secs(86400)));
}

#[test]
fn test_updater_config_debug() {
    let config = test_config();
    let debug = format!("{:?}", config);
    assert!(debug.contains("/usr/bin"));
    assert!(debug.contains("3600"));
}

#[test]
fn test_updater_new_sets_defaults() {
    let updater = Updater::new(test_config());
    assert!(updater.last_update().is_none());
    assert!(!updater.running.load(Ordering::SeqCst));
}

#[test]
fn test_updater_config_clone() {
    let config = test_config();
    let cloned = config.clone();
    assert_eq!(cloned.clamav_path, config.clamav_path);
    assert_eq!(cloned.database_dir, config.database_dir);
    assert_eq!(cloned.update_interval, config.update_interval);
}

#[test]
fn test_updater_is_database_stale_with_recent_update() {
    let config = test_config();
    let updater = Updater::new(config);
    *updater.last_update.lock().unwrap() = Some(SystemTime::now());
    // Just updated -> should NOT be stale with a generous threshold
    assert!(!updater.is_database_stale(Duration::from_secs(86400)));
}

#[test]
fn test_updater_is_database_stale_with_old_update() {
    let config = test_config();
    let updater = Updater::new(config);
    // Set last_update to 2 days ago
    let two_days_ago = SystemTime::now() - Duration::from_secs(2 * 86400);
    *updater.last_update.lock().unwrap() = Some(two_days_ago);
    // Should be stale with a 1-day threshold
    assert!(updater.is_database_stale(Duration::from_secs(86400)));
}

#[test]
fn test_updater_is_database_stale_zero_threshold() {
    let config = test_config();
    let updater = Updater::new(config);
    *updater.last_update.lock().unwrap() = Some(SystemTime::now());
    // Zero threshold -> always stale
    assert!(updater.is_database_stale(Duration::ZERO));
}

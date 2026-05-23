use super::*;

#[test]
fn test_generate_clamd_config() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = DaemonConfig {
        config_file: dir.path().join("clamd.conf").to_string_lossy().to_string(),
        database_dir: dir.path().join("db").to_string_lossy().to_string(),
        listen_addr: "127.0.0.1:3310".to_string(),
        temp_dir: dir.path().join("tmp").to_string_lossy().to_string(),
        ..Default::default()
    };
    generate_clamd_config(&cfg).unwrap();
    let content = fs::read_to_string(&cfg.config_file).unwrap();
    assert!(content.contains("TCPSocket 3310"));
    assert!(content.contains("ScanPE yes"));
}

#[test]
fn test_generate_freshclam_config() {
    let dir = tempfile::tempdir().unwrap();
    let config_file = dir.path().join("freshclam.conf").to_string_lossy().to_string();
    let db_dir = dir.path().join("db").to_string_lossy().to_string();
    generate_freshclam_config(&db_dir, &config_file).unwrap();
    let content = fs::read_to_string(&config_file).unwrap();
    assert!(content.contains("DatabaseMirror database.clamav.net"));
}

#[test]
fn test_empty_config_path_fails() {
    let cfg = DaemonConfig::default();
    assert!(generate_clamd_config(&cfg).is_err());
}

#[test]
fn test_daemon_config_defaults() {
    let cfg = DaemonConfig::default();
    assert!(cfg.clamav_path.is_empty());
    assert!(cfg.config_file.is_empty());
    assert!(cfg.database_dir.is_empty());
    assert_eq!(cfg.listen_addr, "127.0.0.1:3310");
    assert!(cfg.temp_dir.is_empty());
    assert_eq!(cfg.startup_timeout_secs, 120);
}

#[test]
fn test_generate_clamd_config_custom_port() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = DaemonConfig {
        config_file: dir.path().join("clamd.conf").to_string_lossy().to_string(),
        listen_addr: "0.0.0.0:9999".to_string(),
        ..Default::default()
    };
    generate_clamd_config(&cfg).unwrap();
    let content = fs::read_to_string(&cfg.config_file).unwrap();
    assert!(content.contains("TCPSocket 9999"));
    assert!(content.contains("TCPAddr 0.0.0.0"));
}

#[test]
fn test_generate_clamd_config_includes_scan_options() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = DaemonConfig {
        config_file: dir.path().join("clamd.conf").to_string_lossy().to_string(),
        listen_addr: "127.0.0.1:3310".to_string(),
        ..Default::default()
    };
    generate_clamd_config(&cfg).unwrap();
    let content = fs::read_to_string(&cfg.config_file).unwrap();
    assert!(content.contains("ScanPE yes"));
    assert!(content.contains("ScanELF yes"));
    assert!(content.contains("ScanArchive yes"));
    assert!(content.contains("MaxScanSize 100M"));
    assert!(content.contains("MaxFileSize 50M"));
}

#[test]
fn test_generate_freshclam_config_creates_db_dir() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("database");
    let config_file = dir.path().join("freshclam.conf").to_string_lossy().to_string();
    generate_freshclam_config(&db_dir.to_string_lossy(), &config_file).unwrap();
    assert!(db_dir.exists());
    let content = fs::read_to_string(&config_file).unwrap();
    assert!(content.contains("DatabaseMirror database.clamav.net"));
    assert!(content.contains("Checks 24"));
}

#[test]
fn test_generate_freshclam_config_empty_path_fails() {
    let result = generate_freshclam_config("/tmp", "");
    assert!(result.is_err());
}

#[test]
fn test_detect_clamav_path_returns_none_or_some() {
    // Just verify it doesn't panic; actual result depends on environment
    let _ = detect_clamav_path();
}

#[test]
fn test_generate_clamd_config_with_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = DaemonConfig {
        config_file: dir.path().join("clamd.conf").to_string_lossy().to_string(),
        database_dir: dir.path().join("db").to_string_lossy().to_string(),
        temp_dir: dir.path().join("tmp").to_string_lossy().to_string(),
        listen_addr: "127.0.0.1:3310".to_string(),
        ..Default::default()
    };
    generate_clamd_config(&cfg).unwrap();
    let content = fs::read_to_string(&cfg.config_file).unwrap();
    assert!(content.contains("DatabaseDirectory"));
    assert!(content.contains("TemporaryDirectory"));
}

#[test]
fn test_generate_clamd_config_empty_listen_addr() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = DaemonConfig {
        config_file: dir.path().join("clamd.conf").to_string_lossy().to_string(),
        listen_addr: String::new(),
        ..Default::default()
    };
    generate_clamd_config(&cfg).unwrap();
    let content = fs::read_to_string(&cfg.config_file).unwrap();
    // Should use default 127.0.0.1:3310
    assert!(content.contains("TCPSocket 3310"));
    assert!(content.contains("TCPAddr 127.0.0.1"));
}

#[test]
fn test_generate_clamd_config_no_colon_in_addr() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = DaemonConfig {
        config_file: dir.path().join("clamd.conf").to_string_lossy().to_string(),
        listen_addr: "onlyhost".to_string(),
        ..Default::default()
    };
    generate_clamd_config(&cfg).unwrap();
    let content = fs::read_to_string(&cfg.config_file).unwrap();
    assert!(content.contains("TCPAddr onlyhost"));
    assert!(content.contains("TCPSocket 3310")); // default port
}

#[test]
fn test_generate_clamd_config_empty_db_and_temp() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = DaemonConfig {
        config_file: dir.path().join("clamd.conf").to_string_lossy().to_string(),
        database_dir: String::new(),
        temp_dir: String::new(),
        listen_addr: "127.0.0.1:3310".to_string(),
        ..Default::default()
    };
    generate_clamd_config(&cfg).unwrap();
    let content = fs::read_to_string(&cfg.config_file).unwrap();
    assert!(!content.contains("DatabaseDirectory"));
    assert!(!content.contains("TemporaryDirectory"));
}

#[test]
fn test_generate_freshclam_config_empty_db_dir() {
    let dir = tempfile::tempdir().unwrap();
    let config_file = dir.path().join("freshclam.conf").to_string_lossy().to_string();
    generate_freshclam_config("", &config_file).unwrap();
    let content = fs::read_to_string(&config_file).unwrap();
    assert!(content.contains("DatabaseMirror"));
    // No DatabaseDirectory line
    assert!(!content.contains("DatabaseDirectory"));
}

#[test]
fn test_generate_clamd_config_invalid_path() {
    let cfg = DaemonConfig {
        config_file: "/nonexistent/deep/path/config.conf".to_string(),
        listen_addr: "127.0.0.1:3310".to_string(),
        ..Default::default()
    };
    // On Windows this might succeed if the path is valid
    let result = generate_clamd_config(&cfg);
    // Result depends on filesystem permissions
    let _ = result;
}

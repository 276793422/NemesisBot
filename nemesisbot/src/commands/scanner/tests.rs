use super::*;
use tempfile::TempDir;

#[test]
fn test_scanner_full_config_default() {
    let cfg = ScannerFullConfig::default();
    assert!(cfg.enabled.is_empty());
    assert!(cfg.engines.is_empty());
}

#[test]
fn test_clamav_engine_config_default() {
    let cfg = ClamAVEngineConfig::default();
    assert_eq!(cfg.address, "127.0.0.1:3310");
    assert!(cfg.url.is_empty());
    assert!(cfg.clamav_path.is_empty());
    assert!(cfg.data_dir.is_empty());
    assert_eq!(cfg.scan_on_write, true);
    assert_eq!(cfg.scan_on_download, false);
    assert_eq!(cfg.scan_on_exec, true);
    assert_eq!(cfg.max_file_size, 52428800);
    assert_eq!(cfg.update_interval, "24h");
    assert!(!cfg.skip_extensions.is_empty());
    assert!(cfg.state.install_status.is_empty());
}

#[test]
fn test_engine_state_default() {
    let state = EngineState::default();
    assert!(state.install_status.is_empty());
    assert!(state.install_error.is_empty());
    assert!(state.db_status.is_empty());
    assert!(state.last_install_attempt.is_empty());
    assert!(state.last_db_update.is_empty());
}

#[test]
fn test_default_skip_extensions() {
    let exts = default_skip_extensions();
    assert!(exts.contains(&".txt".to_string()));
    assert!(exts.contains(&".md".to_string()));
    assert!(exts.contains(&".json".to_string()));
    assert!(exts.contains(&".log".to_string()));
    assert!(exts.contains(&".css".to_string()));
}

#[test]
fn test_load_scanner_config_no_file() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    let cfg = load_scanner_config(&path).unwrap();
    assert!(cfg.enabled.is_empty());
    assert!(cfg.engines.is_empty());
}

#[test]
fn test_load_scanner_config_valid_file() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    let data = serde_json::json!({
        "enabled": ["clamav"],
        "engines": {
            "clamav": {
                "address": "127.0.0.1:3310",
                "state": {
                    "install_status": "installed",
                    "db_status": "ready"
                }
            }
        }
    });
    std::fs::write(&path, serde_json::to_string(&data).unwrap()).unwrap();

    let cfg = load_scanner_config(&path).unwrap();
    assert_eq!(cfg.enabled.len(), 1);
    assert_eq!(cfg.enabled[0], "clamav");
    assert!(cfg.engines.contains_key("clamav"));
}

#[test]
fn test_save_and_load_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config").join("config.scanner.json");

    let mut cfg = ScannerFullConfig::default();
    cfg.enabled.push("clamav".to_string());
    let engine = ClamAVEngineConfig::default();
    cfg.engines
        .insert("clamav".to_string(), serde_json::to_value(engine).unwrap());

    save_scanner_config(&path, &cfg).unwrap();
    let loaded = load_scanner_config(&path).unwrap();

    assert_eq!(loaded.enabled, cfg.enabled);
    assert!(loaded.engines.contains_key("clamav"));
}

#[test]
fn test_parse_engine_config_full() {
    let raw = serde_json::json!({
        "address": "192.168.1.1:3310",
        "url": "https://example.com/clamav.zip",
        "clamav_path": "/opt/clamav",
        "data_dir": "/var/lib/clamav",
        "scan_on_write": false,
        "scan_on_download": true,
        "scan_on_exec": false,
        "max_file_size": 104857600,
        "update_interval": "12h",
        "skip_extensions": [".exe", ".dll"],
        "state": {
            "install_status": "installed",
            "install_error": "",
            "db_status": "ready",
            "last_install_attempt": "2026-01-01T00:00:00Z",
            "last_db_update": "2026-01-01T00:00:00Z"
        }
    });
    let cfg = parse_engine_config(&raw);
    assert_eq!(cfg.address, "192.168.1.1:3310");
    assert_eq!(cfg.url, "https://example.com/clamav.zip");
    assert_eq!(cfg.clamav_path, "/opt/clamav");
    assert_eq!(cfg.data_dir, "/var/lib/clamav");
    assert_eq!(cfg.scan_on_write, false);
    assert_eq!(cfg.scan_on_download, true);
    assert_eq!(cfg.max_file_size, 104857600);
    assert_eq!(cfg.update_interval, "12h");
    assert_eq!(cfg.skip_extensions.len(), 2);
    assert_eq!(cfg.state.install_status, "installed");
    assert_eq!(cfg.state.db_status, "ready");
}

#[test]
fn test_parse_engine_config_empty_json() {
    let raw = serde_json::json!({});
    let cfg = parse_engine_config(&raw);
    // Should use defaults
    assert_eq!(cfg.address, "127.0.0.1:3310");
    assert_eq!(cfg.max_file_size, 52428800);
}

#[test]
fn test_marshal_engine_config_with_state() {
    let raw = serde_json::json!({"address": "127.0.0.1:3310"});
    let state = EngineState {
        install_status: "installed".to_string(),
        install_error: String::new(),
        db_status: "ready".to_string(),
        last_install_attempt: String::new(),
        last_db_update: String::new(),
    };
    let result = marshal_engine_config(&raw, &state, "/opt/clamav", "/var/lib/clamav");
    assert!(result.is_some());
    let val = result.unwrap();
    let cfg: ClamAVEngineConfig = serde_json::from_value(val).unwrap();
    assert_eq!(cfg.state.install_status, "installed");
    assert_eq!(cfg.clamav_path, "/opt/clamav");
    assert_eq!(cfg.data_dir, "/var/lib/clamav");
}

#[test]
fn test_marshal_engine_config_empty_paths() {
    let raw = serde_json::json!({"address": "127.0.0.1:3310"});
    let state = EngineState::default();
    let result = marshal_engine_config(&raw, &state, "", "");
    assert!(result.is_some());
    let val = result.unwrap();
    let cfg: ClamAVEngineConfig = serde_json::from_value(val).unwrap();
    assert!(cfg.clamav_path.is_empty());
    assert!(cfg.data_dir.is_empty());
}

#[test]
fn test_resolve_tools_dir() {
    let tmp = TempDir::new().unwrap();
    let config_path = tmp.path().join("config").join("config.scanner.json");
    let tools_dir = resolve_tools_dir(&config_path);
    assert!(tools_dir.to_str().unwrap().contains("workspace"));
    assert!(tools_dir.to_str().unwrap().contains("tools"));
}

#[test]
fn test_check_executables_at_path_nonexistent() {
    assert!(!check_executables_at_path(
        "/nonexistent/path/that/does/not/exist"
    ));
}

#[test]
fn test_check_executables_at_path_empty_dir() {
    let tmp = TempDir::new().unwrap();
    assert!(!check_executables_at_path(&tmp.path().to_string_lossy()));
}

#[test]
fn test_cmd_list_empty_config() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    cmd_list(&path).unwrap();
}

#[test]
fn test_cmd_list_with_engines() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    let mut cfg = ScannerFullConfig::default();
    cfg.enabled.push("clamav".to_string());
    let engine = ClamAVEngineConfig {
        address: "127.0.0.1:3310".to_string(),
        state: EngineState {
            install_status: "installed".to_string(),
            db_status: "ready".to_string(),
            ..Default::default()
        },
        ..Default::default()
    };
    cfg.engines
        .insert("clamav".to_string(), serde_json::to_value(engine).unwrap());
    save_scanner_config(&path, &cfg).unwrap();

    cmd_list(&path).unwrap();
}

#[test]
fn test_cmd_enable_adds_to_enabled_list() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    let mut cfg = ScannerFullConfig::default();
    let engine = ClamAVEngineConfig::default();
    cfg.engines
        .insert("clamav".to_string(), serde_json::to_value(engine).unwrap());
    save_scanner_config(&path, &cfg).unwrap();

    cmd_enable(&path, "clamav").unwrap();

    let loaded = load_scanner_config(&path).unwrap();
    assert!(loaded.enabled.contains(&"clamav".to_string()));
}

#[test]
fn test_cmd_enable_already_enabled() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    let mut cfg = ScannerFullConfig::default();
    cfg.enabled.push("clamav".to_string());
    let engine = ClamAVEngineConfig::default();
    cfg.engines
        .insert("clamav".to_string(), serde_json::to_value(engine).unwrap());
    save_scanner_config(&path, &cfg).unwrap();

    cmd_enable(&path, "clamav").unwrap();

    let loaded = load_scanner_config(&path).unwrap();
    assert_eq!(loaded.enabled.len(), 1); // Still just one
}

#[test]
fn test_cmd_disable_removes_from_enabled() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    let mut cfg = ScannerFullConfig::default();
    cfg.enabled.push("clamav".to_string());
    let engine = ClamAVEngineConfig::default();
    cfg.engines
        .insert("clamav".to_string(), serde_json::to_value(engine).unwrap());
    save_scanner_config(&path, &cfg).unwrap();

    cmd_disable(&path, "clamav").unwrap();

    let loaded = load_scanner_config(&path).unwrap();
    assert!(loaded.enabled.is_empty());
}

#[test]
fn test_cmd_disable_not_enabled() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    let cfg = ScannerFullConfig::default();
    save_scanner_config(&path, &cfg).unwrap();

    cmd_disable(&path, "clamav").unwrap();
    // Should succeed, no changes
}

#[test]
fn test_cmd_check_no_engines() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    let cfg = ScannerFullConfig::default();
    save_scanner_config(&path, &cfg).unwrap();

    cmd_check(&path).unwrap();
}

#[test]
fn test_clamav_engine_config_serialization() {
    let cfg = ClamAVEngineConfig::default();
    let json = serde_json::to_value(&cfg).unwrap();
    let deserialized: ClamAVEngineConfig = serde_json::from_value(json).unwrap();
    assert_eq!(deserialized.address, cfg.address);
    assert_eq!(deserialized.max_file_size, cfg.max_file_size);
}

#[test]
fn test_database_file_constant() {
    assert_eq!(DATABASE_FILE, "daily.cvd");
}

// -------------------------------------------------------------------------
// ClamAVEngineConfig serialization roundtrip with all fields
// -------------------------------------------------------------------------

#[test]
fn test_clamav_config_roundtrip_all_fields() {
    let cfg = ClamAVEngineConfig {
        address: "10.0.0.1:3310".to_string(),
        url: "https://example.com/clamav.zip".to_string(),
        clamav_path: "/opt/clamav".to_string(),
        data_dir: "/var/lib/clamav".to_string(),
        scan_on_write: false,
        scan_on_download: true,
        scan_on_exec: true,
        max_file_size: 100_000_000,
        update_interval: "6h".to_string(),
        skip_extensions: vec![".exe".to_string(), ".dll".to_string()],
        state: EngineState {
            install_status: "installed".to_string(),
            install_error: String::new(),
            db_status: "ready".to_string(),
            last_install_attempt: "2026-01-01".to_string(),
            last_db_update: "2026-01-02".to_string(),
        },
    };
    let json = serde_json::to_value(&cfg).unwrap();
    let back: ClamAVEngineConfig = serde_json::from_value(json).unwrap();
    assert_eq!(back.address, cfg.address);
    assert_eq!(back.url, cfg.url);
    assert_eq!(back.clamav_path, cfg.clamav_path);
    assert_eq!(back.data_dir, cfg.data_dir);
    assert_eq!(back.scan_on_write, cfg.scan_on_write);
    assert_eq!(back.scan_on_download, cfg.scan_on_download);
    assert_eq!(back.max_file_size, cfg.max_file_size);
    assert_eq!(back.update_interval, cfg.update_interval);
    assert_eq!(back.skip_extensions, cfg.skip_extensions);
    assert_eq!(back.state.install_status, cfg.state.install_status);
    assert_eq!(back.state.db_status, cfg.state.db_status);
}

// -------------------------------------------------------------------------
// ScannerFullConfig serialization tests
// -------------------------------------------------------------------------

#[test]
fn test_scanner_full_config_serialization() {
    let mut cfg = ScannerFullConfig::default();
    cfg.enabled.push("clamav".to_string());
    cfg.engines.insert(
        "clamav".to_string(),
        serde_json::json!({"address": "127.0.0.1:3310"}),
    );

    let json = serde_json::to_string(&cfg).unwrap();
    let back: ScannerFullConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back.enabled, cfg.enabled);
    assert!(back.engines.contains_key("clamav"));
}

#[test]
fn test_scanner_full_config_empty_engines() {
    let cfg = ScannerFullConfig::default();
    let json = serde_json::to_string(&cfg).unwrap();
    let back: ScannerFullConfig = serde_json::from_str(&json).unwrap();
    assert!(back.enabled.is_empty());
    assert!(back.engines.is_empty());
}

// -------------------------------------------------------------------------
// EngineState tests
// -------------------------------------------------------------------------

#[test]
fn test_engine_state_with_errors() {
    let state = EngineState {
        install_status: "failed".to_string(),
        install_error: "permission denied".to_string(),
        db_status: "error".to_string(),
        last_install_attempt: "2026-01-01".to_string(),
        last_db_update: String::new(),
    };
    let json = serde_json::to_value(&state).unwrap();
    let back: EngineState = serde_json::from_value(json).unwrap();
    assert_eq!(back.install_status, "failed");
    assert_eq!(back.install_error, "permission denied");
    assert_eq!(back.db_status, "error");
}

// -------------------------------------------------------------------------
// default_address / default_max_file_size / default_update_interval
// -------------------------------------------------------------------------

#[test]
fn test_default_values() {
    assert_eq!(default_address(), "127.0.0.1:3310");
    assert_eq!(default_max_file_size(), 52428800);
    assert_eq!(default_update_interval(), "24h");
}

#[test]
fn test_default_skip_extensions_contains_common_types() {
    let exts = default_skip_extensions();
    // Should contain common safe file types
    assert!(exts.contains(&".txt".to_string()));
    assert!(exts.contains(&".md".to_string()));
    assert!(exts.contains(&".json".to_string()));
    assert!(exts.contains(&".yaml".to_string()));
    assert!(exts.contains(&".yml".to_string()));
    assert!(exts.contains(&".toml".to_string()));
    assert!(exts.contains(&".log".to_string()));
    assert!(exts.contains(&".css".to_string()));
    assert!(exts.contains(&".html".to_string()));
    // Should not contain executable extensions
    assert!(!exts.contains(&".exe".to_string()));
    assert!(!exts.contains(&".dll".to_string()));
}

// -------------------------------------------------------------------------
// parse_engine_config partial JSON
// -------------------------------------------------------------------------

#[test]
fn test_parse_engine_config_partial() {
    let raw = serde_json::json!({
        "address": "10.0.0.1:9999",
        "scan_on_write": false
    });
    let cfg = parse_engine_config(&raw);
    assert_eq!(cfg.address, "10.0.0.1:9999");
    assert_eq!(cfg.scan_on_write, false);
    // Other fields should be defaults
    assert_eq!(cfg.scan_on_download, false);
    assert_eq!(cfg.max_file_size, 52428800);
    assert_eq!(cfg.update_interval, "24h");
}

#[test]
fn test_parse_engine_config_null_value() {
    let raw = serde_json::Value::Null;
    let cfg = parse_engine_config(&raw);
    // Should return defaults
    assert_eq!(cfg.address, "127.0.0.1:3310");
}

// -------------------------------------------------------------------------
// marshal_engine_config edge cases
// -------------------------------------------------------------------------

#[test]
fn test_marshal_engine_config_only_state_update() {
    let raw = serde_json::json!({"address": "127.0.0.1:3310", "clamav_path": "/original"});
    let state = EngineState {
        install_status: "installed".to_string(),
        ..Default::default()
    };
    let result = marshal_engine_config(&raw, &state, "", "");
    assert!(result.is_some());
    let cfg: ClamAVEngineConfig = serde_json::from_value(result.unwrap()).unwrap();
    assert_eq!(cfg.state.install_status, "installed");
    assert_eq!(cfg.clamav_path, "/original"); // preserved
}

#[test]
fn test_marshal_engine_config_overwrite_paths() {
    let raw = serde_json::json!({"address": "127.0.0.1:3310", "clamav_path": "/old", "data_dir": "/old_data"});
    let state = EngineState::default();
    let result = marshal_engine_config(&raw, &state, "/new/path", "/new/data");
    assert!(result.is_some());
    let cfg: ClamAVEngineConfig = serde_json::from_value(result.unwrap()).unwrap();
    assert_eq!(cfg.clamav_path, "/new/path");
    assert_eq!(cfg.data_dir, "/new/data");
}

// -------------------------------------------------------------------------
// resolve_tools_dir tests
// -------------------------------------------------------------------------

#[test]
fn test_resolve_tools_dir_from_scanner_config() {
    let tmp = TempDir::new().unwrap();
    let config_path = tmp.path().join("config").join("config.scanner.json");
    let tools_dir = resolve_tools_dir(&config_path);
    assert!(tools_dir.ends_with("tools"));
    // Should be under workspace
    assert!(tools_dir.to_str().unwrap().contains("workspace"));
}

#[test]
fn test_resolve_tools_dir_path_structure() {
    let config_path = std::path::Path::new("/home/user/.nemesisbot/config/config.scanner.json");
    let tools_dir = resolve_tools_dir(config_path);
    assert_eq!(
        tools_dir,
        std::path::PathBuf::from("/home/user/.nemesisbot/workspace/tools")
    );
}

// -------------------------------------------------------------------------
// cmd_add tests
// -------------------------------------------------------------------------

#[test]
fn test_cmd_add_new_engine() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    let cfg = ScannerFullConfig::default();
    save_scanner_config(&path, &cfg).unwrap();

    cmd_add(
        &path,
        "clamav",
        Some("https://example.com/clamav.zip"),
        Some("/opt/clamav"),
        Some("127.0.0.1:9999"),
    )
    .unwrap();

    let loaded = load_scanner_config(&path).unwrap();
    assert!(loaded.engines.contains_key("clamav"));
    let engine = parse_engine_config(loaded.engines.get("clamav").unwrap());
    assert_eq!(engine.url, "https://example.com/clamav.zip");
    assert_eq!(engine.clamav_path, "/opt/clamav");
    assert_eq!(engine.address, "127.0.0.1:9999");
}

#[test]
fn test_cmd_add_update_existing_engine() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    let mut cfg = ScannerFullConfig::default();
    cfg.engines.insert(
        "clamav".to_string(),
        serde_json::json!({"address": "127.0.0.1:3310"}),
    );
    save_scanner_config(&path, &cfg).unwrap();

    cmd_add(&path, "clamav", None, None, Some("10.0.0.1:3310")).unwrap();

    let loaded = load_scanner_config(&path).unwrap();
    let engine = parse_engine_config(loaded.engines.get("clamav").unwrap());
    assert_eq!(engine.address, "10.0.0.1:3310");
}

#[test]
fn test_cmd_add_with_defaults() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    let cfg = ScannerFullConfig::default();
    save_scanner_config(&path, &cfg).unwrap();

    cmd_add(&path, "clamav", None, None, None).unwrap();

    let loaded = load_scanner_config(&path).unwrap();
    assert!(loaded.engines.contains_key("clamav"));
    let engine = parse_engine_config(loaded.engines.get("clamav").unwrap());
    assert_eq!(engine.address, "127.0.0.1:3310"); // default
}

// -------------------------------------------------------------------------
// cmd_remove tests
// -------------------------------------------------------------------------

#[test]
fn test_cmd_remove_existing_engine() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    let mut cfg = ScannerFullConfig::default();
    cfg.engines.insert(
        "clamav".to_string(),
        serde_json::json!({"address": "127.0.0.1:3310"}),
    );
    cfg.enabled.push("clamav".to_string());
    save_scanner_config(&path, &cfg).unwrap();

    cmd_remove(&path, "clamav").unwrap();

    let loaded = load_scanner_config(&path).unwrap();
    assert!(!loaded.engines.contains_key("clamav"));
    assert!(!loaded.enabled.contains(&"clamav".to_string()));
}

// -------------------------------------------------------------------------
// cmd_enable additional tests
// -------------------------------------------------------------------------

#[test]
fn test_cmd_enable_sets_pending_status() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    let mut cfg = ScannerFullConfig::default();
    let engine = ClamAVEngineConfig {
        state: EngineState::default(), // empty install_status
        ..Default::default()
    };
    cfg.engines
        .insert("clamav".to_string(), serde_json::to_value(engine).unwrap());
    save_scanner_config(&path, &cfg).unwrap();

    cmd_enable(&path, "clamav").unwrap();

    let loaded = load_scanner_config(&path).unwrap();
    let engine_cfg = parse_engine_config(loaded.engines.get("clamav").unwrap());
    assert_eq!(engine_cfg.state.install_status, "pending");
}

#[test]
fn test_cmd_enable_preserves_existing_install_status() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    let mut cfg = ScannerFullConfig::default();
    let engine = ClamAVEngineConfig {
        state: EngineState {
            install_status: "installed".to_string(),
            ..Default::default()
        },
        ..Default::default()
    };
    cfg.engines
        .insert("clamav".to_string(), serde_json::to_value(engine).unwrap());
    save_scanner_config(&path, &cfg).unwrap();

    cmd_enable(&path, "clamav").unwrap();

    let loaded = load_scanner_config(&path).unwrap();
    let engine_cfg = parse_engine_config(loaded.engines.get("clamav").unwrap());
    // Should keep "installed" status, not change to "pending"
    assert_eq!(engine_cfg.state.install_status, "installed");
}

// -------------------------------------------------------------------------
// cmd_check with configured engines
// -------------------------------------------------------------------------

#[test]
fn test_cmd_check_with_multiple_engines() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    let mut cfg = ScannerFullConfig::default();
    cfg.enabled.push("clamav".to_string());
    let engine1 = ClamAVEngineConfig {
        address: "127.0.0.1:3310".to_string(),
        state: EngineState {
            install_status: "installed".to_string(),
            db_status: "ready".to_string(),
            ..Default::default()
        },
        ..Default::default()
    };
    cfg.engines
        .insert("clamav".to_string(), serde_json::to_value(engine1).unwrap());
    save_scanner_config(&path, &cfg).unwrap();

    cmd_check(&path).unwrap();
}

// -------------------------------------------------------------------------
// check_executables_at_path with files
// -------------------------------------------------------------------------

#[test]
fn test_check_executables_at_path_with_fake_executable() {
    let tmp = TempDir::new().unwrap();
    // Create a fake clamd file
    std::fs::write(tmp.path().join("clamd"), "fake").unwrap();
    assert!(check_executables_at_path(&tmp.path().to_string_lossy()));
}

#[test]
fn test_check_executables_at_path_with_exe() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("clamd.exe"), "fake").unwrap();
    assert!(check_executables_at_path(&tmp.path().to_string_lossy()));
}

#[test]
fn test_check_executables_at_path_wrong_file() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("readme.txt"), "not clamav").unwrap();
    assert!(!check_executables_at_path(&tmp.path().to_string_lossy()));
}

// -------------------------------------------------------------------------
// ClamAV config serialization roundtrip tests
// -------------------------------------------------------------------------

#[test]
fn test_clamav_config_serialization_roundtrip() {
    let cfg = ClamAVEngineConfig {
        address: "192.168.1.1:3310".to_string(),
        url: "https://example.com/clamav.zip".to_string(),
        clamav_path: "/opt/clamav".to_string(),
        data_dir: "/var/lib/clamav".to_string(),
        scan_on_write: true,
        scan_on_download: true,
        scan_on_exec: false,
        max_file_size: 104857600,
        update_interval: "12h".to_string(),
        skip_extensions: vec![".exe".to_string(), ".dll".to_string()],
        state: EngineState {
            install_status: "installed".to_string(),
            install_error: String::new(),
            db_status: "ready".to_string(),
            last_install_attempt: "2026-01-01T00:00:00Z".to_string(),
            last_db_update: "2026-01-01T00:00:00Z".to_string(),
        },
    };
    let json = serde_json::to_value(&cfg).unwrap();
    let deserialized: ClamAVEngineConfig = serde_json::from_value(json).unwrap();
    assert_eq!(deserialized.address, "192.168.1.1:3310");
    assert_eq!(deserialized.url, "https://example.com/clamav.zip");
    assert_eq!(deserialized.max_file_size, 104857600);
    assert_eq!(deserialized.skip_extensions.len(), 2);
    assert_eq!(deserialized.state.install_status, "installed");
}

#[test]
fn test_engine_state_serialization() {
    let state = EngineState {
        install_status: "pending".to_string(),
        install_error: "some error".to_string(),
        db_status: "missing".to_string(),
        last_install_attempt: "2026-06-01T12:00:00Z".to_string(),
        last_db_update: String::new(),
    };
    let json = serde_json::to_value(&state).unwrap();
    let loaded: EngineState = serde_json::from_value(json).unwrap();
    assert_eq!(loaded.install_status, "pending");
    assert_eq!(loaded.install_error, "some error");
    assert_eq!(loaded.db_status, "missing");
}

// -------------------------------------------------------------------------
// default value tests
// -------------------------------------------------------------------------

#[test]
fn test_default_address() {
    assert_eq!(default_address(), "127.0.0.1:3310");
}

#[test]
fn test_default_max_file_size() {
    assert_eq!(default_max_file_size(), 52428800);
}

// -------------------------------------------------------------------------
// detect_executable_dir tests
// -------------------------------------------------------------------------

#[test]
fn test_detect_executable_dir_empty() {
    let tmp = TempDir::new().unwrap();
    let result = detect_executable_dir(tmp.path(), &["clamd", "clamd.exe"]);
    assert!(result.is_none());
}

#[test]
fn test_detect_executable_dir_with_executable() {
    let tmp = TempDir::new().unwrap();
    let sub = tmp.path().join("bin");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("clamd"), "fake").unwrap();
    let result = detect_executable_dir(tmp.path(), &["clamd", "clamd.exe"]);
    assert!(result.is_some());
    assert!(result.unwrap().contains("bin"));
}

#[test]
fn test_detect_executable_dir_nested() {
    let tmp = TempDir::new().unwrap();
    let nested = tmp.path().join("a").join("b").join("c");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join("clamscan"), "fake").unwrap();
    let result = detect_executable_dir(tmp.path(), &["clamscan"]);
    assert!(result.is_some());
}

// -------------------------------------------------------------------------
// ScannerFullConfig serialization tests
// -------------------------------------------------------------------------

#[test]
fn test_scanner_full_config_with_multiple_engines() {
    let mut cfg = ScannerFullConfig::default();
    cfg.enabled.push("clamav".to_string());
    cfg.enabled.push("custom".to_string());
    cfg.engines.insert(
        "clamav".to_string(),
        serde_json::json!({"address": "127.0.0.1:3310"}),
    );
    cfg.engines.insert(
        "custom".to_string(),
        serde_json::json!({"address": "127.0.0.1:9999"}),
    );

    let json = serde_json::to_string(&cfg).unwrap();
    let loaded: ScannerFullConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(loaded.enabled.len(), 2);
    assert_eq!(loaded.engines.len(), 2);
}

// -------------------------------------------------------------------------
// resolve_tools_dir tests
// -------------------------------------------------------------------------

// -------------------------------------------------------------------------
// resolve_tools_dir additional tests
// -------------------------------------------------------------------------

#[test]
fn test_resolve_tools_dir_with_config_subdir() {
    let tmp = TempDir::new().unwrap();
    let config_path = tmp.path().join("config").join("config.scanner.json");
    let tools_dir = resolve_tools_dir(&config_path);
    let tools_str = tools_dir.to_str().unwrap();
    assert!(tools_str.contains("workspace"));
    assert!(tools_str.contains("tools"));
}

#[test]
fn test_resolve_tools_dir_no_parent() {
    let config_path = std::path::Path::new("config.scanner.json");
    let tools_dir = resolve_tools_dir(config_path);
    // Should still return a path (may be "workspace/tools")
    assert!(!tools_dir.as_os_str().is_empty());
}

// -------------------------------------------------------------------------
// parse_engine_config edge cases
// -------------------------------------------------------------------------

#[test]
fn test_parse_engine_config_partial_state() {
    let raw = serde_json::json!({
        "address": "127.0.0.1:3310",
        "state": {
            "install_status": "installed"
        }
    });
    let cfg = parse_engine_config(&raw);
    assert_eq!(cfg.state.install_status, "installed");
    assert!(cfg.state.install_error.is_empty()); // should default to empty
    assert!(cfg.state.db_status.is_empty());
}

#[test]
fn test_parse_engine_config_invalid_types() {
    let raw = serde_json::json!({
        "address": 12345,
        "scan_on_write": "yes",
        "max_file_size": "big"
    });
    let cfg = parse_engine_config(&raw);
    // Should use defaults for invalid types
    assert_eq!(cfg.address, "127.0.0.1:3310"); // default
}

// -------------------------------------------------------------------------
// marshal_engine_config edge cases
// -------------------------------------------------------------------------

#[test]
fn test_marshal_engine_config_preserves_known_fields() {
    let raw = serde_json::json!({
        "address": "127.0.0.1:3310",
        "scan_on_write": true,
        "max_file_size": 100000
    });
    let state = EngineState::default();
    let result = marshal_engine_config(&raw, &state, "", "");
    assert!(result.is_some());
    let val = result.unwrap();
    let cfg: ClamAVEngineConfig = serde_json::from_value(val).unwrap();
    assert_eq!(cfg.address, "127.0.0.1:3310");
    assert!(cfg.scan_on_write);
    assert_eq!(cfg.max_file_size, 100000);
}

#[test]
fn test_marshal_engine_config_updates_path_only() {
    let raw = serde_json::json!({"address": "127.0.0.1:3310"});
    let state = EngineState::default();
    let result = marshal_engine_config(&raw, &state, "/new/path", "");
    assert!(result.is_some());
    let val = result.unwrap();
    let cfg: ClamAVEngineConfig = serde_json::from_value(val).unwrap();
    assert_eq!(cfg.clamav_path, "/new/path");
    assert!(cfg.data_dir.is_empty());
}

// -------------------------------------------------------------------------
// Additional coverage tests for scanner
// -------------------------------------------------------------------------

#[test]
fn test_scanner_full_config_from_json() {
    let json = r#"{"enabled":["clamav","custom"],"engines":{"clamav":{"address":"127.0.0.1:3310"},"custom":{"address":"127.0.0.1:9999"}}}"#;
    let cfg: ScannerFullConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.enabled.len(), 2);
    assert_eq!(cfg.engines.len(), 2);
}

#[test]
fn test_clamav_engine_config_from_json_minimal() {
    let json = r#"{"address":"0.0.0.0:3310"}"#;
    let cfg: ClamAVEngineConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.address, "0.0.0.0:3310");
    assert!(cfg.url.is_empty());
    assert!(cfg.clamav_path.is_empty());
    assert_eq!(cfg.max_file_size, 52428800); // default
    // Note: serde default for bool is false, so scan_on_write is false for partial JSON
    assert_eq!(cfg.scan_on_write, false);
}

#[test]
fn test_clamav_engine_config_from_json_full() {
    let json = r#"{
        "address":"10.0.0.1:3310",
        "url":"https://example.com/clamav.zip",
        "clamav_path":"/opt/clamav",
        "data_dir":"/var/lib/clamav",
        "scan_on_write":false,
        "scan_on_download":true,
        "scan_on_exec":false,
        "max_file_size":100000000,
        "update_interval":"6h",
        "skip_extensions":[".exe",".dll",".bat"],
        "state":{"install_status":"installed","db_status":"ready","install_error":"","last_install_attempt":"","last_db_update":""}
    }"#;
    let cfg: ClamAVEngineConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.address, "10.0.0.1:3310");
    assert_eq!(cfg.url, "https://example.com/clamav.zip");
    assert_eq!(cfg.clamav_path, "/opt/clamav");
    assert_eq!(cfg.data_dir, "/var/lib/clamav");
    assert!(!cfg.scan_on_write);
    assert!(cfg.scan_on_download);
    assert!(!cfg.scan_on_exec);
    assert_eq!(cfg.max_file_size, 100000000);
    assert_eq!(cfg.update_interval, "6h");
    assert_eq!(cfg.skip_extensions.len(), 3);
    assert_eq!(cfg.state.install_status, "installed");
    assert_eq!(cfg.state.db_status, "ready");
}

#[test]
fn test_engine_state_from_json() {
    let json = r#"{"install_status":"failed","install_error":"timeout","db_status":"missing","last_install_attempt":"2026-01-01","last_db_update":"2026-01-02"}"#;
    let state: EngineState = serde_json::from_str(json).unwrap();
    assert_eq!(state.install_status, "failed");
    assert_eq!(state.install_error, "timeout");
    assert_eq!(state.db_status, "missing");
    assert_eq!(state.last_install_attempt, "2026-01-01");
    assert_eq!(state.last_db_update, "2026-01-02");
}

#[test]
fn test_cmd_list_no_engines() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    let cfg = ScannerFullConfig::default();
    save_scanner_config(&path, &cfg).unwrap();
    cmd_list(&path).unwrap();
}

#[test]
fn test_cmd_list_with_disabled_engine() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    let mut cfg = ScannerFullConfig::default();
    let engine = ClamAVEngineConfig::default();
    cfg.engines
        .insert("clamav".to_string(), serde_json::to_value(engine).unwrap());
    save_scanner_config(&path, &cfg).unwrap();
    cmd_list(&path).unwrap();
}

#[test]
fn test_cmd_list_with_multiple_engines() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    let mut cfg = ScannerFullConfig::default();
    cfg.enabled.push("clamav".to_string());
    let engine1 = ClamAVEngineConfig {
        address: "127.0.0.1:3310".to_string(),
        state: EngineState {
            install_status: "installed".to_string(),
            db_status: "ready".to_string(),
            ..Default::default()
        },
        ..Default::default()
    };
    let engine2 = ClamAVEngineConfig {
        address: "127.0.0.1:9999".to_string(),
        url: "https://example.com/engine2.zip".to_string(),
        ..Default::default()
    };
    cfg.engines
        .insert("clamav".to_string(), serde_json::to_value(engine1).unwrap());
    cfg.engines.insert(
        "engine2".to_string(),
        serde_json::to_value(engine2).unwrap(),
    );
    save_scanner_config(&path, &cfg).unwrap();
    cmd_list(&path).unwrap();
}

#[test]
fn test_cmd_check_with_disabled_engine() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    let mut cfg = ScannerFullConfig::default();
    cfg.enabled.push("clamav".to_string());
    let engine = ClamAVEngineConfig {
        address: "127.0.0.1:3310".to_string(),
        url: "https://example.com/very-long-url-that-is-more-than-forty-characters-to-test-truncation.zip".to_string(),
        ..Default::default()
    };
    cfg.engines
        .insert("clamav".to_string(), serde_json::to_value(engine).unwrap());
    save_scanner_config(&path, &cfg).unwrap();
    cmd_check(&path).unwrap();
}

#[test]
fn test_cmd_check_with_install_error() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    let mut cfg = ScannerFullConfig::default();
    cfg.enabled.push("clamav".to_string());
    let engine = ClamAVEngineConfig {
        state: EngineState {
            install_status: "failed".to_string(),
            install_error: "download failed".to_string(),
            db_status: "missing".to_string(),
            ..Default::default()
        },
        ..Default::default()
    };
    cfg.engines
        .insert("clamav".to_string(), serde_json::to_value(engine).unwrap());
    save_scanner_config(&path, &cfg).unwrap();
    cmd_check(&path).unwrap();
}

#[test]
fn test_cmd_check_with_pending_status() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    let mut cfg = ScannerFullConfig::default();
    cfg.enabled.push("clamav".to_string());
    let engine = ClamAVEngineConfig {
        state: EngineState {
            install_status: "pending".to_string(),
            ..Default::default()
        },
        ..Default::default()
    };
    cfg.engines
        .insert("clamav".to_string(), serde_json::to_value(engine).unwrap());
    save_scanner_config(&path, &cfg).unwrap();
    cmd_check(&path).unwrap();
}

#[test]
fn test_cmd_add_update_with_url_only() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    let mut cfg = ScannerFullConfig::default();
    cfg.engines.insert(
        "clamav".to_string(),
        serde_json::json!({"address": "127.0.0.1:3310"}),
    );
    save_scanner_config(&path, &cfg).unwrap();

    cmd_add(
        &path,
        "clamav",
        Some("https://new-url.com/clamav.zip"),
        None,
        None,
    )
    .unwrap();

    let loaded = load_scanner_config(&path).unwrap();
    let engine = parse_engine_config(loaded.engines.get("clamav").unwrap());
    assert_eq!(engine.url, "https://new-url.com/clamav.zip");
}

#[test]
fn test_cmd_add_update_with_path_only() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    let mut cfg = ScannerFullConfig::default();
    cfg.engines.insert(
        "clamav".to_string(),
        serde_json::json!({"address": "127.0.0.1:3310"}),
    );
    save_scanner_config(&path, &cfg).unwrap();

    cmd_add(&path, "clamav", None, Some("/custom/path"), None).unwrap();

    let loaded = load_scanner_config(&path).unwrap();
    let engine = parse_engine_config(loaded.engines.get("clamav").unwrap());
    assert_eq!(engine.clamav_path, "/custom/path");
}

#[test]
fn test_detect_executable_dir_not_found() {
    let tmp = TempDir::new().unwrap();
    let sub = tmp.path().join("empty_subdir");
    std::fs::create_dir_all(&sub).unwrap();
    let result = detect_executable_dir(tmp.path(), &["nonexistent"]);
    assert!(result.is_none());
}

#[test]
fn test_detect_executable_dir_with_clamd_exe() {
    let tmp = TempDir::new().unwrap();
    let sub = tmp.path().join("bin");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("clamd.exe"), "fake").unwrap();
    let result = detect_executable_dir(tmp.path(), &["clamd.exe", "clamd"]);
    assert!(result.is_some());
    let found = result.unwrap();
    assert!(found.contains("bin"));
}

#[test]
fn test_detect_executable_dir_with_clamscan() {
    let tmp = TempDir::new().unwrap();
    let sub = tmp.path().join("usr").join("local").join("bin");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("clamscan"), "fake").unwrap();
    let result = detect_executable_dir(tmp.path(), &["clamscan"]);
    assert!(result.is_some());
}

#[test]
fn test_check_executables_at_path_with_clamscan() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("clamscan"), "fake").unwrap();
    assert!(check_executables_at_path(&tmp.path().to_string_lossy()));
}

#[test]
fn test_check_executables_at_path_with_clamscan_exe() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("clamscan.exe"), "fake").unwrap();
    assert!(check_executables_at_path(&tmp.path().to_string_lossy()));
}

#[test]
fn test_save_scanner_config_creates_parent_dir() {
    let tmp = TempDir::new().unwrap();
    let path = tmp
        .path()
        .join("nested")
        .join("dir")
        .join("config.scanner.json");
    let cfg = ScannerFullConfig::default();
    save_scanner_config(&path, &cfg).unwrap();
    assert!(path.exists());
}

#[test]
fn test_load_scanner_config_invalid_json() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    std::fs::write(&path, "not valid json").unwrap();
    let result = load_scanner_config(&path);
    assert!(result.is_err());
}

// -------------------------------------------------------------------------
// ClamAVEngineConfig serialization/deserialization
// -------------------------------------------------------------------------

#[test]
fn test_clamav_config_default_values_v2() {
    let config = ClamAVEngineConfig::default();
    assert_eq!(config.address, "127.0.0.1:3310");
    assert_eq!(config.url, "");
    assert_eq!(config.clamav_path, "");
    assert_eq!(config.data_dir, "");
    assert!(config.scan_on_write);
    assert!(!config.scan_on_download);
    assert!(config.scan_on_exec);
    assert_eq!(config.max_file_size, 52428800);
}

#[test]
fn test_clamav_config_serialization_roundtrip_v2() {
    let config = ClamAVEngineConfig {
        address: "192.168.1.1:3310".to_string(),
        url: "https://example.com/clamav.zip".to_string(),
        clamav_path: "/usr/bin/clamscan".to_string(),
        data_dir: "/var/lib/clamav".to_string(),
        scan_on_write: true,
        scan_on_download: true,
        scan_on_exec: false,
        max_file_size: 50000000,
        ..Default::default()
    };
    let json = serde_json::to_value(&config).unwrap();
    let deserialized: ClamAVEngineConfig = serde_json::from_value(json).unwrap();
    assert_eq!(deserialized.address, "192.168.1.1:3310");
    assert_eq!(deserialized.url, "https://example.com/clamav.zip");
    assert_eq!(deserialized.clamav_path, "/usr/bin/clamscan");
    assert!(deserialized.scan_on_write);
    assert!(deserialized.scan_on_download);
    assert!(!deserialized.scan_on_exec);
    assert_eq!(deserialized.max_file_size, 50000000);
}

#[test]
fn test_engine_state_default_v2() {
    let state = EngineState::default();
    assert_eq!(state.install_status, "");
    assert_eq!(state.install_error, "");
    assert_eq!(state.db_status, "");
}

// -------------------------------------------------------------------------
// ScannerFullConfig tests
// -------------------------------------------------------------------------

#[test]
fn test_scanner_full_config_default_v2() {
    let config = ScannerFullConfig::default();
    assert!(config.enabled.is_empty());
    assert!(config.engines.is_empty());
}

#[test]
fn test_scanner_full_config_with_engines_v2() {
    let mut config = ScannerFullConfig::default();
    config.enabled.push("clamav".to_string());
    config.engines.insert(
        "clamav".to_string(),
        serde_json::json!({"address": "127.0.0.1:3310"}),
    );
    assert_eq!(config.enabled.len(), 1);
    assert_eq!(config.engines.len(), 1);
}

// -------------------------------------------------------------------------
// parse_engine_config tests
// -------------------------------------------------------------------------

#[test]
fn test_parse_engine_config_full_v2() {
    let json = serde_json::json!({
        "address": "10.0.0.1:3310",
        "url": "https://clamav.net/download",
        "clamav_path": "/opt/clamav/bin",
        "data_dir": "/var/clamav",
        "scan_on_write": true,
        "scan_on_download": true,
        "scan_on_exec": true,
        "max_file_size": 100000000
    });
    let config = parse_engine_config(&json);
    assert_eq!(config.address, "10.0.0.1:3310");
    assert_eq!(config.url, "https://clamav.net/download");
    assert_eq!(config.clamav_path, "/opt/clamav/bin");
    assert_eq!(config.data_dir, "/var/clamav");
    assert!(config.scan_on_write);
    assert!(config.scan_on_download);
    assert!(config.scan_on_exec);
    assert_eq!(config.max_file_size, 100000000);
}

#[test]
fn test_parse_engine_config_minimal_v2() {
    let json = serde_json::json!({});
    let config = parse_engine_config(&json);
    assert_eq!(config.address, "127.0.0.1:3310"); // default
}

// -------------------------------------------------------------------------
// cmd_enable/cmd_disable tests
// -------------------------------------------------------------------------

#[test]
fn test_cmd_enable_new_engine_v2() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    let mut cfg = ScannerFullConfig::default();
    cfg.engines.insert(
        "clamav".to_string(),
        serde_json::json!({"address": "127.0.0.1:3310"}),
    );
    save_scanner_config(&path, &cfg).unwrap();

    cmd_enable(&path, "clamav").unwrap();

    let loaded = load_scanner_config(&path).unwrap();
    assert!(loaded.enabled.contains(&"clamav".to_string()));
}

#[test]
fn test_cmd_disable_engine_v2() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    let mut cfg = ScannerFullConfig::default();
    cfg.enabled.push("clamav".to_string());
    cfg.engines.insert(
        "clamav".to_string(),
        serde_json::json!({"address": "127.0.0.1:3310"}),
    );
    save_scanner_config(&path, &cfg).unwrap();

    cmd_disable(&path, "clamav").unwrap();

    let loaded = load_scanner_config(&path).unwrap();
    assert!(!loaded.enabled.contains(&"clamav".to_string()));
}

#[test]
fn test_cmd_enable_already_enabled_v2() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    let mut cfg = ScannerFullConfig::default();
    cfg.enabled.push("clamav".to_string());
    cfg.engines.insert(
        "clamav".to_string(),
        serde_json::json!({"address": "127.0.0.1:3310"}),
    );
    save_scanner_config(&path, &cfg).unwrap();

    cmd_enable(&path, "clamav").unwrap();
    // Should still have only one entry
    let loaded = load_scanner_config(&path).unwrap();
    assert_eq!(loaded.enabled.iter().filter(|e| **e == "clamav").count(), 1);
}

// -------------------------------------------------------------------------
// cmd_remove tests
// -------------------------------------------------------------------------

#[test]
fn test_cmd_remove_existing_engine_v2() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    let mut cfg = ScannerFullConfig::default();
    cfg.enabled.push("clamav".to_string());
    cfg.engines.insert(
        "clamav".to_string(),
        serde_json::json!({"address": "127.0.0.1:3310"}),
    );
    save_scanner_config(&path, &cfg).unwrap();

    cmd_remove(&path, "clamav").unwrap();

    let loaded = load_scanner_config(&path).unwrap();
    assert!(!loaded.engines.contains_key("clamav"));
    assert!(!loaded.enabled.contains(&"clamav".to_string()));
}

#[test]
fn test_cmd_remove_nonexistent_engine_v2() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    let mut cfg = ScannerFullConfig::default();
    cfg.engines.insert(
        "clamav".to_string(),
        serde_json::json!({"address": "127.0.0.1:3310"}),
    );
    save_scanner_config(&path, &cfg).unwrap();

    // Remove an existing engine works
    cmd_remove(&path, "clamav").unwrap();
    let loaded = load_scanner_config(&path).unwrap();
    assert!(!loaded.engines.contains_key("clamav"));
}

// -------------------------------------------------------------------------
// cmd_add with various parameters
// -------------------------------------------------------------------------

#[test]
fn test_cmd_add_new_engine_v2() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    save_scanner_config(&path, &ScannerFullConfig::default()).unwrap();

    cmd_add(
        &path,
        "clamav",
        Some("https://scanner.example.com"),
        Some("/opt/scanner"),
        None,
    )
    .unwrap();

    let loaded = load_scanner_config(&path).unwrap();
    assert!(loaded.engines.contains_key("clamav"));
}

#[test]
fn test_cmd_add_with_address_override() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    save_scanner_config(&path, &ScannerFullConfig::default()).unwrap();

    cmd_add(
        &path,
        "clamav",
        Some("https://clamav.net"),
        Some("/opt/clamav"),
        Some("10.0.0.1:3310"),
    )
    .unwrap();

    let loaded = load_scanner_config(&path).unwrap();
    let engine = parse_engine_config(loaded.engines.get("clamav").unwrap());
    assert_eq!(engine.url, "https://clamav.net");
    assert_eq!(engine.clamav_path, "/opt/clamav");
    assert_eq!(engine.address, "10.0.0.1:3310");
}

// -------------------------------------------------------------------------
// default_address and default_max_file_size function tests
// -------------------------------------------------------------------------

#[test]
fn test_default_address_v2() {
    assert_eq!(default_address(), "127.0.0.1:3310");
}

#[test]
fn test_default_max_file_size_v2() {
    assert_eq!(default_max_file_size(), 52428800);
}

// -------------------------------------------------------------------------
// cmd_clamav subcommand tests (testing the subcommand logic via direct calls)
// -------------------------------------------------------------------------

#[test]
fn test_cmd_clamav_enable_and_disable() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    let mut cfg = ScannerFullConfig::default();
    let engine = ClamAVEngineConfig {
        address: "127.0.0.1:3310".to_string(),
        ..Default::default()
    };
    cfg.engines
        .insert("clamav".to_string(), serde_json::to_value(engine).unwrap());
    save_scanner_config(&path, &cfg).unwrap();

    cmd_enable(&path, "clamav").unwrap();
    let loaded = load_scanner_config(&path).unwrap();
    assert!(loaded.enabled.contains(&"clamav".to_string()));

    cmd_disable(&path, "clamav").unwrap();
    let loaded = load_scanner_config(&path).unwrap();
    assert!(!loaded.enabled.contains(&"clamav".to_string()));
}

// -------------------------------------------------------------------------
// ScannerFullConfig with multiple engines
// -------------------------------------------------------------------------

#[test]
fn test_scanner_config_multiple_engines() {
    let mut cfg = ScannerFullConfig::default();
    cfg.enabled.push("clamav".to_string());
    cfg.engines.insert(
        "clamav".to_string(),
        serde_json::json!({"address": "127.0.0.1:3310"}),
    );
    cfg.engines.insert(
        "custom".to_string(),
        serde_json::json!({"address": "127.0.0.1:9999"}),
    );
    assert_eq!(cfg.engines.len(), 2);
    assert_eq!(cfg.enabled.len(), 1);
}

// =========================================================================
// S11b 覆盖率冲刺：scanner run() 分发 arm + cmd_check 深路径（installed/
// failed/pending/db 状态、url 截断、recommendations、persist）+
// download_engine（本地 TcpListener 假 HTTP：非归档/404/坏 zip/tar.gz+
// detect）+ cmd_install / cmd_clamav_install_inner 离线全路径 +
// cmd_clamav enable/disable/update/info。
// 豁免（不测）：真网络下载 URL 分支（下载走本地假服务器覆盖）、
// freshclam 真跑、cmd_clamav_test（engine.start+2s sleep+exit(1)）、
// 全部 exit(1) 分支（cmd_add 非法名 / cmd_remove / cmd_enable 未配置 /
// install/update/info/test 无 clamav 配置）。
// =========================================================================

struct S11bTempHomeEnv {
    _tmp: tempfile::TempDir,
    home: std::path::PathBuf,
}

impl Drop for S11bTempHomeEnv {
    fn drop(&mut self) {
        unsafe { std::env::remove_var("NEMESISBOT_HOME") };
    }
}

fn s11b_temp_home_env() -> S11bTempHomeEnv {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(home.join("workspace").join("config")).unwrap();
    unsafe { std::env::set_var("NEMESISBOT_HOME", tmp.path()) };
    S11bTempHomeEnv { _tmp: tmp, home }
}

/// PATH → 空临时目录（RAII 恢复）。让 lookup_system_clamav 确定性返回
/// None（which 只扫 PATH）。必须持 GLOBAL_STATE_LOCK 使用。
struct S11bMinimalPathEnv {
    _tmp: tempfile::TempDir,
    old: Option<std::ffi::OsString>,
}

impl S11bMinimalPathEnv {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let old = std::env::var_os("PATH");
        unsafe { std::env::set_var("PATH", tmp.path().as_os_str()) };
        S11bMinimalPathEnv { _tmp: tmp, old }
    }
}

impl Drop for S11bMinimalPathEnv {
    fn drop(&mut self) {
        match self.old.take() {
            Some(v) => unsafe { std::env::set_var("PATH", v) },
            None => unsafe { std::env::remove_var("PATH") },
        }
    }
}

fn s11b_engine_json(
    url: &str,
    clamav_path: &str,
    address: &str,
    data_dir: &str,
    install_status: &str,
    db_status: &str,
) -> serde_json::Value {
    serde_json::json!({
        "url": url,
        "clamav_path": clamav_path,
        "address": address,
        "data_dir": data_dir,
        "state": {
            "install_status": install_status,
            "install_error": "",
            "db_status": db_status,
            "last_install_attempt": "",
            "last_db_update": ""
        }
    })
}

fn s11b_write_cfg(path: &std::path::Path, enabled: &[&str], engines: serde_json::Value) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        serde_json::json!({"enabled": enabled, "engines": engines}).to_string(),
    )
    .unwrap();
}

fn s11b_read_cfg(path: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// 本地假 HTTP 服务器（原始 TcpListener，不用真网络）：服务 `hits` 次后退出。
/// 返回 `http://127.0.0.1:PORT/<file>` URL。
fn s11b_serve(file: &str, status: u16, body: Vec<u8>, hits: usize) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let file = file.to_string();
    std::thread::spawn(move || {
        let mut served = 0usize;
        for stream in listener.incoming() {
            if served >= hits {
                break;
            }
            let mut stream = match stream {
                Ok(s) => s,
                Err(_) => break,
            };
            use std::io::{Read, Write};
            // 读完请求头（GET 无 body）
            let mut buf = [0u8; 4096];
            let mut req = Vec::new();
            loop {
                let n = stream.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break;
                }
                req.extend_from_slice(&buf[..n]);
                if req.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let reason = if status == 200 { "OK" } else { "Not Found" };
            let head = format!(
                "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                status,
                reason,
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
            let _ = stream.flush();
            served += 1;
        }
    });
    format!("http://{}/{}", addr, file)
}

// ------------------------------ run() 分发 --------------------------------

#[tokio::test]
async fn test_s11b_run_dispatch_arms() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_temp_home_env();

    run(ScannerAction::List, false).await.unwrap(); // 无配置
    run(
        ScannerAction::Add {
            name: "clamav".into(),
            url: Some("http://example.invalid/clamav.zip".into()),
            path: None,
            address: Some("127.0.0.1:1".into()),
        },
        false,
    )
    .await
    .unwrap();
    run(ScannerAction::List, false).await.unwrap();
    run(
        ScannerAction::Enable { name: "clamav".into() },
        false,
    )
    .await
    .unwrap();
    run(ScannerAction::Check, false).await.unwrap(); // enabled clamav 深路径
    run(
        ScannerAction::Disable { name: "clamav".into() },
        false,
    )
    .await
    .unwrap();
    run(
        ScannerAction::Remove { name: "clamav".into() },
        false,
    )
    .await
    .unwrap();
    run(ScannerAction::Check, false).await.unwrap(); // enabled 空 → 早退
    run(ScannerAction::Install { dir: None }, false)
        .await
        .unwrap(); // enabled 空 → 早退
    assert!(crate::common::scanner_config_path(&th.home).exists());
}

#[tokio::test]
async fn test_s11b_run_clamav_subcommand_arms() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_temp_home_env();
    let cfg_path = crate::common::scanner_config_path(&th.home);

    // 未配置 clamav → Disable 走 "not enabled"；Enable / Info / Update /
    // Install 的「未配置」分支全是 exit(1)（豁免），先 Add 配置再测。
    run(
        ScannerAction::Add {
            name: "clamav".into(),
            url: None,
            path: None,
            address: Some("127.0.0.1:1".into()),
        },
        false,
    )
    .await
    .unwrap();
    run(
        ScannerAction::Clamav {
            action: ClamavAction::Disable,
        },
        false,
    )
    .await
    .unwrap();
    // 未安装 → Enable bail
    assert!(
        run(
            ScannerAction::Clamav {
                action: ClamavAction::Enable,
            },
            false,
        )
        .await
        .is_err()
    );
    // Info：地址 127.0.0.1:1（连接拒绝）→ ready=false
    run(
        ScannerAction::Clamav {
            action: ClamavAction::Info,
        },
        false,
    )
    .await
    .unwrap();
    // Install（clamav install arm）：无 URL/无路径（PATH 收窄 → 无系统安装）
    // → FAILED: no download URL 落盘
    {
        let _path_guard = S11bMinimalPathEnv::new();
        run(
            ScannerAction::Clamav {
                action: ClamavAction::Install {
                    force: false,
                    url: None,
                    dir: None,
                },
            },
            false,
        )
        .await
        .unwrap();
    }
    let cfg = s11b_read_cfg(&cfg_path);
    assert_eq!(cfg["engines"]["clamav"]["state"]["install_status"], "failed");
    assert!(
        cfg["engines"]["clamav"]["state"]["install_error"]
            .as_str()
            .unwrap()
            .contains("no download URL")
    );
    // Update：无路径 + PATH 收窄 → bail "ClamAV not found"
    {
        let _path_guard = S11bMinimalPathEnv::new();
        let err = run(
            ScannerAction::Clamav {
                action: ClamavAction::Update,
            },
            false,
        )
        .await;
        assert!(err.is_err());
    }
}

// --------------------------- cmd_check 深路径 ------------------------------

#[test]
fn test_s11b_cmd_check_installed_failed_disabled_and_url_truncate() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg_path = tmp.path().join("config").join("config.scanner.json");

    let good = tmp.path().join("good_av");
    std::fs::create_dir_all(good.join("database")).unwrap();
    std::fs::write(good.join("clamd.exe"), "MZ").unwrap();
    std::fs::write(good.join("database").join("daily.cvd"), "db").unwrap();
    let bad = tmp.path().join("bad_av");
    std::fs::create_dir_all(&bad).unwrap();
    let long_url = format!("http://example.invalid/{}a.tar.gz", "x".repeat(40));

    s11b_write_cfg(
        &cfg_path,
        &["clamav", "stub"],
        serde_json::json!({
            "clamav": s11b_engine_json(&long_url, good.to_str().unwrap(), "127.0.0.1:1", "", "", ""),
            "stub":   s11b_engine_json("", bad.to_str().unwrap(), "", "", "", ""),
            "off":    s11b_engine_json("", "", "", "", "pending", "missing")
        }),
    );
    cmd_check(&cfg_path).unwrap();

    // 持久化校验：clamav=installed+ready；stub=failed+install_error；off 未动
    let cfg = s11b_read_cfg(&cfg_path);
    assert_eq!(cfg["engines"]["clamav"]["state"]["install_status"], "installed");
    assert_eq!(cfg["engines"]["clamav"]["state"]["db_status"], "ready");
    assert_eq!(cfg["engines"]["clamav"]["clamav_path"], good.to_str().unwrap());
    // 注意：cmd_check 的 marshal 第 4 参（data_dir）传 ""，不落 data_dir
    assert_eq!(cfg["engines"]["clamav"]["data_dir"], "");
    assert_eq!(cfg["engines"]["stub"]["state"]["install_status"], "failed");
    assert!(
        cfg["engines"]["stub"]["state"]["install_error"]
            .as_str()
            .unwrap()
            .contains("executable not found")
    );
    assert_eq!(cfg["engines"]["off"]["state"]["install_status"], "pending", "disabled 引擎不改状态");
}

#[test]
fn test_s11b_cmd_check_pending_and_recommendations() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let cfg_path = tmp.path().join("config").join("config.scanner.json");
    let ok_exe = tmp.path().join("ok_av");
    std::fs::create_dir_all(ok_exe.join("database")).unwrap();
    std::fs::write(ok_exe.join("clamd.exe"), "MZ").unwrap();
    // data_dir 指向空库目录 → db missing → "Run update" 建议
    let empty_db = tmp.path().join("empty_db");
    std::fs::create_dir_all(&empty_db).unwrap();

    s11b_write_cfg(
        &cfg_path,
        &["fresh", "broken", "nodb"],
        serde_json::json!({
            // 无路径 + PATH 收窄 → pending → "Run install" 建议
            "fresh":  s11b_engine_json("", "", "", "", "", ""),
            "broken": s11b_engine_json("", tmp.path().join("bad_av").to_str().unwrap(), "", "", "", ""),
            "nodb":   s11b_engine_json("", ok_exe.to_str().unwrap(), "", empty_db.to_str().unwrap(), "", "")
        }),
    );
    {
        let _path_guard = S11bMinimalPathEnv::new();
        cmd_check(&cfg_path).unwrap();
    }
    let cfg = s11b_read_cfg(&cfg_path);
    assert_eq!(cfg["engines"]["fresh"]["state"]["install_status"], "pending");
    assert_eq!(cfg["engines"]["broken"]["state"]["install_status"], "failed");
    assert_eq!(cfg["engines"]["nodb"]["state"]["db_status"], "missing");
    assert_eq!(cfg["engines"]["nodb"]["state"]["install_status"], "installed");
}

// ---------------------------- download_engine -----------------------------

#[tokio::test]
async fn test_s11b_download_engine_non_archive_and_404() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("dl");
    std::fs::create_dir_all(&target).unwrap();

    // 1) 非归档（URL 文件名 clamd.exe）→ 落盘即返回 target（根目录无子目录可 detect）
    let url = s11b_serve("clamd.exe", 200, b"MZ-fake".to_vec(), 1);
    let dir = download_engine(&url, &target).await.unwrap();
    assert_eq!(std::path::Path::new(&dir), target.as_path());
    assert_eq!(
        std::fs::read(target.join("clamd.exe")).unwrap(),
        b"MZ-fake".to_vec()
    );

    // 2) 404 → bail "HTTP 404 Not Found"
    let url404 = s11b_serve("engine.zip", 404, b"nope".to_vec(), 1);
    let err = download_engine(&url404, &target).await.unwrap_err();
    assert!(err.to_string().contains("HTTP 404"), "err={err}");
}

#[tokio::test]
async fn test_s11b_download_engine_invalid_zip_expandarchive_rc0_quirk() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("dl");
    std::fs::create_dir_all(&target).unwrap();
    // 坏 zip 在 Windows PowerShell 5.1 上的实际行为：Expand-Archive 把异常
    // 写进 error 流但进程退出码仍是 0（手工实证）→ 代码当作“解压成功”→
    // 删归档、返回 target。此处钉住该行为；“Could not auto-extract（归档
    // 保留）”分支需要 Expand-Archive 非零退出，本机 PS 5.1 对坏 zip 不产
    // 生，列为结构性豁免候选。
    let url = s11b_serve("engine.zip", 200, b"definitely not a zip".to_vec(), 1);
    let dir = download_engine(&url, &target).await.unwrap();
    assert_eq!(std::path::Path::new(&dir), target.as_path());
    assert!(
        !target.join("engine.zip").exists(),
        "Expand-Archive rc=0 → 代码视为解压成功并删除归档"
    );
}

#[tokio::test]
async fn test_s11b_download_engine_tar_gz_extracts_and_detects_exe_dir() {
    let tmp = tempfile::tempdir().unwrap();
    // 造真 tar.gz：payload/bin/clamd.exe
    let staging = tmp.path().join("staging");
    let payload_bin = staging.join("payload").join("bin");
    std::fs::create_dir_all(&payload_bin).unwrap();
    std::fs::write(payload_bin.join("clamd.exe"), b"MZ").unwrap();
    let tgz = tmp.path().join("pkg.tar.gz");
    let out = std::process::Command::new("tar")
        .args(["czf", &tgz.to_string_lossy(), "-C", &staging.to_string_lossy(), "payload"])
        .output()
        .expect("tar (Win10+ bsdtar) 应可用");
    assert!(out.status.success(), "tar czf 失败: {out:?}");
    let body = std::fs::read(&tgz).unwrap();

    let target = tmp.path().join("dl");
    std::fs::create_dir_all(&target).unwrap();
    let url = s11b_serve("pkg.tar.gz", 200, body, 1);
    let dir = download_engine(&url, &target).await.unwrap();
    // detect_executable_dir 应定位到含 clamd.exe 的子目录
    let detected = std::path::PathBuf::from(&dir);
    assert!(detected.join("clamd.exe").exists(), "detected={dir}");
    assert!(!target.join("pkg.tar.gz").exists(), "成功解压后归档被清理");
}

// ------------------------ cmd_install / install_inner ---------------------

#[tokio::test]
async fn test_s11b_cmd_install_empty_and_stub_and_delegate() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let cfg_path = tmp.path().join("config").join("config.scanner.json");

    // 空 enabled → 早退
    s11b_write_cfg(&cfg_path, &[], serde_json::json!({}));
    cmd_install(&cfg_path, None).await.unwrap();

    // 非 clamav 引擎 → "install not implemented"
    s11b_write_cfg(&cfg_path, &["stubengine"], serde_json::json!({}));
    cmd_install(&cfg_path, None).await.unwrap();

    // clamav → 委派 install_inner：无 URL（PATH 收窄）→ FAILED persisted
    s11b_write_cfg(
        &cfg_path,
        &["clamav"],
        serde_json::json!({"clamav": s11b_engine_json("", "", "127.0.0.1:1", "", "", "")}),
    );
    {
        let _path_guard = S11bMinimalPathEnv::new();
        cmd_install(&cfg_path, None).await.unwrap();
    }
    let cfg = s11b_read_cfg(&cfg_path);
    assert_eq!(cfg["engines"]["clamav"]["state"]["install_status"], "failed");
}

#[tokio::test]
async fn test_s11b_clamav_install_inner_full_offline_path() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg_path = tmp.path().join("config").join("config.scanner.json");
    let av = tmp.path().join("av");
    std::fs::create_dir_all(&av).unwrap();
    std::fs::write(av.join("clamd.exe"), "MZ").unwrap();

    // 初始未安装：step1 路径命中 → 校验过 → 生成 conf → freshclam 缺失 →
    // updater 快失败 → db missing → 持久化 installed
    s11b_write_cfg(
        &cfg_path,
        &["clamav"],
        serde_json::json!({"clamav": s11b_engine_json("", av.to_str().unwrap(), "127.0.0.1:1", "", "", "")}),
    );
    cmd_clamav_install_inner(false, None, None, &cfg_path)
        .await
        .unwrap();
    let cfg = s11b_read_cfg(&cfg_path);
    assert_eq!(cfg["engines"]["clamav"]["state"]["install_status"], "installed");
    assert_eq!(cfg["engines"]["clamav"]["state"]["db_status"], "missing");
    assert!(
        cfg["engines"]["clamav"]["state"]["last_install_attempt"]
            .as_str()
            .unwrap()
            .len()
            > 0,
        "last_install_attempt 已写入"
    );
    assert!(av.join("freshclam.conf").exists(), "freshclam.conf 已生成");
    assert!(av.join("clamd.conf").exists(), "clamd.conf 已生成");
    assert!(av.join("logs").exists(), "logs 目录已建");

    // 已安装 + !force → 早退（路径不变）
    cmd_clamav_install_inner(false, None, None, &cfg_path)
        .await
        .unwrap();

    // force → 重装路径（状态清空重走全流程）
    cmd_clamav_install_inner(true, None, None, &cfg_path)
        .await
        .unwrap();
    let cfg = s11b_read_cfg(&cfg_path);
    assert_eq!(cfg["engines"]["clamav"]["state"]["install_status"], "installed");

    // 路径无执行体 → failed "executable not found"
    let empty = tmp.path().join("empty_av");
    std::fs::create_dir_all(&empty).unwrap();
    s11b_write_cfg(
        &cfg_path,
        &["clamav"],
        serde_json::json!({"clamav": s11b_engine_json("", empty.to_str().unwrap(), "", "", "", "")}),
    );
    cmd_clamav_install_inner(false, None, None, &cfg_path)
        .await
        .unwrap();
    let cfg = s11b_read_cfg(&cfg_path);
    assert_eq!(cfg["engines"]["clamav"]["state"]["install_status"], "failed");
    assert!(
        cfg["engines"]["clamav"]["state"]["install_error"]
            .as_str()
            .unwrap()
            .contains("executable not found")
    );
}

#[tokio::test]
async fn test_s11b_clamav_install_inner_download_branches() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg_path = tmp.path().join("config").join("config.scanner.json");
    let install_dir = tmp.path().join("tools");

    // 1) 本地假服务器的 tar.gz → 下载解压 detect → installed
    let staging = tmp.path().join("staging");
    let payload_bin = staging.join("payload").join("bin");
    std::fs::create_dir_all(&payload_bin).unwrap();
    std::fs::write(payload_bin.join("clamd.exe"), b"MZ").unwrap();
    let tgz = tmp.path().join("pkg.tar.gz");
    let out = std::process::Command::new("tar")
        .args(["czf", &tgz.to_string_lossy(), "-C", &staging.to_string_lossy(), "payload"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let url = s11b_serve("pkg.tar.gz", 200, std::fs::read(&tgz).unwrap(), 1);

    s11b_write_cfg(
        &cfg_path,
        &["clamav"],
        serde_json::json!({"clamav": s11b_engine_json(&url, "", "", "", "", "")}),
    );
    cmd_clamav_install_inner(false, None, Some(install_dir.to_str().unwrap()), &cfg_path)
        .await
        .unwrap();
    let cfg = s11b_read_cfg(&cfg_path);
    assert_eq!(cfg["engines"]["clamav"]["state"]["install_status"], "installed");
    let detected = cfg["engines"]["clamav"]["clamav_path"].as_str().unwrap();
    assert!(detected.contains("clamav"), "detected={detected}");
    assert!(std::path::Path::new(detected).join("clamd.exe").exists());

    // 2) 死地址下载失败 → "download failed" 落盘 failed，返回 Ok
    s11b_write_cfg(
        &cfg_path,
        &["clamav"],
        serde_json::json!({"clamav": s11b_engine_json("http://127.0.0.1:1/x.zip", "", "", "", "", "")}),
    );
    cmd_clamav_install_inner(false, None, Some(install_dir.to_str().unwrap()), &cfg_path)
        .await
        .unwrap();
    let cfg = s11b_read_cfg(&cfg_path);
    assert_eq!(cfg["engines"]["clamav"]["state"]["install_status"], "failed");
    assert!(
        cfg["engines"]["clamav"]["state"]["install_error"]
            .as_str()
            .unwrap()
            .contains("download failed")
    );
}

// ------------------- cmd_clamav enable/disable/update/info ----------------

#[tokio::test]
async fn test_s11b_cmd_clamav_enable_disable_update_info() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let cfg_path = tmp.path().join("config").join("config.scanner.json");

    // 未配置 → bail
    s11b_write_cfg(&cfg_path, &[], serde_json::json!({}));
    assert!(cmd_clamav_enable(&cfg_path).is_err());

    // 已配置但未安装 → bail "not installed"
    s11b_write_cfg(
        &cfg_path,
        &[],
        serde_json::json!({"clamav": s11b_engine_json("", "", "", "", "pending", "")}),
    );
    let err = cmd_clamav_enable(&cfg_path).unwrap_err();
    assert!(err.to_string().contains("not installed"), "err={err}");

    // installed → enable 成功，enabled 列表加 clamav；再 disable
    s11b_write_cfg(
        &cfg_path,
        &[],
        serde_json::json!({"clamav": s11b_engine_json("", "", "", "", "installed", "")}),
    );
    cmd_clamav_enable(&cfg_path).unwrap();
    assert!(s11b_read_cfg(&cfg_path)["enabled"].as_array().unwrap().iter().any(|v| v == "clamav"));
    cmd_clamav_disable(&cfg_path).unwrap();
    assert!(!s11b_read_cfg(&cfg_path)["enabled"].as_array().unwrap().iter().any(|v| v == "clamav"));
    cmd_clamav_disable(&cfg_path).unwrap(); // 未启用 → "not enabled" Ok

    // update：无路径 + PATH 收窄 → bail "ClamAV not found"
    s11b_write_cfg(
        &cfg_path,
        &[],
        serde_json::json!({"clamav": s11b_engine_json("", "", "", "", "installed", "")}),
    );
    {
        let _path_guard = S11bMinimalPathEnv::new();
        let err = cmd_clamav_update(&cfg_path).await.unwrap_err();
        assert!(err.to_string().contains("ClamAV not found"), "err={err}");
    }

    // update：本地 av 目录（有 clamd.exe、无 freshclam）→ 生成 freshclam.conf
    // 后 updater 快失败（freshclam not found）→ Err("freshclam failed")
    let av = tmp.path().join("av");
    std::fs::create_dir_all(&av).unwrap();
    std::fs::write(av.join("clamd.exe"), "MZ").unwrap();
    s11b_write_cfg(
        &cfg_path,
        &[],
        serde_json::json!({"clamav": s11b_engine_json("", av.to_str().unwrap(), "", "", "installed", "")}),
    );
    let err = cmd_clamav_update(&cfg_path).await.unwrap_err();
    assert!(err.to_string().contains("freshclam"), "err={err}");
    assert!(av.join("freshclam.conf").exists(), "conf 缺失时先生成");

    // info：地址 127.0.0.1:1（连接拒绝）→ ready=false，Ok
    cmd_clamav_info(&cfg_path).await.unwrap();
}

// --------------------- PATH 收窄下 lookup 确定性 ---------------------------

#[test]
fn test_s11b_lookup_system_clamav_none_with_minimal_path() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _path_guard = S11bMinimalPathEnv::new();
    assert!(lookup_system_clamav().is_none());
}

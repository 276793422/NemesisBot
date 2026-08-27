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

// ===========================================================================
// wave_b（coverage 补测，2026-08-27）：miss 行清零补洞。
//
// 目标行（scanner.rs）与本模块的对应关系：
//  - 169-175（lookup_system_clamav 命中返 Some(parent)）→
//    wave_b_lookup_system_clamav_hits_fake_exe_in_narrow_path；
//  - 600-621 渲染矩阵（disabled 引擎四列全 "-"：install/db/address/url 空 ->
//    占位符）+ 653-681 推荐循环的 `_ => {}` 兜底臂（enabled 引擎带非标准
//    install_status="weird"，空 path + 窄 PATH 下 lookup=None 且状态非空 =>
//    状态机不覆写，"weird" 存活到 match 落 _）→
//    wave_b_check_disabled_dash_row_and_weird_state_fallback_arm；
//  - 552-570 状态机另一面：enabled + 空 path + lookup 命中 → 持久化发现的系统
//    路径（563-565）+ 617-620 长 URL 截断臂（纯 ASCII，>40 字节）→
//    wave_b_check_system_path_discovery_persists_and_truncates_url；
//  - 955-960 安装 Step2 系统 PATH 发现端到端（Step1 空 → Step2 命中假 exe →
//    Step4 判 installed → Step6 双 conf 生成 → Step7 updater 快失败 → Step8
//    落盘）→ wave_b_install_discovers_system_path_and_generates_confs；
//  - 1048-1051 / 1071（conf 生成的两个 Err 打印臂）——把 freshclam.conf /
//    clamd.conf 预建成目录逼 fs::write 失败 →
//    wave_b_install_reports_conf_generation_failures_but_continues；
//  - 1141-1152 update 的路径解析中段（配置 path 空 → 系统 PATH 发现）+
//    data_dir 空时的 fallback 臂 →
//    wave_b_update_resolves_path_via_system_lookup_when_unconfigured。
//
// ARTIFACT（span 归因伪影 / 死防御，无行为缺口）：
//  - 141 save_scanner_config 的 create_dir_all ?——调用方安全配置文件本身刚被
//    load 读过（父目录必存在），该 ? 是结构性恒 Ok 直行；
//  - 542 engines.get(name) 的 None => continue——all_names 本就来自
//    cfg.engines.keys()，恒命中，穷尽性防御臂；
//  - 1011-1015 install Step4 死防御——Step2/Step3 之后 detected_path 非空已被
//    上游 match 两臂（Some 继续 / None 直接落 Step3）保证。
//
// ALREADY（既有测试名证据）：下载三态主体（本地假服务器 s11b_serve 系列覆盖
// 964-1008 可达部分）、1101-1105（离线快失败断言 db missing，如
// test_s11b_..updater 快失败先例）、推荐 pending/failed/installed-update 三条
// 具体文案臂、enable/disable/info 往返（2085-2112 一段）。
//
// EXEMPT：
//  - 真 freshclam 成功路径（1096-1100、1160 后真实更新尾部 1188-1203）、
//    120s 超时臂（1106-1110）——需要真实病毒库下载或人为挂起外部进程；
//  - unzip 主机依赖臂（749-750、772-776：Expand-Archive 对坏 zip rc=0 的
//    平台怪癖已在源码注释钉住）、engine.start + sleep(2) + process::exit(1)
//    家族（382-387/440-441/457-461/481/836/869/914-916/1132-1134/1215-1216/
//    1253-1298）——进程级副作用与退出，测试纪律禁止；
//  - 646-648 changed 后保存失败的 Warning 臂——save 路径与 load 路径同一文件，
//    任何能让 save 失败的确定性 FS 故障会先打死 load（load 在其前执行并 ?），
//    注入窗口为空。
// ===========================================================================

mod wave_b {
    use super::{
        cmd_check, cmd_clamav_install_inner, cmd_clamav_update, lookup_system_clamav,
        s11b_engine_json, s11b_read_cfg, s11b_write_cfg,
    };
    use std::path::PathBuf;

    /// 把 PATH 收窄到一个临时目录；可选地预置假可执行文件。
    /// Drop 按 prev-value 恢复。必须持 crate::GLOBAL_STATE_LOCK 使用。
    struct WbPathEnv {
        _dir: tempfile::TempDir,
        old: Option<std::ffi::OsString>,
    }

    impl WbPathEnv {
        fn new(with_clamd: bool) -> Self {
            let dir = tempfile::tempdir().unwrap();
            if with_clamd {
                std::fs::write(dir.path().join("clamd.exe"), b"MZ\x90\x00").unwrap();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ =
                        std::fs::set_permissions(dir.path().join("clamd.exe"),
                            std::fs::Permissions::from_mode(0o755));
                }
            }
            let old = std::env::var_os("PATH");
            unsafe { std::env::set_var("PATH", dir.path().as_os_str()) };
            Self { _dir: dir, old }
        }

        fn fake_dir(&self) -> PathBuf {
            self._dir.path().to_path_buf()
        }
    }

    impl Drop for WbPathEnv {
        fn drop(&mut self) {
            match self.old.take() {
                Some(v) => unsafe { std::env::set_var("PATH", v) },
                None => unsafe { std::env::remove_var("PATH") },
            }
        }
    }

    #[test]
    fn wave_b_lookup_system_clamav_hits_fake_exe_in_narrow_path() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let env = WbPathEnv::new(true);
        let hit = lookup_system_clamav().expect("fake clamd.exe must be discovered");
        assert_eq!(
            std::path::Path::new(&hit),
            env.fake_dir(),
            "返回值必须是含 exe 的目录：{hit}"
        );
    }

    #[test]
    fn wave_b_check_disabled_dash_row_and_weird_state_fallback_arm() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("config.scanner.json");

        // avoff：disabled + 状态字段全空 → 四个占位符列（"-"/"-"/"-"/"-"）。
        // clamav：enabled + 非 install_status 值 + 无 path —— 窄 PATH 下
        // lookup=None 且状态非空，状态机不覆写 ⇒ "weird" 存活进推荐 match
        // 的 `_ => {}` 兜底臂。
        s11b_write_cfg(
            &cfg_path,
            &["clamav"],
            serde_json::json!({
                "avoff": {
                    "url": "", "clamav_path": "", "address": "",
                    "data_dir": "",
                    "state": {"install_status": "", "install_error": "",
                              "db_status": "", "last_install_attempt": "",
                              "last_db_update": ""}
                },
                "clamav": {
                    "url": "", "clamav_path": "", "address": "",
                    "data_dir": "",
                    "state": {"install_status": "weird", "install_error": "",
                              "db_status": "", "last_install_attempt": "",
                              "last_db_update": ""}
                }
            }),
        );

        let _path_env = WbPathEnv::new(false); // 保证 lookup 确定性 None
        cmd_check(&cfg_path).expect("check must render disabled rows and survive weird state");

        // 断言 disabled 行不被持久化改写、weird 也未被状态机覆盖成 pending。
        let cfg = s11b_read_cfg(&cfg_path);
        assert_eq!(cfg["engines"]["avoff"]["state"]["install_status"], "");
        assert_eq!(cfg["engines"]["clamav"]["state"]["install_status"], "weird");
    }

    #[test]
    fn wave_b_check_system_path_discovery_persists_and_truncates_url() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("config.scanner.json");
        // 长 URL（54 字节 ASCII）触发 >40 截断臂（floor 边界内纯 ASCII，
        // 有意避开生产可疑点的多字节切片风险）。
        let long_url = "https://downloads.example.com/clamav/clamav-1.4.1.zip";
        assert!(long_url.len() > 40);

        s11b_write_cfg(
            &cfg_path,
            &["clamav"],
            serde_json::json!({"clamav": s11b_engine_json(long_url, "", "", "", "", "")}),
        );

        let env = WbPathEnv::new(true);
        cmd_check(&cfg_path).expect("check with PATH discovery must succeed");

        // 发现的系统路径必须被 marshal 回配置（persist_path 写入）。
        let cfg = s11b_read_cfg(&cfg_path);
        assert_eq!(
            cfg["engines"]["clamav"]["clamav_path"],
            env.fake_dir().to_string_lossy().as_ref(),
            "PATH 发现结果要持久化"
        );
        assert_eq!(cfg["engines"]["clamav"]["state"]["install_status"], "installed");
        // database 目录不存在于假目录 ⇒ db_status 走 missing 探测分支。
        assert_eq!(cfg["engines"]["clamav"]["state"]["db_status"], "missing");
    }

    #[tokio::test]
    async fn wave_b_install_discovers_system_path_and_generates_confs() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("config.scanner.json");
        s11b_write_cfg(
            &cfg_path,
            &[],
            serde_json::json!({"clamav": s11b_engine_json("", "", "", "", "pending", "")}),
        );

        let env = WbPathEnv::new(true);
        cmd_clamav_install_inner(false, None, None, &cfg_path)
            .await
            .expect("install via system PATH discovery must complete");

        // 落盘校验：发现路径 + installed 状态 + 双 conf 实际生成在发现目录。
        let cfg = s11b_read_cfg(&cfg_path);
        assert_eq!(
            cfg["engines"]["clamav"]["clamav_path"],
            env.fake_dir().to_string_lossy().as_ref()
        );
        assert_eq!(cfg["engines"]["clamav"]["state"]["install_status"], "installed");
        assert!(env.fake_dir().join("freshclam.conf").exists());
        assert!(env.fake_dir().join("clamd.conf").exists());
        // freshclam 二进制缺席 ⇒ DB 更新快失败为 missing，但不影响安装成功语义。
        assert_eq!(cfg["engines"]["clamav"]["state"]["db_status"], "missing");
    }

    #[tokio::test]
    async fn wave_b_install_reports_conf_generation_failures_but_continues() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("config.scanner.json");

        // 配置路径直指本 tempdir 内已备好的 av 目录（Step1 命中，不走 PATH）。
        let av = tmp.path().join("av");
        std::fs::create_dir_all(&av).unwrap();
        std::fs::write(av.join("clamd.exe"), b"MZ").unwrap();

        s11b_write_cfg(
            &cfg_path,
            &[],
            serde_json::json!({"clamav": s11b_engine_json(
                "", av.to_str().unwrap(), "", "", "pending", "")}),
        );

        // 把两个 conf 目标预建成【目录】：生成器的 fs::write 打不开目录 ⇒
        // 两个 Err 打印臂各命中一次。装完流程必须继续而非中断。
        std::fs::create_dir_all(av.join("freshclam.conf")).unwrap();
        std::fs::create_dir_all(av.join("clamd.conf")).unwrap();

        cmd_clamav_install_inner(false, None, None, &cfg_path)
            .await
            .expect("conf generation failures must be reported, not fatal");

        let cfg = s11b_read_cfg(&cfg_path);
        assert_eq!(cfg["engines"]["clamav"]["state"]["install_status"], "installed",
            "conf 失败只警告，安装状态照常落盘");
    }

    #[tokio::test]
    async fn wave_b_update_resolves_path_via_system_lookup_when_unconfigured() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("config.scanner.json");
        // clamav_path 与 data_dir 都为空 ⇒ 路径靠系统 PATH 发现、data_dir 走
        // fallback（= 发现目录）。发现目录里没有 freshclam 二进制 ⇒ 更新阶段
        // 快失败 Err("freshclam ...")，全程离线确定。
        s11b_write_cfg(
            &cfg_path,
            &[],
            serde_json::json!({"clamav": s11b_engine_json("", "", "", "", "installed", "")}),
        );

        let env = WbPathEnv::new(true);
        let err = cmd_clamav_update(&cfg_path).await.unwrap_err();
        assert!(err.to_string().contains("freshclam"), "err={err}");
        // 反证路径解析确实落在发现目录：freshclam.conf 生成到了那里。
        assert!(
            env.fake_dir().join("freshclam.conf").exists(),
            "update 必须把 conf 生成到 PATH 发现目录"
        );
    }
}

// ---------------------------------------------------------------------------
// BUG#35 回归：URL 列展示截断必须落在 char boundary（含非 ASCII 的 URL
// 曾因裸 &url[..37] 直接 panic）。三态：短原样 / 长 ASCII 精确 37 /
// 长多字节不 panic 且边界安全。
// ---------------------------------------------------------------------------

#[test]
fn test_url_display_truncated_short_is_verbatim() {
    let u = "http://127.0.0.1:3310";
    assert_eq!(url_display_truncated(u), u);
}

#[test]
fn test_url_display_truncated_long_ascii_cuts_exactly_37() {
    let u = format!("http://example.com/{}", "a".repeat(60));
    let d = url_display_truncated(&u);
    assert!(d.ends_with("..."));
    assert_eq!(&d[..37], &u[..37]);
    assert_eq!(d.len(), 40, "37 字节正文 + 3 字符省略号");
}

#[test]
fn test_url_display_truncated_multibyte_never_panics_and_stays_on_boundary() {
    // 22 字节 ASCII 前缀 + 10 个中文（30 字节）：总长 >40，且第 37 字节
    // 落在某个 3 字节汉字内部 —— 修复前此调用当场 panic。
    let mut u = String::from("https://example.com/p/");
    for _ in 0..10 {
        u.push('中');
    }
    assert!(u.len() > 40);

    let d = url_display_truncated(&u);
    assert!(d.ends_with("..."));
    let body = &d[..d.len() - 3];
    assert!(
        body.len() <= 37,
        "截断点不得越过请求的 37 字节上限"
    );
    assert!(u.starts_with(body), "展示头必须是原文前缀");
    assert!(
        u.is_char_boundary(body.len()),
        "截断点必须是 char boundary（否则后续 [..] 场景仍会炸）"
    );
}

// ===========================================================================
// r10 批（coverage 补测，2026-08-27）：A 类 miss 收尾。目标行（scanner.rs）
// 与本模块的对应：
//
// 直接命中（in-process / 子进程）：
//  - 169-173 lookup_system_clamav 命中返回段（仅 clamscan.exe 在窄 PATH，
//    走迭代序号 1 命中，补 wave_b 只用 clamd.exe 的另一形态）；
//  - 651-655 changed==false 收尾臂（enabled 引擎不在 engines map：循环零迭代）；
//  - 382-387 / 440-441 / 457-461 / 920-922 / 1138-1140 / 1221-1222 /
//    1262-1266 —— std::process::exit(1) 家族 + ClamavAction::Test 分发臂
//    （875）：全部经【真 exe 子进程】观察退码 1（in-process 必杀测试进程）；
//  - 755-756 download_engine 的 unzip 成功臂（最小 stored-zip 手工字节档）；
//  - 777-783 双失败臂：坏 zip 且 PowerShell 解析错误退码非零（文件名内嵌
//    单引号打破 ps_cmd 的单引号包裹 —— 已实证 ParserError → rc=1）；
//  - 842 detect_executable_dir 对不存在根目录 read_dir Err → None；
//  - 1080-1084 Step7「freshclam.conf 未生成 → 跳过 DB 更新」臂：
//    把 <av>/database 预建成普通文件逼 generate_freshclam_config 在写 conf
//    之前失败（config.rs 先 create_dir_all(db_dir) 再写 conf）；
//  - ⭐1102-1106 install_inner Step7 更新成功链 + 1147-1211 update 成功链
//    （含 1156 data_dir 已配置臂、1176 conf 生成、1194 成功打印、1196-1209
//    持久化 + 1202-1204 空 path 回填臂、1211）：freshclam 桩 = 复制
//    cmd.exe 为 freshclam.exe —— updater 只看「存在 + 退出码 0」，而
//    cmd.exe 无 /c 时忽略多余 dash 参数、stdin 读到 EOF 立即退出 0；两个
//    消费者都起成 stdin=null 的 CLI 子进程，EOF 语义跨环境确定。
//  - ⭐1282-1304 cmd_clamav_test 三态：mock clamd = 复制本测试二进制为
//    <X>\clamd.exe 并以 libtest filter 跑内置服务用例（唯一能同时满足
//    ownership 的 QueryFullProcessImageNameW 路径匹配 == 该字面路径且会说
//    PING/PONG 协议的办法）；is_ready→SCAN→SHUTDOWN 全协议过 mock。
//
// ARTIFACT / 结构性豁免（本轮重审后仍不测）：
//  - 1235 Version 打印：create_engine 返回 ClamavScannerWrapper /
//    ClamAVEngine 两者的 get_info().version 恒为空字符串（无任何 shell-out
//    取版本路径），该 println 门恒 false —— 不改生产代码不可达；
//  - 1296-1299 与 1299-1301 两个互斥打印分支由 infected/clean 两次子进
//    程分别点亮；CLEAN 原始分支历史测试已有等价覆盖路径者不再重复统计。
//
// 环境 RVA（诚实边界）：
//  - 子进程家族依赖 resolve_nemesisbot_bin（target/release 或 debug 先建好），
//    缺失时响亮 panic（与 r9 同款纪律，不做静默 skip）；
//  - 755-756 需要 PATH 可解析 unzip（git-bash 自带）；缺席时 download_engine
//    自然落入 PowerShell Expand-Archive 成功臂，断言（解压成功 + 归档删除）
//    不受影响，但那一轮不会点亮 755-756 本身。
// ===========================================================================

mod r10_process_boundary {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    /// S3 minimal-path guard 的兄弟实现（wave_b 版是 mod 私有，借不到）。
    /// Drop 按 prev-value 恢复。必须持 crate::GLOBAL_STATE_LOCK 使用。
    struct R10NarrowPath {
        _dir: tempfile::TempDir,
        old: Option<std::ffi::OsString>,
    }

    impl R10NarrowPath {
        fn new(files: &[&str]) -> Self {
            let dir = tempfile::tempdir().unwrap();
            for f in files {
                std::fs::write(dir.path().join(f), b"MZ").unwrap();
            }
            let old = std::env::var_os("PATH");
            unsafe { std::env::set_var("PATH", dir.path().as_os_str()) };
            R10NarrowPath { _dir: dir, old }
        }

        fn dir(&self) -> PathBuf {
            self._dir.path().to_path_buf()
        }
    }

    impl Drop for R10NarrowPath {
        fn drop(&mut self) {
            match self.old.take() {
                Some(v) => unsafe { std::env::set_var("PATH", v) },
                None => unsafe { std::env::remove_var("PATH") },
            }
        }
    }

    // ---------------------------------------------------------------------
    // 子进程装置（r9_process_boundary 的同步版 + 显式 home/env 注入）
    // ---------------------------------------------------------------------

    struct R10Outcome {
        code: i32,
        stdout: String,
        stderr: String,
    }

    fn r10_system_root() -> PathBuf {
        std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .filter(|p| p.exists())
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
    }

    /// 给子进程用的合成 PATH：<prepend> 头插 + 最小系统目录。
    /// 不读父进程 PATH，杜绝与并行 env 用例的任何耦合。
    fn r10_synthetic_path(prepend: Option<&Path>) -> String {
        let root = r10_system_root();
        let mut parts: Vec<String> = Vec::new();
        if let Some(p) = prepend {
            parts.push(p.to_string_lossy().into_owned());
        }
        parts.push(root.join("System32").to_string_lossy().into_owned());
        parts.push(root.to_string_lossy().into_owned());
        parts.join(";")
    }

    /// 复制 cmd.exe 到 dest_dir/<name> 并返回该路径。
    ///
    /// 作桩的原理：cmd.exe 收到非 `/` 开头的自有开关一律忽略（实测
    /// `cmd --config-file x --datadir y` rc=0），无 /c 时进入交互读 stdin，
    /// 而 std 里父进程给了 null 句柄 ⇒ 立即 EOF ⇒ 退出码 0。updater 只验
    /// 「文件存在 + exit 0」，故成立；绝不产生窗口（console 继承宿主控制台）。
    fn r10_copy_cmd_as(dest_dir: &Path, name: &str) -> PathBuf {
        let src = r10_system_root().join("System32").join("cmd.exe");
        assert!(src.exists(), "System32\\cmd.exe 必须存在: {}", src.display());
        let dst = dest_dir.join(name);
        std::fs::copy(&src, &dst)
            .unwrap_or_else(|e| panic!("复制 cmd.exe 为 {} 失败: {}", name, e));
        dst
    }

    struct R10Child {
        child: Option<std::process::Child>,
    }

    impl Drop for R10Child {
        fn drop(&mut self) {
            if let Some(mut c) = self.child.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }

    /// 起裸子进程：stdin null（cmd.exe 桩读 EOF 立退 / 服务型不被终端牵住）、
    /// stdout/stderr 全捕获、显式 env + CLI 覆盖 profile。
    fn r10_spawn_raw(
        bin: &Path,
        args: &[&str],
        env_set: &[(&str, &str)],
    ) -> Result<std::process::Child, std::io::Error> {
        let mut cmd = Command::new(bin);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in env_set {
            cmd.env(k, v);
        }
        cmd.envs(test_harness::coverage_cli_env());
        cmd.spawn()
    }

    /// 起 bin + args + env 并等到退出；deadline 内未退则 kill，code=-2 由
    /// 调用方响亮断言。适合「跑完即退」的驱动 CLI 与 mock 镜像以外的场景。
    fn r10_spawn(
        bin: &Path,
        args: &[&str],
        env_set: &[(&str, &str)],
        deadline_secs: u64,
    ) -> (R10Child, R10Outcome) {
        let child = match r10_spawn_raw(bin, args, env_set) {
            Ok(c) => c,
            Err(e) => {
                return (
                    R10Child { child: None },
                    R10Outcome {
                        code: -1,
                        stdout: String::new(),
                        stderr: format!("failed to spawn: {}", e),
                    },
                )
            }
        };
        let mut guard = R10Child { child: Some(child) };
        let outcome = r10_reap(guard.child.as_mut().expect("just set"), deadline_secs);
        (guard, outcome)
    }

    fn r10_reap(child: &mut std::process::Child, deadline_secs: u64) -> R10Outcome {
        let deadline = Instant::now() + Duration::from_secs(deadline_secs);
        loop {
            match child.try_wait().expect("try_wait failed") {
                Some(_) => break,
                None if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return R10Outcome {
                        code: -2,
                        stdout: String::new(),
                        stderr: format!("timed out after {}s", deadline_secs),
                    };
                }
                None => std::thread::sleep(Duration::from_millis(100)),
            }
        }
        let mut out = String::new();
        if let Some(mut s) = child.stdout.take() {
            let _ = s.read_to_string(&mut out);
        }
        let mut err = String::new();
        if let Some(mut s) = child.stderr.take() {
            let _ = s.read_to_string(&mut err);
        }
        let code = child.wait().ok().and_then(|st| st.code()).unwrap_or(-1);
        R10Outcome {
            code,
            stdout: out,
            stderr: err,
        }
    }

    fn r10_bin() -> PathBuf {
        test_harness::resolve_nemesisbot_bin().expect("nemesisbot binary resolved")
    }

    /// 准备隔离 home：env 放 `<tmp>`，则子进程 resolve 得 `<tmp>\.nemesisbot`。
    /// 返回 (cfg 文件路径)。工作目录照 r9 惯例中立化（temp 根）。
    fn r10_stage_home(env_home_base: &Path) -> PathBuf {
        let home = env_home_base.join(".nemesisbot");
        let cfg_dir = home.join("workspace").join("config");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        crate::common::scanner_config_path(&home)
    }

    fn r10_read_cfg(cfg: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(cfg).unwrap()).unwrap()
    }

    // ------------------------- in-process 碎件 ----------------------------

    /// 169-173：窄 PATH 下只有 clamscan.exe —— 迭代第 1 个名字命中（wave_b
    /// 只钉了 clamd.exe 的第 0 个），覆盖同一返回段的另一入口顺序。
    #[test]
    fn r10_lookup_system_clamav_hits_via_clamscan_only() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let env = R10NarrowPath::new(&["clamscan.exe"]);
        let hit = lookup_system_clamav().expect("clamscan.exe 必须 which 命中");
        assert_eq!(
            Path::new(&hit),
            env.dir().as_path(),
            "返回值必须是含可执行文件的目录: {hit}"
        );
    }

    /// 651-655：changed 保持 false 的收尾臂 —— enabled 名字在 engines map
    /// 中不存在 ⇒ 渲染循环零迭代，既不 marshal 也绝不触发保存重写。
    #[test]
    fn r10_cmd_check_skips_save_when_enabled_name_absent_from_engines_map() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("config.scanner.json");

        s11b_write_cfg(&cfg_path, &["ghost"], serde_json::json!({}));
        let before = std::fs::read_to_string(&cfg_path).unwrap();

        cmd_check(&cfg_path).expect("幽灵 enabled 名不能崩 check");

        let after = std::fs::read_to_string(&cfg_path).unwrap();
        assert_eq!(
            before, after,
            "changed=false 不得重写配置文件（紧凑输入若被 pretty 重写会立刻暴露）"
        );
    }

    /// 842：根目录不存在 → read_dir Err 直落 None。
    #[test]
    fn r10_detect_executable_dir_missing_root_returns_none() {
        let missing = tempfile::tempdir()
            .unwrap()
            .path()
            .join("already_removed_subdir");
        assert!(detect_executable_dir(&missing, &["clamd.exe"]).is_none());
    }

    /// 1080-1084：conf 生成失败（database 被预建成普通文件，generator 在写
    /// conf 之前先 create_dir_all(db_dir) 即败）⇒ Step7 走「跳过 DB 更新」臂。
    #[tokio::test]
    async fn r10_install_inner_skips_db_update_when_conf_generation_failed() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("config.scanner.json");
        let av = tmp.path().join("av");
        std::fs::create_dir_all(&av).unwrap();
        std::fs::write(av.join("clamd.exe"), b"MZ").unwrap();
        // 致命道具：<av>/database 是【文件】，让 generator 的 create_dir_all
        // 炸掉并保持在写 freshclam.conf 之前 ⇒ conf 从不存在。
        std::fs::write(av.join("database"), b"not a dir").unwrap();

        s11b_write_cfg(
            &cfg_path,
            &[],
            serde_json::json!({"clamav": s11b_engine_json(
                "", av.to_str().unwrap(), "", "", "pending", "")}),
        );

        cmd_clamav_install_inner(false, None, None, &cfg_path)
            .await
            .expect("conf 生成失败只警告不中断");

        let cfg = s11b_read_cfg(&cfg_path);
        assert_eq!(cfg["engines"]["clamav"]["state"]["install_status"], "installed");
        assert_eq!(cfg["engines"]["clamav"]["state"]["db_status"], "missing",
            "conf 缺失 ⇒ 明确标记 DB missing，而不是装作 ready");
        assert!(!av.join("freshclam.conf").exists(),
            "生成器失败后 conf 必须保持缺席（Step7 skip 臂的前提）");
    }

    // ----------------------- exit(1) 家族（子进程）-----------------------

    fn r10_expect_rc1(o: &R10Outcome, what: &str, marker: &str) {
        assert_eq!(
            o.code, 1,
            "{} 必须 exit(1):\n--- stdout ---\n{}\n--- stderr ---\n{}",
            what, o.stdout, o.stderr
        );
        assert!(
            o.stderr.contains(marker) || o.stdout.contains(marker),
            "{} 缺少标志串 {:?}:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            what,
            marker,
            o.stdout,
            o.stderr
        );
    }

    /// 382-387：非法引擎名 → Unknown engine + Available 列表 + exit(1)。
    /// 合法性检查在 load 配置之前 ⇒ 天然与隔离 home 内容无关。
    #[test]
    fn r10_cli_add_unknown_engine_exits_1() {
        let tmp = tempfile::tempdir().unwrap();
        let o = r10_spawn(
            &r10_bin(),
            &["scanner", "add", "totally_not_an_engine"],
            &[("NEMESISBOT_HOME", tmp.path().to_str().unwrap())],
            120,
        )
        .1;
        r10_expect_rc1(&o, "scanner add 非法名", "Unknown engine:");
        assert!(o.stderr.contains("Available:"), "必须回显可用列表:\n{}", o.stderr);
    }

    /// 440-441：remove 一个不存在的引擎名 → exit(1)。
    #[test]
    fn r10_cli_remove_unknown_engine_exits_1() {
        let tmp = tempfile::tempdir().unwrap();
        let o = r10_spawn(
            &r10_bin(),
            &["scanner", "remove", "ghost"],
            &[("NEMESISBOT_HOME", tmp.path().to_str().unwrap())],
            120,
        )
        .1;
        r10_expect_rc1(&o, "scanner remove 未知名", "not found in configuration");
    }

    /// 457-461：enable 一个未配置的引擎名 → exit(1)。
    #[test]
    fn r10_cli_enable_unknown_engine_exits_1() {
        let tmp = tempfile::tempdir().unwrap();
        let o = r10_spawn(
            &r10_bin(),
            &["scanner", "enable", "ghost"],
            &[("NEMESISBOT_HOME", tmp.path().to_str().unwrap())],
            120,
        )
        .1;
        r10_expect_rc1(&o, "scanner enable 未知名", "Add it first");
    }

    /// 920-922：scanner clamav install 而 engines map 没有 clamav → exit(1)。
    #[test]
    fn r10_cli_clamav_install_unconfigured_exits_1() {
        let tmp = tempfile::tempdir().unwrap();
        let o = r10_spawn(
            &r10_bin(),
            &["scanner", "clamav", "install"],
            &[("NEMESISBOT_HOME", tmp.path().to_str().unwrap())],
            120,
        )
        .1;
        r10_expect_rc1(&o, "clamav install 未配置", "ClamAV engine not found");
    }

    /// 1138-1140：scanner clamav update 未配置 → exit(1)。
    #[test]
    fn r10_cli_clamav_update_unconfigured_exits_1() {
        let tmp = tempfile::tempdir().unwrap();
        let o = r10_spawn(
            &r10_bin(),
            &["scanner", "clamav", "update"],
            &[("NEMESISBOT_HOME", tmp.path().to_str().unwrap())],
            120,
        )
        .1;
        r10_expect_rc1(&o, "clamav update 未配置", "ClamAV engine not found");
    }

    /// 1221-1222：scanner clamav info 未配置 → exit(1)。
    #[test]
    fn r10_cli_clamav_info_unconfigured_exits_1() {
        let tmp = tempfile::tempdir().unwrap();
        let o = r10_spawn(
            &r10_bin(),
            &["scanner", "clamav", "info"],
            &[("NEMESISBOT_HOME", tmp.path().to_str().unwrap())],
            120,
        )
        .1;
        r10_expect_rc1(&o, "clamav info 未配置", "ClamAV engine not found");
    }

    /// 1262-1266（头部未配置 exit(1)）+ 875（ClamavAction::Test 分发臂）：
    /// 同一次子进程同时点亮两处 —— Test 分发此前没有任何存活调用路径。
    #[test]
    fn r10_cli_clamav_test_unconfigured_exits_1_and_lights_dispatch_arm() {
        let tmp = tempfile::tempdir().unwrap();
        let sample = tmp.path().join("sample.vx9");
        std::fs::write(&sample, b"whatever").unwrap();
        let o = r10_spawn(
            &r10_bin(),
            &["scanner", "clamav", "test", sample.to_str().unwrap()],
            &[("NEMESISBOT_HOME", tmp.path().to_str().unwrap())],
            120,
        )
        .1;
        r10_expect_rc1(&o, "clamav test 未配置", "ClamAV engine not found");
    }

    // --------------------- download_engine 归档两侧 -----------------------

    /// 最小 stored-zip 构造器（IEEE CRC32 + 本地头 + 中央目录 + EOCD）。
    fn r10_crc32(data: &[u8]) -> u32 {
        let mut c: u32 = 0xFFFF_FFFF;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
            }
        }
        !c
    }

    fn r10_stored_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        let mut central: Vec<u8> = Vec::new();
        for (name, data) in entries {
            let offset = out.len() as u32;
            let crc = r10_crc32(data);
            let nb = name.as_bytes();
            out.extend_from_slice(&[0x50, 0x4B, 0x03, 0x04]); // local sig
            out.extend_from_slice(&20u16.to_le_bytes()); // version needed
            out.extend_from_slice(&0u16.to_le_bytes()); // flags
            out.extend_from_slice(&0u16.to_le_bytes()); // method = stored
            out.extend_from_slice(&0u16.to_le_bytes()); // mod time
            out.extend_from_slice(&0u16.to_le_bytes()); // mod date
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(nb.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // extra len
            out.extend_from_slice(nb);
            out.extend_from_slice(data);

            central.extend_from_slice(&[0x50, 0x4B, 0x01, 0x02]); // central sig
            central.extend_from_slice(&20u16.to_le_bytes()); // version made by
            central.extend_from_slice(&20u16.to_le_bytes()); // version needed
            central.extend_from_slice(&0u16.to_le_bytes()); // flags
            central.extend_from_slice(&0u16.to_le_bytes()); // method
            central.extend_from_slice(&0u16.to_le_bytes()); // time/date x2
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(nb.len() as u16).to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes()); // extra
            central.extend_from_slice(&0u16.to_le_bytes()); // comment
            central.extend_from_slice(&0u16.to_le_bytes()); // disk #
            central.extend_from_slice(&0u16.to_le_bytes()); // int attrs
            central.extend_from_slice(&0u32.to_le_bytes()); // ext attrs
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(nb);
        }
        let cd_offset = out.len() as u32;
        let cd_size = central.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&[0x50, 0x4B, 0x05, 0x06]); // EOCD
        out.extend_from_slice(&0u16.to_le_bytes()); // this disk
        out.extend_from_slice(&0u16.to_le_bytes()); // cd disk
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // comment len
        out
    }

    /// 744-757 成功臂（755-756 删归档 + 返回 target）：手工 stored-zip 经
    /// 本地假服务器喂入；unzip 可用时走 unzip 成功臂（755-756），缺席时落
    /// PS Expand-Archive 成功臂 —— 两种外部状态下行为断言一致。
    #[tokio::test]
    async fn r10_download_zip_success_removes_archive_and_returns_target() {
        let payload = b"clean payload text\n".to_vec();
        let zip = r10_stored_zip(&[("z.txt", &payload)]);

        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("dl");
        std::fs::create_dir_all(&target).unwrap();

        let url = s11b_serve("engine.zip", 200, zip, 1);
        let dir = download_engine(&url, &target).await.unwrap();

        assert_eq!(Path::new(&dir), target.as_path());
        assert_eq!(std::fs::read(target.join("z.txt")).unwrap(), payload);
        assert!(
            !target.join("engine.zip").exists(),
            "成功解压后归档必须被清理（unzip 或 PS 任一通道）"
        );
    }

    /// 758-783 双失败臂：坏字节 + URL 文件名内嵌单引号 ⇒ powershell 的
    /// `-Command` 单引号包裹被打破 → ParserError → 退出码必然非 0（已在本机
    /// 实证 rc=1），unzip 对垃圾字节也必败。归档保留原地的兜底打印被点亮。
    #[tokio::test]
    async fn r10_download_corrupt_zip_double_failure_keeps_archive_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("dl");
        std::fs::create_dir_all(&target).unwrap();

        // eng'ine.zip —— 存档路径带单引号，destination 路径干净也无所谓，
        // 第一个被打破的引号串就足以让整条命令解析失败。
        let url = s11b_serve("eng'ine.zip", 200, b"definitely not a zip".to_vec(), 1);
        let dir = download_engine(&url, &target).await.unwrap();

        assert_eq!(Path::new(&dir), target.as_path());
        assert!(
            target.join("eng'ine.zip").exists(),
            "双失败臂必须保留归档供人工处理"
        );
        assert!(!target.join("z.txt").exists(), "坏包不应解出任何内容");
    }

    // -------------------- ⭐ freshclam 更新成功链 -------------------------

    /// 1102-1106（Step7 Ok(Ok(())) 成功臂）+ 1122-1125 persist：CLI 子进程跑
    /// `scanner clamav install`；freshclam 桩 = cmd.exe 副本（见
    /// r10_copy_cmd_as）。子进程 stdin=null ⇒ 桩读 EOF 秒退 0 ⇒ updater 判
    /// 成功 ⇒ db_status=ready 持久化。
    #[test]
    fn r10_cli_install_full_chain_marks_db_ready_via_stub_freshclam() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = r10_stage_home(tmp.path());

        let av = tmp.path().join("av");
        std::fs::create_dir_all(&av).unwrap();
        std::fs::write(av.join("clamd.exe"), b"MZ").unwrap();
        r10_copy_cmd_as(&av, "freshclam.exe");

        s11b_write_cfg(
            &cfg,
            &[],
            serde_json::json!({"clamav": s11b_engine_json(
                "", av.to_str().unwrap(), "", "", "pending", "")}),
        );

        let o = r10_spawn(
            &r10_bin(),
            &["scanner", "clamav", "install"],
            &[("NEMESISBOT_HOME", tmp.path().to_str().unwrap())],
            180,
        )
        .1;
        assert_eq!(
            o.code, 0,
            "install 全链必须 rc=0:\n{}\n--- stderr ---\n{}",
            o.stdout, o.stderr
        );
        assert!(o.stdout.contains("virus database ready"), "stdout:\n{}", o.stdout);

        let after = r10_read_cfg(&cfg);
        assert_eq!(after["engines"]["clamav"]["state"]["install_status"], "installed");
        assert_eq!(after["engines"]["clamav"]["state"]["db_status"], "ready");
        assert!(
            after["engines"]["clamav"]["state"]["last_db_update"]
                .as_str()
                .map(|s| !s.is_empty())
                .unwrap_or(false),
            "last_db_update 必须盖章"
        );
        assert!(av.join("freshclam.conf").exists());
        assert!(av.join("database").exists(), "updater 会先建 datadir");
    }

    /// 1147-1211 update 成功链：path 空 ⇒ 系统 PATH 发现（合成 PATH 头插桩
    /// 目录）⇒ 1156 data_dir 已配置臂 ⇒ 1165-1176 conf 现场生成 ⇒ 1189-1194
    /// 成功打印 ⇒ 1196-1209 持久化（含 1202-1204 空 path 回填臂）⇒ 1211。
    #[test]
    fn r10_cli_update_success_chain_resolves_via_path_and_persists_ready() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = r10_stage_home(tmp.path());

        let stub_dir = tmp.path().join("stubbin");
        std::fs::create_dir_all(&stub_dir).unwrap();
        std::fs::write(stub_dir.join("clamd.exe"), b"MZ").unwrap();
        r10_copy_cmd_as(&stub_dir, "freshclam.exe");
        let data_dir = tmp.path().join("configured_datadir"); // 1156 配置臂

        s11b_write_cfg(
            &cfg,
            &[],
            serde_json::json!({"clamav": s11b_engine_json(
                "", "", "", data_dir.to_str().unwrap(), "installed", "")}),
        );
        assert!(!stub_dir.join("freshclam.conf").exists(),
            "前置：conf 缺席，逼迫 update 现场生成（1165-1176）");

        let o = r10_spawn(
            &r10_bin(),
            &["scanner", "clamav", "update"],
            &[
                ("NEMESISBOT_HOME", tmp.path().to_str().unwrap()),
                (
                    "PATH",
                    &r10_synthetic_path(Some(&stub_dir)),
                ),
            ],
            180,
        )
        .1;
        assert_eq!(
            o.code, 0,
            "update 全链必须 rc=0:\n{}\n--- stderr ---\n{}",
            o.stdout, o.stderr
        );
        assert!(o.stdout.contains("Virus database updated."), "stdout:\n{}", o.stdout);

        let after = r10_read_cfg(&cfg);
        assert_eq!(
            after["engines"]["clamav"]["clamav_path"],
            stub_dir.to_string_lossy().as_ref(),
            "空 path 必须被 PATH 发现结果回填（1202-1204）"
        );
        assert_eq!(after["engines"]["clamav"]["state"]["db_status"], "ready");
        assert!(stub_dir.join("freshclam.conf").exists(), "conf 应生成到发现目录");
    }

    // ------------------- ⭐ mock clamd 扫描流程三态 -----------------------

    /// mock clamd 服务体：**不在本进程运行** —— 由各扫描用例把【当前测试二
    /// 进制】复制为 <X>\clamd.exe 后以 libtest filter 起独立进程来承载。
    /// 这是同时满足 ownership 校验（QueryFullProcessImageNameW 得到的镜像路
    /// 径必须等于 `<clamav_path>\clamd.exe` 字面值）与自定义 PONG/SCAN 协议
    /// 的唯一组合。协议按 crates/nemesis-security/src/clamav/client.rs 实证：
    /// 单行请求（如 nPING\n），首非空行响应即可。
    ///
    /// 测试名故意带独特 token `r10zz`：libtest 过滤参数就用这个子串精确选
    /// 中本用例（其他用例无一含该片段）。
    #[test]
    fn r10zz_mock_clamd_service() {
        let addr = match std::env::var("R10_MOCK_ADDR") {
            Ok(a) => a,
            Err(_) => return, // 非受控执行：静默通过，不绑端口
        };
        let mode = std::env::var("R10_MOCK_MODE").unwrap_or_else(|_| "clean".into());
        let expect_infected = mode == "infected";

        let listener = TcpListener::bind(&addr).expect("mock clamd bind");
        // 有界 serve；正常由 SHUTDOWN 优雅退出（profraw 干净落盘）。
        for stream in listener.incoming().take(64) {
            let mut stream = match stream {
                Ok(s) => s,
                Err(_) => break,
            };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                continue; // 就绪探针半开连接等场景：换下一连接
            }
            let upper = line.to_ascii_uppercase();
            if upper.contains("PING") {
                let _ = stream.write_all(b"PONG\n");
            } else if upper.contains("SHUTDOWN") {
                let _ = stream.write_all(b"BYE\n");
                let _ = stream.flush();
                break; // 优雅停机
            } else if upper.contains("SCAN") {
                // 不假设协议前缀字节格式：取首个空格之后的部分当路径回显。
                let requested = match line.find(' ') {
                    Some(i) => line[i + 1..].trim_end(),
                    None => "?",
                };
                let reply = if expect_infected {
                    format!("{}: WIN.Test.EicarProbe FOUND\n", requested)
                } else {
                    format!("{}: OK\n", requested)
                };
                let _ = stream.write_all(reply.as_bytes());
            } else {
                let _ = stream.write_all(b"UNKNOWN COMMAND\n");
            }
            let _ = stream.flush();
            drop(stream); // 每连接关闭 ⇒ client 的 read_line 拿到 EOF
        }
    }

    /// 起复制出来的 mock 镜像（libtest 只选中 r10zz 服务用例）。不等待其退
    /// 出 —— 它要一直监听到驱动发 SHUTDOWN；守卫 Drop 兜底清理。就绪探测：
    /// mock 进程需几百毫秒起步，TCP 连上即算就绪；提前夭折则响亮失败并回
    /// 带 stderr。
    fn r10_spawn_mock(image: &Path, addr: &str, mode: &str) -> R10Child {
        let child = r10_spawn_raw(
            image,
            &[
                "r10zz", // libtest 过滤 token（唯一）
                "--test-threads=1",
                "--nocapture",
            ],
            &[
                ("R10_MOCK_ADDR", addr),
                ("R10_MOCK_MODE", mode),
            ],
        )
        .expect("mock clamd 镜像进程必须能起");
        let mut guard = R10Child { child: Some(child) };

        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match TcpStream::connect(addr) {
                Ok(s) => {
                    drop(s);
                    return guard;
                }
                Err(_) if Instant::now() < deadline => {
                    // 早夭检查：端口未起但进程已退 = 配置/过滤错误
                    let exited = guard
                        .child
                        .as_mut()
                        .and_then(|c| c.try_wait().ok())
                        .flatten()
                        .is_some();
                    if exited {
                        let mut err = String::new();
                        if let Some(mut s) = guard.child.as_mut().unwrap().stderr.take()
                        {
                            let _ = s.read_to_string(&mut err);
                        }
                        panic!("mock clamd 提前退出，stderr:\n{}", err);
                    }
                    std::thread::sleep(Duration::from_millis(150));
                }
                Err(e) => panic!("mock clamd 端口 {addr} 30s 未就绪: {e}"),
            }
        }
    }

    /// 共同夹具：镜像复制 + 服务启动 + 引擎配置写入；返回 (地址, 采样文件,
    /// 服务守卫)。
    fn r10_setup_mock_scan(mode: &str, tmp: &Path) -> (String, PathBuf, R10Child) {
        let mock_dir = tmp.join("mockbin");
        std::fs::create_dir_all(&mock_dir).unwrap();
        let image = r10_copy_self_image(&mock_dir, "clamd.exe");

        let addr = r10_reserve_loopback_address();
        let svc = r10_spawn_mock(&image, &addr, mode);

        let home_cfg = r10_stage_home(tmp);
        s11b_write_cfg(
            &home_cfg,
            &[],
            serde_json::json!({"clamav": s11b_engine_json(
                "", mock_dir.to_str().unwrap(), &addr, "", "installed", "")}),
        );

        let sample = tmp.join("sample.vx9"); // 非 SAFE 白名单扩展
        std::fs::write(&sample, b"X5O!P%@AP probe content").unwrap();
        (addr, sample, svc)
    }

    /// 把【当前测试二进制】复制为指定名字（ownership 的路径匹配前提）。
    fn r10_copy_self_image(dest_dir: &Path, name: &str) -> PathBuf {
        let src = std::env::current_exe().expect("current_exe");
        let dst = dest_dir.join(name);
        std::fs::copy(&src, &dst)
            .unwrap_or_else(|e| panic!("复制自镜像为 {} 失败: {}", name, e));
        dst
    }

    /// 保留一个临时环回端口供 mock 使用（bind→取号→释放端口再交还 mock 重
    /// bind 的窗口极小，且 mock 起不来会在就绪探测处响亮失败）。
    fn r10_reserve_loopback_address() -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().to_string()
    }

    /// 1288-1289 / 1291 / 1293-1298（INFECTED 打印臂）：全链
    /// start-reuse(PING) → ownership 命中镜像 → is_ready(PING) → SCAN 裁决
    /// FOUND → SHUTDOWN → 结果区 INFECTED + Virus 行，rc=0。
    #[test]
    fn r10_cli_test_scan_reports_infected_via_owned_mock_daemon() {
        let tmp = tempfile::tempdir().unwrap();
        let (_addr, sample, svc) = r10_setup_mock_scan("infected", tmp.path());

        let o = r10_spawn(
            &r10_bin(),
            &[
                "scanner",
                "clamav",
                "test",
                sample.to_str().unwrap(),
            ],
            &[("NEMESISBOT_HOME", tmp.path().to_str().unwrap())],
            180,
        )
        .1;
        assert_eq!(
            o.code, 0,
            "infected 流程应正常结束(rc=0)，拦截语义在 stdout:\n{}\n--- stderr ---\n{}",
            o.stdout, o.stderr
        );
        assert!(o.stdout.contains("Scanning:"), "stdout:\n{}", o.stdout);
        assert!(o.stdout.contains("INFECTED"), "stdout:\n{}", o.stdout);
        assert!(o.stdout.contains("EicarProbe"), "病毒名必须透传:\n{}", o.stdout);
        assert!(svc.child.is_some(), "服务守卫存活至断言结束");
    }

    /// 1293-1301 的 CLEAN 侧互斥臂：同样全链，裁决 OK ⇒ Status CLEAN。
    #[test]
    fn r10_cli_test_scan_reports_clean_via_owned_mock_daemon() {
        let tmp = tempfile::tempdir().unwrap();
        let (_addr, sample, svc) = r10_setup_mock_scan("clean", tmp.path());

        let o = r10_spawn(
            &r10_bin(),
            &["scanner", "clamav", "test", sample.to_str().unwrap()],
            &[("NEMESISBOT_HOME", tmp.path().to_str().unwrap())],
            180,
        )
        .1;
        assert_eq!(o.code, 0, "clean 流程 rc=0:\n{}\n{}", o.stdout, o.stderr);
        assert!(o.stdout.contains("CLEAN"), "stdout:\n{}", o.stdout);
        assert!(!o.stdout.contains("INFECTED"), "CLEAN 分支不得出现 INFECTED:\n{}", o.stdout);
        assert!(svc.child.is_some());
    }

    /// 1259-1263 直调入口（已配置但 daemon 不可达）+ 1274-1277 start 失败
    /// WARN + 1280 短眠 + 1282-1286 is_ready=false → exit(1)：地址用
    /// 127.0.0.1:1（特权保留口，连接必然瞬时拒绝）、clamav_path 留空跳过
    /// Manager —— 全程离线秒级完成。
    #[test]
    fn r10_cli_test_unreachable_daemon_warns_then_exits_1() {
        let tmp = tempfile::tempdir().unwrap();
        let home_cfg = r10_stage_home(tmp.path());
        let sample = tmp.path().join("sample.vx9");
        std::fs::write(&sample, b"x").unwrap();

        s11b_write_cfg(
            &home_cfg,
            &[],
            serde_json::json!({"clamav": s11b_engine_json(
                "", "", "127.0.0.1:1", "", "installed", "")}),
        );

        let o = r10_spawn(
            &r10_bin(),
            &["scanner", "clamav", "test", sample.to_str().unwrap()],
            &[("NEMESISBOT_HOME", tmp.path().to_str().unwrap())],
            120,
        )
        .1;
        assert_eq!(
            o.code, 1,
            "daemon 不可达必须 exit(1):\n--- stdout ---\n{}\n--- stderr ---\n{}",
            o.stdout, o.stderr
        );
        assert!(
            o.stderr.contains("Failed to start engine"),
            "start 失败 WARN 臂:\n{}",
            o.stderr
        );
        assert!(
            o.stdout.contains("Attempting scan anyway"),
            "降级继续打印臂:\n{}",
            o.stdout
        );
        assert!(
            o.stderr.contains("not ready"),
            "is_ready=false 提示臂:\n{}",
            o.stderr
        );
    }

    // --------------------- R10 终测补测：config-guard 退码臂 ----------------
    // scanner.rs 三处「配置缺引擎/未知引擎 → eprintln + std::process::exit(1)」
    // 是纯 CLI 守卫，不需要真引擎/真网络。当前 merged lcov 显示 miss：
    //   cmd_add 382-387（Unknown engine + Available 列表）
    //   cmd_remove 440-441（not found in configuration）
    //   cmd_enable 457-461（Add it first with 'scanner add <name>'）
    // home 全隔离（r10_stage_home），配置文件不存在 → load_scanner_config
    // 走默认空 engines map ⇒ contains_key 恒 false，三臂确定性可达。

    /// cmd_add 未知引擎名 → 打印 Unknown engine + Available 清单后 exit(1)。
    #[test]
    fn r10_cmd_add_unknown_engine_lists_available_and_exits_1() {
        let tmp = tempfile::tempdir().unwrap();
        r10_stage_home(tmp.path()); // 返回值是 engine 配置路径清单，此臂用不上
        let (_guard, o) = r10_spawn(
            &r10_bin(),
            &["scanner", "add", "boguseng", "--url", "http://127.0.0.1:9/x"],
            &[("NEMESISBOT_HOME", tmp.path().to_string_lossy().as_ref())],
            30,
        );
        assert_eq!(o.code, 1, "未知引擎必须 exit(1):\n--- stdout ---\n{}\n--- stderr ---\n{}", o.stdout, o.stderr);
        assert!(o.stderr.contains("Unknown engine: boguseng"), "got:\n{}", o.stderr);
        assert!(o.stderr.contains("Available"), "要列出可用引擎:\n{}", o.stderr);
    }

    /// cmd_remove 配置里没有该引擎 → not found in configuration + exit(1)。
    #[test]
    fn r10_cmd_remove_missing_engine_exits_1() {
        let tmp = tempfile::tempdir().unwrap();
        let _cfg = r10_stage_home(tmp.path());
        let (_guard, o) = r10_spawn(
            &r10_bin(),
            &["scanner", "remove", "nosuch"],
            &[("NEMESISBOT_HOME", tmp.path().to_string_lossy().as_ref())],
            30,
        );
        assert_eq!(o.code, 1, "remove 缺引擎必须 exit(1):\nstdout={}\nstderr={}", o.stdout, o.stderr);
        assert!(
            o.stderr.contains("Engine 'nosuch' not found in configuration."),
            "got:\n{}",
            o.stderr
        );
    }

    /// cmd_enable 配置里没有该引擎 → Add it first 提示 + exit(1)。
    #[test]
    fn r10_cmd_enable_missing_engine_exits_1() {
        let tmp = tempfile::tempdir().unwrap();
        let _cfg = r10_stage_home(tmp.path());
        let (_guard, o) = r10_spawn(
            &r10_bin(),
            &["scanner", "enable", "nosuch"],
            &[("NEMESISBOT_HOME", tmp.path().to_string_lossy().as_ref())],
            30,
        );
        assert_eq!(o.code, 1, "enable 缺引擎必须 exit(1):\nstdout={}\nstderr={}", o.stdout, o.stderr);
        assert!(
            o.stderr.contains("Add it first with 'scanner add nosuch'"),
            "got:\n{}",
            o.stderr
        );
    }
}

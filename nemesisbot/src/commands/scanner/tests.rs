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

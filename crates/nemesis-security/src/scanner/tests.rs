use super::*;
use tokio::io::AsyncBufReadExt; // read_line 所在 trait（E0599）

#[tokio::test]
async fn test_stub_scan_file_clean() {
    let scanner = StubScanner;
    assert_eq!(scanner.name(), "stub");
    assert!(scanner.is_ready().await);
    let result = scanner.scan_file(Path::new("/tmp/any.txt")).await;
    assert!(!result.infected);
    assert!(result.virus.is_empty());
}

#[tokio::test]
async fn test_stub_scan_content_clean() {
    let scanner = StubScanner;
    let result = scanner.scan_content(b"EICAR-test-string").await;
    assert!(!result.infected);
}

#[tokio::test]
async fn test_stub_scan_directory_clean() {
    let scanner = StubScanner;
    let results = scanner.scan_directory(Path::new("/tmp")).await;
    assert!(results.is_empty());
}

#[tokio::test]
async fn test_stub_get_info() {
    let scanner = StubScanner;
    let info = scanner.get_info().await;
    assert_eq!(info.name, "stub");
    assert!(info.ready);
}

#[tokio::test]
async fn test_stub_start_stop() {
    let scanner = StubScanner;
    assert!(scanner.start().await.is_ok());
    assert!(scanner.stop().await.is_ok());
}

#[tokio::test]
async fn test_stub_database_status() {
    let scanner = StubScanner;
    let status = scanner.get_database_status().await;
    assert!(!status.available);
}

#[tokio::test]
async fn test_stub_update_database() {
    let scanner = StubScanner;
    assert!(scanner.update_database().await.is_ok());
}

#[tokio::test]
async fn test_stub_get_stats() {
    let scanner = StubScanner;
    let stats = scanner.get_stats();
    assert!(stats.contains_key("ready"));
}

#[tokio::test]
async fn test_scan_engine_build() {
    let engine = ScanEngine::default();
    assert_eq!(engine, ScanEngine::Stub);

    let scanner = engine.build();
    let result = scanner.scan_content(b"hello").await;
    assert!(!result.infected);

    // ClamAV variant currently also returns stub.
    let clamav = ScanEngine::ClamAV.build();
    let result = clamav.scan_content(b"hello").await;
    assert!(!result.infected);
}

#[test]
fn test_extension_rules_whitelist() {
    let rules = ExtensionRules::new(vec!["exe".to_string(), "dll".to_string()], vec![]);
    assert!(rules.should_scan_file(Path::new("program.exe")));
    assert!(rules.should_scan_file(Path::new("lib.dll")));
    assert!(!rules.should_scan_file(Path::new("test.txt")));
}

#[test]
fn test_extension_rules_blacklist() {
    let rules = ExtensionRules::new(vec![], vec!["txt".to_string(), "md".to_string()]);
    assert!(!rules.should_scan_file(Path::new("test.txt")));
    assert!(!rules.should_scan_file(Path::new("README.md")));
    assert!(rules.should_scan_file(Path::new("program.exe")));
}

#[test]
fn test_extension_rules_both_empty() {
    let rules = ExtensionRules::default();
    // When both are empty, scan everything.
    assert!(rules.should_scan_file(Path::new("anything.xyz")));
}

#[tokio::test]
async fn test_scan_chain_empty() {
    let chain = ScanChain::with_defaults();
    let result = chain.scan_file(Path::new("/tmp/test.txt")).await;
    assert!(result.clean);
}

#[test]
fn test_scan_chain_enabled() {
    let chain = ScanChain::with_defaults();
    assert!(!chain.is_enabled());
    chain.set_enabled(true);
    assert!(chain.is_enabled());
}

#[test]
fn test_scan_chain_add_engine() {
    let mut chain = ScanChain::with_defaults();
    assert_eq!(chain.engine_count(), 0);
    chain.add_engine(Box::new(StubScanner));
    assert_eq!(chain.engine_count(), 1);
}

#[test]
fn test_scan_chain_engines_list() {
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(StubScanner));
    let engines = chain.engines();
    assert_eq!(engines.len(), 1);
    assert_eq!(engines[0].name(), "stub");
}

#[tokio::test]
async fn test_scan_chain_start_stop() {
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(StubScanner));
    chain.start().await;
    chain.stop().await;
}

#[test]
fn test_scan_chain_raw_config() {
    let mut chain = ScanChain::with_defaults();
    let mut full_config = ScannerFullConfig::default();
    full_config.enabled.push("stub".to_string());
    full_config
        .engines
        .insert("stub".to_string(), serde_json::json!({"key": "value"}));
    chain.load_from_full_config(&full_config);

    let raw = chain.raw_config("stub");
    assert!(raw.is_some());
    assert_eq!(raw.unwrap()["key"], "value");

    assert!(chain.raw_config("nonexistent").is_none());
}

#[tokio::test]
async fn test_scan_chain_scan_content() {
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(StubScanner));
    let result = chain.scan_content(b"hello world").await;
    assert!(result.clean);
}

#[tokio::test]
async fn test_scan_chain_scan_directory() {
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(StubScanner));
    let result = chain.scan_directory(Path::new("/tmp")).await;
    assert!(result.clean);
}

#[tokio::test]
async fn test_scan_chain_get_stats() {
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(StubScanner));
    let stats = chain.get_stats();
    assert!(stats.contains_key("stub"));
}

#[test]
fn test_create_engine() {
    let engine = create_engine("stub", &serde_json::Value::Null).unwrap();
    assert_eq!(engine.name(), "stub");

    let engine = create_engine("clamav", &serde_json::Value::Null).unwrap();
    assert_eq!(engine.name(), "clamav");

    assert!(create_engine("unknown", &serde_json::Value::Null).is_err());
}

#[test]
fn test_available_engines() {
    let engines = available_engines();
    assert!(engines.contains(&"clamav"));
    assert!(engines.contains(&"stub"));
}

#[test]
fn test_scan_result_merge() {
    let mut r1 = ScanResult::clean_from("stub");
    let r2 = ScanResult::with_threats("clamav", "EICAR", "/tmp/test.exe");
    r1.merge(&r2);
    assert!(r1.infected);
    assert_eq!(r1.virus, "EICAR");
}

#[test]
fn test_scan_chain_result_blocked() {
    let result = ScanChainResult::blocked(
        "clamav",
        "EICAR",
        "/tmp/test.exe",
        vec![ScanResult::with_threats("clamav", "EICAR", "/tmp/test.exe")],
    );
    assert!(!result.clean);
    assert!(result.blocked);
    assert_eq!(result.engine, "clamav");
    assert_eq!(result.virus, "EICAR");
}

#[test]
fn test_extract_paths_from_args() {
    let chain = ScanChain::with_defaults();
    let args = serde_json::json!({"path": "/tmp/test.txt", "content": "hello"});
    let paths = chain.extract_paths_from_args("write_file", &args);
    assert_eq!(paths, vec!["/tmp/test.txt"]);

    let args2 = serde_json::json!({"command": "ls -la /home/user/file.txt"});
    let paths2 = chain.extract_paths_from_args("exec", &args2);
    assert!(paths2.contains(&"/home/user/file.txt".to_string()));
}

#[tokio::test]
async fn test_scan_tool_invocation_clean() {
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(StubScanner));
    chain.set_enabled(true);

    let args = serde_json::json!({"path": "/tmp/test.txt", "content": "hello"});
    let (allowed, error) = chain.scan_tool_invocation("write_file", &args).await;
    assert!(allowed);
    assert!(error.is_none());
}

#[test]
fn test_engine_info_serialization() {
    let info = EngineInfo {
        name: "clamav".to_string(),
        version: "0.103.0".to_string(),
        address: "127.0.0.1:3310".to_string(),
        ready: true,
        start_time: "2026-01-01T00:00:00Z".to_string(),
    };
    let json = serde_json::to_string(&info).unwrap();
    assert!(json.contains("clamav"));
}

#[test]
fn test_database_status_default() {
    let status = DatabaseStatus::default();
    assert!(!status.available);
    assert!(status.version.is_empty());
}

#[test]
fn test_scanner_full_config() {
    let mut config = ScannerFullConfig::default();
    config.enabled.push("clamav".to_string());
    config.engines.insert(
        "clamav".to_string(),
        serde_json::json!({"address": "127.0.0.1:3310"}),
    );

    let mut chain = ScanChain::with_defaults();
    chain.load_from_full_config(&config);
    assert_eq!(chain.engine_count(), 1);
}

#[test]
fn test_load_from_configs() {
    let mut chain = ScanChain::with_defaults();
    let configs = vec![
        ScannerEngineConfig {
            name: "clamav".to_string(),
            engine_type: "clamav".to_string(),
            install_status: "installed".to_string(),
        },
        ScannerEngineConfig {
            name: "yara".to_string(),
            engine_type: "yara".to_string(),
            install_status: "pending".to_string(),
        },
    ];
    chain.load_from_configs(&configs);
    assert_eq!(chain.engine_count(), 1);
}

// ---- Additional scanner tests ----

#[test]
fn test_engine_state_default() {
    let state = EngineState::default();
    assert!(state.install_status.is_empty());
    assert!(state.install_error.is_empty());
    assert!(state.db_status.is_empty());
}

#[test]
fn test_engine_state_serialization() {
    let state = EngineState {
        install_status: "installed".to_string(),
        db_status: "ready".to_string(),
        install_error: String::new(),
        last_install_attempt: "2026-01-01T00:00:00Z".to_string(),
        last_db_update: "2026-01-01T00:00:00Z".to_string(),
    };
    let json = serde_json::to_string(&state).unwrap();
    let de: EngineState = serde_json::from_str(&json).unwrap();
    assert_eq!(de.install_status, "installed");
    assert_eq!(de.db_status, "ready");
}

#[test]
fn test_scan_result_clean_from() {
    let result = ScanResult::clean_from("test_engine");
    assert!(!result.infected);
    assert!(result.virus.is_empty());
    assert_eq!(result.engine, "test_engine");
}

#[test]
fn test_scan_result_with_threats() {
    let result = ScanResult::with_threats("clamav", "Trojan.Generic", "/tmp/evil.exe");
    assert!(result.infected);
    assert_eq!(result.virus, "Trojan.Generic");
    assert_eq!(result.path, "/tmp/evil.exe");
    assert_eq!(result.engine, "clamav");
}

#[test]
fn test_scan_result_merge_clean_into_infected() {
    let mut r1 = ScanResult::with_threats("engine1", "Virus1", "/tmp/a");
    let r2 = ScanResult::clean_from("engine2");
    r1.merge(&r2);
    assert!(r1.infected);
    assert_eq!(r1.virus, "Virus1");
}

#[test]
fn test_scan_result_merge_infected_into_clean() {
    let mut r1 = ScanResult::clean_from("engine1");
    let r2 = ScanResult::with_threats("engine2", "Virus2", "/tmp/b");
    r1.merge(&r2);
    assert!(r1.infected);
    assert_eq!(r1.virus, "Virus2");
}

#[test]
fn test_scan_result_merge_two_infected() {
    let mut r1 = ScanResult::with_threats("engine1", "Virus1", "/tmp/a");
    let r2 = ScanResult::with_threats("engine2", "Virus2", "/tmp/b");
    r1.merge(&r2);
    assert!(r1.infected);
    // First virus should be kept
    assert_eq!(r1.virus, "Virus1");
}

#[test]
fn test_scan_chain_result_clean() {
    let result = ScanChainResult::clean();
    assert!(result.clean);
    assert!(!result.blocked);
    assert!(result.engine.is_empty());
    assert!(result.virus.is_empty());
    assert!(result.results.is_empty());
}

#[test]
fn test_scan_chain_result_blocked_fields() {
    let result = ScanChainResult::blocked(
        "clamav",
        "EICAR-Test",
        "/tmp/eicar.com",
        vec![
            ScanResult::clean_from("stub"),
            ScanResult::with_threats("clamav", "EICAR-Test", "/tmp/eicar.com"),
        ],
    );
    assert!(!result.clean);
    assert!(result.blocked);
    assert_eq!(result.engine, "clamav");
    assert_eq!(result.virus, "EICAR-Test");
    assert_eq!(result.results.len(), 2);
}

#[test]
fn test_extension_rules_case_insensitive() {
    let rules = ExtensionRules::new(vec!["EXE".to_string(), "DLL".to_string()], vec![]);
    assert!(rules.should_scan_file(Path::new("program.exe")));
    assert!(rules.should_scan_file(Path::new("PROGRAM.EXE")));
    assert!(rules.should_scan_file(Path::new("lib.Dll")));
}

#[test]
fn test_extension_rules_skip_case_insensitive() {
    let rules = ExtensionRules::new(vec![], vec!["TXT".to_string(), "MD".to_string()]);
    assert!(!rules.should_scan_file(Path::new("readme.txt")));
    assert!(!rules.should_scan_file(Path::new("README.MD")));
}

#[test]
fn test_extension_rules_no_extension() {
    let rules = ExtensionRules::new(vec!["exe".to_string()], vec![]);
    assert!(!rules.should_scan_file(Path::new("Makefile")));
    assert!(!rules.should_scan_file(Path::new("noext")));
}

#[test]
fn test_extension_rules_skip_no_extension() {
    let rules = ExtensionRules::new(vec![], vec!["txt".to_string()]);
    // File without extension should pass (not in skip list)
    assert!(rules.should_scan_file(Path::new("Makefile")));
}

#[test]
fn test_extension_rules_hidden_file() {
    let rules = ExtensionRules::new(vec!["exe".to_string()], vec![]);
    assert!(!rules.should_scan_file(Path::new(".hidden")));
}

#[test]
fn test_extension_rules_path_with_dirs() {
    let rules = ExtensionRules::new(vec!["exe".to_string()], vec![]);
    assert!(rules.should_scan_file(Path::new("/some/deep/path/program.exe")));
    assert!(!rules.should_scan_file(Path::new("/some/deep/path/document.txt")));
}

#[test]
fn test_scan_chain_config_default() {
    let config = ScanChainConfig::default();
    assert!(!config.enabled);
    assert_eq!(config.max_file_size, 50 * 1024 * 1024);
}

#[test]
fn test_scan_chain_add_multiple_engines() {
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(StubScanner));
    chain.add_engine(Box::new(StubScanner));
    chain.add_engine(Box::new(StubScanner));
    assert_eq!(chain.engine_count(), 3);
}

#[test]
fn test_scan_chain_get_engines_names() {
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(StubScanner));
    chain.add_engine(Box::new(StubScanner));
    let engines = chain.engines();
    assert_eq!(engines.len(), 2);
    assert_eq!(engines[0].name(), "stub");
    assert_eq!(engines[1].name(), "stub");
}

#[tokio::test]
async fn test_scan_chain_scan_file_with_extension_filter() {
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(StubScanner));

    // Test with no extension rules - should scan everything
    let result = chain.scan_file(Path::new("/tmp/test.txt")).await;
    assert!(result.clean);
}

#[test]
fn test_scan_chain_extension_rules_default() {
    let chain = ScanChain::with_defaults();
    let rules = chain.extension_rules();
    assert!(rules.scan_extensions.is_empty());
    assert!(rules.skip_extensions.is_empty());
}

#[test]
fn test_create_engine_with_config() {
    let config = serde_json::json!({
        "address": "127.0.0.1:3310",
        "enabled": true,
        "timeout_secs": 30
    });
    let engine = create_engine("clamav", &config).unwrap();
    assert_eq!(engine.name(), "clamav");
}

#[tokio::test]
async fn test_create_engine_stub_with_null() {
    let engine = create_engine("stub", &serde_json::Value::Null).unwrap();
    assert_eq!(engine.name(), "stub");
    assert!(engine.is_ready().await);
}

#[test]
fn test_extract_paths_from_args_download() {
    let chain = ScanChain::with_defaults();
    let args =
        serde_json::json!({"save_path": "/tmp/download.zip", "url": "https://example.com/file"});
    let paths = chain.extract_paths_from_args("download", &args);
    assert!(paths.contains(&"/tmp/download.zip".to_string()));
}

#[test]
fn test_extract_paths_from_args_exec() {
    let chain = ScanChain::with_defaults();
    let args = serde_json::json!({"command": "python /home/user/script.py --input data.txt"});
    let paths = chain.extract_paths_from_args("exec", &args);
    assert!(paths.contains(&"/home/user/script.py".to_string()));
    assert!(paths.contains(&"data.txt".to_string()));
}

#[test]
fn test_extract_paths_from_args_unknown_tool() {
    let chain = ScanChain::with_defaults();
    let args = serde_json::json!({"path": "/tmp/test.txt"});
    let paths = chain.extract_paths_from_args("unknown_tool", &args);
    assert!(paths.is_empty());
}

#[test]
fn test_extract_paths_from_args_empty_args() {
    let chain = ScanChain::with_defaults();
    let args = serde_json::json!({});
    let paths = chain.extract_paths_from_args("write_file", &args);
    assert!(paths.is_empty());
}

#[tokio::test]
async fn test_scan_tool_invocation_disabled() {
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(StubScanner));
    // Not enabled - should allow everything
    let args = serde_json::json!({"path": "/tmp/test.exe", "content": "malicious"});
    let (allowed, error) = chain.scan_tool_invocation("write_file", &args).await;
    assert!(allowed);
    assert!(error.is_none());
}

#[tokio::test]
async fn test_scan_tool_invocation_download_clean() {
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(StubScanner));
    chain.set_enabled(true);
    let args = serde_json::json!({"save_path": "/tmp/file.zip"});
    let (allowed, error) = chain.scan_tool_invocation("download", &args).await;
    assert!(allowed);
    assert!(error.is_none());
}

#[tokio::test]
async fn test_scan_tool_invocation_exec_clean() {
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(StubScanner));
    chain.set_enabled(true);
    let args = serde_json::json!({"command": "ls -la"});
    let (allowed, error) = chain.scan_tool_invocation("exec", &args).await;
    assert!(allowed);
    assert!(error.is_none());
}

#[tokio::test]
async fn test_scan_tool_invocation_empty_content() {
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(StubScanner));
    chain.set_enabled(true);
    let args = serde_json::json!({"path": "/tmp/test.txt", "content": ""});
    let (allowed, error) = chain.scan_tool_invocation("write_file", &args).await;
    assert!(allowed);
    assert!(error.is_none());
}

#[tokio::test]
async fn test_scan_tool_invocation_no_content_field() {
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(StubScanner));
    chain.set_enabled(true);
    let args = serde_json::json!({"path": "/tmp/test.txt"});
    let (allowed, error) = chain.scan_tool_invocation("write_file", &args).await;
    assert!(allowed);
    assert!(error.is_none());
}

#[tokio::test]
async fn test_scan_tool_invocation_unknown_tool() {
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(StubScanner));
    chain.set_enabled(true);
    let args = serde_json::json!({"path": "/tmp/test.txt", "content": "data"});
    let (allowed, error) = chain.scan_tool_invocation("read_file", &args).await;
    assert!(allowed);
    assert!(error.is_none());
}

#[test]
fn test_scanner_engine_config_fields() {
    let config = ScannerEngineConfig {
        name: "test-engine".to_string(),
        engine_type: "stub".to_string(),
        install_status: "pending".to_string(),
    };
    assert_eq!(config.name, "test-engine");
    assert_eq!(config.engine_type, "stub");
    assert_eq!(config.install_status, "pending");
}

#[test]
fn test_scanner_full_config_default() {
    let config = ScannerFullConfig::default();
    assert!(config.enabled.is_empty());
    assert!(config.engines.is_empty());
}

#[test]
fn test_load_from_configs_all_installed() {
    let mut chain = ScanChain::with_defaults();
    let configs = vec![
        ScannerEngineConfig {
            name: "engine1".to_string(),
            engine_type: "stub".to_string(),
            install_status: "installed".to_string(),
        },
        ScannerEngineConfig {
            name: "engine2".to_string(),
            engine_type: "stub".to_string(),
            install_status: "installed".to_string(),
        },
    ];
    chain.load_from_configs(&configs);
    assert_eq!(chain.engine_count(), 2);
}

#[test]
fn test_load_from_configs_all_pending() {
    let mut chain = ScanChain::with_defaults();
    let configs = vec![
        ScannerEngineConfig {
            name: "engine1".to_string(),
            engine_type: "stub".to_string(),
            install_status: "pending".to_string(),
        },
        ScannerEngineConfig {
            name: "engine2".to_string(),
            engine_type: "stub".to_string(),
            install_status: "failed".to_string(),
        },
    ];
    chain.load_from_configs(&configs);
    assert_eq!(chain.engine_count(), 0);
}

#[test]
fn test_shared_scan_chain_creation() {
    let chain = shared_scan_chain();
    let chain_guard = chain.try_read().unwrap();
    assert_eq!(chain_guard.engine_count(), 0);
}

#[test]
fn test_database_status_serialization() {
    let status = DatabaseStatus {
        available: true,
        version: "0.103.0".to_string(),
        last_update: "2026-01-01".to_string(),
        path: "/var/lib/clamav".to_string(),
        size_bytes: 1024,
    };
    let json = serde_json::to_string(&status).unwrap();
    let de: DatabaseStatus = serde_json::from_str(&json).unwrap();
    assert!(de.available);
    assert_eq!(de.version, "0.103.0");
}

#[test]
fn test_engine_info_all_fields() {
    let info = EngineInfo {
        name: "clamav".to_string(),
        version: "0.103.0".to_string(),
        address: "127.0.0.1:3310".to_string(),
        ready: true,
        start_time: "2026-01-01T00:00:00Z".to_string(),
    };
    assert_eq!(info.name, "clamav");
    assert_eq!(info.version, "0.103.0");
    assert!(info.ready);
}

#[test]
fn test_scan_chain_get_stats_empty() {
    let chain = ScanChain::with_defaults();
    let stats = chain.get_stats();
    assert!(stats.is_empty());
}

#[test]
fn test_get_extension_rules_from_raw_config() {
    let mut chain = ScanChain::with_defaults();
    let mut full_config = ScannerFullConfig::default();
    full_config.enabled.push("stub".to_string());
    full_config.engines.insert(
        "stub".to_string(),
        serde_json::json!({
            "scan_extensions": ["exe", "dll"],
            "skip_extensions": ["txt"]
        }),
    );
    chain.load_from_full_config(&full_config);
    let rules = chain.get_extension_rules();
    assert_eq!(rules.scan_extensions.len(), 2);
    assert_eq!(rules.skip_extensions.len(), 1);
}

#[test]
fn test_get_extension_rules_no_rules_in_config() {
    let mut chain = ScanChain::with_defaults();
    let mut full_config = ScannerFullConfig::default();
    full_config.enabled.push("stub".to_string());
    full_config
        .engines
        .insert("stub".to_string(), serde_json::json!({"key": "value"}));
    chain.load_from_full_config(&full_config);
    let rules = chain.get_extension_rules();
    assert!(rules.scan_extensions.is_empty());
    assert!(rules.skip_extensions.is_empty());
}

#[test]
fn test_load_from_full_config_missing_engine_config() {
    let mut chain = ScanChain::with_defaults();
    let mut full_config = ScannerFullConfig::default();
    full_config.enabled.push("nonexistent_engine".to_string());
    // No config for this engine - should be skipped
    chain.load_from_full_config(&full_config);
    assert_eq!(chain.engine_count(), 0);
}

#[test]
fn test_load_from_full_config_not_installed_status() {
    let mut chain = ScanChain::with_defaults();
    let mut full_config = ScannerFullConfig::default();
    full_config.enabled.push("stub".to_string());
    full_config.engines.insert(
        "stub".to_string(),
        serde_json::json!({"state": {"install_status": "pending"}}),
    );
    chain.load_from_full_config(&full_config);
    assert_eq!(chain.engine_count(), 0);
}

#[tokio::test]
async fn test_scan_chain_scan_directory_empty_dir() {
    let dir = tempfile::tempdir().unwrap();
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(StubScanner));
    let result = chain.scan_directory(dir.path()).await;
    assert!(result.clean);
}

#[tokio::test]
async fn test_scan_chain_scan_file_with_temp_file() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "hello world").unwrap();

    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(StubScanner));
    let result = chain.scan_file(&file_path).await;
    assert!(result.clean);
}

// ---- format_bytes tests ----

#[test]
fn test_format_bytes_kb() {
    assert_eq!(format_bytes(512), "0 KB");
    assert_eq!(format_bytes(1024), "1 KB");
    assert_eq!(format_bytes(1024 * 100), "100 KB");
}

#[test]
fn test_format_bytes_mb() {
    let one_mb = 1024 * 1024;
    assert_eq!(format_bytes(one_mb), "1.0 MB");
    // 44,561,817 bytes = 42.5 MB (42.5 * 1024 * 1024)
    assert_eq!(format_bytes(44_561_817), "42.5 MB");
    assert_eq!(format_bytes(one_mb * 100), "100.0 MB");
}

#[test]
fn test_format_bytes_zero() {
    assert_eq!(format_bytes(0), "0 KB");
}

// ---- Coverage expansion tests for scanner ----

#[test]
fn test_scan_engine_build_with_address_stub() {
    let scanner = ScanEngine::Stub.build_with_address("127.0.0.1:3310");
    assert_eq!(scanner.name(), "stub");
}

#[test]
fn test_scan_engine_build_with_address_clamav() {
    let scanner = ScanEngine::ClamAV.build_with_address("127.0.0.1:3310");
    assert_eq!(scanner.name(), "clamav");
}

#[tokio::test]
async fn test_clamav_wrapper_scan_content_clean() {
    let scanner = ScanEngine::ClamAV.build();
    let result = scanner.scan_content(b"clean content").await;
    assert!(!result.infected);
}

#[tokio::test]
async fn test_clamav_wrapper_scan_file_clean() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "clean data").unwrap();
    let scanner = ScanEngine::ClamAV.build();
    let result = scanner.scan_file(&file_path).await;
    assert!(!result.infected);
}

#[tokio::test]
async fn test_clamav_wrapper_get_info() {
    // Closed port → deterministic "not ready" regardless of a real clamd.
    let scanner = ScanEngine::ClamAV.build_with_address("127.0.0.1:1");
    let info = scanner.get_info().await;
    assert_eq!(info.name, "clamav");
    assert!(!info.ready); // No daemon running
}

#[tokio::test]
async fn test_clamav_wrapper_start_stop() {
    let scanner = ScanEngine::ClamAV.build();
    // Start/stop without a real daemon should handle gracefully
    let _ = scanner.start().await;
    let _ = scanner.stop().await;
}

#[tokio::test]
async fn test_clamav_wrapper_database_status() {
    let scanner = ScanEngine::ClamAV.build();
    let status = scanner.get_database_status().await;
    assert!(!status.available);
}

#[tokio::test]
async fn test_clamav_wrapper_update_database() {
    let scanner = ScanEngine::ClamAV.build();
    let result = scanner.update_database().await;
    // Without a real ClamAV, this should fail gracefully
    let _ = result;
}

#[tokio::test]
async fn test_clamav_wrapper_scan_directory() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "aaa").unwrap();
    std::fs::write(dir.path().join("b.txt"), "bbb").unwrap();
    let scanner = ScanEngine::ClamAV.build();
    let results = scanner.scan_directory(dir.path()).await;
    assert!(!results.is_empty());
}

#[test]
fn test_clamav_wrapper_get_stats() {
    let scanner = ScanEngine::ClamAV.build();
    let stats = scanner.get_stats();
    // Stats may be empty when daemon is not running
    let _ = stats;
}

#[tokio::test]
async fn test_clamav_wrapper_is_ready() {
    let scanner = ScanEngine::ClamAV.build();
    assert!(!scanner.is_ready().await); // No daemon running
}

#[test]
fn test_walkdir() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("file1.txt"), "a").unwrap();
    std::fs::create_dir(dir.path().join("subdir")).unwrap();
    std::fs::write(dir.path().join("subdir/file2.txt"), "b").unwrap();
    let paths = walkdir(dir.path()).unwrap();
    assert_eq!(paths.len(), 2);
}

#[test]
fn test_walkdir_nonexistent() {
    let result = walkdir(Path::new("/nonexistent/path/abc123"));
    assert!(result.is_err());
}

#[test]
fn test_extract_zip_archive_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let zip_path = dir.path().join("test.zip");
    std::fs::write(&zip_path, b"not a zip file").unwrap();
    let result = extract_zip_archive(&zip_path, dir.path());
    assert!(result.is_err());
}

#[test]
fn test_extract_zip_archive_valid() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let zip_path = dir.path().join("test.zip");
    let dest_dir = dir.path().join("extracted");

    // Create a minimal zip file using the zip crate
    let zip_file = std::fs::File::create(&zip_path).unwrap();
    let mut zip_writer = zip::ZipWriter::new(zip_file);
    let options = zip::write::SimpleFileOptions::default();
    zip_writer.start_file("hello.txt", options).unwrap();
    zip_writer.write_all(b"hello world").unwrap();
    zip_writer.finish().unwrap();

    let result = extract_zip_archive(&zip_path, &dest_dir);
    assert!(result.is_ok());
    let extracted = std::fs::read_to_string(dest_dir.join("hello.txt")).unwrap();
    assert_eq!(extracted, "hello world");
}

#[test]
fn test_scan_chain_load_from_full_config_installed() {
    let mut chain = ScanChain::with_defaults();
    let mut full_config = ScannerFullConfig::default();
    full_config.enabled.push("stub".to_string());
    full_config.engines.insert(
        "stub".to_string(),
        serde_json::json!({
            "state": {"install_status": "installed"}
        }),
    );
    chain.load_from_full_config(&full_config);
    assert_eq!(chain.engine_count(), 1);
}

#[tokio::test]
async fn test_scan_chain_default_trait() {
    let mut chain = ScanChain::default();
    chain.add_engine(Box::new(StubScanner));
    chain.set_enabled(true);
    let result = chain.scan_content(b"test").await;
    assert!(result.clean);
}

#[tokio::test]
async fn test_stub_scan_file_with_path() {
    let scanner = StubScanner;
    let result = scanner
        .scan_file(Path::new("/some/deep/path/file.exe"))
        .await;
    assert!(!result.infected);
    assert_eq!(result.path, "/some/deep/path/file.exe");
    assert_eq!(result.engine, "stub");
}

#[test]
fn test_scan_result_clean_with_path() {
    let result = ScanResult::clean_with_path("engine1", "/tmp/test.txt");
    assert!(!result.infected);
    assert_eq!(result.path, "/tmp/test.txt");
    assert_eq!(result.engine, "engine1");
}

#[test]
fn test_install_status_constants() {
    assert_eq!(INSTALL_STATUS_PENDING, "pending");
    assert_eq!(INSTALL_STATUS_INSTALLED, "installed");
    assert_eq!(INSTALL_STATUS_FAILED, "failed");
    assert_eq!(DB_STATUS_MISSING, "missing");
    assert_eq!(DB_STATUS_READY, "ready");
    assert_eq!(DB_STATUS_STALE, "stale");
}

// ---- ClamAVEngine specific tests ----

#[tokio::test]
async fn test_clamav_engine_new() {
    let config = ClamAVEngineConfig::default();
    let engine = ClamAVEngine::new(config);
    assert_eq!(engine.name(), "clamav");
    assert!(!engine.is_ready().await);
    assert_eq!(engine.get_clamav_path(), "");
}

#[test]
fn test_clamav_engine_get_set_data_dir() {
    let config = ClamAVEngineConfig::default();
    let engine = ClamAVEngine::new(config);
    assert!(engine.get_clamav_path().is_empty());
    engine.set_data_dir("/custom/data/dir");
    // Verify it was set by getting extension rules (which reads config)
    let rules = engine.get_extension_rules();
    assert!(rules.scan_extensions.is_empty());
}

#[test]
fn test_clamav_engine_get_extension_rules() {
    let config = ClamAVEngineConfig {
        scan_extensions: vec!["exe".to_string(), "dll".to_string()],
        skip_extensions: vec!["txt".to_string()],
        ..Default::default()
    };
    let engine = ClamAVEngine::new(config);
    let rules = engine.get_extension_rules();
    assert_eq!(rules.scan_extensions.len(), 2);
    assert_eq!(rules.skip_extensions.len(), 1);
}

#[tokio::test]
async fn test_clamav_engine_start_already_started() {
    let config = ClamAVEngineConfig::default();
    let engine = ClamAVEngine::new(config);
    // We can't actually start (no daemon), but we can test double-stop
    let _ = engine.stop().await;
    let result = engine.stop().await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_clamav_engine_get_info() {
    let config = ClamAVEngineConfig {
        address: "127.0.0.1:3310".to_string(),
        ..Default::default()
    };
    let engine = ClamAVEngine::new(config);
    let info = engine.get_info().await;
    assert_eq!(info.name, "clamav");
    assert!(!info.ready);
    assert_eq!(info.address, "127.0.0.1:3310");
}

#[tokio::test]
async fn test_clamav_engine_get_stats() {
    let config = ClamAVEngineConfig::default();
    let engine = ClamAVEngine::new(config);
    let stats = engine.get_stats();
    assert!(stats.contains_key("started"));
    assert!(!stats["started"].as_bool().unwrap());
}

#[tokio::test]
async fn test_clamav_engine_scan_file_not_ready() {
    let config = ClamAVEngineConfig::default();
    let engine = ClamAVEngine::new(config);
    let result = engine.scan_file(Path::new("/tmp/test.txt")).await;
    assert!(!result.infected);
    assert_eq!(result.raw, "engine not ready");
    assert_eq!(result.engine, "clamav");
}

#[tokio::test]
async fn test_clamav_engine_scan_content_not_ready() {
    let config = ClamAVEngineConfig::default();
    let engine = ClamAVEngine::new(config);
    let result = engine.scan_content(b"hello world").await;
    assert!(!result.infected);
    assert_eq!(result.raw, "engine not ready");
    assert_eq!(result.engine, "clamav");
}

#[tokio::test]
async fn test_clamav_engine_scan_directory_empty() {
    let dir = tempfile::tempdir().unwrap();
    let config = ClamAVEngineConfig::default();
    let engine = ClamAVEngine::new(config);
    let results = engine.scan_directory(dir.path()).await;
    assert!(results.is_empty());
}

#[tokio::test]
async fn test_clamav_engine_scan_directory_with_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "aaa").unwrap();
    std::fs::write(dir.path().join("b.txt"), "bbb").unwrap();
    let config = ClamAVEngineConfig::default();
    let engine = ClamAVEngine::new(config);
    let results = engine.scan_directory(dir.path()).await;
    assert_eq!(results.len(), 2);
    // All should report "engine not ready"
    for r in &results {
        assert_eq!(r.raw, "engine not ready");
    }
}

#[tokio::test]
async fn test_clamav_engine_update_database_not_ready() {
    let config = ClamAVEngineConfig::default();
    let engine = ClamAVEngine::new(config);
    let result = engine.update_database().await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not ready"));
}

#[test]
fn test_clamav_engine_target_executables() {
    let config = ClamAVEngineConfig::default();
    let engine = ClamAVEngine::new(config);
    let targets = engine.target_executables();
    assert!(!targets.is_empty());
    if cfg!(windows) {
        assert!(targets[0].ends_with(".exe"));
    }
}

#[test]
fn test_clamav_engine_database_file_name() {
    let config = ClamAVEngineConfig::default();
    let engine = ClamAVEngine::new(config);
    assert_eq!(engine.database_file_name(), "main.cvd");
}

#[test]
fn test_clamav_engine_get_engine_state() {
    let config = ClamAVEngineConfig {
        state: EngineState {
            install_status: "installed".to_string(),
            install_error: String::new(),
            last_install_attempt: String::new(),
            db_status: "ready".to_string(),
            last_db_update: String::new(),
        },
        ..Default::default()
    };
    let engine = ClamAVEngine::new(config);
    let state = engine.get_engine_state();
    assert_eq!(state.install_status, "installed");
    assert_eq!(state.db_status, "ready");
}

#[test]
fn test_clamav_engine_validate_missing() {
    let dir = tempfile::tempdir().unwrap();
    let config = ClamAVEngineConfig::default();
    let engine = ClamAVEngine::new(config);
    let result = engine.validate(&dir.path().to_string_lossy());
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found"));
}

#[test]
fn test_clamav_engine_setup_null() {
    let config = ClamAVEngineConfig::default();
    let engine = ClamAVEngine::new(config);
    let result = engine.setup(&serde_json::Value::Null);
    assert!(result.is_ok());
}

#[test]
fn test_clamav_engine_setup_valid_json() {
    let config = ClamAVEngineConfig::default();
    let engine = ClamAVEngine::new(config);
    let new_config = serde_json::json!({
        "clamav_path": "/usr/bin",
        "address": "127.0.0.1:3310"
    });
    let result = engine.setup(&new_config);
    assert!(result.is_ok());
    assert_eq!(engine.get_clamav_path(), "/usr/bin");
}

#[test]
fn test_clamav_engine_setup_invalid_json() {
    let config = ClamAVEngineConfig::default();
    let engine = ClamAVEngine::new(config);
    let bad_config = serde_json::json!("not an object");
    let result = engine.setup(&bad_config);
    assert!(result.is_err());
}

#[test]
fn test_clamav_engine_detect_install_path_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let config = ClamAVEngineConfig::default();
    let engine = ClamAVEngine::new(config);
    let result = engine.detect_install_path(dir.path());
    assert!(result.is_err());
}

#[tokio::test]
async fn test_clamav_engine_download_no_url() {
    let config = ClamAVEngineConfig {
        url: String::new(),
        ..Default::default()
    };
    let engine = ClamAVEngine::new(config);
    let result = engine
        .download(
            "/tmp/test",
            tokio_util::sync::CancellationToken::new(),
            None,
        )
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("no download URL"));
}

#[tokio::test]
async fn test_clamav_engine_start_fails_ping() {
    let config = ClamAVEngineConfig {
        address: "127.0.0.1:13310".to_string(), // unlikely port
        ..Default::default()
    };
    let engine = ClamAVEngine::new(config);
    let result = engine.start().await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("ping failed"));
}

#[tokio::test]
async fn test_clamav_engine_start_idempotent() {
    let config = ClamAVEngineConfig::default();
    let engine = ClamAVEngine::new(config);
    // Can't really start, so test double-stop (which uses the same idempotency pattern)
    assert!(engine.stop().await.is_ok());
    assert!(engine.stop().await.is_ok());
}

#[test]
fn test_scan_chain_scan_content_empty_engines() {
    let chain = ScanChain::with_defaults();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(chain.scan_content(b"test"));
    assert!(result.clean);
    assert!(result.results.is_empty());
}

#[test]
fn test_extract_paths_from_args_file_path() {
    let chain = ScanChain::with_defaults();
    let args = serde_json::json!({"file_path": "/tmp/other.txt", "path": "/tmp/first.txt"});
    let paths = chain.extract_paths_from_args("write_file", &args);
    assert!(paths.contains(&"/tmp/first.txt".to_string()));
    assert!(paths.contains(&"/tmp/other.txt".to_string()));
}

#[test]
fn test_extract_paths_from_args_network_download() {
    let chain = ScanChain::with_defaults();
    let args = serde_json::json!({"save_path": "/tmp/file.zip"});
    let paths = chain.extract_paths_from_args("network_download", &args);
    assert!(paths.contains(&"/tmp/file.zip".to_string()));
}

#[test]
fn test_extract_paths_from_args_shell() {
    let chain = ScanChain::with_defaults();
    let args = serde_json::json!({"command": "/usr/bin/python script.py"});
    let paths = chain.extract_paths_from_args("shell", &args);
    assert!(paths.iter().any(|p| p.contains("python")));
}

#[test]
fn test_extract_paths_from_args_process_exec() {
    let chain = ScanChain::with_defaults();
    let args = serde_json::json!({"command": "run /home/user/program.exe --flag"});
    let paths = chain.extract_paths_from_args("process_exec", &args);
    assert!(paths.iter().any(|p| p.contains("program.exe")));
}

#[test]
fn test_scan_chain_config_custom() {
    let config = ScanChainConfig {
        enabled: true,
        max_file_size: 100,
    };
    let chain = ScanChain::new(config);
    assert!(!chain.is_enabled()); // enabled in config but AtomicBool starts false
}

#[tokio::test]
async fn test_scan_tool_invocation_execute_command_no_path() {
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(StubScanner));
    chain.set_enabled(true);
    let args = serde_json::json!({"command": "ls"});
    let (allowed, error) = chain.scan_tool_invocation("execute_command", &args).await;
    assert!(allowed);
    assert!(error.is_none());
}

#[test]
fn test_scan_chain_scan_directory_nonexistent() {
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(StubScanner));
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(chain.scan_directory(Path::new("/nonexistent/path/xyz123")));
    assert!(result.clean);
}

// ---- extract_paths_from_args for new tools ----

#[test]
fn test_extract_paths_shell_command() {
    let chain = ScanChain::with_defaults();
    let args = serde_json::json!({"command": "run ./malware.exe --flag"});
    let paths = chain.extract_paths_from_args("shell", &args);
    assert!(paths.contains(&"./malware.exe".to_string()));
}

#[test]
fn test_extract_paths_exec_async_command() {
    let chain = ScanChain::with_defaults();
    let args = serde_json::json!({"command": "python /tmp/script.py arg"});
    let paths = chain.extract_paths_from_args("exec_async", &args);
    assert!(paths.contains(&"/tmp/script.py".to_string()));
}

#[test]
fn test_extract_paths_screen_capture() {
    let chain = ScanChain::with_defaults();
    let args = serde_json::json!({"save_path": "/tmp/cap.png"});
    let paths = chain.extract_paths_from_args("screen_capture", &args);
    assert!(paths.contains(&"/tmp/cap.png".to_string()));

    let args2 = serde_json::json!({"path": "/tmp/cap2.png"});
    let paths2 = chain.extract_paths_from_args("screen_capture", &args2);
    assert!(paths2.contains(&"/tmp/cap2.png".to_string()));
}

#[test]
fn test_extract_paths_install_skill() {
    let chain = ScanChain::with_defaults();
    let args = serde_json::json!({"path": "/skills/my-skill"});
    let paths = chain.extract_paths_from_args("install_skill", &args);
    assert!(paths.contains(&"/skills/my-skill".to_string()));
}

#[test]
fn test_extract_paths_web_fetch_no_file_path() {
    let chain = ScanChain::with_defaults();
    let args = serde_json::json!({"url": "http://example.com/data"});
    let paths = chain.extract_paths_from_args("web_fetch", &args);
    assert!(paths.is_empty());
}

// ---- scan_tool_invocation for new tools ----

#[tokio::test]
async fn test_scan_tool_invocation_shell_clean() {
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(StubScanner));
    chain.set_enabled(true);
    let args = serde_json::json!({"command": "ls -la"});
    let (allowed, error) = chain.scan_tool_invocation("shell", &args).await;
    assert!(allowed);
    assert!(error.is_none());
}

#[tokio::test]
async fn test_scan_tool_invocation_exec_async_clean() {
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(StubScanner));
    chain.set_enabled(true);
    let args = serde_json::json!({"command": "echo hello"});
    let (allowed, error) = chain.scan_tool_invocation("exec_async", &args).await;
    assert!(allowed);
    assert!(error.is_none());
}

#[tokio::test]
async fn test_scan_tool_invocation_web_fetch_clean() {
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(StubScanner));
    chain.set_enabled(true);
    let args = serde_json::json!({"url": "http://example.com/data", "content": "safe content"});
    let (allowed, error) = chain.scan_tool_invocation("web_fetch", &args).await;
    assert!(allowed);
    assert!(error.is_none());
}

#[tokio::test]
async fn test_scan_tool_invocation_screen_capture_clean() {
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(StubScanner));
    chain.set_enabled(true);
    let args = serde_json::json!({"save_path": "/tmp/cap.png"});
    let (allowed, error) = chain.scan_tool_invocation("screen_capture", &args).await;
    assert!(allowed);
    assert!(error.is_none());
}

#[tokio::test]
async fn test_scan_tool_invocation_install_skill_clean() {
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(StubScanner));
    chain.set_enabled(true);
    let args = serde_json::json!({"url": "https://github.com/user/skill"});
    let (allowed, error) = chain.scan_tool_invocation("install_skill", &args).await;
    assert!(allowed);
    assert!(error.is_none());
}

// ---- cron / cluster_rpc / find_skills ----

#[tokio::test]
async fn test_scan_tool_invocation_cron_clean() {
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(StubScanner));
    chain.set_enabled(true);
    let args = serde_json::json!({"action": "add", "command": "echo hello", "every_seconds": 60});
    let (allowed, error) = chain.scan_tool_invocation("cron", &args).await;
    assert!(allowed);
    assert!(error.is_none());
}

#[test]
fn test_extract_paths_cron_command() {
    let chain = ScanChain::with_defaults();
    let args = serde_json::json!({"action": "add", "command": "run ./malware.exe --flag"});
    let paths = chain.extract_paths_from_args("cron", &args);
    assert!(paths.contains(&"./malware.exe".to_string()));
}

#[test]
fn test_extract_paths_cron_no_command() {
    let chain = ScanChain::with_defaults();
    let args = serde_json::json!({"action": "add", "message": "reminder"});
    let paths = chain.extract_paths_from_args("cron", &args);
    assert!(paths.is_empty());
}

// ============================================================
// MockEngine + chain arms (2026-08-25 coverage push)
// ============================================================
// StubScanner 永远 clean，覆盖不了 chain 的 blocked / not-ready-skip /
// start/stop warn 分支。MockEngine 按 flags 可控返回感染/未就绪/启停失败。

struct MockEngine {
    ready: bool,
    infected: bool,
    fail_start: bool,
    fail_stop: bool,
}

#[async_trait]
impl VirusScanner for MockEngine {
    fn name(&self) -> &str {
        "mock"
    }

    async fn get_info(&self) -> EngineInfo {
        EngineInfo {
            name: "mock".to_string(),
            version: String::new(),
            address: String::new(),
            ready: self.ready,
            start_time: String::new(),
        }
    }

    async fn start(&self) -> Result<(), String> {
        if self.fail_start {
            Err("mock start failure".to_string())
        } else {
            Ok(())
        }
    }

    async fn stop(&self) -> Result<(), String> {
        if self.fail_stop {
            Err("mock stop failure".to_string())
        } else {
            Ok(())
        }
    }

    async fn is_ready(&self) -> bool {
        self.ready
    }

    async fn scan_file(&self, path: &Path) -> ScanResult {
        if self.infected {
            ScanResult::with_threats("mock", "Mock.Virus", &path.to_string_lossy())
        } else {
            ScanResult::clean_with_path("mock", &path.to_string_lossy())
        }
    }

    async fn scan_content(&self, _content: &[u8]) -> ScanResult {
        if self.infected {
            ScanResult::with_threats("mock", "Mock.Virus", "")
        } else {
            ScanResult::clean_from("mock")
        }
    }

    async fn scan_directory(&self, dir: &Path) -> Vec<ScanResult> {
        if self.infected {
            vec![ScanResult::with_threats(
                "mock",
                "Mock.Virus",
                &dir.join("x.bin").to_string_lossy(),
            )]
        } else {
            Vec::new()
        }
    }

    async fn get_database_status(&self) -> DatabaseStatus {
        DatabaseStatus::default()
    }

    async fn update_database(&self) -> Result<(), String> {
        Ok(())
    }

    fn get_stats(&self) -> HashMap<String, serde_json::Value> {
        let mut stats = HashMap::new();
        stats.insert("mock".to_string(), serde_json::json!(true));
        stats
    }
}

fn mock_engine(ready: bool, infected: bool) -> MockEngine {
    MockEngine {
        ready,
        infected,
        fail_start: false,
        fail_stop: false,
    }
}

fn mock_chain(infected: bool) -> ScanChain {
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(mock_engine(true, infected)));
    chain.set_enabled(true);
    chain
}

#[tokio::test]
async fn test_mock_chain_scan_file_blocked_short_circuits() {
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(mock_engine(true, true)));
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("target.exe");
    std::fs::write(&f, "x").unwrap();
    let result = chain.scan_file(&f).await;
    assert!(result.blocked);
    assert!(!result.clean);
    assert_eq!(result.engine, "mock");
    assert_eq!(result.virus, "Mock.Virus");
    assert_eq!(result.results.len(), 1);
}

#[tokio::test]
async fn test_mock_chain_scan_file_not_ready_skipped() {
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(mock_engine(false, true)));
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("skip.exe");
    std::fs::write(&f, "x").unwrap();
    let result = chain.scan_file(&f).await;
    assert!(result.clean);
    assert!(!result.blocked);
    assert!(result.results.is_empty(), "engine must be skipped");
}

#[tokio::test]
async fn test_mock_chain_scan_content_blocked() {
    let chain = mock_chain(true);
    let result = chain.scan_content(b"bad").await;
    assert!(result.blocked);
    assert_eq!(result.engine, "mock");
    assert_eq!(result.virus, "Mock.Virus");
}

#[tokio::test]
async fn test_mock_chain_scan_content_not_ready_skipped() {
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(mock_engine(false, true)));
    let result = chain.scan_content(b"bad").await;
    assert!(result.clean);
    assert!(result.results.is_empty());
}

#[tokio::test]
async fn test_mock_chain_scan_directory_blocked() {
    let chain = mock_chain(true);
    let dir = tempfile::tempdir().unwrap();
    let result = chain.scan_directory(dir.path()).await;
    assert!(result.blocked);
    assert_eq!(result.engine, "mock");
}

#[tokio::test]
async fn test_mock_chain_scan_directory_not_ready_skipped() {
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(mock_engine(false, true)));
    let dir = tempfile::tempdir().unwrap();
    let result = chain.scan_directory(dir.path()).await;
    assert!(result.clean);
    assert!(result.results.is_empty());
}

#[tokio::test]
async fn test_chain_start_stop_with_failing_engine_only_warns() {
    // start()/stop() 对引擎错误只 warn 不 panic —— 覆盖两条 warn 分支。
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(MockEngine {
        ready: true,
        infected: false,
        fail_start: true,
        fail_stop: true,
    }));
    chain.start().await;
    chain.stop().await;
}

#[test]
fn test_chain_clear_engines_resets_count() {
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(StubScanner));
    chain.add_engine(Box::new(mock_engine(true, false)));
    assert_eq!(chain.engine_count(), 2);
    chain.clear_engines();
    assert_eq!(chain.engine_count(), 0);
}

// ---- scan_tool_invocation blocked arms (infection via MockEngine) ----

#[tokio::test]
async fn test_mock_invocation_write_file_content_blocked() {
    let chain = mock_chain(true);
    let args = serde_json::json!({"path": "a.bin", "content": "payload"});
    let (allowed, err) = chain.scan_tool_invocation("write_file", &args).await;
    assert!(!allowed);
    let e = err.expect("virus error");
    assert!(e.contains("Mock.Virus"), "{e}");
    assert!(e.contains("a.bin"), "{e}");
}

#[tokio::test]
async fn test_mock_invocation_edit_file_content_blocked() {
    let chain = mock_chain(true);
    let args = serde_json::json!({"path": "b.bin", "content": "payload"});
    let (allowed, err) = chain.scan_tool_invocation("edit_file", &args).await;
    assert!(!allowed);
    assert!(err.unwrap().contains("Mock.Virus"));
}

#[tokio::test]
async fn test_mock_invocation_download_file_blocked() {
    let chain = mock_chain(true);
    let args = serde_json::json!({"path": "dl.exe"});
    let (allowed, err) = chain.scan_tool_invocation("download", &args).await;
    assert!(!allowed);
    assert!(err.unwrap().contains("dl.exe"));
}

#[tokio::test]
async fn test_mock_invocation_exec_file_blocked() {
    let chain = mock_chain(true);
    let args = serde_json::json!({"path": "run.exe"});
    let (allowed, err) = chain.scan_tool_invocation("exec", &args).await;
    assert!(!allowed);
    assert!(err.unwrap().contains("run.exe"));
}

#[tokio::test]
async fn test_mock_invocation_web_fetch_file_blocked() {
    let chain = mock_chain(true);
    let args = serde_json::json!({"url": "http://x/y", "path": "page.exe"});
    let (allowed, err) = chain.scan_tool_invocation("web_fetch", &args).await;
    assert!(!allowed);
    assert!(err.unwrap().contains("page.exe"));
}

#[tokio::test]
async fn test_mock_invocation_web_fetch_html_content_blocked() {
    // content/data/body/html 四键循环，用 html 键覆盖。
    let chain = mock_chain(true);
    let args = serde_json::json!({"url": "http://x/y", "html": "<script>evil</script>"});
    let (allowed, err) = chain.scan_tool_invocation("web_fetch", &args).await;
    assert!(!allowed);
    assert!(err.unwrap().contains("content from web_fetch"));
}

#[tokio::test]
async fn test_mock_invocation_screen_capture_file_blocked() {
    let chain = mock_chain(true);
    let args = serde_json::json!({"save_path": "shot.exe"});
    let (allowed, err) = chain.scan_tool_invocation("screen_capture", &args).await;
    assert!(!allowed);
    assert!(err.unwrap().contains("shot.exe"));
}

#[tokio::test]
async fn test_mock_invocation_install_skill_file_blocked() {
    let chain = mock_chain(true);
    let args = serde_json::json!({"path": "skill.exe"});
    let (allowed, err) = chain.scan_tool_invocation("install_skill", &args).await;
    assert!(!allowed);
    assert!(err.unwrap().contains("skill.exe"));
}

#[tokio::test]
async fn test_mock_invocation_cron_command_blocked() {
    let chain = mock_chain(true);
    let args = serde_json::json!({"command": "run evil.exe"});
    let (allowed, err) = chain.scan_tool_invocation("cron", &args).await;
    assert!(!allowed);
    assert!(err.unwrap().contains("cron command"));
}

// ---- extract_paths_from_args gaps ----

#[test]
fn test_extract_paths_file_write_uses_both_keys() {
    let chain = ScanChain::with_defaults();
    let args = serde_json::json!({"path": "/tmp/a.txt", "file_path": "/tmp/b.txt"});
    let paths = chain.extract_paths_from_args("file_write", &args);
    assert_eq!(
        paths,
        vec!["/tmp/a.txt".to_string(), "/tmp/b.txt".to_string()]
    );
}

#[test]
fn test_extract_paths_network_download_keys() {
    let chain = ScanChain::with_defaults();
    let args = serde_json::json!({"save_path": "/dl/x.zip", "path": "/alt/y.zip"});
    let paths = chain.extract_paths_from_args("network_download", &args);
    assert_eq!(
        paths,
        vec!["/dl/x.zip".to_string(), "/alt/y.zip".to_string()]
    );
}

#[test]
fn test_extract_paths_process_exec_command_parts() {
    let chain = ScanChain::with_defaults();
    let args = serde_json::json!({"command": "run /bin/tool.sh arg"});
    let paths = chain.extract_paths_from_args("process_exec", &args);
    assert!(paths.contains(&"/bin/tool.sh".to_string()));
}

#[test]
fn test_extract_paths_exec_windows_backslash_path() {
    let chain = ScanChain::with_defaults();
    let args = serde_json::json!({"command": "C:\\tools\\evil.exe -x"});
    let paths = chain.extract_paths_from_args("exec", &args);
    assert!(
        paths.contains(&"C:\\tools\\evil.exe".to_string()),
        "{paths:?}"
    );
}

// ---- create_engine config keys ----

#[test]
fn test_create_engine_clamav_empty_config_defaults() {
    let engine = create_engine("clamav", &serde_json::json!({})).unwrap();
    assert_eq!(engine.name(), "clamav");
}

#[test]
fn test_create_engine_clamav_full_config_keys() {
    // 全键变体：address/enabled/timeout_secs 走 scanner_config，
    // clamav_path/data_dir/update_interval 走 wrapper setters。
    let cfg = serde_json::json!({
        "address": "127.0.0.1:33100",
        "enabled": false,
        "timeout_secs": 45,
        "clamav_path": "/opt/clamav",
        "data_dir": "/var/lib/clamav",
        "update_interval": "12h",
    });
    let engine = create_engine("clamav", &cfg).unwrap();
    assert_eq!(engine.name(), "clamav");
}

#[test]
fn test_scan_result_is_clean_helper() {
    assert!(ScanResult::clean_from("e").is_clean());
    assert!(!ScanResult::with_threats("e", "V", "/p").is_clean());
}

#[test]
fn test_load_from_full_config_unknown_engine_skipped() {
    // enabled 列了 bogus 且给了 config → create_engine Err → warn + 跳过。
    let mut chain = ScanChain::with_defaults();
    let mut full = ScannerFullConfig::default();
    full.enabled.push("bogus".to_string());
    full.engines.insert(
        "bogus".to_string(),
        serde_json::json!({"state": {"install_status": "installed"}}),
    );
    chain.load_from_full_config(&full);
    assert_eq!(chain.engine_count(), 0);
}

// ============================================================
// ClamAVEngine success paths via fake clamd (2026-08-25 coverage push)
// ============================================================

async fn serve_clamd(
    responder: Arc<dyn Fn(&str) -> Vec<u8> + Send + Sync>,
    instream_reply: &'static str,
) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let jh = tokio::spawn(async move {
        loop {
            let (socket, _) = match listener.accept().await {
                Ok(x) => x,
                Err(_) => return,
            };
            let responder = responder.clone();
            tokio::spawn(async move {
                let mut socket = socket;
                let (read_half, mut write_half) = socket.split();
                let mut reader = tokio::io::BufReader::new(read_half);
                let mut line = String::new();
                if reader.read_line(&mut line).await.is_err() {
                    return;
                }
                let cmd = line
                    .trim()
                    .strip_prefix('n')
                    .unwrap_or(line.trim())
                    .to_string();
                if cmd == "INSTREAM" {
                    let mut lenbuf = [0u8; 4];
                    let mut terminated = false;
                    loop {
                        if reader.read_exact(&mut lenbuf).await.is_err() {
                            break;
                        }
                        let len = u32::from_be_bytes(lenbuf) as usize;
                        if len == 0 {
                            terminated = true;
                            break;
                        }
                        let mut chunk = vec![0u8; len];
                        if reader.read_exact(&mut chunk).await.is_err() {
                            break;
                        }
                    }
                    if terminated {
                        let _ = write_half.write_all(instream_reply.as_bytes()).await;
                    }
                } else {
                    let resp = responder(&cmd);
                    if !resp.is_empty() {
                        let _ = write_half.write_all(&resp).await;
                    }
                }
            });
        }
    });
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        jh.abort();
    });
    addr.to_string()
}

fn exe_name() -> &'static str {
    if cfg!(windows) { "clamd.exe" } else { "clamd" }
}

fn engine_on(addr: &str) -> ClamAVEngine {
    ClamAVEngine::new(ClamAVEngineConfig {
        address: addr.to_string(),
        ..Default::default()
    })
}

fn pong_and(
    f: impl Fn(&str) -> Vec<u8> + Send + Sync + 'static,
) -> Arc<dyn Fn(&str) -> Vec<u8> + Send + Sync> {
    Arc::new(move |cmd: &str| {
        if cmd == "PING" {
            b"PONG\n".to_vec()
        } else {
            f(cmd)
        }
    })
}

#[tokio::test]
async fn test_engine_get_database_status_default() {
    let engine = engine_on("127.0.0.1:1");
    let status = engine.get_database_status().await;
    assert!(!status.available);
}

#[test]
fn test_engine_detect_install_path_found() {
    let dir = tempfile::tempdir().unwrap();
    let inst = dir.path().join("inst");
    std::fs::create_dir_all(&inst).unwrap();
    std::fs::write(inst.join(exe_name()), "MZ").unwrap();
    let engine = ClamAVEngine::new(ClamAVEngineConfig::default());
    let found = engine.detect_install_path(dir.path()).unwrap();
    assert_eq!(found, inst.to_string_lossy().to_string());
}

#[test]
fn test_engine_validate_success() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(exe_name()), "MZ").unwrap();
    let engine = ClamAVEngine::new(ClamAVEngineConfig::default());
    engine.validate(&dir.path().to_string_lossy()).unwrap();
}

#[tokio::test]
async fn test_engine_start_ping_fail_errors() {
    let engine = engine_on("127.0.0.1:1");
    let err = engine.start().await.unwrap_err();
    assert!(err.contains("ClamAV ping failed"), "{err}");
    assert!(!engine.is_ready().await);
}

#[tokio::test]
async fn test_engine_start_success_via_fake_clamd() {
    // start 成功（PONG）→ is_ready → scan_file 感染 → update_database Ok → stop。
    let addr = serve_clamd(
        pong_and(|cmd: &str| {
            if let Some(p) = cmd.strip_prefix("SCAN ") {
                format!("{}: Win.Evil FOUND\n", p).into_bytes()
            } else {
                Vec::new()
            }
        }),
        "stream: OK\n",
    )
    .await;
    let engine = engine_on(&addr);
    engine.start().await.unwrap();
    assert!(engine.is_ready().await);

    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("x.exe");
    std::fs::write(&f, "MZ").unwrap();
    let r = engine.scan_file(&f).await;
    assert!(r.infected);
    assert_eq!(r.virus, "Win.Evil");
    assert_eq!(r.engine, "clamav");

    // ready → update_database Ok 分支
    engine.update_database().await.unwrap();

    engine.stop().await.unwrap();
    assert!(!engine.is_ready().await);
}

#[tokio::test]
async fn test_engine_scan_file_error_maps_to_scan_error() {
    // started + SCAN 应答空（client "empty response" Err）→ raw "scan error: ..."
    let addr = serve_clamd(pong_and(|_| Vec::new()), "stream: OK\n").await;
    let engine = engine_on(&addr);
    engine.start().await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("y.bin");
    std::fs::write(&f, "data").unwrap();
    let r = engine.scan_file(&f).await;
    assert!(!r.infected);
    assert!(r.raw.contains("scan error"), "raw: {}", r.raw);
}

#[tokio::test]
async fn test_engine_scan_content_success_infected() {
    let addr = serve_clamd(
        pong_and(|_| Vec::new()),
        "stream: Mock.Stream.Virus FOUND\n",
    )
    .await;
    let engine = engine_on(&addr);
    engine.start().await.unwrap();
    let r = engine.scan_content(b"bad").await;
    assert!(r.infected);
    assert_eq!(r.virus, "Mock.Stream.Virus");
    assert_eq!(r.engine, "clamav");
}

#[tokio::test]
async fn test_engine_scan_content_error_maps_to_scan_error() {
    // INSTREAM 应答缺失（服务端不回）→ client Err → "scan error: ..."
    let addr = serve_clamd(pong_and(|_| Vec::new()), "").await;
    let engine = engine_on(&addr);
    engine.start().await.unwrap();
    let r = engine.scan_content(b"bad").await;
    assert!(!r.infected);
    assert!(r.raw.contains("scan error"), "raw: {}", r.raw);
}

#[tokio::test]
async fn test_engine_scan_directory_started_scans_each_file() {
    let addr = serve_clamd(
        pong_and(|cmd: &str| {
            if let Some(p) = cmd.strip_prefix("SCAN ") {
                format!("{}: OK\n", p).into_bytes()
            } else {
                Vec::new()
            }
        }),
        "stream: OK\n",
    )
    .await;
    let engine = engine_on(&addr);
    engine.start().await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.bin"), "aaa").unwrap();
    std::fs::write(dir.path().join("b.bin"), "bbb").unwrap();
    let results = engine.scan_directory(dir.path()).await;
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|r| !r.infected));
}

// ---- ClamAVEngine download via hand-rolled HTTP server ----

/// 一次性 HTTP 应答服务器：accept → 读请求头 → 写 `resp`；
/// `hold_open_secs` > 0 时写完后保持连接（挂住读端）。
async fn serve_http(resp: Arc<Vec<u8>>, hold_open_secs: u64) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let jh = tokio::spawn(async move {
        loop {
            let (mut s, _) = match listener.accept().await {
                Ok(x) => x,
                Err(_) => return,
            };
            let resp = resp.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf).await;
                let _ = s.write_all(&resp).await;
                if hold_open_secs > 0 {
                    tokio::time::sleep(std::time::Duration::from_secs(hold_open_secs)).await;
                }
            });
        }
    });
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        jh.abort();
    });
    addr.to_string()
}

fn engine_with_url(url: String) -> ClamAVEngine {
    ClamAVEngine::new(ClamAVEngineConfig {
        url,
        ..Default::default()
    })
}

#[tokio::test]
async fn test_engine_download_connection_refused() {
    // 关闭端口 → reqwest Err；嵌套目标目录先被 create_dir_all 建好。
    let engine = engine_with_url("http://127.0.0.1:1/clamav.zip".to_string());
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("nested").join("deep");
    let err = engine
        .download(
            &dest.to_string_lossy(),
            tokio_util::sync::CancellationToken::new(),
            None,
        )
        .await
        .unwrap_err();
    assert!(err.contains("download failed"), "{err}");
    assert!(dest.is_dir(), "target dir must be created before download");
}

#[tokio::test]
async fn test_engine_download_non_success_status() {
    let resp: Vec<u8> =
        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec();
    let addr = serve_http(Arc::new(resp), 0).await;
    let engine = engine_with_url(format!("http://{}/clamav.zip", addr));
    let dir = tempfile::tempdir().unwrap();
    let err = engine
        .download(
            &dir.path().to_string_lossy(),
            tokio_util::sync::CancellationToken::new(),
            None,
        )
        .await
        .unwrap_err();
    assert!(err.contains("status: 404"), "{err}");
}

#[tokio::test]
async fn test_engine_download_truncated_stream() {
    // Content-Length: 100 但只发 10 字节就关连接 → stream Err → "download read failed"
    let mut resp = b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n".to_vec();
    resp.extend_from_slice(b"0123456789");
    let addr = serve_http(Arc::new(resp), 0).await;
    let engine = engine_with_url(format!("http://{}/clamav.zip", addr));
    let dir = tempfile::tempdir().unwrap();
    let err = engine
        .download(
            &dir.path().to_string_lossy(),
            tokio_util::sync::CancellationToken::new(),
            None,
        )
        .await
        .unwrap_err();
    assert!(err.contains("download read failed"), "{err}");
    // 失败路径必须清掉临时 zip
    assert!(!dir.path().join("clamav-download.zip").exists());
}

#[tokio::test]
async fn test_engine_download_cancelled_token() {
    // 只发响应头、不发 body 并挂住连接 → stream.next() pending；
    // 预取消的 token 立即就绪 → select 走取消分支。
    let resp: Vec<u8> = b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n".to_vec();
    let addr = serve_http(Arc::new(resp), 5).await;
    let engine = engine_with_url(format!("http://{}/clamav.zip", addr));
    let dir = tempfile::tempdir().unwrap();
    let token = tokio_util::sync::CancellationToken::new();
    token.cancel();
    let err = engine
        .download(&dir.path().to_string_lossy(), token, None)
        .await
        .unwrap_err();
    assert!(err.contains("cancelled"), "{err}");
    assert!(!dir.path().join("clamav-download.zip").exists());
}

#[tokio::test]
async fn test_engine_download_success_extracts_and_detects() {
    use std::io::Write;
    // 真 zip（zip::ZipWriter 手搓）：含 clamd(.exe) → 下载 → 解压 →
    // detect_install_path → 写 clamav_path + 最终进度回调 (len, len)。
    let mut zw = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    zw.start_file(exe_name(), opts).unwrap();
    zw.write_all(b"fake clamd binary").unwrap();
    let cursor = zw.finish().unwrap();
    let zip_bytes = cursor.into_inner();

    let mut resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        zip_bytes.len()
    )
    .into_bytes();
    resp.extend_from_slice(&zip_bytes);
    let addr = serve_http(Arc::new(resp), 0).await;

    let engine = engine_with_url(format!("http://{}/clamav.zip", addr));
    let dir = tempfile::tempdir().unwrap();
    let dir_str = dir.path().to_string_lossy().to_string();

    let calls: Arc<std::sync::Mutex<Vec<(u64, u64)>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let cb_calls = calls.clone();
    let cb: Arc<dyn Fn(u64, u64) + Send + Sync> =
        Arc::new(move |a, b| cb_calls.lock().unwrap().push((a, b)));

    engine
        .download(
            &dir_str,
            tokio_util::sync::CancellationToken::new(),
            Some(cb),
        )
        .await
        .unwrap();

    assert!(
        dir.path().join(exe_name()).exists(),
        "zip must be extracted"
    );
    assert_eq!(
        engine.get_clamav_path(),
        dir_str,
        "install path auto-detected"
    );
    assert!(
        !dir.path().join("clamav-download.zip").exists(),
        "temp zip must be cleaned up"
    );
    let got = calls.lock().unwrap().clone();
    let total = zip_bytes.len() as u64;
    assert!(got.contains(&(total, total)), "progress calls: {got:?}");
}

#[test]
fn test_extract_zip_archive_skips_unsafe_names() {
    use std::io::Write;
    // "../evil.txt" → enclosed_name None → continue（不逃逸解压）。
    let dir = tempfile::tempdir().unwrap();
    let zip_path = dir.path().join("evil.zip");
    let mut zw = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
    let opts = zip::write::SimpleFileOptions::default();
    zw.start_file("../evil.txt", opts).unwrap();
    zw.write_all(b"escape").unwrap();
    zw.finish().unwrap();

    let dest = dir.path().join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    extract_zip_archive(&zip_path, &dest).unwrap();
    assert!(!dir.path().join("evil.txt").exists());
    assert!(!dest.join("evil.txt").exists());
}

#[test]
fn test_extract_zip_archive_dir_and_nested_entries() {
    use std::io::Write;
    // 目录条目（is_dir 分支）+ 嵌套父目录（parent create 分支）。
    let dir = tempfile::tempdir().unwrap();
    let zip_path = dir.path().join("nested.zip");
    let mut zw = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
    let opts = zip::write::SimpleFileOptions::default();
    zw.add_directory("sub/", opts).unwrap();
    zw.start_file("a/b/c.txt", opts).unwrap();
    zw.write_all(b"nested content").unwrap();
    zw.finish().unwrap();

    let dest = dir.path().join("out");
    extract_zip_archive(&zip_path, &dest).unwrap();
    assert!(dest.join("sub").is_dir());
    assert_eq!(
        std::fs::read_to_string(dest.join("a").join("b").join("c.txt")).unwrap(),
        "nested content"
    );
}

// ============================================================
// ClamavScannerWrapper internals (2026-08-25 coverage push)
// ============================================================
// wrapper 是 scanner 模块私有 struct —— tests 子模块可直接访问私有字段
// （manager / manager_config / started）与私有方法。

fn wrapper_on(addr: &str) -> ClamavScannerWrapper {
    ClamavScannerWrapper::new(crate::clamav::scanner::ScannerConfig {
        address: addr.to_string(),
        ..Default::default()
    })
}

#[test]
fn test_wrapper_setters_populate_manager_config() {
    let mut w = wrapper_on("127.0.0.1:3310");
    assert!(w.manager_config.clamav_path.is_empty());
    w.set_clamav_path("C:/clamav".to_string());
    w.set_data_dir("C:/clamav/data".to_string());
    w.set_update_interval("12h".to_string());
    assert_eq!(w.manager_config.clamav_path, "C:/clamav");
    assert_eq!(w.manager_config.data_dir, "C:/clamav/data");
    assert_eq!(w.manager_config.update_interval, "12h");
}

// 归属检查（clamd_is_ours）的 fail-closed 语义是 Windows 专属实现
// （netstat -ano → PID → QueryFullProcessImageNameW）；非 Windows 是文档化
// stub（直接 true 假定是我们的，见 clamav/ownership.rs 头注）——三个
// "外来监听者必须被拒" 测试的前提在 stub 下不成立 → cfg(windows) 门控
// （2026-09-01 Linux 首跑暴露）。
#[cfg(windows)]
#[tokio::test]
async fn test_wrapper_clamd_is_ours_false_for_foreign_listener() {
    // 端口有监听者但它是测试进程（exe 不叫 clamd.exe）→ 非我们的 → false。
    let addr = serve_clamd(pong_and(|_| Vec::new()), "stream: OK\n").await;
    let w = wrapper_on(&addr);
    assert!(!w.clamd_is_ours());
}

#[tokio::test]
async fn test_wrapper_restart_clamd_if_down_up_returns_true() {
    // ping 通 → true（第一分支）。
    let addr = serve_clamd(pong_and(|_| Vec::new()), "stream: OK\n").await;
    let w = wrapper_on(&addr);
    assert!(w.restart_clamd_if_down().await);
}

#[tokio::test]
async fn test_wrapper_restart_clamd_if_down_no_manager_returns_false() {
    // ping 失败 + 无 manager → warn + false。
    let w = wrapper_on("127.0.0.1:1");
    assert!(!w.restart_clamd_if_down().await);
}

#[tokio::test]
async fn test_wrapper_restart_clamd_if_down_manager_error_returns_false() {
    // manager Some 但未启动 → restart Err("no daemon to restart") → false。
    let w = wrapper_on("127.0.0.1:1");
    let mgr = crate::clamav::manager::Manager::new(crate::clamav::manager::ManagerConfig {
        enabled: false,
        clamav_path: String::new(),
        data_dir: String::new(),
        address: String::new(),
        scanner: None,
        update_interval: String::new(),
    });
    *w.manager.lock().await = Some(mgr);
    assert!(!w.restart_clamd_if_down().await);
}

// ---- Windows-only: where.exe 假 clamd.exe + 进程内 PONG ----

#[cfg(windows)]
fn place_where_as(dir: &std::path::Path, name: &str) {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".to_string());
    let src = std::path::Path::new(&windir)
        .join("System32")
        .join("where.exe");
    assert!(src.exists(), "where.exe not found at {}", src.display());
    std::fs::copy(&src, dir.join(format!("{}.exe", name))).unwrap();
}

#[cfg(windows)]
async fn serve_pong() -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let jh = tokio::spawn(async move {
        loop {
            let (mut s, _) = match listener.accept().await {
                Ok(x) => x,
                Err(_) => return,
            };
            let mut buf = [0u8; 64];
            let _ = s.read(&mut buf).await;
            let _ = s.write_all(b"PONG\n").await;
        }
    });
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        jh.abort();
    });
    addr.to_string()
}

#[cfg(windows)]
fn fake_started_manager(
    clamav_dir: &std::path::Path,
    data_dir: &std::path::Path,
    addr: String,
) -> crate::clamav::manager::Manager {
    crate::clamav::manager::Manager::new(crate::clamav::manager::ManagerConfig {
        enabled: true,
        clamav_path: clamav_dir.to_string_lossy().to_string(),
        data_dir: data_dir.to_string_lossy().to_string(),
        address: addr,
        scanner: None,
        update_interval: "1h".to_string(),
    })
}

#[cfg(windows)]
#[tokio::test]
async fn test_wrapper_restart_clamd_if_down_manager_restart_ok_returns_true() {
    // wrapper 指向关闭端口（ping 失败 → manager 分支）；注入一个完整假启动的
    // manager（readiness 地址=进程内 PONG）→ restart Ok → true。
    let clamav_dir = tempfile::tempdir().unwrap();
    place_where_as(clamav_dir.path(), "clamd");
    let data_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(data_dir.path().join("database")).unwrap();
    std::fs::write(
        data_dir.path().join("database").join("main.cvd"),
        "fake cvd",
    )
    .unwrap();

    let pong = serve_pong().await;
    let mut mgr = fake_started_manager(clamav_dir.path(), data_dir.path(), pong);
    mgr.start().await.unwrap();

    let w = wrapper_on("127.0.0.1:1");
    *w.manager.lock().await = Some(mgr);
    assert!(w.restart_clamd_if_down().await);
}

#[tokio::test]
async fn test_wrapper_stop_foreign_clamd_skips_shutdown() {
    // started=true + manager None + 端口无监听（非我们的）→ Ok 且不发 SHUTDOWN。
    let w = wrapper_on("127.0.0.1:1");
    w.started.store(true, Ordering::SeqCst);
    w.stop().await.unwrap();
}

#[cfg(windows)]
#[tokio::test]
async fn test_wrapper_stop_with_manager_stops_manager() {
    // manager Some 分支：stop() → mgr.stop()（杀 where.exe 死 child）→ Ok。
    let clamav_dir = tempfile::tempdir().unwrap();
    place_where_as(clamav_dir.path(), "clamd");
    let data_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(data_dir.path().join("database")).unwrap();
    std::fs::write(
        data_dir.path().join("database").join("main.cvd"),
        "fake cvd",
    )
    .unwrap();

    let pong = serve_pong().await;
    let mut mgr = fake_started_manager(clamav_dir.path(), data_dir.path(), pong);
    mgr.start().await.unwrap();

    let w = wrapper_on("127.0.0.1:1");
    *w.manager.lock().await = Some(mgr);
    w.started.store(true, Ordering::SeqCst);
    w.stop().await.unwrap();
}

#[cfg(windows)]
#[tokio::test]
async fn test_wrapper_start_residual_foreign_refused() {
    // 残留检测：ping 通但不是我们的 clamd → Err "not ours"。
    let addr = serve_clamd(pong_and(|_| Vec::new()), "stream: OK\n").await;
    let w = wrapper_on(&addr);
    let err = w.start().await.unwrap_err();
    assert!(err.contains("not ours"), "{err}");
    assert!(!w.started.load(Ordering::SeqCst));
}

#[tokio::test]
async fn test_wrapper_start_manager_fail_then_ping_fail() {
    // clamav_path 指向空目录 → Manager::start Err → 回退 ping-only →
    // 关闭端口 ping 失败 → Err "ClamAV ping failed"。
    let mut w = wrapper_on("127.0.0.1:1");
    let empty = tempfile::tempdir().unwrap();
    w.set_clamav_path(empty.path().to_string_lossy().to_string());
    let err = w.start().await.unwrap_err();
    assert!(err.contains("ClamAV ping failed"), "{err}");
}

#[cfg(windows)]
#[tokio::test]
async fn test_wrapper_start_manager_fail_fallback_foreign_refused() {
    // Manager 失败 + 回退 ping 通但非我们的 → Err "not reusing foreign"。
    let addr = serve_clamd(pong_and(|_| Vec::new()), "stream: OK\n").await;
    let mut w = wrapper_on(&addr);
    let empty = tempfile::tempdir().unwrap();
    w.set_clamav_path(empty.path().to_string_lossy().to_string());
    let err = w.start().await.unwrap_err();
    // 实际消息是 "clamd at {addr} is not ours; refusing to reuse foreign
    // ClamAV"——注意是 "refusing to reuse"，不是 "not reusing"。
    assert!(err.contains("refusing to reuse foreign ClamAV"), "{err}");
}

#[tokio::test]
async fn test_wrapper_get_info_ready_with_pong() {
    let addr = serve_clamd(pong_and(|_| Vec::new()), "stream: OK\n").await;
    let w = wrapper_on(&addr);
    let info = w.get_info().await;
    assert!(info.ready);
    assert_eq!(info.address, addr);
}

#[tokio::test]
async fn test_wrapper_is_ready_manager_not_running_returns_false() {
    // manager Some 且 is_running()=false → 提前 false（不 ping）。
    let w = wrapper_on("127.0.0.1:1");
    let mgr = crate::clamav::manager::Manager::new(crate::clamav::manager::ManagerConfig {
        enabled: false,
        clamav_path: String::new(),
        data_dir: String::new(),
        address: String::new(),
        scanner: None,
        update_interval: String::new(),
    });
    *w.manager.lock().await = Some(mgr);
    w.started.store(true, Ordering::SeqCst);
    assert!(!w.is_ready().await);
}

#[tokio::test]
async fn test_wrapper_scan_file_success_when_started() {
    let addr = serve_clamd(
        pong_and(|cmd: &str| {
            if let Some(p) = cmd.strip_prefix("SCAN ") {
                format!("{}: OK\n", p).into_bytes()
            } else {
                Vec::new()
            }
        }),
        "stream: OK\n",
    )
    .await;
    let w = wrapper_on(&addr);
    w.started.store(true, Ordering::SeqCst);
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("z.exe");
    std::fs::write(&f, "MZ").unwrap();
    let r = w.scan_file(&f).await;
    assert!(!r.infected);
    assert!(r.path.contains("z.exe"));
    assert_eq!(r.engine, "clamav");
}

#[tokio::test]
async fn test_wrapper_scan_file_fail_open_after_restart_attempt() {
    // started + 连不上 → restart_clamd_if_down false → G5 fail-open（clean +
    // raw "scan error (clamd unavailable, restart attempted)"）。
    let w = wrapper_on("127.0.0.1:1");
    w.started.store(true, Ordering::SeqCst);
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("q.bin");
    std::fs::write(&f, "d").unwrap();
    let r = w.scan_file(&f).await;
    assert!(!r.infected);
    assert!(r.raw.contains("restart attempted"), "raw: {}", r.raw);
}

#[tokio::test]
async fn test_wrapper_scan_content_success_when_started() {
    let addr = serve_clamd(pong_and(|_| Vec::new()), "stream: OK\n").await;
    let w = wrapper_on(&addr);
    w.started.store(true, Ordering::SeqCst);
    let r = w.scan_content(b"payload").await;
    assert!(!r.infected);
}

#[tokio::test]
async fn test_wrapper_scan_content_fail_open_after_restart_attempt() {
    let w = wrapper_on("127.0.0.1:1");
    w.started.store(true, Ordering::SeqCst);
    let r = w.scan_content(b"payload").await;
    assert!(!r.infected);
    assert!(r.raw.contains("restart attempted"), "raw: {}", r.raw);
}

// ============================================================
// S3 batch 4: ScanResult::merge / detect_install_path /
// extension-rule skip / scan_directory / scan_tool_invocation
// 各工具臂 / load_from_full_config 跳过未安装引擎
// ============================================================

#[test]
fn test_scan_result_merge_fills_empty_fields() {
    let mut empty = ScanResult::clean();
    let other = ScanResult::with_threats("clamav", "Win.Eicar", "/tmp/x.exe");
    empty.merge(&other);
    assert!(empty.infected);
    assert_eq!(empty.virus, "Win.Eicar");
    assert_eq!(empty.engine, "clamav");
    assert_eq!(empty.path, "/tmp/x.exe");

    // 已有值不被覆盖：再 merge 另一个威胁，virus/engine 保持第一次的
    let second = ScanResult::with_threats("other", "Other.Virus", "/tmp/y.exe");
    empty.merge(&second);
    assert_eq!(empty.virus, "Win.Eicar");
    assert_eq!(empty.engine, "clamav");
}

#[test]
fn test_clamav_engine_detect_install_path_found_and_missing() {
    use crate::scanner::InstallableEngine;
    let engine = ClamAVEngine::new(ClamAVEngineConfig::default());

    // 目录里放一个假 clamd 可执行 → 找到其父目录
    let dir = tempfile::tempdir().unwrap();
    let nest = dir.path().join("deep").join("bin");
    std::fs::create_dir_all(&nest).unwrap();
    let exe_name = if cfg!(target_os = "windows") {
        "clamd.exe"
    } else {
        "clamd"
    };
    std::fs::write(nest.join(exe_name), b"fake").unwrap();
    let found = engine.detect_install_path(dir.path()).unwrap();
    assert!(found.ends_with("bin"), "found: {found}");

    // 空目录 → Err
    let empty_dir = tempfile::tempdir().unwrap();
    let err = engine.detect_install_path(empty_dir.path()).unwrap_err();
    assert!(err.contains("target executable not found"), "{err}");
}

/// 可配置 mock：文件/内容/目录都可指定是否感染。
struct S3MockEngine {
    infected: bool,
}

#[async_trait::async_trait]
impl crate::scanner::VirusScanner for S3MockEngine {
    fn name(&self) -> &str {
        "s3mock"
    }
    async fn get_info(&self) -> crate::scanner::EngineInfo {
        crate::scanner::EngineInfo {
            name: "s3mock".to_string(),
            version: String::new(),
            address: String::new(),
            ready: true,
            start_time: String::new(),
        }
    }
    async fn start(&self) -> Result<(), String> {
        Ok(())
    }
    async fn stop(&self) -> Result<(), String> {
        Ok(())
    }
    async fn is_ready(&self) -> bool {
        true
    }
    async fn scan_file(&self, path: &std::path::Path) -> crate::scanner::ScanResult {
        if self.infected {
            crate::scanner::ScanResult::with_threats("s3mock", "S3.Test", &path.to_string_lossy())
        } else {
            crate::scanner::ScanResult::clean_with_path("s3mock", &path.to_string_lossy())
        }
    }
    async fn scan_content(&self, _content: &[u8]) -> crate::scanner::ScanResult {
        if self.infected {
            crate::scanner::ScanResult::with_threats("s3mock", "S3.Test", "")
        } else {
            crate::scanner::ScanResult::clean_from("s3mock")
        }
    }
    async fn scan_directory(&self, dir: &std::path::Path) -> Vec<crate::scanner::ScanResult> {
        if self.infected {
            vec![crate::scanner::ScanResult::with_threats(
                "s3mock",
                "S3.Test",
                &dir.join("found.exe").to_string_lossy(),
            )]
        } else {
            Vec::new()
        }
    }
    async fn get_database_status(&self) -> crate::scanner::DatabaseStatus {
        crate::scanner::DatabaseStatus::default()
    }
    async fn update_database(&self) -> Result<(), String> {
        Ok(())
    }
    fn get_stats(&self) -> HashMap<String, serde_json::Value> {
        HashMap::new()
    }
}

#[tokio::test]
async fn test_scan_file_extension_whitelist_skips_file() {
    // 白名单只扫 exe → txt 文件直接 clean，即便引擎是"全感染" mock。
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(S3MockEngine { infected: true }));
    chain.rules = ExtensionRules::new(vec!["exe".to_string()], vec![]);

    let dir = tempfile::tempdir().unwrap();
    let txt = dir.path().join("notes.txt");
    std::fs::write(&txt, "plain").unwrap();
    let r = chain.scan_file(&txt).await;
    assert!(r.clean);
    assert!(!r.blocked);

    // exe 在白名单 → 被 mock 拦
    let exe = dir.path().join("prog.exe");
    std::fs::write(&exe, b"MZ").unwrap();
    let r2 = chain.scan_file(&exe).await;
    assert!(r2.blocked);
    assert_eq!(r2.engine, "s3mock");
}

#[tokio::test]
async fn test_scan_directory_empty_engines_and_blocked() {
    // 无引擎 → clean
    let empty_chain = ScanChain::new(ScanChainConfig::default());
    let dir = tempfile::tempdir().unwrap();
    let r = empty_chain.scan_directory(dir.path()).await;
    assert!(r.clean);

    // 感染引擎 → 目录扫描 blocked
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(S3MockEngine { infected: true }));
    let r2 = chain.scan_directory(dir.path()).await;
    assert!(r2.blocked);
    assert!(r2.path.ends_with("found.exe"), "path: {}", r2.path);
    assert_eq!(r2.engine, "s3mock");
}

#[tokio::test]
async fn test_scan_tool_invocation_no_engines_early_ok() {
    // enabled 但零引擎 → (true, None)
    let chain = ScanChain::with_defaults();
    chain.set_enabled(true);
    let (allowed, err) = chain
        .scan_tool_invocation("download", &serde_json::json!({"save_path": "/tmp/x.exe"}))
        .await;
    assert!(allowed);
    assert!(err.is_none());
}

#[tokio::test]
async fn test_scan_tool_invocation_blocks_each_tool_arm() {
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(Box::new(S3MockEngine { infected: true }));
    chain.set_enabled(true);

    // write_file：content 感染
    let (a, e) = chain
        .scan_tool_invocation(
            "write_file",
            &serde_json::json!({"path": "/tmp/o.exe", "content": "payload"}),
        )
        .await;
    assert!(!a);
    assert!(
        e.as_deref().unwrap_or("").contains("virus detected"),
        "{e:?}"
    );

    // download：save_path 文件感染
    let (a, e) = chain
        .scan_tool_invocation("download", &serde_json::json!({"save_path": "/tmp/dl.exe"}))
        .await;
    assert!(!a);
    assert!(e.as_deref().unwrap_or("").contains("/tmp/dl.exe"), "{e:?}");

    // exec：path 文件感染
    let (a, e) = chain
        .scan_tool_invocation("exec", &serde_json::json!({"path": "/tmp/run.exe"}))
        .await;
    assert!(!a);
    assert!(e.as_deref().unwrap_or("").contains("/tmp/run.exe"), "{e:?}");

    // web_fetch：save_path 文件感染
    let (a, e) = chain
        .scan_tool_invocation(
            "web_fetch",
            &serde_json::json!({"save_path": "/tmp/fetched.exe"}),
        )
        .await;
    assert!(!a);
    assert!(
        e.as_deref().unwrap_or("").contains("/tmp/fetched.exe"),
        "{e:?}"
    );

    // cron：command 内容感染
    let (a, e) = chain
        .scan_tool_invocation("cron", &serde_json::json!({"command": "rm -rf /"}))
        .await;
    assert!(!a);
    assert!(e.as_deref().unwrap_or("").contains("cron command"), "{e:?}");

    // 未知工具 → 放行
    let (a, e) = chain
        .scan_tool_invocation("unknown_tool", &serde_json::json!({"path": "/tmp/x.exe"}))
        .await;
    assert!(a);
    assert!(e.is_none());
}

#[tokio::test]
async fn test_load_from_full_config_skips_pending_engine() {
    // install_status != "installed" → 引擎被跳过
    let mut chain = ScanChain::with_defaults();
    let mut full_config = ScannerFullConfig::default();
    full_config.enabled.push("clamav".to_string());
    full_config.engines.insert(
        "clamav".to_string(),
        serde_json::json!({"state": {"install_status": "pending"}}),
    );
    chain.load_from_full_config(&full_config);
    assert_eq!(chain.engine_count(), 0, "pending engine must be skipped");
}

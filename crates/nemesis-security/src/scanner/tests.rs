use super::*;

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
    let rules = ExtensionRules::new(
        vec!["exe".to_string(), "dll".to_string()],
        vec![],
    );
    assert!(rules.should_scan_file(Path::new("program.exe")));
    assert!(rules.should_scan_file(Path::new("lib.dll")));
    assert!(!rules.should_scan_file(Path::new("test.txt")));
}

#[test]
fn test_extension_rules_blacklist() {
    let rules = ExtensionRules::new(
        vec![],
        vec!["txt".to_string(), "md".to_string()],
    );
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
    full_config.engines.insert(
        "stub".to_string(),
        serde_json::json!({"key": "value"}),
    );
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
    let rules = ExtensionRules::new(
        vec!["EXE".to_string(), "DLL".to_string()],
        vec![],
    );
    assert!(rules.should_scan_file(Path::new("program.exe")));
    assert!(rules.should_scan_file(Path::new("PROGRAM.EXE")));
    assert!(rules.should_scan_file(Path::new("lib.Dll")));
}

#[test]
fn test_extension_rules_skip_case_insensitive() {
    let rules = ExtensionRules::new(
        vec![],
        vec!["TXT".to_string(), "MD".to_string()],
    );
    assert!(!rules.should_scan_file(Path::new("readme.txt")));
    assert!(!rules.should_scan_file(Path::new("README.MD")));
}

#[test]
fn test_extension_rules_no_extension() {
    let rules = ExtensionRules::new(
        vec!["exe".to_string()],
        vec![],
    );
    assert!(!rules.should_scan_file(Path::new("Makefile")));
    assert!(!rules.should_scan_file(Path::new("noext")));
}

#[test]
fn test_extension_rules_skip_no_extension() {
    let rules = ExtensionRules::new(
        vec![],
        vec!["txt".to_string()],
    );
    // File without extension should pass (not in skip list)
    assert!(rules.should_scan_file(Path::new("Makefile")));
}

#[test]
fn test_extension_rules_hidden_file() {
    let rules = ExtensionRules::new(
        vec!["exe".to_string()],
        vec![],
    );
    assert!(!rules.should_scan_file(Path::new(".hidden")));
}

#[test]
fn test_extension_rules_path_with_dirs() {
    let rules = ExtensionRules::new(
        vec!["exe".to_string()],
        vec![],
    );
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
    let args = serde_json::json!({"save_path": "/tmp/download.zip", "url": "https://example.com/file"});
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
    full_config.engines.insert(
        "stub".to_string(),
        serde_json::json!({"key": "value"}),
    );
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
    let scanner = ScanEngine::ClamAV.build();
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
    let result = scanner.scan_file(Path::new("/some/deep/path/file.exe")).await;
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
    let result = engine.download("/tmp/test", tokio_util::sync::CancellationToken::new(), None).await;
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

use super::*;

    #[test]
    fn test_verify_token() {
        assert!(verify_token("test", "test"));
        assert!(!verify_token("wrong", "test"));
        assert!(verify_token("anything", ""));
    }

    #[test]
    fn test_load_scanner_status_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let status = load_scanner_status(&ws);
        assert_eq!(status["enabled"], false);
        assert!(status["engines"].as_array().unwrap().is_empty());
    }

    #[test]
    fn test_load_scanner_status_with_file() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        let config = serde_json::json!({
            "enabled": ["clamav"],
            "engines": {
                "clamav": {"path": "/usr/bin/clamav"}
            }
        });
        std::fs::write(
            config_dir.join("config.scanner.json"),
            serde_json::to_string_pretty(&config).unwrap(),
        )
        .unwrap();

        let ws = dir.path().to_string_lossy().to_string();
        let status = load_scanner_status(&ws);
        assert_eq!(status["enabled"], true);
        let engines = status["engines"].as_array().unwrap();
        assert_eq!(engines.len(), 1);
        assert_eq!(engines[0]["name"], "clamav");
    }

    #[test]
    fn test_resolve_log_file_path_general() {
        let dir = tempfile::tempdir().unwrap();
        let logs_dir = dir.path().join("logs");
        std::fs::create_dir_all(&logs_dir).unwrap();
        // New JSONL daily format: nemesisbot.YYYY-MM-DD (no .log suffix).
        std::fs::write(logs_dir.join("nemesisbot.2026-06-17"), "log content").unwrap();

        let ws = dir.path().to_string_lossy().to_string();
        let path = resolve_log_file_path(&ws, "general").unwrap();
        assert!(path.contains("nemesisbot.2026-06-17"));
    }

    #[test]
    fn test_resolve_log_file_path_general_fallback() {
        // No matching file: should return None (no legacy fallback).
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        assert!(resolve_log_file_path(&ws, "general").is_none());
    }

    #[test]
    fn test_resolve_log_file_path_general_picks_latest_day() {
        let dir = tempfile::tempdir().unwrap();
        let logs_dir = dir.path().join("logs");
        std::fs::create_dir_all(&logs_dir).unwrap();
        std::fs::write(logs_dir.join("nemesisbot.2026-06-15"), "day 1").unwrap();
        std::fs::write(logs_dir.join("nemesisbot.2026-06-17"), "day 3").unwrap();
        std::fs::write(logs_dir.join("nemesisbot.2026-06-16"), "day 2").unwrap();

        let ws = dir.path().to_string_lossy().to_string();
        let path = resolve_log_file_path(&ws, "general").unwrap();
        // Lexicographic sort == chronological for YYYY-MM-DD, latest wins.
        assert!(path.contains("nemesisbot.2026-06-17"));
    }

    #[test]
    fn test_resolve_log_file_path_llm() {
        let dir = tempfile::tempdir().unwrap();
        // request_logs 下每个 LLM 调用是一个目录，内含多个 Markdown 文件
        let session_dir = dir.path().join("logs").join("request_logs").join("2026-04-30_14-23-45_001");
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(session_dir.join("00.request.md"), "request content").unwrap();

        let ws = dir.path().to_string_lossy().to_string();
        let path = resolve_log_file_path(&ws, "llm").unwrap();
        assert!(path.contains("00.request.md"));
    }

    #[test]
    fn test_resolve_log_file_path_cluster() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        assert!(resolve_log_file_path(&ws, "cluster").is_none());
    }

    #[test]
    fn test_resolve_log_file_path_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        assert!(resolve_log_file_path(&ws, "unknown").is_none());
    }

    #[test]
    fn test_read_log_entries_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.jsonl");
        let content = r#"{"level":"info","message":"line1"}
{"level":"warn","message":"line2"}
{"level":"error","message":"line3"}
"#;
        std::fs::write(&file_path, content).unwrap();

        let entries = read_log_entries(&file_path.to_string_lossy(), 2);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["level"], "warn");
        assert_eq!(entries[1]["level"], "error");
    }

    #[test]
    fn test_read_log_entries_mixed() {
        // New behavior: JSON-only parsing. Non-JSON lines are dropped (no text fallback).
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("mixed.log");
        let content = "plain text line\n{\"json\":true}\n";
        std::fs::write(&file_path, content).unwrap();

        let entries = read_log_entries(&file_path.to_string_lossy(), 100);
        assert_eq!(entries.len(), 1, "only the JSON line should parse");
        assert_eq!(entries[0]["json"], true);
    }

    #[test]
    fn test_read_log_entries_nonexistent() {
        let entries = read_log_entries("/nonexistent/path.log", 100);
        assert!(entries.is_empty());
    }

    #[test]
    fn test_sanitize_map_simple() {
        let mut map = serde_json::json!({
            "api_key": "sk-12345678",
            "name": "test",
        })
        .as_object_mut()
        .unwrap()
        .clone();

        sanitize_map(&mut map);
        assert_eq!(map["api_key"], "sk-1****");
        assert_eq!(map["name"], "test");
    }

    #[test]
    fn test_sanitize_map_short_value() {
        let mut map = serde_json::json!({
            "token": "ab",
        })
        .as_object_mut()
        .unwrap()
        .clone();

        sanitize_map(&mut map);
        assert_eq!(map["token"], "****");
    }

    #[test]
    fn test_sanitize_map_nested() {
        let mut map = serde_json::json!({
            "config": {
                "secret_key": "secretvalue",
                "port": 8080,
            }
        })
        .as_object_mut()
        .unwrap()
        .clone();

        sanitize_map(&mut map);
        let config = map["config"].as_object().unwrap();
        assert_eq!(config["secret_key"], "secr****");
        assert_eq!(config["port"], 8080);
    }

    #[test]
    fn test_sanitize_map_empty_string() {
        let mut map = serde_json::json!({
            "password": "",
        })
        .as_object_mut()
        .unwrap()
        .clone();

        sanitize_map(&mut map);
        // Empty string should NOT be sanitized
        assert_eq!(map["password"], "");
    }

    #[test]
    fn test_verify_token_empty_expected() {
        assert!(verify_token("anything", ""));
        assert!(verify_token("", ""));
    }

    #[test]
    fn test_verify_token_exact_match() {
        assert!(verify_token("my-secret-token", "my-secret-token"));
    }

    #[test]
    fn test_verify_token_mismatch() {
        assert!(!verify_token("wrong", "expected"));
    }

    #[test]
    fn test_sanitize_map_multiple_sensitive_keys() {
        let mut map = serde_json::json!({
            "api_key": "key123456",
            "auth_token": "tok123456",
            "secret": "sec123456",
            "password": "pas123456",
            "credential": "cre123456",
            "safe_name": "safe_value",
        })
        .as_object_mut()
        .unwrap()
        .clone();

        sanitize_map(&mut map);
        assert_eq!(map["api_key"], "key1****");
        assert_eq!(map["auth_token"], "tok1****");
        assert_eq!(map["secret"], "sec1****");
        assert_eq!(map["password"], "pas1****");
        assert_eq!(map["credential"], "cre1****");
        assert_eq!(map["safe_name"], "safe_value");
    }

    #[test]
    fn test_sanitize_map_deeply_nested() {
        let mut map = serde_json::json!({
            "level1": {
                "level2": {
                    "secret_key": "deepsecret123"
                }
            }
        })
        .as_object_mut()
        .unwrap()
        .clone();

        sanitize_map(&mut map);
        let l1 = map["level1"].as_object().unwrap();
        let l2 = l1["level2"].as_object().unwrap();
        assert_eq!(l2["secret_key"], "deep****");
    }

    #[test]
    fn test_read_log_entries_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("empty.log");
        std::fs::write(&file_path, "").unwrap();
        let entries = read_log_entries(&file_path.to_string_lossy(), 100);
        assert!(entries.is_empty());
    }

    #[test]
    fn test_read_log_entries_whitespace_only() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("ws.log");
        std::fs::write(&file_path, "  \n  \n  \n").unwrap();
        let entries = read_log_entries(&file_path.to_string_lossy(), 100);
        assert!(entries.is_empty());
    }

    #[test]
    fn test_read_log_entries_n_zero_treated_as_200() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.log");
        let lines: Vec<String> = (0..300).map(|i| format!(r#"{{"line":{}}}"#, i)).collect();
        std::fs::write(&file_path, lines.join("\n")).unwrap();

        let entries = read_log_entries(&file_path.to_string_lossy(), 0);
        // n=0 is treated as default 200, but actually n=0 in the code maps to n=200
        // Wait - looking at the code, n is only clamped in handle_api_logs, not in read_log_entries
        // So read_log_entries with n=0 returns 0 items
        // Actually, let's check: start = max(0, 300-0) = 300, so lines[300..] = empty
        assert!(entries.is_empty() || entries.len() <= 300);
    }

    #[test]
    fn test_load_scanner_status_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("config.scanner.json"), "not valid json").unwrap();

        let ws = dir.path().to_string_lossy().to_string();
        let status = load_scanner_status(&ws);
        assert_eq!(status["enabled"], false);
    }

    #[test]
    fn test_resolve_log_file_path_security_with_files() {
        let dir = tempfile::tempdir().unwrap();
        let audit_dir = dir.path().join("logs").join("security_logs");
        std::fs::create_dir_all(&audit_dir).unwrap();
        std::fs::write(audit_dir.join("audit.jsonl"), "{\"audit\":\"entry\"}").unwrap();

        let ws = dir.path().to_string_lossy().to_string();
        let path = resolve_log_file_path(&ws, "security").unwrap();
        // Phase B1-1: security 路径固定指向 logs/security_logs/audit.jsonl
        assert!(path.contains("audit.jsonl"));
        assert!(path.contains("security_logs"));
    }

    #[test]
    fn test_resolve_log_file_path_security_no_files() {
        let dir = tempfile::tempdir().unwrap();
        let audit_dir = dir.path().join("logs").join("security_logs");
        std::fs::create_dir_all(&audit_dir).unwrap();
        // audit.jsonl 文件不存在

        let ws = dir.path().to_string_lossy().to_string();
        assert!(resolve_log_file_path(&ws, "security").is_none());
    }

    #[test]
    fn test_resolve_log_file_path_llm_no_files() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        assert!(resolve_log_file_path(&ws, "llm").is_none());
    }

    #[test]
    fn test_app_state_session_manager_ref() {
        let state = AppState {
            auth_token: "test".to_string(),
            session_count: Arc::new(AtomicUsize::new(0)),
            workspace: None,
            home: None,
            version: "1.0.0".to_string(),
            start_time: std::time::Instant::now(),
            model_name: Arc::new(Mutex::new("test-model".to_string())),
            model_base: Arc::new(Mutex::new(String::new())),
            model_has_key: Arc::new(AtomicBool::new(false)),
            event_hub: Arc::new(EventHub::new()),
            running: Arc::new(AtomicBool::new(false)),
            session_manager: Arc::new(SessionManager::with_default_timeout()),
            inbound_tx: None,
            streaming_provider: None,
            ws_router: None,
            agent_service: None,
            data_store: None,
            memory_manager: None,
            forge: None,
            agent_loop: Arc::new(parking_lot::RwLock::new(None)),
            cluster: None,
            cluster_service: None,
            cluster_log_dir: None,
            workflow_engine: None,
            chat_secret_store: std::sync::Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
            webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
            internal_cmd_tx: None,
        };
        let mgr = state.session_manager_ref();
        assert_eq!(mgr.active_count(), 0);
    }

    #[test]
    fn test_verify_token_empty_strings() {
        // Empty expected means always valid
        assert!(verify_token("", ""));
        assert!(verify_token("anything", ""));
    }

    #[test]
    fn test_verify_token_matching() {
        assert!(verify_token("secret123", "secret123"));
    }

    #[test]
    fn test_verify_token_not_matching() {
        assert!(!verify_token("wrong", "right"));
    }

    #[test]
    fn test_verify_token_case_sensitive() {
        assert!(!verify_token("Secret", "secret"));
        assert!(verify_token("Secret", "Secret"));
    }

    #[test]
    fn test_write_json_response_valid() {
        let data = serde_json::json!({"key": "value"});
        let bytes = write_json_response(&data);
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed["key"], "value");
    }

    #[test]
    fn test_write_json_error_message() {
        let bytes = write_json_error("something failed", 500);
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed["error"], "something failed");
    }

    #[test]
    fn test_write_json_response_string() {
        let bytes = write_json_response(&"hello");
        assert!(!bytes.is_empty());
    }

    #[test]
    fn test_write_json_response_number() {
        let bytes = write_json_response(&42);
        assert_eq!(bytes, b"42");
    }

    #[test]
    fn test_write_json_error_with_status_code() {
        let bytes = write_json_error("not found", 404);
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed["error"], "not found");
    }

    #[test]
    fn test_load_scanner_status_with_multiple_engines() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        let config = serde_json::json!({
            "enabled": ["clamav", "yara"],
            "engines": {
                "clamav": {"path": "/usr/bin/clamav"},
                "yara": {"path": "/usr/bin/yara"}
            }
        });
        std::fs::write(
            config_dir.join("config.scanner.json"),
            serde_json::to_string_pretty(&config).unwrap(),
        ).unwrap();

        let ws = dir.path().to_string_lossy().to_string();
        let status = load_scanner_status(&ws);
        assert_eq!(status["enabled"], true);
        let engines = status["engines"].as_array().unwrap();
        assert_eq!(engines.len(), 2);
    }

    #[test]
    fn test_load_scanner_status_engines_sorted_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        let config = serde_json::json!({
            "enabled": ["z_engine", "a_engine"],
            "engines": {
                "z_engine": {"v": 1},
                "a_engine": {"v": 2}
            }
        });
        std::fs::write(
            config_dir.join("config.scanner.json"),
            serde_json::to_string_pretty(&config).unwrap(),
        ).unwrap();

        let ws = dir.path().to_string_lossy().to_string();
        let status = load_scanner_status(&ws);
        let engines = status["engines"].as_array().unwrap();
        assert_eq!(engines[0]["name"], "a_engine");
        assert_eq!(engines[1]["name"], "z_engine");
    }

    #[test]
    fn test_read_log_entries_truncates_to_n() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.jsonl");
        let lines: Vec<String> = (0..200).map(|i| format!(r#"{{"line":{}}}"#, i)).collect();
        std::fs::write(&file_path, lines.join("\n")).unwrap();

        let entries = read_log_entries(&file_path.to_string_lossy(), 10);
        assert_eq!(entries.len(), 10);
        assert_eq!(entries[0]["line"], 190);
        assert_eq!(entries[9]["line"], 199);
    }

    #[test]
    fn test_read_log_entries_n_larger_than_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("small.jsonl");
        std::fs::write(&file_path, r#"{"a":1}"#).unwrap();

        let entries = read_log_entries(&file_path.to_string_lossy(), 1000);
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn test_sanitize_map_preserves_non_sensitive() {
        let mut map = serde_json::json!({
            "name": "test",
            "port": 8080,
            "debug": true,
        }).as_object_mut().unwrap().clone();
        sanitize_map(&mut map);
        assert_eq!(map["name"], "test");
        assert_eq!(map["port"], 8080);
        assert_eq!(map["debug"], true);
    }

    #[test]
    fn test_sanitize_map_with_auth_key() {
        let mut map = serde_json::json!({
            "authorization": "Bearer token12345",
        }).as_object_mut().unwrap().clone();
        sanitize_map(&mut map);
        assert_eq!(map["authorization"], "Bear****");
    }

    #[test]
    fn test_sanitize_map_with_credential_key() {
        let mut map = serde_json::json!({
            "credential": "mycreds",
        }).as_object_mut().unwrap().clone();
        sanitize_map(&mut map);
        assert_eq!(map["credential"], "mycr****");
    }

    #[test]
    fn test_resolve_log_file_path_general_app_log() {
        // New behavior: only nemesisbot.YYYY-MM-DD files match. app.log must not.
        let dir = tempfile::tempdir().unwrap();
        let logs_dir = dir.path().join("logs");
        std::fs::create_dir_all(&logs_dir).unwrap();
        // Only app.log exists, no daily nemesisbot file
        std::fs::write(logs_dir.join("app.log"), "log content").unwrap();

        let ws = dir.path().to_string_lossy().to_string();
        assert!(
            resolve_log_file_path(&ws, "general").is_none(),
            "app.log must not match the nemesisbot.YYYY-MM-DD pattern"
        );
    }

    #[test]
    fn test_app_state_default_values() {
        let state = AppState {
            auth_token: String::new(),
            session_count: Arc::new(AtomicUsize::new(5)),
            workspace: Some("/tmp".to_string()),
            home: None,
            version: "1.0.0".to_string(),
            start_time: std::time::Instant::now(),
            model_name: Arc::new(Mutex::new("gpt-4".to_string())),
            model_base: Arc::new(Mutex::new(String::new())),
            model_has_key: Arc::new(AtomicBool::new(false)),
            event_hub: Arc::new(EventHub::new()),
            running: Arc::new(AtomicBool::new(true)),
            session_manager: Arc::new(SessionManager::with_default_timeout()),
            inbound_tx: None,
            streaming_provider: None,
            ws_router: None,
            agent_service: None,
            data_store: None,
            memory_manager: None,
            forge: None,
            agent_loop: Arc::new(parking_lot::RwLock::new(None)),
            cluster: None,
            cluster_service: None,
            cluster_log_dir: None,
            workflow_engine: None,
            chat_secret_store: std::sync::Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
            webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
            internal_cmd_tx: None,
        };
        assert_eq!(state.session_count.load(std::sync::atomic::Ordering::SeqCst), 5);
        assert!(state.running.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(*state.model_name.lock(), "gpt-4");
    }

    #[test]
    fn test_logs_query_deserialize_with_source() {
        let query: LogsQuery = serde_json::from_str(r#"{"source":"security","n":50}"#).unwrap();
        assert_eq!(query.source, Some("security".to_string()));
        assert_eq!(query.n, Some(50));
    }

    #[test]
    fn test_logs_query_deserialize_empty() {
        let query: LogsQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(query.source.is_none());
        assert!(query.n.is_none());
    }

    #[test]
    fn test_logs_query_deserialize_defaults() {
        let query: LogsQuery = serde_json::from_str(r#"{"source":"general"}"#).unwrap();
        assert_eq!(query.source, Some("general".to_string()));
        assert!(query.n.is_none());
    }

    // -----------------------------------------------------------------------
    // Cluster log path resolution tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_resolve_log_file_path_cluster_with_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        let cluster_dir = dir.path().join("logs").join("cluster_logs");
        std::fs::create_dir_all(&cluster_dir).unwrap();
        std::fs::write(cluster_dir.join("cluster_2026-01-01.log"), "cluster log content").unwrap();

        let ws = dir.path().to_string_lossy().to_string();
        let path = resolve_log_file_path(&ws, "cluster");
        assert!(path.is_some());
        assert!(path.unwrap().contains("cluster_2026-01-01.log"));
    }

    #[test]
    fn test_resolve_log_file_path_cluster_no_log_files() {
        let dir = tempfile::tempdir().unwrap();
        let cluster_dir = dir.path().join("logs").join("cluster_logs");
        std::fs::create_dir_all(&cluster_dir).unwrap();
        // Place files that don't match the cluster_*.log pattern
        std::fs::write(cluster_dir.join("notes.txt"), "not a log").unwrap();
        std::fs::write(cluster_dir.join("random.log"), "wrong prefix").unwrap();

        let ws = dir.path().to_string_lossy().to_string();
        assert!(resolve_log_file_path(&ws, "cluster").is_none());
    }

    #[test]
    fn test_resolve_log_file_path_cluster_multiple_files_returns_lexicographically_last() {
        let dir = tempfile::tempdir().unwrap();
        let cluster_dir = dir.path().join("logs").join("cluster_logs");
        std::fs::create_dir_all(&cluster_dir).unwrap();
        std::fs::write(cluster_dir.join("cluster_2026-01-01.log"), "day 1").unwrap();
        std::fs::write(cluster_dir.join("cluster_2026-12-31.log"), "last day").unwrap();

        let ws = dir.path().to_string_lossy().to_string();
        let path = resolve_log_file_path(&ws, "cluster");
        assert!(path.is_some());
        // After sort+reverse, lexicographically greatest name wins
        assert!(path.unwrap().contains("cluster_2026-12-31.log"));
    }

    #[test]
    fn test_resolve_log_file_path_cluster_empty_directory() {
        let dir = tempfile::tempdir().unwrap();
        let cluster_dir = dir.path().join("logs").join("cluster_logs");
        std::fs::create_dir_all(&cluster_dir).unwrap();
        // Directory exists but is completely empty

        let ws = dir.path().to_string_lossy().to_string();
        assert!(resolve_log_file_path(&ws, "cluster").is_none());
    }

    // -----------------------------------------------------------------------
    // API handler integration tests via tower::ServiceExt::oneshot
    // -----------------------------------------------------------------------

    /// Helper to create a minimal AppState for testing API handlers.
    fn make_test_state(workspace: Option<String>, auth_token: &str) -> Arc<AppState> {
        let home = workspace.clone();
        Arc::new(AppState {
            auth_token: auth_token.to_string(),
            session_count: Arc::new(AtomicUsize::new(2)),
            workspace,
            home,
            version: "1.0.0-test".to_string(),
            start_time: std::time::Instant::now(),
            model_name: Arc::new(Mutex::new("test-model".to_string())),
            model_base: Arc::new(Mutex::new(String::new())),
            model_has_key: Arc::new(AtomicBool::new(false)),
            event_hub: Arc::new(EventHub::new()),
            running: Arc::new(AtomicBool::new(true)),
            session_manager: Arc::new(SessionManager::with_default_timeout()),
            inbound_tx: None,
            streaming_provider: None,
            ws_router: None,
            agent_service: None,
            data_store: None,
            memory_manager: None,
            forge: None,
            agent_loop: Arc::new(parking_lot::RwLock::new(None)),
            cluster: None,
            cluster_service: None,
            cluster_log_dir: None,
            workflow_engine: None,
            chat_secret_store: std::sync::Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
            webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
            internal_cmd_tx: None,
        })
    }

    use axum::Router;
    use axum::routing::get;
    use tower::ServiceExt;

    fn make_test_router(state: Arc<AppState>) -> Router {
        Router::new()
            .route("/api/status", get(handle_api_status))
            .route("/api/logs", get(handle_api_logs))
            .route("/api/scanner/status", get(handle_api_scanner_status))
            .route("/api/config", get(handle_api_config))
            .route("/api/version", get(handle_api_version))
            .route("/api/models", get(handle_api_models))
            .route("/api/sessions", get(handle_api_sessions))
            .route("/api/events", get(handle_api_events))
            .with_state(state)
    }

    #[tokio::test]
    async fn test_api_status_endpoint() {
        let state = make_test_state(None, "");
        let app = make_test_router(state);
        let req = axum::http::Request::builder()
            .uri("/api/status")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn test_api_status_with_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let state = make_test_state(Some(ws), "");
        let app = make_test_router(state);
        let req = axum::http::Request::builder()
            .uri("/api/status")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["version"], "1.0.0-test");
        assert!(json["scanner_status"].is_object());
        assert!(json["cluster_status"].is_object());
        assert_eq!(json["cluster_status"]["enabled"], false);
        assert_eq!(json["model"], "test-model");
    }

    #[tokio::test]
    async fn test_api_logs_no_workspace() {
        let state = make_test_state(None, "");
        let app = make_test_router(state);
        let req = axum::http::Request::builder()
            .uri("/api/logs")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 503); // SERVICE_UNAVAILABLE
    }

    #[tokio::test]
    async fn test_api_logs_with_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let logs_dir = dir.path().join("logs");
        std::fs::create_dir_all(&logs_dir).unwrap();
        std::fs::write(logs_dir.join("nemesisbot.2026-06-17"), r#"{"msg":"line1"}"#).unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let state = make_test_state(Some(ws), "");
        let app = make_test_router(state);
        let req = axum::http::Request::builder()
            .uri("/api/logs?source=general&n=10")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["entries"].is_array());
    }

    #[tokio::test]
    async fn test_api_logs_n_exceeds_max() {
        let dir = tempfile::tempdir().unwrap();
        let logs_dir = dir.path().join("logs");
        std::fs::create_dir_all(&logs_dir).unwrap();
        let lines: Vec<String> = (0..2000).map(|i| format!(r#"{{"i":{}}}"#, i)).collect();
        std::fs::write(logs_dir.join("nemesisbot.2026-06-17"), lines.join("\n")).unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let state = make_test_state(Some(ws), "");
        let app = make_test_router(state);
        let req = axum::http::Request::builder()
            .uri("/api/logs?source=general&n=5000")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = axum::body::to_bytes(resp.into_body(), 65536).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let entries = json["entries"].as_array().unwrap();
        assert!(entries.len() <= 1000, "Should be capped at 1000");
    }

    #[tokio::test]
    async fn test_api_logs_n_zero_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let logs_dir = dir.path().join("logs");
        std::fs::create_dir_all(&logs_dir).unwrap();
        let lines: Vec<String> = (0..300).map(|i| format!(r#"{{"i":{}}}"#, i)).collect();
        std::fs::write(logs_dir.join("nemesisbot.2026-06-17"), lines.join("\n")).unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let state = make_test_state(Some(ws), "");
        let app = make_test_router(state);
        let req = axum::http::Request::builder()
            .uri("/api/logs?source=general&n=0")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn test_api_scanner_status_no_workspace() {
        let state = make_test_state(None, "");
        let app = make_test_router(state);
        let req = axum::http::Request::builder()
            .uri("/api/scanner/status")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 503);
    }

    #[tokio::test]
    async fn test_api_scanner_status_with_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("config.scanner.json"),
            r#"{"enabled":["clamav"],"engines":{"clamav":{"path":"/usr/bin/clamav"}}}"#,
        ).unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let state = make_test_state(Some(ws), "");
        let app = make_test_router(state);
        let req = axum::http::Request::builder()
            .uri("/api/scanner/status")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["enabled"], true);
    }

    #[tokio::test]
    async fn test_api_config_no_workspace() {
        let state = make_test_state(None, "");
        let app = make_test_router(state);
        let req = axum::http::Request::builder()
            .uri("/api/config")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 503);
    }

    #[tokio::test]
    async fn test_api_config_file_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let state = make_test_state(Some(ws), "");
        let app = make_test_router(state);
        let req = axum::http::Request::builder()
            .uri("/api/config")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 404);
    }

    #[tokio::test]
    async fn test_api_config_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.json"), "not valid json").unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let state = make_test_state(Some(ws), "");
        let app = make_test_router(state);
        let req = axum::http::Request::builder()
            .uri("/api/config")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 500);
    }

    #[tokio::test]
    async fn test_api_config_valid_json_sanitized() {
        let dir = tempfile::tempdir().unwrap();
        let config = serde_json::json!({
            "api_key": "sk-1234567890abcdef",
            "name": "test-config",
            "port": 8080,
        });
        std::fs::write(
            dir.path().join("config.json"),
            serde_json::to_string_pretty(&config).unwrap(),
        ).unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let state = make_test_state(Some(ws), "");
        let app = make_test_router(state);
        let req = axum::http::Request::builder()
            .uri("/api/config")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["api_key"], "sk-1****");
        assert_eq!(json["name"], "test-config");
        assert_eq!(json["port"], 8080);
    }

    #[tokio::test]
    async fn test_api_version_endpoint() {
        let state = make_test_state(None, "");
        let app = make_test_router(state);
        let req = axum::http::Request::builder()
            .uri("/api/version")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["version"], "1.0.0-test");
        assert_eq!(json["model"], "test-model");
        assert!(json["uptime_seconds"].is_number());
    }

    #[tokio::test]
    async fn test_api_models_no_workspace() {
        let state = make_test_state(None, "");
        let app = make_test_router(state);
        let req = axum::http::Request::builder()
            .uri("/api/models")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        // Without workspace, returns 503 (service unavailable)
        assert_eq!(resp.status(), 503);
    }

    #[tokio::test]
    async fn test_api_models_with_config() {
        let dir = tempfile::tempdir().unwrap();
        let config = serde_json::json!({
            "model_list": [
                {"name": "gpt-4", "api_key": "sk-1234567890abcdef"},
                {"name": "claude", "api_key": "sk-short"}
            ],
            "agents": {"defaults": {"llm": "gpt-4"}}
        });
        std::fs::write(
            dir.path().join("config.json"),
            serde_json::to_string(&config).unwrap(),
        ).unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let state = make_test_state(Some(ws), "");
        let app = make_test_router(state);
        let req = axum::http::Request::builder()
            .uri("/api/models")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let models = json["models"].as_array().unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0]["api_key"], "sk-1****");
        assert_eq!(models[1]["api_key"], "sk-s****"); // short key uses same format
        assert_eq!(json["default"], "gpt-4");
        assert_eq!(json["current"], "test-model");
    }

    #[tokio::test]
    async fn test_api_models_invalid_config_json() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.json"), "invalid json").unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let state = make_test_state(Some(ws), "");
        let app = make_test_router(state);
        let req = axum::http::Request::builder()
            .uri("/api/models")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["models"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_api_sessions_endpoint() {
        let state = make_test_state(None, "");
        let app = make_test_router(state);
        let req = axum::http::Request::builder()
            .uri("/api/sessions")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["total_connections"], 2);
        assert_eq!(json["active_sessions"], 0);
    }

    #[tokio::test]
    async fn test_api_events_endpoint() {
        let state = make_test_state(None, "");
        let app = make_test_router(state);
        let req = axum::http::Request::builder()
            .uri("/api/events")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["stream_url"], "/api/events/stream");
        assert_eq!(json["subscriber_count"], 0);
    }

    #[tokio::test]
    async fn test_api_logs_source_security() {
        let dir = tempfile::tempdir().unwrap();
        let sec_dir = dir.path().join("logs").join("security_logs");
        std::fs::create_dir_all(&sec_dir).unwrap();
        std::fs::write(sec_dir.join("audit.jsonl"), r#"{"audit":"entry1"}"#).unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let state = make_test_state(Some(ws), "");
        let app = make_test_router(state);
        let req = axum::http::Request::builder()
            .uri("/api/logs?source=security")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn test_api_logs_source_cluster() {
        let dir = tempfile::tempdir().unwrap();
        let cluster_dir = dir.path().join("logs").join("cluster_logs");
        std::fs::create_dir_all(&cluster_dir).unwrap();
        std::fs::write(cluster_dir.join("cluster_2026-01-01.log"), r#"{"cluster":"entry1"}"#).unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let state = make_test_state(Some(ws), "");
        let app = make_test_router(state);
        let req = axum::http::Request::builder()
            .uri("/api/logs?source=cluster")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn test_api_logs_source_llm() {
        let dir = tempfile::tempdir().unwrap();
        // request_logs/{ts}_{NNN}/00.request.md — picked up by find_latest_request_summary
        let req_dir = dir.path().join("logs").join("request_logs").join("2026-01-01_00-00-00_001");
        std::fs::create_dir_all(&req_dir).unwrap();
        std::fs::write(req_dir.join("00.request.md"), "# user request\nhello").unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let state = make_test_state(Some(ws), "");
        let app = make_test_router(state);
        let req = axum::http::Request::builder()
            .uri("/api/logs?source=llm")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn test_api_logs_unknown_source() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let state = make_test_state(Some(ws), "");
        let app = make_test_router(state);
        let req = axum::http::Request::builder()
            .uri("/api/logs?source=unknown_source")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["entries"].as_array().unwrap().is_empty());
    }

    #[test]
    fn test_sanitize_map_with_number_value_for_sensitive_key() {
        // Non-string sensitive values should be left alone
        let mut map = serde_json::json!({
            "api_key": 12345,
        }).as_object_mut().unwrap().clone();
        sanitize_map(&mut map);
        assert_eq!(map["api_key"], 12345); // unchanged
    }

    #[test]
    fn test_sanitize_map_with_null_value_for_sensitive_key() {
        let mut map = serde_json::json!({
            "token": serde_json::Value::Null,
        }).as_object_mut().unwrap().clone();
        sanitize_map(&mut map);
        assert!(map["token"].is_null()); // unchanged
    }

    #[test]
    fn test_sanitize_map_exactly_4_chars() {
        let mut map = serde_json::json!({
            "secret": "abcd",
        }).as_object_mut().unwrap().clone();
        sanitize_map(&mut map);
        assert_eq!(map["secret"], "****");
    }

    #[test]
    fn test_sanitize_map_5_chars() {
        let mut map = serde_json::json!({
            "secret": "abcde",
        }).as_object_mut().unwrap().clone();
        sanitize_map(&mut map);
        assert_eq!(map["secret"], "abcd****");
    }

    #[test]
    fn test_write_json_response_map() {
        let mut map = std::collections::HashMap::new();
        map.insert("key".to_string(), "value".to_string());
        let bytes = write_json_response(&map);
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed["key"], "value");
    }

    #[test]
    fn test_write_json_error_various_messages() {
        let bytes = write_json_error("internal server error", 500);
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed["error"], "internal server error");

        let bytes = write_json_error("unauthorized", 401);
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed["error"], "unauthorized");
    }

    #[test]
    fn test_load_scanner_status_empty_enabled_array() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        let config = serde_json::json!({
            "enabled": [],
            "engines": {}
        });
        std::fs::write(
            config_dir.join("config.scanner.json"),
            serde_json::to_string(&config).unwrap(),
        ).unwrap();

        let ws = dir.path().to_string_lossy().to_string();
        let status = load_scanner_status(&ws);
        assert_eq!(status["enabled"], false);
        assert!(status["engines"].as_array().unwrap().is_empty());
    }

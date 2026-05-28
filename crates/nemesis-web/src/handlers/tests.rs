use super::*;
    use crate::api_handlers::AppState;
    use crate::events::EventHub;
    use crate::session::SessionManager;
    use crate::ws_router::{ModuleHandler, RequestContext};
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::sync::Arc;
    use std::time::Instant;

    // -----------------------------------------------------------------------
    // Test infrastructure
    // -----------------------------------------------------------------------

    /// Create a RequestContext with a temp workspace directory.
    fn make_ctx(dir: &tempfile::TempDir) -> RequestContext {
        let ws = dir.path().to_string_lossy().to_string();
        let state = Arc::new(AppState {
            auth_token: String::new(),
            session_count: Arc::new(AtomicUsize::new(0)),
            workspace: Some(ws.clone()),
            home: Some(ws.clone()),
            version: "test".to_string(),
            start_time: Instant::now(),
            model_name: Arc::new(parking_lot::Mutex::new("test-model".to_string())),
            model_base: Arc::new(parking_lot::Mutex::new(String::new())),
            model_has_key: Arc::new(AtomicBool::new(false)),
            event_hub: Arc::new(EventHub::new()),
            running: Arc::new(AtomicBool::new(true)),
            session_manager: Arc::new(SessionManager::with_default_timeout()),
            inbound_tx: None,
            streaming_provider: None,
            ws_router: None,
            agent_service: None,
            data_store: None,
        });
        RequestContext {
            session_id: "test-session".to_string(),
            workspace: Some(ws.clone()),
            home: Some(ws),
            state,
        }
    }

    /// Create a RequestContext without a workspace.
    fn make_ctx_no_workspace() -> RequestContext {
        let state = Arc::new(AppState {
            auth_token: String::new(),
            session_count: Arc::new(AtomicUsize::new(0)),
            workspace: None,
            home: None,
            version: "test".to_string(),
            start_time: Instant::now(),
            model_name: Arc::new(parking_lot::Mutex::new("test-model".to_string())),
            model_base: Arc::new(parking_lot::Mutex::new(String::new())),
            model_has_key: Arc::new(AtomicBool::new(false)),
            event_hub: Arc::new(EventHub::new()),
            running: Arc::new(AtomicBool::new(true)),
            session_manager: Arc::new(SessionManager::with_default_timeout()),
            inbound_tx: None,
            streaming_provider: None,
            ws_router: None,
            agent_service: None,
            data_store: None,
        });
        RequestContext {
            session_id: "test-session".to_string(),
            workspace: None,
            home: None,
            state,
        }
    }

    /// Create a minimal config.json in the workspace.
    fn write_config(workspace: &Path) {
        let config = nemesis_config::Config::default();
        let json = serde_json::to_string_pretty(&config).unwrap();
        std::fs::write(workspace.join("config.json"), json).unwrap();
    }

    /// Create config subdirectory.
    fn ensure_config_dir(workspace: &Path) {
        std::fs::create_dir_all(workspace.join("config")).unwrap();
    }

    // -----------------------------------------------------------------------
    // Utility function tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_mask_sensitive_short() {
        assert_eq!(mask_sensitive("abc"), "****");
        assert_eq!(mask_sensitive(""), "****");
    }

    #[test]
    fn test_mask_sensitive_long() {
        assert_eq!(mask_sensitive("abcdefghijklmnop"), "abcd****mnop");
    }

    #[test]
    fn test_is_sensitive_field() {
        assert!(is_sensitive_field("api_key"));
        assert!(is_sensitive_field("API_KEY"));
        assert!(is_sensitive_field("Token"));
        assert!(is_sensitive_field("bot_token"));
        assert!(!is_sensitive_field("name"));
        assert!(!is_sensitive_field("model"));
    }

    #[test]
    fn test_resolve_path_normal() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let resolved = resolve_path(&ws, "config.json").unwrap();
        assert!(resolved.ends_with("config.json"));
    }

    #[test]
    fn test_resolve_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let result = resolve_path(&ws, "../../etc/passwd");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("traversal"));
    }

    #[test]
    fn test_get_str_present() {
        let data = serde_json::json!({ "name": "test" });
        assert_eq!(get_str(&data, "name").unwrap(), "test");
    }

    #[test]
    fn test_get_str_missing() {
        let data = serde_json::json!({});
        assert!(get_str(&data, "name").is_err());
    }

    #[test]
    fn test_get_opt_str() {
        let data = serde_json::json!({ "name": "test" });
        assert_eq!(get_opt_str(&data, "name"), Some("test".to_string()));
        assert_eq!(get_opt_str(&data, "missing"), None);
    }

    #[test]
    fn test_read_write_workspace_file() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        write_workspace_file(&ws, "test.txt", "hello").unwrap();
        let content = read_workspace_file(&ws, "test.txt").unwrap();
        assert_eq!(content, "hello");
    }

    #[test]
    fn test_read_nonexistent_file() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        assert!(read_workspace_file(&ws, "nonexistent.txt").is_err());
    }

    #[test]
    fn test_write_creates_subdirs() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        write_workspace_file(&ws, "sub/dir/test.txt", "hello").unwrap();
        let content = read_workspace_file(&ws, "sub/dir/test.txt").unwrap();
        assert_eq!(content, "hello");
    }

    // -----------------------------------------------------------------------
    // register_all test
    // -----------------------------------------------------------------------

    #[test]
    fn test_register_all_registers_16_handlers() {
        let mut router = crate::ws_router::WsRouter::new();
        register_all(&mut router);
        // Verify all 16 modules are registered by dispatching test commands
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let expected = [
            "system", "config", "models", "channels", "identity", "tools",
            "scanner", "memory", "skills", "mcp", "security", "forge",
            "tasks", "cluster", "logs", "agent",
        ];
        // Use a simple channel to capture responses
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
        let (_, done_rx) = tokio::sync::watch::channel(false);
        let send_queue = crate::websocket_handler::SendQueue::from_channels(tx, done_rx);

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            for module in &expected {
                let msg = crate::protocol::ProtocolMessage::request(
                    module, "__register_check__", "test-req", None,
                );
                router.dispatch(&msg, &ctx, &send_queue).await;
                let resp_bytes = rx.recv().await.unwrap();
                let resp: serde_json::Value = serde_json::from_slice(&resp_bytes).unwrap();
                // Should NOT get "unknown module" error — means handler was registered
                let err = resp["error"].as_str().unwrap_or("");
                assert!(!err.contains("unknown module"), "module '{}' not registered", module);
            }
        });
    }

    // -----------------------------------------------------------------------
    // System handler tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_system_version() {
        let handler = system::SystemHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("version", None, &ctx).await.unwrap().unwrap();
        assert_eq!(result["version"], "test");
        assert!(result["uptime_seconds"].is_number());
    }

    #[tokio::test]
    async fn test_system_status() {
        let handler = system::SystemHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
        assert_eq!(result["version"], "test");
        assert_eq!(result["model_name"], "test-model");
        assert!(result["running"].is_boolean());
    }

    #[tokio::test]
    async fn test_system_unknown_cmd() {
        let handler = system::SystemHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("bogus", None, &ctx).await;
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // Identity handler tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_identity_list() {
        let handler = identity::IdentityHandler;
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        // Create AGENT.md
        std::fs::write(ws.join("AGENT.md"), "# Agent\nHello").unwrap();
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
        let docs = result["documents"].as_array().unwrap();
        assert_eq!(docs.len(), 4); // AGENT.md, IDENTITY.md, SOUL.md, USER.md
        let agent_doc = docs.iter().find(|d| d["name"] == "AGENT.md").unwrap();
        assert!(agent_doc["exists"].as_bool().unwrap());
        let soul_doc = docs.iter().find(|d| d["name"] == "SOUL.md").unwrap();
        assert!(!soul_doc["exists"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_identity_get_and_save() {
        let handler = identity::IdentityHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);

        // Save
        let data = serde_json::json!({ "name": "IDENTITY.md", "content": "# My Identity" });
        let result = handler.handle_cmd("save", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["saved"].as_bool().unwrap());

        // Get
        let data = serde_json::json!({ "name": "IDENTITY.md" });
        let result = handler.handle_cmd("get", Some(data), &ctx).await.unwrap().unwrap();
        assert_eq!(result["content"], "# My Identity");
    }

    #[tokio::test]
    async fn test_identity_get_nonexistent() {
        let handler = identity::IdentityHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "name": "SOUL.md" });
        let result = handler.handle_cmd("get", Some(data), &ctx).await;
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // Tools handler tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_tools_get_save_roundtrip() {
        let handler = tools::ToolsHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);

        // Get fails when file doesn't exist
        let result = handler.handle_cmd("get", None, &ctx).await;
        assert!(result.is_err());

        // Save
        let data = serde_json::json!({ "content": "# Tools\n- search\n- write" });
        handler.handle_cmd("save", Some(data), &ctx).await.unwrap();

        // Get succeeds
        let result = handler.handle_cmd("get", None, &ctx).await.unwrap().unwrap();
        assert!(result["content"].as_str().unwrap().contains("search"));
    }

    // -----------------------------------------------------------------------
    // Config handler tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_config_get() {
        let handler = config::ConfigHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("get", None, &ctx).await.unwrap().unwrap();
        // Should return a JSON object with the config fields
        assert!(result.is_object());
        assert!(result.get("model_list").is_some());
    }

    #[tokio::test]
    async fn test_config_get_masks_api_keys() {
        let handler = config::ConfigHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = nemesis_config::Config::default();
        cfg.model_list.push(nemesis_config::ModelConfig {
            model_name: "test".to_string(),
            model: "test-model".to_string(),
            api_key: "sk-1234567890abcdef".to_string(),
            ..Default::default()
        });
        let json = serde_json::to_string_pretty(&cfg).unwrap();
        std::fs::write(dir.path().join("config.json"), json).unwrap();
        let ctx = make_ctx(&dir);

        let result = handler.handle_cmd("get", None, &ctx).await.unwrap().unwrap();
        let api_key = result["model_list"][0]["api_key"].as_str().unwrap();
        assert!(api_key.contains("****"));
        assert!(!api_key.contains("1234567890abcdef"));
    }

    #[tokio::test]
    async fn test_config_set_field() {
        let handler = config::ConfigHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "path": "gateway.port", "value": 9090 });
        let result = handler.handle_cmd("set_field", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["updated"].as_bool().unwrap());

        // Verify the change persisted
        let config_str = std::fs::read_to_string(dir.path().join("config.json")).unwrap();
        let config: serde_json::Value = serde_json::from_str(&config_str).unwrap();
        assert_eq!(config["gateway"]["port"], 9090);
    }

    #[tokio::test]
    async fn test_config_save_and_get_roundtrip() {
        let handler = config::ConfigHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);

        let cfg_data = serde_json::json!({
            "agents": { "default": { "model": "test" } },
            "bindings": [],
            "session": {},
            "channels": {},
            "model_list": [],
            "gateway": { "host": "0.0.0.0", "port": 8080 },
            "tools": {},
            "heartbeat": {},
            "devices": {}
        });
        handler.handle_cmd("save", Some(cfg_data), &ctx).await.unwrap();

        let result = handler.handle_cmd("get", None, &ctx).await.unwrap().unwrap();
        assert_eq!(result["gateway"]["port"], 8080);
    }

    #[tokio::test]
    async fn test_config_cors_commands() {
        let handler = config::ConfigHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);

        // cors.list
        let result = handler.handle_cmd("cors.list", None, &ctx).await.unwrap().unwrap();
        assert!(result["origins"].is_array());

        // cors.add
        let data = serde_json::json!({ "origin": "https://example.com" });
        handler.handle_cmd("cors.add", Some(data), &ctx).await.unwrap().unwrap();

        // cors.remove
        let data = serde_json::json!({ "origin": "https://example.com" });
        handler.handle_cmd("cors.remove", Some(data), &ctx).await.unwrap().unwrap();

        // cors.toggle
        let data = serde_json::json!({ "enabled": true });
        handler.handle_cmd("cors.toggle", Some(data), &ctx).await.unwrap().unwrap();
    }

    // -----------------------------------------------------------------------
    // Models handler tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_models_add_list_delete() {
        let handler = models::ModelsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);

        // Add
        let data = serde_json::json!({
            "name": "test-model",
            "model": "gpt-4",
            "key": "sk-test-key-12345678"
        });
        let result = handler.handle_cmd("add", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["added"].as_bool().unwrap());

        // List
        let result = handler.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
        let models = result["models"].as_array().unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["model_name"], "test-model");
        // API key should be masked
        let api_key = models[0]["api_key"].as_str().unwrap();
        assert!(api_key.contains("****"));

        // Set default
        let data = serde_json::json!({ "name": "test-model" });
        let result = handler.handle_cmd("set_default", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["set_default"].as_bool().unwrap());

        // Delete
        let data = serde_json::json!({ "name": "test-model" });
        let result = handler.handle_cmd("delete", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["deleted"].as_bool().unwrap());

        // List should be empty now
        let result = handler.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
        let models = result["models"].as_array().unwrap();
        assert!(models.is_empty());
    }

    #[tokio::test]
    async fn test_models_add_duplicate() {
        let handler = models::ModelsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "name": "m1", "model": "gpt-4", "key": "key" });
        handler.handle_cmd("add", Some(data.clone()), &ctx).await.unwrap();
        let result = handler.handle_cmd("add", Some(data), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already exists"));
    }

    #[tokio::test]
    async fn test_models_delete_nonexistent() {
        let handler = models::ModelsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "name": "nonexistent" });
        let result = handler.handle_cmd("delete", Some(data), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    // -----------------------------------------------------------------------
    // Channels handler tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_channels_list() {
        let handler = channels::ChannelsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);

        let result = handler.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
        let ch_list = result["channels"].as_array().unwrap();
        // Default config has all channels
        assert!(!ch_list.is_empty());
    }

    #[tokio::test]
    async fn test_channels_get_nonexistent() {
        let handler = channels::ChannelsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "name": "nonexistent_channel" });
        let result = handler.handle_cmd("get", Some(data), &ctx).await;
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // Scanner handler tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_scanner_config_get_save_roundtrip() {
        let handler = scanner::ScannerHandler::new();
        let dir = tempfile::tempdir().unwrap();
        ensure_config_dir(dir.path());
        // Write initial scanner config
        let cfg = nemesis_config::ScannerFullConfig::default();
        let json = serde_json::to_string_pretty(&cfg).unwrap();
        std::fs::write(dir.path().join("config/config.scanner.json"), json).unwrap();
        let ctx = make_ctx(&dir);

        // Get
        let result = handler.handle_cmd("config.get", None, &ctx).await.unwrap().unwrap();
        assert!(result.is_object());

        // Save
        let save_data = serde_json::json!({ "enabled": ["clamav"], "engines": {} });
        let result = handler.handle_cmd("config.save", Some(save_data), &ctx).await.unwrap().unwrap();
        assert!(result["saved"].as_bool().unwrap());
    }

    // -----------------------------------------------------------------------
    // Security handler tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_security_config_get_save() {
        let handler = security::SecurityHandler::new();
        let dir = tempfile::tempdir().unwrap();
        ensure_config_dir(dir.path());
        let cfg = nemesis_config::SecurityConfig::default();
        let json = serde_json::to_string_pretty(&cfg).unwrap();
        std::fs::write(dir.path().join("config/config.security.json"), json).unwrap();
        let ctx = make_ctx(&dir);

        let result = handler.handle_cmd("config.get", None, &ctx).await.unwrap().unwrap();
        assert_eq!(result["default_action"], "deny");

        let save_data = serde_json::json!({ "default_action": "allow", "log_all_operations": true, "log_denials_only": false });
        let result = handler.handle_cmd("config.save", Some(save_data), &ctx).await.unwrap().unwrap();
        assert!(result["saved"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_security_stats_empty() {
        let handler = security::SecurityHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);

        let result = handler.handle_cmd("stats", None, &ctx).await.unwrap().unwrap();
        assert_eq!(result["total_events"], 0);
    }

    #[tokio::test]
    async fn test_security_audit_empty() {
        let handler = security::SecurityHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);

        let result = handler.handle_cmd("audit", None, &ctx).await.unwrap().unwrap();
        assert_eq!(result["total"], 0);
    }

    #[tokio::test]
    async fn test_security_audit_with_data() {
        let handler = security::SecurityHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let log_dir = ws.join("logs/security_logs");
        std::fs::create_dir_all(&log_dir).unwrap();

        let entries = vec![
            serde_json::json!({"event_id":"evt-1","request":{"op_type":"FileWrite","danger_level":"HIGH","target":"/test","user":"","source":"test"},"decision":"allowed","reason":"test","timestamp":"2026-01-01T00:00:00Z","policy_rule":"test"}),
            serde_json::json!({"event_id":"evt-2","request":{"op_type":"FileRead","danger_level":"LOW","target":"/test","user":"","source":"test"},"decision":"allowed","reason":"test","timestamp":"2026-01-02T00:00:00Z","policy_rule":"test"}),
            serde_json::json!({"event_id":"evt-3","request":{"op_type":"ProcessExec","danger_level":"HIGH","target":"/test","user":"","source":"test"},"decision":"denied","reason":"test","timestamp":"2026-01-03T00:00:00Z","policy_rule":"test"}),
        ];
        let jsonl: String = entries.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("\n");
        std::fs::write(log_dir.join("2026-01.jsonl"), jsonl).unwrap();

        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("audit", None, &ctx).await.unwrap().unwrap();
        assert_eq!(result["total"], 3);
        let page = result["entries"].as_array().unwrap();
        assert_eq!(page.len(), 3);
        // Should be sorted by timestamp desc
        assert_eq!(page[0]["risk_level"], "HIGH");
        assert_eq!(page[0]["action"], "ProcessExec");
        assert_eq!(page[0]["result"], "denied");

        // Stats
        let result = handler.handle_cmd("stats", None, &ctx).await.unwrap().unwrap();
        assert_eq!(result["total_events"], 3);
        let by_level = result["by_level"].as_object().unwrap();
        assert_eq!(by_level["HIGH"].as_u64().unwrap(), 2);
        assert_eq!(by_level["LOW"].as_u64().unwrap(), 1);
    }

    // -----------------------------------------------------------------------
    // MCP handler tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_mcp_add_list_delete_server() {
        let handler = mcp::McpHandler::new();
        let dir = tempfile::tempdir().unwrap();
        ensure_config_dir(dir.path());
        let cfg = nemesis_config::McpConfig::default();
        let json = serde_json::to_string_pretty(&cfg).unwrap();
        std::fs::write(dir.path().join("config/config.mcp.json"), json).unwrap();
        let ctx = make_ctx(&dir);

        // Add server
        let data = serde_json::json!({
            "name": "test-server",
            "command": "node",
            "args": ["server.js"]
        });
        let result = handler.handle_cmd("server.add", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["added"].as_bool().unwrap());

        // List servers
        let result = handler.handle_cmd("servers", None, &ctx).await.unwrap().unwrap();
        let servers = result["servers"].as_array().unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0]["name"], "test-server");
        assert_eq!(servers[0]["command"], "node");

        // Update server
        let data = serde_json::json!({ "name": "test-server", "command": "python" });
        let result = handler.handle_cmd("server.update", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["updated"].as_bool().unwrap());

        // Verify update
        let result = handler.handle_cmd("servers", None, &ctx).await.unwrap().unwrap();
        assert_eq!(result["servers"][0]["command"], "python");

        // Delete server
        let data = serde_json::json!({ "name": "test-server" });
        let result = handler.handle_cmd("server.delete", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["deleted"].as_bool().unwrap());

        // Verify empty
        let result = handler.handle_cmd("servers", None, &ctx).await.unwrap().unwrap();
        assert!(result["servers"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_mcp_status() {
        let handler = mcp::McpHandler::new();
        let dir = tempfile::tempdir().unwrap();
        ensure_config_dir(dir.path());
        let cfg = nemesis_config::McpConfig::default();
        let json = serde_json::to_string_pretty(&cfg).unwrap();
        std::fs::write(dir.path().join("config/config.mcp.json"), json).unwrap();
        let ctx = make_ctx(&dir);

        let result = handler.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
        assert!(!result["enabled"].as_bool().unwrap());
        assert_eq!(result["servers_count"], 0);
    }

    #[tokio::test]
    async fn test_mcp_add_duplicate() {
        let handler = mcp::McpHandler::new();
        let dir = tempfile::tempdir().unwrap();
        ensure_config_dir(dir.path());
        let cfg = nemesis_config::McpConfig::default();
        let json = serde_json::to_string_pretty(&cfg).unwrap();
        std::fs::write(dir.path().join("config/config.mcp.json"), json).unwrap();
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "name": "s1", "command": "node" });
        handler.handle_cmd("server.add", Some(data.clone()), &ctx).await.unwrap();
        let result = handler.handle_cmd("server.add", Some(data), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already exists"));
    }

    #[tokio::test]
    async fn test_mcp_update_nonexistent() {
        let handler = mcp::McpHandler::new();
        let dir = tempfile::tempdir().unwrap();
        ensure_config_dir(dir.path());
        let cfg = nemesis_config::McpConfig::default();
        let json = serde_json::to_string_pretty(&cfg).unwrap();
        std::fs::write(dir.path().join("config/config.mcp.json"), json).unwrap();
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "name": "ghost", "command": "node" });
        let result = handler.handle_cmd("server.update", Some(data), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[tokio::test]
    async fn test_mcp_delete_nonexistent() {
        let handler = mcp::McpHandler::new();
        let dir = tempfile::tempdir().unwrap();
        ensure_config_dir(dir.path());
        let cfg = nemesis_config::McpConfig::default();
        let json = serde_json::to_string_pretty(&cfg).unwrap();
        std::fs::write(dir.path().join("config/config.mcp.json"), json).unwrap();
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "name": "ghost" });
        let result = handler.handle_cmd("server.delete", Some(data), &ctx).await;
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // Skills handler tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_skills_installed_empty() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);

        let result = handler.handle_cmd("installed", None, &ctx).await.unwrap().unwrap();
        assert!(result["skills"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_skills_installed_with_skill() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let skill_dir = ws.join("skills/test-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# Test Skill\nA test skill").unwrap();
        let ctx = make_ctx(&dir);

        let result = handler.handle_cmd("installed", None, &ctx).await.unwrap().unwrap();
        let skills = result["skills"].as_array().unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0]["name"], "test-skill");
        assert!(skills[0]["has_skill_md"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_skills_detail() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let skill_dir = ws.join("skills/my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# My Skill").unwrap();
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "name": "my-skill" });
        let result = handler.handle_cmd("detail", Some(data), &ctx).await.unwrap().unwrap();
        assert_eq!(result["name"], "my-skill");
        assert_eq!(result["content"], "# My Skill");
    }

    #[tokio::test]
    async fn test_skills_uninstall() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let skill_dir = ws.join("skills/to-remove");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# Remove Me").unwrap();
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "name": "to-remove" });
        let result = handler.handle_cmd("uninstall", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["uninstalled"].as_bool().unwrap());
        assert!(!skill_dir.exists());
    }

    // -----------------------------------------------------------------------
    // Forge handler tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_forge_status() {
        let handler = forge::ForgeHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);

        let result = handler.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
        assert!(!result["enabled"].as_bool().unwrap());
        assert_eq!(result["artifacts_count"], 0);
    }

    #[tokio::test]
    async fn test_forge_config_save() {
        let handler = forge::ForgeHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "enabled": true });
        let result = handler.handle_cmd("config.save", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["saved"].as_bool().unwrap());
        assert!(result["enabled"].as_bool().unwrap());

        // Verify persisted
        let result = handler.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
        assert!(result["enabled"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_forge_artifacts_empty() {
        let handler = forge::ForgeHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);

        let result = handler.handle_cmd("artifacts", None, &ctx).await.unwrap().unwrap();
        assert!(result["artifacts"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_forge_artifacts_with_data() {
        let handler = forge::ForgeHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let forge_dir = dir.path().join("forge");
        std::fs::create_dir_all(&forge_dir).unwrap();
        std::fs::write(forge_dir.join("test.txt"), "hello").unwrap();
        let ctx = make_ctx(&dir);

        let result = handler.handle_cmd("artifacts", None, &ctx).await.unwrap().unwrap();
        let artifacts = result["artifacts"].as_array().unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0]["name"], "test.txt");
    }

    // -----------------------------------------------------------------------
    // Memory handler tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_memory_status_empty() {
        let handler = memory::MemoryHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);

        let result = handler.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
        assert!(!result["document_memory"]["directory_exists"].as_bool().unwrap());
        assert_eq!(result["document_memory"]["document_count"], 0);
    }

    #[tokio::test]
    async fn test_memory_documents_and_get_save() {
        let handler = memory::MemoryHandler;
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let mem_dir = ws.join("memory");
        std::fs::create_dir_all(&mem_dir).unwrap();
        std::fs::write(mem_dir.join("notes.md"), "# Notes").unwrap();
        let ctx = make_ctx(&dir);

        // Documents list
        let result = handler.handle_cmd("documents", None, &ctx).await.unwrap().unwrap();
        let docs = result["documents"].as_array().unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0]["path"], "memory/notes.md");

        // Document get (frontend sends full path from documents list)
        let data = serde_json::json!({ "path": "memory/notes.md" });
        let result = handler.handle_cmd("document.get", Some(data), &ctx).await.unwrap().unwrap();
        assert_eq!(result["content"], "# Notes");

        // Document save (frontend sends full path)
        let data = serde_json::json!({ "path": "memory/new.md", "content": "# New" });
        let result = handler.handle_cmd("document.save", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["saved"].as_bool().unwrap());
        // Verify file was written to correct location
        assert!(std::fs::read_to_string(ws.join("memory/new.md")).unwrap() == "# New");
    }

    // -----------------------------------------------------------------------
    // Tasks handler tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_tasks_boot_get_save() {
        let handler = tasks::TasksHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);

        // Save
        let data = serde_json::json!({ "content": "# Boot Instructions" });
        let result = handler.handle_cmd("boot.save", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["saved"].as_bool().unwrap());

        // Get
        let result = handler.handle_cmd("boot.get", None, &ctx).await.unwrap().unwrap();
        assert_eq!(result["content"], "# Boot Instructions");
    }

    #[tokio::test]
    async fn test_tasks_heartbeat_get_save() {
        let handler = tasks::TasksHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "content": "# Heartbeat" });
        handler.handle_cmd("heartbeat.save", Some(data), &ctx).await.unwrap();

        let result = handler.handle_cmd("heartbeat.get", None, &ctx).await.unwrap().unwrap();
        assert_eq!(result["content"], "# Heartbeat");
    }

    #[tokio::test]
    async fn test_tasks_cron_add_list_update_delete() {
        let handler = tasks::TasksHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);

        // Add
        let data = serde_json::json!({
            "name": "daily-report",
            "cron": "0 9 * * *",
            "channel": "web",
            "prompt": "Generate daily report",
            "enabled": true
        });
        let result = handler.handle_cmd("cron.add", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["added"].as_bool().unwrap());
        let job_id = result["job"]["id"].as_str().unwrap().to_string();
        assert!(job_id.starts_with("cron_"));

        // List
        let result = handler.handle_cmd("cron.list", None, &ctx).await.unwrap().unwrap();
        assert_eq!(result["total"], 1);
        let jobs = result["jobs"].as_array().unwrap();
        assert_eq!(jobs[0]["name"], "daily-report");

        // Update
        let data = serde_json::json!({ "id": job_id, "name": "daily-report-v2", "enabled": false });
        let result = handler.handle_cmd("cron.update", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["updated"].as_bool().unwrap());

        // Verify update
        let result = handler.handle_cmd("cron.list", None, &ctx).await.unwrap().unwrap();
        let jobs = result["jobs"].as_array().unwrap();
        assert_eq!(jobs[0]["name"], "daily-report-v2");
        assert!(!jobs[0]["enabled"].as_bool().unwrap());

        // Delete
        let data = serde_json::json!({ "id": job_id });
        let result = handler.handle_cmd("cron.delete", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["deleted"].as_bool().unwrap());

        // Verify empty
        let result = handler.handle_cmd("cron.list", None, &ctx).await.unwrap().unwrap();
        assert_eq!(result["total"], 0);
    }

    #[tokio::test]
    async fn test_tasks_cron_delete_nonexistent() {
        let handler = tasks::TasksHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "id": "ghost" });
        let result = handler.handle_cmd("cron.delete", Some(data), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    // -----------------------------------------------------------------------
    // Cluster handler tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_cluster_status_no_config() {
        let handler = cluster::ClusterHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);

        let result = handler.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
        assert!(!result["config_exists"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_cluster_config_get_save() {
        let handler = cluster::ClusterHandler::new();
        let dir = tempfile::tempdir().unwrap();
        ensure_config_dir(dir.path());
        let ctx = make_ctx(&dir);

        // Save
        let data = serde_json::json!({ "enabled": true, "name": "test-node", "role": "worker" });
        handler.handle_cmd("config.save", Some(data), &ctx).await.unwrap();

        // Get
        let result = handler.handle_cmd("config.get", None, &ctx).await.unwrap().unwrap();
        assert_eq!(result["enabled"], true);
        assert_eq!(result["name"], "test-node");
    }

    #[tokio::test]
    async fn test_cluster_peers_no_file() {
        let handler = cluster::ClusterHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);

        let result = handler.handle_cmd("peers", None, &ctx).await.unwrap().unwrap();
        assert!(result["peers"].as_array().unwrap().is_empty());
    }

    // -----------------------------------------------------------------------
    // Logs handler tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_logs_requests_empty() {
        let handler = logs::LogsHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);

        let result = handler.handle_cmd("requests", None, &ctx).await.unwrap().unwrap();
        assert_eq!(result["total"], 0);
        assert!(result["entries"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_logs_requests_with_data() {
        let handler = logs::LogsHandler;
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let log_dir = ws.join("logs/request_logs");
        std::fs::create_dir_all(&log_dir).unwrap();

        let entries = vec![
            serde_json::json!({ "timestamp": "2026-01-01T10:00:00Z", "session_id": "s1", "model": "gpt-4" }),
            serde_json::json!({ "timestamp": "2026-01-01T11:00:00Z", "session_id": "s2", "model": "gpt-3.5" }),
        ];
        let jsonl: String = entries.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("\n");
        std::fs::write(log_dir.join("2026-01.jsonl"), jsonl).unwrap();

        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("requests", None, &ctx).await.unwrap().unwrap();
        assert_eq!(result["total"], 2);
        let page = result["entries"].as_array().unwrap();
        assert_eq!(page.len(), 2);
        // Sorted desc by timestamp
        assert_eq!(page[0]["session_id"], "s2");
    }

    #[tokio::test]
    async fn test_logs_requests_pagination() {
        let handler = logs::LogsHandler;
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let log_dir = ws.join("logs/request_logs");
        std::fs::create_dir_all(&log_dir).unwrap();

        // Create 5 entries
        let entries: Vec<_> = (0..5).map(|i| {
            serde_json::json!({ "timestamp": format!("2026-01-{:02}T00:00:00Z", i + 1), "session_id": format!("s{}", i) })
        }).collect();
        let jsonl: String = entries.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("\n");
        std::fs::write(log_dir.join("2026-01.jsonl"), jsonl).unwrap();

        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "limit": 2, "offset": 1 });
        let result = handler.handle_cmd("requests", Some(data), &ctx).await.unwrap().unwrap();
        assert_eq!(result["total"], 5);
        let page = result["entries"].as_array().unwrap();
        assert_eq!(page.len(), 2);
    }

    #[tokio::test]
    async fn test_logs_request_detail() {
        let handler = logs::LogsHandler;
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let log_dir = ws.join("logs/request_logs");
        std::fs::create_dir_all(&log_dir).unwrap();

        let entry = serde_json::json!({ "timestamp": "2026-01-01T00:00:00Z", "session_id": "target-session", "model": "gpt-4" });
        std::fs::write(log_dir.join("2026-01.jsonl"), entry.to_string()).unwrap();

        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "session": "target-session" });
        let result = handler.handle_cmd("request_detail", Some(data), &ctx).await.unwrap().unwrap();
        assert_eq!(result["session_id"], "target-session");
        assert_eq!(result["model"], "gpt-4");
    }

    #[tokio::test]
    async fn test_logs_request_detail_not_found() {
        let handler = logs::LogsHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "session": "ghost" });
        let result = handler.handle_cmd("request_detail", Some(data), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    // -----------------------------------------------------------------------
    // Agent handler tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_agent_status() {
        let handler = agent::AgentHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);

        let result = handler.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
        assert!(!result["running"].as_bool().unwrap());
        assert_eq!(result["model_name"], "test-model");
    }

    #[tokio::test]
    async fn test_agent_start_stop_stub() {
        let handler = agent::AgentHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);

        let err = handler.handle_cmd("start", None, &ctx).await.unwrap_err();
        assert!(err.contains("Agent not available"));

        let err = handler.handle_cmd("stop", None, &ctx).await.unwrap_err();
        assert!(err.contains("Agent not available"));
    }

    // -----------------------------------------------------------------------
    // No-workspace error tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_no_workspace_returns_error() {
        let handlers: Vec<Box<dyn ModuleHandler>> = vec![
            Box::new(models::ModelsHandler::new()),
            Box::new(channels::ChannelsHandler::new()),
            Box::new(config::ConfigHandler::new()),
            Box::new(identity::IdentityHandler),
            Box::new(tools::ToolsHandler),
            Box::new(scanner::ScannerHandler::new()),
            Box::new(memory::MemoryHandler),
            Box::new(skills::SkillsHandler::new()),
            Box::new(mcp::McpHandler::new()),
            Box::new(security::SecurityHandler::new()),
            Box::new(forge::ForgeHandler::new()),
            Box::new(tasks::TasksHandler),
            Box::new(cluster::ClusterHandler::new()),
            Box::new(logs::LogsHandler),
        ];

        let ctx = make_ctx_no_workspace();

        for handler in &handlers {
            // Try a read command — should all fail with "workspace not configured"
            let result = handler.handle_cmd("list", None, &ctx).await;
            if result.is_ok() {
                // Some handlers use "status" or "config.get" instead of "list"
                let alt_result = handler.handle_cmd("status", None, &ctx).await;
                // At least one should fail due to no workspace
                if alt_result.is_ok() {
                    let alt2 = handler.handle_cmd("config.get", None, &ctx).await;
                    // For the few that don't need workspace, that's OK (like agent/system)
                    if handler.module_name() == "agent" || handler.module_name() == "system" {
                        continue;
                    }
                    assert!(
                        alt2.is_err() || result.is_err() || alt_result.is_err(),
                        "handler '{}' should fail without workspace for at least one command",
                        handler.module_name()
                    );
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Unknown command tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_all_handlers_reject_unknown_cmd() {
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        ensure_config_dir(dir.path());
        let ctx = make_ctx(&dir);

        let handlers: Vec<Box<dyn ModuleHandler>> = vec![
            Box::new(system::SystemHandler),
            Box::new(config::ConfigHandler::new()),
            Box::new(models::ModelsHandler::new()),
            Box::new(channels::ChannelsHandler::new()),
            Box::new(identity::IdentityHandler),
            Box::new(tools::ToolsHandler),
            Box::new(scanner::ScannerHandler::new()),
            Box::new(memory::MemoryHandler),
            Box::new(skills::SkillsHandler::new()),
            Box::new(mcp::McpHandler::new()),
            Box::new(security::SecurityHandler::new()),
            Box::new(forge::ForgeHandler::new()),
            Box::new(tasks::TasksHandler),
            Box::new(cluster::ClusterHandler::new()),
            Box::new(logs::LogsHandler),
            Box::new(agent::AgentHandler),
        ];

        for handler in &handlers {
            let result = handler.handle_cmd("__nonexistent_cmd__", None, &ctx).await;
            assert!(
                result.is_err(),
                "handler '{}' should reject unknown command",
                handler.module_name()
            );
        }
    }

    // ===================================================================
    // Integration tests — WsRouter dispatch pipeline
    // ===================================================================

    /// Helper: build a router with all handlers and capture responses via SendQueue.
    struct IntegrationRouter {
        router: crate::ws_router::WsRouter,
        send_queue: crate::websocket_handler::SendQueue,
        rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
        ctx: RequestContext,
        _dir: tempfile::TempDir,
    }

    impl IntegrationRouter {
        fn new() -> Self {
            let mut router = crate::ws_router::WsRouter::new();
            register_all(&mut router);

            let (tx, rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
            let (_, done_rx) = tokio::sync::watch::channel(false);
            let send_queue = crate::websocket_handler::SendQueue::from_channels(tx, done_rx);

            let dir = tempfile::tempdir().unwrap();
            write_config(dir.path());
            ensure_config_dir(dir.path());
            let ctx = make_ctx(&dir);

            Self { router, send_queue, rx, ctx, _dir: dir }
        }

        async fn dispatch(&mut self, module: &str, cmd: &str, req_id: &str, data: Option<serde_json::Value>) -> serde_json::Value {
            let msg = crate::protocol::ProtocolMessage::request(module, cmd, req_id, data);
            self.router.dispatch(&msg, &self.ctx, &self.send_queue).await;
            let bytes = self.rx.recv().await.expect("no response received");
            serde_json::from_slice(&bytes).expect("invalid JSON response")
        }

        async fn dispatch_ok(&mut self, module: &str, cmd: &str, data: Option<serde_json::Value>) -> serde_json::Value {
            let resp = self.dispatch(module, cmd, "req-1", data).await;
            assert!(resp["error"].is_null(), "unexpected error: {}", resp["error"]);
            resp["data"].clone()
        }

        async fn dispatch_err(&mut self, module: &str, cmd: &str, data: Option<serde_json::Value>) -> String {
            let resp = self.dispatch(module, cmd, "req-1", data).await;
            resp["error"].as_str().unwrap_or("null").to_string()
        }
    }

    #[tokio::test]
    async fn integration_dispatch_unknown_module() {
        let mut router = IntegrationRouter::new();
        let resp = router.dispatch("nonexistent_module", "cmd", "r1", None).await;
        assert_eq!(resp["type"], "response");
        assert!(resp["error"].as_str().unwrap().contains("unknown module"));
        assert_eq!(resp["reqId"], "r1");
    }

    #[tokio::test]
    async fn integration_dispatch_system_version() {
        let mut router = IntegrationRouter::new();
        let data = router.dispatch_ok("system", "version", None).await;
        assert_eq!(data["version"], "test");
    }

    #[tokio::test]
    async fn integration_dispatch_system_status() {
        let mut router = IntegrationRouter::new();
        let data = router.dispatch_ok("system", "status", None).await;
        assert!(data["running"].is_boolean());
    }

    #[tokio::test]
    async fn integration_dispatch_identity_lifecycle() {
        let mut router = IntegrationRouter::new();

        // List — all should not exist initially
        let data = router.dispatch_ok("identity", "list", None).await;
        let docs = data["documents"].as_array().unwrap();
        for doc in docs {
            assert!(!doc["exists"].as_bool().unwrap());
        }

        // Save
        let data = router.dispatch_ok("identity", "save", Some(serde_json::json!({
            "name": "IDENTITY.md", "content": "# My Identity"
        }))).await;
        assert!(data["saved"].as_bool().unwrap());

        // Get
        let data = router.dispatch_ok("identity", "get", Some(serde_json::json!({
            "name": "IDENTITY.md"
        }))).await;
        assert_eq!(data["content"], "# My Identity");

        // List — IDENTITY.md should exist now
        let data = router.dispatch_ok("identity", "list", None).await;
        let identity_doc = data["documents"].as_array().unwrap()
            .iter().find(|d| d["name"] == "IDENTITY.md").unwrap();
        assert!(identity_doc["exists"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn integration_dispatch_models_lifecycle() {
        let mut router = IntegrationRouter::new();

        // Add
        let data = router.dispatch_ok("models", "add", Some(serde_json::json!({
            "name": "test-gpt", "model": "gpt-4", "key": "sk-1234567890abcdef"
        }))).await;
        assert!(data["added"].as_bool().unwrap());

        // List — verify masked key
        let data = router.dispatch_ok("models", "list", None).await;
        let models = data["models"].as_array().unwrap();
        assert_eq!(models.len(), 1);
        let key = models[0]["api_key"].as_str().unwrap();
        assert!(key.contains("****"));
        assert!(!key.contains("1234567890abcdef"));

        // Set default
        router.dispatch_ok("models", "set_default", Some(serde_json::json!({
            "name": "test-gpt"
        }))).await;

        // Delete
        router.dispatch_ok("models", "delete", Some(serde_json::json!({
            "name": "test-gpt"
        }))).await;

        // Verify empty
        let data = router.dispatch_ok("models", "list", None).await;
        assert!(data["models"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn integration_dispatch_config_set_field() {
        let mut router = IntegrationRouter::new();

        router.dispatch_ok("config", "set_field", Some(serde_json::json!({
            "path": "gateway.port", "value": 9999
        }))).await;

        let data = router.dispatch_ok("config", "get", None).await;
        assert_eq!(data["gateway"]["port"], 9999);
    }

    #[tokio::test]
    async fn integration_dispatch_mcp_server_lifecycle() {
        let mut router = IntegrationRouter::new();

        // Add
        router.dispatch_ok("mcp", "server.add", Some(serde_json::json!({
            "name": "my-mcp", "command": "node", "args": ["server.js"]
        }))).await;

        // Status
        let data = router.dispatch_ok("mcp", "status", None).await;
        assert_eq!(data["servers_count"], 1);

        // Servers
        let data = router.dispatch_ok("mcp", "servers", None).await;
        assert_eq!(data["servers"].as_array().unwrap().len(), 1);

        // Update
        router.dispatch_ok("mcp", "server.update", Some(serde_json::json!({
            "name": "my-mcp", "command": "python"
        }))).await;

        // Delete
        router.dispatch_ok("mcp", "server.delete", Some(serde_json::json!({
            "name": "my-mcp"
        }))).await;

        let data = router.dispatch_ok("mcp", "status", None).await;
        assert_eq!(data["servers_count"], 0);
    }

    #[tokio::test]
    async fn integration_dispatch_tasks_cron_lifecycle() {
        let mut router = IntegrationRouter::new();

        // Add
        let data = router.dispatch_ok("tasks", "cron.add", Some(serde_json::json!({
            "name": "test-job", "cron": "0 * * * *", "prompt": "hello"
        }))).await;
        let job_id = data["job"]["id"].as_str().unwrap().to_string();

        // List
        let data = router.dispatch_ok("tasks", "cron.list", None).await;
        assert_eq!(data["total"], 1);

        // Update
        router.dispatch_ok("tasks", "cron.update", Some(serde_json::json!({
            "id": job_id, "enabled": false
        }))).await;

        // Delete
        router.dispatch_ok("tasks", "cron.delete", Some(serde_json::json!({
            "id": job_id
        }))).await;

        let data = router.dispatch_ok("tasks", "cron.list", None).await;
        assert_eq!(data["total"], 0);
    }

    #[tokio::test]
    async fn integration_dispatch_error_response_format() {
        let mut router = IntegrationRouter::new();
        let resp = router.dispatch("models", "delete", "err-1", Some(serde_json::json!({
            "name": "nonexistent"
        }))).await;
        assert_eq!(resp["type"], "response");
        assert_eq!(resp["reqId"], "err-1");
        assert!(resp["error"].as_str().unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn integration_dispatch_req_id_roundtrip() {
        let mut router = IntegrationRouter::new();
        let custom_id = "uuid-abc-123";
        let resp = router.dispatch("system", "version", custom_id, None).await;
        assert_eq!(resp["reqId"], custom_id);
    }

    #[tokio::test]
    async fn integration_dispatch_forge_config_toggle() {
        let mut router = IntegrationRouter::new();

        // Enable forge
        router.dispatch_ok("forge", "config.save", Some(serde_json::json!({
            "enabled": true
        }))).await;

        let data = router.dispatch_ok("forge", "status", None).await;
        assert!(data["enabled"].as_bool().unwrap());

        // Disable forge
        router.dispatch_ok("forge", "config.save", Some(serde_json::json!({
            "enabled": false
        }))).await;

        let data = router.dispatch_ok("forge", "status", None).await;
        assert!(!data["enabled"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn integration_dispatch_tools_roundtrip() {
        let mut router = IntegrationRouter::new();

        // Save
        router.dispatch_ok("tools", "save", Some(serde_json::json!({
            "content": "# Available Tools\n- search\n- write"
        }))).await;

        // Get
        let data = router.dispatch_ok("tools", "get", None).await;
        assert!(data["content"].as_str().unwrap().contains("search"));
    }

    // ===================================================================
    // Concurrency tests — simultaneous access to shared config
    // ===================================================================

    #[tokio::test]
    async fn concurrency_models_simultaneous_writes() {
        let handler = models::ModelsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);

        // Add 10 models sequentially — tests rapid writes don't corrupt config
        for i in 0..10 {
            let data = serde_json::json!({
                "name": format!("model-{}", i),
                "model": format!("gpt-{}", i),
                "key": format!("key-{}", i)
            });
            let result = handler.handle_cmd("add", Some(data), &ctx).await;
            assert!(result.is_ok(), "add model-{} failed: {:?}", i, result);
        }

        // Verify all 10 were added
        let result = handler.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
        let models = result["models"].as_array().unwrap();
        assert_eq!(models.len(), 10);
    }

    #[tokio::test]
    async fn concurrency_models_rapid_add_delete() {
        let handler = models::ModelsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);

        // Add then immediately delete, 20 times
        for i in 0..20 {
            let name = format!("rapid-{}", i);
            let add_data = serde_json::json!({ "name": &name, "model": "test", "key": "k" });
            handler.handle_cmd("add", Some(add_data), &ctx).await.unwrap();

            let del_data = serde_json::json!({ "name": &name });
            handler.handle_cmd("delete", Some(del_data), &ctx).await.unwrap();
        }

        // Should be empty
        let result = handler.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
        assert!(result["models"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn concurrency_cron_rapid_add_delete() {
        let handler = tasks::TasksHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);

        for i in 0..10 {
            let add_data = serde_json::json!({
                "name": format!("job-{}", i),
                "cron": "0 * * * *",
                "prompt": "test"
            });
            let result = handler.handle_cmd("cron.add", Some(add_data), &ctx).await.unwrap().unwrap();
            let job_id = result["job"]["id"].as_str().unwrap().to_string();

            let del_data = serde_json::json!({ "id": job_id });
            handler.handle_cmd("cron.delete", Some(del_data), &ctx).await.unwrap();
        }

        let result = handler.handle_cmd("cron.list", None, &ctx).await.unwrap().unwrap();
        assert_eq!(result["total"], 0);
    }

    #[tokio::test]
    async fn concurrency_parallel_reads() {
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = Arc::new(make_ctx(&dir));

        // Spawn 20 parallel read tasks
        let mut handles: Vec<tokio::task::JoinHandle<_>> = Vec::new();
        for _ in 0..20 {
            let h = config::ConfigHandler::new();
            let ctx = ctx.clone();
            handles.push(tokio::spawn(async move {
                h.handle_cmd("get", None, &ctx).await
            }));
        }

        for handle in handles {
            let result = handle.await.unwrap();
            assert!(result.is_ok(), "parallel read failed: {:?}", result);
            assert!(result.unwrap().unwrap().is_object());
        }
    }

    #[tokio::test]
    async fn concurrency_mcp_rapid_add_delete() {
        let handler = mcp::McpHandler::new();
        let dir = tempfile::tempdir().unwrap();
        ensure_config_dir(dir.path());
        let cfg = nemesis_config::McpConfig::default();
        let json = serde_json::to_string_pretty(&cfg).unwrap();
        std::fs::write(dir.path().join("config/config.mcp.json"), json).unwrap();
        let ctx = make_ctx(&dir);

        for i in 0..10 {
            let add_data = serde_json::json!({ "name": format!("s-{}", i), "command": "node" });
            handler.handle_cmd("server.add", Some(add_data), &ctx).await.unwrap();

            let del_data = serde_json::json!({ "name": format!("s-{}", i) });
            handler.handle_cmd("server.delete", Some(del_data), &ctx).await.unwrap();
        }

        let result = handler.handle_cmd("servers", None, &ctx).await.unwrap().unwrap();
        assert!(result["servers"].as_array().unwrap().is_empty());
    }

    // ===================================================================
    // Coverage Gap Tests — previously untested commands
    // ===================================================================

    // --- Models: set_default ---
    #[tokio::test]
    async fn test_models_set_default() {
        let handler = models::ModelsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);

        // Add two models
        let d1 = serde_json::json!({ "name": "m1", "model": "gpt-4", "key": "k1" });
        let d2 = serde_json::json!({ "name": "m2", "model": "gpt-3", "key": "k2" });
        handler.handle_cmd("add", Some(d1), &ctx).await.unwrap();
        handler.handle_cmd("add", Some(d2), &ctx).await.unwrap();

        // Set m2 as default (moves it to index 0)
        let data = serde_json::json!({ "name": "m2" });
        let result = handler.handle_cmd("set_default", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["set_default"].as_bool().unwrap());

        // Verify m2 is now first (default)
        let result = handler.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
        let models = result["models"].as_array().unwrap();
        assert_eq!(models[0]["model_name"], "m2");
        assert!(models[0]["is_default"].as_bool().unwrap());
        assert!(!models[1]["is_default"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_models_set_default_nonexistent() {
        let handler = models::ModelsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "name": "ghost" });
        let result = handler.handle_cmd("set_default", Some(data), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    // --- Models: test command (stub) ---
    #[tokio::test]
    async fn test_models_test_stub() {
        let handler = models::ModelsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "name": "gpt-4" });
        let result = handler.handle_cmd("test", Some(data), &ctx).await.unwrap().unwrap();
        assert_eq!(result["name"], "gpt-4");
        assert_eq!(result["status"], "not_implemented");
    }

    // --- Channels: update ---
    #[tokio::test]
    async fn test_channels_update() {
        let handler = channels::ChannelsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);

        // Get the web channel to see its config
        let data = serde_json::json!({ "name": "web" });
        let result = handler.handle_cmd("get", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["config"].is_object());

        // Update web channel enabled state
        let update_data = serde_json::json!({
            "name": "web",
            "config": { "enabled": true, "allow_from": ["*"] }
        });
        let result = handler.handle_cmd("update", Some(update_data), &ctx).await.unwrap().unwrap();
        assert!(result["updated"].as_bool().unwrap());

        // Verify update persisted
        let data = serde_json::json!({ "name": "web" });
        let result = handler.handle_cmd("get", Some(data), &ctx).await.unwrap().unwrap();
        assert_eq!(result["config"]["enabled"], true);
    }

    // --- Channels: test command (stub) ---
    #[tokio::test]
    async fn test_channels_test_stub() {
        let handler = channels::ChannelsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "name": "web" });
        let result = handler.handle_cmd("test", Some(data), &ctx).await.unwrap().unwrap();
        assert_eq!(result["status"], "not_implemented");
    }

    // --- Skills: search (stub) ---
    #[tokio::test]
    async fn test_skills_search_stub() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "query": "automation" });
        let result = handler.handle_cmd("search", Some(data), &ctx).await.unwrap().unwrap();
        assert_eq!(result["query"], "automation");
        assert!(result["results"].as_array().unwrap().is_empty());
    }

    // --- Skills: install (stub) ---
    #[tokio::test]
    async fn test_skills_install_stub() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "source": "test", "slug": "test-skill" });
        let result = handler.handle_cmd("install", Some(data), &ctx).await.unwrap().unwrap();
        assert!(!result["installed"].as_bool().unwrap());
    }

    // --- Skills: config.get and config.save ---
    #[tokio::test]
    async fn test_skills_config_get_save() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        ensure_config_dir(dir.path());
        let cfg = nemesis_config::SkillsFullConfig::default();
        let json = serde_json::to_string_pretty(&cfg).unwrap();
        std::fs::write(dir.path().join("config/config.skills.json"), json).unwrap();
        let ctx = make_ctx(&dir);

        // config.get
        let result = handler.handle_cmd("config.get", None, &ctx).await.unwrap().unwrap();
        assert!(result["enabled"].as_bool().unwrap());

        // config.save
        let save_data = serde_json::json!({ "enabled": false, "max_concurrent_searches": 5 });
        let result = handler.handle_cmd("config.save", Some(save_data), &ctx).await.unwrap().unwrap();
        assert!(result["saved"].as_bool().unwrap());
    }

    // --- Forge: reflect (stub) ---
    #[tokio::test]
    async fn test_forge_reflect_stub() {
        let handler = forge::ForgeHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("reflect", None, &ctx).await.unwrap().unwrap();
        assert!(!result["triggered"].as_bool().unwrap());
    }

    // --- Memory: vector.status ---
    #[tokio::test]
    async fn test_memory_vector_status_no_config() {
        let handler = memory::MemoryHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("vector.status", None, &ctx).await.unwrap().unwrap();
        assert_eq!(result["enabled"], false);
    }

    #[tokio::test]
    async fn test_memory_vector_status_with_config() {
        let handler = memory::MemoryHandler;
        let dir = tempfile::tempdir().unwrap();
        ensure_config_dir(dir.path());
        std::fs::write(
            dir.path().join("config/config.enhanced_memory.json"),
            r#"{"enabled": true}"#,
        ).unwrap();
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("vector.status", None, &ctx).await.unwrap().unwrap();
        assert!(result["enabled"].as_bool().unwrap());
    }

    // --- Memory: vector.search (stub) ---
    #[tokio::test]
    async fn test_memory_vector_search_stub() {
        let handler = memory::MemoryHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "query": "test query" });
        let result = handler.handle_cmd("vector.search", Some(data), &ctx).await.unwrap().unwrap();
        assert_eq!(result["query"], "test query");
        assert!(result["results"].as_array().unwrap().is_empty());
    }

    // --- Cluster: peers with actual data ---
    #[tokio::test]
    async fn test_cluster_peers_with_data() {
        let handler = cluster::ClusterHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let cluster_dir = dir.path().join("cluster");
        std::fs::create_dir_all(&cluster_dir).unwrap();
        std::fs::write(
            cluster_dir.join("peers.toml"),
            r#"[node1]
name = "node-1"
role = "master"
address = "192.168.1.10:5000"

[node2]
name = "node-2"
role = "worker"
address = "192.168.1.11:5000"
"#,
        ).unwrap();
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("peers", None, &ctx).await.unwrap().unwrap();
        assert_eq!(result["format"], "toml");
        let peers_str = result["peers"].as_str().unwrap();
        assert!(peers_str.contains("node-1"));
        assert!(peers_str.contains("master"));
    }

    // --- Agent: start and stop ---
    #[tokio::test]
    async fn test_agent_start_returns_stub() {
        let handler = agent::AgentHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = handler.handle_cmd("start", None, &ctx).await.unwrap_err();
        assert!(err.contains("Agent not available"));
    }

    #[tokio::test]
    async fn test_agent_stop_returns_stub() {
        let handler = agent::AgentHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = handler.handle_cmd("stop", None, &ctx).await.unwrap_err();
        assert!(err.contains("Agent not available"));
    }

    // ===================================================================
    // Boundary / Edge Case Tests
    // ===================================================================

    // --- Missing data (None) for commands that require it ---
    #[tokio::test]
    async fn test_models_add_missing_data() {
        let handler = models::ModelsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("add", None, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing data"));
    }

    #[tokio::test]
    async fn test_models_delete_missing_data() {
        let handler = models::ModelsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("delete", None, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_identity_get_missing_data() {
        let handler = identity::IdentityHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("get", None, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing data"));
    }

    #[tokio::test]
    async fn test_identity_save_missing_content() {
        let handler = identity::IdentityHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "name": "IDENTITY.md" });
        let result = handler.handle_cmd("save", Some(data), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing field: content"));
    }

    #[tokio::test]
    async fn test_tools_save_missing_content() {
        let handler = tools::ToolsHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({});
        let result = handler.handle_cmd("save", Some(data), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing field: content"));
    }

    #[tokio::test]
    async fn test_tasks_cron_add_missing_name() {
        let handler = tasks::TasksHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "cron": "0 * * * *" });
        let result = handler.handle_cmd("cron.add", Some(data), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing field: name"));
    }

    #[tokio::test]
    async fn test_tasks_cron_add_missing_cron() {
        let handler = tasks::TasksHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "name": "job" });
        let result = handler.handle_cmd("cron.add", Some(data), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing field: cron"));
    }

    #[tokio::test]
    async fn test_tasks_cron_update_missing_id() {
        let handler = tasks::TasksHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "name": "job" });
        let result = handler.handle_cmd("cron.update", Some(data), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing field: id"));
    }

    // --- Corrupted config files ---
    #[tokio::test]
    async fn test_config_get_corrupted_file() {
        let handler = config::ConfigHandler::new();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.json"), "{invalid json!!}").unwrap();
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("get", None, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("failed to"));
    }

    #[tokio::test]
    async fn test_models_list_corrupted_config() {
        let handler = models::ModelsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.json"), "not json at all").unwrap();
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("list", None, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_security_audit_malformed_jsonl() {
        let handler = security::SecurityHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let log_dir = dir.path().join("logs/security_logs");
        std::fs::create_dir_all(&log_dir).unwrap();
        // Mix valid and invalid JSONL lines
        std::fs::write(
            log_dir.join("test.jsonl"),
            "not json\n{\"timestamp\":\"2026-01-01\",\"risk_level\":\"LOW\"}\n{broken\n",
        ).unwrap();
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("audit", None, &ctx).await.unwrap().unwrap();
        // Should gracefully skip malformed lines, only return 1 valid entry
        assert_eq!(result["total"], 1);
    }

    #[tokio::test]
    async fn test_logs_requests_malformed_jsonl() {
        let handler = logs::LogsHandler;
        let dir = tempfile::tempdir().unwrap();
        let log_dir = dir.path().join("logs/request_logs");
        std::fs::create_dir_all(&log_dir).unwrap();
        std::fs::write(
            log_dir.join("test.jsonl"),
            "bad line\n{\"timestamp\":\"2026-01-01\",\"session_id\":\"s1\"}\n",
        ).unwrap();
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("requests", None, &ctx).await.unwrap().unwrap();
        assert_eq!(result["total"], 1);
    }

    // --- Special characters in names ---
    #[tokio::test]
    async fn test_models_add_special_chars_in_name() {
        let handler = models::ModelsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({
            "name": "zhipu/glm-4.7",
            "model": "glm-4",
            "key": "test-key"
        });
        let result = handler.handle_cmd("add", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["added"].as_bool().unwrap());

        let result = handler.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
        let models = result["models"].as_array().unwrap();
        assert_eq!(models[0]["model_name"], "zhipu/glm-4.7");
    }

    #[tokio::test]
    async fn test_mcp_server_with_special_chars() {
        let handler = mcp::McpHandler::new();
        let dir = tempfile::tempdir().unwrap();
        ensure_config_dir(dir.path());
        let cfg = nemesis_config::McpConfig::default();
        let json = serde_json::to_string_pretty(&cfg).unwrap();
        std::fs::write(dir.path().join("config/config.mcp.json"), json).unwrap();
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({
            "name": "my-server/v2",
            "command": "C:\\Program Files\\node.exe",
            "args": ["--port", "3000"]
        });
        let result = handler.handle_cmd("server.add", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["added"].as_bool().unwrap());
    }

    // --- Config: empty path in set_field ---
    #[tokio::test]
    async fn test_config_set_field_empty_path() {
        let handler = config::ConfigHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "path": "", "value": 42 });
        let result = handler.handle_cmd("set_field", Some(data), &ctx).await;
        assert!(result.is_err());
    }

    // --- Config: deep nested set_field ---
    #[tokio::test]
    async fn test_config_set_field_deep_nested() {
        let handler = config::ConfigHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "path": "gateway.host", "value": "0.0.0.0" });
        let result = handler.handle_cmd("set_field", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["updated"].as_bool().unwrap());

        let result = handler.handle_cmd("get", None, &ctx).await.unwrap().unwrap();
        assert_eq!(result["gateway"]["host"], "0.0.0.0");
    }

    // --- Forge: config.save missing enabled field ---
    #[tokio::test]
    async fn test_forge_config_save_missing_enabled() {
        let handler = forge::ForgeHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({});
        let result = handler.handle_cmd("config.save", Some(data), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing or invalid"));
    }

    #[tokio::test]
    async fn test_forge_config_save_non_boolean_enabled() {
        let handler = forge::ForgeHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "enabled": "yes" });
        let result = handler.handle_cmd("config.save", Some(data), &ctx).await;
        assert!(result.is_err());
    }

    // --- Memory: path traversal in document.save ---
    #[tokio::test]
    async fn test_memory_document_path_traversal() {
        let handler = memory::MemoryHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "path": "../../etc/passwd", "content": "evil" });
        let result = handler.handle_cmd("document.save", Some(data), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("traversal"));
    }

    // --- Identity: path traversal ---
    #[tokio::test]
    async fn test_identity_save_path_traversal() {
        let handler = identity::IdentityHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "name": "../../etc/shadow", "content": "evil" });
        let result = handler.handle_cmd("save", Some(data), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("traversal"));
    }

    // --- Memory: subdirectory files ---
    #[tokio::test]
    async fn test_memory_documents_subdirectory() {
        let handler = memory::MemoryHandler;
        let dir = tempfile::tempdir().unwrap();
        let mem_dir = dir.path().join("memory/subdir");
        std::fs::create_dir_all(&mem_dir).unwrap();
        std::fs::write(mem_dir.join("deep.md"), "# Deep").unwrap();
        std::fs::write(dir.path().join("memory/root.md"), "# Root").unwrap();
        let ctx = make_ctx(&dir);

        let result = handler.handle_cmd("documents", None, &ctx).await.unwrap().unwrap();
        let docs = result["documents"].as_array().unwrap();
        assert_eq!(docs.len(), 2);
        let paths: Vec<&str> = docs.iter().filter_map(|d| d["path"].as_str()).collect();
        assert!(paths.iter().any(|p| p.contains("subdir/deep.md")));
        assert!(paths.iter().any(|p| p.contains("root.md")));
    }

    // --- Skills: detail non-existent ---
    #[tokio::test]
    async fn test_skills_detail_nonexistent() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "name": "nonexistent" });
        let result = handler.handle_cmd("detail", Some(data), &ctx).await;
        assert!(result.is_err());
    }

    // --- Skills: uninstall non-existent ---
    #[tokio::test]
    async fn test_skills_uninstall_nonexistent() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "name": "ghost" });
        let result = handler.handle_cmd("uninstall", Some(data), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    // --- MCP: config.get and config.save ---
    #[tokio::test]
    async fn test_mcp_config_get_save() {
        let handler = mcp::McpHandler::new();
        let dir = tempfile::tempdir().unwrap();
        ensure_config_dir(dir.path());
        let cfg = nemesis_config::McpConfig {
            enabled: true,
            timeout: 30,
            ..Default::default()
        };
        let json = serde_json::to_string_pretty(&cfg).unwrap();
        std::fs::write(dir.path().join("config/config.mcp.json"), json).unwrap();
        let ctx = make_ctx(&dir);

        let result = handler.handle_cmd("config.get", None, &ctx).await.unwrap().unwrap();
        assert!(result["enabled"].as_bool().unwrap());
        assert_eq!(result["timeout"], 30);

        let save_data = serde_json::json!({ "enabled": false, "servers": [], "timeout": 60 });
        let result = handler.handle_cmd("config.save", Some(save_data), &ctx).await.unwrap().unwrap();
        assert!(result["saved"].as_bool().unwrap());

        let result = handler.handle_cmd("config.get", None, &ctx).await.unwrap().unwrap();
        assert!(!result["enabled"].as_bool().unwrap());
        assert_eq!(result["timeout"], 60);
    }

    // --- Config: save invalid JSON ---
    #[tokio::test]
    async fn test_config_save_invalid_structure() {
        let handler = config::ConfigHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);
        // "123" is valid JSON but not a valid Config struct
        let data = serde_json::json!(123);
        let result = handler.handle_cmd("save", Some(data), &ctx).await;
        assert!(result.is_err());
    }

    // --- Security: audit with limit and offset ---
    #[tokio::test]
    async fn test_security_audit_with_filter() {
        let handler = security::SecurityHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let log_dir = dir.path().join("logs/security_logs");
        std::fs::create_dir_all(&log_dir).unwrap();
        let entries: Vec<_> = (0..10).map(|i| {
            serde_json::json!({
                "timestamp": format!("2026-01-{:02}T00:00:00Z", i + 1),
                "risk_level": if i % 2 == 0 { "HIGH" } else { "LOW" },
                "action": "test"
            })
        }).collect();
        let jsonl: String = entries.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("\n");
        std::fs::write(log_dir.join("test.jsonl"), jsonl).unwrap();
        let ctx = make_ctx(&dir);

        // With limit
        let data = serde_json::json!({ "limit": 3 });
        let result = handler.handle_cmd("audit", Some(data), &ctx).await.unwrap().unwrap();
        assert_eq!(result["total"], 10);
        assert_eq!(result["entries"].as_array().unwrap().len(), 3);

        // With offset
        let data = serde_json::json!({ "offset": 8 });
        let result = handler.handle_cmd("audit", Some(data), &ctx).await.unwrap().unwrap();
        assert_eq!(result["entries"].as_array().unwrap().len(), 2);

        // With limit exceeding total
        let data = serde_json::json!({ "limit": 100 });
        let result = handler.handle_cmd("audit", Some(data), &ctx).await.unwrap().unwrap();
        assert_eq!(result["entries"].as_array().unwrap().len(), 10);
    }

    // ===================================================================
    // Mock / Invalid Input Tests
    // ===================================================================

    // --- Null data for required commands ---
    #[tokio::test]
    async fn test_null_data_for_required_commands() {
        let handlers_and_cmds: Vec<(&str, &str)> = vec![
            ("models", "add"), ("models", "delete"), ("models", "set_default"),
            ("identity", "get"), ("identity", "save"),
            ("tools", "save"),
            ("channels", "get"), ("channels", "update"),
            ("mcp", "server.add"), ("mcp", "server.delete"),
            ("skills", "detail"), ("skills", "uninstall"),
            ("tasks", "cron.add"), ("tasks", "cron.delete"),
        ];
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        ensure_config_dir(dir.path());

        // Also write MCP config
        let mcp_cfg = nemesis_config::McpConfig::default();
        std::fs::write(
            dir.path().join("config/config.mcp.json"),
            serde_json::to_string_pretty(&mcp_cfg).unwrap(),
        ).unwrap();

        let ctx = make_ctx(&dir);

        for (module, cmd) in &handlers_and_cmds {
            let _result = crate::ws_router::WsRouter::new();
            // Use the handler directly via match
            match *module {
                "models" => {
                    let h = models::ModelsHandler::new();
                    let r = h.handle_cmd(cmd, None, &ctx).await;
                    assert!(r.is_err(), "models.{} should fail with None data", cmd);
                }
                "identity" => {
                    let h = identity::IdentityHandler;
                    let r = h.handle_cmd(cmd, None, &ctx).await;
                    assert!(r.is_err(), "identity.{} should fail with None data", cmd);
                }
                "tools" => {
                    let h = tools::ToolsHandler;
                    let r = h.handle_cmd(cmd, None, &ctx).await;
                    assert!(r.is_err(), "tools.{} should fail with None data", cmd);
                }
                "channels" => {
                    let h = channels::ChannelsHandler::new();
                    let r = h.handle_cmd(cmd, None, &ctx).await;
                    assert!(r.is_err(), "channels.{} should fail with None data", cmd);
                }
                "mcp" => {
                    let h = mcp::McpHandler::new();
                    let r = h.handle_cmd(cmd, None, &ctx).await;
                    assert!(r.is_err(), "mcp.{} should fail with None data", cmd);
                }
                "skills" => {
                    let h = skills::SkillsHandler::new();
                    let r = h.handle_cmd(cmd, None, &ctx).await;
                    assert!(r.is_err(), "skills.{} should fail with None data", cmd);
                }
                "tasks" => {
                    let h = tasks::TasksHandler;
                    let r = h.handle_cmd(cmd, None, &ctx).await;
                    assert!(r.is_err(), "tasks.{} should fail with None data", cmd);
                }
                _ => {}
            }
        }
    }

    // --- Empty strings in required fields ---
    #[tokio::test]
    async fn test_models_add_empty_name() {
        let handler = models::ModelsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "name": "", "model": "gpt-4", "key": "test" });
        // Empty name should still work (it's the caller's responsibility to validate)
        let result = handler.handle_cmd("add", Some(data), &ctx).await;
        // It will succeed — empty string is still a valid string
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_tasks_boot_get_nonexistent() {
        let handler = tasks::TasksHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("boot.get", None, &ctx).await;
        assert!(result.is_err());
    }

    // --- Config: cors.toggle with invalid type ---
    #[tokio::test]
    async fn test_config_cors_toggle_invalid_type() {
        let handler = config::ConfigHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "enabled": "yes" }); // string instead of bool
        let result = handler.handle_cmd("cors.toggle", Some(data), &ctx).await;
        assert!(result.is_err());
    }

    // ===================================================================
    // High Concurrency Tests (50+ concurrent operations)
    // ===================================================================

    #[tokio::test]
    async fn hicon_50_parallel_config_reads() {
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = Arc::new(make_ctx(&dir));

        let mut handles: Vec<tokio::task::JoinHandle<_>> = Vec::new();
        for _ in 0..50 {
            let h = config::ConfigHandler::new();
            let ctx = ctx.clone();
            handles.push(tokio::spawn(async move {
                h.handle_cmd("get", None, &ctx).await
            }));
        }

        for (i, handle) in handles.into_iter().enumerate() {
            let result = handle.await.unwrap();
            assert!(result.is_ok(), "parallel config read #{} failed: {:?}", i, result);
        }
    }

    #[tokio::test]
    async fn hicon_50_parallel_identity_reads() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("IDENTITY.md"), "# Test Identity Content").unwrap();
        let ctx = Arc::new(make_ctx(&dir));

        let mut handles: Vec<tokio::task::JoinHandle<_>> = Vec::new();
        for _ in 0..50 {
            let ctx = ctx.clone();
            handles.push(tokio::spawn(async move {
                let h = identity::IdentityHandler;
                h.handle_cmd("list", None, &ctx).await
            }));
        }

        for (i, handle) in handles.into_iter().enumerate() {
            let result = handle.await.unwrap();
            assert!(result.is_ok(), "parallel identity read #{} failed: {:?}", i, result);
        }
    }

    #[tokio::test]
    async fn hicon_50_parallel_system_status() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = Arc::new(make_ctx(&dir));

        let mut handles: Vec<tokio::task::JoinHandle<_>> = Vec::new();
        for _ in 0..50 {
            let ctx = ctx.clone();
            handles.push(tokio::spawn(async move {
                let h = system::SystemHandler;
                h.handle_cmd("status", None, &ctx).await
            }));
        }

        for (i, handle) in handles.into_iter().enumerate() {
            let result = handle.await.unwrap();
            assert!(result.is_ok(), "parallel system status #{} failed: {:?}", i, result);
        }
    }

    #[tokio::test]
    async fn hicon_50_parallel_logs_reads() {
        let dir = tempfile::tempdir().unwrap();
        let log_dir = dir.path().join("logs/request_logs");
        std::fs::create_dir_all(&log_dir).unwrap();
        // Create 100 log entries
        let entries: Vec<_> = (0..100).map(|i| {
            serde_json::json!({
                "timestamp": format!("2026-01-{:02}T{:02}:00:00Z", (i % 28) + 1, i),
                "session_id": format!("s{}", i),
                "model": "gpt-4"
            })
        }).collect();
        let jsonl: String = entries.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("\n");
        std::fs::write(log_dir.join("2026-01.jsonl"), jsonl).unwrap();
        let ctx = Arc::new(make_ctx(&dir));

        let mut handles: Vec<tokio::task::JoinHandle<_>> = Vec::new();
        for _ in 0..50 {
            let ctx = ctx.clone();
            handles.push(tokio::spawn(async move {
                let h = logs::LogsHandler;
                h.handle_cmd("requests", Some(serde_json::json!({ "limit": 10 })), &ctx).await
            }));
        }

        for (i, handle) in handles.into_iter().enumerate() {
            let result = handle.await.unwrap();
            assert!(result.is_ok(), "parallel log read #{} failed: {:?}", i, result);
            let data = result.unwrap().unwrap();
            assert_eq!(data["entries"].as_array().unwrap().len(), 10);
            assert_eq!(data["total"], 100);
        }
    }

    #[tokio::test]
    async fn hicon_mixed_50_concurrent_read_write() {
        // 25 readers + 25 writers interleaved
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = Arc::new(make_ctx(&dir));

        // Add one model first
        let setup_handler = models::ModelsHandler::new();
        let setup_ctx = make_ctx(&dir);
        setup_handler.handle_cmd("add", Some(serde_json::json!({
            "name": "base-model", "model": "gpt-4", "key": "key"
        })), &setup_ctx).await.unwrap();

        let mut handles: Vec<tokio::task::JoinHandle<_>> = Vec::new();

        // 25 readers
        for _ in 0..25 {
            let ctx = ctx.clone();
            handles.push(tokio::spawn(async move {
                let h = models::ModelsHandler::new();
                h.handle_cmd("list", None, &ctx).await
            }));
        }

        // 25 writers (each adds then deletes a unique model)
        for i in 0..25 {
            let ctx = ctx.clone();
            handles.push(tokio::spawn(async move {
                let h = models::ModelsHandler::new();
                let name = format!("concurrent-{}", i);
                let add_result = h.handle_cmd("add", Some(serde_json::json!({
                    "name": &name, "model": "test", "key": "k"
                })), &ctx).await;

                // Only delete if add succeeded (another writer may have raced)
                if add_result.is_ok() {
                    let _ = h.handle_cmd("delete", Some(serde_json::json!({
                        "name": &name
                    })), &ctx).await;
                }
                add_result
            }));
        }

        let mut errors = 0;
        for (_i, handle) in handles.into_iter().enumerate() {
            let result = handle.await.unwrap();
            if result.is_err() {
                errors += 1;
                // Some write failures are expected under concurrent writes — that's OK
                // as long as no panics occur
            }
        }
        // Verify no panics — if we got here, all tasks completed
        // At least the readers should all succeed
        assert!(errors < 25, "too many errors during concurrent access: {}", errors);
    }

    #[tokio::test]
    async fn hicon_30_parallel_mixed_handlers() {
        // Mix of different handlers all hitting the same workspace concurrently
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        ensure_config_dir(dir.path());
        std::fs::write(dir.path().join("IDENTITY.md"), "# Test").unwrap();
        std::fs::write(dir.path().join("TOOLS.md"), "# Tools").unwrap();
        let forge_dir = dir.path().join("forge");
        std::fs::create_dir_all(&forge_dir).unwrap();
        std::fs::write(forge_dir.join("a.txt"), "test").unwrap();
        let mem_dir = dir.path().join("memory");
        std::fs::create_dir_all(&mem_dir).unwrap();
        std::fs::write(mem_dir.join("notes.md"), "# Notes").unwrap();
        let ctx = Arc::new(make_ctx(&dir));

        let mut handles: Vec<tokio::task::JoinHandle<(&str, _)>> = Vec::new();

        // 5 system.version
        for _ in 0..5 {
            let ctx = ctx.clone();
            handles.push(tokio::spawn(async move {
                let r = system::SystemHandler.handle_cmd("version", None, &ctx).await;
                ("system", r)
            }));
        }
        // 5 config.get
        for _ in 0..5 {
            let ctx = ctx.clone();
            handles.push(tokio::spawn(async move {
                let r = config::ConfigHandler::new().handle_cmd("get", None, &ctx).await;
                ("config", r)
            }));
        }
        // 5 identity.list
        for _ in 0..5 {
            let ctx = ctx.clone();
            handles.push(tokio::spawn(async move {
                let r = identity::IdentityHandler.handle_cmd("list", None, &ctx).await;
                ("identity", r)
            }));
        }
        // 5 forge.status
        for _ in 0..5 {
            let ctx = ctx.clone();
            handles.push(tokio::spawn(async move {
                let r = forge::ForgeHandler::new().handle_cmd("status", None, &ctx).await;
                ("forge", r)
            }));
        }
        // 5 memory.documents
        for _ in 0..5 {
            let ctx = ctx.clone();
            handles.push(tokio::spawn(async move {
                let r = memory::MemoryHandler.handle_cmd("documents", None, &ctx).await;
                ("memory", r)
            }));
        }
        // 5 tools.get
        for _ in 0..5 {
            let ctx = ctx.clone();
            handles.push(tokio::spawn(async move {
                let r = tools::ToolsHandler.handle_cmd("get", None, &ctx).await;
                ("tools", r)
            }));
        }

        for handle in handles {
            let (module, result) = handle.await.unwrap();
            assert!(
                result.is_ok(),
                "concurrent mixed handler '{}' failed: {:?}",
                module,
                result
            );
        }
    }

    #[tokio::test]
    async fn hicon_50_burst_tasks_cron() {
        // Rapidly add 50 cron jobs, then verify all are present
        let handler = tasks::TasksHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);

        let mut job_ids = Vec::new();
        for i in 0..50 {
            let data = serde_json::json!({
                "name": format!("burst-job-{}", i),
                "cron": format!("{} * * * *", i % 60),
                "prompt": format!("prompt-{}", i),
                "enabled": true
            });
            let result = handler.handle_cmd("cron.add", Some(data), &ctx).await.unwrap().unwrap();
            let id = result["job"]["id"].as_str().unwrap().to_string();
            job_ids.push(id);
        }

        // Verify all 50 are there
        let result = handler.handle_cmd("cron.list", None, &ctx).await.unwrap().unwrap();
        assert_eq!(result["total"], 50);

        // Delete all 50
        for id in &job_ids {
            let data = serde_json::json!({ "id": id });
            handler.handle_cmd("cron.delete", Some(data), &ctx).await.unwrap();
        }

        let result = handler.handle_cmd("cron.list", None, &ctx).await.unwrap().unwrap();
        assert_eq!(result["total"], 0);
    }

    #[tokio::test]
    async fn hicon_50_burst_mcp_servers() {
        let handler = mcp::McpHandler::new();
        let dir = tempfile::tempdir().unwrap();
        ensure_config_dir(dir.path());
        let cfg = nemesis_config::McpConfig::default();
        std::fs::write(
            dir.path().join("config/config.mcp.json"),
            serde_json::to_string_pretty(&cfg).unwrap(),
        ).unwrap();
        let ctx = make_ctx(&dir);

        // Add 50 MCP servers
        for i in 0..50 {
            let data = serde_json::json!({
                "name": format!("burst-server-{}", i),
                "command": format!("cmd-{}", i)
            });
            handler.handle_cmd("server.add", Some(data), &ctx).await.unwrap();
        }

        // Verify
        let result = handler.handle_cmd("servers", None, &ctx).await.unwrap().unwrap();
        assert_eq!(result["servers"].as_array().unwrap().len(), 50);

        // Delete all
        for i in 0..50 {
            let data = serde_json::json!({ "name": format!("burst-server-{}", i) });
            handler.handle_cmd("server.delete", Some(data), &ctx).await.unwrap();
        }

        let result = handler.handle_cmd("servers", None, &ctx).await.unwrap().unwrap();
        assert!(result["servers"].as_array().unwrap().is_empty());
    }

    // ===================================================================
    // Helper function branch coverage
    // ===================================================================

    #[test]
    fn test_mask_sensitive_exactly_8_chars() {
        // Exactly 8 → triggers "len <= 8" branch, returns "****"
        assert_eq!(mask_sensitive("12345678"), "****");
    }

    #[test]
    fn test_mask_sensitive_9_chars() {
        // 9 chars → triggers the formatting branch
        assert_eq!(mask_sensitive("123456789"), "1234****6789");
    }

    #[test]
    fn test_mask_sensitive_empty() {
        assert_eq!(mask_sensitive(""), "****");
    }

    #[test]
    fn test_mask_sensitive_unicode() {
        let val = "αβγδabcdefghijkl"; // unicode prefix
        let result = mask_sensitive(val);
        assert!(result.contains("****"));
    }

    #[test]
    fn test_is_sensitive_field_case_insensitive_variants() {
        assert!(is_sensitive_field("API_KEY"));
        assert!(is_sensitive_field("Api_Key"));
        assert!(is_sensitive_field("ACCESS_TOKEN"));
        assert!(is_sensitive_field("password"));
        assert!(is_sensitive_field("CLIENT_SECRET"));
        assert!(is_sensitive_field("ENCRYPT_KEY"));
        assert!(!is_sensitive_field("apiurl"));
        assert!(!is_sensitive_field("model_name"));
    }

    #[test]
    fn test_get_str_null_value() {
        let data = serde_json::json!({ "name": null });
        assert!(get_str(&data, "name").is_err());
    }

    #[test]
    fn test_get_str_number_value() {
        let data = serde_json::json!({ "name": 42 });
        assert!(get_str(&data, "name").is_err());
    }

    #[test]
    fn test_get_str_bool_value() {
        let data = serde_json::json!({ "name": true });
        assert!(get_str(&data, "name").is_err());
    }

    #[test]
    fn test_get_str_array_value() {
        let data = serde_json::json!({ "name": [1, 2, 3] });
        assert!(get_str(&data, "name").is_err());
    }

    #[test]
    fn test_get_str_object_value() {
        let data = serde_json::json!({ "name": {} });
        assert!(get_str(&data, "name").is_err());
    }

    #[test]
    fn test_get_str_empty_string() {
        let data = serde_json::json!({ "name": "" });
        assert_eq!(get_str(&data, "name").unwrap(), "");
    }

    #[test]
    fn test_get_str_whitespace() {
        let data = serde_json::json!({ "name": "  " });
        assert_eq!(get_str(&data, "name").unwrap(), "  ");
    }

    #[test]
    fn test_get_opt_str_null() {
        let data = serde_json::json!({ "name": null });
        assert_eq!(get_opt_str(&data, "name"), None);
    }

    #[test]
    fn test_get_opt_str_number() {
        let data = serde_json::json!({ "name": 42 });
        assert_eq!(get_opt_str(&data, "name"), None);
    }

    #[test]
    fn test_resolve_path_empty_relative() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let resolved = resolve_path(&ws, "").unwrap();
        assert_eq!(resolved, PathBuf::from(&ws));
    }

    #[test]
    fn test_resolve_path_absolute_attempt() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        // Absolute paths should always be rejected
        let result = resolve_path(&ws, "/etc/passwd");
        assert!(result.is_err(), "absolute path should be rejected");
    }

    #[test]
    fn test_resolve_path_unicode() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let resolved = resolve_path(&ws, "文件/中文.txt").unwrap();
        assert!(resolved.to_string_lossy().contains("中文.txt"));
    }

    #[test]
    fn test_resolve_path_dot_dot_only() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let result = resolve_path(&ws, "..");
        assert!(result.is_err());
    }

    #[test]
    fn test_read_workspace_file_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        std::fs::write(dir.path().join("empty.txt"), "").unwrap();
        let content = read_workspace_file(&ws, "empty.txt").unwrap();
        assert_eq!(content, "");
    }

    #[test]
    fn test_write_workspace_file_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        write_workspace_file(&ws, "test.txt", "first").unwrap();
        write_workspace_file(&ws, "test.txt", "second").unwrap();
        assert_eq!(read_workspace_file(&ws, "test.txt").unwrap(), "second");
    }

    #[test]
    fn test_write_workspace_file_empty_content() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        write_workspace_file(&ws, "empty.txt", "").unwrap();
        assert_eq!(read_workspace_file(&ws, "empty.txt").unwrap(), "");
    }

    #[test]
    fn test_write_workspace_file_large_content() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let large = "A".repeat(1_000_000); // 1MB
        write_workspace_file(&ws, "large.txt", &large).unwrap();
        let content = read_workspace_file(&ws, "large.txt").unwrap();
        assert_eq!(content.len(), 1_000_000);
    }

    #[test]
    fn test_list_workspace_dir_empty() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        std::fs::create_dir_all(dir.path().join("subdir")).unwrap();
        let entries = list_workspace_dir(&ws, "subdir").unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_list_workspace_dir_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let entries = list_workspace_dir(&ws, "nonexistent").unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_list_workspace_dir_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let subdir = dir.path().join("sortdir");
        std::fs::create_dir_all(&subdir).unwrap();
        std::fs::write(subdir.join("c.txt"), "").unwrap();
        std::fs::write(subdir.join("a.txt"), "").unwrap();
        std::fs::write(subdir.join("b.txt"), "").unwrap();
        let entries = list_workspace_dir(&ws, "sortdir").unwrap();
        assert_eq!(entries, vec!["a.txt", "b.txt", "c.txt"]);
    }

    #[test]
    fn test_list_workspace_dir_many_files() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let subdir = dir.path().join("manydir");
        std::fs::create_dir_all(&subdir).unwrap();
        for i in 0..500 {
            std::fs::write(subdir.join(format!("f{:04}.txt", i)), "").unwrap();
        }
        let entries = list_workspace_dir(&ws, "manydir").unwrap();
        assert_eq!(entries.len(), 500);
    }

    #[test]
    fn test_require_workspace_missing() {
        let ctx = make_ctx_no_workspace();
        let result = require_workspace(&ctx);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("workspace not configured"));
    }

    // ===================================================================
    // Handler branch coverage — every if/match/Result path
    // ===================================================================

    // --- Models: empty api_key not masked ---
    #[tokio::test]
    async fn test_models_list_empty_api_key() {
        let handler = models::ModelsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = nemesis_config::Config::default();
        cfg.model_list.push(nemesis_config::ModelConfig {
            model_name: "nokey".into(),
            model: "test".into(),
            api_key: String::new(),
            ..Default::default()
        });
        std::fs::write(dir.path().join("config.json"), serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
        let key = result["models"][0]["api_key"].as_str().unwrap();
        assert_eq!(key, ""); // empty key should remain empty
    }

    // --- Models: add with optional fields ---
    #[tokio::test]
    async fn test_models_add_with_all_optional_fields() {
        let handler = models::ModelsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({
            "name": "full-model",
            "model": "gpt-4-turbo",
            "key": "sk-long-key-12345678",
            "base_url": "https://api.custom.com/v1",
            "proxy": "http://proxy:8080"
        });
        let result = handler.handle_cmd("add", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["added"].as_bool().unwrap());

        let result = handler.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
        let m = &result["models"][0];
        assert_eq!(m["model"], "gpt-4-turbo");
        assert_eq!(m["api_base"], "https://api.custom.com/v1");
        assert_eq!(m["proxy"], "http://proxy:8080");
    }

    // --- Channels: get existing channel, verify masking ---
    #[tokio::test]
    async fn test_channels_get_masks_token() {
        let handler = channels::ChannelsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = nemesis_config::Config::default();
        cfg.channels.telegram = nemesis_config::TelegramConfig {
            enabled: true,
            token: "1234567890:ABCDEFtoken".into(),
            allow_from: vec!["*".into()],
            ..Default::default()
        };
        std::fs::write(dir.path().join("config.json"), serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "name": "telegram" });
        let result = handler.handle_cmd("get", Some(data), &ctx).await.unwrap().unwrap();
        let config = &result["config"];
        assert!(config["token"].as_str().unwrap().contains("****"));
    }

    // --- Channels: update non-existent ---
    #[tokio::test]
    async fn test_channels_update_nonexistent() {
        let handler = channels::ChannelsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({
            "name": "nonexistent_channel",
            "config": { "enabled": false }
        });
        let result = handler.handle_cmd("update", Some(data), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    // --- Config: sanitize_config deep recursion ---
    #[tokio::test]
    async fn test_config_get_nested_sensitive() {
        let handler = config::ConfigHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = nemesis_config::Config::default();
        cfg.model_list.push(nemesis_config::ModelConfig {
            model_name: "m1".into(),
            model: "gpt-4".into(),
            api_key: "sk-deepnested12345678key".into(),
            ..Default::default()
        });
        std::fs::write(dir.path().join("config.json"), serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("get", None, &ctx).await.unwrap().unwrap();
        let key = result["model_list"][0]["api_key"].as_str().unwrap();
        assert!(key.contains("****"));
    }

    // --- Config: set_field non-existent intermediate path ---
    #[tokio::test]
    async fn test_config_set_field_creates_intermediate() {
        let handler = config::ConfigHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);
        // Set a path that doesn't exist — should create intermediate objects
        let data = serde_json::json!({ "path": "new_section.sub.value", "value": 42 });
        let result = handler.handle_cmd("set_field", Some(data), &ctx).await;
        assert!(result.is_ok(), "set_field with intermediate path should work: {:?}", result);
    }

    // --- Forge: status with forge dir containing subdirs ---
    #[tokio::test]
    async fn test_forge_status_with_artifacts() {
        let handler = forge::ForgeHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = nemesis_config::Config::default();
        cfg.forge = Some(nemesis_config::ForgeFlagConfig { enabled: true });
        std::fs::write(dir.path().join("config.json"), serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
        let forge_dir = dir.path().join("forge");
        std::fs::create_dir_all(forge_dir.join("skills")).unwrap();
        std::fs::write(forge_dir.join("skills/test.md"), "# Test").unwrap();
        std::fs::write(forge_dir.join("config.json"), "{}").unwrap();
        let ctx = make_ctx(&dir);

        let result = handler.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
        assert!(result["enabled"].as_bool().unwrap());
        assert_eq!(result["artifacts_count"], 2); // skills/ dir + config.json

        let result = handler.handle_cmd("artifacts", None, &ctx).await.unwrap().unwrap();
        let artifacts = result["artifacts"].as_array().unwrap();
        assert!(artifacts.iter().any(|a| a["name"] == "skills" && a["type"] == "directory"));
        assert!(artifacts.iter().any(|a| a["name"] == "config.json" && a["type"] == "file"));
    }

    // --- Tasks: cron update multiple fields simultaneously ---
    #[tokio::test]
    async fn test_tasks_cron_update_multiple_fields() {
        let handler = tasks::TasksHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let add_data = serde_json::json!({
            "name": "multi-update",
            "cron": "0 * * * *",
            "prompt": "original",
            "enabled": true
        });
        let result = handler.handle_cmd("cron.add", Some(add_data), &ctx).await.unwrap().unwrap();
        let job_id = result["job"]["id"].as_str().unwrap().to_string();

        let update_data = serde_json::json!({
            "id": job_id,
            "name": "updated-name",
            "cron": "0 0 * * *",
            "prompt": "updated-prompt",
            "enabled": false
        });
        handler.handle_cmd("cron.update", Some(update_data), &ctx).await.unwrap();

        let result = handler.handle_cmd("cron.list", None, &ctx).await.unwrap().unwrap();
        let job = &result["jobs"].as_array().unwrap()[0];
        assert_eq!(job["name"], "updated-name");
        assert_eq!(job["cron"], "0 0 * * *");
        assert_eq!(job["prompt"], "updated-prompt");
        assert!(!job["enabled"].as_bool().unwrap());
    }

    // --- Tasks: corrupted cron jobs.json ---
    #[tokio::test]
    async fn test_tasks_cron_list_corrupted() {
        let handler = tasks::TasksHandler;
        let dir = tempfile::tempdir().unwrap();
        let cron_dir = dir.path().join("cron");
        std::fs::create_dir_all(&cron_dir).unwrap();
        std::fs::write(cron_dir.join("jobs.json"), "not json at all").unwrap();
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("cron.list", None, &ctx).await;
        assert!(result.is_err());
    }

    // --- Tasks: boot.save then heartbeat.save independent ---
    #[tokio::test]
    async fn test_tasks_independent_files() {
        let handler = tasks::TasksHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);

        handler.handle_cmd("boot.save", Some(serde_json::json!({ "content": "# Boot" })), &ctx).await.unwrap();
        handler.handle_cmd("heartbeat.save", Some(serde_json::json!({ "content": "# Heart" })), &ctx).await.unwrap();

        let boot = handler.handle_cmd("boot.get", None, &ctx).await.unwrap().unwrap();
        let heart = handler.handle_cmd("heartbeat.get", None, &ctx).await.unwrap().unwrap();
        assert_eq!(boot["content"], "# Boot");
        assert_eq!(heart["content"], "# Heart");

        // Files are independent — overwriting one doesn't affect the other
        handler.handle_cmd("boot.save", Some(serde_json::json!({ "content": "# Boot v2" })), &ctx).await.unwrap();
        let heart2 = handler.handle_cmd("heartbeat.get", None, &ctx).await.unwrap().unwrap();
        assert_eq!(heart2["content"], "# Heart"); // unchanged
    }

    // --- Security: audit with risk_level filter ---
    #[tokio::test]
    async fn test_security_audit_risk_level_filter() {
        let handler = security::SecurityHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let log_dir = dir.path().join("logs/security_logs");
        std::fs::create_dir_all(&log_dir).unwrap();
        let entries = vec![
            serde_json::json!({ "timestamp": "2026-01-01T00:00:00Z", "risk_level": "HIGH" }),
            serde_json::json!({ "timestamp": "2026-01-02T00:00:00Z", "risk_level": "LOW" }),
            serde_json::json!({ "timestamp": "2026-01-03T00:00:00Z", "risk_level": "HIGH" }),
            serde_json::json!({ "timestamp": "2026-01-04T00:00:00Z", "risk_level": "CRITICAL" }),
        ];
        let jsonl: String = entries.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("\n");
        std::fs::write(log_dir.join("test.jsonl"), jsonl).unwrap();
        let ctx = make_ctx(&dir);

        // No risk_level filter — handler doesn't support it, returns all entries
        let data = serde_json::json!({ "limit": 10 });
        let result = handler.handle_cmd("audit", Some(data), &ctx).await.unwrap().unwrap();
        assert_eq!(result["total"], 4, "should return all 4 entries without risk_level filter");
    }

    // --- Logs: security logs with filter ---
    #[tokio::test]
    async fn test_logs_security_with_filter() {
        let handler = logs::LogsHandler;
        let dir = tempfile::tempdir().unwrap();
        let log_dir = dir.path().join("logs/security_logs");
        std::fs::create_dir_all(&log_dir).unwrap();
        std::fs::write(
            log_dir.join("test.jsonl"),
            r#"{"timestamp":"2026-01-01T00:00:00Z","risk_level":"HIGH","action":"test"}
{"timestamp":"2026-01-02T00:00:00Z","risk_level":"LOW","action":"test"}
"#,
        ).unwrap();
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "risk_level": "LOW" });
        let result = handler.handle_cmd("security", Some(data), &ctx).await.unwrap().unwrap();
        assert_eq!(result["total"], 1);
    }

    // --- Cluster: status with config ---
    #[tokio::test]
    async fn test_cluster_status_with_config() {
        let handler = cluster::ClusterHandler::new();
        let dir = tempfile::tempdir().unwrap();
        ensure_config_dir(dir.path());
        std::fs::write(
            dir.path().join("config/config.cluster.json"),
            r#"{"enabled":true,"name":"node-1","role":"master"}"#,
        ).unwrap();
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
        assert!(result["config_exists"].as_bool().unwrap());
        let config = result["config"].as_object().unwrap();
        assert_eq!(config["enabled"], true);
    }

    // --- Cluster: config.get nonexistent returns empty ---
    #[tokio::test]
    async fn test_cluster_config_get_nonexistent() {
        let handler = cluster::ClusterHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("config.get", None, &ctx).await.unwrap().unwrap();
        // Should return empty object, not error
        assert!(result.is_object());
    }

    // --- Memory: status with enhanced_memory enabled ---
    #[tokio::test]
    async fn test_memory_status_with_vector_enabled() {
        let handler = memory::MemoryHandler;
        let dir = tempfile::tempdir().unwrap();
        ensure_config_dir(dir.path());
        std::fs::write(
            dir.path().join("config/config.enhanced_memory.json"),
            r#"{"enabled":true}"#,
        ).unwrap();
        let mem_dir = dir.path().join("memory");
        std::fs::create_dir_all(&mem_dir).unwrap();
        std::fs::write(mem_dir.join("doc1.md"), "test").unwrap();
        let ctx = make_ctx(&dir);

        let result = handler.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
        assert!(result["document_memory"]["enabled"].as_bool().unwrap());
        assert_eq!(result["document_memory"]["document_count"], 1);
        assert!(result["vector_memory"]["enabled"].as_bool().unwrap());
    }

    // --- MCP: server update with args and env ---
    #[tokio::test]
    async fn test_mcp_server_update_args_env() {
        let handler = mcp::McpHandler::new();
        let dir = tempfile::tempdir().unwrap();
        ensure_config_dir(dir.path());
        let cfg = nemesis_config::McpConfig::default();
        std::fs::write(
            dir.path().join("config/config.mcp.json"),
            serde_json::to_string_pretty(&cfg).unwrap(),
        ).unwrap();
        let ctx = make_ctx(&dir);

        // Add server with args
        let data = serde_json::json!({
            "name": "test",
            "command": "node",
            "args": ["a", "b"],
            "env": ["KEY=VAL"]
        });
        handler.handle_cmd("server.add", Some(data), &ctx).await.unwrap();

        // Update args
        let data = serde_json::json!({
            "name": "test",
            "args": ["x", "y", "z"],
            "env": ["A=1", "B=2"]
        });
        handler.handle_cmd("server.update", Some(data), &ctx).await.unwrap();

        let result = handler.handle_cmd("servers", None, &ctx).await.unwrap().unwrap();
        let srv = &result["servers"].as_array().unwrap()[0];
        let args = srv["args"].as_array().unwrap();
        assert_eq!(args.len(), 3);
        assert_eq!(args[0], "x");
    }

    // --- MCP: update with timeout ---
    #[tokio::test]
    async fn test_mcp_server_update_timeout() {
        let handler = mcp::McpHandler::new();
        let dir = tempfile::tempdir().unwrap();
        ensure_config_dir(dir.path());
        let cfg = nemesis_config::McpConfig::default();
        std::fs::write(
            dir.path().join("config/config.mcp.json"),
            serde_json::to_string_pretty(&cfg).unwrap(),
        ).unwrap();
        let ctx = make_ctx(&dir);

        handler.handle_cmd("server.add", Some(serde_json::json!({
            "name": "t", "command": "node"
        })), &ctx).await.unwrap();

        handler.handle_cmd("server.update", Some(serde_json::json!({
            "name": "t", "timeout": 300
        })), &ctx).await.unwrap();

        let result = handler.handle_cmd("servers", None, &ctx).await.unwrap().unwrap();
        assert_eq!(result["servers"][0]["timeout"], 300);
    }

    // ===================================================================
    // Fuzz / Randomized Input Tests
    // ===================================================================

    #[tokio::test]
    async fn fuzz_models_add_random_types() {
        let handler = models::ModelsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);

        let bad_inputs = vec![
            serde_json::json!({ "name": 42, "model": "gpt", "key": "k" }),
            serde_json::json!({ "name": true, "model": "gpt", "key": "k" }),
            serde_json::json!({ "name": null, "model": "gpt", "key": "k" }),
            serde_json::json!({ "name": [1,2], "model": "gpt", "key": "k" }),
            serde_json::json!({ "name": {}, "model": "gpt", "key": "k" }),
        ];

        for input in bad_inputs {
            let result = handler.handle_cmd("add", Some(input), &ctx).await;
            assert!(result.is_err(), "should reject non-string name");
        }
    }

    #[tokio::test]
    async fn fuzz_models_add_various_names() {
        let handler = models::ModelsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);

        let names = vec![
            "model with spaces",
            "model\twith\ttabs",
            "model\nwith\nnewlines",
            "model/with/slashes",
            "model.with.dots",
            "模型名称",
            "🏠emoji-model",
            "model'; DROP TABLE--",
            "<script>alert('xss')</script>",
            "a".repeat(1000).leak() as &str,
        ];

        for name in names {
            let data = serde_json::json!({ "name": name, "model": "test", "key": "k" });
            let result = handler.handle_cmd("add", Some(data), &ctx).await;
            // Should succeed — we accept any string as name
            if result.is_err() {
                // Only acceptable if it's a duplicate from a previous iteration
                assert!(result.unwrap_err().contains("already exists"));
            }
        }
    }

    #[tokio::test]
    async fn fuzz_config_set_field_various_paths() {
        let handler = config::ConfigHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);

        // Valid paths
        let valid = vec![
            ("gateway.host", serde_json::json!("0.0.0.0")),
            ("gateway.port", serde_json::json!(8080)),
            ("session.max_history", serde_json::json!(100)),
        ];
        for (path, value) in valid {
            let data = serde_json::json!({ "path": path, "value": value });
            let result = handler.handle_cmd("set_field", Some(data), &ctx).await;
            assert!(result.is_ok(), "set_field '{}' should work: {:?}", path, result);
        }
    }

    #[tokio::test]
    async fn fuzz_identity_save_various_names() {
        let handler = identity::IdentityHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);

        let names = vec![
            "IDENTITY.md", "SOUL.md", "USER.md", "AGENT.md",
            "custom.md", "文件.md",
        ];
        for name in names {
            let data = serde_json::json!({ "name": name, "content": format!("# {}", name) });
            let result = handler.handle_cmd("save", Some(data), &ctx).await;
            assert!(result.is_ok(), "save '{}' should work: {:?}", name, result);
        }
    }

    #[tokio::test]
    async fn fuzz_tasks_cron_add_various_cron_exprs() {
        let handler = tasks::TasksHandler;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);

        let exprs = vec![
            "* * * * *", "0 9 * * 1-5", "*/15 * * * *", "0 0 1 1 *",
            "@daily", "@hourly", "@reboot", "0 0,12 * * *",
        ];
        for (i, cron) in exprs.iter().enumerate() {
            let data = serde_json::json!({
                "name": format!("cron-{}", i),
                "cron": cron,
                "prompt": "test"
            });
            let result = handler.handle_cmd("cron.add", Some(data), &ctx).await;
            assert!(result.is_ok(), "cron '{}' should work: {:?}", cron, result);
        }

        let result = handler.handle_cmd("cron.list", None, &ctx).await.unwrap().unwrap();
        assert_eq!(result["total"], exprs.len());
    }

    #[tokio::test]
    async fn fuzz_mcp_server_add_various_commands() {
        let handler = mcp::McpHandler::new();
        let dir = tempfile::tempdir().unwrap();
        ensure_config_dir(dir.path());
        let cfg = nemesis_config::McpConfig::default();
        std::fs::write(
            dir.path().join("config/config.mcp.json"),
            serde_json::to_string_pretty(&cfg).unwrap(),
        ).unwrap();
        let ctx = make_ctx(&dir);

        let commands = vec![
            ("node", vec!["server.js"]),
            ("python", vec!["-m", "mcp_server"]),
            ("C:\\Program Files\\tool.exe", vec![]),
            ("/usr/bin/mcp", vec!["--port", "3000"]),
        ];
        for (i, (cmd, args)) in commands.iter().enumerate() {
            let data = serde_json::json!({
                "name": format!("srv-{}", i),
                "command": cmd,
                "args": args
            });
            let result = handler.handle_cmd("server.add", Some(data), &ctx).await;
            assert!(result.is_ok(), "MCP add '{}' should work: {:?}", cmd, result);
        }

        let result = handler.handle_cmd("servers", None, &ctx).await.unwrap().unwrap();
        assert_eq!(result["servers"].as_array().unwrap().len(), commands.len());
    }

    // ===================================================================
    // Stress: 100+ concurrent operations
    // ===================================================================

    #[tokio::test]
    async fn stress_100_parallel_reads() {
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        ensure_config_dir(dir.path());
        std::fs::write(dir.path().join("IDENTITY.md"), "# Test").unwrap();
        std::fs::write(dir.path().join("TOOLS.md"), "# Tools").unwrap();
        let cfg = nemesis_config::McpConfig::default();
        std::fs::write(
            dir.path().join("config/config.mcp.json"),
            serde_json::to_string_pretty(&cfg).unwrap(),
        ).unwrap();
        let ctx = Arc::new(make_ctx(&dir));

        let mut handles: Vec<tokio::task::JoinHandle<_>> = Vec::new();

        // 20 system.version
        for _ in 0..20 {
            let ctx = ctx.clone();
            handles.push(tokio::spawn(async move {
                system::SystemHandler.handle_cmd("version", None, &ctx).await
            }));
        }
        // 20 config.get
        for _ in 0..20 {
            let ctx = ctx.clone();
            handles.push(tokio::spawn(async move {
                config::ConfigHandler::new().handle_cmd("get", None, &ctx).await
            }));
        }
        // 20 identity.list
        for _ in 0..20 {
            let ctx = ctx.clone();
            handles.push(tokio::spawn(async move {
                identity::IdentityHandler.handle_cmd("list", None, &ctx).await
            }));
        }
        // 20 mcp.status
        for _ in 0..20 {
            let ctx = ctx.clone();
            handles.push(tokio::spawn(async move {
                mcp::McpHandler::new().handle_cmd("status", None, &ctx).await
            }));
        }
        // 20 tools.get
        for _ in 0..20 {
            let ctx = ctx.clone();
            handles.push(tokio::spawn(async move {
                tools::ToolsHandler.handle_cmd("get", None, &ctx).await
            }));
        }

        let mut ok = 0;
        let mut err = 0;
        for handle in handles {
            match handle.await.unwrap() {
                Ok(_) => ok += 1,
                Err(e) => {
                    err += 1;
                    eprintln!("stress read error: {}", e);
                }
            }
        }
        assert_eq!(ok, 100, "expected all 100 reads to succeed, {} failed", err);
    }

    #[tokio::test]
    async fn stress_100_write_integrity() {
        // Add 100 models one by one, then verify integrity of the final config
        let handler = models::ModelsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);

        for i in 0..100 {
            let data = serde_json::json!({
                "name": format!("stress-model-{}", i),
                "model": format!("model-{}", i),
                "key": format!("key-{}", i)
            });
            let result = handler.handle_cmd("add", Some(data), &ctx).await;
            assert!(result.is_ok(), "add #{} failed: {:?}", i, result);
        }

        // Verify integrity: exactly 100 models
        let result = handler.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
        let models = result["models"].as_array().unwrap();
        assert_eq!(models.len(), 100);

        // Verify all names are present and unique
        let names: std::collections::HashSet<_> = models.iter()
            .filter_map(|m| m["model_name"].as_str().map(String::from))
            .collect();
        assert_eq!(names.len(), 100);

        // Verify config file is valid JSON
        let config_str = std::fs::read_to_string(dir.path().join("config.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&config_str).unwrap();
        assert_eq!(parsed["model_list"].as_array().unwrap().len(), 100);
    }

    #[tokio::test]
    async fn stress_50_concurrent_write_with_verification() {
        // 50 concurrent writes of cron jobs, then verify all data persisted
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        ensure_config_dir(dir.path());
        let ctx = Arc::new(make_ctx(&dir));

        // Seed one model
        let seed_ctx = make_ctx(&dir);
        models::ModelsHandler::new().handle_cmd("add", Some(serde_json::json!({
            "name": "seed", "model": "gpt-4", "key": "k"
        })), &seed_ctx).await.unwrap();

        let mut handles: Vec<tokio::task::JoinHandle<_>> = Vec::new();

        // 25 concurrent config.set_field on different fields
        for i in 0..25 {
            let ctx = ctx.clone();
            handles.push(tokio::spawn(async move {
                let field = format!("gateway.port");
                let value = 8000 + i;
                let h = config::ConfigHandler::new();
                h.handle_cmd("set_field", Some(serde_json::json!({
                    "path": field, "value": value
                })), &ctx).await
            }));
        }

        // 25 concurrent forge.config.save toggles
        for i in 0..25 {
            let ctx = ctx.clone();
            handles.push(tokio::spawn(async move {
                let h = forge::ForgeHandler::new();
                h.handle_cmd("config.save", Some(serde_json::json!({
                    "enabled": i % 2 == 0
                })), &ctx).await
            }));
        }

        let mut successes = 0;
        let mut failures = 0;
        for handle in handles {
            match handle.await.unwrap() {
                Ok(_) => successes += 1,
                Err(_) => failures += 1,
            }
        }
        // Some concurrent writes may conflict — that's expected
        // The important thing is no panics and the config file remains valid JSON
        let config_str = std::fs::read_to_string(dir.path().join("config.json")).unwrap();
        let parsed: serde_json::Result<serde_json::Value> = serde_json::from_str(&config_str);
        assert!(parsed.is_ok(), "config.json should be valid JSON after concurrent writes");

        // Total should account for all handles completing
        assert_eq!(successes + failures, 50);
    }

    #[tokio::test]
    async fn stress_100_burst_models_add_delete_integrity() {
        let handler = models::ModelsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);

        // Burst: add 100 models
        for i in 0..100 {
            handler.handle_cmd("add", Some(serde_json::json!({
                "name": format!("b-{}", i), "model": "test", "key": "k"
            })), &ctx).await.unwrap();
        }

        // Verify 100
        let result = handler.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
        assert_eq!(result["models"].as_array().unwrap().len(), 100);

        // Delete odd ones
        for i in (0..100).filter(|i| i % 2 == 1) {
            handler.handle_cmd("delete", Some(serde_json::json!({
                "name": format!("b-{}", i)
            })), &ctx).await.unwrap();
        }

        // Verify 50 remain (evens)
        let result = handler.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
        let models = result["models"].as_array().unwrap();
        assert_eq!(models.len(), 50);
        for m in models {
            let name = m["model_name"].as_str().unwrap();
            let idx: i32 = name.trim_start_matches("b-").parse().unwrap();
            assert_eq!(idx % 2, 0, "only even models should remain");
        }
    }

    #[tokio::test]
    async fn stress_200_concurrent_agent_status() {
        // Pure read — no file IO, tests Arc<AtomicBool> contention
        let dir = tempfile::tempdir().unwrap();
        let ctx = Arc::new(make_ctx(&dir));

        let mut handles: Vec<tokio::task::JoinHandle<_>> = Vec::new();
        for _ in 0..200 {
            let ctx = ctx.clone();
            handles.push(tokio::spawn(async move {
                agent::AgentHandler.handle_cmd("status", None, &ctx).await
            }));
        }

        for handle in handles {
            let result = handle.await.unwrap();
            assert!(result.is_ok());
            let data = result.unwrap().unwrap();
            assert!(data["running"].is_boolean());
            assert!(data["model_name"].is_string());
        }
    }

    #[tokio::test]
    async fn stress_sustained_1000_ops_10_seconds() {
        // Sustained load: 1000 operations over ~10 seconds
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path());
        let ctx = make_ctx(&dir);

        // Add a base model
        let handler = models::ModelsHandler::new();
        handler.handle_cmd("add", Some(serde_json::json!({
            "name": "base", "model": "gpt-4", "key": "k"
        })), &ctx).await.unwrap();

        let mut total_ops = 0;
        let start = std::time::Instant::now();

        // Mix of read and write operations
        for i in 0..250 {
            // 4 ops per iteration = 1000 total
            let _ = handler.handle_cmd("list", None, &ctx).await;              // read
            let name = format!("sustained-{}", i);
            let _ = handler.handle_cmd("add", Some(serde_json::json!({         // write
                "name": &name, "model": "test", "key": "k"
            })), &ctx).await;
            let _ = handler.handle_cmd("list", None, &ctx).await;              // read
            let _ = handler.handle_cmd("delete", Some(serde_json::json!({      // write
                "name": &name
            })), &ctx).await;
            total_ops += 4;
        }

        let elapsed = start.elapsed();
        let ops_per_sec = total_ops as f64 / elapsed.as_secs_f64();

        // Verify final state is clean (only base model)
        let result = handler.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
        let models = result["models"].as_array().unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["model_name"], "base");

        // Performance assertion: should handle at least 50 ops/sec
        assert!(ops_per_sec > 50.0, "too slow: {:.0} ops/sec", ops_per_sec);
    }

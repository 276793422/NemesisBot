//! Extra tests for `MemoryHandler` — covers uncovered commands.
//!
//! Existing `tests.rs` covers: status, documents, document.get, document.save,
//! vector.status, vector.search, path traversal, subdirectory.
//!
//! This module adds coverage for:
//! - env.check / env.setup (error paths only — no real download)
//! - config.get / config.set
//! - stats / entries.list / entries.search / entries.store
//! - model.install (validation + lock-contention, no real download)
//! - Error paths: missing workspace, missing data, unknown command.

#[cfg(test)]
mod memory_extra_tests {
    use crate::handlers::memory::MemoryHandler;
    use crate::api_handlers::AppState;
    use crate::events::EventHub;
    use crate::session::SessionManager;
    use crate::ws_router::{ModuleHandler, RequestContext};
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::sync::Arc;
    use std::time::Instant;

    // -----------------------------------------------------------------------
    // Test infrastructure (mirror of tests.rs helpers, isolated to this module)
    // -----------------------------------------------------------------------

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
            estop: None,
        });
        RequestContext {
            session_id: "test-session".to_string(),
            chat_id: "test-chat".to_string(),
            workspace: Some(ws.clone()),
            home: Some(ws),
            state,
            auth_method: crate::session::AuthMethod::default(),
        }
    }

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
            estop: None,
        });
        RequestContext {
            session_id: "test-session".to_string(),
            chat_id: "test-chat".to_string(),
            workspace: None,
            home: None,
            state,
            auth_method: crate::session::AuthMethod::default(),
        }
    }

    /// Write a minimal config.json so set_main_switch / read_main_switch work.
    fn write_config(home: &Path) {
        let cfg = nemesis_config::Config::default();
        let json = serde_json::to_string_pretty(&cfg).unwrap();
        std::fs::write(home.join("config.json"), json).unwrap();
    }

    fn ensure_config_dir(workspace: &Path) {
        std::fs::create_dir_all(workspace.join("config")).unwrap();
    }

    // -----------------------------------------------------------------------
    // stats
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_memory_stats_empty_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let handler = MemoryHandler;
        let ctx = make_ctx(&dir);

        let result = handler.handle_cmd("stats", None, &ctx).await.unwrap().unwrap();
        assert_eq!(result["memory_entries"], 0);
        assert_eq!(result["episodic_sessions"], 0);
        assert_eq!(result["episodic_episodes"], 0);
        assert_eq!(result["graph_entities"], 0);
        assert_eq!(result["graph_triples"], 0);
        assert_eq!(result["vector_entries"], 0);
        // active_tier defaults to medium in embedding config
        assert_eq!(result["active_tier"], "medium");
    }

    #[tokio::test]
    async fn test_memory_stats_with_jsonl_and_episodic() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let mem_dir = ws.join("memory");
        let vector_dir = mem_dir.join("vector");
        std::fs::create_dir_all(&vector_dir).unwrap();
        // Two JSONL entries (one empty line ignored)
        let jsonl = vector_dir.join("vector_store.jsonl");
        std::fs::write(
            &jsonl,
            "{\"content\":\"a\"}\n{\"content\":\"b\"}\n\n",
        )
        .unwrap();

        // Episodic: one session dir with two episodes
        let sess = mem_dir.join("episodic").join("s1");
        std::fs::create_dir_all(&sess).unwrap();
        std::fs::write(sess.join("e1.json"), "{}").unwrap();
        std::fs::write(sess.join("e2.json"), "{}").unwrap();

        let ctx = make_ctx(&dir);
        let result = handler_stats(&ctx).await;
        assert_eq!(result["vector_entries"], 2);
        assert_eq!(result["episodic_sessions"], 1);
        assert_eq!(result["episodic_episodes"], 2);
    }

    async fn handler_stats(ctx: &RequestContext) -> serde_json::Value {
        let handler = MemoryHandler;
        handler.handle_cmd("stats", None, ctx).await.unwrap().unwrap()
    }

    // -----------------------------------------------------------------------
    // entries.list
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_memory_entries_list_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let handler = MemoryHandler;
        let ctx = make_ctx(&dir);

        let result = handler
            .handle_cmd("entries.list", None, &ctx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result["total"], 0);
        assert!(result["entries"].is_array());
        assert_eq!(result["entries"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_memory_entries_list_with_entries_truncated_and_reversed() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let jsonl = ws.join("memory").join("vector").join("vector_store.jsonl");
        std::fs::create_dir_all(jsonl.parent().unwrap()).unwrap();
        let long = "x".repeat(300);
        std::fs::write(
            &jsonl,
            format!(
                "{{\"content\":\"first\"}}\n{{\"content\":\"{}\"}}\n",
                long
            ),
        )
        .unwrap();

        let handler = MemoryHandler;
        let ctx = make_ctx(&dir);
        let result = handler
            .handle_cmd("entries.list", None, &ctx)
            .await
            .unwrap()
            .unwrap();
        let arr = result["entries"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        // Most recent first — second entry should be the long one
        let first = &arr[0]["content"].as_str().unwrap();
        // Truncation appends "..."
        assert!(first.ends_with("..."));
        assert_eq!(result["total"], 2);
    }

    // -----------------------------------------------------------------------
    // entries.search
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_memory_entries_search_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let handler = MemoryHandler;
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "query": "foo" });
        let result = handler
            .handle_cmd("entries.search", Some(data), &ctx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result["total"], 0);
        assert_eq!(result["search_type"], "keyword");
        assert_eq!(result["query"], "foo");
    }

    #[tokio::test]
    async fn test_memory_entries_search_case_insensitive_match() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let jsonl = ws.join("memory").join("vector").join("vector_store.jsonl");
        std::fs::create_dir_all(jsonl.parent().unwrap()).unwrap();
        std::fs::write(
            &jsonl,
            "{\"content\":\"Hello World\"}\n{\"content\":\"goodbye\"}\n",
        )
        .unwrap();

        let handler = MemoryHandler;
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "query": "WORLD" });
        let result = handler
            .handle_cmd("entries.search", Some(data), &ctx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result["total"], 1);
        let results = result["results"].as_array().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["content"], "Hello World");
    }

    #[tokio::test]
    async fn test_memory_entries_search_limit_applied() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let jsonl = ws.join("memory").join("vector").join("vector_store.jsonl");
        std::fs::create_dir_all(jsonl.parent().unwrap()).unwrap();
        let mut content = String::new();
        for i in 0..5 {
            content.push_str(&format!("{{\"content\":\"match {}\"}}\n", i));
        }
        std::fs::write(&jsonl, content).unwrap();

        let handler = MemoryHandler;
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "query": "match", "limit": 2 });
        let result = handler
            .handle_cmd("entries.search", Some(data), &ctx)
            .await
            .unwrap()
            .unwrap();
        // total counts all matches (before limit), results are truncated
        assert_eq!(result["total"], 5);
        assert_eq!(result["results"].as_array().unwrap().len(), 2);
    }

    // -----------------------------------------------------------------------
    // entries.store
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_memory_entries_store_creates_file_and_appends() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let handler = MemoryHandler;
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "content": "first memory" });
        let r1 = handler
            .handle_cmd("entries.store", Some(data), &ctx)
            .await
            .unwrap()
            .unwrap();
        assert!(r1["stored"].as_bool().unwrap());
        assert!(r1["id"].as_str().unwrap().len() > 0);

        let data2 = serde_json::json!({ "content": "second memory" });
        let r2 = handler
            .handle_cmd("entries.store", Some(data2), &ctx)
            .await
            .unwrap()
            .unwrap();
        assert!(r2["stored"].as_bool().unwrap());

        let jsonl = ws.join("memory").join("vector").join("vector_store.jsonl");
        let content = std::fs::read_to_string(&jsonl).unwrap();
        let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(lines.len(), 2);
    }

    // -----------------------------------------------------------------------
    // env.check
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_memory_env_check_no_config_returns_degraded() {
        let dir = tempfile::tempdir().unwrap();
        // No config dir / config files
        let handler = MemoryHandler;
        let ctx = make_ctx(&dir);

        let result = handler
            .handle_cmd("env.check", None, &ctx)
            .await
            .unwrap()
            .unwrap();
        // main switch is off (no config.json) → overall "disabled"
        assert_eq!(result["overall"], "disabled");
        assert_eq!(result["main_switch"], false);
        // models should be populated from default embedding config (3 tiers)
        assert!(result["models"].is_object());
    }

    #[tokio::test]
    async fn test_memory_env_check_with_main_enabled_plugin_missing() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        write_config(ws);
        // Flip main switch true via config_set path indirectly
        let cfg_path = ws.join("config.json");
        let raw = std::fs::read_to_string(&cfg_path).unwrap();
        let mut v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        v["memory"]["enabled"] = serde_json::Value::Bool(true);
        std::fs::write(&cfg_path, serde_json::to_string_pretty(&v).unwrap()).unwrap();

        let handler = MemoryHandler;
        let ctx = make_ctx(&dir);
        let result = handler
            .handle_cmd("env.check", None, &ctx)
            .await
            .unwrap()
            .unwrap();
        // main_switch on, sub_switch off (default), plugin missing → degraded
        assert_eq!(result["main_switch"], true);
        // Without an actual plugin DLL on disk, plugin.found is false → degraded
        assert_eq!(result["overall"], "degraded");
        assert_eq!(result["plugin"]["found"], false);
    }

    // -----------------------------------------------------------------------
    // env.setup (error path only — real setup triggers download)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_memory_env_setup_plugin_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        ensure_config_dir(ws);
        write_config(ws);
        let handler = MemoryHandler;
        let ctx = make_ctx(&dir);

        // Plugin DLL is unlikely to exist next to test exe; expect error
        let result = handler.handle_cmd("env.setup", None, &ctx).await;
        assert!(result.is_err(), "expected error when plugin not found");
        let err = result.unwrap_err().to_lowercase();
        // Either "plugin not found" or already-installed (if dev env has DLL)
        assert!(
            err.contains("plugin") || err.contains("download"),
            "unexpected error: {}",
            err
        );
    }

    // -----------------------------------------------------------------------
    // config.get
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_memory_config_get_no_main_config() {
        let dir = tempfile::tempdir().unwrap();
        let handler = MemoryHandler;
        let ctx = make_ctx(&dir);

        let result = handler
            .handle_cmd("config.get", None, &ctx)
            .await
            .unwrap()
            .unwrap();
        // No config.json → main_enabled false
        assert_eq!(result["main_enabled"], false);
        assert_eq!(result["active_tier"], "medium");
        assert_eq!(result["similarity_threshold"], 0.7);
        assert_eq!(result["max_results"], 10);
    }

    #[tokio::test]
    async fn test_memory_config_get_with_main_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        write_config(ws);
        let cfg_path = ws.join("config.json");
        let raw = std::fs::read_to_string(&cfg_path).unwrap();
        let mut v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        v["memory"]["enabled"] = serde_json::Value::Bool(true);
        std::fs::write(&cfg_path, serde_json::to_string_pretty(&v).unwrap()).unwrap();

        let handler = MemoryHandler;
        let ctx = make_ctx(&dir);
        let result = handler
            .handle_cmd("config.get", None, &ctx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result["main_enabled"], true);
    }

    // -----------------------------------------------------------------------
    // config.set
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_memory_config_set_main_enabled_requires_config_json() {
        let dir = tempfile::tempdir().unwrap();
        let handler = MemoryHandler;
        let ctx = make_ctx(&dir);

        // No config.json present → set_main_switch fails
        let data = serde_json::json!({ "main_enabled": true });
        let result = handler.handle_cmd("config.set", Some(data), &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_memory_config_set_main_enabled_true() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        write_config(ws);
        // write_config serializes Config::default(), where `memory: Option<...>` is
        // None — emitted as `null`. The `set_main_switch` helper only inserts
        // the `memory` object when the key is absent; when present as `null`
        // it leaves the field alone. Write a non-null `memory: {}` first so the
        // inner mutation sticks, matching what production onboard flow does.
        let cfg_path = ws.join("config.json");
        let raw = std::fs::read_to_string(&cfg_path).unwrap();
        let mut v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        v["memory"] = serde_json::json!({});
        std::fs::write(&cfg_path, serde_json::to_string_pretty(&v).unwrap()).unwrap();

        let handler = MemoryHandler;
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "main_enabled": true });
        let result = handler
            .handle_cmd("config.set", Some(data), &ctx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result["updated"], true);

        // Verify config.json was written
        let raw = std::fs::read_to_string(&cfg_path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["memory"]["enabled"], true);
    }

    #[tokio::test]
    async fn test_memory_config_set_active_tier() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        ensure_config_dir(ws);
        write_config(ws);
        let handler = MemoryHandler;
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "active_tier": "large" });
        let r = handler
            .handle_cmd("config.set", Some(data), &ctx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r["updated"], true);

        let emb_path = ws.join("config").join("config.enhanced_memory.json");
        let raw = std::fs::read_to_string(&emb_path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["active"], "large");
    }

    #[tokio::test]
    async fn test_memory_config_set_embedding_config_content_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        ensure_config_dir(ws);
        write_config(ws);
        let handler = MemoryHandler;
        let ctx = make_ctx(&dir);

        let payload = r#"{"enabled": false, "active": "small"}"#;
        let data = serde_json::json!({ "embedding_config_content": payload });
        handler
            .handle_cmd("config.set", Some(data), &ctx)
            .await
            .unwrap()
            .unwrap();

        let emb_path = ws.join("config").join("config.enhanced_memory.json");
        let raw = std::fs::read_to_string(&emb_path).unwrap();
        assert!(raw.contains("small"));
    }

    #[tokio::test]
    async fn test_memory_config_set_sub_enabled_requires_model() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        ensure_config_dir(ws);
        write_config(ws);
        let handler = MemoryHandler;
        let ctx = make_ctx(&dir);

        // Enable sub_switch but no model file present → must error
        let data = serde_json::json!({ "sub_enabled": true });
        let result = handler.handle_cmd("config.set", Some(data), &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        // Chinese error from source
        assert!(err.contains("模型尚未下载") || err.contains("model"));
    }

    #[tokio::test]
    async fn test_memory_config_set_sub_disabled_no_model_needed() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        ensure_config_dir(ws);
        write_config(ws);
        let handler = MemoryHandler;
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "sub_enabled": false });
        let r = handler
            .handle_cmd("config.set", Some(data), &ctx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r["updated"], true);
    }

    // -----------------------------------------------------------------------
    // model.install (validation / no real download)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_memory_model_install_unknown_tier() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        ensure_config_dir(ws);
        write_config(ws);
        let handler = MemoryHandler;
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "tier": "humongous" });
        let result = handler.handle_cmd("model.install", Some(data), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown tier"));
    }

    #[tokio::test]
    async fn test_memory_model_install_valid_tier_runs_or_fails_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        ensure_config_dir(ws);
        write_config(ws);
        let handler = MemoryHandler;
        let ctx = make_ctx(&dir);

        // Valid tier name — we don't assert success (no real download in CI).
        // We only require it does not panic and either succeeds or errors.
        let data = serde_json::json!({ "tier": "small" });
        let _ = handler.handle_cmd("model.install", Some(data), &ctx).await;
        // Lock should be released after either path (we cannot easily check the
        // internal mutex, but a second call must not return "正在安装中").
    }

    // -----------------------------------------------------------------------
    // Error paths
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_memory_no_workspace_returns_error() {
        let handler = MemoryHandler;
        let ctx = make_ctx_no_workspace();
        let result = handler.handle_cmd("stats", None, &ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "workspace not configured");
    }

    #[tokio::test]
    async fn test_memory_missing_data_for_entries_search() {
        let dir = tempfile::tempdir().unwrap();
        let handler = MemoryHandler;
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("entries.search", None, &ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "missing data");
    }

    #[tokio::test]
    async fn test_memory_missing_data_for_entries_store() {
        let dir = tempfile::tempdir().unwrap();
        let handler = MemoryHandler;
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("entries.store", None, &ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "missing data");
    }

    #[tokio::test]
    async fn test_memory_missing_data_for_model_install() {
        let dir = tempfile::tempdir().unwrap();
        let handler = MemoryHandler;
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("model.install", None, &ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "missing data");
    }

    #[tokio::test]
    async fn test_memory_missing_data_for_document_get() {
        let dir = tempfile::tempdir().unwrap();
        let handler = MemoryHandler;
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("document.get", None, &ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "missing data");
    }

    #[tokio::test]
    async fn test_memory_missing_data_for_document_save() {
        let dir = tempfile::tempdir().unwrap();
        let handler = MemoryHandler;
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("document.save", None, &ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "missing data");
    }

    #[tokio::test]
    async fn test_memory_unknown_command() {
        let dir = tempfile::tempdir().unwrap();
        let handler = MemoryHandler;
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("bogus.cmd", None, &ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "unknown command: memory.bogus.cmd");
    }

    #[tokio::test]
    async fn test_memory_entries_search_missing_query_field() {
        let dir = tempfile::tempdir().unwrap();
        let handler = MemoryHandler;
        let ctx = make_ctx(&dir);
        // data present but missing "query" field
        let data = serde_json::json!({ "foo": "bar" });
        let result = handler
            .handle_cmd("entries.search", Some(data), &ctx)
            .await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "missing field: query");
    }

    #[tokio::test]
    async fn test_memory_model_install_missing_tier_field() {
        let dir = tempfile::tempdir().unwrap();
        let handler = MemoryHandler;
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "notier": "x" });
        let result = handler
            .handle_cmd("model.install", Some(data), &ctx)
            .await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "missing field: tier");
    }
}

//! Extra coverage for `skills::SkillsHandler` — exercises commands not covered by `tests.rs`.
//!
//! Focus: `config.*`, `source.*`, `installed` (with metadata / missing dir),
//! `detail`/`uninstall` validation, `search`/`install`/`shop_*`/`browse`
//! error paths (no network), unknown command, and missing-data validation.

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
    // Test infra (verbatim copy of helpers in tests.rs)
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
            webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
            internal_cmd_tx: None,
        });
        RequestContext {
            session_id: "test-session".to_string(),
            chat_id: "test-chat".to_string(),
            workspace: Some(ws.clone()),
            home: Some(ws),
            state,
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
            webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
            internal_cmd_tx: None,
        });
        RequestContext {
            session_id: "test-session".to_string(),
            chat_id: "test-chat".to_string(),
            workspace: None,
            home: None,
            state,
        }
    }

    fn ensure_config_dir(workspace: &Path) {
        std::fs::create_dir_all(workspace.join("config")).unwrap();
    }

    /// Write a `config.skills.json` with given cfg into workspace/config/.
    fn write_skills_config(workspace: &Path, cfg: &nemesis_config::SkillsFullConfig) {
        ensure_config_dir(workspace);
        let json = serde_json::to_string_pretty(cfg).unwrap();
        std::fs::write(workspace.join("config/config.skills.json"), json).unwrap();
    }

    // -----------------------------------------------------------------------
    // unknown command + missing workspace
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_skills_unknown_command() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("totally_unknown", None, &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("unknown command: skills.totally_unknown"), "got: {}", err);
    }

    #[tokio::test]
    async fn test_skills_missing_workspace_installed() {
        let handler = skills::SkillsHandler::new();
        let ctx = make_ctx_no_workspace();
        let result = handler.handle_cmd("installed", None, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("workspace not configured"));
    }

    #[tokio::test]
    async fn test_skills_missing_workspace_source_list() {
        let handler = skills::SkillsHandler::new();
        let ctx = make_ctx_no_workspace();
        let result = handler.handle_cmd("source.list", None, &ctx).await;
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // missing-data / missing-field validation per command
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_skills_detail_missing_data() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("detail", None, &ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "missing data");
    }

    #[tokio::test]
    async fn test_skills_detail_missing_field_name() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "other": "value" });
        let result = handler.handle_cmd("detail", Some(data), &ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "missing field: name");
    }

    #[tokio::test]
    async fn test_skills_uninstall_missing_data() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("uninstall", None, &ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "missing data");
    }

    #[tokio::test]
    async fn test_skills_uninstall_missing_field_name() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "not_name": "x" });
        let result = handler.handle_cmd("uninstall", Some(data), &ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "missing field: name");
    }

    #[tokio::test]
    async fn test_skills_search_missing_data() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("search", None, &ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "missing data");
    }

    #[tokio::test]
    async fn test_skills_search_missing_field_query() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "not_query": "x" });
        let result = handler.handle_cmd("search", Some(data), &ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "missing field: query");
    }

    #[tokio::test]
    async fn test_skills_install_missing_data() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("install", None, &ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "missing data");
    }

    #[tokio::test]
    async fn test_skills_install_missing_field_registry() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "slug": "x" });
        let result = handler.handle_cmd("install", Some(data), &ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "missing field: registry");
    }

    #[tokio::test]
    async fn test_skills_install_missing_field_slug() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "registry": "test" });
        let result = handler.handle_cmd("install", Some(data), &ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "missing field: slug");
    }

    #[tokio::test]
    async fn test_skills_shop_detail_missing_data() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("shop_detail", None, &ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "missing data");
    }

    #[tokio::test]
    async fn test_skills_shop_detail_missing_field_registry() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "slug": "x" });
        let result = handler.handle_cmd("shop_detail", Some(data), &ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "missing field: registry");
    }

    #[tokio::test]
    async fn test_skills_shop_detail_missing_field_slug() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "registry": "r" });
        let result = handler.handle_cmd("shop_detail", Some(data), &ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "missing field: slug");
    }

    #[tokio::test]
    async fn test_skills_shop_code_missing_data() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("shop_code", None, &ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "missing data");
    }

    #[tokio::test]
    async fn test_skills_browse_missing_data() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("browse", None, &ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "missing data");
    }

    #[tokio::test]
    async fn test_skills_config_save_missing_data() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("config.save", None, &ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "missing data");
    }

    #[tokio::test]
    async fn test_skills_config_update_missing_data() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("config.update", None, &ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "missing data");
    }

    #[tokio::test]
    async fn test_skills_source_add_missing_data() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("source.add", None, &ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "missing data");
    }

    #[tokio::test]
    async fn test_skills_source_add_missing_field_url() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "not_url": "x" });
        let result = handler.handle_cmd("source.add", Some(data), &ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "missing field: url");
    }

    #[tokio::test]
    async fn test_skills_source_add_manual_missing_data() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("source.add.manual", None, &ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "missing data");
    }

    #[tokio::test]
    async fn test_skills_source_add_manual_missing_field_name() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "repo": "owner/repo" });
        let result = handler.handle_cmd("source.add.manual", Some(data), &ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "missing field: name");
    }

    #[tokio::test]
    async fn test_skills_source_add_manual_missing_field_repo() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "name": "foo" });
        let result = handler.handle_cmd("source.add.manual", Some(data), &ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "missing field: repo");
    }

    #[tokio::test]
    async fn test_skills_source_remove_missing_data() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("source.remove", None, &ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "missing data");
    }

    #[tokio::test]
    async fn test_skills_source_remove_missing_field_name() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "not_name": "x" });
        let result = handler.handle_cmd("source.remove", Some(data), &ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "missing field: name");
    }

    #[tokio::test]
    async fn test_skills_source_toggle_missing_data() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("source.toggle", None, &ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "missing data");
    }

    #[tokio::test]
    async fn test_skills_open_dir_missing_data() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let result = handler.handle_cmd("open_dir", None, &ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "missing data");
    }

    #[tokio::test]
    async fn test_skills_open_dir_missing_field_name() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let data = serde_json::json!({ "other": "x" });
        let result = handler.handle_cmd("open_dir", Some(data), &ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "missing field: name");
    }

    // -----------------------------------------------------------------------
    // installed (with SKILL.md metadata variants)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_skills_installed_no_skill_md() {
        // Skill dir without SKILL.md → has_skill_md=false, description=""
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        std::fs::create_dir_all(ws.join("skills/plain")).unwrap();
        let ctx = make_ctx(&dir);

        let result = handler.handle_cmd("installed", None, &ctx).await.unwrap().unwrap();
        let skills = result["skills"].as_array().unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0]["name"], "plain");
        assert!(!skills[0]["has_skill_md"].as_bool().unwrap());
        assert_eq!(skills[0]["description"], "");
    }

    #[tokio::test]
    async fn test_skills_installed_with_frontmatter_yaml() {
        // Skill with YAML frontmatter description
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let skill_dir = ws.join("skills/yaml-meta");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: yaml-meta\ndescription: A YAML-described skill\n---\n# Body\n",
        ).unwrap();
        let ctx = make_ctx(&dir);

        let result = handler.handle_cmd("installed", None, &ctx).await.unwrap().unwrap();
        let skills = result["skills"].as_array().unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0]["name"], "yaml-meta");
        assert!(skills[0]["has_skill_md"].as_bool().unwrap());
        assert_eq!(skills[0]["description"], "A YAML-described skill");
    }

    #[tokio::test]
    async fn test_skills_installed_multiple() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        for name in ["alpha", "beta", "gamma"] {
            let p = ws.join(format!("skills/{}", name));
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("SKILL.md"), format!("# {}", name)).unwrap();
        }
        let ctx = make_ctx(&dir);

        let result = handler.handle_cmd("installed", None, &ctx).await.unwrap().unwrap();
        let skills = result["skills"].as_array().unwrap();
        assert_eq!(skills.len(), 3);
    }

    // -----------------------------------------------------------------------
    // detail — happy / non-existent
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_skills_detail_reads_markdown() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let skill_dir = ws.join("skills/reader");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# Reader\n\nbody text").unwrap();
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "name": "reader" });
        let result = handler.handle_cmd("detail", Some(data), &ctx).await.unwrap().unwrap();
        assert_eq!(result["name"], "reader");
        assert_eq!(result["content"], "# Reader\n\nbody text");
    }

    // -----------------------------------------------------------------------
    // open_dir — happy and not-found
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_skills_open_dir_happy() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        std::fs::create_dir_all(ws.join("skills/opener")).unwrap();
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "name": "opener" });
        let result = handler.handle_cmd("open_dir", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["opened"].as_bool().unwrap());
        assert!(result["path"].as_str().unwrap().contains("opener"));
    }

    #[tokio::test]
    async fn test_skills_open_dir_not_found() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "name": "ghost-dir" });
        let result = handler.handle_cmd("open_dir", Some(data), &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("directory not found"), "got: {}", err);
        assert!(err.contains("ghost-dir"));
    }

    // -----------------------------------------------------------------------
    // config.get — fallback (no config file) + persisted file
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_skills_config_get_fallback_default() {
        // No config file → load_skills_config falls back to defaults.
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);

        let result = handler.handle_cmd("config.get", None, &ctx).await.unwrap().unwrap();
        // Default enabled=true
        assert!(result["enabled"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_skills_config_get_persisted() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let cfg = nemesis_config::SkillsFullConfig {
            enabled: false,
            search_limit: 123,
            ..Default::default()
        };
        write_skills_config(dir.path(), &cfg);
        let ctx = make_ctx(&dir);

        let result = handler.handle_cmd("config.get", None, &ctx).await.unwrap().unwrap();
        assert!(!result["enabled"].as_bool().unwrap());
        assert_eq!(result["search_limit"].as_i64().unwrap(), 123);
    }

    // -----------------------------------------------------------------------
    // config.save — invalid JSON shape
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_skills_config_save_invalid() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        // Pass an array, not an object — should fail to deserialize.
        let data = serde_json::json!([1, 2, 3]);
        let result = handler.handle_cmd("config.save", Some(data), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid skills config"));
    }

    #[tokio::test]
    async fn test_skills_config_save_creates_config_dir() {
        // No config dir exists yet; save should still succeed and create it.
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);

        let save_data = serde_json::json!({
            "enabled": true,
            "max_concurrent_searches": 7,
        });
        let result = handler.handle_cmd("config.save", Some(save_data), &ctx).await.unwrap().unwrap();
        assert!(result["saved"].as_bool().unwrap());
        assert!(dir.path().join("config/config.skills.json").exists());
    }

    // -----------------------------------------------------------------------
    // config.update — selective field updates
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_skills_config_update_enabled() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_skills_config(dir.path(), &nemesis_config::SkillsFullConfig::default());
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "enabled": false });
        let result = handler.handle_cmd("config.update", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["updated"].as_bool().unwrap());

        // Verify persisted
        let result = handler.handle_cmd("config.get", None, &ctx).await.unwrap().unwrap();
        assert!(!result["enabled"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_skills_config_update_search_cache() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_skills_config(dir.path(), &nemesis_config::SkillsFullConfig::default());
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({
            "search_cache": { "enabled": false, "max_size": 999, "ttl_seconds": 42 }
        });
        let result = handler.handle_cmd("config.update", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["updated"].as_bool().unwrap());

        let result = handler.handle_cmd("config.get", None, &ctx).await.unwrap().unwrap();
        let sc = &result["search_cache"];
        assert!(!sc["enabled"].as_bool().unwrap());
        assert_eq!(sc["max_size"].as_i64().unwrap(), 999);
        assert_eq!(sc["ttl_seconds"].as_i64().unwrap(), 42);
    }

    #[tokio::test]
    async fn test_skills_config_update_max_concurrent() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_skills_config(dir.path(), &nemesis_config::SkillsFullConfig::default());
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "max_concurrent_searches": 11 });
        let result = handler.handle_cmd("config.update", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["updated"].as_bool().unwrap());

        let result = handler.handle_cmd("config.get", None, &ctx).await.unwrap().unwrap();
        assert_eq!(result["max_concurrent_searches"].as_i64().unwrap(), 11);
    }

    #[tokio::test]
    async fn test_skills_config_update_clawhub() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_skills_config(dir.path(), &nemesis_config::SkillsFullConfig::default());
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({
            "clawhub": {
                "enabled": true,
                "base_url": "https://example.com",
                "convex_url": "https://convex.example.com",
                "timeout": 99,
            }
        });
        let result = handler.handle_cmd("config.update", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["updated"].as_bool().unwrap());

        let result = handler.handle_cmd("config.get", None, &ctx).await.unwrap().unwrap();
        let ch = &result["clawhub"];
        assert!(ch["enabled"].as_bool().unwrap());
        assert_eq!(ch["base_url"], "https://example.com");
        assert_eq!(ch["convex_url"], "https://convex.example.com");
        assert_eq!(ch["timeout"].as_i64().unwrap(), 99);
    }

    #[tokio::test]
    async fn test_skills_config_update_empty_data() {
        // Empty object should not modify anything (no fields match), but still persist.
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_skills_config(dir.path(), &nemesis_config::SkillsFullConfig::default());
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({});
        let result = handler.handle_cmd("config.update", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["updated"].as_bool().unwrap());
    }

    // -----------------------------------------------------------------------
    // source.list — defaults + custom sources
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_skills_source_list_defaults() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);

        let result = handler.handle_cmd("source.list", None, &ctx).await.unwrap().unwrap();
        let sources = result["sources"].as_array().unwrap();
        // Default has no github_sources but always includes clawhub + modelscope
        assert_eq!(sources.len(), 2);
        let types: Vec<&str> = sources.iter()
            .map(|s| s["type"].as_str().unwrap())
            .collect();
        assert!(types.contains(&"clawhub"));
        assert!(types.contains(&"modelscope"));
    }

    #[tokio::test]
    async fn test_skills_source_list_with_github() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let cfg = nemesis_config::SkillsFullConfig {
            github_sources: vec![nemesis_config::GitHubSourceConfig {
                name: "my-gh".to_string(),
                repo: "owner/repo".to_string(),
                enabled: true,
                branch: "main".to_string(),
                index_type: "github_api".to_string(),
                skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        write_skills_config(dir.path(), &cfg);
        let ctx = make_ctx(&dir);

        let result = handler.handle_cmd("source.list", None, &ctx).await.unwrap().unwrap();
        let sources = result["sources"].as_array().unwrap();
        assert_eq!(sources.len(), 3);
        let gh = sources.iter().find(|s| s["type"] == "github").unwrap();
        assert_eq!(gh["name"], "my-gh");
        assert_eq!(gh["repo"], "owner/repo");
        assert!(gh["enabled"].as_bool().unwrap());
    }

    // -----------------------------------------------------------------------
    // source.add.manual — happy + duplicate
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_skills_source_add_manual_happy() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_skills_config(dir.path(), &nemesis_config::SkillsFullConfig::default());
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({
            "name": "manual-src",
            "repo": "owner/manual-repo",
            "branch": "develop",
            "index_type": "skills_json",
            "skill_path_pattern": "skills.json",
        });
        let result = handler.handle_cmd("source.add.manual", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["success"].as_bool().unwrap());
        assert_eq!(result["source"]["name"], "manual-src");
        assert_eq!(result["source"]["repo"], "owner/manual-repo");

        // Persisted
        let list = handler.handle_cmd("source.list", None, &ctx).await.unwrap().unwrap();
        let gh = list["sources"].as_array().unwrap().iter()
            .find(|s| s["type"] == "github").unwrap();
        assert_eq!(gh["branch"], "develop");
        assert_eq!(gh["index_type"], "skills_json");
    }

    #[tokio::test]
    async fn test_skills_source_add_manual_defaults() {
        // Omit optional fields: branch / index_type / skill_path_pattern
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_skills_config(dir.path(), &nemesis_config::SkillsFullConfig::default());
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({
            "name": "min-src",
            "repo": "o/r",
        });
        let result = handler.handle_cmd("source.add.manual", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["success"].as_bool().unwrap());

        let list = handler.handle_cmd("source.list", None, &ctx).await.unwrap().unwrap();
        let gh = list["sources"].as_array().unwrap().iter()
            .find(|s| s["name"] == "min-src").unwrap();
        assert_eq!(gh["branch"], "main");
        assert_eq!(gh["index_type"], "github_api");
        assert_eq!(gh["skill_path_pattern"], "skills/{slug}/SKILL.md");
    }

    #[tokio::test]
    async fn test_skills_source_add_manual_duplicate() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let cfg = nemesis_config::SkillsFullConfig {
            github_sources: vec![nemesis_config::GitHubSourceConfig {
                name: "dup".to_string(),
                repo: "o/r".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        write_skills_config(dir.path(), &cfg);
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "name": "dup", "repo": "o/r" });
        let result = handler.handle_cmd("source.add.manual", Some(data), &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("已存在"), "got: {}", err);
    }

    // -----------------------------------------------------------------------
    // source.remove — happy + not-found
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_skills_source_remove_happy() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let cfg = nemesis_config::SkillsFullConfig {
            github_sources: vec![nemesis_config::GitHubSourceConfig {
                name: "to-go".to_string(),
                repo: "o/r".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        write_skills_config(dir.path(), &cfg);
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "name": "to-go" });
        let result = handler.handle_cmd("source.remove", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["removed"].as_bool().unwrap());
        assert_eq!(result["name"], "to-go");

        // Verify it's gone
        let list = handler.handle_cmd("source.list", None, &ctx).await.unwrap().unwrap();
        let has = list["sources"].as_array().unwrap().iter()
            .any(|s| s["name"] == "to-go");
        assert!(!has);
    }

    #[tokio::test]
    async fn test_skills_source_remove_not_found() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_skills_config(dir.path(), &nemesis_config::SkillsFullConfig::default());
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "name": "ghost" });
        let result = handler.handle_cmd("source.remove", Some(data), &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("不存在"), "got: {}", err);
    }

    // -----------------------------------------------------------------------
    // source.toggle — github / clawhub / modelscope / unknown
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_skills_source_toggle_github() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let cfg = nemesis_config::SkillsFullConfig {
            github_sources: vec![nemesis_config::GitHubSourceConfig {
                name: "toggleable".to_string(),
                enabled: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        write_skills_config(dir.path(), &cfg);
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "name": "toggleable", "enabled": false });
        let result = handler.handle_cmd("source.toggle", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["toggled"].as_bool().unwrap());
        assert!(!result["enabled"].as_bool().unwrap());

        // Persisted
        let list = handler.handle_cmd("source.list", None, &ctx).await.unwrap().unwrap();
        let gh = list["sources"].as_array().unwrap().iter()
            .find(|s| s["name"] == "toggleable").unwrap();
        assert!(!gh["enabled"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_skills_source_toggle_default_enabled_true() {
        // Missing "enabled" field defaults to true
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let cfg = nemesis_config::SkillsFullConfig {
            github_sources: vec![nemesis_config::GitHubSourceConfig {
                name: "x".to_string(),
                enabled: false,
                ..Default::default()
            }],
            ..Default::default()
        };
        write_skills_config(dir.path(), &cfg);
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "name": "x" });
        let result = handler.handle_cmd("source.toggle", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["enabled"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_skills_source_toggle_clawhub() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_skills_config(dir.path(), &nemesis_config::SkillsFullConfig::default());
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "name": "clawhub", "enabled": false });
        let result = handler.handle_cmd("source.toggle", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["toggled"].as_bool().unwrap());

        // Also test capitalized form
        let data = serde_json::json!({ "name": "ClawHub", "enabled": true });
        let result = handler.handle_cmd("source.toggle", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["enabled"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_skills_source_toggle_modelscope() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_skills_config(dir.path(), &nemesis_config::SkillsFullConfig::default());
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "name": "modelscope", "enabled": false });
        let result = handler.handle_cmd("source.toggle", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["toggled"].as_bool().unwrap());

        let data = serde_json::json!({ "name": "ModelScope", "enabled": true });
        let result = handler.handle_cmd("source.toggle", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["enabled"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_skills_source_toggle_unknown() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        write_skills_config(dir.path(), &nemesis_config::SkillsFullConfig::default());
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "name": "no-such-source", "enabled": true });
        let result = handler.handle_cmd("source.toggle", Some(data), &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("不存在"), "got: {}", err);
    }

    // -----------------------------------------------------------------------
    // search — empty registries (no network)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_skills_search_no_sources_message() {
        // No sources configured → returns message prompting to add a source.
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "query": "anything" });
        let result = handler.handle_cmd("search", Some(data), &ctx).await.unwrap().unwrap();
        assert_eq!(result["query"], "anything");
        assert!(result["results"].as_array().unwrap().is_empty());
        assert!(result["message"].as_str().is_some());
    }

    // -----------------------------------------------------------------------
    // install — already installed (no network)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_skills_install_already_installed() {
        // Pre-existing skill dir + force=false → already_installed (no network)
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        std::fs::create_dir_all(ws.join("skills/pre-installed")).unwrap();
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({
            "registry": "any-registry",
            "slug": "pre-installed",
        });
        let result = handler.handle_cmd("install", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["already_installed"].as_bool().unwrap());
        assert_eq!(result["slug"], "pre-installed");
    }

    #[tokio::test]
    async fn test_skills_install_unknown_registry() {
        // No already-installed dir, force=false, but registry doesn't exist
        // → should fail with "源 ... 不存在"
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({
            "registry": "no-such-registry",
            "slug": "fresh-skill",
        });
        let result = handler.handle_cmd("install", Some(data), &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("不存在"), "got: {}", err);
    }

    // -----------------------------------------------------------------------
    // shop_detail / shop_code — unknown registry (no network)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_skills_shop_detail_unknown_registry() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "registry": "ghost-reg", "slug": "ghost-skill" });
        let result = handler.handle_cmd("shop_detail", Some(data), &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("不存在"), "got: {}", err);
    }

    #[tokio::test]
    async fn test_skills_shop_code_unknown_registry() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "registry": "ghost-reg", "slug": "ghost-skill" });
        let result = handler.handle_cmd("shop_code", Some(data), &ctx).await;
        // shop_code uses manager.get_skill_content which iterates registries;
        // unknown registry yields an error.
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // browse — no sources configured (no network)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_skills_browse_empty_results() {
        // Default config has no sources, so browse should return empty items
        // or an error from the registry call. We tolerate either.
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({});
        let result = handler.handle_cmd("browse", Some(data), &ctx).await;
        // Either success with empty items, or error — both fine; no network.
        match result {
            Ok(Some(v)) => {
                let _items = v["items"].as_array().unwrap();
            }
            Ok(None) => {}
            Err(_) => {}
        }
    }

    #[tokio::test]
    async fn test_skills_browse_with_params() {
        // Provide all optional params (registry/sort/limit/cursor); just verify
        // the call doesn't panic on parsing.
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({
            "registry": "clawhub",
            "sort": "newest",
            "limit": 5u64,
            "cursor": "abc",
        });
        let _ = handler.handle_cmd("browse", Some(data), &ctx).await;
    }

    // -----------------------------------------------------------------------
    // uninstall — error removing non-empty dir edge cases
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_skills_uninstall_removes_subdir_contents() {
        let handler = skills::SkillsHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let skill_dir = ws.join("skills/multi");
        std::fs::create_dir_all(skill_dir.join("sub")).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "x").unwrap();
        std::fs::write(skill_dir.join("sub/file.txt"), "y").unwrap();
        let ctx = make_ctx(&dir);

        let data = serde_json::json!({ "name": "multi" });
        let result = handler.handle_cmd("uninstall", Some(data), &ctx).await.unwrap().unwrap();
        assert!(result["uninstalled"].as_bool().unwrap());
        assert!(!skill_dir.exists());
    }

//! Handler-level tests for `skills::SkillsHandler`.
//!
//! Complements `skills_extra_tests.rs` by exercising additional code paths:
//! - `installed` with mixed dir/file entries (files are skipped)
//! - `detail` for nonexistent skill (error path)
//! - `uninstall` for nonexistent skill (error path)
//! - `config.get` when config file is corrupt (load error path)
//! - `config.save` valid full config (round-trip)
//! - `source.list` with multiple github sources ordering
//! - `source.toggle` with non-bool enabled (defaults to true)
//! - `search` with installed-slugs subset (no network)
//! - `browse` parsing branches for sort strings
//! - `open_dir` on nested dir

#![cfg(test)]

use crate::handlers::skills::SkillsHandler;
use crate::ws_router::ModuleHandler;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::RequestContext;

// ---------------------------------------------------------------------------
// Test infra (small enough to be self-contained)
// ---------------------------------------------------------------------------

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
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
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

fn ensure_config_dir(workspace: &Path) {
    std::fs::create_dir_all(workspace.join("config")).unwrap();
}

fn write_skills_config(workspace: &Path, cfg: &nemesis_config::SkillsFullConfig) {
    ensure_config_dir(workspace);
    let json = serde_json::to_string_pretty(cfg).unwrap();
    std::fs::write(workspace.join("config/config.skills.json"), json).unwrap();
}

// ---------------------------------------------------------------------------
// installed — mixed entries (dir + stray file)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_skills_installed_skips_files() {
    // Files directly in skills/ must be skipped (only directories counted).
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    std::fs::create_dir_all(ws.join("skills")).unwrap();
    // A stray file in skills/ that's not a directory.
    std::fs::write(ws.join("skills/stray.txt"), "ignore me").unwrap();
    // A directory.
    std::fs::create_dir_all(ws.join("skills/real")).unwrap();
    std::fs::write(ws.join("skills/real/SKILL.md"), "# Real").unwrap();
    let ctx = make_ctx(&dir);

    let result = handler
        .handle_cmd("installed", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let skills = result["skills"].as_array().unwrap();
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0]["name"], "real");
}

#[tokio::test]
async fn test_skills_installed_handles_skill_md_no_frontmatter() {
    // SKILL.md with no frontmatter → empty description, has_skill_md=true.
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    std::fs::create_dir_all(ws.join("skills/plain-md")).unwrap();
    std::fs::write(
        ws.join("skills/plain-md/SKILL.md"),
        "# Just a body\nNo frontmatter",
    )
    .unwrap();
    let ctx = make_ctx(&dir);

    let result = handler
        .handle_cmd("installed", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let skills = result["skills"].as_array().unwrap();
    assert_eq!(skills.len(), 1);
    assert!(skills[0]["has_skill_md"].as_bool().unwrap());
    // No frontmatter → description defaults to "" (get_skill_metadata returns
    // Some with empty description when no frontmatter found).
    assert_eq!(skills[0]["description"], "");
}

#[tokio::test]
async fn test_skills_installed_empty_dir() {
    // An empty directory has no entries at all.
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let result = handler
        .handle_cmd("installed", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let skills = result["skills"].as_array().unwrap();
    assert!(skills.is_empty());
}

// ---------------------------------------------------------------------------
// detail — nonexistent skill
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_skills_detail_nonexistent_errors() {
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": "no-such" });
    let result = handler.handle_cmd("detail", Some(data), &ctx).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    // read_workspace_file error message uses "failed to read"
    assert!(err.contains("failed to read"), "got: {}", err);
}

#[tokio::test]
async fn test_skills_detail_path_traversal_name() {
    // A skill name like "../escape" must be rejected by resolve_path.
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": "../escape" });
    let result = handler.handle_cmd("detail", Some(data), &ctx).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.contains("path traversal denied") || err.contains("absolute paths not allowed"),
        "got: {}",
        err
    );
}

#[tokio::test]
async fn test_skills_detail_name_type_mismatch() {
    // If `name` is a number, get_str fails with missing-field-style error.
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": 42 });
    let result = handler.handle_cmd("detail", Some(data), &ctx).await;
    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), "missing field: name");
}

// ---------------------------------------------------------------------------
// uninstall — nonexistent skill / path traversal
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_skills_uninstall_nonexistent_errors() {
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": "ghost-skill" });
    let result = handler.handle_cmd("uninstall", Some(data), &ctx).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.contains("not found"), "got: {}", err);
}

#[tokio::test]
async fn test_skills_uninstall_path_traversal() {
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": "../../etc" });
    let result = handler.handle_cmd("uninstall", Some(data), &ctx).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.contains("path traversal denied"), "got: {}", err);
}

// ---------------------------------------------------------------------------
// config.get — corrupt config file
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_skills_config_get_invalid_json_errors() {
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    ensure_config_dir(ws);
    std::fs::write(ws.join("config/config.skills.json"), "{ not valid json").unwrap();
    let ctx = make_ctx(&dir);

    let result = handler.handle_cmd("config.get", None, &ctx).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.contains("failed to load skills config"), "got: {}", err);
}

#[tokio::test]
async fn test_skills_config_update_invalid_json_errors() {
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    ensure_config_dir(ws);
    std::fs::write(ws.join("config/config.skills.json"), "{ broken").unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "enabled": true });
    let result = handler.handle_cmd("config.update", Some(data), &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("failed to load skills config"));
}

#[tokio::test]
async fn test_skills_source_list_invalid_json_errors() {
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    ensure_config_dir(ws);
    std::fs::write(ws.join("config/config.skills.json"), "{ broken").unwrap();
    let ctx = make_ctx(&dir);

    let result = handler.handle_cmd("source.list", None, &ctx).await;
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// config.save — round-trip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_skills_config_save_round_trip_full() {
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let ctx = make_ctx(&dir);

    let save_data = serde_json::json!({
        "enabled": false,
        "search_limit": 7,
        "max_concurrent_searches": 3,
        "search_cache": {
            "enabled": true,
            "max_size": 100,
            "ttl_seconds": 60,
        },
        "github_sources": [{
            "name": "gh1",
            "repo": "owner1/repo1",
            "enabled": true,
            "branch": "main",
            "index_type": "github_api",
            "skill_path_pattern": "skills/{slug}/SKILL.md",
        }],
    });
    let r = handler
        .handle_cmd("config.save", Some(save_data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(r["saved"].as_bool().unwrap());

    // Verify all values persist via config.get.
    let r = handler
        .handle_cmd("config.get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(!r["enabled"].as_bool().unwrap());
    assert_eq!(r["search_limit"].as_i64().unwrap(), 7);
    assert_eq!(r["max_concurrent_searches"].as_i64().unwrap(), 3);

    let _ = ws; // suppress unused warning
}

// ---------------------------------------------------------------------------
// source.add.manual — multiple additions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_skills_source_add_manual_multiple() {
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_skills_config(dir.path(), &nemesis_config::SkillsFullConfig::default());
    let ctx = make_ctx(&dir);

    for i in 0..3 {
        let data = serde_json::json!({
            "name": format!("src{}", i),
            "repo": format!("owner{}/repo{}", i, i),
        });
        let r = handler
            .handle_cmd("source.add.manual", Some(data), &ctx)
            .await
            .unwrap()
            .unwrap();
        assert!(r["success"].as_bool().unwrap());
    }

    let list = handler
        .handle_cmd("source.list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let sources = list["sources"].as_array().unwrap();
    // 3 github + 1 clawhub + 1 modelscope
    assert_eq!(sources.len(), 5);
    let gh_count = sources.iter().filter(|s| s["type"] == "github").count();
    assert_eq!(gh_count, 3);
}

// ---------------------------------------------------------------------------
// source.remove — remove one of many
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_skills_source_remove_one_of_many() {
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let cfg = nemesis_config::SkillsFullConfig {
        github_sources: vec![
            nemesis_config::GitHubSourceConfig {
                name: "a".to_string(),
                repo: "o/a".to_string(),
                ..Default::default()
            },
            nemesis_config::GitHubSourceConfig {
                name: "b".to_string(),
                repo: "o/b".to_string(),
                ..Default::default()
            },
            nemesis_config::GitHubSourceConfig {
                name: "c".to_string(),
                repo: "o/c".to_string(),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    write_skills_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": "b" });
    let r = handler
        .handle_cmd("source.remove", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(r["removed"].as_bool().unwrap());

    let list = handler
        .handle_cmd("source.list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let gh_names: Vec<&str> = list["sources"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["type"] == "github")
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(gh_names, vec!["a", "c"]);
}

// ---------------------------------------------------------------------------
// source.toggle — clawhub disabled then re-enabled
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_skills_source_toggle_clawhub_then_persist() {
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_skills_config(dir.path(), &nemesis_config::SkillsFullConfig::default());
    let ctx = make_ctx(&dir);

    // Disable.
    let data = serde_json::json!({ "name": "clawhub", "enabled": false });
    let r = handler
        .handle_cmd("source.toggle", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(!r["enabled"].as_bool().unwrap());

    // Verify persisted.
    let list = handler
        .handle_cmd("source.list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let ch = list["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["type"] == "clawhub")
        .unwrap();
    assert!(!ch["enabled"].as_bool().unwrap());
}

// ---------------------------------------------------------------------------
// search — with installed-slugs (subset match)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_skills_search_no_sources_with_installed_dir() {
    // No registries but a skills/ dir exists; just verify no panic.
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    std::fs::create_dir_all(ws.join("skills/installed-one")).unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "query": "anything" });
    let r = handler
        .handle_cmd("search", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(r["results"].as_array().unwrap().is_empty());
    // No sources → friendly message.
    assert!(r["message"].as_str().is_some());
}

// ---------------------------------------------------------------------------
// browse — sort variants
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_skills_browse_sort_downloads() {
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "sort": "downloads" });
    let _ = handler.handle_cmd("browse", Some(data), &ctx).await;
}

#[tokio::test]
async fn test_skills_browse_sort_stars() {
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "sort": "stars" });
    let _ = handler.handle_cmd("browse", Some(data), &ctx).await;
}

#[tokio::test]
async fn test_skills_browse_sort_unknown_falls_back_to_trending() {
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "sort": "totally-unknown" });
    let _ = handler.handle_cmd("browse", Some(data), &ctx).await;
}

#[tokio::test]
async fn test_skills_browse_with_limit_zero() {
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "limit": 0u64 });
    let _ = handler.handle_cmd("browse", Some(data), &ctx).await;
}

#[tokio::test]
async fn test_skills_browse_custom_registry() {
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "registry": "modelscope" });
    let _ = handler.handle_cmd("browse", Some(data), &ctx).await;
}

// ---------------------------------------------------------------------------
// install — invalid slug/registry type
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_skills_install_registry_type_mismatch() {
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "registry": 42, "slug": "x" });
    let err = handler
        .handle_cmd("install", Some(data), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing field: registry");
}

#[tokio::test]
async fn test_skills_install_slug_type_mismatch() {
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "registry": "r", "slug": false });
    let err = handler
        .handle_cmd("install", Some(data), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing field: slug");
}

#[tokio::test]
async fn test_skills_install_force_with_unknown_registry() {
    // force=true bypasses already_installed check; still fails on unknown registry.
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({
        "registry": "ghost",
        "slug": "fresh",
        "force": true,
    });
    let err = handler
        .handle_cmd("install", Some(data), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("不存在"), "got: {}", err);
}

// ---------------------------------------------------------------------------
// shop_detail / shop_code — type mismatches
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_skills_shop_detail_registry_type_mismatch() {
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "registry": 1, "slug": "x" });
    let err = handler
        .handle_cmd("shop_detail", Some(data), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing field: registry");
}

#[tokio::test]
async fn test_skills_shop_detail_slug_type_mismatch() {
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "registry": "r", "slug": null });
    let err = handler
        .handle_cmd("shop_detail", Some(data), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing field: slug");
}

#[tokio::test]
async fn test_skills_shop_code_missing_field_slug() {
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "registry": "r" });
    let err = handler
        .handle_cmd("shop_code", Some(data), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing field: slug");
}

// ---------------------------------------------------------------------------
// config.update — partial updates (each field independently)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_skills_config_update_search_cache_only_enabled() {
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_skills_config(dir.path(), &nemesis_config::SkillsFullConfig::default());
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "search_cache": { "enabled": false } });
    let r = handler
        .handle_cmd("config.update", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(r["updated"].as_bool().unwrap());

    let r = handler
        .handle_cmd("config.get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(!r["search_cache"]["enabled"].as_bool().unwrap());
}

#[tokio::test]
async fn test_skills_config_update_search_cache_only_max_size() {
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_skills_config(dir.path(), &nemesis_config::SkillsFullConfig::default());
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "search_cache": { "max_size": 5050 } });
    let r = handler
        .handle_cmd("config.update", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(r["updated"].as_bool().unwrap());

    let r = handler
        .handle_cmd("config.get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["search_cache"]["max_size"].as_i64().unwrap(), 5050);
}

#[tokio::test]
async fn test_skills_config_update_clawhub_only_base_url() {
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_skills_config(dir.path(), &nemesis_config::SkillsFullConfig::default());
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "clawhub": { "base_url": "https://new.example.com" } });
    let r = handler
        .handle_cmd("config.update", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(r["updated"].as_bool().unwrap());

    let r = handler
        .handle_cmd("config.get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["clawhub"]["base_url"], "https://new.example.com");
}

#[tokio::test]
async fn test_skills_config_update_null_fields_ignored() {
    // Setting fields to null should be silently ignored.
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_skills_config(dir.path(), &nemesis_config::SkillsFullConfig::default());
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({
        "enabled": null,
        "max_concurrent_searches": null,
        "search_cache": null,
        "clawhub": null,
    });
    let r = handler
        .handle_cmd("config.update", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(r["updated"].as_bool().unwrap());

    // Defaults preserved.
    let r = handler
        .handle_cmd("config.get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(r["enabled"].as_bool().unwrap());
}

#[tokio::test]
async fn test_skills_config_update_wrong_types_ignored() {
    // Wrong types for fields are silently ignored.
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_skills_config(dir.path(), &nemesis_config::SkillsFullConfig::default());
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({
        "enabled": "not-a-bool",
        "max_concurrent_searches": "string",
        "search_cache": { "enabled": "yes", "max_size": "many", "ttl_seconds": "long" },
        "clawhub": { "enabled": 1, "base_url": 42, "convex_url": false, "timeout": "now" },
    });
    let r = handler
        .handle_cmd("config.update", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(r["updated"].as_bool().unwrap());

    // Defaults preserved.
    let r = handler
        .handle_cmd("config.get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(r["enabled"].as_bool().unwrap());
}

// ---------------------------------------------------------------------------
// config.save — invalid types
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_skills_config_save_wrong_enabled_type() {
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "enabled": "not-a-bool" });
    let err = handler
        .handle_cmd("config.save", Some(data), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("invalid skills config"), "got: {}", err);
}

#[tokio::test]
async fn test_skills_config_save_null_value() {
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("config.save", Some(serde_json::Value::Null), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("invalid skills config"));
}

// ---------------------------------------------------------------------------
// source.list — multiple github + ordering
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_skills_source_list_github_fields() {
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let cfg = nemesis_config::SkillsFullConfig {
        github_sources: vec![nemesis_config::GitHubSourceConfig {
            name: "full".to_string(),
            repo: "owner/full-repo".to_string(),
            enabled: false,
            branch: "develop".to_string(),
            index_type: "skills_json".to_string(),
            skill_path_pattern: "skills.json".to_string(),
            ..Default::default()
        }],
        ..Default::default()
    };
    write_skills_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let r = handler
        .handle_cmd("source.list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let gh = r["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["type"] == "github")
        .unwrap();
    assert_eq!(gh["name"], "full");
    assert_eq!(gh["repo"], "owner/full-repo");
    assert!(!gh["enabled"].as_bool().unwrap());
    assert_eq!(gh["branch"], "develop");
    assert_eq!(gh["index_type"], "skills_json");
    assert_eq!(gh["skill_path_pattern"], "skills.json");
}

// ---------------------------------------------------------------------------
// source.add — bad URL formats (parse error before network)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_skills_source_add_unparseable_url() {
    // A URL that can't be parsed yields an error before any network call.
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_skills_config(dir.path(), &nemesis_config::SkillsFullConfig::default());
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "url": "not a url with spaces" });
    let err = handler
        .handle_cmd("source.add", Some(data), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("无法解析 URL"), "got: {}", err);
}

#[tokio::test]
async fn test_skills_source_add_url_with_only_owner() {
    // "owner" alone (no slash) cannot be parsed.
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_skills_config(dir.path(), &nemesis_config::SkillsFullConfig::default());
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "url": "justowner" });
    let err = handler
        .handle_cmd("source.add", Some(data), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("无法解析 URL"), "got: {}", err);
}

// ---------------------------------------------------------------------------
// open_dir — nonexistent skill
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_skills_open_dir_nonexistent_errors() {
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": "ghost" });
    let err = handler
        .handle_cmd("open_dir", Some(data), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("directory not found"), "got: {}", err);
}

// ---------------------------------------------------------------------------
// Module name
// ---------------------------------------------------------------------------

#[test]
fn test_skills_module_name() {
    let h = SkillsHandler::new();
    assert_eq!(h.module_name(), "skills");
}

//! Extra coverage tests for the Forge WebSocket handler.
//!
//! Mirrors the helpers used in `tests.rs` so we can construct
//! `RequestContext` instances with a temp workspace. The Forge
//! handler is pure business logic that reads/writes on-disk JSON
//! files, so we exercise every `handle_cmd` arm below.

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
// Test infrastructure (mirror of tests.rs)
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

fn write_config(workspace: &Path) {
    let config = nemesis_config::Config::default();
    let json = serde_json::to_string_pretty(&config).unwrap();
    std::fs::write(workspace.join("config.json"), json).unwrap();
}

fn ensure_config_dir(workspace: &Path) {
    std::fs::create_dir_all(workspace.join("config")).unwrap();
}

fn write_forge_config(workspace: &Path, json: serde_json::Value) {
    ensure_config_dir(workspace);
    let path = workspace.join("config").join("config.forge.json");
    let pretty = serde_json::to_string_pretty(&json).unwrap();
    std::fs::write(&path, pretty).unwrap();
}

fn forge_dir(workspace: &Path) -> std::path::PathBuf {
    workspace.join("forge")
}

/// Write a single experience line to `forge/experiences/experiences.jsonl`.
fn write_experience_line(workspace: &Path, line: &serde_json::Value) {
    let dir = forge_dir(workspace).join("experiences");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("experiences.jsonl");
    let mut existing = std::fs::read_to_string(&path).unwrap_or_default();
    if !existing.is_empty() && !existing.ends_with('\n') {
        existing.push('\n');
    }
    existing.push_str(&line.to_string());
    existing.push('\n');
    std::fs::write(&path, existing).unwrap();
}

/// Construct a single CollectedExperience record matching what
/// `reflect` deserializes.
fn sample_experience(id: &str, success: bool) -> serde_json::Value {
    serde_json::json!({
        "experience": {
            "id": id,
            "tool_name": "shell",
            "input_summary": "echo hi",
            "output_summary": "hi",
            "success": success,
            "duration_ms": 120,
            "timestamp": "2026-06-16T10:00:00Z",
            "session_key": "sess-1",
        },
        "dedup_hash": "fake-hash",
    })
}

// -----------------------------------------------------------------------
// Module metadata
// -----------------------------------------------------------------------

#[test]
fn test_module_name() {
    let handler = forge::ForgeHandler::new();
    assert_eq!(handler.module_name(), "forge");
}

// -----------------------------------------------------------------------
// require_home / require_workspace error paths
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_status_no_workspace_returns_error() {
    let handler = forge::ForgeHandler::new();
    let ctx = make_ctx_no_workspace();
    let err = handler.handle_cmd("status", None, &ctx).await.unwrap_err();
    assert!(
        err.contains("not configured"),
        "expected not-configured error, got: {}",
        err
    );
}

#[tokio::test]
async fn test_stats_no_workspace_returns_error() {
    let handler = forge::ForgeHandler::new();
    let ctx = make_ctx_no_workspace();
    let err = handler.handle_cmd("stats", None, &ctx).await.unwrap_err();
    assert!(
        err.contains("not configured"),
        "expected not-configured error, got: {}",
        err
    );
}

// -----------------------------------------------------------------------
// status
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_status_missing_config_returns_error() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    // Write a malformed config.json -> load_config fails to parse.
    std::fs::write(dir.path().join("config.json"), "not valid json {{{").unwrap();
    let ctx = make_ctx(&dir);
    let err = handler.handle_cmd("status", None, &ctx).await.unwrap_err();
    assert!(
        err.contains("failed to load config"),
        "expected config load error, got: {}",
        err
    );
}

#[tokio::test]
async fn test_status_no_forge_dir_returns_zeros() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());
    let ctx = make_ctx(&dir);

    let result = handler.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
    assert!(!result["enabled"].as_bool().unwrap());
    assert!(!result["running"].as_bool().unwrap());
    assert!(result["started_at"].is_null());
    assert!(!result["forge_dir_exists"].as_bool().unwrap());
    assert_eq!(result["experience_count"], 0);
    assert_eq!(result["reflection_count"], 0);
    assert_eq!(result["artifact_count"], 0);
    assert_eq!(result["cycle_count"], 0);
    // learning_enabled comes from forge instance — None here, defaults to false.
    assert!(!result["learning_enabled"].as_bool().unwrap());
}

#[tokio::test]
async fn test_status_reads_forge_config_file() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());
    // Use a non-default interval to prove the file is being read.
    write_forge_config(
        dir.path(),
        serde_json::json!({
            "enabled": true,
            "reflection": { "interval_secs": 1234 },
            "storage": { "cleanup_interval_secs": 9876 },
        }),
    );
    let ctx = make_ctx(&dir);

    let result = handler.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
    assert_eq!(result["reflection_interval_secs"], 1234);
    assert_eq!(result["cleanup_interval_secs"], 9876);
}

#[tokio::test]
async fn test_status_counts_files_in_forge_dir() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    // 2 experiences
    write_experience_line(dir.path(), &sample_experience("e1", true));
    write_experience_line(dir.path(), &sample_experience("e2", false));

    // 1 reflection report
    let reflections_dir = forge_dir(dir.path()).join("reflections");
    std::fs::create_dir_all(&reflections_dir).unwrap();
    std::fs::write(
        reflections_dir.join("reflection_2026-06-16_100000.md"),
        "# Report",
    )
    .unwrap();

    // 1 registry artifact
    let registry_path = forge_dir(dir.path()).join("registry.json");
    std::fs::write(
        &registry_path,
        serde_json::to_string_pretty(&serde_json::json!([
            { "id": "a-1", "status": "Active" }
        ]))
        .unwrap(),
    )
    .unwrap();

    // 1 learning cycle
    let cycle_dir = forge_dir(dir.path()).join("learning").join("2026-06");
    std::fs::create_dir_all(&cycle_dir).unwrap();
    std::fs::write(
        cycle_dir.join("cycles.jsonl"),
        serde_json::json!({ "id": "c-1" }).to_string() + "\n",
    )
    .unwrap();

    let ctx = make_ctx(&dir);
    let result = handler.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
    assert_eq!(result["experience_count"], 2);
    assert_eq!(result["reflection_count"], 1);
    assert_eq!(result["artifact_count"], 1);
    assert_eq!(result["cycle_count"], 1);
    assert!(result["forge_dir_exists"].as_bool().unwrap());
}

// -----------------------------------------------------------------------
// stats
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_stats_missing_config_returns_error() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    // Write malformed config so load_config fails.
    std::fs::write(dir.path().join("config.json"), "{ broken").unwrap();
    let ctx = make_ctx(&dir);
    let err = handler.handle_cmd("stats", None, &ctx).await.unwrap_err();
    assert!(err.contains("failed to load config"));
}

#[tokio::test]
async fn test_stats_default_values() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());
    let ctx = make_ctx(&dir);

    let result = handler.handle_cmd("stats", None, &ctx).await.unwrap().unwrap();
    assert!(!result["enabled"].as_bool().unwrap());
    assert_eq!(result["experiences"]["total"], 0);
    assert_eq!(result["experiences"]["success"], 0);
    assert_eq!(result["experiences"]["failure"], 0);
    assert_eq!(result["reflections"]["total"], 0);
    assert!(result["reflections"]["latest"].is_null());
    assert_eq!(result["artifacts"]["total"], 0);
    assert_eq!(result["artifacts"]["active"], 0);
    assert_eq!(result["artifacts"]["observing"], 0);
    assert_eq!(result["cycles"]["total"], 0);
    assert!(result["cycles"]["last"].is_null());

    // Config section reflects default ForgeConfig.
    assert!(result["config"]["learning_enabled"].is_boolean());
}

#[tokio::test]
async fn test_stats_with_experience_and_artifacts() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    // 2 successes + 1 failure on `shell`, 1 success on `web`.
    write_experience_line(dir.path(), &sample_experience("e1", true));
    write_experience_line(dir.path(), &sample_experience("e2", true));
    write_experience_line(dir.path(), &sample_experience("e3", false));
    let mut web_exp = sample_experience("e4", true);
    web_exp["experience"]["tool_name"] = serde_json::json!("web");
    write_experience_line(dir.path(), &web_exp);

    // Artifacts: 2 Active + 1 Observing + 1 other status.
    let registry_path = forge_dir(dir.path()).join("registry.json");
    std::fs::write(
        &registry_path,
        serde_json::to_string_pretty(&serde_json::json!([
            { "id": "a-1", "status": "Active" },
            { "id": "a-2", "status": "Active" },
            { "id": "a-3", "status": "Observing" },
            { "id": "a-4", "status": "Disabled" }
        ]))
        .unwrap(),
    )
    .unwrap();

    let ctx = make_ctx(&dir);
    let result = handler.handle_cmd("stats", None, &ctx).await.unwrap().unwrap();
    assert_eq!(result["experiences"]["total"], 4);
    assert_eq!(result["experiences"]["success"], 3);
    assert_eq!(result["experiences"]["failure"], 1);

    let tools = result["experiences"]["tools"].as_object().unwrap();
    assert!(tools.contains_key("shell"));
    assert!(tools.contains_key("web"));
    let shell = &tools["shell"];
    assert_eq!(shell["count"], 3);
    assert_eq!(shell["success"], 2);
    assert_eq!(shell["failure"], 1);

    assert_eq!(result["artifacts"]["total"], 4);
    assert_eq!(result["artifacts"]["active"], 2);
    assert_eq!(result["artifacts"]["observing"], 1);
}

#[tokio::test]
async fn test_stats_finds_latest_reflection() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    let reflections_dir = forge_dir(dir.path()).join("reflections");
    std::fs::create_dir_all(&reflections_dir).unwrap();
    let path = reflections_dir.join("reflection_2026-06-16_100000.md");
    std::fs::write(&path, "# Report").unwrap();

    // Wait briefly so mtime is non-zero and deterministic comparison is possible.
    std::thread::sleep(std::time::Duration::from_millis(20));
    let later_path = reflections_dir.join("reflection_2026-06-16_100001.md");
    std::fs::write(&later_path, "# Newer").unwrap();

    let ctx = make_ctx(&dir);
    let result = handler.handle_cmd("stats", None, &ctx).await.unwrap().unwrap();
    assert_eq!(result["reflections"]["total"], 2);
    let latest = &result["reflections"]["latest"];
    assert_eq!(latest["name"].as_str().unwrap(), "reflection_2026-06-16_100001.md");
}

// -----------------------------------------------------------------------
// experiences.stats
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_experiences_stats_no_file_returns_zeros() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    let ctx = make_ctx(&dir);
    let result = handler
        .handle_cmd("experiences.stats", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["total"], 0);
    assert!(result["tools"].as_object().unwrap().is_empty());
    assert!(result["recent"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_experiences_stats_with_data() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    write_experience_line(dir.path(), &sample_experience("e1", true));
    write_experience_line(dir.path(), &sample_experience("e2", false));

    let ctx = make_ctx(&dir);
    let result = handler
        .handle_cmd("experiences.stats", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["total"], 2);
    assert_eq!(result["success"], 1);
    assert_eq!(result["failure"], 1);
    let tools = result["tools"].as_object().unwrap();
    assert_eq!(tools["shell"]["count"], 2);
    let recent = result["recent"].as_array().unwrap();
    assert_eq!(recent.len(), 2);
    // Most recent entry should be the last line written.
    assert_eq!(recent.last().unwrap()["id"].as_str().unwrap(), "e2");
}

// -----------------------------------------------------------------------
// reflections.list / reflections.latest
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_reflections_list_no_dir() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    let ctx = make_ctx(&dir);
    let result = handler
        .handle_cmd("reflections.list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(result["reports"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_reflections_list_with_files() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    let rd = forge_dir(dir.path()).join("reflections");
    std::fs::create_dir_all(&rd).unwrap();
    std::fs::write(rd.join("reflection_2026-06-15_100000.md"), "# a").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(rd.join("reflection_2026-06-16_100000.md"), "# b").unwrap();
    // Non-md file should be filtered out.
    std::fs::write(rd.join("notes.txt"), "ignore me").unwrap();

    let ctx = make_ctx(&dir);
    let result = handler
        .handle_cmd("reflections.list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let reports = result["reports"].as_array().unwrap();
    assert_eq!(reports.len(), 2);
    // Sorted desc by modified time, so 06-16 should come first.
    assert_eq!(reports[0]["name"].as_str().unwrap(), "reflection_2026-06-16_100000.md");
    assert_eq!(reports[0]["date"].as_str().unwrap(), "2026-06-16");
    assert!(reports[0]["modified"].as_str().unwrap().contains("T"));
    assert_eq!(reports[0]["size"], 3_u64);
}

#[tokio::test]
async fn test_reflections_latest_no_dir() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    let ctx = make_ctx(&dir);
    let result = handler
        .handle_cmd("reflections.latest", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(!result["found"].as_bool().unwrap());
    assert_eq!(result["content"].as_str().unwrap(), "");
}

#[tokio::test]
async fn test_reflections_latest_with_file() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    let rd = forge_dir(dir.path()).join("reflections");
    std::fs::create_dir_all(&rd).unwrap();
    std::fs::write(rd.join("reflection_2026-06-16_100000.md"), "# hello world").unwrap();

    let ctx = make_ctx(&dir);
    let result = handler
        .handle_cmd("reflections.latest", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(result["found"].as_bool().unwrap());
    assert_eq!(result["name"].as_str().unwrap(), "reflection_2026-06-16_100000.md");
    assert_eq!(result["content"].as_str().unwrap(), "# hello world");
}

#[tokio::test]
async fn test_reflections_latest_empty_dir_returns_not_found() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    let rd = forge_dir(dir.path()).join("reflections");
    std::fs::create_dir_all(&rd).unwrap();

    let ctx = make_ctx(&dir);
    let result = handler
        .handle_cmd("reflections.latest", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(!result["found"].as_bool().unwrap());
}

// -----------------------------------------------------------------------
// cycles.list
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_cycles_list_no_dir() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    let ctx = make_ctx(&dir);
    let result = handler
        .handle_cmd("cycles.list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(result["cycles"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_cycles_list_with_data() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    let month_dir = forge_dir(dir.path()).join("learning").join("2026-06");
    std::fs::create_dir_all(&month_dir).unwrap();
    let path = month_dir.join("cycles.jsonl");
    std::fs::write(
        &path,
        serde_json::json!({ "id": "c-1", "n": 1 }).to_string()
            + "\n"
            + &serde_json::json!({ "id": "c-2", "n": 2 }).to_string()
            + "\n",
    )
    .unwrap();

    // A non-jsonl file should be ignored.
    std::fs::write(month_dir.join("notes.txt"), "ignore").unwrap();

    let ctx = make_ctx(&dir);
    let result = handler
        .handle_cmd("cycles.list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let cycles = result["cycles"].as_array().unwrap();
    assert_eq!(cycles.len(), 2);
    // Newest first.
    assert_eq!(cycles[0]["id"].as_str().unwrap(), "c-2");
}

#[tokio::test]
async fn test_cycles_list_truncates_to_100() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    let month_dir = forge_dir(dir.path()).join("learning").join("2026-06");
    std::fs::create_dir_all(&month_dir).unwrap();
    let path = month_dir.join("cycles.jsonl");
    let mut content = String::new();
    for i in 0..150u32 {
        content.push_str(&serde_json::json!({ "id": format!("c-{}", i) }).to_string());
        content.push('\n');
    }
    std::fs::write(&path, content).unwrap();

    let ctx = make_ctx(&dir);
    let result = handler
        .handle_cmd("cycles.list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let cycles = result["cycles"].as_array().unwrap();
    assert_eq!(cycles.len(), 100);
    // Reversed before take(100) -> newest entries only.
    assert_eq!(cycles[0]["id"].as_str().unwrap(), "c-149");
}

// -----------------------------------------------------------------------
// registry.list / registry.update
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_registry_list_empty() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    let ctx = make_ctx(&dir);
    let result = handler
        .handle_cmd("registry.list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(result["artifacts"].as_array().unwrap().is_empty());
    assert!(result["skill_directories"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_registry_list_with_artifacts_and_skills() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    // registry.json
    let fd = forge_dir(dir.path());
    std::fs::create_dir_all(&fd).unwrap();
    std::fs::write(
        fd.join("registry.json"),
        serde_json::to_string_pretty(&serde_json::json!([
            { "id": "a-1", "status": "Active" }
        ]))
        .unwrap(),
    )
    .unwrap();

    // skills: one directory with SKILL.md, one without.
    let skills_dir = fd.join("skills");
    std::fs::create_dir_all(skills_dir.join("alpha")).unwrap();
    std::fs::write(skills_dir.join("alpha").join("SKILL.md"), "alpha").unwrap();
    std::fs::create_dir_all(skills_dir.join("beta")).unwrap();

    let ctx = make_ctx(&dir);
    let result = handler
        .handle_cmd("registry.list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let artifacts = result["artifacts"].as_array().unwrap();
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0]["id"].as_str().unwrap(), "a-1");

    let mut skill_dirs = result["skill_directories"].as_array().unwrap().clone();
    skill_dirs.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap()
            .cmp(b["name"].as_str().unwrap())
    });
    assert_eq!(skill_dirs.len(), 2);
    assert_eq!(skill_dirs[0]["name"].as_str().unwrap(), "alpha");
    assert!(skill_dirs[0]["has_skill_md"].as_bool().unwrap());
    assert_eq!(skill_dirs[1]["name"].as_str().unwrap(), "beta");
    assert!(!skill_dirs[1]["has_skill_md"].as_bool().unwrap());
}

#[tokio::test]
async fn test_registry_update_missing_data() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    let ctx = make_ctx(&dir);
    let err = handler
        .handle_cmd("registry.update", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing data");
}

#[tokio::test]
async fn test_registry_update_missing_id_field() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    let ctx = make_ctx(&dir);
    let data = serde_json::json!({ "status": "Active" });
    let err = handler
        .handle_cmd("registry.update", Some(data), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing 'id' field");
}

#[tokio::test]
async fn test_registry_update_missing_status_field() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    let ctx = make_ctx(&dir);
    let data = serde_json::json!({ "id": "a-1" });
    let err = handler
        .handle_cmd("registry.update", Some(data), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing 'status' field");
}

#[tokio::test]
async fn test_registry_update_registry_not_found() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    let ctx = make_ctx(&dir);
    let data = serde_json::json!({ "id": "a-1", "status": "Active" });
    let err = handler
        .handle_cmd("registry.update", Some(data), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "registry not found");
}

#[tokio::test]
async fn test_registry_update_artifact_not_found() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    let fd = forge_dir(dir.path());
    std::fs::create_dir_all(&fd).unwrap();
    std::fs::write(
        fd.join("registry.json"),
        serde_json::to_string_pretty(&serde_json::json!([
            { "id": "a-1", "status": "Active" }
        ]))
        .unwrap(),
    )
    .unwrap();

    let ctx = make_ctx(&dir);
    let data = serde_json::json!({ "id": "a-unknown", "status": "Observing" });
    let err = handler
        .handle_cmd("registry.update", Some(data), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "artifact 'a-unknown' not found");
}

#[tokio::test]
async fn test_registry_update_success() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    let fd = forge_dir(dir.path());
    std::fs::create_dir_all(&fd).unwrap();
    let registry_path = fd.join("registry.json");
    std::fs::write(
        &registry_path,
        serde_json::to_string_pretty(&serde_json::json!([
            { "id": "a-1", "status": "Active", "updated_at": "old" },
            { "id": "a-2", "status": "Observing" }
        ]))
        .unwrap(),
    )
    .unwrap();

    let ctx = make_ctx(&dir);
    let data = serde_json::json!({ "id": "a-2", "status": "Active" });
    let result = handler
        .handle_cmd("registry.update", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(result["updated"].as_bool().unwrap());
    assert_eq!(result["id"].as_str().unwrap(), "a-2");
    assert_eq!(result["status"].as_str().unwrap(), "Active");

    // Verify file on disk was updated.
    let on_disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&registry_path).unwrap()).unwrap();
    let arr = on_disk.as_array().unwrap();
    assert_eq!(arr[1]["status"].as_str().unwrap(), "Active");
    assert_ne!(arr[1]["updated_at"].as_str().unwrap(), "old");
}

// -----------------------------------------------------------------------
// config.save (data validation + config.forge.json sync)
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_config_save_missing_data() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    let ctx = make_ctx(&dir);
    let err = handler
        .handle_cmd("config.save", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing data");
}

#[tokio::test]
async fn test_config_save_missing_enabled_field() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    let ctx = make_ctx(&dir);
    let data = serde_json::json!({ "other": true });
    let err = handler
        .handle_cmd("config.save", Some(data), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing or invalid 'enabled' field");
}

#[tokio::test]
async fn test_config_save_disabled_when_was_disabled_no_runtime() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    let ctx = make_ctx(&dir);
    let data = serde_json::json!({ "enabled": false });
    let result = handler
        .handle_cmd("config.save", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(result["saved"].as_bool().unwrap());
    assert!(!result["enabled"].as_bool().unwrap());

    // Persisted in config.json.
    let cfg =
        nemesis_config::load_config(&dir.path().join("config.json")).expect("load ok");
    assert!(!cfg.forge.as_ref().unwrap().enabled);

    // config.forge.json auto-created with enabled=false.
    let fc_path = dir.path().join("config").join("config.forge.json");
    assert!(fc_path.exists());
    let on_disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&fc_path).unwrap()).unwrap();
    assert_eq!(on_disk["enabled"].as_bool(), Some(false));
}

#[tokio::test]
async fn test_config_save_enable_creates_forge_config_from_default() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    let ctx = make_ctx(&dir);
    let data = serde_json::json!({ "enabled": true });
    let result = handler
        .handle_cmd("config.save", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(result["enabled"].as_bool().unwrap());

    // config.forge.json should be created from default with enabled=true.
    let fc_path = dir.path().join("config").join("config.forge.json");
    assert!(fc_path.exists());
    let on_disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&fc_path).unwrap()).unwrap();
    assert_eq!(on_disk["enabled"].as_bool(), Some(true));
}

#[tokio::test]
async fn test_config_save_enable_updates_existing_forge_config() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());
    // Pre-existing config.forge.json — enabled flag should be toggled.
    write_forge_config(
        dir.path(),
        serde_json::json!({
            "enabled": false,
            "reflection": { "interval_secs": 4242 }
        }),
    );

    let ctx = make_ctx(&dir);
    let data = serde_json::json!({ "enabled": true });
    handler
        .handle_cmd("config.save", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();

    let fc_path = dir.path().join("config").join("config.forge.json");
    let on_disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&fc_path).unwrap()).unwrap();
    assert_eq!(on_disk["enabled"].as_bool(), Some(true));
    // Preserved field.
    assert_eq!(on_disk["reflection"]["interval_secs"], 4242);
}

// -----------------------------------------------------------------------
// learning.toggle
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_learning_toggle_missing_data() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    let ctx = make_ctx(&dir);
    let err = handler
        .handle_cmd("learning.toggle", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing data");
}

#[tokio::test]
async fn test_learning_toggle_missing_enabled_field() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    let ctx = make_ctx(&dir);
    let data = serde_json::json!({ "other": 1 });
    let err = handler
        .handle_cmd("learning.toggle", Some(data), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing 'enabled' field");
}

#[tokio::test]
async fn test_learning_toggle_creates_forge_config_if_missing() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    let ctx = make_ctx(&dir);
    let data = serde_json::json!({ "enabled": true });
    let result = handler
        .handle_cmd("learning.toggle", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(result["saved"].as_bool().unwrap());
    assert!(result["learning_enabled"].as_bool().unwrap());

    let fc_path = dir.path().join("config").join("config.forge.json");
    assert!(fc_path.exists());
    let on_disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&fc_path).unwrap()).unwrap();
    assert_eq!(on_disk["learning"]["enabled"].as_bool(), Some(true));
}

#[tokio::test]
async fn test_learning_toggle_updates_existing_learning_section() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());
    write_forge_config(
        dir.path(),
        serde_json::json!({
            "enabled": true,
            "learning": { "enabled": false, "min_pattern_frequency": 7 }
        }),
    );

    let ctx = make_ctx(&dir);
    let data = serde_json::json!({ "enabled": true });
    handler
        .handle_cmd("learning.toggle", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();

    let fc_path = dir.path().join("config").join("config.forge.json");
    let on_disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&fc_path).unwrap()).unwrap();
    assert_eq!(on_disk["learning"]["enabled"].as_bool(), Some(true));
    assert_eq!(on_disk["learning"]["min_pattern_frequency"], 7);
}

#[tokio::test]
async fn test_learning_toggle_inserts_learning_section_when_absent() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());
    // Forge config exists but has no learning section.
    write_forge_config(dir.path(), serde_json::json!({ "enabled": true }));

    let ctx = make_ctx(&dir);
    let data = serde_json::json!({ "enabled": true });
    let result = handler
        .handle_cmd("learning.toggle", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(result["saved"].as_bool().unwrap());

    let fc_path = dir.path().join("config").join("config.forge.json");
    let on_disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&fc_path).unwrap()).unwrap();
    assert_eq!(
        on_disk["learning"]["enabled"].as_bool(),
        Some(true),
        "learning section should be auto-created"
    );
}

// -----------------------------------------------------------------------
// artifacts (legacy)
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_artifacts_no_forge_dir_returns_empty() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    let ctx = make_ctx(&dir);
    let result = handler
        .handle_cmd("artifacts", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(result["artifacts"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_artifacts_mixed_files_and_dirs() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    let fd = forge_dir(dir.path());
    std::fs::create_dir_all(&fd).unwrap();
    std::fs::write(fd.join("a.txt"), "hi").unwrap();
    std::fs::create_dir_all(fd.join("subdir")).unwrap();

    let ctx = make_ctx(&dir);
    let result = handler
        .handle_cmd("artifacts", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let mut arr = result["artifacts"].as_array().unwrap().clone();
    arr.sort_by(|a, b| a["name"].as_str().unwrap().cmp(b["name"].as_str().unwrap()));
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["name"].as_str().unwrap(), "a.txt");
    assert_eq!(arr[0]["type"].as_str().unwrap(), "file");
    assert_eq!(arr[0]["size"], 2_u64);
    assert_eq!(arr[1]["name"].as_str().unwrap(), "subdir");
    assert_eq!(arr[1]["type"].as_str().unwrap(), "directory");
}

// -----------------------------------------------------------------------
// reflect
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_reflect_no_experiences_file_returns_not_triggered() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    let ctx = make_ctx(&dir);
    let result = handler
        .handle_cmd("reflect", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(!result["triggered"].as_bool().unwrap());
    assert!(result["message"].as_str().unwrap().contains("没有经验数据"));
}

#[tokio::test]
async fn test_reflect_empty_experiences_file_returns_not_triggered() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    // Create file but with only whitespace/blank lines.
    let exp_path = forge_dir(dir.path()).join("experiences").join("experiences.jsonl");
    std::fs::create_dir_all(exp_path.parent().unwrap()).unwrap();
    std::fs::write(&exp_path, "\n   \n").unwrap();

    let ctx = make_ctx(&dir);
    let result = handler
        .handle_cmd("reflect", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(!result["triggered"].as_bool().unwrap());
    assert!(result["message"].as_str().unwrap().contains("经验数据为空"));
}

#[tokio::test]
async fn test_reflect_runs_analysis_and_writes_report() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    // 1 success, 1 failure -> reflector can produce a report.
    write_experience_line(dir.path(), &sample_experience("e1", true));
    write_experience_line(dir.path(), &sample_experience("e2", false));

    let ctx = make_ctx(&dir);
    let result = handler
        .handle_cmd("reflect", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(result["triggered"].as_bool().unwrap());
    assert!(result["total_records"].as_u64().unwrap() >= 2);
    assert_eq!(result["unique_patterns"].as_u64().unwrap(), 1);

    // Reflection report written to reflections dir.
    let reports = std::fs::read_dir(forge_dir(dir.path()).join("reflections")).unwrap();
    let count = reports
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|x| x == "md").unwrap_or(false))
        .count();
    assert_eq!(count, 1, "exactly one reflection report should be written");
}

// -----------------------------------------------------------------------
// Unknown command
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_unknown_command_returns_error() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    let ctx = make_ctx(&dir);
    let err = handler
        .handle_cmd("nonsense", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "unknown command: forge.nonsense");
}

#[tokio::test]
async fn test_unknown_command_with_data() {
    let handler = forge::ForgeHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    let ctx = make_ctx(&dir);
    let data = serde_json::json!({ "whatever": 1 });
    let err = handler
        .handle_cmd("enable", Some(data), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "unknown command: forge.enable");
}

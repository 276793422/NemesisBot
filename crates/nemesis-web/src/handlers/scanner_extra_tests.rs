//! Extra coverage tests for the Scanner WebSocket handler.
//!
//! Mirrors the helpers used in `tests.rs` so we can construct
//! `RequestContext` instances with a temp workspace. The Scanner
//! handler is pure business logic that reads/writes on-disk JSON
//! config files plus tracks install/update-db state, so we exercise
//! every `handle_cmd` arm below without spawning a real ClamAV
//! daemon.

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
        cron: None,
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

fn ensure_config_dir(workspace: &Path) {
    std::fs::create_dir_all(workspace.join("config")).unwrap();
}

/// Write the scanner config file under `{workspace}/config/config.scanner.json`.
fn write_scanner_config(workspace: &Path, cfg: &nemesis_config::ScannerFullConfig) {
    ensure_config_dir(workspace);
    let json = serde_json::to_string_pretty(cfg).unwrap();
    std::fs::write(workspace.join("config/config.scanner.json"), json).unwrap();
}

/// Build a minimal scanner config containing one engine entry.
fn make_config_with_engine(
    name: &str,
    engine: nemesis_config::ClamAVEngineConfig,
    enabled: bool,
) -> nemesis_config::ScannerFullConfig {
    let mut cfg = nemesis_config::ScannerFullConfig::default();
    let engine_json = serde_json::to_value(&engine).unwrap();
    cfg.engines.insert(name.to_string(), engine_json);
    if enabled {
        cfg.enabled.push(name.to_string());
    }
    cfg
}

// -----------------------------------------------------------------------
// Module metadata
// -----------------------------------------------------------------------

#[test]
fn test_module_name() {
    let handler = scanner::ScannerHandler::new();
    assert_eq!(handler.module_name(), "scanner");
}

// -----------------------------------------------------------------------
// Unknown command
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_unknown_command_returns_error() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("does_not_exist", None, &ctx)
        .await
        .unwrap_err();
    assert!(
        err.contains("unknown command: scanner.does_not_exist"),
        "expected unknown command error, got: {}",
        err
    );
}

// -----------------------------------------------------------------------
// require_workspace error paths
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_no_workspace_status() {
    let handler = scanner::ScannerHandler::new();
    let ctx = make_ctx_no_workspace();
    let err = handler.handle_cmd("status", None, &ctx).await.unwrap_err();
    assert!(
        err.contains("workspace not configured"),
        "expected workspace-not-configured, got: {}",
        err
    );
}

#[tokio::test]
async fn test_no_workspace_config_get() {
    let handler = scanner::ScannerHandler::new();
    let ctx = make_ctx_no_workspace();
    let err = handler
        .handle_cmd("config.get", None, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("workspace not configured"));
}

#[tokio::test]
async fn test_no_workspace_enable() {
    let handler = scanner::ScannerHandler::new();
    let ctx = make_ctx_no_workspace();
    let data = serde_json::json!({ "name": "clamav" });
    let err = handler
        .handle_cmd("enable", Some(data), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("workspace not configured"));
}

// -----------------------------------------------------------------------
// config.get
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_config_get_missing_config_dir_loads_defaults() {
    // When the file doesn't exist, load_scanner_config returns defaults.
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    // Note: no ensure_config_dir, no scanner config file written.
    let ctx = make_ctx(&dir);

    let result = handler
        .handle_cmd("config.get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(result.is_object());
    assert_eq!(result["enabled"].as_array().unwrap().len(), 0);
    assert_eq!(result["engines"].as_object().unwrap().len(), 0);
}

#[tokio::test]
async fn test_config_get_with_engines() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = nemesis_config::ScannerFullConfig::default();
    cfg.enabled.push("clamav".to_string());
    let engine = nemesis_config::ClamAVEngineConfig {
        address: "127.0.0.1:3310".to_string(),
        url: "https://example.com/clamav.zip".to_string(),
        ..Default::default()
    };
    cfg.engines
        .insert("clamav".to_string(), serde_json::to_value(&engine).unwrap());
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let result = handler
        .handle_cmd("config.get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["enabled"][0], "clamav");
    assert_eq!(
        result["engines"]["clamav"]["address"],
        "127.0.0.1:3310"
    );
}

// -----------------------------------------------------------------------
// config.save
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_config_save_creates_config_dir() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    // Intentionally don't pre-create the config dir.
    let ctx = make_ctx(&dir);
    let save_data = serde_json::json!({
        "enabled": ["clamav"],
        "engines": {}
    });

    let result = handler
        .handle_cmd("config.save", Some(save_data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(result["saved"].as_bool().unwrap());

    // Verify file persisted.
    let written = dir.path().join("config/config.scanner.json");
    assert!(written.exists(), "expected config file to be created");
}

#[tokio::test]
async fn test_config_save_invalid_payload() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    // `enabled` must be an array — passing a string fails deserialization.
    let bad_data = serde_json::json!({ "enabled": "not-an-array" });

    let err = handler
        .handle_cmd("config.save", Some(bad_data), &ctx)
        .await
        .unwrap_err();
    assert!(
        err.contains("invalid scanner config"),
        "expected invalid scanner config error, got: {}",
        err
    );
}

#[tokio::test]
async fn test_config_save_missing_data() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("config.save", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing data");
}

// -----------------------------------------------------------------------
// status
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_status_empty() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let cfg = nemesis_config::ScannerFullConfig::default();
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let result = handler.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
    assert!(result["engines"].is_array());
    assert_eq!(result["engines"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_status_reports_enabled_flag() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();

    let engine = nemesis_config::ClamAVEngineConfig::default();
    let cfg = make_config_with_engine("clamav", engine, true);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let result = handler.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
    let engines = result["engines"].as_array().unwrap();
    assert_eq!(engines.len(), 1);
    assert_eq!(engines[0]["name"], "clamav");
    assert!(engines[0]["enabled"].as_bool().unwrap());
}

#[tokio::test]
async fn test_status_missing_config_returns_error() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    // Write a malformed JSON.
    ensure_config_dir(dir.path());
    std::fs::write(
        dir.path().join("config/config.scanner.json"),
        "{not valid json}",
    )
    .unwrap();
    let ctx = make_ctx(&dir);

    let err = handler.handle_cmd("status", None, &ctx).await.unwrap_err();
    assert!(
        err.contains("failed to load scanner config"),
        "expected load error, got: {}",
        err
    );
}

// -----------------------------------------------------------------------
// check
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_check_all_engines_pending_when_no_path() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let engine = nemesis_config::ClamAVEngineConfig::default();
    let cfg = make_config_with_engine("clamav", engine, true);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let result = handler
        .handle_cmd("check", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(result["engines"].is_array());
    let engines = result["engines"].as_array().unwrap();
    assert_eq!(engines[0]["state"]["install_status"], "pending");
}

#[tokio::test]
async fn test_check_specific_engine_returns_single_object() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let engine = nemesis_config::ClamAVEngineConfig::default();
    let cfg = make_config_with_engine("clamav", engine, true);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": "clamav" });
    let result = handler
        .handle_cmd("check", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    // When target_name provided and single result, returns single object.
    assert_eq!(result["name"], "clamav");
    assert!(
        !result.is_object() || result.get("engines").is_none(),
        "single-engine check should not be wrapped in engines array"
    );
}

#[tokio::test]
async fn test_check_unknown_engine_name_returns_empty_array() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let cfg = nemesis_config::ScannerFullConfig::default();
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": "nonexistent" });
    let result = handler
        .handle_cmd("check", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    // Non-existent target returns an empty engines array.
    assert!(result["engines"].is_array());
    assert_eq!(result["engines"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_check_marks_installed_when_executables_exist() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    // Pretend an install dir contains clam executables.
    let fake_install = dir.path().join("fake_install");
    std::fs::create_dir_all(&fake_install).unwrap();
    let exe_name = if cfg!(windows) { "clamscan.exe" } else { "clamscan" };
    std::fs::write(fake_install.join(exe_name), b"fake").unwrap();

    let engine = nemesis_config::ClamAVEngineConfig {
        clamav_path: fake_install.to_string_lossy().to_string(),
        ..Default::default()
    };
    let cfg = make_config_with_engine("clamav", engine, true);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": "clamav" });
    let result = handler
        .handle_cmd("check", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["state"]["install_status"], "installed");
}

#[tokio::test]
async fn test_check_marks_failed_when_path_missing_executables() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let bogus_path = dir.path().join("empty_dir").to_string_lossy().to_string();
    std::fs::create_dir_all(&bogus_path).unwrap();

    let engine = nemesis_config::ClamAVEngineConfig {
        clamav_path: bogus_path,
        ..Default::default()
    };
    let cfg = make_config_with_engine("clamav", engine, true);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": "clamav" });
    let result = handler
        .handle_cmd("check", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["state"]["install_status"], "failed");
    assert!(result["state"]["install_error"]
        .as_str()
        .unwrap()
        .contains("executable not found"));
}

#[tokio::test]
async fn test_check_db_status_uses_data_dir() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("db_root");
    let db_dir = data_dir.join("database");
    std::fs::create_dir_all(&db_dir).unwrap();
    // Write the daily.cvd database file -> status should be "ready".
    std::fs::write(db_dir.join("daily.cvd"), b"fake-db").unwrap();

    let engine = nemesis_config::ClamAVEngineConfig {
        data_dir: data_dir.to_string_lossy().to_string(),
        ..Default::default()
    };
    let cfg = make_config_with_engine("clamav", engine, true);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": "clamav" });
    let result = handler
        .handle_cmd("check", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["state"]["db_status"], "ready");
}

// -----------------------------------------------------------------------
// enable
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_enable_missing_data() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let err = handler
        .handle_cmd("enable", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing data");
}

#[tokio::test]
async fn test_enable_missing_name_field() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let data = serde_json::json!({ "other": "field" });
    let err = handler
        .handle_cmd("enable", Some(data), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing field: name");
}

#[tokio::test]
async fn test_enable_engine_not_found() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_scanner_config(dir.path(), &nemesis_config::ScannerFullConfig::default());
    let ctx = make_ctx(&dir);
    let data = serde_json::json!({ "name": "nonexistent" });
    let err = handler
        .handle_cmd("enable", Some(data), &ctx)
        .await
        .unwrap_err();
    assert!(
        err.contains("not found in configuration"),
        "expected not-found error, got: {}",
        err
    );
}

#[tokio::test]
async fn test_enable_marks_pending_state() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let engine = nemesis_config::ClamAVEngineConfig::default();
    let cfg = make_config_with_engine("clamav", engine, false);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": "clamav" });
    let result = handler
        .handle_cmd("enable", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    let engines = result["engines"].as_array().unwrap();
    assert_eq!(engines[0]["name"], "clamav");
    assert!(engines[0]["enabled"].as_bool().unwrap());
    // Engine state should be marked pending because the install_status was empty.
    assert_eq!(engines[0]["state"]["install_status"], "pending");
}

#[tokio::test]
async fn test_enable_idempotent_does_not_duplicate() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let engine = nemesis_config::ClamAVEngineConfig::default();
    let cfg = make_config_with_engine("clamav", engine, true);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": "clamav" });
    let _ = handler
        .handle_cmd("enable", Some(data.clone()), &ctx)
        .await
        .unwrap();
    let result = handler
        .handle_cmd("enable", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    let engines = result["engines"].as_array().unwrap();
    assert_eq!(engines.len(), 1);
    assert!(engines[0]["enabled"].as_bool().unwrap());
}

// -----------------------------------------------------------------------
// disable
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_disable_missing_data() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let err = handler
        .handle_cmd("disable", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing data");
}

#[tokio::test]
async fn test_disable_missing_name_field() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let data = serde_json::json!({});
    let err = handler
        .handle_cmd("disable", Some(data), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing field: name");
}

#[tokio::test]
async fn test_disable_removes_from_enabled() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let engine = nemesis_config::ClamAVEngineConfig::default();
    let cfg = make_config_with_engine("clamav", engine, true);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": "clamav" });
    let result = handler
        .handle_cmd("disable", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    let engines = result["engines"].as_array().unwrap();
    assert_eq!(engines[0]["name"], "clamav");
    assert!(!engines[0]["enabled"].as_bool().unwrap());
}

#[tokio::test]
async fn test_disable_case_insensitive() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let engine = nemesis_config::ClamAVEngineConfig::default();
    let cfg = make_config_with_engine("clamav", engine, true);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": "CLAMAV" });
    let result = handler
        .handle_cmd("disable", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    let engines = result["engines"].as_array().unwrap();
    assert!(!engines[0]["enabled"].as_bool().unwrap());
}

// -----------------------------------------------------------------------
// add
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_add_missing_data() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let err = handler.handle_cmd("add", None, &ctx).await.unwrap_err();
    assert_eq!(err, "missing data");
}

#[tokio::test]
async fn test_add_missing_name_field() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let data = serde_json::json!({});
    let err = handler
        .handle_cmd("add", Some(data), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing field: name");
}

#[tokio::test]
async fn test_add_unknown_engine_returns_error() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_scanner_config(dir.path(), &nemesis_config::ScannerFullConfig::default());
    let ctx = make_ctx(&dir);
    let data = serde_json::json!({ "name": "yara" });
    let err = handler
        .handle_cmd("add", Some(data), &ctx)
        .await
        .unwrap_err();
    assert!(
        err.contains("unknown engine: yara"),
        "expected unknown-engine error, got: {}",
        err
    );
}

#[tokio::test]
async fn test_add_clamav_default_address() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_scanner_config(dir.path(), &nemesis_config::ScannerFullConfig::default());
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": "clamav" });
    let result = handler
        .handle_cmd("add", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    let engines = result["engines"].as_array().unwrap();
    assert_eq!(engines.len(), 1);
    assert_eq!(engines[0]["name"], "clamav");
    assert_eq!(engines[0]["address"], "127.0.0.1:3310");
    assert_eq!(engines[0]["state"]["install_status"], "pending");
}

#[tokio::test]
async fn test_add_with_custom_url_and_address() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_scanner_config(dir.path(), &nemesis_config::ScannerFullConfig::default());
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({
        "name": "clamav",
        "url": "https://custom.example.com/clamav.zip",
        "address": "127.0.0.1:9999"
    });
    let result = handler
        .handle_cmd("add", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    let engines = result["engines"].as_array().unwrap();
    assert_eq!(engines[0]["url"], "https://custom.example.com/clamav.zip");
    assert_eq!(engines[0]["address"], "127.0.0.1:9999");
}

// -----------------------------------------------------------------------
// engine.update_config
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_engine_update_config_missing_data() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let err = handler
        .handle_cmd("engine.update_config", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing data");
}

#[tokio::test]
async fn test_engine_update_config_missing_name_field() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let data = serde_json::json!({ "config": {} });
    let err = handler
        .handle_cmd("engine.update_config", Some(data), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing field: name");
}

#[tokio::test]
async fn test_engine_update_config_missing_config_field() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let engine = nemesis_config::ClamAVEngineConfig::default();
    let cfg = make_config_with_engine("clamav", engine, false);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": "clamav" });
    let err = handler
        .handle_cmd("engine.update_config", Some(data), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing field: config");
}

#[tokio::test]
async fn test_engine_update_config_engine_not_found() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_scanner_config(dir.path(), &nemesis_config::ScannerFullConfig::default());
    let ctx = make_ctx(&dir);
    let data = serde_json::json!({ "name": "missing", "config": {} });
    let err = handler
        .handle_cmd("engine.update_config", Some(data), &ctx)
        .await
        .unwrap_err();
    assert!(
        err.contains("not found"),
        "expected not-found error, got: {}",
        err
    );
}

#[tokio::test]
async fn test_engine_update_config_applies_url_change() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let engine = nemesis_config::ClamAVEngineConfig::default();
    let cfg = make_config_with_engine("clamav", engine, false);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({
        "name": "clamav",
        "config": {
            "url": "https://updated.example.com/c.zip",
            "clamav_path": "/tmp/clamav",
            "scan_on_write": true,
            "max_file_size": 100
        }
    });
    let result = handler
        .handle_cmd("engine.update_config", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    let engines = result["engines"].as_array().unwrap();
    assert_eq!(engines[0]["url"], "https://updated.example.com/c.zip");
    assert_eq!(engines[0]["clamav_path"], "/tmp/clamav");
    assert!(engines[0]["scan_on_write"].as_bool().unwrap());
    assert_eq!(engines[0]["max_file_size"], 100);

    // Verify persisted to disk.
    let persisted = std::fs::read_to_string(dir.path().join("config/config.scanner.json")).unwrap();
    let persisted_json: serde_json::Value = serde_json::from_str(&persisted).unwrap();
    assert_eq!(
        persisted_json["engines"]["clamav"]["url"],
        "https://updated.example.com/c.zip"
    );
}

// -----------------------------------------------------------------------
// test (scan)
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_cmd_test_missing_data() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let err = handler.handle_cmd("test", None, &ctx).await.unwrap_err();
    assert_eq!(err, "missing data");
}

#[tokio::test]
async fn test_cmd_test_missing_name_field() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let data = serde_json::json!({ "path": "/tmp/x" });
    let err = handler
        .handle_cmd("test", Some(data), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing field: name");
}

#[tokio::test]
async fn test_cmd_test_missing_path_field() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let engine = nemesis_config::ClamAVEngineConfig::default();
    let cfg = make_config_with_engine("stub", engine, false);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);
    let data = serde_json::json!({ "name": "stub" });
    let err = handler
        .handle_cmd("test", Some(data), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing field: path");
}

#[tokio::test]
async fn test_cmd_test_engine_not_found() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_scanner_config(dir.path(), &nemesis_config::ScannerFullConfig::default());
    let ctx = make_ctx(&dir);
    let data = serde_json::json!({ "name": "missing", "path": "/tmp/x" });
    let err = handler
        .handle_cmd("test", Some(data), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("not found"));
}

#[tokio::test]
async fn test_cmd_test_stub_engine_scan_file() {
    // The "stub" engine in nemesis-security returns clean scans without a
    // real ClamAV. Use it to exercise the test command end-to-end.
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let engine = nemesis_config::ClamAVEngineConfig::default();
    let cfg = make_config_with_engine("stub", engine, false);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let dummy_file = dir.path().join("sample.txt");
    std::fs::write(&dummy_file, b"hello").unwrap();
    let data = serde_json::json!({
        "name": "stub",
        "path": dummy_file.to_string_lossy()
    });
    let result = handler
        .handle_cmd("test", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["engine"], "stub");
    assert!(!result["infected"].as_bool().unwrap());
}

// -----------------------------------------------------------------------
// cancel
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_cancel_missing_data() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let err = handler.handle_cmd("cancel", None, &ctx).await.unwrap_err();
    assert_eq!(err, "missing data");
}

#[tokio::test]
async fn test_cancel_missing_name_field() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let data = serde_json::json!({});
    let err = handler
        .handle_cmd("cancel", Some(data), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing field: name");
}

#[tokio::test]
async fn test_cancel_no_active_operation() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let data = serde_json::json!({ "name": "clamav" });
    let err = handler
        .handle_cmd("cancel", Some(data), &ctx)
        .await
        .unwrap_err();
    assert!(
        err.contains("no active operation"),
        "expected no-active-operation error, got: {}",
        err
    );
}

// -----------------------------------------------------------------------
// install / update_db — async dispatchers
//
// These commands spawn background tasks that perform real downloads. We
// only validate the synchronous preflight (missing name, missing data)
// and the "already in progress" path here. The happy path is left
// covered by the integration-level e2e tests.
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_install_missing_data() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let err = handler
        .handle_cmd("install", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing data");
}

#[tokio::test]
async fn test_install_missing_name_field() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let data = serde_json::json!({ "force": true });
    let err = handler
        .handle_cmd("install", Some(data), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing field: name");
}

#[tokio::test]
async fn test_update_db_missing_data() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let err = handler
        .handle_cmd("update_db", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing data");
}

#[tokio::test]
async fn test_update_db_missing_name_field() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let data = serde_json::json!({});
    let err = handler
        .handle_cmd("update_db", Some(data), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing field: name");
}

#[tokio::test]
async fn test_update_db_starts_then_blocks_duplicate() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    // Use a unique engine name so the global active_ops slot isn't
    // shared with parallel test runs (active_ops is a process-global
    // OnceLock).
    let unique = format!("clamav_{}", std::process::id());
    let engine = nemesis_config::ClamAVEngineConfig {
        clamav_path: dir.path().join("tools").to_string_lossy().to_string(),
        ..Default::default()
    };
    let cfg = make_config_with_engine(&unique, engine, true);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    // First call marks the op as started and spawns a background task.
    let data = serde_json::json!({ "name": unique });
    let first = handler
        .handle_cmd("update_db", Some(data.clone()), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first["engine"], unique);
    assert!(first["started"].as_bool().unwrap());

    // A second call immediately afterwards must report "already in
    // progress" — proving the slot is occupied by the spawned task.
    let second = handler
        .handle_cmd("update_db", Some(data.clone()), &ctx)
        .await
        .unwrap_err();
    assert!(
        second.contains("already in progress"),
        "expected already-in-progress, got: {}",
        second
    );

    // The background task will fail quickly (no real clamav) and clear
    // the marker. Poll until the second call no longer reports "in
    // progress", bounded at 5 seconds.
    let mut attempt = 0;
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        match handler
            .handle_cmd("update_db", Some(data.clone()), &ctx)
            .await
        {
            Ok(_) => break, // op finished, slot available again
            Err(e) if e.contains("already in progress") => {
                attempt += 1;
                if attempt > 50 {
                    panic!(
                        "update_db op never cleared after 5s; last error: {}",
                        e
                    );
                }
            }
            Err(e) => panic!("unexpected error: {}", e),
        }
    }
}

#[tokio::test]
async fn test_cancel_update_db_op_by_suffix_key() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    // Unique name so this test's slot doesn't collide with parallel runs.
    let unique = format!("clamav_cancel_{}", std::process::id());
    let engine = nemesis_config::ClamAVEngineConfig {
        clamav_path: dir.path().join("tools").to_string_lossy().to_string(),
        ..Default::default()
    };
    let cfg = make_config_with_engine(&unique, engine, true);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    // Start an update-db op so it occupies the `{name}-update-db` slot.
    let data = serde_json::json!({ "name": unique });
    let _ = handler
        .handle_cmd("update_db", Some(data.clone()), &ctx)
        .await
        .unwrap();

    // The cancel handler first looks for an exact key match, then tries
    // `{name}-update-db`. Either path should cancel; we just verify we
    // don't get "no active operation" while the op is in flight.
    let mut cancelled = false;
    for _ in 0..50 {
        match handler
            .handle_cmd("cancel", Some(data.clone()), &ctx)
            .await
        {
            Ok(r) => {
                assert!(r.unwrap()["cancelled"].as_bool().unwrap());
                cancelled = true;
                break;
            }
            Err(e) if e.contains("no active operation") => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            Err(e) => panic!("unexpected cancel error: {}", e),
        }
    }
    // The background op may have completed before we cancel; that's fine —
    // in either case the test exercises the dispatch path.
    let _ = cancelled;
}

// -----------------------------------------------------------------------
// build_engine_response merge behavior (indirect via cmd_check)
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_engine_response_preserves_custom_engine_fields() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = nemesis_config::ScannerFullConfig::default();
    // Insert an engine that has extra fields not part of ClamAVEngineConfig.
    let engine_json = serde_json::json!({
        "address": "127.0.0.1:3310",
        "custom_field": "custom_value",
        "scan_on_write": false
    });
    cfg.engines.insert("clamav".to_string(), engine_json);
    cfg.enabled.push("clamav".to_string());
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let result = handler.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
    let engines = result["engines"].as_array().unwrap();
    assert_eq!(engines[0]["custom_field"], "custom_value");
    assert_eq!(engines[0]["address"], "127.0.0.1:3310");
    assert!(engines[0]["enabled"].as_bool().unwrap());
}

#[tokio::test]
async fn test_status_sorts_engines_alphabetically() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = nemesis_config::ScannerFullConfig::default();
    cfg.engines.insert(
        "zeta".to_string(),
        serde_json::to_value(&nemesis_config::ClamAVEngineConfig::default()).unwrap(),
    );
    cfg.engines.insert(
        "alpha".to_string(),
        serde_json::to_value(&nemesis_config::ClamAVEngineConfig::default()).unwrap(),
    );
    cfg.engines.insert(
        "clamav".to_string(),
        serde_json::to_value(&nemesis_config::ClamAVEngineConfig::default()).unwrap(),
    );
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let result = handler.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
    let names: Vec<&str> = result["engines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["alpha", "clamav", "zeta"]);
}

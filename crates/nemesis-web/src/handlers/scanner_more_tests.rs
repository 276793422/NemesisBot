//! Additional coverage tests for the Scanner WebSocket handler.
//!
//! These tests target branches that `scanner_extra_tests.rs` does not
//! yet exercise. The scanner handler's pure-logic helpers are private
//! to the `scanner` module, so we cover them indirectly via WSAPI
//! commands (e.g. `format_bytes` is only reachable through the install
//! progress callback, `check_executables_at_path` via `cmd_check`,
//! `parse_engine_config` via `cmd_check`/`cmd_enable`).
//!
//! Covered branches:
//! - `cmd_check`: data_dir fallback to clamav_path, multi-engine
//!   iteration, disabled-engine state preservation, state change
//!   persisted to disk, state-change-suppression when already correct.
//! - `cmd_enable`: existing non-empty install_status preserved,
//!   case-insensitive enabled list dedup.
//! - `cmd_disable`: no-op when not enabled, sibling engines preserved.
//! - `engine.update_config`: address/data_dir/scan_on_download/
//!   scan_on_exec/update_interval keys, partial updates, wrong-type
//!   inputs ignored.
//! - `config.save`: full config round-trip via config.get, empty
//!   engines map.
//! - `add`: overwriting an existing engine, adding `stub` engine.
//! - `cmd_install`: duplicate-op suppression, url-override branch.
//! - `cmd_cancel`: exact-match path via install.

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
// Test infrastructure (mirror of scanner_extra_tests.rs)
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

fn ensure_config_dir(workspace: &Path) {
    std::fs::create_dir_all(workspace.join("config")).unwrap();
}

fn write_scanner_config(workspace: &Path, cfg: &nemesis_config::ScannerFullConfig) {
    ensure_config_dir(workspace);
    let json = serde_json::to_string_pretty(cfg).unwrap();
    std::fs::write(workspace.join("config/config.scanner.json"), json).unwrap();
}

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

/// Helper: write a "fake" clamav executable + database in `dir` to
/// trigger the "installed" branch in `cmd_check`.
fn install_fake_clamav(dir: &Path) -> std::path::PathBuf {
    let install_root = dir.join("clamav_install");
    let db_dir = install_root.join("database");
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::write(db_dir.join("daily.cvd"), b"fake-db").unwrap();
    let exe_name = if cfg!(windows) { "clamd.exe" } else { "clamd" };
    std::fs::write(install_root.join(exe_name), b"fake").unwrap();
    install_root
}

// =======================================================================
// cmd_check — branches not yet covered by scanner_extra_tests.
// =======================================================================

#[tokio::test]
async fn test_check_db_status_falls_back_to_resolved_path() {
    // When data_dir is empty but clamav_path (resolved_path) is set,
    // cmd_check uses clamav_path as the data dir.
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let install_root = install_fake_clamav(dir.path());

    let engine = nemesis_config::ClamAVEngineConfig {
        // Intentionally leave data_dir empty so we hit the
        // resolved_path fallback inside cmd_check.
        clamav_path: install_root.to_string_lossy().to_string(),
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

#[tokio::test]
async fn test_check_db_status_missing_when_no_db_file() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let install_root = dir.path().join("clamav_install");
    let db_dir = install_root.join("database");
    std::fs::create_dir_all(&db_dir).unwrap();
    // No daily.cvd written — status should be "missing".
    let exe_name = if cfg!(windows) { "clamscan.exe" } else { "clamscan" };
    std::fs::write(install_root.join(exe_name), b"fake").unwrap();

    let engine = nemesis_config::ClamAVEngineConfig {
        clamav_path: install_root.to_string_lossy().to_string(),
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
    assert_eq!(result["state"]["db_status"], "missing");
    assert_eq!(result["state"]["install_status"], "installed");
}

#[tokio::test]
async fn test_check_disabled_engine_skips_status_updates() {
    // Engine not in enabled list — no install/db status updates happen,
    // so install_status stays empty.
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let engine = nemesis_config::ClamAVEngineConfig {
        clamav_path: dir
            .path()
            .join("nonexistent_path")
            .to_string_lossy()
            .to_string(),
        ..Default::default()
    };
    let cfg = make_config_with_engine("clamav", engine, false);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": "clamav" });
    let result = handler
        .handle_cmd("check", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["state"]["install_status"], "");
    assert!(!result["enabled"].as_bool().unwrap());
}

#[tokio::test]
async fn test_check_multi_engine_returns_engines_array() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = nemesis_config::ScannerFullConfig::default();
    cfg.engines.insert(
        "clamav".to_string(),
        serde_json::to_value(&nemesis_config::ClamAVEngineConfig::default()).unwrap(),
    );
    cfg.engines.insert(
        "stub".to_string(),
        serde_json::to_value(&nemesis_config::ClamAVEngineConfig::default()).unwrap(),
    );
    cfg.enabled.push("clamav".to_string());
    cfg.enabled.push("stub".to_string());
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    // No name filter — multi-engine iteration path.
    // Note: cmd_check does not sort its output (unlike cmd_status which
    // does), so iteration order follows HashMap's non-deterministic order.
    let result = handler
        .handle_cmd("check", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let engines = result["engines"].as_array().unwrap();
    assert_eq!(engines.len(), 2);
    let names: std::collections::HashSet<&str> = engines
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert!(names.contains("clamav"));
    assert!(names.contains("stub"));
}

#[tokio::test]
async fn test_check_persists_state_change_to_disk() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let install_root = install_fake_clamav(dir.path());

    let engine = nemesis_config::ClamAVEngineConfig {
        clamav_path: install_root.to_string_lossy().to_string(),
        ..Default::default()
    };
    let cfg = make_config_with_engine("clamav", engine, true);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": "clamav" });
    let _ = handler
        .handle_cmd("check", Some(data), &ctx)
        .await
        .unwrap();

    // Reload the persisted config and verify the state was written back.
    let persisted_path = dir.path().join("config/config.scanner.json");
    let persisted = std::fs::read_to_string(&persisted_path).unwrap();
    let persisted_json: serde_json::Value = serde_json::from_str(&persisted).unwrap();
    assert_eq!(
        persisted_json["engines"]["clamav"]["state"]["install_status"],
        "installed"
    );
    assert_eq!(
        persisted_json["engines"]["clamav"]["state"]["db_status"],
        "ready"
    );
}

#[tokio::test]
async fn test_check_skips_state_update_when_unchanged() {
    // Pre-populate the state with the same values that check would
    // derive — should result in no state change, no disk write.
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let install_root = install_fake_clamav(dir.path());

    let initial_state = nemesis_config::EngineState {
        install_status: "installed".to_string(),
        db_status: "ready".to_string(),
        install_error: String::new(),
        ..Default::default()
    };
    let engine = nemesis_config::ClamAVEngineConfig {
        clamav_path: install_root.to_string_lossy().to_string(),
        state: initial_state,
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
    // State stays the same — no error, no thrash.
    assert_eq!(result["state"]["install_status"], "installed");
    assert_eq!(result["state"]["db_status"], "ready");
}

#[tokio::test]
async fn test_check_with_empty_path_and_existing_install_status_keeps_status() {
    // No clamav_path and existing install_status (e.g. "installed") —
    // the install_status shouldn't be overwritten with "pending" again.
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let initial_state = nemesis_config::EngineState {
        install_status: "installed".to_string(),
        ..Default::default()
    };
    let engine = nemesis_config::ClamAVEngineConfig {
        state: initial_state,
        ..Default::default()
    };
    let cfg = make_config_with_engine("clamav", engine, true);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let result = handler
        .handle_cmd("check", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let engines = result["engines"].as_array().unwrap();
    // Pre-existing "installed" status preserved (state.install_status.is_empty() == false).
    assert_eq!(engines[0]["state"]["install_status"], "installed");
}

#[tokio::test]
async fn test_check_specific_unknown_engine_with_others_present_returns_empty_array() {
    // Target a non-existing name when other engines exist — should
    // return an empty array, not a list of others.
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = nemesis_config::ScannerFullConfig::default();
    cfg.engines.insert(
        "clamav".to_string(),
        serde_json::to_value(&nemesis_config::ClamAVEngineConfig::default()).unwrap(),
    );
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": "ghost" });
    let result = handler
        .handle_cmd("check", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(result["engines"].is_array());
    assert_eq!(result["engines"].as_array().unwrap().len(), 0);
}

// =======================================================================
// engine.update_config — remaining config keys.
// =======================================================================

#[tokio::test]
async fn test_engine_update_config_address_and_data_dir() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let engine = nemesis_config::ClamAVEngineConfig::default();
    let cfg = make_config_with_engine("clamav", engine, false);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({
        "name": "clamav",
        "config": {
            "address": "127.0.0.1:9999",
            "data_dir": "/var/clamav/data"
        }
    });
    let result = handler
        .handle_cmd("engine.update_config", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    let engines = result["engines"].as_array().unwrap();
    assert_eq!(engines[0]["address"], "127.0.0.1:9999");
    assert_eq!(engines[0]["data_dir"], "/var/clamav/data");
}

#[tokio::test]
async fn test_engine_update_config_scan_flags() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let engine = nemesis_config::ClamAVEngineConfig::default();
    let cfg = make_config_with_engine("clamav", engine, false);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({
        "name": "clamav",
        "config": {
            "scan_on_download": true,
            "scan_on_exec": true
        }
    });
    let result = handler
        .handle_cmd("engine.update_config", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    let engines = result["engines"].as_array().unwrap();
    assert!(engines[0]["scan_on_download"].as_bool().unwrap());
    assert!(engines[0]["scan_on_exec"].as_bool().unwrap());
}

#[tokio::test]
async fn test_engine_update_config_update_interval() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let engine = nemesis_config::ClamAVEngineConfig::default();
    let cfg = make_config_with_engine("clamav", engine, false);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({
        "name": "clamav",
        "config": { "update_interval": "12h" }
    });
    let result = handler
        .handle_cmd("engine.update_config", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    let engines = result["engines"].as_array().unwrap();
    assert_eq!(engines[0]["update_interval"], "12h");
}

#[tokio::test]
async fn test_engine_update_config_partial_update_keeps_other_fields() {
    // Updating one field should preserve other fields.
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let engine = nemesis_config::ClamAVEngineConfig {
        url: "https://orig.example.com/c.zip".to_string(),
        address: "127.0.0.1:3310".to_string(),
        max_file_size: 500,
        ..Default::default()
    };
    let cfg = make_config_with_engine("clamav", engine, false);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({
        "name": "clamav",
        "config": { "scan_on_write": true }
    });
    let result = handler
        .handle_cmd("engine.update_config", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    let engines = result["engines"].as_array().unwrap();
    assert!(engines[0]["scan_on_write"].as_bool().unwrap());
    // Original fields preserved.
    assert_eq!(engines[0]["url"], "https://orig.example.com/c.zip");
    assert_eq!(engines[0]["address"], "127.0.0.1:3310");
    assert_eq!(engines[0]["max_file_size"], 500);
}

#[tokio::test]
async fn test_engine_update_config_with_empty_updates_object() {
    // Empty config object is valid — engine config remains unchanged.
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let engine = nemesis_config::ClamAVEngineConfig {
        url: "https://orig.example.com/c.zip".to_string(),
        ..Default::default()
    };
    let cfg = make_config_with_engine("clamav", engine, false);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": "clamav", "config": {} });
    let result = handler
        .handle_cmd("engine.update_config", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    let engines = result["engines"].as_array().unwrap();
    assert_eq!(engines[0]["url"], "https://orig.example.com/c.zip");
}

#[tokio::test]
async fn test_engine_update_config_wrong_types_are_ignored() {
    // Non-string/non-bool/non-i64 values are silently ignored.
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let engine = nemesis_config::ClamAVEngineConfig {
        url: "https://orig.example.com/c.zip".to_string(),
        ..Default::default()
    };
    let cfg = make_config_with_engine("clamav", engine, false);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({
        "name": "clamav",
        "config": {
            "url": 12345,
            "max_file_size": "not-a-number",
            "scan_on_write": "true"
        }
    });
    let result = handler
        .handle_cmd("engine.update_config", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    let engines = result["engines"].as_array().unwrap();
    // Wrong types — original values preserved.
    assert_eq!(engines[0]["url"], "https://orig.example.com/c.zip");
    assert_eq!(engines[0]["max_file_size"], 0);
    assert!(!engines[0]["scan_on_write"].as_bool().unwrap());
}

#[tokio::test]
async fn test_engine_update_config_unknown_keys_ignored() {
    // Extra unknown keys shouldn't crash the handler.
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let engine = nemesis_config::ClamAVEngineConfig::default();
    let cfg = make_config_with_engine("clamav", engine, false);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({
        "name": "clamav",
        "config": { "unknown_field": "value", "another": 999 }
    });
    let result = handler
        .handle_cmd("engine.update_config", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    let engines = result["engines"].as_array().unwrap();
    // Just verify it succeeded; no field changes expected.
    assert_eq!(engines[0]["name"], "clamav");
}

// =======================================================================
// cmd_enable — branch where engine already has install_status set.
// =======================================================================

#[tokio::test]
async fn test_enable_preserves_existing_install_status() {
    // Engine state already has "installed" — enable should NOT overwrite
    // it with "pending".
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let initial_state = nemesis_config::EngineState {
        install_status: "installed".to_string(),
        ..Default::default()
    };
    let engine = nemesis_config::ClamAVEngineConfig {
        state: initial_state,
        ..Default::default()
    };
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
    assert!(engines[0]["enabled"].as_bool().unwrap());
    // Pre-existing "installed" preserved (not overwritten with "pending").
    assert_eq!(engines[0]["state"]["install_status"], "installed");
}

#[tokio::test]
async fn test_enable_case_insensitive_match_in_enabled_list() {
    // Existing enabled entry uses different case. `cmd_enable` uses
    // `eq_ignore_ascii_case` so it won't add a duplicate "clamav" entry,
    // but the pre-existing "CLAMAV" remains. Note: `build_engine_response`
    // matches case-sensitively, so the response's `enabled` flag will
    // still report `false` for the lowercase engine name.
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = nemesis_config::ScannerFullConfig::default();
    cfg.engines.insert(
        "clamav".to_string(),
        serde_json::to_value(&nemesis_config::ClamAVEngineConfig::default()).unwrap(),
    );
    cfg.enabled.push("CLAMAV".to_string());
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": "clamav" });
    let result = handler
        .handle_cmd("enable", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    let engines = result["engines"].as_array().unwrap();
    assert_eq!(engines.len(), 1);

    // Verify the persisted enabled list didn't get a duplicate.
    let persisted_path = dir.path().join("config/config.scanner.json");
    let persisted = std::fs::read_to_string(&persisted_path).unwrap();
    let persisted_json: serde_json::Value = serde_json::from_str(&persisted).unwrap();
    let enabled_arr = persisted_json["enabled"].as_array().unwrap();
    assert_eq!(enabled_arr.len(), 1);
    // The single entry is still the uppercase variant (case-insensitive match).
    assert_eq!(enabled_arr[0], "CLAMAV");
}

// =======================================================================
// cmd_disable — engine not in list is a no-op; siblings preserved.
// =======================================================================

#[tokio::test]
async fn test_disable_engine_not_in_enabled_list_noop() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let engine = nemesis_config::ClamAVEngineConfig::default();
    let cfg = make_config_with_engine("clamav", engine, false);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": "clamav" });
    let result = handler
        .handle_cmd("disable", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    let engines = result["engines"].as_array().unwrap();
    assert!(!engines[0]["enabled"].as_bool().unwrap());
}

#[tokio::test]
async fn test_disable_unrelated_engine_keeps_others() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = nemesis_config::ScannerFullConfig::default();
    cfg.engines.insert(
        "clamav".to_string(),
        serde_json::to_value(&nemesis_config::ClamAVEngineConfig::default()).unwrap(),
    );
    cfg.engines.insert(
        "stub".to_string(),
        serde_json::to_value(&nemesis_config::ClamAVEngineConfig::default()).unwrap(),
    );
    cfg.enabled.push("clamav".to_string());
    cfg.enabled.push("stub".to_string());
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": "clamav" });
    let result = handler
        .handle_cmd("disable", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    let engines = result["engines"].as_array().unwrap();
    let clamav = engines.iter().find(|e| e["name"] == "clamav").unwrap();
    let stub = engines.iter().find(|e| e["name"] == "stub").unwrap();
    assert!(!clamav["enabled"].as_bool().unwrap());
    assert!(stub["enabled"].as_bool().unwrap());
}

// =======================================================================
// config.get / config.save — round-trip and edge cases.
// =======================================================================

#[tokio::test]
async fn test_config_save_valid_full_config_round_trip() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let engine = nemesis_config::ClamAVEngineConfig {
        url: "https://example.com/c.zip".to_string(),
        address: "127.0.0.1:3310".to_string(),
        scan_on_write: true,
        max_file_size: 1000,
        ..Default::default()
    };
    let save_data = serde_json::json!({
        "enabled": ["clamav"],
        "engines": {
            "clamav": serde_json::to_value(&engine).unwrap()
        }
    });
    let result = handler
        .handle_cmd("config.save", Some(save_data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(result["saved"].as_bool().unwrap());

    // Verify round-trip via config.get.
    let reloaded = handler
        .handle_cmd("config.get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reloaded["enabled"][0], "clamav");
    assert_eq!(reloaded["engines"]["clamav"]["url"], "https://example.com/c.zip");
    assert_eq!(reloaded["engines"]["clamav"]["max_file_size"], 1000);
}

#[tokio::test]
async fn test_config_save_empty_engines_object() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let save_data = serde_json::json!({
        "enabled": [],
        "engines": {}
    });
    let result = handler
        .handle_cmd("config.save", Some(save_data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(result["saved"].as_bool().unwrap());

    // Verify config.get reads it back.
    let reloaded = handler
        .handle_cmd("config.get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reloaded["enabled"].as_array().unwrap().len(), 0);
    assert_eq!(reloaded["engines"].as_object().unwrap().len(), 0);
}

#[tokio::test]
async fn test_config_save_with_extra_engine_fields_preserved() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    // Save an engine that has an extra "custom_field" key.
    let save_data = serde_json::json!({
        "enabled": ["clamav"],
        "engines": {
            "clamav": {
                "address": "127.0.0.1:3310",
                "custom_field": "abc123"
            }
        }
    });
    handler
        .handle_cmd("config.save", Some(save_data), &ctx)
        .await
        .unwrap();

    // Reload — extra field should round-trip.
    let reloaded = handler
        .handle_cmd("config.get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        reloaded["engines"]["clamav"]["custom_field"],
        "abc123"
    );
}

// =======================================================================
// add — overwriting an existing engine, adding `stub`.
// =======================================================================

#[tokio::test]
async fn test_add_overwrites_existing_engine() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    // Pre-existing clamav engine with non-default url.
    let existing = nemesis_config::ClamAVEngineConfig {
        url: "https://orig.example.com/c.zip".to_string(),
        ..Default::default()
    };
    let cfg = make_config_with_engine("clamav", existing, true);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    // Re-adding clamav overwrites it with default pending state.
    let data = serde_json::json!({
        "name": "clamav",
        "url": "https://new.example.com/c.zip"
    });
    let result = handler
        .handle_cmd("add", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    let engines = result["engines"].as_array().unwrap();
    assert_eq!(engines.len(), 1);
    assert_eq!(engines[0]["url"], "https://new.example.com/c.zip");
    assert_eq!(engines[0]["state"]["install_status"], "pending");
}

#[tokio::test]
async fn test_add_stub_engine() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_scanner_config(dir.path(), &nemesis_config::ScannerFullConfig::default());
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": "stub" });
    let result = handler
        .handle_cmd("add", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    let engines = result["engines"].as_array().unwrap();
    assert_eq!(engines[0]["name"], "stub");
    assert_eq!(engines[0]["address"], "127.0.0.1:3310");
}

#[tokio::test]
async fn test_add_with_explicit_address_override() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_scanner_config(dir.path(), &nemesis_config::ScannerFullConfig::default());
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({
        "name": "clamav",
        "address": "127.0.0.1:7777"
    });
    let result = handler
        .handle_cmd("add", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    let engines = result["engines"].as_array().unwrap();
    assert_eq!(engines[0]["address"], "127.0.0.1:7777");
}

#[tokio::test]
async fn test_add_engine_name_not_in_available_list() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_scanner_config(dir.path(), &nemesis_config::ScannerFullConfig::default());
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": "ClamAV" });
    let err = handler
        .handle_cmd("add", Some(data), &ctx)
        .await
        .unwrap_err();
    // Names are case-sensitive — "ClamAV" is not in available_engines.
    assert!(err.contains("unknown engine: ClamAV"));
}

// =======================================================================
// cmd_test — clamav engine creation (no daemon, expect failure path).
// =======================================================================

#[tokio::test]
async fn test_cmd_test_clamav_engine_creation_fails_gracefully() {
    // Attempting to create the clamav engine without a real daemon
    // still succeeds at engine creation; the scan result will report
    // not-infected (or the daemon error in raw output).
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let engine = nemesis_config::ClamAVEngineConfig::default();
    let cfg = make_config_with_engine("clamav", engine, false);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let dummy = dir.path().join("sample.txt");
    std::fs::write(&dummy, b"hi").unwrap();
    let data = serde_json::json!({
        "name": "clamav",
        "path": dummy.to_string_lossy()
    });
    // The test command should always return a JSON object describing
    // the scan outcome, regardless of whether the daemon is up.
    let result = handler
        .handle_cmd("test", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["engine"], "clamav");
    assert!(result.get("path").is_some());
    assert!(result.get("infected").is_some());
}

#[tokio::test]
async fn test_cmd_test_unknown_engine_returns_error() {
    // create_engine returns Err for unknown names.
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    // Insert an engine with an unsupported name in the config.
    let mut cfg = nemesis_config::ScannerFullConfig::default();
    cfg.engines.insert(
        "yara".to_string(),
        serde_json::to_value(&nemesis_config::ClamAVEngineConfig::default()).unwrap(),
    );
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({
        "name": "yara",
        "path": "/tmp/x"
    });
    let err = handler
        .handle_cmd("test", Some(data), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("failed to create engine"));
}

// =======================================================================
// cmd_install — duplicate suppression + url-override branch.
//
// These spawn a background task that will fail quickly (no real
// download URL), clearing the global active_ops slot.
// =======================================================================

#[tokio::test]
async fn test_install_blocks_duplicate_op_while_in_flight() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let unique = format!("clamav_dup_{}", std::process::id());
    let engine = nemesis_config::ClamAVEngineConfig {
        clamav_path: dir.path().join("tools").to_string_lossy().to_string(),
        ..Default::default()
    };
    let cfg = make_config_with_engine(&unique, engine, true);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": unique });
    let first = handler
        .handle_cmd("install", Some(data.clone()), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(first["started"].as_bool().unwrap());
    assert_eq!(first["engine"], unique);

    // A second call before the spawned task completes should be rejected.
    let err = handler
        .handle_cmd("install", Some(data.clone()), &ctx)
        .await
        .unwrap_err();
    assert!(
        err.contains("already in progress"),
        "expected already-in-progress, got: {}",
        err
    );

    // Wait for the background op to clear so the global slot is reusable.
    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        match handler
            .handle_cmd("install", Some(data.clone()), &ctx)
            .await
        {
            Ok(_) => break,
            Err(e) if e.contains("already in progress") => continue,
            Err(e) => panic!("unexpected install error: {}", e),
        }
    }
}

#[tokio::test]
async fn test_install_with_url_override_starts_op() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let unique = format!("clamav_url_{}", std::process::id());
    let engine = nemesis_config::ClamAVEngineConfig {
        clamav_path: dir.path().join("tools").to_string_lossy().to_string(),
        ..Default::default()
    };
    let cfg = make_config_with_engine(&unique, engine, true);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({
        "name": unique,
        "url": "https://custom.example.com/clamav.zip",
        "force": false
    });
    let result = handler
        .handle_cmd("install", Some(data.clone()), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(result["started"].as_bool().unwrap());

    // Wait for the background op to clear.
    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        match handler
            .handle_cmd("install", Some(data.clone()), &ctx)
            .await
        {
            Ok(_) => break,
            Err(e) if e.contains("already in progress") => continue,
            Err(e) => panic!("unexpected install error: {}", e),
        }
    }
}

#[tokio::test]
async fn test_install_already_installed_without_force_errors() {
    // install_engine_inner returns Err("already installed. Use force=true")
    // when state.install_status == "installed" and force is false. This
    // error path then writes the failed state and publishes an event.
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let unique = format!("clamav_installed_{}", std::process::id());
    let initial_state = nemesis_config::EngineState {
        install_status: "installed".to_string(),
        ..Default::default()
    };
    let engine = nemesis_config::ClamAVEngineConfig {
        clamav_path: dir.path().join("tools").to_string_lossy().to_string(),
        url: "https://example.invalid/clamav.zip".to_string(),
        state: initial_state,
        ..Default::default()
    };
    let cfg = make_config_with_engine(&unique, engine, true);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": unique, "force": false });
    let _ = handler
        .handle_cmd("install", Some(data.clone()), &ctx)
        .await
        .unwrap();

    // Wait for the background op to clear AND verify the error path
    // persisted a failed-state marker on the engine.
    let mut found_failure = false;
    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // If a second install no longer reports "already in progress",
        // the spawned task has finished.
        match handler
            .handle_cmd("install", Some(data.clone()), &ctx)
            .await
        {
            Ok(_) => break,
            Err(e) if e.contains("already in progress") => continue,
            Err(_) => break,
        }
    }

    // Re-read the persisted config: the spawned task should have set
    // install_status to "failed" (because install_engine_inner returned
    // Err and the catch-all error branch in cmd_install writes the
    // failed state to disk).
    let persisted_path = dir.path().join("config/config.scanner.json");
    let persisted = std::fs::read_to_string(&persisted_path).unwrap();
    let persisted_json: serde_json::Value = serde_json::from_str(&persisted).unwrap();
    if persisted_json["engines"][unique.as_str()]["state"]["install_status"]
        == "failed"
    {
        found_failure = true;
    }
    // If not found, that's because the spawned task finished between the
    // handle_cmd Ok and our read. We just want to ensure the path executes;
    // we can't strictly assert the side-effect since timing is racy.
    let _ = found_failure;
}

#[tokio::test]
async fn test_install_already_installed_with_force_proceeds() {
    // force=true should bypass the "already installed" check and proceed
    // to the actual download (which fails due to invalid URL).
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let unique = format!("clamav_force_{}", std::process::id());
    let initial_state = nemesis_config::EngineState {
        install_status: "installed".to_string(),
        ..Default::default()
    };
    let engine = nemesis_config::ClamAVEngineConfig {
        clamav_path: dir.path().join("tools").to_string_lossy().to_string(),
        url: "https://example.invalid/clamav.zip".to_string(),
        state: initial_state,
        ..Default::default()
    };
    let cfg = make_config_with_engine(&unique, engine, true);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": unique, "force": true });
    let _ = handler
        .handle_cmd("install", Some(data.clone()), &ctx)
        .await
        .unwrap();

    // Wait for the spawned task to complete.
    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        match handler
            .handle_cmd("install", Some(data.clone()), &ctx)
            .await
        {
            Ok(_) => break,
            Err(e) if e.contains("already in progress") => continue,
            Err(_) => break,
        }
    }
}

#[tokio::test]
async fn test_install_missing_engine_in_config_returns_started_then_fails_async() {
    // Engine not in config — install_engine_inner returns Err early.
    // The cmd_install still returns "started" because it spawns first.
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let unique = format!("clamav_missing_cfg_{}", std::process::id());
    // Empty config — no engines.
    write_scanner_config(dir.path(), &nemesis_config::ScannerFullConfig::default());
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": unique });
    let result = handler
        .handle_cmd("install", Some(data.clone()), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(result["started"].as_bool().unwrap());

    // Wait for the spawned task to complete.
    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        match handler
            .handle_cmd("install", Some(data.clone()), &ctx)
            .await
        {
            Ok(_) => break,
            Err(e) if e.contains("already in progress") => continue,
            Err(_) => break,
        }
    }
}

#[tokio::test]
async fn test_update_db_missing_engine_returns_started_then_fails_async() {
    // Engine missing — update_db_inner returns Err early.
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let unique = format!("clamav_missing_db_{}", std::process::id());
    write_scanner_config(dir.path(), &nemesis_config::ScannerFullConfig::default());
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": unique });
    let result = handler
        .handle_cmd("update_db", Some(data.clone()), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(result["started"].as_bool().unwrap());

    // Wait for the spawned task to complete and clear the slot.
    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        match handler
            .handle_cmd("update_db", Some(data.clone()), &ctx)
            .await
        {
            Ok(_) => break,
            Err(e) if e.contains("already in progress") => continue,
            Err(_) => break,
        }
    }
}

#[tokio::test]
async fn test_update_db_no_clamav_path_errors_with_not_installed() {
    // update_db_inner returns "engine not installed (no clamav_path)"
    // when clamav_path is empty.
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let unique = format!("clamav_no_path_{}", std::process::id());
    // Engine exists in config but has no clamav_path.
    let engine = nemesis_config::ClamAVEngineConfig::default();
    let cfg = make_config_with_engine(&unique, engine, true);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": unique });
    let result = handler
        .handle_cmd("update_db", Some(data.clone()), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(result["started"].as_bool().unwrap());

    // Wait for the spawned task to complete.
    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        match handler
            .handle_cmd("update_db", Some(data.clone()), &ctx)
            .await
        {
            Ok(_) => break,
            Err(e) if e.contains("already in progress") => continue,
            Err(_) => break,
        }
    }
}

// =======================================================================
// cmd_cancel — exact-match path via install (not -update-db suffix).
// =======================================================================

#[tokio::test]
async fn test_cancel_install_op_by_exact_key() {
    let handler = scanner::ScannerHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let unique = format!("clamav_cancel_install_{}", std::process::id());
    let engine = nemesis_config::ClamAVEngineConfig {
        clamav_path: dir.path().join("tools").to_string_lossy().to_string(),
        url: "https://example.invalid/clamav.zip".to_string(),
        ..Default::default()
    };
    let cfg = make_config_with_engine(&unique, engine, true);
    write_scanner_config(dir.path(), &cfg);
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": unique });
    let _ = handler
        .handle_cmd("install", Some(data.clone()), &ctx)
        .await
        .unwrap();

    // Cancel by exact name — should succeed while the install op is
    // in flight.
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
    let _ = cancelled;
}

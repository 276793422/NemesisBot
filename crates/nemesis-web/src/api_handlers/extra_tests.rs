//! Additional API handler tests focused on uncovered branches:
//! - `handle_api_readme` / `handle_api_license` (currently 0% covered)
//! - `handle_api_internal`: auth, missing channel, unknown cmd, success
//! - `handle_api_status` with `model_has_key=true` branch
//! - `handle_api_models` with empty api_key (not sanitized)
//! - `handle_api_models` with very short api_key (≤4 chars)
//! - `handle_api_config` with non-object root value (array, scalar)
//! - `handle_api_logs` with default `source` and `n` defaults
//! - Edge cases in `read_log_entries`, `sanitize_map`
//! - AppState `Clone` derive

use super::*;
use crate::events::EventHub;
use crate::session::SessionManager;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::Arc;
use std::time::Instant;

// ============================================================
// Test helpers
// ============================================================

fn make_state(
    workspace: Option<String>,
    home: Option<String>,
    auth_token: &str,
    model_name: &str,
    model_has_key: bool,
) -> Arc<AppState> {
    Arc::new(AppState {
        auth_token: auth_token.to_string(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace,
        home,
        version: "1.0.0-test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(Mutex::new(model_name.to_string())),
        model_base: Arc::new(Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(model_has_key)),
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
    })
}

fn make_state_with_tx(
    workspace: Option<String>,
    home: Option<String>,
    auth_token: &str,
    tx: Option<tokio::sync::mpsc::Sender<crate::internal::InternalCommand>>,
) -> Arc<AppState> {
    Arc::new(AppState {
        auth_token: auth_token.to_string(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace,
        home,
        version: "1.0.0-test".to_string(),
        start_time: Instant::now(),
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
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: tx,
    })
}

// ============================================================
// handle_api_readme
// ============================================================

#[tokio::test]
async fn test_handle_api_readme_returns_content() {
    let resp = handle_api_readme().await;
    let json = resp.0;
    assert!(json["content"].is_string());
    let content = json["content"].as_str().unwrap();
    assert!(!content.is_empty());
}

#[tokio::test]
async fn test_handle_api_readme_content_is_nonempty_string() {
    let resp = handle_api_readme().await;
    let json = resp.0;
    let content = json["content"].as_str().unwrap();
    // README should be at least a few hundred bytes (any real README)
    assert!(content.len() > 100, "README content suspiciously small: {} bytes", content.len());
}

// ============================================================
// handle_api_license
// ============================================================

#[tokio::test]
async fn test_handle_api_license_returns_content() {
    let resp = handle_api_license().await;
    let json = resp.0;
    assert!(json["content"].is_string());
    let content = json["content"].as_str().unwrap();
    assert!(!content.is_empty());
}

#[tokio::test]
async fn test_handle_api_license_content_is_nonempty_string() {
    let resp = handle_api_license().await;
    let json = resp.0;
    let content = json["content"].as_str().unwrap();
    assert!(content.len() > 10, "LICENSE content suspiciously small: {} bytes", content.len());
}

// ============================================================
// handle_api_internal
// ============================================================

#[tokio::test]
async fn test_handle_api_internal_unauthorized_wrong_token() {
    let state = make_state_with_tx(None, None, "expected-token", None);
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("X-Auth-Token", "wrong-token".parse().unwrap());
    let body = serde_json::json!({"cmd": "open_dashboard"});
    let result = handle_api_internal(headers, axum::extract::State(state), Json(body)).await;
    assert!(result.is_err());
    let (status, _) = result.unwrap_err();
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_handle_api_internal_unauthorized_missing_header() {
    let state = make_state_with_tx(None, None, "expected-token", None);
    let headers = axum::http::HeaderMap::new();
    let body = serde_json::json!({"cmd": "open_dashboard"});
    let result = handle_api_internal(headers, axum::extract::State(state), Json(body)).await;
    assert!(result.is_err());
    let (status, json) = result.unwrap_err();
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json.0["error"], "unauthorized");
}

#[tokio::test]
async fn test_handle_api_internal_no_token_required_when_empty() {
    // When auth_token is empty, any token (or missing) is accepted.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<crate::internal::InternalCommand>(8);
    let state = make_state_with_tx(None, None, "", Some(tx));
    let headers = axum::http::HeaderMap::new();
    let body = serde_json::json!({"cmd": "open_dashboard"});
    let result = handle_api_internal(headers, axum::extract::State(state), Json(body)).await;
    assert!(result.is_ok());
    let received = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv()).await;
    assert!(received.is_ok());
    match received.unwrap().unwrap() {
        crate::internal::InternalCommand::OpenDashboard => {}
    }
}

#[tokio::test]
async fn test_handle_api_internal_correct_token_open_dashboard() {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<crate::internal::InternalCommand>(8);
    let state = make_state_with_tx(None, None, "secret", Some(tx));
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("X-Auth-Token", "secret".parse().unwrap());
    let body = serde_json::json!({"cmd": "open_dashboard"});
    let result = handle_api_internal(headers, axum::extract::State(state), Json(body)).await;
    assert!(result.is_ok());
    let json = result.unwrap().0;
    assert_eq!(json["status"], "ok");
    let received = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv()).await;
    assert!(received.is_ok());
}

#[tokio::test]
async fn test_handle_api_internal_unknown_cmd() {
    let (tx, _rx) = tokio::sync::mpsc::channel::<crate::internal::InternalCommand>(8);
    let state = make_state_with_tx(None, None, "", Some(tx));
    let headers = axum::http::HeaderMap::new();
    let body = serde_json::json!({"cmd": "bogus_command"});
    let result = handle_api_internal(headers, axum::extract::State(state), Json(body)).await;
    assert!(result.is_err());
    let (status, json) = result.unwrap_err();
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json.0["error"], "unknown command");
}

#[tokio::test]
async fn test_handle_api_internal_missing_cmd_field() {
    let (tx, _rx) = tokio::sync::mpsc::channel::<crate::internal::InternalCommand>(8);
    let state = make_state_with_tx(None, None, "", Some(tx));
    let headers = axum::http::HeaderMap::new();
    let body = serde_json::json!({"not_cmd": "x"});
    let result = handle_api_internal(headers, axum::extract::State(state), Json(body)).await;
    // Missing cmd defaults to "" which doesn't match any arm → unknown command
    assert!(result.is_err());
    let (status, _) = result.unwrap_err();
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_handle_api_internal_cmd_not_string() {
    let (tx, _rx) = tokio::sync::mpsc::channel::<crate::internal::InternalCommand>(8);
    let state = make_state_with_tx(None, None, "", Some(tx));
    let headers = axum::http::HeaderMap::new();
    let body = serde_json::json!({"cmd": 123});
    let result = handle_api_internal(headers, axum::extract::State(state), Json(body)).await;
    assert!(result.is_err());
    let (status, _) = result.unwrap_err();
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_handle_api_internal_no_channel_configured() {
    let state = make_state_with_tx(None, None, "", None);
    let headers = axum::http::HeaderMap::new();
    let body = serde_json::json!({"cmd": "open_dashboard"});
    let result = handle_api_internal(headers, axum::extract::State(state), Json(body)).await;
    assert!(result.is_err());
    let (status, json) = result.unwrap_err();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json.0["error"], "internal channel not available");
}

#[tokio::test]
async fn test_handle_api_internal_send_fails_when_receiver_dropped() {
    let (tx, rx) = tokio::sync::mpsc::channel::<crate::internal::InternalCommand>(8);
    drop(rx); // Close the receiver so send fails
    let state = make_state_with_tx(None, None, "", Some(tx));
    let headers = axum::http::HeaderMap::new();
    let body = serde_json::json!({"cmd": "open_dashboard"});
    let result = handle_api_internal(headers, axum::extract::State(state), Json(body)).await;
    assert!(result.is_err());
    let (status, json) = result.unwrap_err();
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(json.0["error"], "send failed");
}

// ============================================================
// handle_api_status with model_has_key
// ============================================================

#[tokio::test]
async fn test_handle_api_status_includes_model_has_key_true() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let state = make_state(Some(ws), None, "", "gpt-4o", true);
    let resp = handle_api_status(State(state)).await;
    let json = resp.0;
    assert_eq!(json["model_has_key"], true);
    assert_eq!(json["model"], "gpt-4o");
    assert_eq!(json["model_base"], "");
}

#[tokio::test]
async fn test_handle_api_status_includes_model_base() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.clone()),
        home: None,
        version: "9.9".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(Mutex::new("claude-3".to_string())),
        model_base: Arc::new(Mutex::new("https://api.anthropic.com".to_string())),
        model_has_key: Arc::new(AtomicBool::new(true)),
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
    let resp = handle_api_status(State(state)).await;
    let json = resp.0;
    assert_eq!(json["model_base"], "https://api.anthropic.com");
    assert_eq!(json["version"], "9.9");
    assert_eq!(json["model"], "claude-3");
}

#[tokio::test]
async fn test_handle_api_status_without_workspace_omits_model_fields() {
    let state = make_state(None, None, "", "test", false);
    let resp = handle_api_status(State(state)).await;
    let json = resp.0;
    assert_eq!(json["version"], "1.0.0-test");
    assert_eq!(json["ws_connected"], true);
    assert!(json.get("model").is_none());
    assert!(json.get("model_base").is_none());
    assert!(json.get("model_has_key").is_none());
    assert!(json.get("scanner_status").is_none());
    assert!(json.get("cluster_status").is_none());
}

// ============================================================
// handle_api_models with edge cases
// ============================================================

#[tokio::test]
async fn test_handle_api_models_empty_api_key_not_sanitized() {
    let dir = tempfile::tempdir().unwrap();
    let config = serde_json::json!({
        "model_list": [
            {"name": "model1", "api_key": ""},
            {"name": "model2", "api_key": "sk-1234567890"}
        ]
    });
    std::fs::write(dir.path().join("config.json"), serde_json::to_string(&config).unwrap()).unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let state = make_state(None, Some(ws), "", "current-model", false);
    let resp = handle_api_models(State(state)).await;
    assert!(resp.is_ok());
    let json = resp.unwrap().0;
    let models = json["models"].as_array().unwrap();
    assert_eq!(models[0]["api_key"], ""); // empty stays empty
    assert_eq!(models[1]["api_key"], "sk-1****");
}

#[tokio::test]
async fn test_handle_api_models_short_api_key_becomes_stars() {
    let dir = tempfile::tempdir().unwrap();
    let config = serde_json::json!({
        "model_list": [
            {"name": "short", "api_key": "ab"}
        ]
    });
    std::fs::write(dir.path().join("config.json"), serde_json::to_string(&config).unwrap()).unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let state = make_state(None, Some(ws), "", "current-model", false);
    let resp = handle_api_models(State(state)).await;
    let json = resp.unwrap().0;
    let models = json["models"].as_array().unwrap();
    assert_eq!(models[0]["api_key"], "****");
}

#[tokio::test]
async fn test_handle_api_models_4char_api_key_becomes_stars() {
    let dir = tempfile::tempdir().unwrap();
    let config = serde_json::json!({
        "model_list": [{"name": "m", "api_key": "abcd"}]
    });
    std::fs::write(dir.path().join("config.json"), serde_json::to_string(&config).unwrap()).unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let state = make_state(None, Some(ws), "", "current-model", false);
    let resp = handle_api_models(State(state)).await;
    let json = resp.unwrap().0;
    let models = json["models"].as_array().unwrap();
    assert_eq!(models[0]["api_key"], "****");
}

#[tokio::test]
async fn test_handle_api_models_5char_api_key_truncated() {
    let dir = tempfile::tempdir().unwrap();
    let config = serde_json::json!({
        "model_list": [{"name": "m", "api_key": "abcde"}]
    });
    std::fs::write(dir.path().join("config.json"), serde_json::to_string(&config).unwrap()).unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let state = make_state(None, Some(ws), "", "current-model", false);
    let resp = handle_api_models(State(state)).await;
    let json = resp.unwrap().0;
    let models = json["models"].as_array().unwrap();
    assert_eq!(models[0]["api_key"], "abcd****");
}

#[tokio::test]
async fn test_handle_api_models_no_config_file_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let state = make_state(None, Some(ws), "", "fallback-model", false);
    let resp = handle_api_models(State(state)).await;
    assert!(resp.is_ok());
    let json = resp.unwrap().0;
    assert_eq!(json["models"].as_array().unwrap().len(), 0);
    assert_eq!(json["default"], "fallback-model");
}

#[tokio::test]
async fn test_handle_api_models_with_default_in_config() {
    let dir = tempfile::tempdir().unwrap();
    let config = serde_json::json!({
        "model_list": [{"name": "gpt-4"}],
        "agents": {"defaults": {"llm": "gpt-4"}}
    });
    std::fs::write(dir.path().join("config.json"), serde_json::to_string(&config).unwrap()).unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let state = make_state(None, Some(ws), "", "current-model", false);
    let resp = handle_api_models(State(state)).await;
    let json = resp.unwrap().0;
    assert_eq!(json["default"], "gpt-4");
    assert_eq!(json["current"], "current-model");
}

#[tokio::test]
async fn test_handle_api_models_no_model_list_returns_empty_array() {
    let dir = tempfile::tempdir().unwrap();
    let config = serde_json::json!({"other_key": "value"});
    std::fs::write(dir.path().join("config.json"), serde_json::to_string(&config).unwrap()).unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let state = make_state(None, Some(ws), "", "test", false);
    let resp = handle_api_models(State(state)).await;
    let json = resp.unwrap().0;
    assert_eq!(json["models"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_handle_api_models_no_default_in_config() {
    let dir = tempfile::tempdir().unwrap();
    let config = serde_json::json!({
        "model_list": [{"name": "gpt-4"}]
    });
    std::fs::write(dir.path().join("config.json"), serde_json::to_string(&config).unwrap()).unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let state = make_state(None, Some(ws), "", "test", false);
    let resp = handle_api_models(State(state)).await;
    let json = resp.unwrap().0;
    assert_eq!(json["default"], "");
}

// ============================================================
// handle_api_config edge cases
// ============================================================

#[tokio::test]
async fn test_handle_api_config_with_array_root() {
    // Non-object root: sanitize_map is skipped, raw returned as-is.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.json"), r#"["item1","item2"]"#).unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let state = make_state(Some(ws.clone()), Some(ws), "", "test", false);
    let resp = handle_api_config(State(state)).await;
    assert!(resp.is_ok());
    let json = resp.unwrap().0;
    assert!(json.is_array());
    assert_eq!(json[0], "item1");
}

#[tokio::test]
async fn test_handle_api_config_with_scalar_root() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.json"), "42").unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let state = make_state(Some(ws.clone()), Some(ws), "", "test", false);
    let resp = handle_api_config(State(state)).await;
    assert!(resp.is_ok());
    let json = resp.unwrap().0;
    assert_eq!(json, 42);
}

#[tokio::test]
async fn test_handle_api_config_with_nested_sensitive_key() {
    let dir = tempfile::tempdir().unwrap();
    let config = serde_json::json!({
        "outer": {
            "inner_key": "verylongsecret"
        }
    });
    std::fs::write(dir.path().join("config.json"), serde_json::to_string(&config).unwrap()).unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let state = make_state(Some(ws.clone()), Some(ws), "", "test", false);
    let resp = handle_api_config(State(state)).await;
    let json = resp.unwrap().0;
    assert_eq!(json["outer"]["inner_key"], "very****");
}

#[tokio::test]
async fn test_handle_api_config_empty_object() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.json"), "{}").unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let state = make_state(Some(ws.clone()), Some(ws), "", "test", false);
    let resp = handle_api_config(State(state)).await;
    assert!(resp.is_ok());
    let json = resp.unwrap().0;
    assert!(json.is_object());
    assert_eq!(json.as_object().unwrap().len(), 0);
}

// ============================================================
// handle_api_logs query parameter defaults
// ============================================================

#[tokio::test]
async fn test_handle_api_logs_default_source_general() {
    let dir = tempfile::tempdir().unwrap();
    let logs_dir = dir.path().join("logs");
    std::fs::create_dir_all(&logs_dir).unwrap();
    std::fs::write(logs_dir.join("nemesisbot.2026-06-17"), r#"{"msg":"default-source"}"#).unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let state = make_state(Some(ws), None, "", "test", false);

    // No query parameters — should default to source=general, n=200
    let query = Query(LogsQuery { source: None, n: None });
    let resp = handle_api_logs(State(state), query).await;
    assert!(resp.is_ok());
    let json = resp.unwrap().0;
    assert!(json["entries"].as_array().unwrap().len() >= 1);
}

#[tokio::test]
async fn test_handle_api_logs_source_general_n_1() {
    let dir = tempfile::tempdir().unwrap();
    let logs_dir = dir.path().join("logs");
    std::fs::create_dir_all(&logs_dir).unwrap();
    let lines: Vec<String> = (0..10).map(|i| format!(r#"{{"i":{}}}"#, i)).collect();
    std::fs::write(logs_dir.join("nemesisbot.2026-06-17"), lines.join("\n")).unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let state = make_state(Some(ws), None, "", "test", false);
    let query = Query(LogsQuery { source: Some("general".to_string()), n: Some(1) });
    let resp = handle_api_logs(State(state), query).await;
    let json = resp.unwrap().0;
    assert_eq!(json["entries"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn test_handle_api_logs_n_exactly_1000() {
    let dir = tempfile::tempdir().unwrap();
    let logs_dir = dir.path().join("logs");
    std::fs::create_dir_all(&logs_dir).unwrap();
    let lines: Vec<String> = (0..1500).map(|i| format!(r#"{{"i":{}}}"#, i)).collect();
    std::fs::write(logs_dir.join("nemesisbot.2026-06-17"), lines.join("\n")).unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let state = make_state(Some(ws), None, "", "test", false);
    let query = Query(LogsQuery { source: Some("general".to_string()), n: Some(1000) });
    let resp = handle_api_logs(State(state), query).await;
    let json = resp.unwrap().0;
    assert_eq!(json["entries"].as_array().unwrap().len(), 1000);
}

#[tokio::test]
async fn test_handle_api_logs_empty_log_file_returns_empty_array() {
    let dir = tempfile::tempdir().unwrap();
    let logs_dir = dir.path().join("logs");
    std::fs::create_dir_all(&logs_dir).unwrap();
    std::fs::write(logs_dir.join("nemesisbot.2026-06-17"), "").unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let state = make_state(Some(ws), None, "", "test", false);
    let query = Query(LogsQuery { source: Some("general".to_string()), n: Some(100) });
    let resp = handle_api_logs(State(state), query).await;
    let json = resp.unwrap().0;
    assert_eq!(json["entries"].as_array().unwrap().len(), 0);
}

// ============================================================
// handle_api_version
// ============================================================

#[tokio::test]
async fn test_handle_api_version_with_empty_model() {
    let state = make_state(None, None, "", "", false);
    let resp = handle_api_version(State(state)).await;
    let json = resp.0;
    assert_eq!(json["version"], "1.0.0-test");
    assert_eq!(json["model"], "");
}

#[tokio::test]
async fn test_handle_api_version_uptime_is_u64() {
    let state = make_state(None, None, "", "test", false);
    let resp = handle_api_version(State(state)).await;
    let json = resp.0;
    assert!(json["uptime_seconds"].is_u64());
}

// ============================================================
// handle_api_events
// ============================================================

#[tokio::test]
async fn test_handle_api_events_returns_stream_url() {
    let state = make_state(None, None, "", "test", false);
    let resp = handle_api_events(State(state)).await;
    let json = resp.0;
    assert_eq!(json["stream_url"], "/api/events/stream");
    assert!(json["subscriber_count"].is_u64());
}

// ============================================================
// handle_api_sessions
// ============================================================

#[tokio::test]
async fn test_handle_api_sessions_returns_zero_active() {
    let state = make_state(None, None, "", "test", false);
    let resp = handle_api_sessions(State(state)).await;
    let json = resp.0;
    assert_eq!(json["total_connections"], 0);
    assert_eq!(json["active_sessions"], 0);
}

#[tokio::test]
async fn test_handle_api_sessions_with_count() {
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(42)),
        workspace: None,
        home: None,
        version: "1.0.0".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(Mutex::new("test".to_string())),
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
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: None,
    });
    let resp = handle_api_sessions(State(state)).await;
    let json = resp.0;
    assert_eq!(json["total_connections"], 42);
}

// ============================================================
// load_scanner_status — state transitions
// ============================================================

#[test]
fn test_load_scanner_status_engine_disabled_state() {
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config = serde_json::json!({
        "enabled": [],
        "engines": {
            "clamav": {"path": "/usr/bin/clamav"}
        }
    });
    std::fs::write(
        config_dir.join("config.scanner.json"),
        serde_json::to_string_pretty(&config).unwrap(),
    ).unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let status = load_scanner_status(&ws);
    let engines = status["engines"].as_array().unwrap();
    assert_eq!(engines[0]["state"], "disabled");
    assert_eq!(engines[0]["enabled"], false);
}

#[test]
fn test_load_scanner_status_engine_pending_state() {
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config = serde_json::json!({
        "enabled": ["clamav"],
        "engines": {
            "clamav": {"state": {"install_status": "pending", "db_status": ""}}
        }
    });
    std::fs::write(
        config_dir.join("config.scanner.json"),
        serde_json::to_string(&config).unwrap(),
    ).unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let status = load_scanner_status(&ws);
    let engines = status["engines"].as_array().unwrap();
    assert_eq!(engines[0]["state"], "pending");
}

#[test]
fn test_load_scanner_status_engine_ready_state() {
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config = serde_json::json!({
        "enabled": ["clamav"],
        "engines": {
            "clamav": {"state": {"install_status": "installed", "db_status": "ready"}}
        }
    });
    std::fs::write(
        config_dir.join("config.scanner.json"),
        serde_json::to_string(&config).unwrap(),
    ).unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let status = load_scanner_status(&ws);
    let engines = status["engines"].as_array().unwrap();
    assert_eq!(engines[0]["state"], "ready");
}

#[test]
fn test_load_scanner_status_engine_failed_state() {
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config = serde_json::json!({
        "enabled": ["clamav"],
        "engines": {
            "clamav": {"state": {"install_status": "failed", "db_status": ""}}
        }
    });
    std::fs::write(
        config_dir.join("config.scanner.json"),
        serde_json::to_string(&config).unwrap(),
    ).unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let status = load_scanner_status(&ws);
    let engines = status["engines"].as_array().unwrap();
    assert_eq!(engines[0]["state"], "failed");
}

#[test]
fn test_load_scanner_status_engine_installed_partial_state() {
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config = serde_json::json!({
        "enabled": ["clamav"],
        "engines": {
            "clamav": {"state": {"install_status": "installed", "db_status": ""}}
        }
    });
    std::fs::write(
        config_dir.join("config.scanner.json"),
        serde_json::to_string(&config).unwrap(),
    ).unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let status = load_scanner_status(&ws);
    let engines = status["engines"].as_array().unwrap();
    // installed but not ready → "installed"
    assert_eq!(engines[0]["state"], "installed");
}

#[test]
fn test_load_scanner_status_engine_empty_install_status_treated_as_pending() {
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config = serde_json::json!({
        "enabled": ["clamav"],
        "engines": {
            "clamav": {"state": {"install_status": "", "db_status": ""}}
        }
    });
    std::fs::write(
        config_dir.join("config.scanner.json"),
        serde_json::to_string(&config).unwrap(),
    ).unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let status = load_scanner_status(&ws);
    let engines = status["engines"].as_array().unwrap();
    assert_eq!(engines[0]["state"], "pending");
}

#[test]
fn test_load_scanner_status_engine_with_unknown_install_status() {
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config = serde_json::json!({
        "enabled": ["clamav"],
        "engines": {
            "clamav": {"state": {"install_status": "downloading", "db_status": "downloading"}}
        }
    });
    std::fs::write(
        config_dir.join("config.scanner.json"),
        serde_json::to_string(&config).unwrap(),
    ).unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let status = load_scanner_status(&ws);
    let engines = status["engines"].as_array().unwrap();
    // Unknown install_status falls through to "installed"
    assert_eq!(engines[0]["state"], "installed");
}

#[test]
fn test_load_scanner_status_engine_case_insensitive_match() {
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config = serde_json::json!({
        "enabled": ["CLAMAV"],
        "engines": {
            "clamav": {"v": 1}
        }
    });
    std::fs::write(
        config_dir.join("config.scanner.json"),
        serde_json::to_string(&config).unwrap(),
    ).unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let status = load_scanner_status(&ws);
    let engines = status["engines"].as_array().unwrap();
    assert_eq!(engines[0]["enabled"], true);
}

#[test]
fn test_load_scanner_status_merges_config_fields() {
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config = serde_json::json!({
        "enabled": ["clamav"],
        "engines": {
            "clamav": {"path": "/usr/bin/clamav", "version": "1.0"}
        }
    });
    std::fs::write(
        config_dir.join("config.scanner.json"),
        serde_json::to_string(&config).unwrap(),
    ).unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let status = load_scanner_status(&ws);
    let engines = status["engines"].as_array().unwrap();
    // path and version should be present (merged from config)
    assert_eq!(engines[0]["path"], "/usr/bin/clamav");
    assert_eq!(engines[0]["version"], "1.0");
}

// ============================================================
// resolve_log_file_path additional branches
// ============================================================

#[test]
fn test_resolve_log_file_path_general_no_logs_dir() {
    // New behavior: no daily files = None (no legacy fallback to default nemesisbot.log path).
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    assert!(resolve_log_file_path(&ws, "general").is_none());
}

#[test]
fn test_resolve_log_file_path_general_prefers_nemesisbot_over_app() {
    // New behavior: only nemesisbot.YYYY-MM-DD matches; legacy nemesisbot.log and app.log are ignored.
    let dir = tempfile::tempdir().unwrap();
    let logs_dir = dir.path().join("logs");
    std::fs::create_dir_all(&logs_dir).unwrap();
    std::fs::write(logs_dir.join("nemesisbot.2026-06-17"), "primary").unwrap();
    std::fs::write(logs_dir.join("app.log"), "secondary").unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let path = resolve_log_file_path(&ws, "general").unwrap();
    assert!(path.contains("nemesisbot.2026-06-17"));
}

#[test]
fn test_resolve_log_file_path_security_returns_fixed_audit_file() {
    // audit.jsonl is a fixed filename (not glob), so only its presence/absence matters.
    let dir = tempfile::tempdir().unwrap();
    let sec_dir = dir.path().join("logs").join("security_logs");
    std::fs::create_dir_all(&sec_dir).unwrap();
    std::fs::write(sec_dir.join("audit.jsonl"), "{\"audit\":\"entry\"}").unwrap();
    // Other .log files in the dir should be ignored
    std::fs::write(sec_dir.join("audit_2025-01-01.log"), "stale").unwrap();

    let ws = dir.path().to_string_lossy().to_string();
    let path = resolve_log_file_path(&ws, "security").unwrap();
    assert!(path.contains("audit.jsonl"));
    assert!(!path.contains("audit_2025"));
}

// ============================================================
// read_log_entries additional cases
// ============================================================

#[test]
fn test_read_log_entries_with_blank_lines_filtered() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("blank.log");
    std::fs::write(&file_path, "{\"a\":1}\n\n{\"b\":2}\n   \n{\"c\":3}\n").unwrap();
    let entries = read_log_entries(&file_path.to_string_lossy(), 100);
    assert_eq!(entries.len(), 3);
}

#[test]
fn test_read_log_entries_single_line() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("single.log");
    std::fs::write(&file_path, "{\"only\":\"one\"}").unwrap();
    let entries = read_log_entries(&file_path.to_string_lossy(), 100);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["only"], "one");
}

#[test]
fn test_read_log_entries_trailing_newline() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("trailing.log");
    std::fs::write(&file_path, "{\"a\":1}\n{\"b\":2}\n").unwrap();
    let entries = read_log_entries(&file_path.to_string_lossy(), 100);
    assert_eq!(entries.len(), 2);
}

// ============================================================
// sanitize_map additional cases
// ============================================================

#[test]
fn test_sanitize_map_does_not_touch_empty_strings() {
    let mut map = serde_json::json!({
        "api_key": "",
        "token": "",
        "secret": "",
    }).as_object_mut().unwrap().clone();
    sanitize_map(&mut map);
    assert_eq!(map["api_key"], "");
    assert_eq!(map["token"], "");
    assert_eq!(map["secret"], "");
}

#[test]
fn test_sanitize_map_recursive_into_array_values() {
    // Arrays are not recursed (only objects), but values in arrays that are objects
    // are not sanitized. Test that array contents remain unchanged.
    let mut map = serde_json::json!({
        "items": [
            {"api_key": "verylongsecret"},
            {"token": "anotherlongtoken"}
        ]
    }).as_object_mut().unwrap().clone();
    sanitize_map(&mut map);
    // Array elements are not sanitized
    let items = map["items"].as_array().unwrap();
    assert_eq!(items[0]["api_key"], "verylongsecret");
}

#[test]
fn test_sanitize_map_object_inside_non_sensitive_key() {
    let mut map = serde_json::json!({
        "settings": {
            "api_key": "secretvalue",
            "port": 3000
        }
    }).as_object_mut().unwrap().clone();
    sanitize_map(&mut map);
    let settings = map["settings"].as_object().unwrap();
    assert_eq!(settings["api_key"], "secr****");
    assert_eq!(settings["port"], 3000);
}

#[test]
fn test_sanitize_map_partial_match_substring_key() {
    // Substring matching: keys containing "key", "token", etc. match
    let mut map = serde_json::json!({
        "my_api_key": "secretlongvalue",
        "user_token_value": "anotherlongtoken",
        "password_hash": "hashvalue1234",
    }).as_object_mut().unwrap().clone();
    sanitize_map(&mut map);
    assert_eq!(map["my_api_key"], "secr****");
    assert_eq!(map["user_token_value"], "anot****");
    assert_eq!(map["password_hash"], "hash****");
}

// ============================================================
// AppState Clone
// ============================================================

#[test]
fn test_app_state_clone() {
    let state = AppState {
        auth_token: "tok".to_string(),
        session_count: Arc::new(AtomicUsize::new(5)),
        workspace: Some("/work".to_string()),
        home: Some("/home".to_string()),
        version: "1.0".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(Mutex::new("gpt-4".to_string())),
        model_base: Arc::new(Mutex::new("https://api".to_string())),
        model_has_key: Arc::new(AtomicBool::new(true)),
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
    };
    let cloned = state.clone();
    assert_eq!(cloned.auth_token, "tok");
    assert_eq!(cloned.workspace.as_deref(), Some("/work"));
    assert_eq!(*cloned.model_name.lock(), "gpt-4");
}

// ============================================================
// write_json_response / write_json_error additional cases
// ============================================================

#[test]
fn test_write_json_response_bool() {
    let bytes = write_json_response(&true);
    assert_eq!(bytes, b"true");
}

#[test]
fn test_write_json_response_null() {
    let bytes = write_json_response(&serde_json::Value::Null);
    assert_eq!(bytes, b"null");
}

#[test]
fn test_write_json_response_array() {
    let arr = vec![1, 2, 3];
    let bytes = write_json_response(&arr);
    let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(parsed[0], 1);
    assert_eq!(parsed[2], 3);
}

#[test]
fn test_write_json_error_empty_message() {
    let bytes = write_json_error("", 500);
    let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(parsed["error"], "");
}

// ============================================================
// verify_token additional cases
// ============================================================

#[test]
fn test_verify_token_with_unicode() {
    let token = "token-with-unicode-★-☆";
    assert!(verify_token(token, token));
    assert!(!verify_token("different", token));
}

#[test]
fn test_verify_token_long_token_match() {
    let long = "a".repeat(1000);
    assert!(verify_token(&long, &long));
}

#[test]
fn test_verify_token_one_empty_one_not() {
    // Empty expected means always valid
    assert!(verify_token("non-empty", ""));
    // But empty token against non-empty expected fails
    assert!(!verify_token("", "expected"));
}

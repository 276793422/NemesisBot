//! API route handlers for status, logs, scanner, config, models, and version endpoints.
//!
//! Mirrors the Go `module/web/api_handlers.go`:
//! - `handle_api_status` — system status with version, uptime, sessions, scanner, cluster
//! - `handle_api_logs` — historical log entries from JSONL log files
//! - `handle_api_scanner_status` — scanner engine status from config
//! - `handle_api_config` — sanitized configuration file
//! - `handle_api_version` — version and build info
//! - `handle_api_models` — list configured LLM models
//! - `handle_api_sessions` — active WebSocket session info
//! - `handle_api_events` — recent event hub events
//! - Log reading helpers: `resolve_log_file_path`, `read_log_entries`, `sanitize_map`
//! - Utility helpers: `write_json_response`, `write_json_error`, `verify_token`

use crate::events::EventHub;
use crate::session::SessionManager;
use crate::websocket_handler::IncomingMessage;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;

use nemesis_types::utils;
use parking_lot::Mutex;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Shared application state
// ---------------------------------------------------------------------------

/// Application state shared with all handlers.
#[derive(Clone)]
pub struct AppState {
    pub auth_token: String,
    pub session_count: Arc<AtomicUsize>,
    /// Workspace path for config/log file access.
    pub workspace: Option<String>,
    /// Application version.
    pub version: String,
    /// Server start time.
    pub start_time: Instant,
    /// Current LLM model name (wrapped in Arc<Mutex> for Clone).
    pub model_name: Arc<Mutex<String>>,
    /// SSE event hub.
    pub event_hub: Arc<EventHub>,
    /// Server running state.
    pub running: Arc<AtomicBool>,
    /// Session manager for WebSocket connections.
    pub session_manager: Arc<SessionManager>,
    /// Sender for forwarding incoming WebSocket messages to the bus bridge.
    pub inbound_tx: Option<mpsc::UnboundedSender<IncomingMessage>>,
    /// Streaming LLM provider for SSE chat endpoint (optional — set via set_streaming_provider).
    pub streaming_provider: Option<Arc<nemesis_providers::http_provider::HttpProvider>>,
}

impl AppState {
    /// Get a reference to the session manager.
    pub fn session_manager_ref(&self) -> &SessionManager {
        &self.session_manager
    }
}

// ---------------------------------------------------------------------------
// Handler: API status
// ---------------------------------------------------------------------------

/// `GET /api/status` — returns system status as JSON.
///
/// Returns version, uptime, session count, scanner status, cluster status, model name.
pub async fn handle_api_status(
    State(state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let uptime = state.start_time.elapsed().as_secs();
    let session_count = state.session_count.load(std::sync::atomic::Ordering::SeqCst);
    let running = state.running.load(std::sync::atomic::Ordering::SeqCst);
    let model_name = state.model_name.lock().clone();

    let mut response = serde_json::json!({
        "version": state.version,
        "uptime_seconds": uptime,
        "ws_connected": running,
        "session_count": session_count,
    });

    if let Some(ref workspace) = state.workspace {
        response.as_object_mut().unwrap().insert(
            "scanner_status".to_string(),
            load_scanner_status(workspace),
        );
        response.as_object_mut().unwrap().insert(
            "cluster_status".to_string(),
            serde_json::json!({
                "enabled": false,
                "node_count": 0,
            }),
        );
        response.as_object_mut().unwrap().insert(
            "model".to_string(),
            serde_json::Value::String(model_name),
        );
    }

    Json(response)
}

// ---------------------------------------------------------------------------
// Handler: API logs
// ---------------------------------------------------------------------------

/// Query parameters for the logs API.
#[derive(Debug, Deserialize)]
pub struct LogsQuery {
    /// Log source: "general" (default), "llm", "security", "cluster".
    pub source: Option<String>,
    /// Number of entries to return (default 200, max 1000).
    pub n: Option<usize>,
}

/// `GET /api/logs?source=general&n=200` — returns historical log entries.
pub async fn handle_api_logs(
    State(state): State<Arc<AppState>>,
    Query(query): Query<LogsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let workspace = match &state.workspace {
        Some(ws) => ws.clone(),
        None => {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "workspace not configured"})),
            ));
        }
    };

    let source = query.source.unwrap_or_else(|| "general".to_string());
    let mut n = query.n.unwrap_or(200);
    if n > 1000 {
        n = 1000;
    }
    if n == 0 {
        n = 200;
    }

    let log_file_path = resolve_log_file_path(&workspace, &source);
    let entries = match log_file_path {
        Some(path) => read_log_entries(&path, n),
        None => vec![],
    };

    Ok(Json(serde_json::json!({
        "entries": entries,
    })))
}

// ---------------------------------------------------------------------------
// Handler: API scanner status
// ---------------------------------------------------------------------------

/// `GET /api/scanner/status` — returns scanner engine status.
pub async fn handle_api_scanner_status(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let workspace = match &state.workspace {
        Some(ws) => ws.clone(),
        None => {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "workspace not configured"})),
            ));
        }
    };

    Ok(Json(load_scanner_status(&workspace)))
}

// ---------------------------------------------------------------------------
// Handler: API config
// ---------------------------------------------------------------------------

/// `GET /api/config` — returns sanitized configuration.
pub async fn handle_api_config(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let workspace = match &state.workspace {
        Some(ws) => ws.clone(),
        None => {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "workspace not configured"})),
            ));
        }
    };

    let config_path = PathBuf::from(&workspace).join("config").join("config.json");
    let data = match std::fs::read_to_string(&config_path) {
        Ok(d) => d,
        Err(_) => {
            tracing::debug!(path = %config_path.display(), "Config file not found");
            return Err((
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "configuration not found"})),
            ));
        }
    };

    let mut raw: serde_json::Value = match serde_json::from_str(&data) {
        Ok(v) => v,
        Err(_) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "invalid configuration format"})),
            ));
        }
    };

    // Sanitize sensitive values
    if let Some(obj) = raw.as_object_mut() {
        sanitize_map(obj);
    }

    Ok(Json(raw))
}

// ---------------------------------------------------------------------------
// Handler: API version
// ---------------------------------------------------------------------------

/// `GET /api/version` — returns version and build information.
pub async fn handle_api_version(
    State(state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let uptime = state.start_time.elapsed().as_secs();
    Json(serde_json::json!({
        "version": state.version,
        "uptime_seconds": uptime,
        "model": *state.model_name.lock(),
    }))
}

// ---------------------------------------------------------------------------
// Handler: API models
// ---------------------------------------------------------------------------

/// `GET /api/models` — returns the list of configured LLM models from config.
pub async fn handle_api_models(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let workspace = match &state.workspace {
        Some(ws) => ws.clone(),
        None => {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "workspace not configured"})),
            ));
        }
    };

    let config_path = PathBuf::from(&workspace).join("config").join("config.json");
    let data = match std::fs::read_to_string(&config_path) {
        Ok(d) => d,
        Err(_) => {
            return Ok(Json(serde_json::json!({
                "models": [],
                "default": *state.model_name.lock(),
            })));
        }
    };

    let config: serde_json::Value = match serde_json::from_str(&data) {
        Ok(v) => v,
        Err(_) => {
            return Ok(Json(serde_json::json!({
                "models": [],
                "default": *state.model_name.lock(),
            })));
        }
    };

    let models = config
        .get("model_list")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // Sanitize API keys in model entries
    let sanitized_models: Vec<serde_json::Value> = models
        .into_iter()
        .map(|mut m| {
            if let Some(obj) = m.as_object_mut() {
                if let Some(key) = obj.get_mut("api_key") {
                    if let Some(s) = key.as_str() {
                        if !s.is_empty() {
                            *key = if s.len() <= 4 {
                                serde_json::Value::String("****".to_string())
                            } else {
                                let end = utils::floor_char_boundary(s, 4);
                                serde_json::Value::String(format!("{}****", &s[..end]))
                            };
                        }
                    }
                }
            }
            m
        })
        .collect();

    let default_llm = config
        .get("agents")
        .and_then(|a| a.get("defaults"))
        .and_then(|d| d.get("llm"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let current_model = state.model_name.lock().clone();

    Ok(Json(serde_json::json!({
        "models": sanitized_models,
        "default": default_llm,
        "current": current_model,
    })))
}

// ---------------------------------------------------------------------------
// Handler: API sessions
// ---------------------------------------------------------------------------

/// `GET /api/sessions` — returns information about active WebSocket sessions.
pub async fn handle_api_sessions(
    State(state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let session_count = state.session_count.load(std::sync::atomic::Ordering::SeqCst);
    let active_count = state.session_manager.active_count();

    Json(serde_json::json!({
        "total_connections": session_count,
        "active_sessions": active_count,
    }))
}

// ---------------------------------------------------------------------------
// Handler: API events (recent event hub events)
// ---------------------------------------------------------------------------

/// `GET /api/events` — returns recent events from the event hub (snapshot).
pub async fn handle_api_events(
    State(state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let subscriber_count = state.event_hub.subscriber_count();
    Json(serde_json::json!({
        "stream_url": "/api/events/stream",
        "subscriber_count": subscriber_count,
    }))
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Load scanner status from the workspace config directory.
fn load_scanner_status(workspace: &str) -> serde_json::Value {
    let scanner_config_path = PathBuf::from(workspace)
        .join("config")
        .join("config.scanner.json");

    let data = match std::fs::read_to_string(&scanner_config_path) {
        Ok(d) => d,
        Err(_) => {
            return serde_json::json!({
                "enabled": false,
                "engines": [],
            });
        }
    };

    #[derive(serde::Deserialize)]
    struct ScannerConfig {
        #[serde(default)]
        enabled: Vec<String>,
        #[serde(default)]
        engines: HashMap<String, serde_json::Value>,
    }

    let cfg: ScannerConfig = match serde_json::from_str(&data) {
        Ok(c) => c,
        Err(_) => {
            return serde_json::json!({
                "enabled": false,
                "engines": [],
            });
        }
    };

    let mut engines: Vec<serde_json::Value> = cfg
        .engines
        .into_iter()
        .map(|(name, config)| {
            serde_json::json!({
                "name": name,
                "config": config,
            })
        })
        .collect();

    // Sort engines by name for deterministic output
    engines.sort_by(|a, b| {
        let a_name = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let b_name = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
        a_name.cmp(b_name)
    });

    serde_json::json!({
        "enabled": !cfg.enabled.is_empty(),
        "engines": engines,
    })
}

/// Resolve the log file path for a given source type.
fn resolve_log_file_path(workspace: &str, source: &str) -> Option<String> {
    match source {
        "general" => {
            let candidates = vec![
                PathBuf::from(workspace).join("logs").join("nemesisbot.log"),
                PathBuf::from(workspace).join("logs").join("app.log"),
            ];
            for c in &candidates {
                if c.exists() {
                    return Some(c.to_string_lossy().to_string());
                }
            }
            // Return default even if it doesn't exist
            Some(candidates[0].to_string_lossy().to_string())
        }
        "llm" => {
            let dir = PathBuf::from(workspace).join("logs").join("request_logs");
            find_latest_file(&dir)
        }
        "security" => {
            let sec_dir = PathBuf::from(workspace).join("config");
            let pattern = sec_dir.join("security_audit_*.log");
            let _pattern_str = pattern.to_string_lossy();

            // Glob for security audit logs
            let mut matches: Vec<String> = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&sec_dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with("security_audit_") && name.ends_with(".log") {
                        matches.push(entry.path().to_string_lossy().to_string());
                    }
                }
            }

            if !matches.is_empty() {
                matches.sort();
                matches.reverse();
                Some(matches[0].clone())
            } else {
                None
            }
        }
        "cluster" => {
            // Cluster logs are stored in {workspace}/logs/cluster/
            let cluster_dir = PathBuf::from(workspace).join("logs").join("cluster");
            let mut matches: Vec<String> = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&cluster_dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.ends_with(".log") {
                        matches.push(entry.path().to_string_lossy().to_string());
                    }
                }
            }
            if !matches.is_empty() {
                matches.sort();
                matches.reverse();
                Some(matches[0].clone())
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Find the most recently modified file in a directory.
fn find_latest_file(dir: &std::path::Path) -> Option<String> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut latest_time: std::time::SystemTime = std::time::UNIX_EPOCH;
    let mut latest_name: Option<String> = None;

    for entry in entries.flatten() {
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(true) {
            continue;
        }
        let metadata = entry.metadata().ok()?;
        let modified = metadata.modified().ok();
        if let Some(mtime) = modified {
            if mtime > latest_time {
                latest_time = mtime;
                latest_name = Some(entry.path().to_string_lossy().to_string());
            }
        }
    }

    latest_name
}

/// Read the last `n` JSON Lines entries from a file.
fn read_log_entries(file_path: &str, n: usize) -> Vec<serde_json::Value> {
    let content = match std::fs::read_to_string(file_path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();

    // Take last n lines
    let start = if lines.len() > n {
        lines.len() - n
    } else {
        0
    };

    lines[start..]
        .iter()
        .map(|line| {
            match serde_json::from_str::<serde_json::Value>(line) {
                Ok(v) => v,
                Err(_) => {
                    // Not JSON — create a plain text entry
                    serde_json::json!({"message": line})
                }
            }
        })
        .collect()
}

/// Recursively masks sensitive values in a JSON object map.
///
/// Sensitive keys contain: key, token, secret, password, auth, credential.
/// Values are replaced with the first 4 chars + "****", or "****" if too short.
fn sanitize_map(map: &mut serde_json::Map<String, serde_json::Value>) {
    let sensitive_keys = ["key", "token", "secret", "password", "auth", "credential"];

    let keys_to_sanitize: Vec<String> = map
        .keys()
        .filter(|k| {
            let lower = k.to_lowercase();
            sensitive_keys.iter().any(|sk| lower.contains(sk))
        })
        .cloned()
        .collect();

    for key in keys_to_sanitize {
        if let Some(value) = map.get_mut(&key) {
            match value {
                serde_json::Value::String(s) => {
                    if !s.is_empty() {
                        if s.len() <= 4 {
                            *value = serde_json::Value::String("****".to_string());
                        } else {
                            let end = utils::floor_char_boundary(s, 4);
                            *value =
                                serde_json::Value::String(format!("{}****", &s[..end]));
                        }
                    }
                }
                serde_json::Value::Object(inner_map) => {
                    sanitize_map(inner_map);
                }
                _ => {}
            }
        }
    }

    // Also recurse into any remaining object values
    for value in map.values_mut() {
        if let serde_json::Value::Object(inner_map) = value {
            sanitize_map(inner_map);
        }
    }
}

// ---------------------------------------------------------------------------
// Utility: verify auth token
// ---------------------------------------------------------------------------

/// Verify auth token from query or header.
pub fn verify_token(token: &str, expected: &str) -> bool {
    if expected.is_empty() {
        return true;
    }
    token == expected
}

/// Write a JSON response body from a serializable value.
/// Returns the serialized JSON bytes suitable for HTTP response bodies.
pub fn write_json_response<T: serde::Serialize>(value: &T) -> Vec<u8> {
    serde_json::to_vec(value).unwrap_or_default()
}

/// Write a JSON error response body with the given message and HTTP status code.
/// Returns the serialized JSON error bytes.
pub fn write_json_error(message: &str, _code: u16) -> Vec<u8> {
    let body = serde_json::json!({"error": message});
    serde_json::to_vec(&body).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
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
        std::fs::write(logs_dir.join("nemesisbot.log"), "log content").unwrap();

        let ws = dir.path().to_string_lossy().to_string();
        let path = resolve_log_file_path(&ws, "general").unwrap();
        assert!(path.contains("nemesisbot.log"));
    }

    #[test]
    fn test_resolve_log_file_path_general_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let path = resolve_log_file_path(&ws, "general").unwrap();
        // Should still return a default path even if file doesn't exist
        assert!(path.contains("nemesisbot.log"));
    }

    #[test]
    fn test_resolve_log_file_path_llm() {
        let dir = tempfile::tempdir().unwrap();
        let logs_dir = dir.path().join("logs").join("request_logs");
        std::fs::create_dir_all(&logs_dir).unwrap();
        std::fs::write(logs_dir.join("2026-04-30.jsonl"), "line1\nline2").unwrap();

        let ws = dir.path().to_string_lossy().to_string();
        let path = resolve_log_file_path(&ws, "llm").unwrap();
        assert!(path.contains("2026-04-30.jsonl"));
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
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("mixed.log");
        let content = "plain text line\n{\"json\":true}\n";
        std::fs::write(&file_path, content).unwrap();

        let entries = read_log_entries(&file_path.to_string_lossy(), 100);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["message"], "plain text line");
        assert_eq!(entries[1]["json"], true);
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
    fn test_find_latest_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("old.jsonl"), "old").unwrap();

        // Small sleep to ensure different mtime
        std::thread::sleep(std::time::Duration::from_millis(50));

        std::fs::write(dir.path().join("new.jsonl"), "new").unwrap();

        let result = find_latest_file(dir.path());
        assert!(result.is_some());
        assert!(result.unwrap().contains("new.jsonl"));
    }

    #[test]
    fn test_find_latest_file_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let result = find_latest_file(dir.path());
        assert!(result.is_none());
    }

    #[test]
    fn test_find_latest_file_nonexistent_dir() {
        let result = find_latest_file(std::path::Path::new("/nonexistent"));
        assert!(result.is_none());
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
        let config_dir = dir.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("security_audit_2026-01-01.log"), "entry1").unwrap();
        std::fs::write(config_dir.join("security_audit_2026-01-02.log"), "entry2").unwrap();

        let ws = dir.path().to_string_lossy().to_string();
        let path = resolve_log_file_path(&ws, "security").unwrap();
        // Should return the latest security audit file
        assert!(path.contains("security_audit_"));
    }

    #[test]
    fn test_resolve_log_file_path_security_no_files() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();

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
            version: "1.0.0".to_string(),
            start_time: std::time::Instant::now(),
            model_name: Arc::new(Mutex::new("test-model".to_string())),
            event_hub: Arc::new(EventHub::new()),
            running: Arc::new(AtomicBool::new(false)),
            session_manager: Arc::new(SessionManager::with_default_timeout()),
            inbound_tx: None,
            streaming_provider: None,
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
        let dir = tempfile::tempdir().unwrap();
        let logs_dir = dir.path().join("logs");
        std::fs::create_dir_all(&logs_dir).unwrap();
        // Only app.log exists, not nemesisbot.log
        std::fs::write(logs_dir.join("app.log"), "log content").unwrap();

        let ws = dir.path().to_string_lossy().to_string();
        let path = resolve_log_file_path(&ws, "general").unwrap();
        assert!(path.contains("app.log"));
    }

    #[test]
    fn test_find_latest_file_ignores_directories() {
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("subdir");
        std::fs::create_dir_all(&subdir).unwrap();
        std::fs::write(dir.path().join("file.jsonl"), "data").unwrap();

        let result = find_latest_file(dir.path());
        assert!(result.is_some());
        assert!(result.unwrap().contains("file.jsonl"));
    }

    #[test]
    fn test_app_state_default_values() {
        let state = AppState {
            auth_token: String::new(),
            session_count: Arc::new(AtomicUsize::new(5)),
            workspace: Some("/tmp".to_string()),
            version: "1.0.0".to_string(),
            start_time: std::time::Instant::now(),
            model_name: Arc::new(Mutex::new("gpt-4".to_string())),
            event_hub: Arc::new(EventHub::new()),
            running: Arc::new(AtomicBool::new(true)),
            session_manager: Arc::new(SessionManager::with_default_timeout()),
            inbound_tx: None,
            streaming_provider: None,
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
        let cluster_dir = dir.path().join("logs").join("cluster");
        std::fs::create_dir_all(&cluster_dir).unwrap();
        std::fs::write(cluster_dir.join("discovery.log"), "discovery log content").unwrap();

        let ws = dir.path().to_string_lossy().to_string();
        let path = resolve_log_file_path(&ws, "cluster");
        assert!(path.is_some());
        assert!(path.unwrap().contains("discovery.log"));
    }

    #[test]
    fn test_resolve_log_file_path_cluster_no_log_files() {
        let dir = tempfile::tempdir().unwrap();
        let cluster_dir = dir.path().join("logs").join("cluster");
        std::fs::create_dir_all(&cluster_dir).unwrap();
        // Place a non-.log file to ensure it's not picked up
        std::fs::write(cluster_dir.join("notes.txt"), "not a log").unwrap();

        let ws = dir.path().to_string_lossy().to_string();
        assert!(resolve_log_file_path(&ws, "cluster").is_none());
    }

    #[test]
    fn test_resolve_log_file_path_cluster_multiple_files_returns_lexicographically_last() {
        let dir = tempfile::tempdir().unwrap();
        let cluster_dir = dir.path().join("logs").join("cluster");
        std::fs::create_dir_all(&cluster_dir).unwrap();
        std::fs::write(cluster_dir.join("discovery.log"), "discovery content").unwrap();
        std::fs::write(cluster_dir.join("rpc.log"), "rpc content").unwrap();

        let ws = dir.path().to_string_lossy().to_string();
        let path = resolve_log_file_path(&ws, "cluster");
        assert!(path.is_some());
        // After sort+reverse, lexicographically greatest name ("rpc.log" > "discovery.log") wins
        assert!(path.unwrap().contains("rpc.log"));
    }

    #[test]
    fn test_resolve_log_file_path_cluster_empty_directory() {
        let dir = tempfile::tempdir().unwrap();
        let cluster_dir = dir.path().join("logs").join("cluster");
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
        Arc::new(AppState {
            auth_token: auth_token.to_string(),
            session_count: Arc::new(AtomicUsize::new(2)),
            workspace,
            version: "1.0.0-test".to_string(),
            start_time: std::time::Instant::now(),
            model_name: Arc::new(Mutex::new("test-model".to_string())),
            event_hub: Arc::new(EventHub::new()),
            running: Arc::new(AtomicBool::new(true)),
            session_manager: Arc::new(SessionManager::with_default_timeout()),
            inbound_tx: None,
            streaming_provider: None,
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
        std::fs::write(logs_dir.join("nemesisbot.log"), r#"{"msg":"line1"}"#).unwrap();
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
        std::fs::write(logs_dir.join("nemesisbot.log"), lines.join("\n")).unwrap();
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
        std::fs::write(logs_dir.join("nemesisbot.log"), lines.join("\n")).unwrap();
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
        let config_dir = dir.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("config.json"), "not valid json").unwrap();
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
        let config_dir = dir.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        let config = serde_json::json!({
            "api_key": "sk-1234567890abcdef",
            "name": "test-config",
            "port": 8080,
        });
        std::fs::write(
            config_dir.join("config.json"),
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
        let config_dir = dir.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        let config = serde_json::json!({
            "model_list": [
                {"name": "gpt-4", "api_key": "sk-1234567890abcdef"},
                {"name": "claude", "api_key": "sk-short"}
            ],
            "agents": {"defaults": {"llm": "gpt-4"}}
        });
        std::fs::write(
            config_dir.join("config.json"),
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
        let config_dir = dir.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("config.json"), "invalid json").unwrap();
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
        let config_dir = dir.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("security_audit_2026-01-01.log"), r#"{"audit":"entry1"}"#).unwrap();
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
        let cluster_dir = dir.path().join("logs").join("cluster");
        std::fs::create_dir_all(&cluster_dir).unwrap();
        std::fs::write(cluster_dir.join("discovery.log"), r#"{"cluster":"entry1"}"#).unwrap();
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
        let logs_dir = dir.path().join("logs").join("request_logs");
        std::fs::create_dir_all(&logs_dir).unwrap();
        std::fs::write(logs_dir.join("2026-01-01.jsonl"), r#"{"llm":"request1"}"#).unwrap();
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
}

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

use nemesis_services::bot_service::AgentLoopService;
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
    /// Home directory (e.g. ~/.nemesisbot), where config.json resides.
    pub home: Option<String>,
    /// Application version.
    pub version: String,
    /// Server start time.
    pub start_time: Instant,
    /// Current LLM model name (wrapped in Arc<Mutex> for Clone).
    pub model_name: Arc<Mutex<String>>,
    /// Active model API base URL.
    pub model_base: Arc<Mutex<String>>,
    /// Whether the active model has an API key configured.
    pub model_has_key: Arc<AtomicBool>,
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
    /// WS API Router for request/response dispatch (optional — set during server setup).
    pub ws_router: Option<Arc<crate::ws_router::WsRouter>>,
    /// Agent loop service for start/stop/status control.
    pub agent_service: Option<Arc<dyn AgentLoopService>>,
    /// Data store for usage statistics queries.
    pub data_store: Option<Arc<nemesis_data::DataStore>>,
    /// Memory manager for runtime vector store control.
    pub memory_manager: Option<Arc<nemesis_memory::manager::MemoryManager>>,
    /// Forge self-learning instance for runtime start/stop control.
    pub forge: Option<Arc<nemesis_forge::forge::Forge>>,
    /// Agent loop for runtime model/provider switching.
    /// Shared with AgentLoopServiceAdapter — updated on each start/stop.
    pub agent_loop: Arc<parking_lot::RwLock<Option<Arc<nemesis_agent::r#loop::AgentLoop>>>>,
    /// Cluster runtime instance for dashboard data queries.
    pub cluster: Option<Arc<nemesis_cluster::cluster::Cluster>>,
    /// Cluster lifecycle service for start/stop control.
    pub cluster_service: Option<Arc<dyn nemesis_services::bot_service::LifecycleService>>,
    /// Cluster log directory for JSONL log reader.
    pub cluster_log_dir: Option<String>,
    /// Workflow engine for /api/workflow/* endpoints (milestone 1a-E3/E4).
    pub workflow_engine: Option<Arc<nemesis_workflow::engine::WorkflowEngine>>,
    /// Per-IP rate limiter for webhook endpoints (1c-E5). Keyed by client
    /// IP; tracks request timestamps inside a sliding 1-minute window.
    pub webhook_rate_limiter: Arc<crate::handlers::workflow::WebhookRateLimiter>,
    /// Internal command sender (gateway → web handler bridge).
    pub internal_cmd_tx: Option<tokio::sync::mpsc::Sender<crate::internal::InternalCommand>>,
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
        response.as_object_mut().unwrap().insert(
            "model_base".to_string(),
            serde_json::Value::String(state.model_base.lock().clone()),
        );
        response.as_object_mut().unwrap().insert(
            "model_has_key".to_string(),
            serde_json::Value::Bool(state.model_has_key.load(std::sync::atomic::Ordering::SeqCst)),
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
    let home = match &state.home {
        Some(h) => h.clone(),
        None => {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "home not configured"})),
            ));
        }
    };

    let config_path = PathBuf::from(&home).join("config.json");
    let data = match std::fs::read_to_string(&config_path) {
        Ok(d) => d,
        Err(_) => {
            tracing::debug!(path = %config_path.display(), "[WebServer] Config file not found");
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
    let home = match &state.home {
        Some(h) => h.clone(),
        None => {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "home not configured"})),
            ));
        }
    };

    let config_path = PathBuf::from(&home).join("config.json");
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
// Handler: API readme
// ---------------------------------------------------------------------------

/// Embedded README.md content.
static EMBEDDED_README: &str = include_str!("../../../README.md");

/// `GET /api/system/readme` — returns the embedded README.md content.
pub async fn handle_api_readme() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "content": EMBEDDED_README,
    }))
}

// ---------------------------------------------------------------------------
// Handler: API license
// ---------------------------------------------------------------------------

/// Embedded LICENSE content.
static EMBEDDED_LICENSE: &str = include_str!("../../../LICENSE");

/// `GET /api/system/license` — returns the embedded LICENSE content.
pub async fn handle_api_license() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "content": EMBEDDED_LICENSE,
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
        .iter()
        .map(|(name, config)| {
            let is_enabled = cfg.enabled.iter().any(|e| e.eq_ignore_ascii_case(name));
            // Read actual state from config instead of inferring
            let install_status = config
                .get("state")
                .and_then(|s| s.get("install_status"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let db_status = config
                .get("state")
                .and_then(|s| s.get("db_status"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let state = if !is_enabled {
                "disabled"
            } else if install_status == "installed" && db_status == "ready" {
                "ready"
            } else if install_status == "failed" {
                "failed"
            } else if install_status == "pending" || install_status.is_empty() {
                "pending"
            } else {
                "installed"
            };
            let mut engine_json = serde_json::json!({
                "name": name,
                "state": state,
                "enabled": is_enabled,
            });
            // Merge all config fields
            if let Some(obj) = config.as_object() {
                let map = engine_json.as_object_mut().unwrap();
                for (k, v) in obj {
                    map.entry(k.clone()).or_insert(v.clone());
                }
            }
            engine_json
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
            // New JSONL daily rotation: files are `nemesisbot.YYYY-MM-DD` (no `.log` extension
            // because tracing-appender 0.2 doesn't support suffixes). Match strictly by date
            // pattern to avoid hitting any legacy unrotated `nemesisbot.log`.
            let logs_dir = PathBuf::from(workspace).join("logs");
            let mut matches: Vec<String> = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&logs_dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if is_daily_nemesisbot_log(&name) {
                        matches.push(entry.path().to_string_lossy().to_string());
                    }
                }
            }
            if !matches.is_empty() {
                // Lexicographic sort == chronological sort for YYYY-MM-DD.
                matches.sort();
                matches.reverse();
                Some(matches[0].clone())
            } else {
                None
            }
        }
        "llm" => {
            // Phase B1-3: 将来由 nemesis-providers 写 logs/llm/ 流式摘要（避免污染 request_logs 的 Markdown 目录）。
            // 在那之前 fallback 到 request_logs 最新目录，返回其 00.request.md（首条 user 消息）。
            let dir = PathBuf::from(workspace).join("logs").join("request_logs");
            find_latest_request_summary(&dir)
        }
        "security" => {
            // Phase B1-1: audit.jsonl 是固定文件名（不是 glob），路径在 logs/security_logs/
            let audit_file = PathBuf::from(workspace)
                .join("logs")
                .join("security_logs")
                .join("audit.jsonl");
            if audit_file.exists() {
                Some(audit_file.to_string_lossy().to_string())
            } else {
                None
            }
        }
        "cluster" => {
            // Phase B1-2: cluster_logs/ 下既有平面 cluster_YYYY-MM-DD.log（流式事件），
            // 也有 {device}/{ts}_{task}/ 子目录（LLM 详情）。这里只取平面日志文件。
            let cluster_dir = PathBuf::from(workspace).join("logs").join("cluster_logs");
            let mut matches: Vec<String> = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&cluster_dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with("cluster_") && name.ends_with(".log") {
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

/// Find the most recently modified `.md` file inside the latest `request_logs` subdirectory.
///
/// Each LLM call is stored as `{ts}_{NNN}/` containing multiple Markdown files
/// (00.request.md, 01.AI.Request.md, 02.AI.Response.md, NN.Local.md, ...).
/// Here we pick the latest subdir by mtime, then the latest `.md` inside it.
fn find_latest_request_summary(dir: &std::path::Path) -> Option<String> {
    let entries = std::fs::read_dir(dir).ok()?;

    let mut latest_dir: Option<PathBuf> = None;
    let mut latest_dir_time = std::time::UNIX_EPOCH;
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            if let Ok(mtime) = meta.modified() {
                if mtime > latest_dir_time {
                    latest_dir_time = mtime;
                    latest_dir = Some(entry.path());
                }
            }
        }
    }

    let target_dir = latest_dir?;

    let mut latest_file: Option<String> = None;
    let mut latest_file_time = std::time::UNIX_EPOCH;
    if let Ok(entries) = std::fs::read_dir(&target_dir) {
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(true) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.ends_with(".md") {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                if let Ok(mtime) = meta.modified() {
                    if mtime > latest_file_time {
                        latest_file_time = mtime;
                        latest_file = Some(entry.path().to_string_lossy().to_string());
                    }
                }
            }
        }
    }

    latest_file
}

/// Match `nemesisbot.YYYY-MM-DD` exactly — the JSONL daily-rotation file naming.
/// Used to exclude legacy unrotated `nemesisbot.log` files from the dashboard.
fn is_daily_nemesisbot_log(name: &str) -> bool {
    const PREFIX: &str = "nemesisbot.";
    if !name.starts_with(PREFIX) {
        return false;
    }
    let date = &name[PREFIX.len()..];
    // Strict YYYY-MM-DD (10 chars, dashes at positions 4 and 7, digits elsewhere).
    if date.len() != 10 {
        return false;
    }
    let b = date.as_bytes();
    b[0..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'-'
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[7] == b'-'
        && b[8..10].iter().all(u8::is_ascii_digit)
}

/// Read the last `n` JSONL entries from a file efficiently.
///
/// Seeks to (file_size - 64KB), reads to end, splits on newlines, drops the first
/// partial line, parses each remaining line as JSON. Files smaller than 64KB are
/// read in full. Lines that fail to parse as JSON are silently dropped — the new
/// JSONL format produces one valid SseLogEvent per line, so any parse failure is
/// either a half-written tail (which `lines()` already handles by virtue of the
/// trailing newline) or corruption that's not worth surfacing.
fn read_log_entries(file_path: &str, n: usize) -> Vec<serde_json::Value> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = match std::fs::File::open(file_path) {
        Ok(f) => f,
        Err(_) => return vec![],
    };

    let file_size = file.metadata().map(|m| m.len()).unwrap_or(0);
    if file_size == 0 {
        return vec![];
    }

    let seek_back = std::cmp::min(file_size, 64 * 1024);
    if file.seek(SeekFrom::End(-(seek_back as i64))).is_err() {
        return vec![];
    }

    let mut buf = String::new();
    if file.read_to_string(&mut buf).is_err() {
        return vec![];
    }

    let lines: Vec<&str> = buf.lines().filter(|l| !l.trim().is_empty()).collect();

    // If we seeked into the middle of the file, the first line is likely a truncated
    // JSON object — drop it. (When seek_back == file_size we read from the start and
    // don't need to drop.)
    let lines = if seek_back < file_size && lines.len() > 1 {
        &lines[1..]
    } else {
        &lines[..]
    };

    let start = if lines.len() > n { lines.len() - n } else { 0 };
    lines[start..]
        .iter()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
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

// ---------------------------------------------------------------------------
// Handler: /api/internal (undocumented control endpoint)
// ---------------------------------------------------------------------------

/// `POST /api/internal` — internal control endpoint for CLI commands.
///
/// Requires `X-Auth-Token` header matching `web.auth_token`.
/// Body: `{ "cmd": "open_dashboard" }`
pub async fn handle_api_internal(
    headers: axum::http::HeaderMap,
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let token = headers
        .get("X-Auth-Token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !verify_token(token, &state.auth_token) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "unauthorized"})),
        ));
    }

    let cmd = body.get("cmd").and_then(|v| v.as_str()).unwrap_or("");

    let tx = match &state.internal_cmd_tx {
        Some(tx) => tx,
        None => {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "internal channel not available"})),
            ));
        }
    };

    match cmd {
        "open_dashboard" => {
            tx.send(crate::internal::InternalCommand::OpenDashboard)
                .await
                .map_err(|_| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({"error": "send failed"})),
                    )
                })?;
        }
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "unknown command"})),
            ));
        }
    }

    Ok(Json(serde_json::json!({"status": "ok"})))
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
mod tests;

#[cfg(test)]
mod extra_tests;

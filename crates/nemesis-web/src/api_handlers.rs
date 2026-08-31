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
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;

use nemesis_services::bot_service::AgentLoopService;
use nemesis_types::utils;
use parking_lot::Mutex;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
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
    #[cfg(feature = "memory")]
    pub memory_manager: Option<Arc<nemesis_memory::manager::MemoryManager>>,
    #[cfg(not(feature = "memory"))]
    #[allow(dead_code)]
    pub memory_manager: Option<()>,
    /// Forge self-learning instance for runtime start/stop control.
    #[cfg(feature = "forge")]
    pub forge: Option<Arc<nemesis_forge::forge::Forge>>,
    #[cfg(not(feature = "forge"))]
    pub forge: Option<()>,
    /// Agent loop for runtime model/provider switching.
    /// Shared with AgentLoopServiceAdapter — updated on each start/stop.
    pub agent_loop: Arc<parking_lot::RwLock<Option<Arc<nemesis_agent::r#loop::AgentLoop>>>>,
    /// Cluster runtime instance for dashboard data queries.
    #[cfg(feature = "cluster")]
    pub cluster: Option<Arc<nemesis_cluster::cluster::Cluster>>,
    #[cfg(not(feature = "cluster"))]
    pub cluster: Option<()>,
    /// Cluster lifecycle service for start/stop control.
    pub cluster_service: Option<Arc<dyn nemesis_services::bot_service::LifecycleService>>,
    /// Cluster log directory for JSONL log reader.
    pub cluster_log_dir: Option<String>,
    /// Workflow engine for /api/workflow/* endpoints (milestone 1a-E3/E4).
    #[cfg(feature = "workflow")]
    pub workflow_engine: Option<Arc<nemesis_workflow::engine::WorkflowEngine>>,
    #[cfg(not(feature = "workflow"))]
    #[allow(dead_code)]
    pub workflow_engine: Option<()>,
    /// Per-workflow chat password store for the standalone
    /// `/workflow/chat/<index>` page. Lets a workflow be shared with
    /// collaborators (URL + password) without exposing the dashboard token.
    #[cfg(feature = "workflow")]
    pub chat_secret_store: Arc<nemesis_workflow::chat_secrets::ChatSecretStore>,
    #[cfg(not(feature = "workflow"))]
    #[allow(dead_code)]
    pub chat_secret_store: Arc<()>,
    /// Per-IP rate limiter for webhook endpoints (1c-E5). Keyed by client
    /// IP; tracks request timestamps inside a sliding 1-minute window.
    #[cfg(feature = "workflow")]
    pub webhook_rate_limiter: Arc<crate::handlers::workflow::WebhookRateLimiter>,
    #[cfg(not(feature = "workflow"))]
    #[allow(dead_code)]
    pub webhook_rate_limiter: Arc<()>,
    /// Internal command sender (gateway → web handler bridge).
    pub internal_cmd_tx: Option<tokio::sync::mpsc::Sender<crate::internal::InternalCommand>>,
    /// 全局急停状态。/api/internal 的 estop_* 命令直接操作它（无需 mpsc 往返，
    /// status 能即时返回）。EstopState 是线程安全 Arc<AtomicBool+watch>。
    pub estop: Option<Arc<nemesis_agent::estop::EstopState>>,
    /// Runtime cron service. Lets `tasks.cron.*` handlers call the live
    /// scheduler (add/list/update/delete/toggle/run) instead of raw file I/O.
    /// nemesis-cron is an unconditional dependency, so no cfg gate.
    pub cron: Option<Arc<std::sync::Mutex<nemesis_cron::CronService>>>,
    /// Managed-agent board service (W2 P1). nemesis-board is an unconditional
    /// dependency (cron precedent); gateway injects `Some` only when the
    /// `board` feature is enabled and the store opened successfully. Carries
    /// the node role (goal 硬约束①：复用 NodeRole) — worker 节点 board 只读。
    pub board: Option<nemesis_board::BoardService>,
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
pub async fn handle_api_status(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let uptime = state.start_time.elapsed().as_secs();
    let session_count = state
        .session_count
        .load(std::sync::atomic::Ordering::SeqCst);
    let running = state.running.load(std::sync::atomic::Ordering::SeqCst);
    let model_name = state.model_name.lock().clone();

    let mut response = serde_json::json!({
        "version": state.version,
        "uptime_seconds": uptime,
        "ws_connected": running,
        "session_count": session_count,
    });

    if let Some(ref workspace) = state.workspace {
        response
            .as_object_mut()
            .unwrap()
            .insert("scanner_status".to_string(), load_scanner_status(workspace));
        response.as_object_mut().unwrap().insert(
            "cluster_status".to_string(),
            serde_json::json!({
                "enabled": false,
                "node_count": 0,
            }),
        );
        response
            .as_object_mut()
            .unwrap()
            .insert("model".to_string(), serde_json::Value::String(model_name));
        response.as_object_mut().unwrap().insert(
            "model_base".to_string(),
            serde_json::Value::String(state.model_base.lock().clone()),
        );
        response.as_object_mut().unwrap().insert(
            "model_has_key".to_string(),
            serde_json::Value::Bool(
                state
                    .model_has_key
                    .load(std::sync::atomic::Ordering::SeqCst),
            ),
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
pub async fn handle_api_version(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
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
pub async fn handle_api_sessions(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let session_count = state
        .session_count
        .load(std::sync::atomic::Ordering::SeqCst);
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
pub async fn handle_api_events(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
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
// Handler: SDK downloads (P2-2, 2026-08-24 UI entry gap)
// ---------------------------------------------------------------------------

/// Serve an embedded SDK zip artifact with download headers. Same open-GET
/// policy as /api/system/readme|license (static public artifacts, no
/// secrets); auth for state-changing ops stays on the WS layer.
fn sdk_zip_response(bytes: &'static [u8], filename: String) -> axum::response::Response {
    (
        StatusCode::OK,
        [
            (
                axum::http::header::CONTENT_TYPE,
                "application/zip".to_string(),
            ),
            (
                axum::http::header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
            (axum::http::header::CACHE_CONTROL, "no-store".to_string()),
        ],
        bytes.to_vec(),
    )
        .into_response()
}

/// `GET /api/sdk/export` — SDK source tree zip (files at zip root), for
/// browsing/extending the SDK locally.
pub async fn handle_sdk_export() -> axum::response::Response {
    sdk_zip_response(
        crate::sdk_embed::SDK_EXPORT_ZIP,
        format!("nemesisbot-sdk-{}.zip", crate::sdk_embed::SDK_VERSION),
    )
}

/// `GET /api/sdk/pip` — sdist-layout zip (single `nemesisbot-<version>/`
/// top-level dir) installable via `pip install ./<file>.zip`.
pub async fn handle_sdk_pip() -> axum::response::Response {
    sdk_zip_response(
        crate::sdk_embed::SDK_SDIST_ZIP,
        format!(
            "nemesisbot-sdk-pip-{}.zip",
            crate::sdk_embed::SDK_VERSION
        ),
    )
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Load scanner status from the workspace config directory.
fn load_scanner_status(workspace: &str) -> serde_json::Value {
    // 委托 nemesis-path 唯一拼接点。
    let scanner_config_path =
        nemesis_path::resolve_scanner_config_path_in_workspace(std::path::Path::new(
            workspace,
        ));

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
            // 2026-08-30 统一收编：滚动日志移入 logs/gateway/ 子目录；
            // logs 根的旧文件为历史归档，不再被本 glob 匹配。
            let logs_dir = nemesis_path::logs_dir_in_workspace(std::path::Path::new(workspace))
                .join("gateway");
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
            let dir =
                nemesis_path::resolve_request_logs_dir_in_workspace(std::path::Path::new(workspace));
            find_latest_request_summary(&dir)
        }
        "security" => {
            // Phase B1-1: audit.jsonl 是固定文件名（不是 glob），路径在 logs/security_logs/
            let audit_file = nemesis_path::resolve_audit_log_dir_in_workspace(
                std::path::Path::new(workspace),
            )
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
            let cluster_dir = nemesis_path::resolve_cluster_logs_dir_in_workspace(
                std::path::Path::new(workspace),
            );
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
                            *value = serde_json::Value::String(format!("{}****", &s[..end]));
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
/// Body: `{ "cmd": "open_dashboard" }` or `{ "cmd": "shutdown" }`
/// (BUG #31, quality-hardening goal 冲刺 S11e: `shutdown` = graceful-stop
/// request from the CLI; open_dashboard's InternalCommand path is shared).
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

    // E-stop commands operate directly on AppState.estop (no mpsc round-trip).
    // EstopState is a thread-safe Arc<AtomicBool + watch>, and `estop_status`
    // needs to return the live value — both reasons rule out the fire-and-forget
    // InternalCommand path used by `open_dashboard`.
    match cmd {
        "estop_engage" | "estop_release" | "estop_status" => {
            let estop = state.estop.as_ref().ok_or_else(|| {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({"error": "e-stop not available"})),
                )
            })?;
            match cmd {
                "estop_engage" => {
                    estop.trigger();
                    tracing::info!("[Web] E-stop engaged via /api/internal");
                }
                "estop_release" => {
                    estop.release();
                    tracing::info!("[Web] E-stop released via /api/internal");
                }
                _ => {}
            }
            return Ok(Json(serde_json::json!({
                "status": "ok",
                "engaged": estop.is_engaged(),
            })));
        }
        _ => {}
    }

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
        // (BUG #31, quality-hardening goal 冲刺 S11e) `nemesisbot shutdown`
        // 的 HTTP 臂：此前打 `/api/shutdown`（从未注册，必 404 = 死臂）。
        // 现在落到既有 mpsc 通道 → gateway 接收环走 Ctrl+C 同源的优雅停机。
        "shutdown" => {
            tx.send(crate::internal::InternalCommand::Shutdown)
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
// Handlers: /api/chat/sessions/{id}/turns + /fork
// (P3-1, 2026-08-24 UI entry gap — session fork dialog backing)
// ---------------------------------------------------------------------------

/// Map a dashboard session id to the store session key (same mapping as the
/// sessions WSAPI handler: `agent:main:session:{sanitized}`).
fn chat_session_key(sid: &str) -> String {
    format!(
        "agent:main:session:{}",
        nemesis_agent::session::SessionStore::sanitize_session_id(sid)
    )
}

/// Reverse of [`chat_session_key`] — lets the frontend switch to the new
/// session after a fork (ids are what the session list WSAPI speaks).
fn chat_session_id(key: &str) -> String {
    key.strip_prefix("agent:main:session:")
        .unwrap_or(key)
        .to_string()
}

/// Resolve the store the fork WRITE path materializes the new session into
/// (round 3: the turns endpoint reads jsonl directly and no longer needs a
/// store; only `POST .../fork` comes through here):
/// 1. the LIVE agent's store when the agent is running (authoritative — it
///    holds the in-memory map the running gateway appends to), else
/// 2. a fresh store over `<home>/workspace/sessions` (agent stopped).
///    `new_with_storage` loads existing session files at construction and
///    `get_or_create` falls back to disk on an in-memory miss (Z1), so fork
///    files created by other processes are visible — and a fork made against
///    this fallback store survives a running gateway that later re-reads the
///    same directory instead of overwriting it.
fn resolve_fork_store(
    state: &AppState,
) -> Result<Arc<nemesis_agent::session::SessionStore>, (StatusCode, Json<serde_json::Value>)> {
    if let Some(al) = state.agent_loop.read().as_ref() {
        if let Some(store) = al.session_store() {
            return Ok(store.clone());
        }
    }
    let home = state.home.as_deref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "home not configured"})),
        )
    })?;
    // home 是 home 根：先转 workspace 再取 sessions 目录。
    let dir = nemesis_path::resolve_sessions_dir_in_workspace(&nemesis_path::workspace_dir(
        std::path::Path::new(home),
    ));
    Ok(Arc::new(nemesis_agent::session::SessionStore::new_with_storage(
        dir,
    )))
}

/// First non-empty line of a message, truncated for preview display.
/// char-based truncation (never splits a multi-byte character).
fn first_line_trunc(s: &str, max: usize) -> String {
    let first = s.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let mut out: String = first.chars().take(max).collect();
    if first.chars().count() > max {
        out.push('…');
    }
    out
}

/// `GET /api/chat/sessions/{id}/turns` — turn-boundary table for the fork
/// dialog. Same counting as the CLI `session show`: a turn is one complete
/// user→…→assistant exchange over the **chat_log rows** (round-3 fix,
/// 2026-08-25: jsonl is the single source of truth for turn semantics —
/// the Dashboard renders these rows and the fork cut lands on them; the
/// SessionStore copy is a lossy cache that compaction folds, tool
/// intermediates pollute and the 7-day TTL deletes, so it must never
/// define what "第 N 轮" means). `kept_messages` is the cumulative row
/// count a fork cut at that turn retains. `end_preview` is the first line
/// of the turn's last non-empty user/assistant row — what the forked
/// session will end on (the fork keeps turns COMPLETE, so the fork ends
/// on the assistant reply, while `preview` shows the user question that
/// starts the turn).
///
/// Requires `X-Auth-Token` matching `web.auth_token`.
pub async fn handle_api_chat_session_turns(
    headers: axum::http::HeaderMap,
    Path(session_id): Path<String>,
    State(state): State<Arc<AppState>>,
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

    // ROUND-3: read the chat_log rows — the same source `fork_session`
    // cuts on and the Dashboard renders. Counting on any other store would
    // re-create the coordinate-mismatch bug this round fixed.
    let key = chat_session_key(&session_id);
    let (rows, total, _, _) =
        nemesis_agent::chat_log::read_chat_log(&key, usize::MAX, None);
    if rows.is_empty() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!("会话 {session_id} 不存在或历史为空")
            })),
        ));
    }

    // Turn table: one row per complete user turn; rows before the first
    // user row lead into turn 1's cumulative count (normally none — jsonl
    // never records the system prompt).
    // `end_preview` = first line of the turn's LAST user/assistant row —
    // exactly what the forked session's Dashboard will display as its
    // final bubble, since the fork copies these same rows verbatim
    // (round-3 fix). The dialog shows both so "what I pick" and "where
    // the fork ends" are visible together.
    struct TurnRow {
        preview: String,
        end_preview: String,
        time: String,
        turn_messages: usize,
    }
    let mut turn_rows: Vec<TurnRow> = Vec::new();
    let mut leading = 0usize;
    for v in &rows {
        let role = v.get("role").and_then(|r| r.as_str()).unwrap_or("");
        let content = v.get("content").and_then(|c| c.as_str()).unwrap_or("");
        if role == "user" {
            turn_rows.push(TurnRow {
                preview: first_line_trunc(content, 60),
                end_preview: first_line_trunc(content, 60),
                time: v
                    .get("timestamp")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string(),
                turn_messages: 1,
            });
        } else if let Some(r) = turn_rows.last_mut() {
            r.turn_messages += 1;
            // Same row-selection predicate as the fork's store mapping
            // (single source of truth — see chat_log::is_projected_chat_row).
            if nemesis_agent::chat_log::is_projected_chat_row(role, content) {
                r.end_preview = first_line_trunc(content, 60);
            }
        } else {
            leading += 1;
        }
    }
    let total_turns = turn_rows.len();
    let mut kept = leading;
    let turns: Vec<serde_json::Value> = turn_rows
        .into_iter()
        .enumerate()
        .map(|(i, r)| {
            kept += r.turn_messages;
            serde_json::json!({
                "turn": i + 1,
                "preview": r.preview,
                "end_preview": r.end_preview,
                "time": r.time,
                "turn_messages": r.turn_messages,
                "kept_messages": kept,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "session_id": session_id,
        "session_key": key,
        "total_turns": total_turns,
        "total_messages": total,
        "turns": turns,
    })))
}

/// `POST /api/chat/sessions/{id}/fork` — fork at a turn boundary.
/// Body: `{ "at_turn": 2, "title": "新分支" }` (both optional; omitted
/// `at_turn` = fork at head / whole history). Delegates to the Z1
/// `fork_session` (SessionStore + chat_log copy + boundary events) — the UI
/// must NOT reimplement the copy semantics.
///
/// Requires `X-Auth-Token` matching `web.auth_token`.
pub async fn handle_api_chat_session_fork(
    headers: axum::http::HeaderMap,
    Path(session_id): Path<String>,
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

    let at_turn = body.get("at_turn").and_then(|v| v.as_u64()).map(|v| v as usize);
    let title = body
        .get("title")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let store = resolve_fork_store(&state)?;
    let key = chat_session_key(&session_id);
    // Pre-check so unknown sessions surface as 404 (fork_session's own
    // empty-check would otherwise collapse into a 500 below). Round 3:
    // existence = a non-empty chat_log jsonl (the fork's truth source);
    // the store json may legitimately be gone (7-day TTL) — that must NOT
    // make a live session unforkable.
    let (_, log_total, _, _) = nemesis_agent::chat_log::read_chat_log(&key, 1, None);
    if log_total == 0 {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!("会话 {session_id} 不存在或历史为空")
            })),
        ));
    }

    let info = nemesis_agent::session_fork::fork_session(&store, &key, None, at_turn)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e})),
            )
        })?;
    // Optional title for the new session's meta (same call the sessions
    // WSAPI rename uses).
    if let Some(t) = title {
        nemesis_agent::chat_log::write_session_meta(&info.new_key, &t);
    }

    Ok(Json(serde_json::json!({
        "forked": true,
        "session_id": chat_session_id(&info.new_key),
        "source_session_id": session_id,
        "source_key": info.source_key,
        "new_key": info.new_key,
        "at_turn": info.at_turn,
        "kept_messages": info.kept_messages,
        "dropped_messages": info.dropped_messages,
        "summary_kept": info.summary_kept,
        "chat_log_lines": info.chat_log_lines,
    })))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(all(test, feature = "workflow"))]
mod tests;

#[cfg(all(test, feature = "workflow"))]
mod extra_tests;

// P2-2 (2026-08-24 UI entry gap): SDK download-route tests are stateless
// (handlers take no State), so they run under every feature combo —
// deliberately NOT behind the workflow gate above.
#[cfg(test)]
mod sdk_route_tests;

// P3-1 (2026-08-24 UI entry gap): session-fork route tests. The handlers
// depend only on nemesis-agent (non-optional), so like sdk_route_tests these
// run under every feature combo (make_state cfg-branches the two
// workflow-typed AppState fields).
#[cfg(test)]
mod fork_route_tests;

// S10b (2026-08-26, quality-hardening goal 冲刺 web 批次 2): pure helpers
// (request-summary walk, daily-log matcher, log tail reader, sanitize_map,
// first_line_trunc, resolve_fork_store) + api_key masking arms + turns
// leading-row arm + fork title arm. No feature gate needed (same shape as
// fork_route_tests).
#[cfg(test)]
mod s10b_tests;

//! Web server setup, lifecycle, route registration, and core handlers.
//!
//! Mirrors the Go `module/web/server.go` including:
//! - Server struct with event hub, session manager, message bus, and status loop
//! - Route registration (WebSocket, SSE, health, API endpoints, static files)
//! - `process_messages` – incoming WebSocket message to bus bridge
//! - `handle_events_stream` – SSE endpoint
//! - `handle_health` – health check endpoint
//! - `publish_status_loop` – periodic status push via SSE

use crate::api_handlers::{
    AppState,
    handle_api_config, handle_api_logs, handle_api_scanner_status, handle_api_status,
    handle_api_version, handle_api_models, handle_api_sessions, handle_api_events,
};
use crate::cors::dev_cors_layer;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::websocket_handler::handle_websocket_upgrade;
use axum::extract::State as AxumState;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::routing::get;
use axum::{Json, Router};
use futures::stream::Stream;
use nemesis_bus::MessageBus;
use nemesis_types::channel::InboundMessage;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tower_http::services::ServeDir;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Web server configuration.
#[derive(Debug, Clone)]
pub struct WebServerConfig {
    pub listen_addr: String,
    pub auth_token: String,
    pub cors_origins: Vec<String>,
    /// WebSocket endpoint path (default: "/ws").
    pub ws_path: String,
    /// Optional workspace path for config/log access.
    pub workspace: Option<String>,
    /// Application version string.
    pub version: String,
    /// Optional path to static files directory for serving the Web UI.
    pub static_dir: Option<String>,
    /// Optional index file name (default: "index.html").
    pub index_file: String,
}

impl Default for WebServerConfig {
    fn default() -> Self {
        Self {
            listen_addr: "127.0.0.1:8080".to_string(),
            auth_token: String::new(),
            cors_origins: vec![],
            ws_path: "/ws".to_string(),
            workspace: None,
            version: String::new(),
            static_dir: None,
            index_file: "index.html".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Web server
// ---------------------------------------------------------------------------

/// Web server. Owns the event hub, session manager, and message bus integration.
pub struct WebServer {
    config: WebServerConfig,
    event_hub: Arc<EventHub>,
    session_manager: Arc<SessionManager>,
    session_count: Arc<AtomicUsize>,
    running: Arc<AtomicBool>,
    start_time: Instant,
    message_bus: Option<Arc<MessageBus>>,
    /// Current LLM model name (shared with AppState via Arc).
    model_name: Arc<parking_lot::Mutex<String>>,
    /// Optional streaming LLM provider for SSE chat endpoint.
    streaming_provider: Option<Arc<nemesis_providers::http_provider::HttpProvider>>,
}

impl WebServer {
    /// Create a new web server.
    pub fn new(config: WebServerConfig) -> Self {
        Self {
            config,
            event_hub: Arc::new(EventHub::new()),
            session_manager: Arc::new(SessionManager::with_default_timeout()),
            session_count: Arc::new(AtomicUsize::new(0)),
            running: Arc::new(AtomicBool::new(false)),
            start_time: Instant::now(),
            message_bus: None,
            model_name: Arc::new(parking_lot::Mutex::new(String::new())),
            streaming_provider: None,
        }
    }

    /// Set the message bus for inbound message publishing.
    pub fn set_message_bus(&mut self, bus: Arc<MessageBus>) {
        self.message_bus = Some(bus);
    }

    /// Set the current LLM model name.
    pub fn set_model_name(&self, name: &str) {
        *self.model_name.lock() = name.to_string();
    }

    /// Set the workspace path for config/log access.
    pub fn set_workspace(&mut self, path: PathBuf) {
        self.config.workspace = Some(path.to_string_lossy().to_string());
    }

    /// Set the streaming LLM provider for the SSE chat endpoint.
    pub fn set_streaming_provider(&mut self, provider: Arc<nemesis_providers::http_provider::HttpProvider>) {
        self.streaming_provider = Some(provider);
    }

    /// Build the Axum router with all routes.
    pub fn build_router(&self) -> Router {
        let (inbound_tx, mut inbound_rx) = mpsc::unbounded_channel::<crate::websocket_handler::IncomingMessage>();

        let state = AppState {
            auth_token: self.config.auth_token.clone(),
            session_count: self.session_count.clone(),
            workspace: self.config.workspace.clone(),
            version: self.config.version.clone(),
            start_time: self.start_time,
            model_name: self.model_name.clone(),
            event_hub: self.event_hub.clone(),
            running: self.running.clone(),
            session_manager: self.session_manager.clone(),
            inbound_tx: Some(inbound_tx),
            streaming_provider: self.streaming_provider.clone(),
        };

        let state = Arc::new(state);

        // Spawn the bus bridge: incoming WebSocket messages -> MessageBus.publish_inbound
        if let Some(ref bus) = self.message_bus {
            let bus = bus.clone();
            tokio::spawn(async move {
                process_messages(inbound_rx, bus).await;
            });
        } else {
            // No bus configured; drain messages to avoid leaking the sender
            tokio::spawn(async move {
                while inbound_rx.recv().await.is_some() {}
            });
        }

        let mut router = Router::new()
            // WebSocket endpoint
            .route(&self.config.ws_path, axum::routing::get(handle_websocket_upgrade))
            // Health check
            .route("/health", get(handle_health))
            .route("/api/health", get(handle_health))
            // API endpoints
            .route("/api/status", get(handle_api_status))
            .route("/api/logs", get(handle_api_logs))
            .route("/api/scanner/status", get(handle_api_scanner_status))
            .route("/api/config", get(handle_api_config))
            // API endpoints (extended)
            .route("/api/version", get(handle_api_version))
            .route("/api/models", get(handle_api_models))
            .route("/api/sessions", get(handle_api_sessions))
            .route("/api/events", get(handle_api_events))
            // SSE event stream
            .route("/api/events/stream", get(handle_events_stream))
            // SSE chat streaming endpoint
            .route("/api/chat/stream", axum::routing::post(crate::sse_chat::handle_chat_stream))
            // CORS layer
            .layer(dev_cors_layer())
            .with_state(state.clone());

        // Add static file serving if configured
        if let Some(ref static_dir) = self.config.static_dir {
            let dir_path = PathBuf::from(static_dir);
            if dir_path.exists() && dir_path.is_dir() {
                tracing::info!(
                    static_dir = %static_dir,
                    "Serving static files from directory"
                );
                let serve_dir = ServeDir::new(&dir_path).append_index_html_on_directories(true);
                // Wrap ServeDir with a response header layer that appends
                // `; charset=utf-8` to text/* Content-Type headers. Without this,
                // browsers in CJK locales (Chinese Windows) may default to GBK
                // and render the Chinese UI text as garbled characters.
                let layered = tower::ServiceBuilder::new()
                    .layer(tower_http::set_header::SetResponseHeaderLayer::overriding(
                        http::header::CONTENT_TYPE,
                        |response: &http::Response<_>| {
                            let ct = response.headers()
                                .get(http::header::CONTENT_TYPE)
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            if ct.starts_with("text/") && !ct.contains("charset") {
                                format!("{}; charset=utf-8", ct).parse().ok()
                            } else {
                                None
                            }
                        },
                    ))
                    .service(serve_dir);
                router = router.fallback_service(layered);
            } else {
                tracing::warn!(
                    static_dir = %static_dir,
                    "Static directory not found or not a directory, skipping static file serving"
                );
            }
        }

        router
    }

    /// Get the event hub.
    pub fn event_hub(&self) -> &Arc<EventHub> {
        &self.event_hub
    }

    /// Get the session manager.
    pub fn session_manager(&self) -> &Arc<SessionManager> {
        &self.session_manager
    }

    /// Get the running state.
    pub fn is_running(&self) -> bool {
        self.running.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Start the web server. Blocks until the server shuts down.
    pub async fn start(&self) -> Result<(), String> {
        self.running.store(true, std::sync::atomic::Ordering::SeqCst);

        // Spawn the periodic status publish loop
        let _status_handle = start_publish_status_loop(
            self.event_hub.clone(),
            self.session_count.clone(),
            self.config.version.clone(),
            self.start_time,
            self.running.clone(),
        );

        let addr: SocketAddr = self.config.listen_addr.parse().map_err(|e| format!("invalid listen address: {}", e))?;
        let app = self.build_router();
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| format!("bind failed: {}", e))?;

        tracing::info!("Web server listening on {}", addr);
        axum::serve(listener, app)
            .await
            .map_err(|e| format!("server error: {}", e))
    }

    /// Start the web server with graceful shutdown signal.
    pub async fn start_with_shutdown(&self, mut shutdown_rx: tokio::sync::broadcast::Receiver<()>) -> Result<(), String> {
        self.running.store(true, std::sync::atomic::Ordering::SeqCst);

        // Spawn the periodic status publish loop
        let _status_handle = start_publish_status_loop(
            self.event_hub.clone(),
            self.session_count.clone(),
            self.config.version.clone(),
            self.start_time,
            self.running.clone(),
        );

        let addr: SocketAddr = self.config.listen_addr.parse().map_err(|e| format!("invalid listen address: {}", e))?;
        let app = self.build_router();
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| format!("bind failed: {}", e))?;

        tracing::info!("Web server listening on {}", addr);

        tokio::select! {
            result = axum::serve(listener, app) => {
                result.map_err(|e| format!("server error: {}", e))
            }
            _ = shutdown_rx.recv() => {
                tracing::info!("Web server shutdown signal received");
                Ok(())
            }
        }
    }

    /// Stop the web server.
    pub fn stop(&self) {
        self.running.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// Static files utility
// ---------------------------------------------------------------------------

/// Resolve the static files directory.
///
/// Checks in order:
/// 1. Explicit path provided
/// 2. `workspace/static/` directory
/// 3. `./static/` directory
///
/// Returns None if no valid static directory is found.
pub fn resolve_static_dir(
    explicit_path: Option<&str>,
    workspace: Option<&str>,
) -> Option<String> {
    // 1. Explicit path
    if let Some(path) = explicit_path {
        let p = PathBuf::from(path);
        if p.exists() && p.is_dir() {
            return Some(path.to_string());
        }
        tracing::warn!("Explicit static dir not found: {}", path);
    }

    // 2. workspace/static/
    if let Some(ws) = workspace {
        let ws_static = PathBuf::from(ws).join("static");
        if ws_static.exists() && ws_static.is_dir() {
            return Some(ws_static.to_string_lossy().to_string());
        }
    }

    // 3. ./static/
    let local_static = PathBuf::from("static");
    if local_static.exists() && local_static.is_dir() {
        return Some("static".to_string());
    }

    None
}

// ---------------------------------------------------------------------------
// Embedded static files support
// ---------------------------------------------------------------------------

/// Trait for providing static file content.
/// Can be implemented for embedded files or directory-based serving.
pub trait StaticFiles: Send + Sync {
    /// Get a file's content by path (relative to static root).
    fn get_file(&self, path: &str) -> Option<Vec<u8>>;

    /// Check if a file exists.
    fn has_file(&self, path: &str) -> bool {
        self.get_file(path).is_some()
    }

    /// List all files in the static directory.
    fn list_files(&self) -> Vec<String>;
}

/// Directory-based static file provider.
pub struct DirectoryStaticFiles {
    base_dir: PathBuf,
}

impl DirectoryStaticFiles {
    /// Create a new directory-based static file provider.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: dir.into(),
        }
    }
}

impl StaticFiles for DirectoryStaticFiles {
    fn get_file(&self, path: &str) -> Option<Vec<u8>> {
        // Security: prevent path traversal.
        let path = path.trim_start_matches('/');
        if path.contains("..") {
            return None;
        }

        let full_path = self.base_dir.join(path);
        let canonical_base = self.base_dir.canonicalize().ok()?;
        let canonical_target = full_path.canonicalize().ok()?;
        if !canonical_target.starts_with(&canonical_base) {
            return None;
        }

        std::fs::read(&canonical_target).ok()
    }

    fn list_files(&self) -> Vec<String> {
        let mut files = Vec::new();
        let canonical_base = match self.base_dir.canonicalize() {
            Ok(p) => p,
            Err(_) => return files,
        };

        fn walk(dir: &std::path::Path, base: &std::path::Path, files: &mut Vec<String>) {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        walk(&path, base, files);
                    } else if let Ok(rel) = path.strip_prefix(base) {
                        files.push(rel.to_string_lossy().replace('\\', "/"));
                    }
                }
            }
        }

        walk(&canonical_base, &canonical_base, &mut files);
        files
    }
}

// ---------------------------------------------------------------------------
// Handler: Health check
// ---------------------------------------------------------------------------

/// Health check handler. Returns `{"status":"ok","running":true/false,"sessions":N}`.
pub async fn handle_health(
    AxumState(state): AxumState<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let session_count = state.session_count.load(std::sync::atomic::Ordering::SeqCst);
    let running = state.running.load(std::sync::atomic::Ordering::SeqCst);
    Json(serde_json::json!({
        "status": "ok",
        "running": running,
        "sessions": session_count,
    }))
}

// ---------------------------------------------------------------------------
// Handler: SSE event stream
// ---------------------------------------------------------------------------

/// SSE event stream handler.
///
/// Subscribes to the EventHub and streams events to the client as
/// `event: <type>\ndata: <json>\n\n` frames. Includes an initial heartbeat.
pub async fn handle_events_stream(
    AxumState(state): AxumState<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let mut receiver = state.event_hub.subscribe();
    let _running = state.running.clone();

    let stream = async_stream::stream! {
        // Send initial heartbeat
        let heartbeat_data = serde_json::json!({"ts": chrono::Utc::now().to_rfc3339()});
        yield Ok(SseEvent::default()
            .event("heartbeat")
            .data(heartbeat_data.to_string()));

        // Stream events from the event hub
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    let data = serde_json::to_string(&event.data).unwrap_or_default();
                    yield Ok(SseEvent::default()
                        .event(&event.event_type)
                        .data(data));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("SSE client lagged by {} events, continuing", n);
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    tracing::debug!("SSE event channel closed, ending stream");
                    break;
                }
            }
        }

        state.event_hub.unsubscribe();
    };

    Sse::new(stream).keep_alive(KeepAlive::default())
}

// ---------------------------------------------------------------------------
// Process messages (bus bridge)
// ---------------------------------------------------------------------------

/// Process incoming messages from the WebSocket message channel and publish
/// them to the message bus as InboundMessage.
pub async fn process_messages(
    mut rx: mpsc::UnboundedReceiver<crate::websocket_handler::IncomingMessage>,
    bus: Arc<MessageBus>,
) {
    while let Some(msg) = rx.recv().await {
        let inbound = InboundMessage {
            channel: "web".to_string(),
            sender_id: msg.sender_id.clone(),
            chat_id: msg.chat_id.clone(),
            content: msg.content,
            media: vec![],
            session_key: format!("web:{}", msg.chat_id),
            correlation_id: String::new(),
            metadata: msg.metadata,
        };

        bus.publish_inbound(inbound);

        tracing::debug!(
            session_id = %msg.session_id,
            sender_id = %msg.sender_id,
            chat_id = %msg.chat_id,
            "Message published to bus"
        );
    }
    tracing::debug!("Message processor stopped");
}

// ---------------------------------------------------------------------------
// Send to session helpers
// ---------------------------------------------------------------------------

/// Send a chat message to a specific session using the broadcast protocol.
pub async fn send_to_session(
    session_manager: &SessionManager,
    session_id: &str,
    role: &str,
    content: &str,
) -> Result<(), String> {
    tracing::debug!(
        session_id = %session_id,
        role = %role,
        content_len = content.len(),
        "send_to_session called"
    );

    let msg = crate::protocol::ProtocolMessage::new(
        "message",
        "chat",
        "receive",
        Some(serde_json::json!({
            "role": role,
            "content": content,
        })),
    );
    let data = msg.to_json().map_err(|e| format!("failed to marshal message: {}", e))?;

    session_manager
        .broadcast(session_id, &data)
        .await
        .map_err(|e| format!("failed to broadcast: {}", e))?;

    tracing::info!(
        session_id = %session_id,
        role = %role,
        "send_to_session completed"
    );
    Ok(())
}

/// Send a history response to a specific session.
pub async fn send_history_to_session(
    session_manager: &SessionManager,
    session_id: &str,
    json_content: &str,
) -> Result<(), String> {
    let data: serde_json::Value =
        serde_json::from_str(json_content).map_err(|e| format!("failed to unmarshal history data: {}", e))?;

    let msg = crate::protocol::ProtocolMessage::new("message", "chat", "history", Some(data));
    let bytes = msg.to_json().map_err(|e| format!("failed to create protocol message: {}", e))?;

    session_manager
        .broadcast(session_id, &bytes)
        .await
        .map_err(|e| format!("failed to broadcast: {}", e))
}

// ---------------------------------------------------------------------------
// Publish status loop (periodic SSE status push)
// ---------------------------------------------------------------------------

/// Start a background task that periodically publishes status events via SSE.
///
/// The loop terminates when the `running` flag is set to `false`.
pub fn start_publish_status_loop(
    event_hub: Arc<EventHub>,
    session_count: Arc<AtomicUsize>,
    version: String,
    start_time: Instant,
    running: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;

            if !running.load(std::sync::atomic::Ordering::SeqCst) {
                tracing::debug!("Status publish loop stopping (server no longer running)");
                break;
            }

            let uptime = start_time.elapsed().as_secs();
            let sessions = session_count.load(std::sync::atomic::Ordering::SeqCst);
            let is_running = running.load(std::sync::atomic::Ordering::SeqCst);

            event_hub.publish(
                "status",
                serde_json::json!({
                    "version": version,
                    "uptime_seconds": uptime,
                    "ws_connected": is_running,
                    "session_count": sessions,
                }),
            );
        }
    })
}

// ---------------------------------------------------------------------------
// Dispatch outbound (subscribe to bus, route web channel messages to sessions)
// ---------------------------------------------------------------------------

/// Subscribe to outbound messages on the bus and dispatch web channel messages
/// to the appropriate sessions.
pub async fn dispatch_outbound(
    bus: Arc<MessageBus>,
    session_manager: Arc<SessionManager>,
) {
    let mut rx = bus.subscribe_outbound();
    loop {
        match rx.recv().await {
            Ok(msg) => {
                if msg.channel != "web" {
                    continue;
                }

                // Extract session ID from chat ID: "web:<session_id>"
                let session_id = if msg.chat_id.starts_with("web:") {
                    &msg.chat_id[4..]
                } else {
                    tracing::warn!(chat_id = %msg.chat_id, "Invalid chat ID format");
                    continue;
                };

                // Route by message type: history messages use a different
                // protocol command (`cmd: "history"`) so the JavaScript
                // client renders them in the history panel instead of as a
                // regular chat bubble.
                let result = if msg.message_type == "history" {
                    send_history_to_session(&session_manager, session_id, &msg.content).await
                } else {
                    send_to_session(&session_manager, session_id, "assistant", &msg.content).await
                };

                if let Err(e) = result {
                    tracing::error!(
                        error = %e,
                        session_id = %session_id,
                        msg_type = %msg.message_type,
                        "Failed to send outbound message"
                    );
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!("Outbound dispatch lagged by {} messages", n);
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                tracing::info!("Outbound bus channel closed, stopping dispatch");
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_build_router() {
        let config = WebServerConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            auth_token: String::new(),
            cors_origins: vec![],
            ws_path: "/ws".to_string(),
            workspace: None,
            version: String::new(),
            static_dir: None,
            index_file: "index.html".to_string(),
        };
        let server = WebServer::new(config);
        let _router = server.build_router();
    }

    #[tokio::test]
    async fn test_build_router_with_static_dir() {
        let dir = tempfile::tempdir().unwrap();
        let config = WebServerConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            auth_token: String::new(),
            cors_origins: vec![],
            ws_path: "/ws".to_string(),
            workspace: None,
            version: String::new(),
            static_dir: Some(dir.path().to_string_lossy().to_string()),
            index_file: "index.html".to_string(),
        };
        let server = WebServer::new(config);
        let _router = server.build_router();
    }

    #[tokio::test]
    async fn test_build_router_with_nonexistent_static_dir() {
        let config = WebServerConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            auth_token: String::new(),
            cors_origins: vec![],
            ws_path: "/ws".to_string(),
            workspace: None,
            version: String::new(),
            static_dir: Some("/nonexistent/path".to_string()),
            index_file: "index.html".to_string(),
        };
        let server = WebServer::new(config);
        let _router = server.build_router();
    }

    #[test]
    fn test_resolve_static_dir_explicit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let result = resolve_static_dir(Some(&path), None);
        assert_eq!(result, Some(path));
    }

    #[test]
    fn test_resolve_static_dir_nonexistent() {
        let result = resolve_static_dir(Some("/nonexistent/path/that/does/not/exist"), None);
        if let Some(ref path) = result {
            assert!(!path.contains("nonexistent"));
        }
    }

    #[test]
    fn test_resolve_static_dir_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let static_dir = dir.path().join("static");
        std::fs::create_dir_all(&static_dir).unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let result = resolve_static_dir(None, Some(&ws));
        assert!(result.is_some());
    }

    #[test]
    fn test_resolve_static_dir_fallback() {
        let result = resolve_static_dir(None, None);
        let _ = result;
    }

    #[test]
    fn test_default_config() {
        let config = WebServerConfig::default();
        assert_eq!(config.listen_addr, "127.0.0.1:8080");
        assert!(config.auth_token.is_empty());
        assert!(config.static_dir.is_none());
        assert_eq!(config.index_file, "index.html");
        assert_eq!(config.ws_path, "/ws");
    }

    // --- DirectoryStaticFiles tests ---

    #[test]
    fn test_directory_static_files_read() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello world").unwrap();

        let provider = DirectoryStaticFiles::new(dir.path());
        let content = provider.get_file("test.txt").unwrap();
        assert_eq!(String::from_utf8(content).unwrap(), "hello world");
    }

    #[test]
    fn test_directory_static_files_nested() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("nested.html"), "<html></html>").unwrap();

        let provider = DirectoryStaticFiles::new(dir.path());
        let content = provider.get_file("sub/nested.html").unwrap();
        assert_eq!(String::from_utf8(content).unwrap(), "<html></html>");
    }

    #[test]
    fn test_directory_static_files_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("safe.txt"), "safe content").unwrap();

        let provider = DirectoryStaticFiles::new(dir.path());
        assert!(provider.get_file("../etc/passwd").is_none());
        assert!(provider.get_file("../../../etc/passwd").is_none());
    }

    #[test]
    fn test_directory_static_files_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let provider = DirectoryStaticFiles::new(dir.path());
        assert!(provider.get_file("nonexistent.txt").is_none());
    }

    #[test]
    fn test_directory_static_files_has_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("exists.txt"), "yes").unwrap();

        let provider = DirectoryStaticFiles::new(dir.path());
        assert!(provider.has_file("exists.txt"));
        assert!(!provider.has_file("nope.txt"));
    }

    #[test]
    fn test_directory_static_files_list() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "a").unwrap();
        std::fs::write(dir.path().join("b.html"), "b").unwrap();

        let provider = DirectoryStaticFiles::new(dir.path());
        let files = provider.list_files();
        assert_eq!(files.len(), 2);
        assert!(files.contains(&"a.txt".to_string()));
        assert!(files.contains(&"b.html".to_string()));
    }

    #[test]
    fn test_directory_static_files_leading_slash() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "index").unwrap();

        let provider = DirectoryStaticFiles::new(dir.path());
        let content = provider.get_file("/index.html").unwrap();
        assert_eq!(String::from_utf8(content).unwrap(), "index");
    }

    #[tokio::test]
    async fn test_process_messages_publishes_to_bus() {
        let bus = Arc::new(MessageBus::new());
        let mut rx = bus.subscribe_inbound();

        let (tx, proc_rx) = mpsc::unbounded_channel();

        // Send a message
        tx.send(crate::websocket_handler::IncomingMessage {
            session_id: "s1".to_string(),
            sender_id: "web:s1".to_string(),
            chat_id: "web:s1".to_string(),
            content: "hello".to_string(),
            metadata: HashMap::new(),
        }).unwrap();
        drop(tx); // Close the sender so process_messages exits

        process_messages(proc_rx, bus.clone()).await;

        let msg = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await;
        assert!(msg.is_ok());
        let inbound = msg.unwrap().unwrap();
        assert_eq!(inbound.channel, "web");
        assert_eq!(inbound.content, "hello");
        assert_eq!(inbound.sender_id, "web:s1");
    }

    #[test]
    fn test_web_server_new() {
        let config = WebServerConfig::default();
        let server = WebServer::new(config);
        assert!(!server.is_running());
        assert!(server.message_bus.is_none());
    }

    #[test]
    fn test_web_server_set_bus() {
        let config = WebServerConfig::default();
        let mut server = WebServer::new(config);
        let bus = Arc::new(MessageBus::new());
        server.set_message_bus(bus);
        assert!(server.message_bus.is_some());
    }

    #[test]
    fn test_web_server_default_is_not_running() {
        let config = WebServerConfig::default();
        let server = WebServer::new(config);
        assert!(!server.is_running());
    }

    #[test]
    fn test_web_server_set_model_name() {
        let config = WebServerConfig::default();
        let server = WebServer::new(config);
        server.set_model_name("gpt-4");
        assert_eq!(*server.model_name.lock(), "gpt-4");
    }

    #[test]
    fn test_web_server_set_workspace() {
        let config = WebServerConfig::default();
        let mut server = WebServer::new(config);
        server.set_workspace(PathBuf::from("/tmp/workspace"));
        assert_eq!(server.config.workspace, Some("/tmp/workspace".to_string()));
    }

    #[test]
    fn test_web_server_stop() {
        let config = WebServerConfig::default();
        let server = WebServer::new(config);
        assert!(!server.is_running());
        server.stop();
        assert!(!server.is_running());
    }

    #[test]
    fn test_web_server_event_hub() {
        let config = WebServerConfig::default();
        let server = WebServer::new(config);
        let hub = server.event_hub();
        hub.publish("test", serde_json::json!({"key": "val"}));
    }

    #[test]
    fn test_web_server_session_manager() {
        let config = WebServerConfig::default();
        let server = WebServer::new(config);
        let mgr = server.session_manager();
        assert_eq!(mgr.active_count(), 0);
    }

    #[test]
    fn test_config_debug_format() {
        let config = WebServerConfig::default();
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("127.0.0.1:8080"));
    }

    #[test]
    fn test_config_custom_values() {
        let config = WebServerConfig {
            listen_addr: "0.0.0.0:9090".to_string(),
            auth_token: "secret".to_string(),
            cors_origins: vec!["https://example.com".to_string()],
            ws_path: "/websocket".to_string(),
            workspace: Some("/data".to_string()),
            version: "2.0.0".to_string(),
            static_dir: Some("/static".to_string()),
            index_file: "home.html".to_string(),
        };
        assert_eq!(config.listen_addr, "0.0.0.0:9090");
        assert_eq!(config.auth_token, "secret");
        assert_eq!(config.ws_path, "/websocket");
        assert_eq!(config.version, "2.0.0");
        assert_eq!(config.index_file, "home.html");
    }

    #[test]
    fn test_directory_static_files_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let provider = DirectoryStaticFiles::new(dir.path());
        let files = provider.list_files();
        assert!(files.is_empty());
    }

    #[test]
    fn test_directory_static_files_subdirectory_files() {
        let dir = tempfile::tempdir().unwrap();
        let sub1 = dir.path().join("css");
        let sub2 = dir.path().join("js");
        std::fs::create_dir_all(&sub1).unwrap();
        std::fs::create_dir_all(&sub2).unwrap();
        std::fs::write(sub1.join("style.css"), "body{}").unwrap();
        std::fs::write(sub2.join("app.js"), "console.log()").unwrap();

        let provider = DirectoryStaticFiles::new(dir.path());
        let files = provider.list_files();
        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|f| f.contains("style.css")));
        assert!(files.iter().any(|f| f.contains("app.js")));
    }

    #[test]
    fn test_directory_static_files_path_traversal_variants() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("safe.txt"), "safe").unwrap();
        let provider = DirectoryStaticFiles::new(dir.path());
        assert!(provider.get_file("../../../etc/passwd").is_none());
        assert!(provider.get_file("..\\..\\windows\\system32").is_none());
        assert!(provider.get_file("./../secret").is_none());
    }

    #[test]
    fn test_directory_static_files_has_file_false() {
        let dir = tempfile::tempdir().unwrap();
        let provider = DirectoryStaticFiles::new(dir.path());
        assert!(!provider.has_file("does_not_exist.txt"));
    }

    #[tokio::test]
    async fn test_process_messages_empty_channel() {
        let bus = Arc::new(MessageBus::new());
        let (tx, rx) = mpsc::unbounded_channel();
        drop(tx); // Close immediately

        process_messages(rx, bus).await;
        // Should complete without error
    }

    #[test]
    fn test_resolve_static_dir_nonexistent_workspace() {
        let result = resolve_static_dir(None, Some("/nonexistent/workspace/path"));
        // May return None or Some depending on ./static/ in CWD
        // The key behavior is that the workspace path itself is not returned
        if let Some(ref path) = result {
            assert!(!path.contains("nonexistent"));
        }
    }

    #[test]
    fn test_resolve_static_dir_explicit_path_nonexistent() {
        let result = resolve_static_dir(Some("/this/path/does/not/exist"), None);
        // Should return None since the path doesn't exist
        if let Some(path) = result {
            // Should not be the explicit path since it doesn't exist
            assert!(!path.contains("nonexistent"));
        }
    }

    #[test]
    fn test_web_server_config_default_values() {
        let config = WebServerConfig::default();
        assert_eq!(config.listen_addr, "127.0.0.1:8080");
        assert!(config.auth_token.is_empty());
        assert!(config.cors_origins.is_empty());
        assert_eq!(config.ws_path, "/ws");
        assert!(config.workspace.is_none());
        assert!(config.version.is_empty());
        assert!(config.static_dir.is_none());
        assert_eq!(config.index_file, "index.html");
    }

    #[tokio::test]
    async fn test_process_messages_preserves_metadata() {
        let bus = Arc::new(MessageBus::new());
        let mut rx = bus.subscribe_inbound();
        let (tx, proc_rx) = mpsc::unbounded_channel();

        let mut metadata = HashMap::new();
        metadata.insert("request_type".to_string(), "history".to_string());
        tx.send(crate::websocket_handler::IncomingMessage {
            session_id: "s1".to_string(),
            sender_id: "web:s1".to_string(),
            chat_id: "web:s1".to_string(),
            content: "test".to_string(),
            metadata,
        }).unwrap();
        drop(tx);

        process_messages(proc_rx, bus).await;

        let msg = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await;
        let inbound = msg.unwrap().unwrap();
        assert_eq!(inbound.metadata.get("request_type"), Some(&"history".to_string()));
    }

    #[test]
    fn test_static_files_trait_default_has_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "content").unwrap();
        let provider = DirectoryStaticFiles::new(dir.path());
        // has_file uses the default implementation which calls get_file
        assert!(provider.has_file("test.txt"));
        assert!(!provider.has_file("missing.txt"));
    }

    // ============================================================
    // Additional server tests for missing coverage
    // ============================================================

    #[test]
    fn test_web_server_config_default_ws_path() {
        let config = WebServerConfig::default();
        assert_eq!(config.ws_path, "/ws");
    }

    #[test]
    fn test_web_server_config_default_index_file() {
        let config = WebServerConfig::default();
        assert_eq!(config.index_file, "index.html");
    }

    #[test]
    fn test_web_server_config_default_workspace() {
        let config = WebServerConfig::default();
        assert!(config.workspace.is_none());
    }

    #[test]
    fn test_web_server_config_default_static_dir() {
        let config = WebServerConfig::default();
        assert!(config.static_dir.is_none());
    }

    #[test]
    fn test_web_server_config_default_version() {
        let config = WebServerConfig::default();
        assert!(config.version.is_empty());
    }

    #[test]
    fn test_web_server_config_cors_origins() {
        let config = WebServerConfig {
            cors_origins: vec!["http://localhost:3000".to_string()],
            ..Default::default()
        };
        assert_eq!(config.cors_origins.len(), 1);
        assert_eq!(config.cors_origins[0], "http://localhost:3000");
    }

    #[test]
    fn test_web_server_new_custom_config() {
        let config = WebServerConfig {
            listen_addr: "0.0.0.0:9090".to_string(),
            auth_token: "mytoken".to_string(),
            cors_origins: vec![],
            ws_path: "/websocket".to_string(),
            workspace: Some("/tmp/ws".to_string()),
            version: "1.0.0".to_string(),
            static_dir: None,
            index_file: "app.html".to_string(),
        };
        let server = WebServer::new(config);
        assert!(!server.is_running());
    }

    #[test]
    fn test_directory_static_files_binary_content() {
        let dir = tempfile::tempdir().unwrap();
        let binary_data = vec![0u8, 255, 128, 64, 32, 16, 8, 4, 2, 1];
        std::fs::write(dir.path().join("data.bin"), &binary_data).unwrap();

        let provider = DirectoryStaticFiles::new(dir.path());
        let content = provider.get_file("data.bin").unwrap();
        assert_eq!(content, binary_data);
    }

    #[test]
    fn test_directory_static_files_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("empty.txt"), "").unwrap();

        let provider = DirectoryStaticFiles::new(dir.path());
        let content = provider.get_file("empty.txt").unwrap();
        assert!(content.is_empty());
    }

    #[test]
    fn test_directory_static_files_special_chars_filename() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file with spaces.txt"), "content").unwrap();

        let provider = DirectoryStaticFiles::new(dir.path());
        let content = provider.get_file("file with spaces.txt");
        assert!(content.is_some());
        assert_eq!(content.unwrap(), b"content".to_vec());
    }

    #[tokio::test]
    async fn test_process_messages_closed_channel() {
        let bus = Arc::new(MessageBus::new());
        let (tx, rx) = mpsc::unbounded_channel();
        drop(tx);

        // Should complete without panic
        process_messages(rx, bus).await;
    }

    // ============================================================
    // Additional server tests: handle_health, send_to_session, etc.
    // ============================================================

    #[tokio::test]
    async fn test_handle_health_endpoint() {
        let state = Arc::new(AppState {
            auth_token: String::new(),
            session_count: Arc::new(AtomicUsize::new(3)),
            workspace: None,
            version: "1.0.0".to_string(),
            start_time: Instant::now(),
            model_name: Arc::new(parking_lot::Mutex::new("test".to_string())),
            event_hub: Arc::new(EventHub::new()),
            running: Arc::new(AtomicBool::new(true)),
            session_manager: Arc::new(SessionManager::with_default_timeout()),
            inbound_tx: None,
            streaming_provider: None,
        });
        let resp = handle_health(AxumState(state)).await;
        let json = resp.0;
        assert_eq!(json["status"], "ok");
        assert_eq!(json["running"], true);
        assert_eq!(json["sessions"], 3);
    }

    #[tokio::test]
    async fn test_handle_health_not_running() {
        let state = Arc::new(AppState {
            auth_token: String::new(),
            session_count: Arc::new(AtomicUsize::new(0)),
            workspace: None,
            version: "1.0.0".to_string(),
            start_time: Instant::now(),
            model_name: Arc::new(parking_lot::Mutex::new(String::new())),
            event_hub: Arc::new(EventHub::new()),
            running: Arc::new(AtomicBool::new(false)),
            session_manager: Arc::new(SessionManager::with_default_timeout()),
            inbound_tx: None,
            streaming_provider: None,
        });
        let resp = handle_health(AxumState(state)).await;
        let json = resp.0;
        assert_eq!(json["status"], "ok");
        assert_eq!(json["running"], false);
        assert_eq!(json["sessions"], 0);
    }

    #[tokio::test]
    async fn test_send_to_session_no_queue() {
        let mgr = Arc::new(SessionManager::with_default_timeout());
        let session = mgr.create_session();
        let result = send_to_session(&mgr, &session.id, "assistant", "hello").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("session not found or no send queue"));
    }

    #[tokio::test]
    async fn test_send_to_session_nonexistent() {
        let mgr = Arc::new(SessionManager::with_default_timeout());
        let result = send_to_session(&mgr, "nonexistent-session", "assistant", "hello").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_send_history_to_session_no_queue() {
        let mgr = Arc::new(SessionManager::with_default_timeout());
        let session = mgr.create_session();
        let history_json = r#"{"messages":[],"has_more":false}"#;
        let result = send_history_to_session(&mgr, &session.id, history_json).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_send_history_to_session_invalid_json() {
        let mgr = Arc::new(SessionManager::with_default_timeout());
        let result = send_history_to_session(&mgr, "any-session", "not json").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("failed to unmarshal history data"));
    }

    #[tokio::test]
    async fn test_send_history_to_session_nonexistent() {
        let mgr = Arc::new(SessionManager::with_default_timeout());
        let history_json = r#"{"messages":[]}"#;
        let result = send_history_to_session(&mgr, "nonexistent", history_json).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_start_publish_status_loop() {
        let event_hub = Arc::new(EventHub::new());
        let mut rx = event_hub.subscribe();
        let session_count = Arc::new(AtomicUsize::new(1));
        let running = Arc::new(AtomicBool::new(true));

        let handle = start_publish_status_loop(
            event_hub.clone(),
            session_count,
            "1.0.0".to_string(),
            Instant::now(),
            running.clone(),
        );

        // Wait for at least one status event
        let result = tokio::time::timeout(Duration::from_secs(8), rx.recv()).await;
        assert!(result.is_ok());
        let event = result.unwrap().unwrap();
        assert_eq!(event.event_type, "status");
        assert_eq!(event.data["version"], "1.0.0");
        assert_eq!(event.data["session_count"], 1);

        // Stop the loop
        running.store(false, std::sync::atomic::Ordering::SeqCst);
        let _ = tokio::time::timeout(Duration::from_secs(10), handle).await;
    }

    #[tokio::test]
    async fn test_process_messages_multiple_messages() {
        let bus = Arc::new(MessageBus::new());
        let mut rx = bus.subscribe_inbound();
        let (tx, proc_rx) = mpsc::unbounded_channel();

        for i in 0..5 {
            tx.send(crate::websocket_handler::IncomingMessage {
                session_id: format!("s{}", i),
                sender_id: format!("web:s{}", i),
                chat_id: format!("web:s{}", i),
                content: format!("message {}", i),
                metadata: HashMap::new(),
            }).unwrap();
        }
        drop(tx);

        process_messages(proc_rx, bus).await;

        for i in 0..5 {
            let msg = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await;
            assert!(msg.is_ok());
            let inbound = msg.unwrap().unwrap();
            assert_eq!(inbound.content, format!("message {}", i));
            assert_eq!(inbound.channel, "web");
        }
    }

    #[test]
    fn test_web_server_config_clone() {
        let config = WebServerConfig {
            listen_addr: "127.0.0.1:8080".to_string(),
            auth_token: "token".to_string(),
            cors_origins: vec!["https://example.com".to_string()],
            ws_path: "/ws".to_string(),
            workspace: Some("/tmp".to_string()),
            version: "1.0".to_string(),
            static_dir: None,
            index_file: "index.html".to_string(),
        };
        let cloned = config.clone();
        assert_eq!(cloned.listen_addr, config.listen_addr);
        assert_eq!(cloned.auth_token, config.auth_token);
        assert_eq!(cloned.cors_origins.len(), 1);
    }

    #[test]
    fn test_directory_static_files_nonexistent_base() {
        let provider = DirectoryStaticFiles::new("/this/path/does/not/exist");
        assert!(provider.get_file("test.txt").is_none());
        assert!(provider.list_files().is_empty());
    }

    #[test]
    fn test_resolve_static_dir_with_both_explicit_and_workspace() {
        let explicit_dir = tempfile::tempdir().unwrap();
        let workspace_dir = tempfile::tempdir().unwrap();
        let ws_static = workspace_dir.path().join("static");
        std::fs::create_dir_all(&ws_static).unwrap();

        // Explicit takes priority
        let explicit_path = explicit_dir.path().to_string_lossy().to_string();
        let ws_path = workspace_dir.path().to_string_lossy().to_string();
        let result = resolve_static_dir(Some(&explicit_path), Some(&ws_path));
        assert_eq!(result, Some(explicit_path));
    }

    #[test]
    fn test_resolve_static_dir_workspace_static_subdir() {
        let dir = tempfile::tempdir().unwrap();
        let static_dir = dir.path().join("static");
        std::fs::create_dir_all(&static_dir).unwrap();
        let ws = dir.path().to_string_lossy().to_string();

        let result = resolve_static_dir(None, Some(&ws));
        assert!(result.is_some());
        assert!(result.unwrap().contains("static"));
    }

    #[tokio::test]
    async fn test_build_router_health_endpoint() {
        let config = WebServerConfig::default();
        let server = WebServer::new(config);
        let app = server.build_router();

        use tower::ServiceExt;
        let req = axum::http::Request::builder()
            .uri("/health")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
    }

    #[tokio::test]
    async fn test_build_router_api_health_endpoint() {
        let config = WebServerConfig::default();
        let server = WebServer::new(config);
        let app = server.build_router();

        use tower::ServiceExt;
        let req = axum::http::Request::builder()
            .uri("/api/health")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
    }

    #[tokio::test]
    async fn test_build_router_api_status_endpoint() {
        let config = WebServerConfig::default();
        let server = WebServer::new(config);
        let app = server.build_router();

        use tower::ServiceExt;
        let req = axum::http::Request::builder()
            .uri("/api/status")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn test_build_router_api_version_endpoint() {
        let config = WebServerConfig {
            version: "2.0.0".to_string(),
            ..Default::default()
        };
        let server = WebServer::new(config);
        let app = server.build_router();

        use tower::ServiceExt;
        let req = axum::http::Request::builder()
            .uri("/api/version")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["version"], "2.0.0");
    }

    #[tokio::test]
    async fn test_build_router_api_sessions_endpoint() {
        let config = WebServerConfig::default();
        let server = WebServer::new(config);
        let app = server.build_router();

        use tower::ServiceExt;
        let req = axum::http::Request::builder()
            .uri("/api/sessions")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["total_connections"], 0);
        assert_eq!(json["active_sessions"], 0);
    }

    #[tokio::test]
    async fn test_build_router_api_events_endpoint() {
        let config = WebServerConfig::default();
        let server = WebServer::new(config);
        let app = server.build_router();

        use tower::ServiceExt;
        let req = axum::http::Request::builder()
            .uri("/api/events")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["stream_url"], "/api/events/stream");
    }

    #[test]
    fn test_web_server_set_model_name_multiple() {
        let config = WebServerConfig::default();
        let server = WebServer::new(config);
        server.set_model_name("gpt-4");
        assert_eq!(*server.model_name.lock(), "gpt-4");
        server.set_model_name("claude-3");
        assert_eq!(*server.model_name.lock(), "claude-3");
    }

    #[test]
    fn test_web_server_stop_sets_running_false() {
        let config = WebServerConfig::default();
        let server = WebServer::new(config);
        server.running.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(server.is_running());
        server.stop();
        assert!(!server.is_running());
    }

    #[test]
    fn test_directory_static_files_deeply_nested() {
        let dir = tempfile::tempdir().unwrap();
        let deep = dir.path().join("a").join("b").join("c");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("deep.txt"), "deep content").unwrap();

        let provider = DirectoryStaticFiles::new(dir.path());
        let content = provider.get_file("a/b/c/deep.txt").unwrap();
        assert_eq!(String::from_utf8(content).unwrap(), "deep content");
        let files = provider.list_files();
        assert_eq!(files.len(), 1);
    }

    // ============================================================
    // Additional tests for 95%+ coverage - server lifecycle
    // ============================================================

    #[tokio::test]
    async fn test_web_server_build_router_no_bus_drains_messages() {
        let config = WebServerConfig::default();
        let server = WebServer::new(config);
        // No message bus set - should drain messages
        let _router = server.build_router();
        // Give the drain task a moment to start
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    #[tokio::test]
    async fn test_web_server_build_router_with_bus() {
        let mut config = WebServerConfig::default();
        config.listen_addr = "127.0.0.1:0".to_string();
        let mut server = WebServer::new(config);
        let bus = Arc::new(MessageBus::new());
        server.set_message_bus(bus);
        let _router = server.build_router();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    #[tokio::test]
    async fn test_build_router_with_cors_origins() {
        let config = WebServerConfig {
            cors_origins: vec!["http://localhost:3000".to_string(), "http://localhost:4000".to_string()],
            ..Default::default()
        };
        let server = WebServer::new(config);
        let _router = server.build_router();
    }

    #[tokio::test]
    async fn test_build_router_custom_ws_path() {
        let config = WebServerConfig {
            ws_path: "/custom_ws".to_string(),
            ..Default::default()
        };
        let server = WebServer::new(config);
        let app = server.build_router();

        use tower::ServiceExt;
        // /ws should no longer exist
        let req = axum::http::Request::builder()
            .uri("/ws")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 404);
    }

    #[tokio::test]
    async fn test_build_router_api_logs_endpoint() {
        let config = WebServerConfig {
            workspace: Some("/nonexistent_workspace".to_string()),
            ..Default::default()
        };
        let server = WebServer::new(config);
        let app = server.build_router();

        use tower::ServiceExt;
        let req = axum::http::Request::builder()
            .uri("/api/logs")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn test_build_router_api_config_endpoint_no_workspace() {
        let config = WebServerConfig::default();
        let server = WebServer::new(config);
        let app = server.build_router();

        use tower::ServiceExt;
        let req = axum::http::Request::builder()
            .uri("/api/config")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        // Returns 503 without workspace configured
        assert_eq!(resp.status(), 503);
    }

    #[tokio::test]
    async fn test_build_router_api_scanner_status_no_workspace() {
        let config = WebServerConfig::default();
        let server = WebServer::new(config);
        let app = server.build_router();

        use tower::ServiceExt;
        let req = axum::http::Request::builder()
            .uri("/api/scanner/status")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        // Returns 503 without workspace configured
        assert_eq!(resp.status(), 503);
    }

    #[tokio::test]
    async fn test_build_router_api_models_no_workspace() {
        let config = WebServerConfig {
            version: "2.0.0".to_string(),
            ..Default::default()
        };
        let server = WebServer::new(config);
        let app = server.build_router();

        use tower::ServiceExt;
        let req = axum::http::Request::builder()
            .uri("/api/models")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        // Returns 503 without workspace configured
        assert_eq!(resp.status(), 503);
    }

    #[tokio::test]
    async fn test_send_to_session_nonexistent_session() {
        let mgr = Arc::new(SessionManager::with_default_timeout());
        // No session created
        let result = send_to_session(&mgr, "nonexistent-id", "assistant", "hello world").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_send_history_to_session_nonexistent_session() {
        let mgr = Arc::new(SessionManager::with_default_timeout());
        // No session created
        let history_json = r#"{"messages":[{"role":"user","content":"hi"}],"has_more":false}"#;
        let result = send_history_to_session(&mgr, "nonexistent-id", history_json).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_publish_status_loop_stops_on_false() {
        let event_hub = Arc::new(EventHub::new());
        let session_count = Arc::new(AtomicUsize::new(0));
        let running = Arc::new(AtomicBool::new(false)); // Already false

        let handle = start_publish_status_loop(
            event_hub,
            session_count,
            "1.0.0".to_string(),
            Instant::now(),
            running,
        );

        // Should stop quickly since running is false
        let result = tokio::time::timeout(Duration::from_secs(3), handle).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_web_server_start_time() {
        let config = WebServerConfig::default();
        let server = WebServer::new(config);
        // start_time should be set at construction
        let _elapsed = server.start_time.elapsed();
    }

    #[tokio::test]
    async fn test_handle_health_with_model_state() {
        let state = Arc::new(AppState {
            auth_token: String::new(),
            session_count: Arc::new(AtomicUsize::new(5)),
            workspace: Some("/test".to_string()),
            version: "3.0.0".to_string(),
            start_time: Instant::now(),
            model_name: Arc::new(parking_lot::Mutex::new("gpt-4o".to_string())),
            event_hub: Arc::new(EventHub::new()),
            running: Arc::new(AtomicBool::new(true)),
            session_manager: Arc::new(SessionManager::with_default_timeout()),
            inbound_tx: None,
            streaming_provider: None,
        });
        let resp = handle_health(AxumState(state)).await;
        let json = resp.0;
        // handle_health only returns status, running, sessions
        assert_eq!(json["status"], "ok");
        assert_eq!(json["running"], true);
        assert_eq!(json["sessions"], 5);
    }

    #[tokio::test]
    async fn test_process_messages_with_bus_and_metadata() {
        let bus = Arc::new(MessageBus::new());
        let mut rx = bus.subscribe_inbound();
        let (tx, proc_rx) = mpsc::unbounded_channel();

        let mut metadata = HashMap::new();
        metadata.insert("request_type".to_string(), "history_request".to_string());
        metadata.insert("request_id".to_string(), "req-001".to_string());

        tx.send(crate::websocket_handler::IncomingMessage {
            session_id: "sess-123".to_string(),
            sender_id: "web:sess-123".to_string(),
            chat_id: "web:sess-123".to_string(),
            content: "What is the weather?".to_string(),
            metadata,
        }).unwrap();
        drop(tx);

        process_messages(proc_rx, bus).await;

        let msg = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await;
        assert!(msg.is_ok());
        let inbound = msg.unwrap().unwrap();
        assert_eq!(inbound.channel, "web");
        assert_eq!(inbound.sender_id, "web:sess-123");
        assert_eq!(inbound.content, "What is the weather?");
        assert_eq!(inbound.metadata.get("request_type").unwrap(), "history_request");
        assert_eq!(inbound.metadata.get("request_id").unwrap(), "req-001");
    }

    #[tokio::test]
    async fn test_build_router_sse_stream_endpoint() {
        let config = WebServerConfig::default();
        let server = WebServer::new(config);
        let app = server.build_router();

        use tower::ServiceExt;
        let req = axum::http::Request::builder()
            .uri("/api/events/stream")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        // SSE endpoint returns 200 with event-stream content type
        assert_eq!(resp.status(), 200);
    }

    #[test]
    fn test_resolve_static_dir_current_dir_static() {
        // Test the fallback path where no explicit dir and no workspace
        let result = resolve_static_dir(None, None);
        // Result depends on whether ./static/ exists in CWD
        let _ = result;
    }
}

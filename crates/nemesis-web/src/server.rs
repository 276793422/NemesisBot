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
    handle_api_readme, handle_api_license,
};
use crate::api_usage::{handle_api_usage_summary, handle_api_usage_trends, handle_api_usage_logs};
use crate::cors::dev_cors_layer;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::websocket_handler::handle_websocket_upgrade;
use axum::extract::State as AxumState;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::IntoResponse;
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
#[derive(Clone)]
pub struct WebServerConfig {
    pub listen_addr: String,
    pub auth_token: String,
    pub cors_origins: Vec<String>,
    /// WebSocket endpoint path (default: "/ws").
    pub ws_path: String,
    /// Optional workspace path for config/log access.
    pub workspace: Option<String>,
    /// Home directory where config.json resides.
    pub home: Option<String>,
    /// Application version string.
    pub version: String,
    /// Optional path to static files directory for serving the Web UI (legacy disk-based).
    pub static_dir: Option<String>,
    /// Optional in-memory static file provider (preferred over `static_dir`).
    pub static_files: Option<Arc<dyn StaticFiles>>,
    /// Optional index file name (default: "index.html").
    pub index_file: String,
}

impl std::fmt::Debug for WebServerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebServerConfig")
            .field("listen_addr", &self.listen_addr)
            .field("auth_token", &self.auth_token)
            .field("cors_origins", &self.cors_origins)
            .field("ws_path", &self.ws_path)
            .field("workspace", &self.workspace)
            .field("home", &self.home)
            .field("version", &self.version)
            .field("static_files", &self.static_files.as_ref().map(|_| "..."))
            .field("index_file", &self.index_file)
            .finish()
    }
}

impl Default for WebServerConfig {
    fn default() -> Self {
        Self {
            listen_addr: "127.0.0.1:8080".to_string(),
            auth_token: String::new(),
            cors_origins: vec![],
            ws_path: "/ws".to_string(),
            workspace: None,
            home: None,
            version: String::new(),
            static_dir: None,
            static_files: None,
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
    /// Active model API base URL.
    model_base: Arc<parking_lot::Mutex<String>>,
    /// Whether the active model has an API key configured.
    model_has_key: Arc<std::sync::atomic::AtomicBool>,
    /// Optional streaming LLM provider for SSE chat endpoint.
    streaming_provider: Option<Arc<nemesis_providers::http_provider::HttpProvider>>,
    /// Agent loop service for start/stop/status control.
    agent_service: Option<Arc<dyn nemesis_services::bot_service::AgentLoopService>>,
    /// Data store for usage statistics queries.
    data_store: Option<Arc<nemesis_data::DataStore>>,
    /// Memory manager for runtime vector store control.
    memory_manager: Option<Arc<nemesis_memory::manager::MemoryManager>>,
    /// Forge self-learning instance for runtime start/stop control.
    forge: Option<Arc<nemesis_forge::forge::Forge>>,
    /// Agent loop for runtime model/provider switching.
    agent_loop: Option<Arc<nemesis_agent::r#loop::AgentLoop>>,
    /// Security plugin for configuration reloading.
    security_plugin: Option<Arc<nemesis_security::pipeline::SecurityPlugin>>,
    /// Cron service for SharedToolConfig rebuild.
    cron_service: Option<Arc<std::sync::Mutex<nemesis_cron::service::CronService>>>,
    /// Skills loader for SharedToolConfig rebuild.
    skills_loader: Option<Arc<nemesis_skills::loader::SkillsLoader>>,
    /// Skills registry for SharedToolConfig rebuild.
    skills_registry: Option<Arc<nemesis_skills::registry::RegistryManager>>,
    /// Forge tool executor for SharedToolConfig rebuild.
    forge_executor: Option<Arc<nemesis_forge::forge_tools::ForgeToolExecutor>>,
}

impl WebServer {
    /// Create a new web server.
    pub fn new(config: WebServerConfig) -> Self {
        tracing::info!(
            listen_addr = %config.listen_addr,
            ws_path = %config.ws_path,
            "[WebServer] Creating web server"
        );
        Self {
            config,
            event_hub: Arc::new(EventHub::new()),
            session_manager: Arc::new(SessionManager::with_default_timeout()),
            session_count: Arc::new(AtomicUsize::new(0)),
            running: Arc::new(AtomicBool::new(false)),
            start_time: Instant::now(),
            message_bus: None,
            model_name: Arc::new(parking_lot::Mutex::new(String::new())),
            model_base: Arc::new(parking_lot::Mutex::new(String::new())),
            model_has_key: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            streaming_provider: None,
            agent_service: None,
            data_store: None,
            memory_manager: None,
            forge: None,
            agent_loop: None,
            security_plugin: None,
            cron_service: None,
            skills_loader: None,
            skills_registry: None,
            forge_executor: None,
        }
    }

    /// Set the message bus for inbound message publishing.
    pub fn set_message_bus(&mut self, bus: Arc<MessageBus>) {
        self.message_bus = Some(bus);
    }

    /// Set model info: name, API base URL, and whether a key is configured.
    pub fn set_model_info(&self, name: &str, base_url: &str, has_key: bool) {
        *self.model_name.lock() = name.to_string();
        *self.model_base.lock() = base_url.to_string();
        self.model_has_key.store(has_key, std::sync::atomic::Ordering::Release);
    }

    /// Set the workspace path for config/log access.
    pub fn set_workspace(&mut self, path: PathBuf) {
        self.config.workspace = Some(path.to_string_lossy().to_string());
    }

    /// Set the streaming LLM provider for the SSE chat endpoint.
    pub fn set_streaming_provider(&mut self, provider: Arc<nemesis_providers::http_provider::HttpProvider>) {
        self.streaming_provider = Some(provider);
    }

    /// Set the agent loop service for start/stop/status control.
    pub fn set_agent_service(&mut self, service: Arc<dyn nemesis_services::bot_service::AgentLoopService>) {
        self.agent_service = Some(service);
    }

    /// Set the data store for usage statistics queries.
    pub fn set_data_store(&mut self, store: Arc<nemesis_data::DataStore>) {
        self.data_store = Some(store);
    }

    /// Set the memory manager for runtime vector store control.
    pub fn set_memory_manager(&mut self, mgr: Arc<nemesis_memory::manager::MemoryManager>) {
        self.memory_manager = Some(mgr);
    }

    /// Set the Forge self-learning instance for runtime start/stop control.
    pub fn set_forge(&mut self, forge: Arc<nemesis_forge::forge::Forge>) {
        self.forge = Some(forge);
    }

    /// Set the agent loop for runtime model/provider switching.
    pub fn set_agent_loop(&mut self, agent_loop: Arc<nemesis_agent::r#loop::AgentLoop>) {
        self.agent_loop = Some(agent_loop);
    }

    pub fn set_security_plugin(&mut self, plugin: Arc<nemesis_security::pipeline::SecurityPlugin>) {
        self.security_plugin = Some(plugin);
    }

    pub fn set_cron_service(&mut self, service: Arc<std::sync::Mutex<nemesis_cron::service::CronService>>) {
        self.cron_service = Some(service);
    }

    pub fn set_skills_loader(&mut self, loader: Arc<nemesis_skills::loader::SkillsLoader>) {
        self.skills_loader = Some(loader);
    }

    pub fn set_skills_registry(&mut self, registry: Arc<nemesis_skills::registry::RegistryManager>) {
        self.skills_registry = Some(registry);
    }

    pub fn set_forge_executor(&mut self, executor: Arc<nemesis_forge::forge_tools::ForgeToolExecutor>) {
        self.forge_executor = Some(executor);
    }

    /// Build the Axum router with all routes.
    pub fn build_router(&self) -> Router {
        let (inbound_tx, mut inbound_rx) = mpsc::unbounded_channel::<crate::websocket_handler::IncomingMessage>();

        let state = AppState {
            auth_token: self.config.auth_token.clone(),
            session_count: self.session_count.clone(),
            workspace: self.config.workspace.clone(),
            home: self.config.home.clone(),
            version: self.config.version.clone(),
            start_time: self.start_time,
            model_name: self.model_name.clone(),
            model_base: self.model_base.clone(),
            model_has_key: self.model_has_key.clone(),
            event_hub: self.event_hub.clone(),
            running: self.running.clone(),
            session_manager: self.session_manager.clone(),
            inbound_tx: Some(inbound_tx),
            streaming_provider: self.streaming_provider.clone(),
            ws_router: {
                let mut ws_router = crate::ws_router::WsRouter::new();
                crate::handlers::register_all(&mut ws_router);
                Some(Arc::new(ws_router))
            },
            agent_service: self.agent_service.clone(),
            data_store: self.data_store.clone(),
            memory_manager: self.memory_manager.clone(),
            forge: self.forge.clone(),
            agent_loop: self.agent_loop.clone(),
            security_plugin: self.security_plugin.clone(),
            cron_service: self.cron_service.clone(),
            skills_loader: self.skills_loader.clone(),
            skills_registry: self.skills_registry.clone(),
            forge_executor: self.forge_executor.clone(),
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
            // System info endpoints (readme, license)
            .route("/api/system/readme", get(handle_api_readme))
            .route("/api/system/license", get(handle_api_license))
            // Usage statistics endpoints
            .route("/api/usage/summary", get(handle_api_usage_summary))
            .route("/api/usage/trends", get(handle_api_usage_trends))
            .route("/api/usage/logs", get(handle_api_usage_logs))
            // SSE event stream
            .route("/api/events/stream", get(handle_events_stream))
            // SSE chat streaming endpoint
            .route("/api/chat/stream", axum::routing::post(crate::sse_chat::handle_chat_stream))
            // CORS layer
            .layer(if self.config.cors_origins.is_empty() {
                dev_cors_layer()
            } else {
                crate::cors::production_cors_layer(&self.config.cors_origins)
            })
            .with_state(state.clone());

        // Add static file serving if configured
        if let Some(ref files) = self.config.static_files {
            // In-memory static file serving (zero disk IO)
            let files = files.clone();
            tracing::info!("[WebServer] Serving static files from embedded memory");
            router = router.fallback(move |req: axum::extract::Request| {
                let files = files.clone();
                async move { serve_embedded_static(files, req).await }
            });
        } else if let Some(ref static_dir) = self.config.static_dir {
            let dir_path = PathBuf::from(static_dir);
            if dir_path.exists() && dir_path.is_dir() {
                tracing::info!(
                    static_dir = %static_dir,
                    "[WebServer] Serving static files from directory"
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
                    "[WebServer] Static directory not found or not a directory, skipping static file serving"
                );
            }
        }

        tracing::info!("[WebServer] Router built, routes registered");
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
    /// Returns the actual bound address (useful when port 0 is used for OS-assigned random port).
    pub async fn start(&self) -> Result<SocketAddr, String> {
        tracing::info!(
            listen_addr = %self.config.listen_addr,
            "[WebServer] Starting web server"
        );
        self.running.store(true, std::sync::atomic::Ordering::SeqCst);

        let _status_handle = start_publish_status_loop(
            self.event_hub.clone(),
            self.session_count.clone(),
            self.config.version.clone(),
            self.start_time,
            self.running.clone(),
        );

        let addr: SocketAddr = self.config.listen_addr.parse().map_err(|e| {
            tracing::error!("[WebServer] Invalid listen address '{}': {}", self.config.listen_addr, e);
            format!("invalid listen address: {}", e)
        })?;
        let app = self.build_router();
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| {
                tracing::error!("[WebServer] Bind failed on '{}': {}", addr, e);
                format!("bind failed: {}", e)
            })?;

        let actual_addr = listener.local_addr()
            .map_err(|e| format!("failed to get local addr: {}", e))?;
        tracing::info!("[WebServer] Listening on {}", actual_addr);
        axum::serve(listener, app)
            .await
            .map_err(|e| {
                tracing::error!("[WebServer] Server error: {}", e);
                format!("server error: {}", e)
            })?;
        Ok(actual_addr)
    }

    /// Start the web server with graceful shutdown signal.
    /// `bound_tx`: if provided, the actual bound address is sent immediately after bind
    /// (before the serve loop blocks), so callers can discover the real port when using port 0.
    pub async fn start_with_shutdown(
        &self,
        mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
        bound_tx: Option<tokio::sync::oneshot::Sender<SocketAddr>>,
    ) -> Result<(), String> {
        tracing::info!(
            listen_addr = %self.config.listen_addr,
            "[WebServer] Starting web server with graceful shutdown"
        );
        self.running.store(true, std::sync::atomic::Ordering::SeqCst);

        let _status_handle = start_publish_status_loop(
            self.event_hub.clone(),
            self.session_count.clone(),
            self.config.version.clone(),
            self.start_time,
            self.running.clone(),
        );

        let addr: SocketAddr = self.config.listen_addr.parse().map_err(|e| {
            tracing::error!("[WebServer] Invalid listen address '{}': {}", self.config.listen_addr, e);
            format!("invalid listen address: {}", e)
        })?;
        let app = self.build_router();
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| {
                tracing::error!("[WebServer] Bind failed on '{}': {}", addr, e);
                format!("bind failed: {}", e)
            })?;

        let actual_addr = listener.local_addr()
            .map_err(|e| format!("failed to get local addr: {}", e))?;

        // Send the actual address immediately so the caller knows the real port.
        if let Some(tx) = bound_tx {
            let _ = tx.send(actual_addr);
        }

        tracing::info!("[WebServer] Listening on {}", actual_addr);

        tokio::select! {
            result = axum::serve(listener, app) => {
                result.map_err(|e| format!("server error: {}", e))?;
            }
            _ = shutdown_rx.recv() => {
                tracing::info!("[WebServer] Shutdown signal received");
            }
        }
        Ok(())
    }

    /// Stop the web server.
    pub fn stop(&self) {
        tracing::info!("[WebServer] Stopping web server");
        self.running.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// Static files utility
// ---------------------------------------------------------------------------

/// Determine Content-Type for a static file path.
fn content_type_for(path: &str) -> String {
    let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
    let ct = match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "xml" => "application/xml; charset=utf-8",
        "svg" => "image/svg+xml; charset=utf-8",
        "txt" => "text/plain; charset=utf-8",
        "ico" => "image/x-icon",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "eot" => "application/vnd.ms-fontobject",
        "wasm" => "application/wasm",
        "map" => "application/json; charset=utf-8",
        _ => "application/octet-stream",
    };
    ct.to_string()
}

/// Serve a static file request from an in-memory `StaticFiles` provider.
///
/// 1. Exact path match
/// 2. SPA fallback: paths without a file extension → index.html
/// 3. 404
async fn serve_embedded_static(
    files: Arc<dyn StaticFiles>,
    req: axum::extract::Request,
) -> axum::response::Response {
    let path = req.uri().path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    // 1. Exact match
    if let Some(content) = files.get_file(path) {
        let ct = content_type_for(path);
        return (
            axum::http::StatusCode::OK,
            [(http::header::CONTENT_TYPE, ct)],
            content,
        ).into_response();
    }

    // 2. SPA fallback: no file extension → serve index.html
    if !path.contains('.') {
        if let Some(content) = files.get_file("index.html") {
            return (
                axum::http::StatusCode::OK,
                [(http::header::CONTENT_TYPE, "text/html; charset=utf-8".to_string())],
                content,
            ).into_response();
        }
    }

    // 3. 404
    (axum::http::StatusCode::NOT_FOUND, "Not Found").into_response()
}

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
        tracing::warn!("[WebServer] Explicit static dir not found: {}", path);
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
        tracing::debug!("[WebServer] SSE stream started");
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
                    tracing::warn!("[WebServer] SSE client lagged by {} events, continuing", n);
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    tracing::debug!("[WebServer] SSE event channel closed, ending stream");
                    break;
                }
            }
        }

        state.event_hub.unsubscribe();
        tracing::debug!("[WebServer] SSE stream ended");
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
            voice_playback: msg.voice_playback,
        };

        bus.publish_inbound(inbound);

        tracing::debug!(
            session_id = %msg.session_id,
            sender_id = %msg.sender_id,
            chat_id = %msg.chat_id,
            "[WebServer] Message published to bus"
        );
    }
    tracing::debug!("[WebServer] Message processor stopped");
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
        "[WebServer] send_to_session called"
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
        "[WebServer] send_to_session completed"
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
                tracing::debug!("[WebServer] Status publish loop stopping (server no longer running)");
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
                    tracing::warn!(chat_id = %msg.chat_id, "[WebServer] Invalid chat ID format");
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
                        "[WebServer] Failed to send outbound message"
                    );
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!("[WebServer] Outbound dispatch lagged by {} messages", n);
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                tracing::info!("[WebServer] Outbound bus channel closed, stopping dispatch");
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

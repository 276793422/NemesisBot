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
    AppState, handle_api_config, handle_api_events, handle_api_license, handle_api_logs,
    handle_api_models, handle_api_readme, handle_api_scanner_status, handle_api_sessions,
    handle_api_status, handle_api_version,
};
use crate::api_usage::{
    handle_api_usage_logs, handle_api_usage_pricing, handle_api_usage_summary,
    handle_api_usage_trends,
};
use crate::cors::dev_cors_layer;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::websocket_handler::handle_websocket_upgrade;
use axum::extract::State as AxumState;
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use futures::stream::Stream;
use nemesis_bus::MessageBus;
use nemesis_types::channel::InboundMessage;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tower_http::services::ServeDir;

// ---------------------------------------------------------------------------
// Port allocation（bind 失败端口走查）
// ---------------------------------------------------------------------------

/// 网关 Web 端口带下界（单一真相源）。配置端口落在此带内时，bind 走查
/// 从配置端口起 +1 前进、到 [`WEB_PORT_MAX`] 回绕 [`WEB_PORT_MIN`]
/// 循环往复，绕满一整圈仍无可用端口才 loud 失败。
pub const WEB_PORT_MIN: u16 = 49000;

/// 网关 Web 端口带上界（见 [`WEB_PORT_MIN`]）。
pub const WEB_PORT_MAX: u16 = 50000;

/// WebSocket 通道的专属端口（`config.json` `channels.websocket.port` 默认值）。
/// Web 走查必须让位：web 若抢占 49001，WebSocket 通道随后 bind 直接失败。
const WEBSOCKET_CHANNEL_PORT: u16 = 49001;

/// 配置端口在 [`WEB_PORT_MIN`]..=[`WEB_PORT_MAX`] 之外时的线性走查步数
/// （+1 最多 20 次，不回绕——不强行改写用户显式配置的端口段）。
const OUT_OF_RANGE_WALK_ATTEMPTS: u16 = 20;

/// 生成 bind 走查的端口尝试序列（纯函数，单测锚点）。
///
/// - `0` → `[0]`（OS 随机分配，单次 bind，不进走查）；
/// - 端口带内 → 从 `base_port` 起回绕整圈：带内全部端口按序各出现一次
///   （[`WEB_PORT_MIN`]..=[`WEB_PORT_MAX`] 共 1001 个），到 [`WEB_PORT_MAX`]
///   后回绕 [`WEB_PORT_MIN`]；
/// - 端口带外 → 从 `base_port` 线性向上 [`OUT_OF_RANGE_WALK_ATTEMPTS`] 个。
///
/// 序列全程跳过 [`WEBSOCKET_CHANNEL_PORT`]（49001，WebSocket 通道专属端口）。
fn port_walk_sequence(base_port: u16) -> Vec<u16> {
    let span = WEB_PORT_MAX - WEB_PORT_MIN + 1; // 10001
    let mut seq: Vec<u16> = if base_port == 0 {
        vec![0]
    } else if (WEB_PORT_MIN..=WEB_PORT_MAX).contains(&base_port) {
        (0..span)
            .map(|i| WEB_PORT_MIN + (base_port - WEB_PORT_MIN + i) % span)
            .collect()
    } else {
        (0..OUT_OF_RANGE_WALK_ATTEMPTS)
            .map(|i| base_port.saturating_add(i))
            .collect()
    };
    // 49001 让位 WebSocket 通道（port 0 不可能等于 49001，无需特判）。
    seq.retain(|p| *p != WEBSOCKET_CHANNEL_PORT);
    seq
}

/// bind 失败端口走查：按 [`port_walk_sequence`] 逐个尝试 `ip:port`，
/// 返回第一个绑定成功的 listener；序列耗尽仍全占用则 loud 失败。
/// 落点偏离 `base_port` 时打 warn（运维可见端口漂移）。
async fn bind_with_port_walk(
    ip: std::net::IpAddr,
    base_port: u16,
) -> Result<tokio::net::TcpListener, String> {
    let sequence = port_walk_sequence(base_port);
    let mut last_err = String::new();
    for (i, &try_port) in sequence.iter().enumerate() {
        let try_addr = std::net::SocketAddr::new(ip, try_port);
        match tokio::net::TcpListener::bind(try_addr).await {
            Ok(l) => {
                if try_port != base_port {
                    tracing::warn!(
                        "[WebServer] Port {} busy, using {} instead",
                        base_port,
                        try_port
                    );
                }
                return Ok(l);
            }
            Err(e) => {
                last_err = format!("{e}");
                tracing::warn!(
                    "[WebServer] Bind attempt {} failed on '{}': {}",
                    i,
                    try_addr,
                    e
                );
            }
        }
    }
    Err(format!(
        "bind failed: no available port for base {base_port} after trying {} ports: {last_err}",
        sequence.len()
    ))
}

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
    /// Optional streaming LLM provider for SSE chat endpoint + persona
    /// generation（协议感知槽，B 根修 2026-09-17；经 factory 装配）。
    streaming_provider: Option<Arc<dyn nemesis_providers::router::LLMProvider>>,
    /// Agent loop service for start/stop/status control.
    agent_service: Option<Arc<dyn nemesis_services::bot_service::AgentLoopService>>,
    /// Data store for usage statistics queries.
    data_store: Option<Arc<nemesis_data::DataStore>>,
    /// Memory manager for runtime vector store control.
    #[cfg(feature = "memory")]
    memory_manager: Option<Arc<nemesis_memory::manager::MemoryManager>>,
    #[cfg(not(feature = "memory"))]
    #[allow(dead_code)]
    memory_manager: Option<()>,
    /// Forge self-learning instance for runtime start/stop control.
    #[cfg(feature = "forge")]
    forge: Option<Arc<nemesis_forge::forge::Forge>>,
    #[cfg(not(feature = "forge"))]
    forge: Option<()>,
    /// Agent loop for runtime model/provider switching.
    /// Shared with AgentLoopServiceAdapter — updated on each start/stop.
    agent_loop: Option<Arc<parking_lot::RwLock<Option<Arc<nemesis_agent::r#loop::AgentLoop>>>>>,
    /// Cluster runtime instance for dashboard data queries.
    #[cfg(feature = "cluster")]
    cluster: Option<Arc<nemesis_cluster::cluster::Cluster>>,
    #[cfg(not(feature = "cluster"))]
    cluster: Option<()>,
    /// Cluster lifecycle service for start/stop control.
    cluster_service: Option<Arc<dyn nemesis_services::bot_service::LifecycleService>>,
    /// Cluster log directory for JSONL log reader.
    cluster_log_dir: Option<String>,
    /// Workflow engine for trigger / execution APIs (milestone 1a-E3/E4).
    #[cfg(feature = "workflow")]
    workflow_engine: Option<Arc<nemesis_workflow::engine::WorkflowEngine>>,
    #[cfg(not(feature = "workflow"))]
    #[allow(dead_code)]
    workflow_engine: Option<()>,
    /// Per-workflow chat password store for the standalone workflow-chat page.
    /// Gateway constructs this from `{home}/workspace/workflow/chat_secrets.json`
    /// and shares it with handlers that verify passwords.
    #[cfg(feature = "workflow")]
    chat_secret_store: Option<Arc<nemesis_workflow::chat_secrets::ChatSecretStore>>,
    #[cfg(not(feature = "workflow"))]
    #[allow(dead_code)]
    chat_secret_store: Option<()>,
    /// Per-IP rate limiter for webhook endpoints (milestone 1c-E5).
    /// Created lazily; shared with AppState.
    #[cfg(feature = "workflow")]
    webhook_rate_limiter: Arc<crate::handlers::workflow::WebhookRateLimiter>,
    #[cfg(not(feature = "workflow"))]
    #[allow(dead_code)]
    webhook_rate_limiter: Arc<()>,
    /// Internal command sender for /api/internal endpoint.
    internal_cmd_tx: Option<tokio::sync::mpsc::Sender<crate::internal::InternalCommand>>,
    /// Global e-stop state for /api/internal estop commands.
    estop: Option<Arc<nemesis_agent::estop::EstopState>>,
    /// Runtime cron service (set by gateway; flows into AppState for tasks.cron.*).
    cron: Option<Arc<std::sync::Mutex<nemesis_cron::CronService>>>,
    /// Managed-agent board service (set by gateway when the `board` feature is on;
    /// flows into AppState for board.* WSAPI commands).
    board: Option<nemesis_board::BoardService>,
    /// Conversation→WS router for cron-initiated live delivery (Opt 2).
    /// Populated on inbound in `process_messages`; read by the gateway cron
    /// fire handler to pick a live `chat_id` for the targeted conversation.
    conv_router: Option<crate::conv_router::SharedConvRouter>,
    /// C5 (2026-09-04): shared LSP manager singleton (same Arc the LspTool
    /// registers with). Held for the Phase-2 diagnostics loop (C1-C3) and
    /// any dashboard-driven LSP operations; gateway shutdown_all lives in
    /// gateway.rs Step 24.
    lsp_manager: Option<Arc<nemesis_lsp::LspManager>>,
    /// M1a (2026-09-05): receiving end of the agent tool-event broadcast
    /// channel. gateway injects after creating the channel; `start` spawns
    /// the pump that routes each event to the Dashboard WS session
    /// (`{type:"push", cmd:"tool_event"}`) + EventHub (SSE fallback).
    agent_event_rx: Option<tokio::sync::broadcast::Receiver<nemesis_types::agent::AgentEvent>>,
    /// 反向桥中继服务端（goal：反向桥与多设备汇聚）。`set_relay` 注入后
    /// build_router 挂载 `/bridge`、`/d/<node_id>/`、`/relay` 等桥路由；
    /// None = 接入门不开放（fail-closed，路由不存在）。
    relay: Option<std::sync::Arc<crate::relay::RelayServer>>,
    /// 本机桥身份（goal 批次二）：桥客户端接入中继时用的 node_id。子路径
    /// 中间件据此识别 `/d/<自身 node_id>/` 前缀（剥前缀 + base 注入）；
    /// None = 无身份（`--relay` 纯中继不服务自己的面板），前缀剥离永不
    /// 命中。`set_bridge_identity` 注入。
    bridge_node_id: Option<String>,
    /// `--relay` 纯中继形态开关（2026-09-20，用户裁决：纯中继不暴露 hub
    /// 自身 dashboard——`/api/*` 的信任边界是「本机/内网」，而纯中继绑
    /// 0.0.0.0 公网，暴露即失守；dashboard 静态资源也无装配价值）。
    /// true = build_router 丢弃 dashboard 全量路由（/ws、/api/*、静态
    /// 资源），只保留 /health + 反向桥路由。默认 false，正常模式不变。
    /// `set_relay_only` 打开。
    relay_only: bool,
    /// 入站过滤链（BUG 2026-09-23 项目会话历史修复）：Dashboard WS 消息
    /// 唯一咽喉点（`process_messages_with_router`）在 `bus.publish_inbound`
    /// 之前过链——链上过滤器可就地拦截应答（如 history 只读查询），消息
    /// 不再进入 bus 扇出，与任何 agent loop 的存亡/忙闲解耦。`set_inbound_filter_chain`
    /// 注入；None = 直通（legacy 调用方/测试零影响）。
    inbound_filters: Option<Arc<nemesis_bus::FilterChain<InboundMessage>>>,
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
            relay_only: false,
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
            cluster: None,
            cluster_service: None,
            cluster_log_dir: None,
            workflow_engine: None,
            chat_secret_store: None,
            #[cfg(feature = "workflow")]
            webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
            #[cfg(not(feature = "workflow"))]
            webhook_rate_limiter: std::sync::Arc::new(()),
            internal_cmd_tx: None,
            estop: None,
            cron: None,
            board: None,
            conv_router: None,
            lsp_manager: None,
            agent_event_rx: None,
            relay: None,
            bridge_node_id: None,
            inbound_filters: None,
        }
    }

    /// 注入本机桥身份（goal 批次二）：子路径中间件据此识别
    /// `/d/<自身 node_id>/` 前缀。正常启动在桥客户端装配时注入（与
    /// bridge_client 同一身份）；`--relay` 纯中继不注入。
    pub fn set_bridge_identity(&mut self, node_id: String) {
        self.bridge_node_id = Some(node_id);
    }

    /// Set the message bus for inbound message publishing.
    pub fn set_message_bus(&mut self, bus: Arc<MessageBus>) {
        self.message_bus = Some(bus);
    }

    /// 注入入站过滤链（须在 `start`/`build_router` 前调用）。链在
    /// `process_messages_with_router` 内、`bus.publish_inbound` 之前执行；
    /// `Intercepted` = 过滤器已就地应答，消息不进 bus 扇出（continue）。
    /// 谁想拦什么，谁构造 [`nemesis_bus::FilterChain`] 并往里注册过滤器。
    pub fn set_inbound_filter_chain(
        &mut self,
        chain: Arc<nemesis_bus::FilterChain<InboundMessage>>,
    ) {
        self.inbound_filters = Some(chain);
    }

    /// Set model info: name, API base URL, and whether a key is configured.
    pub fn set_model_info(&self, name: &str, base_url: &str, has_key: bool) {
        *self.model_name.lock() = name.to_string();
        *self.model_base.lock() = base_url.to_string();
        self.model_has_key
            .store(has_key, std::sync::atomic::Ordering::Release);
    }

    /// Set the workspace path for config/log access.
    pub fn set_workspace(&mut self, path: PathBuf) {
        self.config.workspace = Some(path.to_string_lossy().to_string());
    }

    /// Set the streaming LLM provider for the SSE chat endpoint.
    pub fn set_streaming_provider(
        &mut self,
        provider: Arc<dyn nemesis_providers::router::LLMProvider>,
    ) {
        self.streaming_provider = Some(provider);
    }

    /// Set the agent loop service for start/stop/status control.
    pub fn set_agent_service(
        &mut self,
        service: Arc<dyn nemesis_services::bot_service::AgentLoopService>,
    ) {
        self.agent_service = Some(service);
    }

    /// Set the data store for usage statistics queries.
    pub fn set_data_store(&mut self, store: Arc<nemesis_data::DataStore>) {
        self.data_store = Some(store);
    }

    /// Set the memory manager for runtime vector store control.
    #[cfg(feature = "memory")]
    pub fn set_memory_manager(&mut self, mgr: Arc<nemesis_memory::manager::MemoryManager>) {
        self.memory_manager = Some(mgr);
    }

    /// Set the Forge self-learning instance for runtime start/stop control.
    #[cfg(feature = "forge")]
    pub fn set_forge(&mut self, forge: Arc<nemesis_forge::forge::Forge>) {
        self.forge = Some(forge);
    }

    /// Set the agent loop ref for runtime model/provider switching.
    pub fn set_agent_loop(
        &mut self,
        agent_loop: Arc<parking_lot::RwLock<Option<Arc<nemesis_agent::r#loop::AgentLoop>>>>,
    ) {
        self.agent_loop = Some(agent_loop);
    }

    /// Set the cluster runtime instance for dashboard data queries.
    #[cfg(feature = "cluster")]
    pub fn set_cluster(&mut self, cluster: Arc<nemesis_cluster::cluster::Cluster>) {
        self.cluster = Some(cluster);
    }

    /// Set the cluster lifecycle service for start/stop control.
    pub fn set_cluster_service(
        &mut self,
        svc: Arc<dyn nemesis_services::bot_service::LifecycleService>,
    ) {
        self.cluster_service = Some(svc);
    }

    /// Set the cluster log directory for JSONL log reader.
    pub fn set_cluster_log_dir(&mut self, dir: String) {
        self.cluster_log_dir = Some(dir);
    }

    /// Set the workflow engine for /api/workflow/* endpoints (milestone 1a-E3/E4).
    #[cfg(feature = "workflow")]
    pub fn set_workflow_engine(&mut self, engine: Arc<nemesis_workflow::engine::WorkflowEngine>) {
        self.workflow_engine = Some(engine);
    }

    /// Set the per-workflow chat password store.
    #[cfg(feature = "workflow")]
    pub fn set_chat_secret_store(
        &mut self,
        store: Arc<nemesis_workflow::chat_secrets::ChatSecretStore>,
    ) {
        self.chat_secret_store = Some(store);
    }

    /// Set the internal command sender for /api/internal endpoint.
    pub fn set_internal_cmd_tx(
        &mut self,
        tx: tokio::sync::mpsc::Sender<crate::internal::InternalCommand>,
    ) {
        self.internal_cmd_tx = Some(tx);
    }

    /// Set the global e-stop state for /api/internal estop commands.
    pub fn set_estop(&mut self, estop: Arc<nemesis_agent::estop::EstopState>) {
        self.estop = Some(estop);
    }

    /// Set the runtime cron service for `tasks.cron.*` handlers.
    pub fn set_cron(&mut self, cron: Arc<std::sync::Mutex<nemesis_cron::CronService>>) {
        self.cron = Some(cron);
    }

    /// Set the managed-agent board service (gateway injects when the `board`
    /// feature is enabled and the store opened successfully).
    pub fn set_board(&mut self, board: nemesis_board::BoardService) {
        self.board = Some(board);
    }

    /// Set the conversation→WS router (Opt 2). Gateway shares the same Arc
    /// with the cron fire handler so binds (here, on inbound) and lookups
    /// (there, on fire) see the same table.
    pub fn set_conv_router(&mut self, router: crate::conv_router::SharedConvRouter) {
        self.conv_router = Some(router);
    }

    /// 反向桥中继服务端（goal：反向桥与多设备汇聚）。注入后 build_router
    /// 挂载桥路由（`/bridge`、`/d/<node_id>/`、`__auth`、`/relay`、
    /// `/api/relay/status`）。不注入 = 接入门不开放。
    pub fn set_relay(&mut self, relay: std::sync::Arc<crate::relay::RelayServer>) {
        self.relay = Some(relay);
    }

    /// `--relay` 纯中继形态开关（须在 `start`/`build_router` 前调用）。
    /// true = build_router 丢弃 dashboard 全量路由（/ws、全量 /api/*、
    /// 静态资源），公网暴露面收敛到 /health + 反向桥路由。见字段 doc。
    pub fn set_relay_only(&mut self, relay_only: bool) {
        self.relay_only = relay_only;
    }

    /// C5 (2026-09-04): hold the shared LSP manager singleton (same Arc as
    /// the registered LspTool). Consumers: Phase-2 diagnostics loop (C1-C3);
    /// process teardown stays in gateway.rs Step 24 (`shutdown_all`).
    pub fn set_lsp_manager(&mut self, mgr: Arc<nemesis_lsp::LspManager>) {
        self.lsp_manager = Some(mgr);
    }

    /// The held LSP manager, if injected (C5).
    pub fn lsp_manager(&self) -> Option<Arc<nemesis_lsp::LspManager>> {
        self.lsp_manager.clone()
    }

    /// M1a (2026-09-05): hold the receiving end of the agent tool-event
    /// broadcast channel. `start`/`start_with_shutdown` spawn the pump from
    /// it; injecting `None` (default) keeps the pump off.
    pub fn set_agent_event_rx(
        &mut self,
        rx: tokio::sync::broadcast::Receiver<nemesis_types::agent::AgentEvent>,
    ) {
        self.agent_event_rx = Some(rx);
    }

    /// M1a: spawn the tool-event pump if a receiver was injected. Called
    /// from `start`/`start_with_shutdown` alongside the status loop.
    fn start_agent_event_pump(&self) {
        let Some(rx) = self.agent_event_rx.as_ref().map(|rx| rx.resubscribe()) else {
            return;
        };
        let session_manager = self.session_manager.clone();
        let event_hub = self.event_hub.clone();
        tokio::spawn(async move {
            pump_agent_events(rx, session_manager, event_hub).await;
        });
        tracing::info!("[AgentEventPump] tool-event pump started");
    }

    /// Build the Axum router with all routes.
    pub fn build_router(&self) -> Router {
        let (inbound_tx, mut inbound_rx) =
            mpsc::unbounded_channel::<crate::websocket_handler::IncomingMessage>();

        // minimal（无 feature）形态下这四个槽位别名退化为 Option<()>（Copy），
        // 字面量里 .clone() 会触发 clippy::clone_on_copy（2026-09-05 远端
        // minimal clippy 实录；workspace 全量 feature unification 掩盖）。
        // 按 cfg 取值：full 形态 clone Arc 槽，minimal 形态直接 Copy。
        #[cfg(feature = "memory")]
        let memory_manager = self.memory_manager.clone();
        #[cfg(not(feature = "memory"))]
        let memory_manager = self.memory_manager;
        #[cfg(feature = "forge")]
        let forge = self.forge.clone();
        #[cfg(not(feature = "forge"))]
        let forge = self.forge;
        #[cfg(feature = "cluster")]
        let cluster = self.cluster.clone();
        #[cfg(not(feature = "cluster"))]
        let cluster = self.cluster;
        #[cfg(feature = "workflow")]
        let workflow_engine = self.workflow_engine.clone();
        #[cfg(not(feature = "workflow"))]
        let workflow_engine = self.workflow_engine;

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
            memory_manager,
            forge,
            agent_loop: self
                .agent_loop
                .clone()
                .unwrap_or_else(|| Arc::new(parking_lot::RwLock::new(None))),
            cluster,
            cluster_service: self.cluster_service.clone(),
            cluster_log_dir: self.cluster_log_dir.clone(),
            workflow_engine,
            #[cfg(feature = "workflow")]
            chat_secret_store: self.chat_secret_store.clone().unwrap_or_else(|| {
                Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory())
            }),
            #[cfg(not(feature = "workflow"))]
            chat_secret_store: std::sync::Arc::new(()),
            webhook_rate_limiter: self.webhook_rate_limiter.clone(),
            internal_cmd_tx: self.internal_cmd_tx.clone(),
            estop: self.estop.clone(),
            cron: self.cron.clone(),
            board: self.board.clone(),
        };

        let state = Arc::new(state);

        // L8 PTY 内嵌终端：模块级会话管理器（terminal feature 门控；
        // 幂等 OnceLock，重复 build_router 不覆盖）。cwd 取自 config
        // 的 workspace（None = 继承进程 cwd）。
        #[cfg(feature = "terminal")]
        crate::pty::ensure_manager(self.config.workspace.clone());

        // Spawn the bus bridge: incoming WebSocket messages -> MessageBus.publish_inbound
        if let Some(ref bus) = self.message_bus {
            let bus = bus.clone();
            let conv_router = self.conv_router.clone();
            let session_manager = self.session_manager.clone();
            let inbound_filters = self.inbound_filters.clone();
            tokio::spawn(async move {
                process_messages_with_router(
                    inbound_rx,
                    bus,
                    conv_router,
                    Some(session_manager),
                    inbound_filters,
                )
                .await;
            });
        } else {
            // No bus configured; drain messages to avoid leaking the sender
            tokio::spawn(async move { while inbound_rx.recv().await.is_some() {} });
        }

        let router = Router::new()
            // WebSocket endpoint
            .route(
                &self.config.ws_path,
                axum::routing::get(handle_websocket_upgrade),
            )
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
            // SDK export downloads (P2-2, 二次开发 page)
            .route(
                "/api/sdk/export",
                get(crate::api_handlers::handle_sdk_export),
            )
            .route("/api/sdk/pip", get(crate::api_handlers::handle_sdk_pip))
            // Usage statistics endpoints
            .route("/api/usage/summary", get(handle_api_usage_summary))
            .route("/api/usage/trends", get(handle_api_usage_trends))
            .route("/api/usage/logs", get(handle_api_usage_logs))
            .route(
                "/api/usage/logs/{id}",
                get(crate::api_usage::handle_api_usage_log_detail),
            )
            .route("/api/usage/pricing", get(handle_api_usage_pricing))
            // 价目表管理（A2 在线更新 / 自定义条目 / 离线导入）
            .route(
                "/api/usage/pricing/update",
                post(crate::api_usage::handle_api_usage_pricing_update),
            )
            .route(
                "/api/usage/pricing/custom",
                post(crate::api_usage::handle_api_usage_pricing_custom_upsert),
            )
            .route(
                "/api/usage/pricing/custom/remove",
                post(crate::api_usage::handle_api_usage_pricing_custom_remove),
            )
            // LiteLLM 原始表 ~2MB，默认 body 上限贴边 → 放宽到 16MB。
            .route(
                "/api/usage/pricing/import",
                post(crate::api_usage::handle_api_usage_pricing_import)
                    .layer(axum::extract::DefaultBodyLimit::max(16 * 1024 * 1024)),
            )
            // SSE event stream
            .route("/api/events/stream", get(handle_events_stream))
            // T8（多模态）：Dashboard 图片上传（raw body + ?name=；25MB 上限
            // 需放宽 axum 默认 2MB body 限制，+1MB 余量留给超限错误路径）。
            .route(
                "/api/upload/image",
                axum::routing::post(crate::handlers::upload::handle_upload_image).layer(
                    axum::extract::DefaultBodyLimit::max(
                        crate::handlers::upload::UPLOAD_BODY_LIMIT_BYTES,
                    ),
                ),
            )
            // SSE chat streaming endpoint
            .route(
                "/api/chat/stream",
                axum::routing::post(crate::sse_chat::handle_chat_stream),
            )
            // Session fork dialog backing (P3-1, 2026-08-24 UI entry gap)
            .route(
                "/api/chat/sessions/{id}/turns",
                get(crate::api_handlers::handle_api_chat_session_turns),
            )
            .route(
                "/api/chat/sessions/{id}/fork",
                axum::routing::post(crate::api_handlers::handle_api_chat_session_fork),
            )
            // L4 会话分享（2026-09-07）：公开只读端点。**故意不过
            // verify_token** —— token 即凭据（分享链接发给无凭据的接收方），
            // 未知/撤销/会话已删一律 404；见 crate::share 模块头。
            .route("/api/share/{token}", get(crate::share::handle_api_share))
            // Swarm M3（§5.4/D6）：看板资产下载——公开端点（asset_token 即
            // 凭据，不认 dashboard token；token 随引用走，见 handlers/board_asset）。
            .route(
                "/api/board/asset/{ref}",
                get(crate::handlers::board_asset::handle_board_asset_download),
            );

        // `--relay` 纯中继形态（set_relay_only）：丢弃上面构建的 dashboard
        // 全量路由（/ws、全量 /api/*），只保留 /health——/api/* 的信任边界
        // 是「本机/内网」，而纯中继绑 0.0.0.0 公网（FIX-1 绑定语义），暴露
        // 即失守。全量链照常构建一次（AppState 槽位全空，纯内存操作无副
        // 作用），随后整体替换——路由装配保持单链形态，零分叉回归风险。
        let router = if self.relay_only {
            Router::new().route("/health", get(handle_health))
        } else {
            router
        };

        // L8 PTY 内嵌终端端点（terminal feature 门控；config
        // terminal.enabled 运行闸 + token 闸在 handler 内）。
        #[cfg(feature = "terminal")]
        let router = if self.relay_only {
            router
        } else {
            router.route("/ws/pty", get(crate::pty::handle_pty_upgrade))
        };

        // Workflow REST endpoints (milestone 1a-E3/E4)
        #[cfg(feature = "workflow")]
        let router = if self.relay_only {
            router
        } else {
            router.merge(crate::handlers::workflow::routes())
        };

        // 反向桥路由（goal：反向桥与多设备汇聚）。`set_relay` 注入后才
        // 挂载——None = 接入门不开放（fail-closed：路由不存在，伪装 404
        // 的语义由 handler 再按开关复核）。
        //
        // 路由形态三条（axum 0.8.9 实测语义）：
        // - `{*rest}` catch-all 不匹配空段 → `/d/<id>` 与 `/d/<id>/` 必须
        //   单独注册（`/d/<id>/` 是授权后 redirect 的目标形态，主入口）；
        // - `{*rest}` 路由含两个 path 参数，handler 闭包须以
        //   `Path<(String, String)>` 提取（`Path<String>` 会 500
        //   "Expected 1 but got 2"）；rest 不进 handler——转发的是原样
        //   URI，前缀由设备侧剥离。
        let router = if let Some(ref relay) = self.relay {
            let relay_bridge = relay.clone();
            let relay_dev_root = relay.clone();
            let relay_dev_slash = relay.clone();
            let relay_dev = relay.clone();
            let relay_auth_get = relay.clone();
            let relay_auth_post = relay.clone();
            let relay_page = relay.clone();
            let relay_login = relay.clone();
            let relay_api = relay.clone();
            router
                .route(
                    "/bridge",
                    get(move |ws: axum::extract::ws::WebSocketUpgrade| {
                        let relay = relay_bridge.clone();
                        async move { crate::relay::handle_bridge_ws(ws, relay).await }
                    }),
                )
                .route(
                    "/d/{node_id}",
                    any(move |node_id: axum::extract::Path<String>,
                              req: axum::extract::Request| {
                        let relay = relay_dev_root.clone();
                        async move {
                            crate::relay::handle_device_request(relay, node_id.0, req).await
                        }
                    }),
                )
                .route(
                    "/d/{node_id}/",
                    any(move |node_id: axum::extract::Path<String>,
                              req: axum::extract::Request| {
                        let relay = relay_dev_slash.clone();
                        async move {
                            crate::relay::handle_device_request(relay, node_id.0, req).await
                        }
                    }),
                )
                .route(
                    "/d/{node_id}/{*rest}",
                    any(move |path: axum::extract::Path<(String, String)>,
                              req: axum::extract::Request| {
                        let relay = relay_dev.clone();
                        async move {
                            crate::relay::handle_device_request(relay, path.0 .0, req).await
                        }
                    }),
                )
                .route(
                    "/d/{node_id}/__auth",
                    get(move |node_id: axum::extract::Path<String>, req: axum::extract::Request| {
                        let relay = relay_auth_get.clone();
                        async move { crate::relay::handle_auth_page(relay, node_id.0, req).await }
                    })
                    .post(
                        move |node_id: axum::extract::Path<String>,
                              req: axum::extract::Request| {
                            let relay = relay_auth_post.clone();
                            async move {
                                crate::relay::handle_auth_submit(relay, node_id.0, req).await
                            }
                        },
                    ),
                )
                .route(
                    "/relay",
                    get(move |req: axum::extract::Request| {
                        let relay = relay_page.clone();
                        async move { crate::relay::handle_relay_status_page(relay, req).await }
                    }),
                )
                .route(
                    "/relay/login",
                    post(move |req: axum::extract::Request| {
                        let relay = relay_login.clone();
                        async move { crate::relay::handle_relay_login(relay, req).await }
                    }),
                )
                .route(
                    "/api/relay/status",
                    get(move |req: axum::extract::Request| {
                        let relay = relay_api.clone();
                        async move { crate::relay::handle_relay_api_status(relay, req).await }
                    }),
                )
        } else {
            router
        };

        // 批次三：通道页【中继通道】端点。**无条件注册**——overview 的
        // 客户端态与 client/reconnect 独立于服务端配置；relay 未注入时
        // overview 的 server 字段诚实回 null（enabled 端点回 400）。
        // dashboard 信任边界（与 /api/status 同语义：本机/内网）。
        // `--relay` 纯中继例外：无 dashboard 消费这些端点（服务端态走
        // /relay 状态页的 /api/relay/status），不挂。
        let router = if self.relay_only {
            router
        } else {
            let relay_overview = self.relay.clone();
            let relay_enabled = self.relay.clone();
            router
                .route(
                    "/api/relay/overview",
                    get(move || {
                        let relay = relay_overview.clone();
                        async move { crate::relay::handle_relay_api_overview(relay).await }
                    }),
                )
                .route(
                    "/api/relay/enabled",
                    post(move |req: axum::extract::Request| {
                        let relay = relay_enabled.clone();
                        async move { crate::relay::handle_relay_api_enabled(relay, req).await }
                    }),
                )
                .route(
                    "/api/relay/client/reconnect",
                    post(crate::relay::handle_relay_api_client_reconnect),
                )
        };

        let router = if self.relay_only {
            router
        } else {
            // Internal control endpoint (undocumented)
            router.route(
                "/api/internal",
                axum::routing::post(crate::api_handlers::handle_api_internal),
            )
        };

        // F1（2026-09-22 审计修复）：REST 控制面统一鉴权。route_layer 只
        // 作用于其上已注册的路由（含 /api/internal），静态文件 fallback
        // 不受影响；豁免清单见 `auth_exempt_path`。relay-only 形态**不挂**：
        // 其路由面（/bridge、/d/*、/relay、/relay/login、/api/relay/status、
        // /health）是 relay 自有信任边界（设备/管理侧鉴权在 relay 模块内），
        // dashboard token 对其无意义，挂上会把 relay 整体 401。
        let cors = if self.config.cors_origins.is_empty() {
            dev_cors_layer()
        } else {
            crate::cors::production_cors_layer(&self.config.cors_origins)
        };
        let mut router = if self.relay_only {
            router.layer(cors).with_state(state.clone())
        } else {
            // 闭包捕获 ws_path（AppState 不携带它，且测试里 120+ 处字面量
            // 构造 AppState，加字段会大面积破坏）——workflow_chat 豁免只对
            // WS 路径成立，其余路径照常鉴权。
            let ws_path = self.config.ws_path.clone();
            let auth_state = state.clone();
            router
                .route_layer(axum::middleware::from_fn(
                    move |req: axum::extract::Request, next: axum::middleware::Next| {
                        let ws_path = ws_path.clone();
                        let state = auth_state.clone();
                        async move { auth_middleware(AxumState(state), req, next, &ws_path).await }
                    },
                ))
                // CORS layer（外层——preflight OPTIONS 不进鉴权闸）
                .layer(cors)
                .with_state(state.clone())
        };

        // Add static file serving if configured（`--relay` 纯中继不挂——
        // dashboard 静态资源无装配价值，未匹配路径一律 404；run_relay 侧
        // 也不传 static_files，双保险）。
        if self.relay_only {
            tracing::info!("[WebServer] relay-only：dashboard 静态资源不装配（未匹配路径 404）");
        } else if let Some(ref files) = self.config.static_files {
            // In-memory static file serving (zero disk IO)
            let files = files.clone();
            tracing::info!("[WebServer] Serving static files from embedded memory");
            router = router.fallback(move |req: axum::extract::Request| {
                let files = files.clone();
                async move {
                    // Handle CORS preflight for static assets (WebKitGTK may send
                    // OPTIONS before loading module scripts with crossorigin attr).
                    if req.method() == http::method::Method::OPTIONS {
                        let origin = req
                            .headers()
                            .get(http::header::ORIGIN)
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("*")
                            .to_string();
                        return (
                            axum::http::StatusCode::NO_CONTENT,
                            [
                                (http::header::ACCESS_CONTROL_ALLOW_ORIGIN, origin),
                                (
                                    http::header::ACCESS_CONTROL_ALLOW_METHODS,
                                    "GET, OPTIONS".to_string(),
                                ),
                                (http::header::ACCESS_CONTROL_ALLOW_HEADERS, "*".to_string()),
                                (http::header::VARY, "Origin".to_string()),
                            ],
                        )
                            .into_response();
                    }
                    serve_embedded_static(files, req).await
                }
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
                            let ct = response
                                .headers()
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

        // 桥子路径外壳（goal 批次二）：`/d/<自身 node_id>/` 前缀剥离 +
        // HTML `<base href>` 注入。**不能**用 `Router::layer`——axum 0.8
        // 的 layer 运行于路由匹配之后（routing/mod.rs `Router::layer` →
        // path_router.layer 包装 endpoint），改 URI 来不及；外壳形态
        // （无路由 Router + fallback_service）保证改写在匹配前发生。
        // `/d/<他人>/` 转发请求在外壳内原样透传。无身份（--relay）时
        // 前缀剥离永不命中，但直连 base 注入仍生效（相对构建产物在
        // `/chat/` 等子路径入口的正确性必需）。
        let inner = router;
        tracing::info!("[WebServer] Router built, routes registered");
        Router::new().fallback_service(crate::relay::BridgeSubpathService::new(
            self.bridge_node_id.clone(),
            inner,
        ))
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
        self.running
            .store(true, std::sync::atomic::Ordering::SeqCst);

        let _status_handle = start_publish_status_loop(
            self.event_hub.clone(),
            self.session_count.clone(),
            self.config.version.clone(),
            self.start_time,
            self.running.clone(),
        );
        // M1a: tool events → Dashboard WS push + EventHub (no-op if the
        // gateway never injected a receiver).
        self.start_agent_event_pump();

        let addr: SocketAddr = self.config.listen_addr.parse().map_err(|e| {
            tracing::error!(
                "[WebServer] Invalid listen address '{}': {}",
                self.config.listen_addr,
                e
            );
            format!("invalid listen address: {}", e)
        })?;
        let app = self.build_router();
        // bind 失败端口走查（幽灵占用/TIME_WAIT 恢复）：49000~50000 带内
        // 循环往复、绕满一圈才 loud 失败；详见 bind_with_port_walk。
        let listener = bind_with_port_walk(addr.ip(), addr.port()).await?;

        let actual_addr = listener
            .local_addr()
            .map_err(|e| format!("failed to get local addr: {}", e))?;
        tracing::info!("[WebServer] Listening on {}", actual_addr);
        axum::serve(listener, app).await.map_err(|e| {
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
        self.running
            .store(true, std::sync::atomic::Ordering::SeqCst);

        let _status_handle = start_publish_status_loop(
            self.event_hub.clone(),
            self.session_count.clone(),
            self.config.version.clone(),
            self.start_time,
            self.running.clone(),
        );
        // M1a: tool events → Dashboard WS push + EventHub (no-op if the
        // gateway never injected a receiver).
        self.start_agent_event_pump();

        let addr: SocketAddr = self.config.listen_addr.parse().map_err(|e| {
            tracing::error!(
                "[WebServer] Invalid listen address '{}': {}",
                self.config.listen_addr,
                e
            );
            format!("invalid listen address: {}", e)
        })?;
        let app = self.build_router();
        // bind 失败端口走查（幽灵占用/TIME_WAIT 恢复）：49000~50000 带内
        // 循环往复、绕满一圈才 loud 失败；详见 bind_with_port_walk。
        let listener = bind_with_port_walk(addr.ip(), addr.port()).await?;

        let actual_addr = listener
            .local_addr()
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
        self.running
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// Static files utility
// ---------------------------------------------------------------------------

/// Determine Content-Type for a static file path.
pub(crate) fn content_type_for(path: &str) -> String {
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

/// CORS headers appended to every static-file response.
///
/// The main CORS middleware is applied to the route router via `.layer()`,
/// but on Linux/WebKitGTK the fallback handler may not inherit it.
/// WebKitGTK is stricter than WebView2 (Chromium) about `crossorigin`
/// module scripts — without `Access-Control-Allow-Origin` it silently
/// blocks the JS, resulting in a blank page.
fn cors_origin_value(req: &axum::extract::Request) -> String {
    req.headers()
        .get(http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("*")
        .to_string()
}

/// Serve a static file request from an in-memory `StaticFiles` provider.
///
/// 1. Exact path match
/// 2. Path-prefix rule: `workflow/chat/<anything>` → `workflow-chat.html`
///    (the standalone entry; the rest of the path is parsed client-side)
/// 3. SPA fallback: paths without a file extension → index.html
/// 4. 404
///
/// Exact `/chat` + `/chat/` are handled inside rule 3's slot: they serve the
/// standalone chat page shell (`chat/index.html`) rather than the dashboard
/// fallback (L2b). Deeper `/chat/<x>` paths keep the SPA fallback.
async fn serve_embedded_static(
    files: Arc<dyn StaticFiles>,
    req: axum::extract::Request,
) -> axum::response::Response {
    let path = req.uri().path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    let origin = cors_origin_value(&req);

    // 1. Exact match
    if let Some(content) = files.get_file(path) {
        let ct = content_type_for(path);
        return (
            axum::http::StatusCode::OK,
            [
                (http::header::CONTENT_TYPE, ct),
                (http::header::ACCESS_CONTROL_ALLOW_ORIGIN, origin),
                (http::header::VARY, "Origin".to_string()),
            ],
            content,
        )
            .into_response();
    }

    // 2. Path-prefix rule: standalone workflow-chat page.
    //    The 8-hex index lives in the URL path (not query), so any path
    //    beginning with `workflow/chat/` serves the same HTML shell; the
    //    client reads window.location.pathname to know which workflow to
    //    resolve.
    if path.starts_with("workflow/chat/")
        && let Some(content) = files.get_file("workflow-chat/index.html")
    {
        return (
            axum::http::StatusCode::OK,
            [
                (
                    http::header::CONTENT_TYPE,
                    "text/html; charset=utf-8".to_string(),
                ),
                (http::header::ACCESS_CONTROL_ALLOW_ORIGIN, origin),
                (http::header::VARY, "Origin".to_string()),
            ],
            content,
        )
            .into_response();
    }

    // 2b. Standalone chat page（LO2，2026-09-04 四轮盲审）：`/chat` 与
    //     `/chat/` 都直达 chat 壳——旧行为两者都落到 SPA fallback 的
    //     index.html（Dashboard），用户从友好的 /chat 地址进来看到的是
    //     错页面。chat 入口无子路径语义（不像 workflow/chat 的 index 在
    //     path 里），精确匹配两级即可；更深层 /chat/foo 不劫持。
    if (path == "chat" || path == "chat/")
        && let Some(content) = files.get_file("chat/index.html")
    {
        return (
            axum::http::StatusCode::OK,
            [
                (
                    http::header::CONTENT_TYPE,
                    "text/html; charset=utf-8".to_string(),
                ),
                (http::header::ACCESS_CONTROL_ALLOW_ORIGIN, origin),
                (http::header::VARY, "Origin".to_string()),
            ],
            content,
        )
            .into_response();
    }

    // 2c. 会话分享只读页（L4，2026-09-07）：`/share` 与 `/share/` 直达
    //     share 壳（token 在 ?t= 查询参数里，无子路径语义）。不加此规则
    //     两者都会落到 SPA fallback 的 Dashboard index.html——分享链接
    //     打开的是错页面。资产（js/css）带 hash 后缀走规则 1 精确匹配。
    if (path == "share" || path == "share/")
        && let Some(content) = files.get_file("share/index.html")
    {
        return (
            axum::http::StatusCode::OK,
            [
                (
                    http::header::CONTENT_TYPE,
                    "text/html; charset=utf-8".to_string(),
                ),
                (http::header::ACCESS_CONTROL_ALLOW_ORIGIN, origin),
                (http::header::VARY, "Origin".to_string()),
            ],
            content,
        )
            .into_response();
    }

    // 3. SPA fallback: no file extension → serve index.html
    if !path.contains('.')
        && let Some(content) = files.get_file("index.html")
    {
        return (
            axum::http::StatusCode::OK,
            [
                (
                    http::header::CONTENT_TYPE,
                    "text/html; charset=utf-8".to_string(),
                ),
                (http::header::ACCESS_CONTROL_ALLOW_ORIGIN, origin),
                (http::header::VARY, "Origin".to_string()),
            ],
            content,
        )
            .into_response();
    }

    // 4. 404
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
pub fn resolve_static_dir(explicit_path: Option<&str>, workspace: Option<&str>) -> Option<String> {
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
pub async fn handle_health(AxumState(state): AxumState<Arc<AppState>>) -> Json<serde_json::Value> {
    let session_count = state
        .session_count
        .load(std::sync::atomic::Ordering::SeqCst);
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
/// `event: <type>\ndata: <json>\nid: <seq>\n\n` frames. Includes an initial heartbeat.
///
/// L2（devtool-upgrade 阶段 6）断线补拉：每帧带 `id: <seq>`；浏览器
/// EventSource 重连时**自动**回传 `Last-Event-ID` header——端点开头从
/// EventHub 环形缓冲重放 seq>last 的事件（先重放后进 live，seq 游标去重）；
/// 缺口已滑出缓冲 → 先发 `resync` 提示事件（前端全量刷新兜底）。
/// broadcast `Lagged`（慢消费者丢帧）同路补偿——从缓冲回捞而非裸 continue。
pub async fn handle_events_stream(
    AxumState(state): AxumState<Arc<AppState>>,
    headers: HeaderMap,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    // 浏览器 EventSource 重连自动回传的断点游标（HeaderMap 键大小写不敏感）。
    let mut receiver = state.event_hub.subscribe();
    let last_id = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());

    let mut last_sent: u64 = 0;
    let (replay, gap) = match last_id {
        Some(after) => state.event_hub.replay_after(after),
        None => (Vec::new(), false),
    };
    if let Some(after) = last_id {
        last_sent = after;
    }
    let gap_hint = gap.then(|| {
        serde_json::json!({
            "reason": "events_outside_replay_window",
            "latest_seq": state.event_hub.latest_seq(),
        })
        .to_string()
    });
    let _running = state.running.clone();

    let stream = async_stream::stream! {
        tracing::debug!("[WebServer] SSE stream started");
        // Send initial heartbeat
        let heartbeat_data = serde_json::json!({"ts": chrono::Local::now().to_rfc3339()});
        yield Ok(SseEvent::default()
            .event("heartbeat")
            .data(heartbeat_data.to_string()));

        // L2 断线补拉：先重放缓冲内缺口
        if let Some(hint) = gap_hint {
            yield Ok(SseEvent::default().event("resync").data(hint));
        }
        for event in replay {
            last_sent = last_sent.max(event.seq);
            let data = serde_json::to_string(&event.data).unwrap_or_default();
            yield Ok(SseEvent::default()
                .event(event.event_type.clone())
                .id(event.seq.to_string())
                .data(data));
        }

        // Stream events from the event hub
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    // 重放桥接期已送达的 seq 跳过（订阅先于重放快照所致重叠）
                    if event.seq <= last_sent {
                        continue;
                    }
                    last_sent = event.seq;
                    let data = serde_json::to_string(&event.data).unwrap_or_default();
                    yield Ok(SseEvent::default()
                        .event(&event.event_type)
                        .id(event.seq.to_string())
                        .data(data));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    // L2：慢消费者丢帧从环形缓冲回捞（原实现裸 continue=丢事件
                    // 无补偿）；滑出缓冲才 resync。
                    tracing::warn!("[WebServer] SSE client lagged by {} events, backfilling from replay buffer", n);
                    let (backfill, gap) = state.event_hub.replay_after(last_sent);
                    if gap {
                        yield Ok(SseEvent::default()
                            .event("resync")
                            .data(serde_json::json!({
                                "reason": "lag_beyond_replay_window",
                                "latest_seq": state.event_hub.latest_seq(),
                            }).to_string()));
                    }
                    for event in backfill {
                        last_sent = last_sent.max(event.seq);
                        let data = serde_json::to_string(&event.data).unwrap_or_default();
                        yield Ok(SseEvent::default()
                            .event(event.event_type.clone())
                            .id(event.seq.to_string())
                            .data(data));
                    }
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
/// them to the message bus as InboundMessage. (No conv_router — legacy 2-arg
/// entry point kept for tests / simpler callers.)
pub async fn process_messages(
    rx: mpsc::UnboundedReceiver<crate::websocket_handler::IncomingMessage>,
    bus: Arc<MessageBus>,
) {
    process_messages_with_router(rx, bus, None, None, None).await;
}

/// Same as [`process_messages`] but also records conversation→chat_id
/// bindings into `conv_router` (when provided) so cron jobs can live-push
/// replies to the targeted conversation's open tab (Opt 2). `session_manager`
/// (when provided) enables the P8 user-row echo: the inbound user row is
/// recorded into the chat_event_log ring and echoed back to the sending
/// connection (see the record block below). `inbound_filters` (when
/// provided) runs the 入站过滤链 against each built InboundMessage before
/// `bus.publish_inbound` — `Intercepted` messages are answered in-place by
/// the filter and never fan out on the bus (BUG 2026-09-23 项目会话历史修复).
pub async fn process_messages_with_router(
    mut rx: mpsc::UnboundedReceiver<crate::websocket_handler::IncomingMessage>,
    bus: Arc<MessageBus>,
    conv_router: Option<crate::conv_router::SharedConvRouter>,
    session_manager: Option<Arc<SessionManager>>,
    inbound_filters: Option<Arc<nemesis_bus::FilterChain<InboundMessage>>>,
) {
    while let Some(msg) = rx.recv().await {
        let session_key = match msg.metadata.get("session_id") {
            // Multi-session: client-chosen conversation id → own session_key
            // (agent: prefix is adopted by loop.rs:1623, bypassing routing).
            Some(sid) if !sid.is_empty() => format!(
                "agent:main:session:{}",
                nemesis_agent::session::SessionStore::sanitize_session_id(sid)
            ),
            // No session_id → the single shared default conversation "legacy".
            // This is the ONLY web inbound chokepoint (process_messages handles
            // every Dashboard WS message), so routing the default here guarantees
            // EVERY web conversation lands in `agent:main:session:*` and thus
            // shows up in the session list (sessions.list filters on that
            // prefix). Must stay in lockstep with loop.rs handle_history_request
            // (same legacy fallback) so chat-write and history-read share a key.
            // Formerly `web:{chat_id}`, which the route resolver collapsed to
            // `agent:main:main` — invisible to the list (the bug).
            _ => "agent:main:session:legacy".to_string(),
        };

        // Opt 2: record conversation → live chat_id binding so a cron job
        // targeting this conversation can live-push its reply to this tab.
        // (The reply is persisted to history via session_key regardless.)
        if let Some(ref router) = conv_router {
            router.bind(&session_key, &msg.chat_id);
        }

        // P8 补全（2026-09-21）：user 行入环 + 发起连接回声帧。此前 user 行
        // 只落 chat_log（loop.rs turn 开始时）与发送方本地视图，环形补拉窗
        // 口里没有 user 行——多端场景（第二标签）的落后信号即便走增量 sync
        // 也拉不到它，而「游标短路」修掉无谓全量刷新后它就没有任何到达路径
        // （真机 F 剧本 F3 复现：tab2 只见回复不见提问）。入环后 seq 空间完
        // 整（chat 行与工具事件交错），增量通道全覆盖；回声帧推进发送方游标
        // （record 触发的 chat.activity 才能被同 seq 短路），发起方去重靠前
        // 端尾行同文比对（本地已有该行）。纯图消息（content 空）不入环——
        // 本地占位已在发送时渲染，历史恢复走 chat_log 全量，与旧行为一致。
        // ⚠️ 咽喉点流经两类消息：chat.send（用户文本）与 chat.history_request
        // （content 是请求 JSON、metadata 带 request_type="history"）——只对
        // 前者入环+回声。判据用排除式（BUG 修复 2026-09-21：history_request
        // 曾被误当 user 行入环回声，前端每个会话凭空出现一条请求 JSON「消
        // 息」并触发悬空占位「...」）；agent 侧消费惯例即只认
        // request_type=="history"（loop.rs 两处），其余按用户消息处理。
        let is_history_request =
            msg.metadata.get("request_type").map(String::as_str) == Some("history");
        if !msg.content.is_empty() && !is_history_request {
            let seq = crate::chat_event_log::record(&session_key, "user", &msg.content, None, None);
            if let Some(ref sm) = session_manager {
                let mut echo = serde_json::json!({
                    "role": "user",
                    "content": msg.content,
                    "seq": seq,
                });
                if let Some(sid) = session_key.strip_prefix("agent:main:session:") {
                    echo["session_id"] = serde_json::Value::String(sid.to_string());
                }
                let frame =
                    crate::protocol::ProtocolMessage::new("message", "chat", "receive", Some(echo));
                if let Ok(bytes) = frame.to_json() {
                    // 失败 soft：发起连接已断（切页/关闭）——行已入环，重连
                    // 后 sync 补拉自愈。可丢通道（BUG 2026-09-22）：帧带
                    // seq，慢客户端满队列时丢它可被 sync 补回，不应阻塞
                    // 入站咽喉点。
                    let _ = sm.broadcast_droppable(&msg.session_id, &bytes);
                }
            }
        }

        let inbound = InboundMessage {
            channel: "web".to_string(),
            sender_id: msg.sender_id.clone(),
            chat_id: msg.chat_id.clone(),
            content: msg.content,
            media: msg.media,
            session_key,
            correlation_id: String::new(),
            metadata: msg.metadata,
            voice_playback: msg.voice_playback,
        };

        // 入站过滤链（BUG 2026-09-23 项目会话历史修复）：在 bus 扇出之前
        // 过链——`Intercepted` = 过滤器已就地应答（如 history 只读查询经
        // bus.publish_outbound 回帧），消息不再进入扇出，与任何 agent loop
        // 的存亡/忙闲解耦；`Rejected` = 策略拒绝，回执拒绝原因后丢弃；
        // `Pass` = 直通。链为 None（legacy 调用方/未装配）时语义不变。
        if let Some(ref chain) = inbound_filters {
            match chain.run(&inbound).await {
                nemesis_bus::FilterDecision::Intercepted => {
                    // 拦截方负责应答（可观测性契约：FilterChain::run 已记 info）。
                    continue;
                }
                nemesis_bus::FilterDecision::Rejected(reason) => {
                    tracing::warn!(
                        session_id = %msg.session_id,
                        chat_id = %inbound.chat_id,
                        reason = %reason,
                        "[WebServer] Inbound message rejected by filter chain"
                    );
                    bus.publish_outbound(nemesis_types::channel::OutboundMessage::new(
                        "web",
                        &inbound.chat_id,
                        &reason,
                    ));
                    continue;
                }
                nemesis_bus::FilterDecision::Pass => {}
            }
        }

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
///
/// `model` (optional, `provider/name`) is forwarded into the `receive` frame
/// so the Dashboard can render a per-message "供应商·模型名" badge; `None`
/// omits it (non-assistant / badge-less messages).
///
/// `session_key`（L2，optional）是 agent 会话键——chat_event_log 环形缓冲的
/// 记录键。缺省回退到 `session_id`（连接级 id；旧路径/无元数据帧仍可补拉，
/// 只是跨连接补拉要靠会话键才寻址得到）。
pub async fn send_to_session(
    session_manager: &SessionManager,
    session_id: &str,
    role: &str,
    content: &str,
    model: Option<&str>,
    session_key: Option<&str>,
    source_node: Option<&str>,
) -> Result<(), String> {
    tracing::debug!(
        session_id = %session_id,
        role = %role,
        content_len = content.len(),
        "[WebServer] send_to_session called"
    );

    let mut data = serde_json::json!({
        "role": role,
        "content": content,
    });
    if let Some(m) = model {
        data["model"] = serde_json::Value::String(m.to_string());
    }
    // L6++：帧带 agent 会话 id（session_key=`agent:main:session:{sid}` 前缀
    // 还原）——前端按当前会话过滤 receive 帧：异会话晚到的回复不进当前视图
    // （后端已持久化，切到该会话时从磁盘加载），否则跨组切换时会串台。
    // 无 session_key 的旧路径帧不带该字段，前端保持接受（legacy 兼容）。
    if let Some(sid) = session_key.and_then(|s| s.strip_prefix("agent:main:session:")) {
        data["session_id"] = serde_json::Value::String(sid.to_string());
    }
    // 集群续行归属（2026-09-23）：干活的是远端 worker 节点，前端据此渲染
    // 「节点 X」徽章（与模型徽章并列）。缺省不写键——旧前端零感知。
    if let Some(node) = source_node {
        data["source_node"] = serde_json::Value::String(node.to_string());
    }
    // L2（devtool-upgrade 阶段 6）：chat 帧盖会话内单调 seq 并进 per-session
    // 环形缓冲——`chat.sync {session_id, after_seq}` 断线补拉的数据源。
    // 记录键优先会话键（跨连接稳定），无元数据才退连接 id。
    let record_key = session_key.unwrap_or(session_id);
    let seq = crate::chat_event_log::record(record_key, role, content, model, source_node);
    data["seq"] = serde_json::json!(seq);
    let msg = crate::protocol::ProtocolMessage::new("message", "chat", "receive", Some(data));
    let data = msg
        .to_json()
        .map_err(|e| format!("failed to marshal message: {}", e))?;

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
    let data: serde_json::Value = serde_json::from_str(json_content)
        .map_err(|e| format!("failed to unmarshal history data: {}", e))?;

    let msg = crate::protocol::ProtocolMessage::new("message", "chat", "history", Some(data));
    let bytes = msg
        .to_json()
        .map_err(|e| format!("failed to create protocol message: {}", e))?;

    session_manager
        .broadcast(session_id, &bytes)
        .await
        .map_err(|e| format!("failed to broadcast: {}", e))
}

// ---------------------------------------------------------------------------
// Agent tool-event pump (M1a)
// ---------------------------------------------------------------------------

/// Pump agent tool-events to their consumers: every event is published to the
/// [`EventHub`] (SSE fallback / future subscribers), and events whose
/// `chat_id` maps to a live web session (`web:<session_id>`) are additionally
/// delivered to that session as a WS push frame
/// `{type:"push", cmd:"tool_event", data:{...}}`.
///
/// Runs until the broadcast channel closes (gateway shutdown drops the
/// sender). Lagged receivers skip and continue — events are best-effort
/// observability, never correctness.
pub async fn pump_agent_events(
    mut rx: tokio::sync::broadcast::Receiver<nemesis_types::agent::AgentEvent>,
    session_manager: Arc<SessionManager>,
    event_hub: Arc<EventHub>,
) {
    loop {
        match rx.recv().await {
            Ok(event) => {
                let mut data = match serde_json::to_value(&event) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "[AgentEventPump] event serialization failed; dropped"
                        );
                        continue;
                    }
                };
                // M7（devtool-upgrade 阶段 5）：审批请求走独立 SSE 事件类型
                // `approval-requested` 全局广播（useSSE 白名单订阅）——审批不
                // 属于单一会话，不做 web: 定向 ws push；审批卡渲染在 App 级。
                // BUG-B（2026-09-20）：前端契约是 AgentEvent 的**内层 data**
                // （useApprovals.upsertFromPayload 直接读 request_id 等平铺
                // 字段），此处曾把 adjacently tagged 整体
                // `{"kind":..,"data":{..}}` 原样 publish——前端首行类型卫
                // 不命中静默 return，审批卡实时不弹（刷新后 seedPending 走
                // WSAPI 平铺 entry_json 才可见，即用户实测「不刷新看不到」）。
                // 与下方 session.created 分支的平铺先例对齐，取内层展平。
                if matches!(
                    event,
                    nemesis_types::agent::AgentEvent::ApprovalRequested { .. }
                ) {
                    event_hub.publish(
                        "approval-requested",
                        data.get("data").cloned().unwrap_or(data),
                    );
                    continue;
                }
                // F6（devtool-upgrade 阶段 5）：裁决结果同走全局广播——所有
                // 前端窗口据此摘除本地审批卡（竞速败方不再挂到倒计时结束）。
                if matches!(
                    event,
                    nemesis_types::agent::AgentEvent::ApprovalResolved { .. }
                ) {
                    // BUG-B（2026-09-20）：同 approval-requested，展平内层。
                    event_hub.publish(
                        "approval-resolved",
                        data.get("data").cloned().unwrap_or(data),
                    );
                    continue;
                }
                // F7（devtool-upgrade 阶段 5）：结构化提问同走全局广播——
                // question 工具阻塞等答，QuestionCard 渲染选项卡；了结事件
                // 让所有窗口摘卡（与 approval-resolved 同语义）。
                if matches!(
                    event,
                    nemesis_types::agent::AgentEvent::QuestionAsked { .. }
                ) {
                    // BUG-B（2026-09-20）：同 approval-requested，展平内层
                    // （useQuestions.upsertFromPayload 期望平铺 question_id 等）。
                    event_hub.publish("question-asked", data.get("data").cloned().unwrap_or(data));
                    continue;
                }
                if matches!(
                    event,
                    nemesis_types::agent::AgentEvent::QuestionResolved { .. }
                ) {
                    // BUG-B（2026-09-20）：同族展平（前端 removeLocal 读平铺
                    // question_id）。
                    event_hub.publish(
                        "question-resolved",
                        data.get("data").cloned().unwrap_or(data),
                    );
                    continue;
                }
                // SB（2026-09-17）：会话物化 → SSE `session.created` 全局广播
                // ——前端会话列表 force 刷新（修「隐式会话不进侧栏」）。全局
                // 语义（侧栏是跨会话面），不做 web: 定向 push。
                if let nemesis_types::agent::AgentEvent::SessionCreated { session_id, .. } = &event
                {
                    event_hub.publish(
                        "session.created",
                        serde_json::json!({ "session_id": session_id }),
                    );
                    continue;
                }
                // BUG-A（2026-09-20）：帧内层注入 web 会话 id。chat_id 是
                // 连接级（`web:{连接id}`，session.rs create_session_with_method
                // 派生），与前端会话 id（sessionStore.currentId，create/list
                // 返回的会话 sid）不同域——前端按 `web:${currentId}` 过滤恒
                // 不等，工具卡/任务清单/模式徽标的实时帧全灭（2026-09-20 实
                // 测：服务端帧全数到达浏览器、UI 零反应）。session_key 末段
                // = 发起会话 id（handle_chat_send 把前端 session_id 放
                // metadata，loop 派生 session_key），注入后前端可精确过滤；
                // 无 session_key 的事件不注入（SSE 消费方按字段缺席忽略）。
                if let Some(sk) = event.session_key()
                    && let Some(sid) = sk.rsplit(':').next()
                    && let Some(inner) = data.get_mut("data")
                {
                    inner["session_id"] = serde_json::Value::String(sid.to_string());
                }
                // P1（2026-09-21）：工具事件入 chat_event_log（与 chat 行同
                // 键空间、同 seq 序列）——sync/replay 回放通道据此恢复工具卡
                // （切页/重连/重载后「中间流程」可见，此前工具事件只走本
                // WS 实时 push、无任何持久化）。键取 session_key，与
                // send_to_session 的 record 同域；无 session_key 的事件不落
                // （归属不到前端会话，回放无意义）。载荷存帧 data 原样
                // （含上方注入的 session_id）——回放端复用实时帧同款处理。
                // P8 精修：seq 注入实时帧（record_tool 返回值）——前端对
                // chat 行与 tool 条目统一推进补拉游标，chat.activity 信号
                // 的 seq ≤ 游标短路才能生效（否则本端每个工具事件都被误判
                // 「落后」触发无谓全量刷新）。存环条目不注入 seq（回放端
                // 用 ChatEvent.seq，实时帧多出的字段回放端不读）。
                if let Some(sk) = event.session_key() {
                    let tool_seq = crate::chat_event_log::record_tool(sk, data.clone());
                    data["seq"] = serde_json::Value::from(tool_seq);
                }
                event_hub.publish("tool_event", data.clone());

                let chat_id = event.chat_id().to_string();
                if let Some(session_id) = chat_id.strip_prefix("web:") {
                    // BUG 2026-09-22 慢客户端洪泛：带 seq = 已落
                    // chat_event_log 环，丢帧可被前端 chat.sync 补拉自愈
                    // → 走可丢通道（满即丢，事件泵绝不被单个慢客户端的
                    // 满队列阻塞、连坐全部会话）；无 seq（未落环、不可
                    // 补拉）保留必达通道（低频，非洪泛源）。
                    let droppable = data.get("seq").is_some();
                    let frame = crate::protocol::ProtocolMessage::new(
                        "push",
                        "chat",
                        "tool_event",
                        Some(data),
                    );
                    match frame.to_json() {
                        Ok(bytes) => {
                            let result = if droppable {
                                session_manager.broadcast_droppable(session_id, &bytes)
                            } else {
                                session_manager.broadcast(session_id, &bytes).await
                            };
                            if let Err(e) = result {
                                tracing::debug!(
                                    session_id = %session_id,
                                    error = %e,
                                    "[AgentEventPump] session broadcast failed (session gone?)"
                                );
                            }
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "[AgentEventPump] frame encode failed");
                        }
                    }
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(skipped = n, "[AgentEventPump] lagged; events skipped");
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                tracing::info!("[AgentEventPump] channel closed; pump exiting");
                break;
            }
        }
    }
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
                tracing::debug!(
                    "[WebServer] Status publish loop stopping (server no longer running)"
                );
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
pub async fn dispatch_outbound(bus: Arc<MessageBus>, session_manager: Arc<SessionManager>) {
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
                    send_to_session(
                        &session_manager,
                        session_id,
                        "assistant",
                        &msg.content,
                        msg.meta.model.as_deref(),
                        msg.meta.session_key.as_deref(),
                        msg.meta.source_node.as_deref(),
                    )
                    .await
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
// F1（2026-09-22 审计修复）：REST 控制面统一鉴权
// ---------------------------------------------------------------------------

/// 统一鉴权豁免路径（[`auth_middleware`] 内短路）。
/// - `/health`、`/api/health`：探活——监控场景无凭据可达是健康检查语义的一部分；
/// - `/api/share/`：L4 会话分享——token 即凭据（`share.rs` 模块头注释）；
/// - `/api/board/asset/`：看板资产下载——asset_token 即凭据（`handlers/board_asset.rs` 模块头注释）；
/// - `/api/workflow/chat/`：独立 workflow-chat 页的公共元数据 + 密码校验
///   （`handlers/workflow.rs` 注册处注释自认 unauthenticated，凭据是 per-workflow 密码）。
fn auth_exempt_path(path: &str) -> bool {
    path == "/health"
        || path == "/api/health"
        || path.starts_with("/api/share/")
        || path.starts_with("/api/board/asset/")
        || path.starts_with("/api/workflow/chat/")
}

/// 从请求提取 token：`X-Auth-Token` 头 → `?token=` 查询参数 →
/// `Authorization: Bearer`。查询参数兜底 EventSource（SSE 无法携带自定义头）。
fn extract_request_token(req: &axum::extract::Request) -> Option<String> {
    if let Some(v) = req
        .headers()
        .get("x-auth-token")
        .and_then(|v| v.to_str().ok())
    {
        return Some(v.to_string());
    }
    #[derive(serde::Deserialize)]
    struct TokenQuery {
        token: Option<String>,
    }
    if let Ok(q) = axum::extract::Query::<TokenQuery>::try_from_uri(req.uri())
        && let Some(t) = q.token.as_deref()
        && !t.is_empty()
    {
        return Some(t.to_string());
    }
    if let Some(v) = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        && let Some(t) = v.strip_prefix("Bearer ")
    {
        return Some(t.trim().to_string());
    }
    None
}

/// 统一 REST 鉴权中间件（`build_router` 里 `route_layer` 挂载，覆盖其上全部
/// 已注册路由）。空 expected token 恒放行（`verify_token` 文档化约定——默认
/// 部署与既有测试零影响）；非空 token 时 REST 与 `/ws` 信任边界一致（此前
/// token 只保护 `/ws`，`/api/config`、`/api/chat/stream` 等全部裸奔）。
/// workflow-chat 的 WS 升级（`<ws_path>?workflow_chat=&pwd=`）自带
/// per-workflow 密码闸（`websocket_handler`），不持 dashboard token——**只对
/// WS 路径放行**；其余路径带 `workflow_chat=` 照常鉴权（否则构成查询参数
/// 旁路，任何端点拼上该键即可绕过整个 auth 闸）。
pub(crate) async fn auth_middleware(
    AxumState(state): AxumState<Arc<AppState>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
    ws_path: &str,
) -> axum::response::Response {
    let is_workflow_chat_upgrade = req.uri().path() == ws_path
        && req
            .uri()
            .query()
            .map(|q| q.split('&').any(|p| p.starts_with("workflow_chat=")))
            .unwrap_or(false);
    if is_workflow_chat_upgrade || auth_exempt_path(req.uri().path()) {
        return next.run(req).await;
    }
    let token = extract_request_token(&req).unwrap_or_default();
    if crate::api_handlers::verify_token(&token, &state.auth_token) {
        next.run(req).await
    } else {
        tracing::warn!(
            path = %req.uri().path(),
            "[WebServer] REST request rejected: missing or invalid auth token"
        );
        (
            axum::http::StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "unauthorized" })),
        )
            .into_response()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(all(test, feature = "workflow"))]
mod tests;

// M1a (2026-09-05): tool-event pump routing tests (no workflow dependency).
#[cfg(test)]
mod agent_event_pump_tests;

#[cfg(all(test, feature = "workflow"))]
mod extra_tests;

// F1（2026-09-22 审计修复）：统一鉴权中间件测试（无 workflow 依赖）。
#[cfg(test)]
mod auth_tests;

// R4 覆盖率（2026-08-27）：workflow/chat/ 路径前缀静态壳 + bind-failed 错误路径。
#[cfg(test)]
mod r4_tests;

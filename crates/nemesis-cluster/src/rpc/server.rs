//! RPC server - listens for incoming RPC requests from peers.
//!
//! Accepts TCP connections, optionally authenticates them, reads framed
//! RPC requests, routes them to registered handlers, and sends responses.
//! Mirrors Go's `rpc.Server` with auth token validation, connection
//! management, and handler registration per action.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use parking_lot::RwLock;
use tokio::net::TcpListener;
use tokio::sync::broadcast;

use crate::transport::conn::{TcpConn, TcpConnConfig, WireMessage};

// ---------------------------------------------------------------------------
// Handler trait
// ---------------------------------------------------------------------------

/// Handler function type for RPC actions.
/// Takes a JSON payload and returns a JSON result or error string.
pub type RpcHandlerFn = Box<dyn Fn(serde_json::Value) -> Result<serde_json::Value, String> + Send + Sync>;

// ---------------------------------------------------------------------------
// RpcServerConfig
// ---------------------------------------------------------------------------

/// Configuration for the RPC server.
#[derive(Debug, Clone)]
pub struct RpcServerConfig {
    /// Address to bind to (e.g. "0.0.0.0:21949").
    pub bind_address: String,
    /// Maximum number of concurrent connections.
    pub max_connections: usize,
    /// Timeout for sending responses.
    pub send_timeout: std::time::Duration,
    /// Idle connection timeout.
    pub idle_timeout: std::time::Duration,
}

impl Default for RpcServerConfig {
    fn default() -> Self {
        Self {
            bind_address: "0.0.0.0:21949".into(),
            max_connections: 100,
            send_timeout: std::time::Duration::from_secs(10),
            idle_timeout: std::time::Duration::from_secs(65 * 60),
        }
    }
}

// ---------------------------------------------------------------------------
// RpcServer
// ---------------------------------------------------------------------------

/// RPC server that processes incoming cluster requests over TCP.
///
/// Features:
/// - Binds a TCP listener on the configured address
/// - Authenticates connections with an optional auth token
/// - Routes requests to registered action handlers
/// - Manages connection lifecycle
pub struct RpcServer {
    config: RpcServerConfig,
    /// Handler map wrapped in Arc<RwLock> so that the accept loop and
    /// per-connection tasks share the **same** live map. Dynamic registration
    /// via `register_handler` is immediately visible to in-flight requests
    /// (matching Go's per-request handler map lookup).
    handlers: Arc<RwLock<HashMap<String, Arc<RpcHandlerFn>>>>,
    running: RwLock<bool>,
    auth_token: RwLock<String>,
    listener_port: RwLock<u16>,
    shutdown_tx: broadcast::Sender<()>,
    conn_count: Arc<AtomicUsize>,
    /// Handle to the accept loop task — aborted on stop() to release the TCP port immediately.
    accept_handle: RwLock<Option<tokio::task::JoinHandle<()>>>,
}

impl RpcServer {
    /// Create a new RPC server with the given configuration.
    pub fn new(config: RpcServerConfig) -> Self {
        let (shutdown_tx, _) = broadcast::channel(1);
        let server = Self {
            config,
            handlers: Arc::new(RwLock::new(HashMap::new())),
            running: RwLock::new(false),
            auth_token: RwLock::new(String::new()),
            listener_port: RwLock::new(0),
            shutdown_tx,
            conn_count: Arc::new(AtomicUsize::new(0)),
            accept_handle: RwLock::new(None),
        };
        // Register defaults in constructor, not in start().
        // On first start: basic_handlers overwrite defaults → custom handlers overwrite defaults.
        // On restart: start() doesn't touch handlers → all custom handlers survive.
        server.register_default_handlers();
        server
    }

    /// Set the authentication token for RPC connections.
    /// When set, all connections must send the token as the first line.
    pub fn set_auth_token(&self, token: &str) {
        *self.auth_token.write() = token.to_string();
    }

    /// Register a handler for a specific RPC action.
    pub fn register_handler(&self, action: &str, handler: RpcHandlerFn) {
        tracing::info!(
            action = action,
            "[RpcServer] Handler registered for action: {}",
            action,
        );
        self.handlers.write().insert(action.to_string(), Arc::from(handler));
    }

    /// Unregister a handler for a specific action.
    pub fn unregister_handler(&self, action: &str) {
        tracing::info!(
            action = action,
            "[RpcServer] Handler unregistered for action: {}",
            action,
        );
        self.handlers.write().remove(action);
    }

    /// Start the RPC server. Binds a TCP listener and starts the accept loop.
    pub async fn start(&self) -> Result<(), String> {
        {
            let running = self.running.read();
            if *running {
                return Err("server already running".into());
            }
        }

        // Bind TCP listener with SO_REUSEADDR to allow quick restarts.
        let socket = tokio::net::TcpSocket::new_v4()
            .map_err(|e| format!("failed to create socket: {}", e))?;
        socket.set_reuseaddr(true)
            .map_err(|e| format!("failed to set SO_REUSEADDR: {}", e))?;
        let addr: std::net::SocketAddr = self.config.bind_address.parse()
            .map_err(|e| format!("invalid bind address '{}': {}", self.config.bind_address, e))?;
        socket.bind(addr)
            .map_err(|e| format!("failed to bind {}: {}", self.config.bind_address, e))?;
        let listener = socket.listen(128)
            .map_err(|e| format!("failed to listen on {}: {}", self.config.bind_address, e))?;

        let actual_port = listener.local_addr()
            .map_err(|e| format!("failed to get local addr: {}", e))?
            .port();

        *self.listener_port.write() = actual_port;
        *self.running.write() = true;

        tracing::info!(
            bind_address = %self.config.bind_address,
            port = actual_port,
            "[RpcServer] RPC server started"
        );

        // Start accept loop
        let shutdown_rx = self.shutdown_tx.subscribe();
        // Clone the Arc to share the live handler map with the accept loop.
        // Dynamic registration via `register_handler` is immediately visible.
        let handlers = Arc::clone(&self.handlers);
        let auth_token = self.auth_token.read().clone();
        let max_conns = self.config.max_connections;
        let conn_count = Arc::clone(&self.conn_count);
        let idle_timeout = self.config.idle_timeout;

        let handle = tokio::spawn(Self::accept_loop(
            listener,
            shutdown_rx,
            handlers,
            auth_token,
            max_conns,
            conn_count,
            idle_timeout,
        ));
        *self.accept_handle.write() = Some(handle);

        Ok(())
    }

    /// Stop the RPC server.
    ///
    /// Sends a shutdown signal and aborts the accept loop task to release
    /// the TCP port immediately (critical for clean restart).
    pub fn stop(&self) -> Result<(), String> {
        {
            let running = self.running.read();
            if !*running {
                return Err("server not running".into());
            }
        }
        *self.running.write() = false;
        let _ = self.shutdown_tx.send(());
        // Abort the accept loop task — this immediately drops the TcpListener,
        // releasing the port. Without this, the async task may not have processed
        // the shutdown signal yet when start() tries to rebind.
        if let Some(handle) = self.accept_handle.write().take() {
            handle.abort();
        }
        tracing::info!("[RpcServer] RPC server stopped");
        Ok(())
    }

    /// Check if the server is running.
    pub fn is_running(&self) -> bool {
        *self.running.read()
    }

    /// Get the actual port the server is listening on.
    pub fn port(&self) -> u16 {
        *self.listener_port.read()
    }

    /// Get the number of active connections.
    pub fn connection_count(&self) -> usize {
        self.conn_count.load(Ordering::Relaxed)
    }

    /// Get the bind address.
    pub fn bind_address(&self) -> &str {
        &self.config.bind_address
    }

    // -----------------------------------------------------------------------
    // Accept loop
    // -----------------------------------------------------------------------

    async fn accept_loop(
        listener: TcpListener,
        mut shutdown_rx: broadcast::Receiver<()>,
        handlers: Arc<RwLock<HashMap<String, Arc<RpcHandlerFn>>>>,
        auth_token: String,
        max_conns: usize,
        conn_count: Arc<AtomicUsize>,
        idle_timeout: std::time::Duration,
    ) {
        loop {
            tokio::select! {
                accept_result = listener.accept() => {
                    match accept_result {
                        Ok((stream, remote_addr)) => {
                            let current = conn_count.load(Ordering::Relaxed);
                            if current >= max_conns {
                                tracing::warn!(
                                    remote = %remote_addr,
                                    "[RpcServer] Rejecting connection: max_conns reached"
                                );
                                drop(stream);
                                continue;
                            }

                            tracing::info!(remote = %remote_addr, "[RpcServer] Accepted RPC connection");

                            let handlers = Arc::clone(&handlers);
                            let auth_token = auth_token.clone();
                            let conn_count = Arc::clone(&conn_count);
                            conn_count.fetch_add(1, Ordering::Relaxed);

                            tokio::spawn(async move {
                                Self::handle_connection(
                                    stream,
                                    remote_addr.to_string(),
                                    handlers,
                                    auth_token,
                                    idle_timeout,
                                )
                                .await;
                                conn_count.fetch_sub(1, Ordering::Relaxed);
                            });
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "[RpcServer] Accept error");
                        }
                    }
                }
                _ = shutdown_rx.recv() => {
                    tracing::info!("[RpcServer] RPC server accept loop shutting down");
                    break;
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Connection handling
    // -----------------------------------------------------------------------

    async fn handle_connection(
        stream: tokio::net::TcpStream,
        remote_addr: String,
        handlers: Arc<RwLock<HashMap<String, Arc<RpcHandlerFn>>>>,
        auth_token: String,
        _idle_timeout: std::time::Duration,
    ) {
        // Auth is enforced via AEAD frame encryption: when `auth_token` is
        // non-empty, TcpConn derives an AES-256-GCM key from it and decrypts
        // every inbound frame. A peer that does not share the token cannot
        // produce frames with a valid GCM tag, so its first byte stream is
        // rejected as a decrypt error in the read loop. No text-line auth
        // handshake is performed, eliminating the BufReader desync bug where
        // `read_line('\n')` could consume frame bytes past the newline.
        let config = TcpConnConfig {
            address: remote_addr.clone(),
            auth_token: if !auth_token.is_empty() {
                Some(auth_token)
            } else {
                None
            },
            ..Default::default()
        };

        let mut conn = TcpConn::new(stream, config);

        if let Err(e) = conn.start().await {
            tracing::error!(remote = %remote_addr, error = %e, "[RpcServer] Failed to start TcpConn");
            return;
        }

        // Process incoming messages — frames are already decrypted by TcpConn's
        // read loop, so `wire_msg` is the plaintext WireMessage.
        loop {
            match conn.receive().await {
                Some(wire_msg) => {
                    if wire_msg.msg_type == "request" {
                        Self::handle_request(&conn, &wire_msg, &handlers).await;
                    }
                }
                None => {
                    tracing::debug!(remote = %remote_addr, "[RpcServer] Connection closed");
                    break;
                }
            }
        }

        conn.close();
    }

    // -----------------------------------------------------------------------
    // Request handling
    // -----------------------------------------------------------------------

    async fn handle_request(
        conn: &TcpConn,
        wire_msg: &WireMessage,
        handlers: &RwLock<HashMap<String, Arc<RpcHandlerFn>>>,
    ) {
        let action = &wire_msg.action;
        tracing::info!(
            action = %action,
            from = %wire_msg.from,
            id = %wire_msg.id,
            "[RpcServer] Received RPC request"
        );

        // Parse payload as JSON value
        let mut payload = wire_msg.payload.clone();

        // Inject _rpc metadata before passing to handlers (matching Go's enhancePayload)
        if let Some(obj) = payload.as_object_mut() {
            let rpc_meta = serde_json::json!({
                "from": wire_msg.from,
                "to": wire_msg.to,
                "id": wire_msg.id,
            });
            obj.insert("_rpc".to_string(), rpc_meta);
        } else {
            // If payload is not an object, wrap it
            let mut map = serde_json::Map::new();
            map.insert("_rpc".to_string(), serde_json::json!({
                "from": wire_msg.from,
                "to": wire_msg.to,
                "id": wire_msg.id,
            }));
            payload = serde_json::Value::Object(map);
        }

        // Look up handler — read-lock the live map so dynamically registered
        // handlers are immediately visible (matches Go's per-request lookup).
        // The read guard is explicitly dropped before any `.await` so the
        // future remains Send-safe.
        let no_handler;
        let handler = {
            let guard = handlers.read();
            match guard.get(action).cloned() {
                Some(h) => {
                    no_handler = false;
                    h
                }
                None => {
                    no_handler = true;
                    Arc::new(Box::new(|_payload| Ok(serde_json::Value::Null))
                        as RpcHandlerFn)
                }
            }
        }; // guard dropped here

        if no_handler {
            tracing::warn!(
                action = %action,
                from = %wire_msg.from,
                id = %wire_msg.id,
                "[RpcServer] No handler for action: {}",
                action,
            );
            let resp = WireMessage::new_error(
                wire_msg,
                &format!("no handler for action: {}", action),
            );
            let _ = conn.send(&resp).await;
            return;
        }

        // Execute handler
        let result = handler(payload);

        // Send response
        match result {
            Ok(value) => {
                tracing::info!(
                    action = %action,
                    from = %wire_msg.from,
                    id = %wire_msg.id,
                    "[RpcServer] Request handled successfully: action={}, from={}",
                    action,
                    wire_msg.from,
                );
                let resp = WireMessage::new_response(wire_msg, value);
                if let Err(e) = conn.send(&resp).await {
                    tracing::error!(error = %e, "[RpcServer] Failed to send response");
                }
            }
            Err(err) => {
                tracing::warn!(
                    action = %action,
                    from = %wire_msg.from,
                    id = %wire_msg.id,
                    error = %err,
                    "[RpcServer] Request handler returned error: action={}, from={}",
                    action,
                    wire_msg.from,
                );
                let resp = WireMessage::new_error(wire_msg, &err);
                if let Err(e) = conn.send(&resp).await {
                    tracing::error!(error = %e, "[RpcServer] Failed to send error response");
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Default handlers
    // -----------------------------------------------------------------------

    fn register_default_handlers(&self) {
        // Ping handler (lowercase to match Go's action names)
        self.register_handler("ping", Box::new(|_payload| {
            Ok(serde_json::json!({"status": "pong"}))
        }));

        // GetInfo handler
        self.register_handler("get_info", Box::new(|_payload| {
            Ok(serde_json::json!({
                "version": env!("CARGO_PKG_VERSION"),
                "status": "online",
            }))
        }));

        // GetCapabilities handler
        self.register_handler("get_capabilities", Box::new(|_payload| {
            Ok(serde_json::json!({
                "capabilities": ["cluster", "rpc"],
            }))
        }));

        // ListActions handler
        self.register_handler("list_actions", Box::new(|_payload| {
            Ok(serde_json::json!({
                "actions": ["ping", "get_info", "get_capabilities", "list_actions", "peer_chat", "peer_chat_callback"],
            }))
        }));

        // peer_chat default handler — placeholder that returns an ACK.
        //
        // **这不是遗漏。** 此默认桩会被两层覆盖：
        //   1. Cluster 启动时 `register_peer_chat_handlers()`（cluster.rs:1630）调用
        //      `build_peer_chat_handler()` 注册 ACK 桩，覆盖此默认桩。
        //   2. **gateway.rs:1189 用真正的 PeerChatHandler 再次覆盖**，做完整工作：
        //      提取 `payload._rpc.from`、自动注册未知节点、入队 ClusterTaskList 异步处理。
        //
        // gateway 通过 `cluster.register_rpc_handler("peer_chat", ...)` 注册（不是直接
        // 调 `rpc_server.register_handler`），cluster 内部转发到 server。此默认桩仅在
        // 非 gateway 场景（轻量 cluster node）下生效。
        // **修改 peer_chat 行为的正确位置是 gateway.rs:1189，不是这里。**
        // 对应 Go 版本的 `registerPeerChatHandlers`，由 `SetRPCChannel` 在就绪后注册。
        self.register_handler("peer_chat", Box::new(|payload| {
            let task_id = payload
                .get("task_id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            tracing::info!(task_id = %task_id, "[RpcServer] peer_chat request received (default handler)");
            Ok(serde_json::json!({
                "status": "accepted",
                "task_id": task_id,
            }))
        }));

        // peer_chat_callback default handler — placeholder that acknowledges receipt.
        //
        // **这不是遗漏。** 此默认桩会被两层覆盖：
        //   1. Cluster 启动时 `register_peer_chat_handlers()`（cluster.rs:1635）调用
        //      `build_callback_handler()` 注册桩，覆盖此默认桩。
        //   2. **gateway.rs:1250 用真正的 callback 路由 handler 再次覆盖**：路由到
        //      ClusterAgent（嵌套 cluster_rpc）、续行快照（cluster_continuation bus
        //      消息触发 AgentLoop 续行）、TaskManager（dashboard 发起的 peer_chat）。
        //
        // 此默认桩仅在非 gateway 场景（轻量 cluster node）下生效。
        // **修改 peer_chat_callback 行为的正确位置是 gateway.rs:1250，不是这里。**
        // 对应 Go 版本中 `SetRPCChannel` 注册的 callback handler。
        self.register_handler("peer_chat_callback", Box::new(|payload| {
            let task_id = payload
                .get("task_id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            tracing::info!(task_id = %task_id, "[RpcServer] peer_chat_callback received (default handler)");
            Ok(serde_json::json!({
                "status": "received",
                "task_id": task_id,
            }))
        }));
    }

    // -----------------------------------------------------------------------
    // Synchronous frame-based handler (for backward compat and testing)
    // -----------------------------------------------------------------------

    /// Process a raw request synchronously. Used for testing without TCP.
    pub fn handle_request_sync(
        &self,
        action: &str,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let handlers = self.handlers.read();
        let handler = match handlers.get(action) {
            Some(h) => Arc::clone(h),
            None => {
                tracing::warn!(
                    action = action,
                    "[RpcServer] No handler for sync request: action={}",
                    action,
                );
                return Err(format!("no handler for action: {}", action));
            }
        };
        drop(handlers);
        handler(payload)
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests;

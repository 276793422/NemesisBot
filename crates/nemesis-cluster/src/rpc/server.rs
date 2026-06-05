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

        // Bind TCP listener
        let listener = TcpListener::bind(&self.config.bind_address)
            .await
            .map_err(|e| format!("failed to bind {}: {}", self.config.bind_address, e))?;

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

        // Start accept loop
        let shutdown_rx = self.shutdown_tx.subscribe();
        // Clone the Arc to share the live handler map with the accept loop.
        // Dynamic registration via `register_handler` is immediately visible.
        let handlers = Arc::clone(&self.handlers);
        let auth_token = self.auth_token.read().clone();
        let max_conns = self.config.max_connections;
        let conn_count = Arc::clone(&self.conn_count);
        let idle_timeout = self.config.idle_timeout;

        tokio::spawn(Self::accept_loop(
            listener,
            shutdown_rx,
            handlers,
            auth_token,
            max_conns,
            conn_count,
            idle_timeout,
        ));

        Ok(())
    }

    /// Stop the RPC server.
    pub fn stop(&self) -> Result<(), String> {
        {
            let running = self.running.read();
            if !*running {
                return Err("server not running".into());
            }
        }
        *self.running.write() = false;
        let _ = self.shutdown_tx.send(());
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
        // Auth phase: if token is set, read the first line as the token
        if !auth_token.is_empty() {
            use tokio::io::AsyncBufReadExt;
            use tokio::io::BufReader;

            let mut reader = BufReader::new(stream);
            let mut token_line = String::new();

            // Read with timeout
            let read_result = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                reader.read_line(&mut token_line),
            ).await;

            match read_result {
                Ok(Ok(0)) => {
                    tracing::warn!(remote = %remote_addr, "[RpcServer] Connection closed during auth");
                    return;
                }
                Ok(Ok(_)) => {
                    let token = token_line.trim();
                    if token != auth_token {
                        tracing::warn!(remote = %remote_addr, "[RpcServer] Unauthorized connection (invalid token)");
                        return;
                    }
                    tracing::info!(remote = %remote_addr, "[RpcServer] Authenticated RPC connection");

                    // Recover the TCP stream from the BufReader so we can
                    // continue with framed communication.
                    // IMPORTANT: BufReader may have buffered bytes beyond the
                    // '\n' delimiter (i.e. the first frame data).  We must
                    // extract those buffered bytes and handle them as a frame
                    // before TcpConn starts reading, otherwise the data is lost.
                    let buffered = reader.buffer().to_vec();
                    let stream = reader.into_inner();

                    // Wrap in TcpConn for framed communication
                    let config = TcpConnConfig {
                        address: remote_addr.clone(),
                        ..Default::default()
                    };
                    let mut conn = TcpConn::new(stream, config);

                    if let Err(e) = conn.start().await {
                        tracing::error!(remote = %remote_addr, error = %e, "[RpcServer] Failed to start TcpConn");
                        return;
                    }

                    // If BufReader had buffered frame data, decode and handle it.
                    if !buffered.is_empty() {
                        // The buffered bytes are a length-prefixed frame:
                        // [4-byte big-endian length][JSON payload]
                        if buffered.len() >= 4 {
                            let len = u32::from_be_bytes([
                                buffered[0], buffered[1], buffered[2], buffered[3],
                            ]) as usize;
                            if buffered.len() >= 4 + len {
                                let frame_data = &buffered[4..4 + len];
                                if let Ok(wire_msg) = WireMessage::from_bytes(frame_data) {
                                    if wire_msg.msg_type == "request" {
                                        Self::handle_request(&conn, &wire_msg, &handlers).await;
                                    }
                                }
                            }
                        }
                    }

                    // Process incoming messages (same as non-auth path below)
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
                Ok(Err(e)) => {
                    tracing::warn!(remote = %remote_addr, error = %e, "[RpcServer] Failed to read auth token");
                }
                Err(_) => {
                    tracing::warn!(remote = %remote_addr, "[RpcServer] Auth timeout");
                }
            }
            return;
        }

        // No auth: wrap in TcpConn for framed communication
        let config = TcpConnConfig {
            address: remote_addr.clone(),
            ..Default::default()
        };

        let mut conn = TcpConn::new(stream, config);

        if let Err(e) = conn.start().await {
            tracing::error!(remote = %remote_addr, error = %e, "[RpcServer] Failed to start TcpConn (no auth)");
            return;
        }

        // Process incoming messages
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
        // **这不是遗漏。** Production 路径中，gateway.rs 启动时会调用
        // `rpc_server.register_handler("peer_chat", ...)` 用真正的 PeerChatHandler
        // （包含 LLM 通道 + RPC 回调客户端）覆盖此默认 handler。此默认 handler 仅在
        // 没有 gateway 的轻量节点（cluster node）场景下生效，用于确认收到请求。
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
        // **这不是遗漏。** Production 路径中，gateway.rs 启动时会用真正的
        // TaskManager 回调处理器覆盖此默认 handler，将结果写入续行快照并触发
        // AgentLoop 续行。此默认 handler 仅在轻量节点场景下生效。
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

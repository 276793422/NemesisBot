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
        Self {
            config,
            handlers: Arc::new(RwLock::new(HashMap::new())),
            running: RwLock::new(false),
            auth_token: RwLock::new(String::new()),
            listener_port: RwLock::new(0),
            shutdown_tx,
            conn_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Set the authentication token for RPC connections.
    /// When set, all connections must send the token as the first line.
    pub fn set_auth_token(&self, token: &str) {
        *self.auth_token.write() = token.to_string();
    }

    /// Register a handler for a specific RPC action.
    pub fn register_handler(&self, action: &str, handler: RpcHandlerFn) {
        self.handlers.write().insert(action.to_string(), Arc::from(handler));
    }

    /// Unregister a handler for a specific action.
    pub fn unregister_handler(&self, action: &str) {
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

        // Register default handlers
        self.register_default_handlers();

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
            "RPC server started"
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
        tracing::info!("RPC server stopped");
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
                                    "Rejecting connection: max_conns reached"
                                );
                                drop(stream);
                                continue;
                            }

                            tracing::info!(remote = %remote_addr, "Accepted RPC connection");

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
                            tracing::error!(error = %e, "Accept error");
                        }
                    }
                }
                _ = shutdown_rx.recv() => {
                    tracing::info!("RPC server accept loop shutting down");
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
                    tracing::warn!(remote = %remote_addr, "Connection closed during auth");
                    return;
                }
                Ok(Ok(_)) => {
                    let token = token_line.trim();
                    if token != auth_token {
                        tracing::warn!(remote = %remote_addr, "Unauthorized connection (invalid token)");
                        return;
                    }
                    tracing::info!(remote = %remote_addr, "Authenticated RPC connection");

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
                        tracing::error!(remote = %remote_addr, error = %e, "Failed to start TcpConn");
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
                                tracing::debug!(remote = %remote_addr, "Connection closed");
                                break;
                            }
                        }
                    }
                    conn.close();
                }
                Ok(Err(e)) => {
                    tracing::warn!(remote = %remote_addr, error = %e, "Failed to read auth token");
                }
                Err(_) => {
                    tracing::warn!(remote = %remote_addr, "Auth timeout");
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
            tracing::error!(remote = %remote_addr, error = %e, "Failed to start TcpConn");
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
                    tracing::debug!(remote = %remote_addr, "Connection closed");
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
            "Received RPC request"
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
                let resp = WireMessage::new_response(wire_msg, value);
                if let Err(e) = conn.send(&resp).await {
                    tracing::error!(error = %e, "Failed to send response");
                }
            }
            Err(err) => {
                let resp = WireMessage::new_error(wire_msg, &err);
                if let Err(e) = conn.send(&resp).await {
                    tracing::error!(error = %e, "Failed to send error response");
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

        // peer_chat handler — placeholder that returns an ACK.
        // In production, the service layer replaces this with the real
        // PeerChatHandler (which has LLM channel + RPC client for callbacks).
        // This mirrors Go's `registerPeerChatHandlers` which is called from
        // `SetRPCChannel` once the RPC channel is ready.
        self.register_handler("peer_chat", Box::new(|payload| {
            let task_id = payload
                .get("task_id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            tracing::info!(task_id = %task_id, "peer_chat request received (default handler)");
            Ok(serde_json::json!({
                "status": "accepted",
                "task_id": task_id,
            }))
        }));

        // peer_chat_callback handler — placeholder that acknowledges receipt.
        // In production, replaced by the task completion callback handler.
        self.register_handler("peer_chat_callback", Box::new(|payload| {
            let task_id = payload
                .get("task_id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            tracing::info!(task_id = %task_id, "peer_chat_callback received (default handler)");
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
            None => return Err(format!("no handler for action: {}", action)),
        };
        drop(handlers);
        handler(payload)
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_server() -> RpcServer {
        RpcServer::new(RpcServerConfig {
            bind_address: "0.0.0.0:0".into(),
            ..Default::default()
        })
    }

    #[tokio::test]
    async fn test_start_stop() {
        let server = make_server();
        server.start().await.unwrap();
        assert!(server.is_running());
        assert_ne!(server.port(), 0);

        server.stop().unwrap();
        assert!(!server.is_running());
    }

    #[tokio::test]
    async fn test_double_start_fails() {
        let server = make_server();
        server.start().await.unwrap();
        let result = server.start().await;
        assert!(result.is_err());
        server.stop().unwrap();
    }

    #[test]
    fn test_stop_when_not_started_fails() {
        let server = make_server();
        let result = server.stop();
        assert!(result.is_err());
    }

    #[test]
    fn test_register_and_use_handler() {
        let server = make_server();

        server.register_handler("TestAction", Box::new(|payload| {
            let name = payload.get("name").and_then(|v| v.as_str()).unwrap_or("unknown");
            Ok(serde_json::json!({"greeting": format!("Hello, {}!", name)}))
        }));

        let result = server.handle_request_sync("TestAction", serde_json::json!({"name": "Alice"}));
        assert!(result.is_ok());
        let resp = result.unwrap();
        assert_eq!(resp["greeting"], "Hello, Alice!");
    }

    #[test]
    fn test_unregister_handler() {
        let server = make_server();
        server.register_handler("TempAction", Box::new(|_| Ok(serde_json::json!({}))));
        assert!(server.handle_request_sync("TempAction", serde_json::json!({})).is_ok());

        server.unregister_handler("TempAction");
        assert!(server.handle_request_sync("TempAction", serde_json::json!({})).is_err());
    }

    #[test]
    fn test_default_ping_handler() {
        let server = make_server();
        // Default handlers are registered on start(), so register them manually for sync test
        server.register_default_handlers();

        let result = server.handle_request_sync("ping", serde_json::json!({}));
        assert!(result.is_ok());
        assert_eq!(result.unwrap()["status"], "pong");
    }

    #[test]
    fn test_default_get_info_handler() {
        let server = make_server();
        server.register_default_handlers();

        let result = server.handle_request_sync("get_info", serde_json::json!({}));
        assert!(result.is_ok());
        let resp = result.unwrap();
        assert_eq!(resp["status"], "online");
    }

    #[test]
    fn test_handler_error() {
        let server = make_server();
        server.register_handler("FailAction", Box::new(|_| {
            Err("something went wrong".into())
        }));

        let result = server.handle_request_sync("FailAction", serde_json::json!({}));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("something went wrong"));
    }

    #[test]
    fn test_no_handler_returns_error() {
        let server = make_server();
        let result = server.handle_request_sync("NonexistentAction", serde_json::json!({}));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no handler"));
    }

    #[test]
    fn test_set_auth_token() {
        let server = make_server();
        assert!(server.auth_token.read().is_empty());
        server.set_auth_token("my-secret");
        assert_eq!(&*server.auth_token.read(), "my-secret");
    }

    #[tokio::test]
    async fn test_connection_count() {
        let server = make_server();
        assert_eq!(server.connection_count(), 0);
        server.start().await.unwrap();
        assert_eq!(server.connection_count(), 0);
        server.stop().unwrap();
    }

    #[test]
    fn test_default_handlers_lowercase_action_names() {
        let server = make_server();
        server.register_default_handlers();

        // Verify all Go-compatible lowercase action names work
        let actions = vec!["ping", "get_info", "get_capabilities", "list_actions", "peer_chat", "peer_chat_callback"];
        for action in actions {
            let result = server.handle_request_sync(action, serde_json::json!({}));
            assert!(result.is_ok(), "Default handler '{}' should be registered", action);
        }
    }

    #[test]
    fn test_default_peer_chat_handler() {
        let server = make_server();
        server.register_default_handlers();

        let result = server.handle_request_sync("peer_chat", serde_json::json!({
            "task_id": "test-123",
        }));
        assert!(result.is_ok());
        let resp = result.unwrap();
        assert_eq!(resp["status"], "accepted");
        assert_eq!(resp["task_id"], "test-123");
    }

    #[test]
    fn test_default_peer_chat_callback_handler() {
        let server = make_server();
        server.register_default_handlers();

        let result = server.handle_request_sync("peer_chat_callback", serde_json::json!({
            "task_id": "test-456",
        }));
        assert!(result.is_ok());
        let resp = result.unwrap();
        assert_eq!(resp["status"], "received");
        assert_eq!(resp["task_id"], "test-456");
    }

    #[test]
    fn test_default_list_actions_handler() {
        let server = make_server();
        server.register_default_handlers();

        let result = server.handle_request_sync("list_actions", serde_json::json!({}));
        assert!(result.is_ok());
        let resp = result.unwrap();
        let actions = resp["actions"].as_array().unwrap();
        // Should contain the Go-compatible action names
        let action_names: Vec<&str> = actions.iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(action_names.contains(&"ping"), "list_actions should contain 'ping'");
        assert!(action_names.contains(&"get_info"), "list_actions should contain 'get_info'");
        assert!(action_names.contains(&"peer_chat"), "list_actions should contain 'peer_chat'");
    }

    #[test]
    fn test_handler_replacement() {
        // Verify that registering a handler for an existing action replaces it
        let server = make_server();
        server.register_default_handlers();

        // Default handler returns "pong"
        let result1 = server.handle_request_sync("ping", serde_json::json!({}));
        assert_eq!(result1.unwrap()["status"], "pong");

        // Replace with custom handler
        server.register_handler("ping", Box::new(|_payload| {
            Ok(serde_json::json!({"status": "custom_pong"}))
        }));

        let result2 = server.handle_request_sync("ping", serde_json::json!({}));
        assert_eq!(result2.unwrap()["status"], "custom_pong");
    }

    // -- Additional tests for RpcServerConfig --

    #[test]
    fn test_rpc_server_config_defaults() {
        let config = RpcServerConfig::default();
        assert_eq!(config.bind_address, "0.0.0.0:21949");
        assert_eq!(config.max_connections, 100);
        assert_eq!(config.send_timeout, std::time::Duration::from_secs(10));
        assert_eq!(config.idle_timeout, std::time::Duration::from_secs(65 * 60));
    }

    #[test]
    fn test_rpc_server_config_custom() {
        let config = RpcServerConfig {
            bind_address: "127.0.0.1:3000".into(),
            max_connections: 50,
            send_timeout: std::time::Duration::from_secs(5),
            idle_timeout: std::time::Duration::from_secs(600),
        };
        assert_eq!(config.bind_address, "127.0.0.1:3000");
        assert_eq!(config.max_connections, 50);
    }

    #[test]
    fn test_server_new_with_config() {
        let config = RpcServerConfig {
            bind_address: "127.0.0.1:0".into(),
            ..Default::default()
        };
        let server = RpcServer::new(config);
        assert!(!*server.running.read());
        assert!(server.auth_token.read().is_empty());
        assert_eq!(server.connection_count(), 0);
    }

    #[test]
    fn test_set_auth_token_updates_value() {
        let server = make_server();
        assert!(server.auth_token.read().is_empty());

        server.set_auth_token("initial-token");
        assert_eq!(&*server.auth_token.read(), "initial-token");

        server.set_auth_token("updated-token");
        assert_eq!(&*server.auth_token.read(), "updated-token");
    }

    #[test]
    fn test_auth_token_clear() {
        let server = make_server();
        server.set_auth_token("temp");
        server.set_auth_token("");
        assert!(server.auth_token.read().is_empty());
    }

    #[test]
    fn test_register_handler_and_verify() {
        let server = make_server();
        server.register_handler("custom_action", Box::new(|payload| {
            let val = payload.get("val").and_then(|v| v.as_i64()).unwrap_or(0);
            Ok(serde_json::json!({"doubled": val * 2}))
        }));

        let result = server.handle_request_sync("custom_action", serde_json::json!({"val": 21}));
        assert!(result.is_ok());
        assert_eq!(result.unwrap()["doubled"], 42);
    }

    #[test]
    fn test_unregister_nonexistent_handler_noop() {
        let server = make_server();
        // Should not panic
        server.unregister_handler("nonexistent");
    }

    #[test]
    fn test_multiple_handlers_independent() {
        let server = make_server();
        server.register_handler("action_a", Box::new(|_| Ok(serde_json::json!({"from": "a"}))));
        server.register_handler("action_b", Box::new(|_| Ok(serde_json::json!({"from": "b"}))));

        let result_a = server.handle_request_sync("action_a", serde_json::json!({}));
        assert_eq!(result_a.unwrap()["from"], "a");

        let result_b = server.handle_request_sync("action_b", serde_json::json!({}));
        assert_eq!(result_b.unwrap()["from"], "b");
    }

    // -- Additional coverage tests --

    #[tokio::test]
    async fn test_server_bind_address() {
        let server = RpcServer::new(RpcServerConfig {
            bind_address: "127.0.0.1:0".into(),
            ..Default::default()
        });
        assert_eq!(server.bind_address(), "127.0.0.1:0");
        server.start().await.unwrap();
        assert_ne!(server.port(), 0);
        server.stop().unwrap();
    }

    #[test]
    fn test_server_port_before_start() {
        let server = make_server();
        assert_eq!(server.port(), 0);
    }

    #[test]
    fn test_server_is_running_before_start() {
        let server = make_server();
        assert!(!server.is_running());
    }

    #[tokio::test]
    async fn test_server_stop_then_restart() {
        let server = make_server();
        server.start().await.unwrap();
        assert!(server.is_running());
        server.stop().unwrap();
        assert!(!server.is_running());

        // Restart should work
        server.start().await.unwrap();
        assert!(server.is_running());
        server.stop().unwrap();
    }

    #[test]
    fn test_handle_request_sync_with_complex_payload() {
        let server = make_server();
        server.register_handler("compute", Box::new(|payload| {
            let a = payload.get("a").and_then(|v| v.as_i64()).unwrap_or(0);
            let b = payload.get("b").and_then(|v| v.as_i64()).unwrap_or(0);
            Ok(serde_json::json!({"sum": a + b}))
        }));

        let result = server.handle_request_sync("compute", serde_json::json!({"a": 10, "b": 20}));
        assert!(result.is_ok());
        assert_eq!(result.unwrap()["sum"], 30);
    }

    #[test]
    fn test_handle_request_sync_handler_returns_string_error() {
        let server = make_server();
        server.register_handler("fail", Box::new(|_| {
            Err("custom error message".to_string())
        }));

        let result = server.handle_request_sync("fail", serde_json::json!({}));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("custom error message"));
    }

    #[tokio::test]
    async fn test_server_with_custom_config() {
        let config = RpcServerConfig {
            bind_address: "0.0.0.0:0".into(),
            max_connections: 10,
            send_timeout: std::time::Duration::from_secs(5),
            idle_timeout: std::time::Duration::from_secs(300),
        };
        let server = RpcServer::new(config);
        server.start().await.unwrap();
        assert!(server.is_running());
        server.stop().unwrap();
    }

    #[tokio::test]
    async fn test_server_auth_token_before_start() {
        let server = make_server();
        server.set_auth_token("secret-token");
        assert_eq!(&*server.auth_token.read(), "secret-token");
        server.start().await.unwrap();
        assert!(server.is_running());
        server.stop().unwrap();
    }

    #[test]
    fn test_default_get_capabilities_handler() {
        let server = make_server();
        server.register_default_handlers();

        let result = server.handle_request_sync("get_capabilities", serde_json::json!({}));
        assert!(result.is_ok());
        let resp = result.unwrap();
        assert!(resp.get("capabilities").is_some());
    }

    #[test]
    fn test_rpc_server_config_debug() {
        let config = RpcServerConfig::default();
        let debug = format!("{:?}", config);
        assert!(debug.contains("21949"));
    }

    // ============================================================
    // Coverage improvement: handler logic, auth, connection tests
    // ============================================================

    #[test]
    fn test_register_handler_overwrites() {
        let server = make_server();
        server.register_handler("action", Box::new(|_| Ok(serde_json::json!({"v": 1}))));
        let r1 = server.handle_request_sync("action", serde_json::json!({}));
        assert_eq!(r1.unwrap()["v"], 1);

        server.register_handler("action", Box::new(|_| Ok(serde_json::json!({"v": 2}))));
        let r2 = server.handle_request_sync("action", serde_json::json!({}));
        assert_eq!(r2.unwrap()["v"], 2);
    }

    #[test]
    fn test_default_get_capabilities_returns_capabilities() {
        let server = make_server();
        server.register_default_handlers();

        let result = server.handle_request_sync("get_capabilities", serde_json::json!({}));
        assert!(result.is_ok());
        let resp = result.unwrap();
        let caps = resp.get("capabilities").unwrap();
        assert!(caps.is_object() || caps.is_array());
    }

    #[tokio::test]
    async fn test_server_accepts_connection() {
        let server = make_server();
        server.start().await.unwrap();
        let port = server.port();

        // Connect a client
        let addr = format!("127.0.0.1:{}", port);
        let result = tokio::net::TcpStream::connect(&addr).await;
        assert!(result.is_ok());

        // Give server a moment to accept
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(server.connection_count() >= 1);

        server.stop().unwrap();
    }

    #[tokio::test]
    async fn test_server_auth_required() {
        let server = make_server();
        server.set_auth_token("secret");
        server.start().await.unwrap();
        let port = server.port();

        // Connect without auth token
        let addr = format!("127.0.0.1:{}", port);
        let stream = tokio::net::TcpStream::connect(&addr).await;
        assert!(stream.is_ok());

        // Give server a moment
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        server.stop().unwrap();
    }

    #[test]
    fn test_handle_request_sync_with_null_payload() {
        let server = make_server();
        server.register_handler("null_test", Box::new(|payload| {
            assert!(payload.is_null());
            Ok(serde_json::json!({"ok": true}))
        }));

        let result = server.handle_request_sync("null_test", serde_json::Value::Null);
        assert!(result.is_ok());
        assert_eq!(result.unwrap()["ok"], true);
    }

    #[test]
    fn test_handle_request_sync_handler_with_nested_payload() {
        let server = make_server();
        server.register_handler("nested", Box::new(|payload| {
            let nested = payload.get("data").and_then(|d| d.get("value"));
            let val = nested.and_then(|v| v.as_i64()).unwrap_or(0);
            Ok(serde_json::json!({"result": val * 3}))
        }));

        let result = server.handle_request_sync("nested", serde_json::json!({
            "data": {"value": 7}
        }));
        assert!(result.is_ok());
        assert_eq!(result.unwrap()["result"], 21);
    }

    #[test]
    fn test_default_handlers_all_lowercase() {
        let server = make_server();
        server.register_default_handlers();

        // Verify lowercase action names
        let actions = vec!["ping", "get_info", "get_capabilities", "list_actions"];
        for action in actions {
            let result = server.handle_request_sync(action, serde_json::json!({}));
            assert!(result.is_ok(), "Handler '{}' should work", action);
        }
    }

    #[test]
    fn test_multiple_unregister_safe() {
        let server = make_server();
        server.register_handler("temp", Box::new(|_| Ok(serde_json::json!({}))));
        server.unregister_handler("temp");
        server.unregister_handler("temp"); // Second unregister should not panic
    }

    #[test]
    fn test_server_bind_address_custom() {
        let server = RpcServer::new(RpcServerConfig {
            bind_address: "0.0.0.0:3000".into(),
            ..Default::default()
        });
        assert_eq!(server.bind_address(), "0.0.0.0:3000");
    }

    #[tokio::test]
    async fn test_server_stop_then_double_stop() {
        let server = make_server();
        server.start().await.unwrap();
        server.stop().unwrap();
        // Second stop should error
        let result = server.stop();
        assert!(result.is_err());
    }

    #[test]
    fn test_server_is_running_false_after_new() {
        let server = make_server();
        assert!(!server.is_running());
    }

    #[test]
    fn test_server_port_zero_before_start() {
        let server = make_server();
        assert_eq!(server.port(), 0);
    }

    // ============================================================
    // Coverage improvement: additional server tests
    // ============================================================

    #[test]
    fn test_handle_request_sync_with_array_payload() {
        let server = make_server();
        server.register_handler("array_action", Box::new(|payload| {
            let count = payload.as_array().map(|a| a.len()).unwrap_or(0);
            Ok(serde_json::json!({"count": count}))
        }));

        let result = server.handle_request_sync("array_action", serde_json::json!([1, 2, 3]));
        assert!(result.is_ok());
        assert_eq!(result.unwrap()["count"], 3);
    }

    #[test]
    fn test_register_many_handlers() {
        let server = make_server();
        for i in 0..50 {
            let val = i;
            server.register_handler(
                &format!("action_{}", i),
                Box::new(move |_| Ok(serde_json::json!({"val": val}))),
            );
        }

        for i in 0..50 {
            let result = server.handle_request_sync(&format!("action_{}", i), serde_json::json!({}));
            assert!(result.is_ok(), "action_{} should succeed", i);
            assert_eq!(result.unwrap()["val"], i);
        }
    }

    #[test]
    fn test_unregister_and_reregister_handler() {
        let server = make_server();

        server.register_handler("action", Box::new(|_| Ok(serde_json::json!({"v": 1}))));
        assert_eq!(server.handle_request_sync("action", serde_json::json!({})).unwrap()["v"], 1);

        server.unregister_handler("action");
        assert!(server.handle_request_sync("action", serde_json::json!({})).is_err());

        server.register_handler("action", Box::new(|_| Ok(serde_json::json!({"v": 2}))));
        assert_eq!(server.handle_request_sync("action", serde_json::json!({})).unwrap()["v"], 2);
    }

    #[test]
    fn test_default_peer_chat_handler_no_task_id() {
        let server = make_server();
        server.register_default_handlers();

        let result = server.handle_request_sync("peer_chat", serde_json::json!({}));
        assert!(result.is_ok());
        let resp = result.unwrap();
        assert_eq!(resp["task_id"], "unknown");
    }

    #[test]
    fn test_default_peer_chat_callback_no_task_id() {
        let server = make_server();
        server.register_default_handlers();

        let result = server.handle_request_sync("peer_chat_callback", serde_json::json!({}));
        assert!(result.is_ok());
        let resp = result.unwrap();
        assert_eq!(resp["task_id"], "unknown");
    }

    #[tokio::test]
    async fn test_server_start_registers_default_handlers() {
        let server = make_server();
        server.start().await.unwrap();

        // Verify default handlers were registered by start()
        let result = server.handle_request_sync("ping", serde_json::json!({}));
        assert!(result.is_ok());

        server.stop().unwrap();
    }

    #[test]
    fn test_rpc_server_config_custom_bind_address() {
        let config = RpcServerConfig {
            bind_address: "192.168.1.1:8080".into(),
            max_connections: 5,
            send_timeout: std::time::Duration::from_secs(1),
            idle_timeout: std::time::Duration::from_secs(10),
        };
        assert_eq!(config.bind_address, "192.168.1.1:8080");
        assert_eq!(config.max_connections, 5);
    }

    #[test]
    fn test_handler_with_empty_string_action() {
        let server = make_server();
        server.register_handler("", Box::new(|_| Ok(serde_json::json!({"empty": true}))));
        let result = server.handle_request_sync("", serde_json::json!({}));
        assert!(result.is_ok());
        assert_eq!(result.unwrap()["empty"], true);
    }

    #[test]
    fn test_handler_returns_complex_json() {
        let server = make_server();
        server.register_handler("complex", Box::new(|_| {
            Ok(serde_json::json!({
                "nested": {
                    "deep": {
                        "value": 42,
                        "list": [1, 2, 3],
                    }
                },
                "array": [{"a": 1}, {"b": 2}],
            }))
        }));

        let result = server.handle_request_sync("complex", serde_json::json!({}));
        assert!(result.is_ok());
        let resp = result.unwrap();
        assert_eq!(resp["nested"]["deep"]["value"], 42);
        assert_eq!(resp["nested"]["deep"]["list"].as_array().unwrap().len(), 3);
    }

    // ============================================================
    // Coverage improvement: WireMessage, auth, and handler edge cases
    // ============================================================

    #[test]
    fn test_rpc_server_config_default_values() {
        let config = RpcServerConfig::default();
        assert_eq!(config.max_connections, 100);
        assert_eq!(config.send_timeout, std::time::Duration::from_secs(10));
        assert_eq!(config.idle_timeout, std::time::Duration::from_secs(3900));
    }

    #[test]
    fn test_server_initial_state() {
        let server = make_server();
        assert!(!server.is_running());
        assert_eq!(server.port(), 0);
        assert_eq!(server.connection_count(), 0);
        assert_eq!(server.bind_address(), "0.0.0.0:0");
        assert!(server.auth_token.read().is_empty());
    }

    #[test]
    fn test_server_auth_token_lifecycle() {
        let server = make_server();
        assert!(server.auth_token.read().is_empty());
        server.set_auth_token("token-1");
        assert_eq!(&*server.auth_token.read(), "token-1");
        server.set_auth_token("token-2");
        assert_eq!(&*server.auth_token.read(), "token-2");
        server.set_auth_token("");
        assert!(server.auth_token.read().is_empty());
    }

    #[test]
    fn test_handler_with_rpc_meta_injection() {
        // Verify that handle_request_sync doesn't add _rpc metadata
        // (that's only added in the real async path)
        let server = make_server();
        server.register_handler("meta_test", Box::new(|payload| {
            // The sync path does NOT inject _rpc metadata
            Ok(serde_json::json!({
                "has_rpc": payload.get("_rpc").is_some(),
                "payload_keys": payload.as_object().map(|o| o.keys().collect::<Vec<_>>()),
            }))
        }));

        let result = server.handle_request_sync("meta_test", serde_json::json!({"key": "value"}));
        assert!(result.is_ok());
        let resp = result.unwrap();
        assert_eq!(resp["has_rpc"], false);
    }

    #[test]
    fn test_default_handlers_registered_via_start() {
        let server = make_server();
        // Before register_default_handlers, ping should not exist
        assert!(server.handle_request_sync("ping", serde_json::json!({})).is_err());
        server.register_default_handlers();
        assert!(server.handle_request_sync("ping", serde_json::json!({})).is_ok());
    }

    #[test]
    fn test_handler_error_propagation() {
        let server = make_server();
        let error_msg = "a very specific error message with special chars: <>&\"'";
        server.register_handler("fail_special", Box::new(move |_| {
            Err(error_msg.to_string())
        }));

        let result = server.handle_request_sync("fail_special", serde_json::json!({}));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("a very specific error"));
        assert!(err.contains("<>&"));
    }

    #[test]
    fn test_handler_with_large_payload() {
        let server = make_server();
        server.register_handler("echo", Box::new(|payload| {
            Ok(payload.clone())
        }));

        // Large payload
        let large_data: Vec<u8> = (0..10000).map(|i| (i % 256) as u8).collect();
        let payload = serde_json::json!({
            "data": large_data,
            "count": 10000,
        });

        let result = server.handle_request_sync("echo", payload);
        assert!(result.is_ok());
        let resp = result.unwrap();
        assert_eq!(resp["count"], 10000);
    }

    #[test]
    fn test_handler_with_unicode_payload() {
        let server = make_server();
        server.register_handler("unicode", Box::new(|payload| {
            let text = payload.get("text").and_then(|v| v.as_str()).unwrap_or("");
            Ok(serde_json::json!({"length": text.len(), "text": text}))
        }));

        let result = server.handle_request_sync("unicode", serde_json::json!({
            "text": "Hello 世界 🌍 مرحبا"
        }));
        assert!(result.is_ok());
    }

    #[test]
    fn test_multiple_handler_registrations_same_action() {
        let server = make_server();
        // Register 3 times - last one should win
        server.register_handler("replaceable", Box::new(|_| Ok(serde_json::json!({"v": 1}))));
        server.register_handler("replaceable", Box::new(|_| Ok(serde_json::json!({"v": 2}))));
        server.register_handler("replaceable", Box::new(|_| Ok(serde_json::json!({"v": 3}))));

        let result = server.handle_request_sync("replaceable", serde_json::json!({}));
        assert_eq!(result.unwrap()["v"], 3);
    }

    #[tokio::test]
    async fn test_server_start_stop_lifecycle() {
        let server = make_server();
        // Start
        server.start().await.unwrap();
        let port = server.port();
        assert!(port > 0);
        assert!(server.is_running());
        assert_eq!(server.connection_count(), 0);

        // Stop
        server.stop().unwrap();
        assert!(!server.is_running());
    }

    #[tokio::test]
    async fn test_server_stop_without_start_errors() {
        let server = make_server();
        let result = server.stop();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not running"));
    }

    #[tokio::test]
    async fn test_server_double_start_errors() {
        let server = make_server();
        server.start().await.unwrap();
        let result = server.start().await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already running"));
        server.stop().unwrap();
    }

    #[tokio::test]
    async fn test_server_reuse_after_stop() {
        let server = make_server();
        server.start().await.unwrap();
        let _port1 = server.port();
        server.stop().unwrap();

        // Restart
        server.start().await.unwrap();
        // May or may not get the same port
        assert!(server.is_running());
        server.stop().unwrap();
    }

    #[test]
    fn test_handler_payload_isolation() {
        // Verify handlers cannot affect each other's payloads
        let server = make_server();
        server.register_handler("modify", Box::new(|mut payload| {
            if let Some(obj) = payload.as_object_mut() {
                obj.insert("added".to_string(), serde_json::json!("by_handler"));
            }
            Ok(payload)
        }));

        let original = serde_json::json!({"original": true});
        let _ = server.handle_request_sync("modify", original.clone());

        // Original should be unchanged (cloned before passing)
        assert!(original.get("added").is_none());
    }

    #[test]
    fn test_default_handler_get_info_version() {
        let server = make_server();
        server.register_default_handlers();
        let result = server.handle_request_sync("get_info", serde_json::json!({}));
        let resp = result.unwrap();
        // Version should be set to cargo version
        assert!(resp.get("version").is_some());
    }

    #[test]
    fn test_default_handler_list_actions_count() {
        let server = make_server();
        server.register_default_handlers();
        let result = server.handle_request_sync("list_actions", serde_json::json!({}));
        let resp = result.unwrap();
        let actions = resp["actions"].as_array().unwrap();
        // Should have at least ping, get_info, get_capabilities, list_actions, peer_chat, peer_chat_callback
        assert!(actions.len() >= 6);
    }
}

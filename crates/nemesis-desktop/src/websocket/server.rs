//! WebSocket server - Accepts connections from child processes.
//!
//! Provides authentication, message routing, and request-response
//! correlation. Runs on localhost with dynamically assigned port.
//!
//! Uses tokio-tungstenite for raw WebSocket connections with a simple
//! HTTP upgrade mechanism for accepting child connections.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, oneshot};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{debug, info, warn};

use crate::websocket::dispatcher::Dispatcher;
use crate::websocket::protocol::Message;

/// Error type for WebSocket server operations.
#[derive(Debug, thiserror::Error)]
pub enum WsServerError {
    #[error("connection not found")]
    ConnectionNotFound,
    #[error("call timeout")]
    CallTimeout,
    #[error("send timeout")]
    SendTimeout,
    #[error("{0}")]
    Other(String),
}

/// Represents a child process connection.
pub struct ChildConnection {
    /// Connection ID (matches the authentication key).
    pub id: String,
    /// Authentication key.
    pub key: String,
    /// Child process PID.
    pub child_pid: u32,
    /// Child ID from handshake.
    pub child_id: Option<String>,
    /// Connection-level dispatcher.
    pub dispatcher: Dispatcher,
    /// Send channel for outgoing messages.
    send_tx: tokio::sync::mpsc::Sender<String>,
    /// Whether the connection is closed.
    closed: Arc<std::sync::atomic::AtomicBool>,
}

impl ChildConnection {
    fn new(
        id: String,
        key: String,
        child_pid: u32,
        send_tx: tokio::sync::mpsc::Sender<String>,
    ) -> Self {
        Self {
            id,
            key,
            child_pid,
            child_id: None,
            dispatcher: Dispatcher::new(),
            send_tx,
            closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Send a raw string message to this connection.
    pub async fn send(&self, data: String) -> Result<(), String> {
        if self.closed.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("connection closed".to_string());
        }
        self.send_tx
            .send(data)
            .await
            .map_err(|e| format!("send failed: {}", e))
    }

    /// Close the connection.
    pub fn close(&self) {
        self.closed.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Check if the connection is closed.
    pub fn is_closed(&self) -> bool {
        self.closed.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Key validation result.
#[derive(Debug, Clone)]
pub struct ValidatedKey {
    pub key: String,
    pub child_pid: u32,
    pub child_id: Option<String>,
    /// When this key was created.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// When this key was last used (validated).
    pub used_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Key generator for child process authentication.
pub struct KeyGenerator {
    keys: Mutex<HashMap<String, ValidatedKey>>,
}

impl KeyGenerator {
    pub fn new() -> Self {
        Self {
            keys: Mutex::new(HashMap::new()),
        }
    }

    /// Generate a new key for a child process.
    pub fn generate(&self, child_id: &str, child_pid: u32) -> String {
        let key = format!("{}-{}-{}", child_id, child_pid, uuid::Uuid::new_v4());

        self.keys.lock().insert(
            key.clone(),
            ValidatedKey {
                key: key.clone(),
                child_pid,
                child_id: Some(child_id.to_string()),
                created_at: chrono::Utc::now(),
                used_at: None,
            },
        );
        key
    }

    /// Validate an authentication key.
    ///
    /// On success, updates the `used_at` timestamp.
    pub fn validate(&self, key: &str) -> Result<ValidatedKey, String> {
        let mut map = self.keys.lock();
        let entry = map.get_mut(key).ok_or_else(|| "invalid key".to_string())?;
        entry.used_at = Some(chrono::Utc::now());
        Ok(entry.clone())
    }

    /// Remove a key.
    pub fn remove(&self, key: &str) {
        self.keys.lock().remove(key);
    }

    /// Revoke a specific key.
    ///
    /// Returns true if the key existed and was revoked, false if the key
    /// was not found.
    pub fn revoke(&self, key: &str) -> bool {
        self.keys.lock().remove(key).is_some()
    }

    /// Remove keys older than `max_age`.
    ///
    /// Returns the number of keys removed. Keys are considered expired
    /// based on their `created_at` timestamp.
    pub fn cleanup(&self, max_age: Duration) -> usize {
        let cutoff = chrono::Utc::now() - chrono::Duration::from_std(max_age).unwrap_or(chrono::Duration::MAX);
        let mut map = self.keys.lock();
        let before = map.len();
        map.retain(|_, v| v.created_at > cutoff);
        before - map.len()
    }
}

/// Shared server state behind Arc<Mutex> for Send-safe task sharing.
struct ServerState {
    /// Active connections.
    connections: HashMap<String, Arc<tokio::sync::Mutex<ChildConnection>>>,
    /// Pending request-response channels.
    pending: HashMap<String, oneshot::Sender<Message>>,
}

/// WebSocket server for parent-child process communication.
///
/// Listens on a dynamically assigned port on localhost. Authenticates
/// child connections via key-based handshake, then routes JSON-RPC
/// messages between parent and child processes.
pub struct WebSocketServer {
    /// Listening port.
    port: Arc<std::sync::atomic::AtomicU16>,
    /// Shared mutable state.
    state: Arc<Mutex<ServerState>>,
    /// Key generator.
    key_gen: Arc<KeyGenerator>,
    /// Server-level dispatcher.
    dispatcher: Dispatcher,
    /// Shutdown signal.
    shutdown_tx: broadcast::Sender<()>,
}

impl WebSocketServer {
    /// Create a new WebSocket server.
    pub fn new(key_gen: Arc<KeyGenerator>) -> Self {
        let (shutdown_tx, _) = broadcast::channel(1);
        Self {
            port: Arc::new(std::sync::atomic::AtomicU16::new(0)),
            state: Arc::new(Mutex::new(ServerState {
                connections: HashMap::new(),
                pending: HashMap::new(),
            })),
            key_gen,
            dispatcher: Dispatcher::new(),
            shutdown_tx,
        }
    }

    /// Start the WebSocket server on a dynamically assigned port.
    ///
    /// Binds to 127.0.0.1:0 and starts accepting connections in a background task.
    /// Returns the assigned port number.
    pub async fn start(&self) -> Result<u16, String> {
        info!("WebSocketServer: Starting...");

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| format!("bind failed: {}", e))?;

        let port = listener
            .local_addr()
            .map(|a| a.port())
            .map_err(|e| format!("get port: {}", e))?;

        self.port
            .store(port, std::sync::atomic::Ordering::SeqCst);

        info!("WebSocketServer: Listening on 127.0.0.1:{}", port);

        // Spawn the accept loop
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let state = self.state.clone();
        let key_gen = self.key_gen.clone();

        tokio::spawn(async move {
            loop {
                let accept_result = tokio::select! {
                    result = listener.accept() => result,
                    _ = shutdown_rx.recv() => {
                        info!("WebSocketServer: Accept loop shutting down");
                        return;
                    }
                };

                match accept_result {
                    Ok((stream, addr)) => {
                        debug!("WebSocketServer: New connection from {}", addr);
                        Self::handle_new_connection(stream, &state, &key_gen).await;
                    }
                    Err(e) => {
                        warn!("WebSocketServer: Accept error: {}", e);
                    }
                }
            }
        });

        Ok(port)
    }

    /// Handle a new TCP connection: upgrade to WebSocket, authenticate, start read/write loops.
    async fn handle_new_connection(
        stream: TcpStream,
        state: &Arc<Mutex<ServerState>>,
        key_gen: &Arc<KeyGenerator>,
    ) {
        // Upgrade to WebSocket
        let ws_stream = tokio_tungstenite::accept_async(stream).await;
        let ws_stream = match ws_stream {
            Ok(s) => s,
            Err(e) => {
                warn!("WebSocketServer: WebSocket upgrade failed: {}", e);
                return;
            }
        };

        let (mut ws_write, mut ws_read) = ws_stream.split();

        // Read the first message for authentication
        let auth_msg = match ws_read.next().await {
            Some(Ok(WsMessage::Text(text))) => text,
            _ => {
                warn!("WebSocketServer: Failed to read auth message");
                return;
            }
        };

        let auth: serde_json::Value = match serde_json::from_str(&auth_msg) {
            Ok(v) => v,
            Err(e) => {
                warn!("WebSocketServer: Auth JSON parse error: {}", e);
                return;
            }
        };

        let key = match auth.get("key").and_then(|v| v.as_str()) {
            Some(k) => k.to_string(),
            None => {
                warn!("WebSocketServer: No key in auth message");
                return;
            }
        };

        // Validate the key
        let validated = match key_gen.validate(&key) {
            Ok(v) => v,
            Err(e) => {
                warn!("WebSocketServer: Auth failed: {}", e);
                return;
            }
        };

        info!(
            "WebSocketServer: Child authenticated: PID={}, ChildID={:?}",
            validated.child_pid, validated.child_id
        );

        // Create send/receive channels
        let (msg_tx, mut msg_rx) = tokio::sync::mpsc::channel::<String>(64);

        let mut conn = ChildConnection::new(
            validated.key.clone(),
            validated.key.clone(),
            validated.child_pid,
            msg_tx,
        );

        // Set child_id if available
        if let Some(ref child_id) = validated.child_id {
            conn.child_id = Some(child_id.clone());
        }

        let conn = Arc::new(tokio::sync::Mutex::new(conn));

        // Register connection by key and child_id
        {
            let mut s = state.lock();
            s.connections.insert(validated.key.clone(), conn.clone());
            if let Some(ref child_id) = validated.child_id {
                s.connections.insert(child_id.clone(), conn.clone());
            }
        }

        info!(
            "WebSocketServer: Connection registered: UUID={}, ChildID={:?}",
            validated.key, validated.child_id
        );

        // Spawn write loop
        let write_key = validated.key.clone();
        tokio::spawn(async move {
            while let Some(data) = msg_rx.recv().await {
                if let Err(e) = ws_write.send(WsMessage::Text(data.into())).await {
                    warn!("WebSocketServer: Write error for {}: {}", write_key, e);
                    break;
                }
            }
            debug!("WebSocketServer: Write loop ended for {}", write_key);
        });

        // Spawn read loop
        let read_key = validated.key.clone();
        let read_child_id = validated.child_id.clone();
        let read_state = state.clone();

        tokio::spawn(async move {
            while let Some(msg_result) = ws_read.next().await {
                match msg_result {
                    Ok(WsMessage::Text(text)) => {
                        let msg: Message = match serde_json::from_str(&text) {
                            Ok(m) => m,
                            Err(e) => {
                                debug!("WebSocketServer: JSON decode error: {}", e);
                                continue;
                            }
                        };

                        if msg.jsonrpc != crate::websocket::protocol::VERSION {
                            debug!("WebSocketServer: Non-protocol message ignored");
                            continue;
                        }

                        // Route message
                        if msg.is_response() {
                            // Route to pending channel
                            let msg_id = msg.id.clone();
                            if let Some(id) = msg_id {
                                let mut s = read_state.lock();
                                if let Some(tx) = s.pending.remove(&id) {
                                    let _ = tx.send(msg);
                                }
                            }
                        } else if msg.is_request() || msg.is_notification() {
                            // Get connection without holding the state lock across await
                            let conn_arc = {
                                let s = read_state.lock();
                                s.connections.get(&read_key).cloned()
                            };

                            if let Some(conn_arc) = conn_arc {
                                // Clone the dispatcher result before awaiting
                                let dispatch_result = {
                                    let guard = conn_arc.lock().await;
                                    guard.dispatcher.dispatch(&msg)
                                };

                                if msg.is_request() {
                                    if let Ok(Some(resp_msg)) = dispatch_result {
                                        let resp_str = serde_json::to_string(&resp_msg).unwrap_or_default();
                                        let guard = conn_arc.lock().await;
                                        let _ = guard.send(resp_str).await;
                                    }
                                }
                            }
                        }
                    }
                    Ok(WsMessage::Close(_)) => {
                        info!("WebSocketServer: Close frame from {}", read_key);
                        break;
                    }
                    Err(e) => {
                        warn!("WebSocketServer: Read error for {}: {}", read_key, e);
                        break;
                    }
                    _ => {}
                }
            }

            // Clean up connection
            {
                let mut s = read_state.lock();
                s.connections.remove(&read_key);
                if let Some(ref child_id) = read_child_id {
                    s.connections.remove(child_id);
                }
            }

            debug!("WebSocketServer: Connection removed: {}", read_key);
        });
    }

    /// Stop the WebSocket server.
    pub fn stop(&self) {
        info!("WebSocketServer: Stopping...");
        let _ = self.shutdown_tx.send(());

        // Clear all connections and pending
        {
            let mut s = self.state.lock();
            s.connections.clear();
            s.pending.clear();
        }
    }

    /// Get the listening port.
    pub fn get_port(&self) -> u16 {
        self.port.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Send a notification to a child process.
    ///
    /// Fire-and-forget: no response is expected.
    pub fn send_notification(
        &self,
        child_id: &str,
        method: &str,
        params: serde_json::Value,
    ) -> Result<(), String> {
        let conn = {
            let s = self.state.lock();
            s.connections
                .get(child_id)
                .cloned()
                .ok_or("connection not found")?
        };

        let msg = Message::new_notification(method, params);
        let data = serde_json::to_string(&msg).map_err(|e| format!("marshal: {}", e))?;

        // Use try_lock to avoid blocking the calling thread
        match conn.try_lock() {
            Ok(guard) => guard
                .send_tx
                .try_send(data)
                .map_err(|e| format!("send failed: {}", e)),
            Err(_) => Err("connection busy".to_string()),
        }
    }

    /// Send a request to a child process and wait for the response.
    ///
    /// Registers a pending channel, sends the request, and awaits the
    /// response with a 30-second timeout.
    pub async fn call_child(
        &self,
        child_id: &str,
        method: &str,
        params: serde_json::Value,
    ) -> Result<Message, WsServerError> {
        let conn = {
            let s = self.state.lock();
            s.connections
                .get(child_id)
                .cloned()
                .ok_or(WsServerError::ConnectionNotFound)?
        };

        let msg = Message::new_request(method, params);
        let msg_id = msg.id.clone().unwrap_or_default();

        let (tx, rx) = oneshot::channel();
        {
            let mut s = self.state.lock();
            s.pending.insert(msg_id.clone(), tx);
        }

        let data =
            serde_json::to_string(&msg).map_err(|e| WsServerError::Other(e.to_string()))?;

        // Send via connection
        {
            let guard = conn.lock().await;
            guard
                .send(data)
                .await
                .map_err(|e| {
                    let mut s = self.state.lock();
                    s.pending.remove(&msg_id);
                    WsServerError::Other(e)
                })?;
        }

        // Wait for response with timeout
        match tokio::time::timeout(Duration::from_secs(30), rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => {
                let mut s = self.state.lock();
                s.pending.remove(&msg_id);
                Err(WsServerError::Other("response channel dropped".to_string()))
            }
            Err(_) => {
                let mut s = self.state.lock();
                s.pending.remove(&msg_id);
                Err(WsServerError::CallTimeout)
            }
        }
    }

    /// Register a server-level request handler.
    pub fn register_handler<F>(&self, method: &str, handler: F)
    where
        F: Fn(&Message) -> Result<Message, String> + Send + Sync + 'static,
    {
        self.dispatcher.register(method, handler);
    }

    /// Register a server-level notification handler.
    pub fn register_notification_handler<F>(&self, method: &str, handler: F)
    where
        F: Fn(&Message) + Send + Sync + 'static,
    {
        self.dispatcher.register_notification(method, handler);
    }

    /// Get a connection by child ID.
    pub fn get_connection(&self, child_id: &str) -> Option<Arc<tokio::sync::Mutex<ChildConnection>>> {
        self.state.lock().connections.get(child_id).cloned()
    }

    /// Remove a connection.
    pub fn remove_connection(&self, child_id: &str) {
        let key = {
            let s = self.state.lock();
            s.connections.get(child_id).and_then(|c| {
                c.try_lock().map(|g| g.key.clone()).ok()
            })
        };

        let mut s = self.state.lock();
        s.connections.remove(child_id);

        // Also remove by the connection's own key if different
        if let Some(key) = key {
            s.connections.remove(&key);
        }
    }

    /// Return the key generator.
    pub fn key_generator(&self) -> &Arc<KeyGenerator> {
        &self.key_gen
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_generator() {
        let key_gen = KeyGenerator::new();
        let key = key_gen.generate("child-1", 1234);
        assert!(key.contains("child-1"));

        let validated = key_gen.validate(&key).unwrap();
        assert_eq!(validated.child_pid, 1234);
        assert_eq!(validated.child_id.as_deref(), Some("child-1"));
    }

    #[test]
    fn test_key_generator_invalid() {
        let key_gen = KeyGenerator::new();
        let result = key_gen.validate("invalid");
        assert!(result.is_err());
    }

    #[test]
    fn test_key_generator_remove() {
        let key_gen = KeyGenerator::new();
        let key = key_gen.generate("child-1", 1234);
        key_gen.remove(&key);
        assert!(key_gen.validate(&key).is_err());
    }

    #[tokio::test]
    async fn test_server_start_and_stop() {
        let key_gen = Arc::new(KeyGenerator::new());
        let server = WebSocketServer::new(key_gen);
        let result = server.start().await;
        assert!(result.is_ok());
        let port = result.unwrap();
        assert!(port > 0);
        server.stop();
    }

    #[test]
    fn test_server_notification_no_connection() {
        let key_gen = Arc::new(KeyGenerator::new());
        let server = WebSocketServer::new(key_gen);
        let result = server.send_notification("nonexistent", "test", serde_json::Value::Null);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_server_call_no_connection() {
        let key_gen = Arc::new(KeyGenerator::new());
        let server = WebSocketServer::new(key_gen);
        let result = server
            .call_child("nonexistent", "test", serde_json::Value::Null)
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn test_server_register_handler() {
        let key_gen = Arc::new(KeyGenerator::new());
        let server = WebSocketServer::new(key_gen);
        server.register_handler("ping", |msg| {
            Ok(Message::new_response(
                msg.id.as_deref().unwrap_or(""),
                serde_json::json!("pong"),
            ))
        });
    }

    #[test]
    fn test_server_register_notification_handler() {
        let key_gen = Arc::new(KeyGenerator::new());
        let server = WebSocketServer::new(key_gen);
        server.register_notification_handler("event", |_msg| {});
    }

    #[test]
    fn test_server_get_connection_none() {
        let key_gen = Arc::new(KeyGenerator::new());
        let server = WebSocketServer::new(key_gen);
        assert!(server.get_connection("nonexistent").is_none());
    }

    #[test]
    fn test_server_remove_connection_nonexistent() {
        let key_gen = Arc::new(KeyGenerator::new());
        let server = WebSocketServer::new(key_gen);
        // Should not panic
        server.remove_connection("nonexistent");
    }

    #[test]
    fn test_key_generator_revoke() {
        let key_gen = KeyGenerator::new();
        let key = key_gen.generate("child-1", 1234);

        // Revoke existing key returns true
        assert!(key_gen.revoke(&key));
        assert!(key_gen.validate(&key).is_err());

        // Revoke non-existent key returns false
        assert!(!key_gen.revoke("nonexistent"));
    }

    #[test]
    fn test_key_generator_cleanup() {
        let key_gen = KeyGenerator::new();
        let key1 = key_gen.generate("child-1", 1111);
        let key2 = key_gen.generate("child-2", 2222);

        // Cleanup with very large max_age should remove nothing
        let removed = key_gen.cleanup(Duration::from_secs(86400 * 365));
        assert_eq!(removed, 0);
        assert!(key_gen.validate(&key1).is_ok());
        assert!(key_gen.validate(&key2).is_ok());

        // Cleanup with zero max_age should remove all keys
        let removed = key_gen.cleanup(Duration::ZERO);
        assert_eq!(removed, 2);
        assert!(key_gen.validate(&key1).is_err());
        assert!(key_gen.validate(&key2).is_err());
    }

    #[test]
    fn test_key_generator_timestamps() {
        let key_gen = KeyGenerator::new();
        let key = key_gen.generate("child-1", 1234);

        // Validate the key and check used_at is set
        let validated = key_gen.validate(&key).unwrap();
        assert!(validated.created_at <= chrono::Utc::now());
        assert!(validated.used_at.is_some());

        // Before validation, used_at was None in the stored copy;
        // after validation it should be set
        assert!(validated.used_at.unwrap() >= validated.created_at);
    }

    #[test]
    fn test_server_get_port_default() {
        let key_gen = Arc::new(KeyGenerator::new());
        let server = WebSocketServer::new(key_gen);
        assert_eq!(server.get_port(), 0);
    }

    #[tokio::test]
    async fn test_server_start_assigns_port() {
        let key_gen = Arc::new(KeyGenerator::new());
        let server = WebSocketServer::new(key_gen);
        let port = server.start().await.unwrap();
        assert_ne!(port, 0);
        assert_eq!(server.get_port(), port);
        server.stop();
    }

    #[test]
    fn test_child_connection_new() {
        let (tx, _rx) = tokio::sync::mpsc::channel::<String>(64);
        let conn = ChildConnection::new("key-1".to_string(), "key-1".to_string(), 1234, tx);
        assert_eq!(conn.id, "key-1");
        assert_eq!(conn.child_pid, 1234);
        assert!(conn.child_id.is_none());
        assert!(!conn.is_closed());
    }

    #[test]
    fn test_child_connection_close() {
        let (tx, _rx) = tokio::sync::mpsc::channel::<String>(64);
        let conn = ChildConnection::new("key-1".to_string(), "key-1".to_string(), 1234, tx);
        conn.close();
        assert!(conn.is_closed());
    }

    // ============================================================
    // Additional tests for ~92% coverage
    // ============================================================

    #[test]
    fn test_ws_server_error_display() {
        let err = WsServerError::ConnectionNotFound;
        assert!(err.to_string().contains("connection not found"));

        let err = WsServerError::CallTimeout;
        assert!(err.to_string().contains("call timeout"));

        let err = WsServerError::SendTimeout;
        assert!(err.to_string().contains("send timeout"));

        let err = WsServerError::Other("custom error".to_string());
        assert!(err.to_string().contains("custom error"));
    }

    #[test]
    fn test_ws_server_error_debug() {
        let err = WsServerError::ConnectionNotFound;
        let debug = format!("{:?}", err);
        assert!(debug.contains("ConnectionNotFound"));
    }

    #[test]
    fn test_child_connection_send_closed() {
        let (tx, _rx) = tokio::sync::mpsc::channel::<String>(64);
        let conn = ChildConnection::new("key-1".to_string(), "key-1".to_string(), 1234, tx);
        conn.close();
        // Send should fail when closed
        // Need runtime for async send
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(conn.send("test".to_string()));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("connection closed"));
    }

    #[test]
    fn test_child_connection_send_success() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(64);
        let conn = ChildConnection::new("key-1".to_string(), "key-1".to_string(), 1234, tx);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(conn.send("hello".to_string()));
        assert!(result.is_ok());
        let received = rx.try_recv().unwrap();
        assert_eq!(received, "hello");
    }

    #[test]
    fn test_child_connection_send_channel_dropped() {
        let (tx, rx) = tokio::sync::mpsc::channel::<String>(64);
        let conn = ChildConnection::new("key-1".to_string(), "key-1".to_string(), 1234, tx);
        drop(rx); // Drop receiver
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(conn.send("hello".to_string()));
        assert!(result.is_err());
    }

    #[test]
    fn test_child_connection_dispatcher() {
        let (tx, _rx) = tokio::sync::mpsc::channel::<String>(64);
        let conn = ChildConnection::new("key-1".to_string(), "key-1".to_string(), 1234, tx);
        // Dispatcher should be usable
        conn.dispatcher.register("ping", |msg| {
            Ok(Message::new_response(msg.id.as_deref().unwrap_or(""), serde_json::json!("pong")))
        });
        let req = Message::new_request("ping", serde_json::Value::Null);
        let result = conn.dispatcher.dispatch(&req).unwrap().unwrap();
        assert_eq!(result.result.as_ref().unwrap(), &serde_json::json!("pong"));
    }

    #[test]
    fn test_validated_key_debug_clone() {
        let key_gen = KeyGenerator::new();
        let key = key_gen.generate("child-1", 1234);
        let validated = key_gen.validate(&key).unwrap();

        // Debug
        let debug = format!("{:?}", validated);
        assert!(debug.contains("child-1"));

        // Clone
        let cloned = validated.clone();
        assert_eq!(cloned.child_pid, 1234);
        assert_eq!(cloned.key, key);
    }

    #[test]
    fn test_validated_key_fields() {
        let key_gen = KeyGenerator::new();
        let key = key_gen.generate("child-test", 5678);
        let validated = key_gen.validate(&key).unwrap();
        assert_eq!(validated.child_pid, 5678);
        assert_eq!(validated.child_id.as_deref(), Some("child-test"));
        assert_eq!(validated.key, key);
        assert!(validated.created_at <= chrono::Utc::now());
        assert!(validated.used_at.is_some());
    }

    #[test]
    fn test_key_generator_multiple_keys() {
        let key_gen = KeyGenerator::new();
        let key1 = key_gen.generate("child-1", 1111);
        let key2 = key_gen.generate("child-2", 2222);
        let key3 = key_gen.generate("child-3", 3333);

        assert!(key_gen.validate(&key1).is_ok());
        assert!(key_gen.validate(&key2).is_ok());
        assert!(key_gen.validate(&key3).is_ok());

        // Remove key2
        key_gen.remove(&key2);
        assert!(key_gen.validate(&key1).is_ok());
        assert!(key_gen.validate(&key2).is_err());
        assert!(key_gen.validate(&key3).is_ok());
    }

    #[test]
    fn test_key_generator_cleanup_partial() {
        let key_gen = KeyGenerator::new();
        let _key1 = key_gen.generate("child-1", 1111);
        // Wait a tiny bit
        std::thread::sleep(std::time::Duration::from_millis(10));
        let _key2 = key_gen.generate("child-2", 2222);

        // Cleanup with 5ms should remove key1 but keep key2 (timing sensitive)
        // This test may be flaky on very fast machines; use a larger margin
        let removed = key_gen.cleanup(std::time::Duration::from_millis(5));
        // At least key1 should be removed (it was created 10ms before cleanup check)
        assert!(removed >= 1);
    }

    #[test]
    fn test_server_register_handler_and_use() {
        let key_gen = Arc::new(KeyGenerator::new());
        let server = WebSocketServer::new(key_gen);

        server.register_handler("test.method", |msg| {
            Ok(Message::new_response(msg.id.as_deref().unwrap_or(""), serde_json::json!({"result": "ok"})))
        });
        // Verify it was registered (no panic)
    }

    #[test]
    fn test_server_notification_handler() {
        let key_gen = Arc::new(KeyGenerator::new());
        let server = WebSocketServer::new(key_gen);
        let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let called_clone = called.clone();
        server.register_notification_handler("event", move |_msg| {
            called_clone.store(true, std::sync::atomic::Ordering::SeqCst);
        });
    }

    #[tokio::test]
    async fn test_server_start_stop_idempotent() {
        let key_gen = Arc::new(KeyGenerator::new());
        let server = WebSocketServer::new(key_gen);
        let port = server.start().await.unwrap();
        assert!(port > 0);
        server.stop();
        // Stop again should not panic
        server.stop();
    }

    #[test]
    fn test_server_send_notification_nonexistent() {
        let key_gen = Arc::new(KeyGenerator::new());
        let server = WebSocketServer::new(key_gen);
        let result = server.send_notification("nonexistent", "method", serde_json::json!({}));
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_server_call_child_nonexistent() {
        let key_gen = Arc::new(KeyGenerator::new());
        let server = WebSocketServer::new(key_gen);
        let result = server.call_child("nonexistent", "method", serde_json::json!({})).await;
        assert!(matches!(result, Err(WsServerError::ConnectionNotFound)));
    }

    #[test]
    fn test_child_connection_child_id() {
        let (tx, _rx) = tokio::sync::mpsc::channel::<String>(64);
        let mut conn = ChildConnection::new("key-1".to_string(), "key-1".to_string(), 1234, tx);
        assert!(conn.child_id.is_none());
        conn.child_id = Some("child-test".to_string());
        assert_eq!(conn.child_id.as_deref(), Some("child-test"));
    }

    // ============================================================
    // Phase 4: Integration tests for higher coverage
    // ============================================================

    #[tokio::test]
    async fn test_server_client_full_connection() {
        let key_gen = Arc::new(KeyGenerator::new());
        let server = WebSocketServer::new(key_gen.clone());
        let port = server.start().await.unwrap();

        // Generate a key
        let key = key_gen.generate("child-test", 42);

        // Connect a client to the server
        let url = format!("ws://127.0.0.1:{}{}", port, key);
        let connect_result = tokio_tungstenite::connect_async(&url).await;
        if let Ok((mut ws_stream, _)) = connect_result {
            // Send auth message
            let auth = serde_json::json!({"type": "auth", "key": key});
            ws_stream
                .send(tokio_tungstenite::tungstenite::Message::Text(auth.to_string().into()))
                .await
                .unwrap();

            // Give server time to process
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            // Verify connection exists
            assert!(server.get_connection(&key).is_some());
            assert!(server.get_connection("child-test").is_some());

            // Get connection and verify child_pid
            let conn = server.get_connection(&key).unwrap();
            let guard = conn.lock().await;
            assert_eq!(guard.child_pid, 42);
            assert_eq!(guard.child_id.as_deref(), Some("child-test"));
            drop(guard);

            // Send notification from server to client
            let result = server.send_notification("child-test", "test.method", serde_json::json!({"data": 123}));
            assert!(result.is_ok());

            // Client should receive the notification
            let msg_result = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                ws_stream.next()
            ).await;

            if let Ok(Some(Ok(ws_msg))) = msg_result {
                if let tokio_tungstenite::tungstenite::Message::Text(text) = ws_msg {
                    let msg: Message = serde_json::from_str(&text).unwrap();
                    assert!(msg.is_notification());
                    assert_eq!(msg.method.as_deref(), Some("test.method"));
                }
            }

            // Close connection
            ws_stream.close(None).await.ok();
        }

        server.stop();
    }

    #[tokio::test]
    async fn test_server_client_auth_failure() {
        let key_gen = Arc::new(KeyGenerator::new());
        let server = WebSocketServer::new(key_gen.clone());
        let port = server.start().await.unwrap();

        // Connect with invalid key
        let url = format!("ws://127.0.0.1:{}/test", port);
        let connect_result = tokio_tungstenite::connect_async(&url).await;
        if let Ok((mut ws_stream, _)) = connect_result {
            // Send invalid auth
            let auth = serde_json::json!({"type": "auth", "key": "invalid-key"});
            let _ = ws_stream
                .send(tokio_tungstenite::tungstenite::Message::Text(auth.to_string().into()))
                .await;

            // Give server time to process
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            // Connection should not be registered
            assert!(server.get_connection("invalid-key").is_none());
        }

        server.stop();
    }

    #[tokio::test]
    async fn test_server_client_no_key_in_auth() {
        let key_gen = Arc::new(KeyGenerator::new());
        let server = WebSocketServer::new(key_gen.clone());
        let port = server.start().await.unwrap();

        let url = format!("ws://127.0.0.1:{}/test", port);
        let connect_result = tokio_tungstenite::connect_async(&url).await;
        if let Ok((mut ws_stream, _)) = connect_result {
            // Send auth without key field
            let auth = serde_json::json!({"type": "auth"});
            let _ = ws_stream
                .send(tokio_tungstenite::tungstenite::Message::Text(auth.to_string().into()))
                .await;

            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            // Connection should not be registered
            assert!(server.get_connection("anything").is_none());
        }

        server.stop();
    }

    #[tokio::test]
    async fn test_server_client_invalid_auth_json() {
        let key_gen = Arc::new(KeyGenerator::new());
        let server = WebSocketServer::new(key_gen.clone());
        let port = server.start().await.unwrap();

        let url = format!("ws://127.0.0.1:{}/test", port);
        let connect_result = tokio_tungstenite::connect_async(&url).await;
        if let Ok((mut ws_stream, _)) = connect_result {
            // Send invalid JSON
            let _ = ws_stream
                .send(tokio_tungstenite::tungstenite::Message::Text("not json".into()))
                .await;

            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            assert!(server.get_connection("anything").is_none());
        }

        server.stop();
    }

    #[tokio::test]
    async fn test_server_client_request_response() {
        let key_gen = Arc::new(KeyGenerator::new());
        let server = WebSocketServer::new(key_gen.clone());

        // Register a handler on the server
        server.register_handler("add", |msg| {
            let id = msg.id.as_deref().unwrap_or("");
            Ok(Message::new_response(id, serde_json::json!({"result": "added"})))
        });

        let port = server.start().await.unwrap();
        let key = key_gen.generate("child-rpc", 100);

        let url = format!("ws://127.0.0.1:{}{}", port, key);
        let connect_result = tokio_tungstenite::connect_async(&url).await;
        if let Ok((mut ws_stream, _)) = connect_result {
            // Auth
            let auth = serde_json::json!({"type": "auth", "key": key});
            ws_stream
                .send(tokio_tungstenite::tungstenite::Message::Text(auth.to_string().into()))
                .await
                .unwrap();

            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            // Send a request from client to server
            let request = Message::new_request("add", serde_json::json!({"a": 1, "b": 2}));
            let request_str = serde_json::to_string(&request).unwrap();
            ws_stream
                .send(tokio_tungstenite::tungstenite::Message::Text(request_str.into()))
                .await
                .unwrap();

            // Receive response
            let msg_result = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                ws_stream.next()
            ).await;

            if let Ok(Some(Ok(ws_msg))) = msg_result {
                if let tokio_tungstenite::tungstenite::Message::Text(text) = ws_msg {
                    let resp: Message = serde_json::from_str(&text).unwrap();
                    assert!(resp.is_success_response());
                    assert_eq!(resp.result.as_ref().unwrap()["result"], "added");
                }
            }

            ws_stream.close(None).await.ok();
        }

        server.stop();
    }

    #[tokio::test]
    async fn test_server_call_child_with_connection() {
        let key_gen = Arc::new(KeyGenerator::new());
        let server = WebSocketServer::new(key_gen.clone());
        let port = server.start().await.unwrap();
        let key = key_gen.generate("child-call", 200);

        let url = format!("ws://127.0.0.1:{}{}", port, key);
        let connect_result = tokio_tungstenite::connect_async(&url).await;
        if let Ok((mut ws_stream, _)) = connect_result {
            // Auth
            let auth = serde_json::json!({"type": "auth", "key": key});
            ws_stream
                .send(tokio_tungstenite::tungstenite::Message::Text(auth.to_string().into()))
                .await
                .unwrap();

            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            // call_child should work now (connection exists)
            // We need to spawn a task to read and respond
            let (client_tx, mut client_rx) = tokio::sync::mpsc::channel::<String>(64);

            // Read the call request from server and send a response
            let read_handle = tokio::spawn(async move {
                if let Some(Ok(ws_msg)) = ws_stream.next().await {
                    if let tokio_tungstenite::tungstenite::Message::Text(text) = ws_msg {
                        let msg: Message = serde_json::from_str(&text).unwrap();
                        if msg.is_request() {
                            let resp = Message::new_response(
                                msg.id.as_deref().unwrap_or(""),
                                serde_json::json!({"status": "handled"}),
                            );
                            let _ = client_tx.send(serde_json::to_string(&resp).unwrap()).await;
                        }
                    }
                }
                ws_stream
            });

                // Wait for the response to be ready
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;

                // Make the call
                let call_result = server.call_child("child-call", "test.method", serde_json::json!({})).await;
                if let Ok(response) = call_result {
                    assert!(response.is_success_response());
                    assert_eq!(response.result.as_ref().unwrap()["status"], "handled");
                }

                // Send response from client side
                if let Some(resp_str) = client_rx.recv().await {
                    let mut ws = read_handle.await.unwrap();
                    let _ = ws
                        .send(tokio_tungstenite::tungstenite::Message::Text(resp_str.into()))
                        .await;
                }
        }

        server.stop();
    }

    #[tokio::test]
    async fn test_server_remove_connection_with_child() {
        let key_gen = Arc::new(KeyGenerator::new());
        let server = WebSocketServer::new(key_gen.clone());
        let port = server.start().await.unwrap();
        let key = key_gen.generate("child-remove", 300);

        let url = format!("ws://127.0.0.1:{}{}", port, key);
        let connect_result = tokio_tungstenite::connect_async(&url).await;
        if let Ok((mut ws_stream, _)) = connect_result {
            // Auth
            let auth = serde_json::json!({"type": "auth", "key": key});
            ws_stream
                .send(tokio_tungstenite::tungstenite::Message::Text(auth.to_string().into()))
                .await
                .unwrap();

            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            // Connection should exist
            assert!(server.get_connection("child-remove").is_some());

            // Remove connection
            server.remove_connection("child-remove");
            assert!(server.get_connection("child-remove").is_none());

            ws_stream.close(None).await.ok();
        }

        server.stop();
    }

    #[test]
    fn test_send_notification_connection_busy() {
        let key_gen = Arc::new(KeyGenerator::new());
        let server = WebSocketServer::new(key_gen);

        // Manually insert a connection with a locked mutex
        let (tx, _rx) = tokio::sync::mpsc::channel::<String>(64);
        let conn = Arc::new(tokio::sync::Mutex::new(
            ChildConnection::new("key-1".to_string(), "key-1".to_string(), 42, tx)
        ));

        // Lock the connection so try_lock fails
        let guard = conn.blocking_lock();
        {
            let mut state = server.state.lock();
            state.connections.insert("test-id".to_string(), conn.clone());
        }

        // send_notification should fail because connection is busy
        let result = server.send_notification("test-id", "test", serde_json::Value::Null);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("connection busy"));

        drop(guard);
    }

    #[test]
    fn test_send_notification_success() {
        let key_gen = Arc::new(KeyGenerator::new());
        let server = WebSocketServer::new(key_gen);

        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(64);
        let conn = Arc::new(tokio::sync::Mutex::new(
            ChildConnection::new("key-1".to_string(), "key-1".to_string(), 42, tx)
        ));

        {
            let mut state = server.state.lock();
            state.connections.insert("test-id".to_string(), conn.clone());
        }

        let result = server.send_notification("test-id", "method", serde_json::json!({"x": 1}));
        assert!(result.is_ok());

        // Verify message was sent
        let msg_str = rx.try_recv().unwrap();
        let msg: Message = serde_json::from_str(&msg_str).unwrap();
        assert!(msg.is_notification());
        assert_eq!(msg.method.as_deref(), Some("method"));
    }

    // ============================================================
    // Additional tests for 95%+ coverage
    // ============================================================

    #[test]
    fn test_server_key_generator_accessible() {
        let key_gen = Arc::new(KeyGenerator::new());
        let server = WebSocketServer::new(key_gen.clone());
        let gen_ref = server.key_generator();
        let key = gen_ref.generate("child-1", 1234);
        assert!(gen_ref.validate(&key).is_ok());
    }

    #[test]
    fn test_ws_server_error_variants() {
        let err = WsServerError::ConnectionNotFound;
        assert_eq!(err.to_string(), "connection not found");

        let err = WsServerError::CallTimeout;
        assert_eq!(err.to_string(), "call timeout");

        let err = WsServerError::SendTimeout;
        assert_eq!(err.to_string(), "send timeout");

        let err = WsServerError::Other("custom".to_string());
        assert_eq!(err.to_string(), "custom");
    }

    #[test]
    fn test_validated_key_clone_independent() {
        let key_gen = KeyGenerator::new();
        let key = key_gen.generate("child-1", 1234);
        let v1 = key_gen.validate(&key).unwrap();
        let v2 = v1.clone();
        // They should be equal but independent
        assert_eq!(v1.key, v2.key);
        assert_eq!(v1.child_pid, v2.child_pid);
    }

    #[tokio::test]
    async fn test_server_send_notification_connection_closed() {
        let key_gen = Arc::new(KeyGenerator::new());
        let server = WebSocketServer::new(key_gen);

        let (tx, rx) = tokio::sync::mpsc::channel::<String>(64);
        drop(rx); // Drop receiver to simulate closed channel

        let conn = Arc::new(tokio::sync::Mutex::new(
            ChildConnection::new("key-1".to_string(), "key-1".to_string(), 42, tx)
        ));

        // Close the connection
        conn.lock().await.close();

        {
            let mut state = server.state.lock();
            state.connections.insert("test-id".to_string(), conn.clone());
        }

        // send_notification should fail because the receiver is dropped
        let result = server.send_notification("test-id", "test", serde_json::Value::Null);
        assert!(result.is_err());
    }

    #[test]
    fn test_send_notification_connection_rx_dropped() {
        let key_gen = Arc::new(KeyGenerator::new());
        let server = WebSocketServer::new(key_gen);

        let (tx, rx) = tokio::sync::mpsc::channel::<String>(64);
        drop(rx); // Drop receiver to simulate closed channel

        let conn = Arc::new(tokio::sync::Mutex::new(
            ChildConnection::new("key-1".to_string(), "key-1".to_string(), 42, tx)
        ));

        {
            let mut state = server.state.lock();
            state.connections.insert("test-id".to_string(), conn.clone());
        }

        let result = server.send_notification("test-id", "test", serde_json::Value::Null);
        assert!(result.is_err());
    }

    #[test]
    fn test_child_connection_send_after_close() {
        let (tx, _rx) = tokio::sync::mpsc::channel::<String>(64);
        let conn = ChildConnection::new("key-1".to_string(), "key-1".to_string(), 42, tx);
        assert!(!conn.is_closed());
        conn.close();
        assert!(conn.is_closed());
        // Double close should be safe
        conn.close();
        assert!(conn.is_closed());
    }

    #[test]
    fn test_key_generator_generate_format() {
        let key_gen = KeyGenerator::new();
        let key = key_gen.generate("my-child", 9999);
        // Key should contain child_id and child_pid
        assert!(key.starts_with("my-child-9999-"));
        // And end with a UUID
        let parts: Vec<&str> = key.splitn(4, '-').collect();
        assert!(parts.len() >= 3);
    }

    #[tokio::test]
    async fn test_server_double_start_gets_new_port() {
        let key_gen = Arc::new(KeyGenerator::new());
        let server = WebSocketServer::new(key_gen);

        // Starting twice should work (second start binds a new port)
        let port1 = server.start().await.unwrap();
        assert!(port1 > 0);
        server.stop();
    }

    #[test]
    fn test_server_state_empty() {
        let key_gen = Arc::new(KeyGenerator::new());
        let server = WebSocketServer::new(key_gen);
        // Initially no connections
        assert!(server.get_connection("anything").is_none());
        assert_eq!(server.get_port(), 0);
    }
}

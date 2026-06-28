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
    pub created_at: chrono::DateTime<chrono::Local>,
    /// When this key was last used (validated).
    pub used_at: Option<chrono::DateTime<chrono::Local>>,
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
                created_at: chrono::Local::now(),
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
        entry.used_at = Some(chrono::Local::now());
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
        let cutoff = chrono::Local::now() - chrono::Duration::from_std(max_age).unwrap_or(chrono::Duration::MAX);
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
        info!("[WebSocketServer] Starting...");

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| format!("bind failed: {}", e))?;

        let port = listener
            .local_addr()
            .map(|a| a.port())
            .map_err(|e| format!("get port: {}", e))?;

        self.port
            .store(port, std::sync::atomic::Ordering::SeqCst);

        info!("[WebSocketServer] Listening on 127.0.0.1:{}", port);

        // Spawn the accept loop
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let state = self.state.clone();
        let key_gen = self.key_gen.clone();

        tokio::spawn(async move {
            loop {
                let accept_result = tokio::select! {
                    result = listener.accept() => result,
                    _ = shutdown_rx.recv() => {
                        info!("[WebSocketServer] Accept loop shutting down");
                        return;
                    }
                };

                match accept_result {
                    Ok((stream, addr)) => {
                        debug!("[WebSocketServer] New connection from {}", addr);
                        Self::handle_new_connection(stream, &state, &key_gen).await;
                    }
                    Err(e) => {
                        warn!("[WebSocketServer] Accept error: {}", e);
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
                warn!("[WebSocketServer] WebSocket upgrade failed: {}", e);
                return;
            }
        };

        let (mut ws_write, mut ws_read) = ws_stream.split();

        // Read the first message for authentication
        let auth_msg = match ws_read.next().await {
            Some(Ok(WsMessage::Text(text))) => text,
            _ => {
                warn!("[WebSocketServer] Failed to read auth message");
                return;
            }
        };

        let auth: serde_json::Value = match serde_json::from_str(&auth_msg) {
            Ok(v) => v,
            Err(e) => {
                warn!("[WebSocketServer] Auth JSON parse error: {}", e);
                return;
            }
        };

        let key = match auth.get("key").and_then(|v| v.as_str()) {
            Some(k) => k.to_string(),
            None => {
                warn!("[WebSocketServer] No key in auth message");
                return;
            }
        };

        // Validate the key
        let validated = match key_gen.validate(&key) {
            Ok(v) => v,
            Err(e) => {
                warn!("[WebSocketServer] Auth failed: {}", e);
                return;
            }
        };

        info!(
            "[WebSocketServer] Child authenticated: PID={}, ChildID={:?}",
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
            "[WebSocketServer] Connection registered: UUID={}, ChildID={:?}",
            validated.key, validated.child_id
        );

        // Spawn write loop
        let write_key = validated.key.clone();
        tokio::spawn(async move {
            while let Some(data) = msg_rx.recv().await {
                if let Err(e) = ws_write.send(WsMessage::Text(data.into())).await {
                    warn!("[WebSocketServer] Write error for {}: {}", write_key, e);
                    break;
                }
            }
            debug!("[WebSocketServer] Write loop ended for {}", write_key);
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
                                debug!("[WebSocketServer] JSON decode error: {}", e);
                                continue;
                            }
                        };

                        if msg.jsonrpc != crate::websocket::protocol::VERSION {
                            debug!("[WebSocketServer] Non-protocol message ignored");
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
                        info!("[WebSocketServer] Close frame from {}", read_key);
                        break;
                    }
                    Err(e) => {
                        warn!("[WebSocketServer] Read error for {}: {}", read_key, e);
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

            debug!("[WebSocketServer] Connection removed: {}", read_key);
        });
    }

    /// Stop the WebSocket server.
    pub fn stop(&self) {
        info!("[WebSocketServer] Stopping...");
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
mod tests;
#[cfg(test)]
mod extra_tests;

//! TCP connection wrappers for cluster transport.
//!
//! Provides two levels of abstraction:
//! - `Connection` — synchronous framed TCP connection for simple use cases
//! - `TcpConn` — async connection with read/write loops, idle monitoring,
//!   auth token exchange, and dropped-message tracking (mirrors Go's `TCPConn`)

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream as TokioTcpStream;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tracing::{debug, trace, warn};

use super::frame::{
    decrypt_frame, derive_key, encrypt_frame, write_frame_async, AsyncFrameReader, AES_KEY_SIZE,
    MAX_FRAME_SIZE,
};

// ===========================================================================
// Synchronous Connection (backward-compatible)
// ===========================================================================

/// A framed TCP connection for sending and receiving binary messages.
pub struct Connection {
    stream: Option<TcpStream>,
    remote_addr: String,
}

/// Error type for connection operations.
#[derive(Debug, thiserror::Error)]
pub enum ConnectionError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Connection closed")]
    Closed,
    #[error("Not connected")]
    NotConnected,
}

impl Connection {
    /// Create a connection wrapper around an existing TCP stream.
    pub fn new(stream: TcpStream) -> Self {
        let remote_addr = stream
            .peer_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| "unknown".into());
        Self {
            stream: Some(stream),
            remote_addr,
        }
    }

    /// Connect to a remote address.
    pub fn connect(addr: &str) -> Result<Self, ConnectionError> {
        let stream = TcpStream::connect(addr)?;
        let remote_addr = stream
            .peer_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| addr.to_string());
        Ok(Self {
            stream: Some(stream),
            remote_addr,
        })
    }

    /// Send a framed message (4-byte length prefix + payload).
    pub fn send(&mut self, data: &[u8]) -> Result<(), ConnectionError> {
        let stream = self.stream.as_mut().ok_or(ConnectionError::NotConnected)?;
        let len = data.len() as u32;
        stream.write_all(&len.to_be_bytes())?;
        stream.write_all(data)?;
        stream.flush()?;
        Ok(())
    }

    /// Receive a framed message.
    pub fn recv(&mut self) -> Result<Vec<u8>, ConnectionError> {
        let stream = self.stream.as_mut().ok_or(ConnectionError::NotConnected)?;

        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf)?;
        let len = u32::from_be_bytes(len_buf) as usize;

        if len > MAX_FRAME_SIZE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Frame too large: {} bytes", len),
            )
            .into());
        }

        let mut data = vec![0u8; len];
        stream.read_exact(&mut data)?;
        Ok(data)
    }

    /// Close the connection.
    pub fn close(&mut self) {
        if let Some(stream) = self.stream.take() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }

    /// Check whether the connection is still open.
    pub fn is_connected(&self) -> bool {
        self.stream.is_some()
    }

    /// Get the remote address.
    pub fn remote_addr(&self) -> &str {
        &self.remote_addr
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.close();
    }
}

// ===========================================================================
// WireMessage — unified transport message (mirrors Go's RPCMessage)
// ===========================================================================

/// Unified wire message for the transport layer.
///
/// Corresponds to Go's `RPCMessage` with version, type, from/to, action, payload,
/// timestamp, and optional error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireMessage {
    /// Protocol version (currently "1.0").
    pub version: String,
    /// Unique message ID for request/response correlation.
    pub id: String,
    /// Message type: "request", "response", or "error".
    #[serde(rename = "type")]
    pub msg_type: String,
    /// Source node ID.
    pub from: String,
    /// Destination node ID.
    pub to: String,
    /// Action name (e.g., "peer_chat", "ping").
    pub action: String,
    /// JSON payload.
    #[serde(default)]
    pub payload: serde_json::Value,
    /// Unix timestamp in seconds (matching Go's time.Now().Unix()).
    pub timestamp: i64,
    /// Error message (non-empty for error responses).
    #[serde(default)]
    pub error: String,
}

impl WireMessage {
    const VERSION: &'static str = "1.0";

    /// Create a new request message.
    pub fn new_request(from: &str, to: &str, action: &str, payload: serde_json::Value) -> Self {
        let id = format!(
            "msg-{}-{:08x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            rand::random::<u32>()
        );
        Self {
            version: Self::VERSION.to_string(),
            id,
            msg_type: "request".to_string(),
            from: from.to_string(),
            to: to.to_string(),
            action: action.to_string(),
            payload,
            timestamp: chrono::Local::now().timestamp(),
            error: String::new(),
        }
    }

    /// Create a response to a request.
    pub fn new_response(request: &WireMessage, payload: serde_json::Value) -> Self {
        Self {
            version: request.version.clone(),
            id: request.id.clone(),
            msg_type: "response".to_string(),
            from: request.to.clone(),
            to: request.from.clone(),
            action: request.action.clone(),
            payload,
            timestamp: chrono::Local::now().timestamp(),
            error: String::new(),
        }
    }

    /// Create an error response to a request.
    pub fn new_error(request: &WireMessage, error: &str) -> Self {
        Self {
            version: request.version.clone(),
            id: request.id.clone(),
            msg_type: "error".to_string(),
            from: request.to.clone(),
            to: request.from.clone(),
            action: request.action.clone(),
            payload: serde_json::Value::Null,
            timestamp: chrono::Local::now().timestamp(),
            error: error.to_string(),
        }
    }

    /// Validate required fields.
    pub fn validate(&self) -> Result<(), String> {
        if self.version.is_empty() {
            return Err("missing version".into());
        }
        if self.id.is_empty() {
            return Err("missing id".into());
        }
        if self.from.is_empty() {
            return Err("missing from".into());
        }
        if self.to.is_empty() {
            return Err("missing to".into());
        }
        if self.action.is_empty() {
            return Err("missing action".into());
        }
        Ok(())
    }

    /// Serialize to JSON bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(self).map_err(|e| format!("JSON marshal error: {}", e))
    }

    /// Deserialize from JSON bytes.
    pub fn from_bytes(data: &[u8]) -> Result<Self, String> {
        serde_json::from_slice(data).map_err(|e| format!("JSON unmarshal error: {}", e))
    }

    /// Check if this is a request message.
    pub fn is_request(&self) -> bool {
        self.msg_type == "request"
    }

    /// Check if this is a response message.
    pub fn is_response(&self) -> bool {
        self.msg_type == "response"
    }

    /// Check if this is an error message.
    pub fn is_error(&self) -> bool {
        self.msg_type == "error"
    }
}

// ===========================================================================
// TcpConnConfig
// ===========================================================================

/// Configuration for an async TCP connection.
///
/// Mirrors Go's `TCPConnConfig`.
#[derive(Debug, Clone)]
pub struct TcpConnConfig {
    /// Local node ID.
    pub node_id: String,
    /// Remote address.
    pub address: String,
    /// Buffer size for the receive channel (default: 100).
    pub read_buffer_size: usize,
    /// Buffer size for the send channel (default: 100).
    pub send_buffer_size: usize,
    /// Timeout for sending a message to the write loop (default: 10s).
    pub send_timeout: Duration,
    /// Idle timeout before connection is closed (default: 30s).
    pub idle_timeout: Duration,
    /// Optional heartbeat interval. If set, sends heartbeat pings.
    pub heartbeat_interval: Option<Duration>,
    /// Optional auth token sent on connection start.
    pub auth_token: Option<String>,
}

impl Default for TcpConnConfig {
    fn default() -> Self {
        Self {
            node_id: String::new(),
            address: String::new(),
            read_buffer_size: 100,
            send_buffer_size: 100,
            send_timeout: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(30),
            heartbeat_interval: None,
            auth_token: None,
        }
    }
}

// ===========================================================================
// TcpConn — async connection with read/write loops
// ===========================================================================

/// An async TCP connection with dedicated read/write loops and idle monitoring.
///
/// Mirrors Go's `TCPConn`:
/// - Read loop: reads framed messages, JSON-parses them, sends to recv channel
/// - Write loop: reads from send channel, writes framed messages to TCP
/// - Idle monitor: periodically checks last activity, closes if idle too long
/// - Auth token: optionally sent on start before any data
pub struct TcpConn {
    node_id: String,
    address: String,
    local_addr: String,
    remote_addr: String,
    send_tx: mpsc::Sender<Vec<u8>>,
    recv_rx: Option<mpsc::Receiver<WireMessage>>,
    recv_tx_holder: Option<mpsc::Sender<WireMessage>>,
    close_tx: broadcast::Sender<()>,
    closed: Arc<AtomicBool>,
    started: AtomicBool,
    dropped_count: Arc<AtomicU64>,
    config: TcpConnConfig,
    created_at: Instant,
    last_used: Arc<RwLock<Instant>>,
    tasks: Vec<JoinHandle<()>>,
    /// Held until `start()` splits the stream.
    _conn: Option<TokioTcpStream>,
    /// Held until `start()` moves it into the write loop.
    _send_rx: Option<mpsc::Receiver<Vec<u8>>>,
}

impl TcpConn {
    /// Create a new TcpConn wrapping an existing tokio TCP stream.
    ///
    /// The connection is not started — call `start()` to launch the read/write
    /// loops.
    pub fn new(conn: TokioTcpStream, config: TcpConnConfig) -> Self {
        let local_addr = conn
            .local_addr()
            .map(|a| a.to_string())
            .unwrap_or_default();
        let remote_addr = conn
            .peer_addr()
            .map(|a| a.to_string())
            .unwrap_or_default();

        let (send_tx, send_rx) = mpsc::channel(config.send_buffer_size);
        let (recv_tx, recv_rx) = mpsc::channel(config.read_buffer_size);
        let (close_tx, _) = broadcast::channel(1);

        Self {
            node_id: config.node_id.clone(),
            address: config.address.clone(),
            local_addr,
            remote_addr,
            send_tx,
            recv_rx: Some(recv_rx),
            recv_tx_holder: Some(recv_tx),
            close_tx,
            closed: Arc::new(AtomicBool::new(false)),
            started: AtomicBool::new(false),
            dropped_count: Arc::new(AtomicU64::new(0)),
            config,
            created_at: Instant::now(),
            last_used: Arc::new(RwLock::new(Instant::now())),
            tasks: Vec::new(),
            // We need to store send_rx to move it into the write loop later
            _send_rx: Some(send_rx),
            _conn: Some(conn),
        }
    }

    /// Start the read/write/idle loops.
    ///
    /// If `auth_token` is configured, frames are AEAD-encrypted with
    /// AES-256-GCM using a key derived from the token (SHA-256). Both ends
    /// of the connection must use the same token; an authenticating peer
    /// produces ciphertext that fails GCM tag verification on the reader
    /// side, surfacing as a clean read error rather than a desync.
    pub async fn start(&mut self) -> Result<(), String> {
        if self.started.load(Ordering::SeqCst) {
            return Err("already started".into());
        }
        if self.closed.load(Ordering::SeqCst) {
            return Err("connection is closed".into());
        }

        let conn = self._conn.take().ok_or("no connection (already started)")?;
        let mut send_rx = self._send_rx.take().ok_or("send_rx already taken")?;
        let recv_tx = self.recv_tx_holder.take().ok_or("recv_tx already taken")?;

        // Derive an AES-256 key from the auth_token (if any). Both the read
        // and write loops use this key to decrypt/encrypt every frame payload.
        // Empty token → None → plaintext frames (same as legacy no-auth mode).
        let cipher_key: Option<[u8; AES_KEY_SIZE]> = self
            .config
            .auth_token
            .as_ref()
            .filter(|t| !t.is_empty())
            .map(|t| derive_key(t));

        // Save addresses before splitting
        let local = self.local_addr.clone();
        let remote = self.remote_addr.clone();

        // Split the TCP stream
        let (read_half, mut write_half) = tokio::io::split(conn);

        // Shared state for tasks
        let closed_r = self.closed.clone();
        let closed_w = self.closed.clone();
        let closed_i = self.closed.clone();
        let dropped_r = self.dropped_count.clone();
        let last_used_r = self.last_used.clone();
        let last_used_w = self.last_used.clone();
        let last_used_i = self.last_used.clone();
        let idle_timeout = self.config.idle_timeout;
        let heartbeat_interval = self.config.heartbeat_interval;
        let node_id = self.node_id.clone();
        let _node_id_w = node_id.clone();
        let _node_id_i = node_id.clone();
        let address = self.address.clone();
        let address_w = address.clone();
        let address_i = address.clone();

        // --- Read loop ---
        // If a cipher key is configured, every frame payload is decrypted
        // before being parsed as a WireMessage JSON. A wrong key (failed
        // authentication) surfaces as a decrypt error and closes the loop.
        let key_r = cipher_key;
        let read_task = tokio::spawn(async move {
            let mut reader = AsyncFrameReader::with_capacity(read_half, 4096);
            loop {
                if closed_r.load(Ordering::SeqCst) {
                    break;
                }
                match reader.read_frame().await {
                    Ok(ciphertext) => {
                        let plaintext = if let Some(ref key) = key_r {
                            match decrypt_frame(&ciphertext, key) {
                                Ok(pt) => pt,
                                Err(e) => {
                                    warn!(
                                        node_id = %node_id,
                                        address = %address,
                                        error = %e,
                                        "[Transport] Frame decrypt failed, closing connection"
                                    );
                                    break;
                                }
                            }
                        } else {
                            ciphertext
                        };
                        match WireMessage::from_bytes(&plaintext) {
                            Ok(msg) => {
                                trace!("[Transport] Received message: id={}, type={}", msg.id, msg.msg_type);
                                *last_used_r.write() = Instant::now();
                                if recv_tx.try_send(msg).is_err() {
                                    let count = dropped_r.fetch_add(1, Ordering::SeqCst);
                                    warn!(
                                        node_id = %node_id,
                                        address = %address,
                                        dropped = count + 1,
                                        "[Transport] Receive buffer full, dropping message"
                                    );
                                }
                            }
                            Err(e) => {
                                warn!(
                                    node_id = %node_id,
                                    address = %address,
                                    error = %e,
                                    "[Transport] Failed to parse wire message"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        if !closed_r.load(Ordering::SeqCst) {
                            // RPC clients use short-lived connections: each call
                            // opens a TCP stream, sends one request, reads one
                            // response, then closes. When the peer closes after
                            // a successful round-trip, read_frame surfaces this
                            // as UnexpectedEof. That's the normal happy path —
                            // log at debug so it doesn't pollute the log.
                            // Any other error kind (frame too large, network
                            // reset, decrypt mismatch surfaced as io error) is
                            // still a real problem worth a warn.
                            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                                debug!(
                                    node_id = %node_id,
                                    address = %address,
                                    "[Transport] Connection closed by peer"
                                );
                            } else {
                                warn!(
                                    node_id = %node_id,
                                    address = %address,
                                    error = %e,
                                    "[Transport] Read error, closing connection"
                                );
                            }
                        }
                        break;
                    }
                }
            }
        });

        // --- Write loop ---
        // If a cipher key is configured, every frame payload is encrypted
        // before being written to the TCP stream. This must mirror the read
        // loop's key decision (both derive from the same auth_token).
        let key_w = cipher_key;
        let write_task = tokio::spawn(async move {
            while let Some(data) = send_rx.recv().await {
                if closed_w.load(Ordering::SeqCst) {
                    break;
                }
                let wire_bytes = if let Some(ref key) = key_w {
                    match encrypt_frame(&data, key) {
                        Ok(ct) => ct,
                        Err(e) => {
                            warn!(
                                address = %address_w,
                                error = %e,
                                "[Transport] Frame encrypt failed, closing connection"
                            );
                            break;
                        }
                    }
                } else {
                    data
                };
                match write_frame_async(&mut write_half, &wire_bytes).await {
                    Ok(()) => {
                        *last_used_w.write() = Instant::now();
                    }
                    Err(e) => {
                        if !closed_w.load(Ordering::SeqCst) {
                            warn!(
                                address = %address_w,
                                error = %e,
                                "[Transport] Write error, closing connection"
                            );
                        }
                        break;
                    }
                }
            }
            // Shutdown write half
            let _ = write_half.shutdown().await;
        });

        // --- Idle monitor ---
        let idle_task = tokio::spawn(async move {
            let check_interval = idle_timeout / 2;
            let mut interval = tokio::time::interval(check_interval);
            loop {
                interval.tick().await;
                if closed_i.load(Ordering::SeqCst) {
                    break;
                }
                let elapsed = last_used_i.read().elapsed();
                if elapsed > idle_timeout {
                    warn!(
                        address = %address_i,
                        idle_for = ?elapsed,
                        "[Transport] Connection idle too long, closing"
                    );
                    break;
                }
            }
        });

        // --- Optional heartbeat ---
        let heartbeat_task = if let Some(hb_interval) = heartbeat_interval {
            let closed_h = self.closed.clone();
            let send_tx_h = self.send_tx.clone();
            let last_used_h = self.last_used.clone();
            Some(tokio::spawn(async move {
                let mut interval = tokio::time::interval(hb_interval);
                loop {
                    interval.tick().await;
                    if closed_h.load(Ordering::SeqCst) {
                        break;
                    }
                    // Build a heartbeat message (empty payload)
                    let mut buf = Vec::new();
                    let len = 0u32;
                    buf.extend_from_slice(&len.to_be_bytes());
                    // Send empty frame as heartbeat
                    if send_tx_h.try_send(buf).is_err() {
                        break;
                    }
                    let _ = last_used_h; // update last_used in write loop
                }
            }))
        } else {
            None
        };

        self.tasks.push(read_task);
        self.tasks.push(write_task);
        self.tasks.push(idle_task);
        if let Some(task) = heartbeat_task {
            self.tasks.push(task);
        }

        self.started.store(true, Ordering::SeqCst);
        debug!(
            node_id = %self.node_id,
            address = %self.address,
            local = %local,
            remote = %remote,
            "[Transport] TcpConn started"
        );
        Ok(())
    }

    /// Send a wire message through the connection.
    ///
    /// The message is JSON-serialized and queued in the send buffer.
    /// Returns an error if the send timeout elapses or the connection is closed.
    pub async fn send(&self, msg: &WireMessage) -> Result<(), String> {
        if self.closed.load(Ordering::SeqCst) {
            return Err("connection is closed".into());
        }
        let data = msg.to_bytes()?;
        tokio::time::timeout(self.config.send_timeout, async {
            self.send_tx
                .send(data)
                .await
                .map_err(|e| format!("send failed: {}", e))
        })
        .await
        .map_err(|_| "send timeout".to_string())?
    }

    /// Receive the next wire message from the connection.
    ///
    /// Returns `None` if the connection is closed and no more messages are
    /// buffered.
    pub async fn receive(&mut self) -> Option<WireMessage> {
        if let Some(ref mut rx) = self.recv_rx {
            rx.recv().await
        } else {
            None
        }
    }

    /// Close the connection gracefully.
    ///
    /// Signals all background tasks to stop and aborts them.
    /// Safe to call multiple times.
    pub fn close(&mut self) {
        if self
            .closed
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return; // Already closed
        }

        // Signal close to all tasks
        let _ = self.close_tx.send(());

        // Abort all tasks
        for task in self.tasks.drain(..) {
            task.abort();
        }

        debug!(
            node_id = %self.node_id,
            address = %self.address,
            dropped = self.dropped_count.load(Ordering::SeqCst),
            "[Transport] TcpConn closed"
        );
    }

    /// Check if the connection is active (started and not closed).
    pub fn is_active(&self) -> bool {
        !self.closed.load(Ordering::SeqCst) && self.started.load(Ordering::SeqCst)
    }

    /// Check if the connection is closed.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Get the local node ID.
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// Get the remote address.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Get the local socket address.
    pub fn local_addr(&self) -> &str {
        &self.local_addr
    }

    /// Get the remote socket address.
    pub fn remote_addr(&self) -> &str {
        &self.remote_addr
    }

    /// Get the time when this connection was created.
    pub fn created_at(&self) -> Instant {
        self.created_at
    }

    /// Get the time of last activity (read or write).
    pub fn last_used(&self) -> Instant {
        *self.last_used.read()
    }

    /// Update the last-used timestamp to now.
    ///
    /// Mirrors Go's `TCPConn.UpdateLastUsed`.
    pub fn update_last_used(&self) {
        *self.last_used.write() = Instant::now();
    }

    /// Set the node ID for this connection.
    ///
    /// Mirrors Go's `TCPConn.SetNodeID`.
    pub fn set_node_id(&mut self, node_id: String) {
        self.node_id = node_id;
    }

    /// Get the count of dropped messages (receive buffer full).
    pub fn dropped_count(&self) -> u64 {
        self.dropped_count.load(Ordering::SeqCst)
    }
}

impl Drop for TcpConn {
    fn drop(&mut self) {
        self.close();
    }
}

impl std::fmt::Debug for TcpConn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TcpConn")
            .field("node_id", &self.node_id)
            .field("address", &self.address)
            .field("local_addr", &self.local_addr)
            .field("remote_addr", &self.remote_addr)
            .field("closed", &self.closed.load(Ordering::SeqCst))
            .field("started", &self.started.load(Ordering::SeqCst))
            .field("dropped_count", &self.dropped_count.load(Ordering::SeqCst))
            .finish()
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests;

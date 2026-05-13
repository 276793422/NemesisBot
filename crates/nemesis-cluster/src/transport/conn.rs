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

use super::frame::{write_frame_async, AsyncFrameReader, MAX_FRAME_SIZE};

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
            timestamp: chrono::Utc::now().timestamp(),
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
            timestamp: chrono::Utc::now().timestamp(),
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
            timestamp: chrono::Utc::now().timestamp(),
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
    /// If `auth_token` is configured, it is sent as the first frame before
    /// the loops begin.
    pub async fn start(&mut self) -> Result<(), String> {
        if self.started.load(Ordering::SeqCst) {
            return Err("already started".into());
        }
        if self.closed.load(Ordering::SeqCst) {
            return Err("connection is closed".into());
        }

        let mut conn = self._conn.take().ok_or("no connection (already started)")?;
        let mut send_rx = self._send_rx.take().ok_or("send_rx already taken")?;
        let recv_tx = self.recv_tx_holder.take().ok_or("recv_tx already taken")?;

        // Send auth token if configured — as a plain text line matching Go's
        // `conn.Write([]byte(tc.authToken + "\n"))` protocol.  The server
        // reads this with `read_line('\n')` *before* switching to framed mode.
        if let Some(ref token) = self.config.auth_token {
            use tokio::io::AsyncWriteExt;
            // Write token + newline directly (not framed)
            let auth_bytes = format!("{}\n", token);
            conn.write_all(auth_bytes.as_bytes())
                .await
                .map_err(|e| format!("auth token send failed: {}", e))?;
            conn.flush()
                .await
                .map_err(|e| format!("auth token flush failed: {}", e))?;
            trace!("Auth token sent to {}", self.address);
        }

        // Save addresses before splitting
        let local = self.local_addr.clone();
        let remote = self.remote_addr.clone();

        // Split the TCP stream
        let (read_half, mut write_half) = tokio::io::split(conn);

        // Send auth token again through the write half if needed
        // (Already sent above before split)

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
        let read_task = tokio::spawn(async move {
            let mut reader = AsyncFrameReader::with_capacity(read_half, 4096);
            loop {
                if closed_r.load(Ordering::SeqCst) {
                    break;
                }
                match reader.read_frame().await {
                    Ok(data) => {
                        match WireMessage::from_bytes(&data) {
                            Ok(msg) => {
                                trace!("Received message: id={}, type={}", msg.id, msg.msg_type);
                                *last_used_r.write() = Instant::now();
                                if recv_tx.try_send(msg).is_err() {
                                    let count = dropped_r.fetch_add(1, Ordering::SeqCst);
                                    warn!(
                                        node_id = %node_id,
                                        address = %address,
                                        dropped = count + 1,
                                        "Receive buffer full, dropping message"
                                    );
                                }
                            }
                            Err(e) => {
                                warn!(
                                    node_id = %node_id,
                                    address = %address,
                                    error = %e,
                                    "Failed to parse wire message"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        if !closed_r.load(Ordering::SeqCst) {
                            warn!(
                                node_id = %node_id,
                                address = %address,
                                error = %e,
                                "Read error, closing connection"
                            );
                        }
                        break;
                    }
                }
            }
        });

        // --- Write loop ---
        let write_task = tokio::spawn(async move {
            while let Some(data) = send_rx.recv().await {
                if closed_w.load(Ordering::SeqCst) {
                    break;
                }
                match write_frame_async(&mut write_half, &data).await {
                    Ok(()) => {
                        *last_used_w.write() = Instant::now();
                    }
                    Err(e) => {
                        if !closed_w.load(Ordering::SeqCst) {
                            warn!(
                                address = %address_w,
                                error = %e,
                                "Write error, closing connection"
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
                        "Connection idle too long, closing"
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
            "TcpConn started"
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
            "TcpConn closed"
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
mod tests {
    use super::*;

    #[test]
    fn test_connection_lifecycle() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        let client = Connection::connect(&addr).unwrap();
        assert!(client.is_connected());
        assert!(!client.remote_addr().is_empty());
    }

    #[test]
    fn test_send_recv_roundtrip() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        let mut client = Connection::connect(&addr).unwrap();
        let (server_stream, _) = listener.accept().unwrap();
        let mut server = Connection::new(server_stream);

        client.send(b"hello world").unwrap();
        let data = server.recv().unwrap();
        assert_eq!(data, b"hello world");
    }

    #[test]
    fn test_close() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        let mut client = Connection::connect(&addr).unwrap();
        assert!(client.is_connected());
        client.close();
        assert!(!client.is_connected());
    }

    #[test]
    fn test_wire_message_new_request() {
        let msg = WireMessage::new_request("node-a", "node-b", "ping", serde_json::json!({}));
        assert_eq!(msg.msg_type, "request");
        assert_eq!(msg.from, "node-a");
        assert_eq!(msg.to, "node-b");
        assert_eq!(msg.action, "ping");
        assert!(msg.validate().is_ok());
    }

    #[test]
    fn test_wire_message_new_response() {
        let req = WireMessage::new_request("a", "b", "test", serde_json::json!({}));
        let resp = WireMessage::new_response(&req, serde_json::json!({"ok": true}));
        assert_eq!(resp.msg_type, "response");
        assert_eq!(resp.id, req.id);
        assert_eq!(resp.from, "b");
        assert_eq!(resp.to, "a");
        assert!(resp.is_response());
    }

    #[test]
    fn test_wire_message_new_error() {
        let req = WireMessage::new_request("a", "b", "test", serde_json::json!({}));
        let err = WireMessage::new_error(&req, "something went wrong");
        assert_eq!(err.msg_type, "error");
        assert_eq!(err.error, "something went wrong");
        assert!(err.is_error());
    }

    #[test]
    fn test_wire_message_validate() {
        let msg = WireMessage::new_request("a", "b", "c", serde_json::json!({}));
        assert!(msg.validate().is_ok());

        let bad = WireMessage {
            version: String::new(),
            id: String::new(),
            msg_type: "request".into(),
            from: String::new(),
            to: String::new(),
            action: String::new(),
            payload: serde_json::Value::Null,
            timestamp: 0,
            error: String::new(),
        };
        assert!(bad.validate().is_err());
    }

    #[test]
    fn test_wire_message_serialization() {
        let msg = WireMessage::new_request("a", "b", "ping", serde_json::json!({"key": "val"}));
        let bytes = msg.to_bytes().unwrap();
        let back = WireMessage::from_bytes(&bytes).unwrap();
        assert_eq!(back.id, msg.id);
        assert_eq!(back.from, "a");
        assert_eq!(back.to, "b");
        assert_eq!(back.action, "ping");
    }

    #[tokio::test]
    async fn test_tcp_conn_send_receive() {
        // Set up a TCP listener
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // Channel to signal when server has sent its response
        let (server_done_tx, server_done_rx) = tokio::sync::oneshot::channel();

        // Server side: accept and create a TcpConn
        let server_addr = addr;
        let server_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut server_conn = TcpConn::new(
                stream,
                TcpConnConfig {
                    address: server_addr.to_string(),
                    ..Default::default()
                },
            );
            server_conn.start().await.unwrap();

            // Read a message
            let msg = server_conn.receive().await.unwrap();
            assert_eq!(msg.action, "hello");
            assert_eq!(msg.from, "client");

            // Send a response
            let resp = WireMessage::new_response(&msg, serde_json::json!({"status": "ok"}));
            server_conn.send(&resp).await.unwrap();

            // Give the write loop time to flush the data to the TCP stream
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;

            // Signal that the response has been sent
            let _ = server_done_tx.send(());
        });

        // Client side
        let client_stream = TokioTcpStream::connect(addr).await.unwrap();
        let mut client_conn = TcpConn::new(
            client_stream,
            TcpConnConfig {
                node_id: "client".into(),
                address: addr.to_string(),
                ..Default::default()
            },
        );
        client_conn.start().await.unwrap();

        // Send a request
        let req = WireMessage::new_request("client", "server", "hello", serde_json::json!({}));
        client_conn.send(&req).await.unwrap();

        // Receive response
        let resp = client_conn.receive().await.unwrap();
        assert_eq!(resp.id, req.id);
        assert_eq!(resp.msg_type, "response");

        // Wait for server to finish
        server_done_rx.await.unwrap();
        let _ = server_handle.await;
    }

    #[tokio::test]
    async fn test_tcp_conn_close() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let _ = stream; // just accept
        });

        let client_stream = TokioTcpStream::connect(addr).await.unwrap();
        let mut client = TcpConn::new(client_stream, TcpConnConfig::default());
        client.start().await.unwrap();
        assert!(client.is_active());
        assert!(!client.is_closed());

        client.close();
        assert!(!client.is_active());
        assert!(client.is_closed());

        // Double close is safe
        client.close();
        assert!(client.is_closed());

        server_handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_tcp_conn_auth_token() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let token = "secret-token-123";

        // Server: accept and read auth line (plain text with newline)
        let server_handle = tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let (stream, _) = listener.accept().await.unwrap();
            let (read_half, _) = tokio::io::split(stream);
            let mut reader = tokio::io::BufReader::new(read_half);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            // The client sends "token\n"; trim the newline for comparison
            assert_eq!(line.trim(), token);
        });

        // Client: start with auth token (sent as plain text line)
        let client_stream = TokioTcpStream::connect(addr).await.unwrap();
        let mut client = TcpConn::new(
            client_stream,
            TcpConnConfig {
                auth_token: Some(token.to_string()),
                ..Default::default()
            },
        );
        client.start().await.unwrap();

        server_handle.await.unwrap();
    }

    #[test]
    fn test_tcp_conn_config_default() {
        let config = TcpConnConfig::default();
        assert_eq!(config.read_buffer_size, 100);
        assert_eq!(config.send_buffer_size, 100);
        assert_eq!(config.send_timeout, Duration::from_secs(10));
        assert_eq!(config.idle_timeout, Duration::from_secs(30));
        assert!(config.auth_token.is_none());
        assert!(config.heartbeat_interval.is_none());
    }

    // ============================================================
    // Coverage improvement: WireMessage validation, Connection errors, TcpConn state
    // ============================================================

    #[test]
    fn test_wire_message_validate_missing_version() {
        let msg = WireMessage {
            version: String::new(),
            id: "id".into(),
            msg_type: "request".into(),
            from: "a".into(),
            to: "b".into(),
            action: "c".into(),
            payload: serde_json::Value::Null,
            timestamp: 0,
            error: String::new(),
        };
        assert_eq!(msg.validate(), Err("missing version".into()));
    }

    #[test]
    fn test_wire_message_validate_missing_id() {
        let msg = WireMessage {
            version: "1.0".into(),
            id: String::new(),
            msg_type: "request".into(),
            from: "a".into(),
            to: "b".into(),
            action: "c".into(),
            payload: serde_json::Value::Null,
            timestamp: 0,
            error: String::new(),
        };
        assert_eq!(msg.validate(), Err("missing id".into()));
    }

    #[test]
    fn test_wire_message_validate_missing_from() {
        let msg = WireMessage {
            version: "1.0".into(),
            id: "id".into(),
            msg_type: "request".into(),
            from: String::new(),
            to: "b".into(),
            action: "c".into(),
            payload: serde_json::Value::Null,
            timestamp: 0,
            error: String::new(),
        };
        assert_eq!(msg.validate(), Err("missing from".into()));
    }

    #[test]
    fn test_wire_message_validate_missing_to() {
        let msg = WireMessage {
            version: "1.0".into(),
            id: "id".into(),
            msg_type: "request".into(),
            from: "a".into(),
            to: String::new(),
            action: "c".into(),
            payload: serde_json::Value::Null,
            timestamp: 0,
            error: String::new(),
        };
        assert_eq!(msg.validate(), Err("missing to".into()));
    }

    #[test]
    fn test_wire_message_validate_missing_action() {
        let msg = WireMessage {
            version: "1.0".into(),
            id: "id".into(),
            msg_type: "request".into(),
            from: "a".into(),
            to: "b".into(),
            action: String::new(),
            payload: serde_json::Value::Null,
            timestamp: 0,
            error: String::new(),
        };
        assert_eq!(msg.validate(), Err("missing action".into()));
    }

    #[test]
    fn test_wire_message_from_bytes_invalid() {
        let result = WireMessage::from_bytes(b"not json");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("JSON unmarshal error"));
    }

    #[test]
    fn test_wire_message_serialization_roundtrip_full() {
        let msg = WireMessage {
            version: "1.0".into(),
            id: "test-id".into(),
            msg_type: "request".into(),
            from: "node-a".into(),
            to: "node-b".into(),
            action: "ping".into(),
            payload: serde_json::json!({"key": "value", "num": 42}),
            timestamp: 1715385600,
            error: String::new(),
        };
        let bytes = msg.to_bytes().unwrap();
        let back = WireMessage::from_bytes(&bytes).unwrap();
        assert_eq!(back.version, "1.0");
        assert_eq!(back.id, "test-id");
        assert_eq!(back.from, "node-a");
        assert_eq!(back.to, "node-b");
        assert_eq!(back.action, "ping");
        assert_eq!(back.timestamp, 1715385600);
        assert_eq!(back.payload["key"], "value");
        assert_eq!(back.payload["num"], 42);
    }

    #[test]
    fn test_wire_message_error_with_message() {
        let req = WireMessage::new_request("a", "b", "test", serde_json::json!({}));
        let err = WireMessage::new_error(&req, "something failed");
        assert_eq!(err.msg_type, "error");
        assert_eq!(err.error, "something failed");
        assert_eq!(err.from, "b");
        assert_eq!(err.to, "a");
        assert_eq!(err.id, req.id);
        assert!(err.payload.is_null());
    }

    #[test]
    fn test_connection_double_close_safe() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let mut client = Connection::connect(&addr).unwrap();
        client.close();
        assert!(!client.is_connected());
        client.close(); // Second close should not panic
        assert!(!client.is_connected());
    }

    #[test]
    fn test_connection_send_after_close_errors() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let mut client = Connection::connect(&addr).unwrap();
        client.close();
        let result = client.send(b"test");
        assert!(result.is_err());
    }

    #[test]
    fn test_connection_recv_after_close_errors() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let mut client = Connection::connect(&addr).unwrap();
        client.close();
        let result = client.recv();
        assert!(result.is_err());
    }

    #[test]
    fn test_connection_error_display() {
        let err = ConnectionError::Closed;
        assert_eq!(format!("{}", err), "Connection closed");
        let err = ConnectionError::NotConnected;
        assert_eq!(format!("{}", err), "Not connected");
    }

    #[tokio::test]
    async fn test_tcp_conn_not_started_not_active() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let _ = stream;
        });
        let client_stream = TokioTcpStream::connect(addr).await.unwrap();
        let client = TcpConn::new(client_stream, TcpConnConfig::default());
        assert!(!client.is_active());
        assert!(!client.is_closed());
        assert!(client.node_id().is_empty());
        assert!(client.address().is_empty());
        server_handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_tcp_conn_accessors() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let _ = stream;
        });
        let client_stream = TokioTcpStream::connect(addr).await.unwrap();
        let config = TcpConnConfig {
            node_id: "test-node".into(),
            address: addr.to_string(),
            ..Default::default()
        };
        let client = TcpConn::new(client_stream, config);
        assert_eq!(client.node_id(), "test-node");
        assert_eq!(client.address(), addr.to_string());
        assert_eq!(client.dropped_count(), 0);
        let _created = client.created_at();
        let _last_used = client.last_used();
        server_handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_tcp_conn_set_node_id() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let _ = stream;
        });
        let client_stream = TokioTcpStream::connect(addr).await.unwrap();
        let mut client = TcpConn::new(client_stream, TcpConnConfig::default());
        assert_eq!(client.node_id(), "");
        client.set_node_id("new-node-id".into());
        assert_eq!(client.node_id(), "new-node-id");
        server_handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_tcp_conn_send_closed_errors() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let _ = stream;
        });
        let client_stream = TokioTcpStream::connect(addr).await.unwrap();
        let mut client = TcpConn::new(client_stream, TcpConnConfig::default());
        client.close();
        let msg = WireMessage::new_request("a", "b", "test", serde_json::json!({}));
        let result = client.send(&msg).await;
        assert!(result.is_err());
        server_handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_tcp_conn_close_marks_as_closed() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let _ = stream;
        });
        let client_stream = TokioTcpStream::connect(addr).await.unwrap();
        let mut client = TcpConn::new(client_stream, TcpConnConfig::default());
        assert!(!client.is_closed());
        client.close();
        assert!(client.is_closed());
        assert!(!client.is_active());
        server_handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_tcp_conn_start_twice_errors() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let _ = stream;
        });
        let client_stream = TokioTcpStream::connect(addr).await.unwrap();
        let mut client = TcpConn::new(client_stream, TcpConnConfig::default());
        client.start().await.unwrap();
        let result = client.start().await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already started"));
        client.close();
        server_handle.await.unwrap();
    }

    #[test]
    fn test_tcp_conn_config_debug() {
        let config = TcpConnConfig {
            node_id: "test".into(),
            address: "127.0.0.1:8080".into(),
            ..Default::default()
        };
        let debug = format!("{:?}", config);
        assert!(debug.contains("test"));
        assert!(debug.contains("127.0.0.1:8080"));
    }

    // ============================================================
    // Coverage improvement: more edge cases
    // ============================================================

    #[test]
    fn test_wire_message_new_response_flips_from_to() {
        let req = WireMessage::new_request("client", "server", "ping", serde_json::json!({}));
        let resp = WireMessage::new_response(&req, serde_json::json!({"ok": true}));
        assert_eq!(resp.from, "server");
        assert_eq!(resp.to, "client");
        assert_eq!(resp.msg_type, "response");
        assert_eq!(resp.action, "ping");
        assert_eq!(resp.id, req.id);
    }

    #[test]
    fn test_wire_message_new_error_flips_from_to() {
        let req = WireMessage::new_request("client", "server", "ping", serde_json::json!({}));
        let err = WireMessage::new_error(&req, "test error");
        assert_eq!(err.from, "server");
        assert_eq!(err.to, "client");
        assert_eq!(err.msg_type, "error");
        assert_eq!(err.error, "test error");
    }

    #[test]
    fn test_wire_message_is_error() {
        let req = WireMessage::new_request("a", "b", "test", serde_json::json!({}));
        assert!(!req.is_error());
        let err = WireMessage::new_error(&req, "fail");
        assert!(err.is_error());
    }

    #[test]
    fn test_wire_message_validate_valid() {
        let msg = WireMessage::new_request("a", "b", "test", serde_json::json!({}));
        assert!(msg.validate().is_ok());
    }

    #[test]
    fn test_connection_error_io_variant() {
        let err = ConnectionError::Io(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "io error"));
        assert!(format!("{}", err).contains("io error"));
    }

    #[tokio::test]
    async fn test_tcp_conn_receive_closed_returns_none() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            drop(stream); // Immediately close
        });

        let client_stream = TokioTcpStream::connect(addr).await.unwrap();
        let mut client = TcpConn::new(client_stream, TcpConnConfig::default());
        client.start().await.unwrap();

        // Wait for server to close
        server_handle.await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Receive on closed connection should return None
        let result = client.receive().await;
        assert!(result.is_none());
    }

    #[test]
    fn test_connection_connect_to_invalid_addr() {
        let result = Connection::connect("999.999.999.999:99999");
        assert!(result.is_err());
    }
}

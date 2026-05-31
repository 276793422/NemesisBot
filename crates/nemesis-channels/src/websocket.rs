//! WebSocket channel implementation.
//!
//! Standalone WebSocket server for external program integration.
//! Runs on a separate port (default 49001), supports single client connection,
//! simple JSON protocol for message exchange.
//!
//! Mirrors Go's `module/channels/websocket_channel.go`.

use async_trait::async_trait;
use chrono::Utc;
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::SystemTime;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::{handshake::server::{Request, Response}, Message};
use tracing::{debug, info, warn};

use nemesis_types::channel::{InboundMessage, OutboundMessage};
use nemesis_types::error::{NemesisError, Result};

use crate::base::{BaseChannel, Channel};

// ---------------------------------------------------------------------------
// Protocol types
// ---------------------------------------------------------------------------

/// Message sent from the client to the server.
#[derive(serde::Deserialize, Debug)]
struct ClientMessage {
    #[serde(rename = "type")]
    msg_type: String,
    #[serde(default)]
    content: String,
}

/// Message sent from the server to the client.
#[derive(serde::Serialize)]
struct ServerMessage {
    #[serde(rename = "type")]
    msg_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    timestamp: String,
}

impl ServerMessage {
    /// Returns current UTC time as ISO 8601 / RFC 3339 string (matches Go's `time.Time` JSON format).
    fn now_timestamp() -> String {
        Utc::now().to_rfc3339()
    }

    fn message(role: &'static str, content: String) -> Self {
        Self {
            msg_type: "message",
            role: Some(role),
            content: Some(content),
            error: None,
            timestamp: Self::now_timestamp(),
        }
    }

    fn pong() -> Self {
        Self {
            msg_type: "pong",
            role: None,
            content: None,
            error: None,
            timestamp: Self::now_timestamp(),
        }
    }

    fn error_msg(err: impl ToString) -> Self {
        Self {
            msg_type: "error",
            role: None,
            content: None,
            error: Some(err.to_string()),
            timestamp: Self::now_timestamp(),
        }
    }
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Configuration for the WebSocket channel.
///
/// Passed from `gateway.rs` → `ChannelInitConfig` → `WebSocketChannel::new()`.
#[derive(Debug, Clone, Default)]
pub struct WebSocketChannelConfig {
    pub host: String,
    pub port: u16,
    pub path: String,
    pub auth_token: String,
    pub allow_from: Vec<String>,
    pub sync_to: Vec<String>,
}

// ---------------------------------------------------------------------------
// Active connection (send-queue pattern)
// ---------------------------------------------------------------------------

/// Holds the send-end of an mpsc channel connected to a spawned writer task.
/// The writer task owns the actual `SplitSink` and writes to the WebSocket.
struct ActiveConnection {
    send_tx: tokio::sync::mpsc::UnboundedSender<String>,
    client_id: String,
}

// ---------------------------------------------------------------------------
// WebSocketChannel
// ---------------------------------------------------------------------------

/// Standalone WebSocket server channel.
///
/// Accepts one client at a time on `{host}:{port}{path}`. Inbound messages
/// are published to the message bus; outbound messages are written to the
/// connected WebSocket client via a send-queue.
pub struct WebSocketChannel {
    base: Arc<BaseChannel>,
    config: WebSocketChannelConfig,
    bus_sender: tokio::sync::broadcast::Sender<InboundMessage>,
    /// Current active connection. `None` when no client is connected.
    /// Wrapped in `Arc` so the reader task can clear it on disconnect.
    active_conn: Arc<parking_lot::Mutex<Option<ActiveConnection>>>,
    /// Handle to the accept-loop task.
    accept_task: parking_lot::Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Handles to all reader tasks. Aborted on stop().
    reader_tasks: Arc<parking_lot::Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}

impl WebSocketChannel {
    /// Creates a new `WebSocketChannel`.
    pub fn new(
        config: WebSocketChannelConfig,
        bus_sender: tokio::sync::broadcast::Sender<InboundMessage>,
    ) -> Self {
        let allow_from = config.allow_from.clone();
        Self {
            base: Arc::new(BaseChannel::with_allow_list("websocket", allow_from)),
            config,
            bus_sender,
            active_conn: Arc::new(parking_lot::Mutex::new(None)),
            accept_task: parking_lot::Mutex::new(None),
            reader_tasks: Arc::new(parking_lot::Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl Channel for WebSocketChannel {
    fn name(&self) -> &str {
        self.base.name()
    }

    async fn start(&self) -> Result<()> {
        let addr = format!("{}:{}", self.config.host, self.config.port);
        let listener = TcpListener::bind(&addr)
            .await
            .map_err(|e| NemesisError::Channel(format!("WebSocket bind failed: {e}")))?;

        let local_addr = listener.local_addr().unwrap_or_else(|_| addr.parse().unwrap_or_else(|_| "0.0.0.0:0".parse().unwrap()));
        info!(
            host = %self.config.host,
            port = %self.config.port,
            path = %self.config.path,
            local_addr = %local_addr,
            "[WebSocketChannel] server listening"
        );

        self.base.set_running(true);

        // Clone fields for the accept loop
        let config = self.config.clone();
        let bus_sender = self.bus_sender.clone();
        let active_conn = Arc::clone(&self.active_conn);
        let reader_tasks = Arc::clone(&self.reader_tasks);
        let base_name = self.base.name().to_string();
        let base_allow_list = self.config.allow_from.clone();
        let base_for_reader = Arc::clone(&self.base);

        let accept_task = tokio::spawn(async move {
            loop {
                let accept_result = listener.accept().await;
                match accept_result {
                    Ok((stream, remote_addr)) => {
                        debug!(addr = %remote_addr, "[WebSocketChannel] new TCP connection");

                        // Single-client check
                        {
                            let guard = active_conn.lock();
                            if guard.is_some() {
                                warn!(addr = %remote_addr, "[WebSocketChannel] rejected: client already connected");
                                drop(guard);
                                // Just drop the stream — client gets connection reset
                                continue;
                            }
                        }

                        // WebSocket upgrade (with optional path check and auth)
                        let ws_path = config.path.clone();
                        let ws_stream = if config.auth_token.is_empty() {
                            tokio_tungstenite::accept_hdr_async(stream, move |req: &Request, response: Response| {
                                // Path validation
                                if !ws_path.is_empty() && req.uri().path() != ws_path {
                                    warn!(path = %req.uri().path(), expected = %ws_path, "[WebSocketChannel] rejected: wrong path");
                                    return Err(http::Response::builder()
                                        .status(404)
                                        .body(Some("Not Found".to_string()))
                                        .unwrap());
                                }
                                Ok(response)
                            }).await
                        } else {
                            let token = config.auth_token.clone();
                            let auth_ws_path = config.path.clone();
                            tokio_tungstenite::accept_hdr_async(stream, move |req: &Request, response: Response| {
                                // Path validation
                                if !auth_ws_path.is_empty() && req.uri().path() != auth_ws_path {
                                    warn!(path = %req.uri().path(), expected = %auth_ws_path, "[WebSocketChannel] rejected: wrong path");
                                    return Err(http::Response::builder()
                                        .status(404)
                                        .body(Some("Not Found".to_string()))
                                        .unwrap());
                                }
                                // Token validation
                                let query = req.uri().query().unwrap_or("");
                                let mut valid = false;
                                for pair in query.split('&') {
                                    if let Some((key, value)) = pair.split_once('=') {
                                        if key == "token" && value == token {
                                            valid = true;
                                            break;
                                        }
                                    }
                                }
                                if valid {
                                    Ok(response)
                                } else {
                                    warn!("[WebSocketChannel] auth failed: invalid or missing token");
                                    Err(http::Response::builder()
                                        .status(401)
                                        .body(Some("Unauthorized".to_string()))
                                        .unwrap())
                                }
                            })
                            .await
                        };

                        let ws_stream = match ws_stream {
                            Ok(s) => s,
                            Err(e) => {
                                warn!(addr = %remote_addr, error = %e, "[WebSocketChannel] upgrade failed");
                                continue;
                            }
                        };

                        let (sink, stream) = ws_stream.split();

                        // Create send-queue
                        let (send_tx, mut send_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
                        let client_id = format!("client_{}", SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs());

                        // Store active connection
                        {
                            let mut guard = active_conn.lock();
                            *guard = Some(ActiveConnection {
                                send_tx: send_tx.clone(),
                                client_id: client_id.clone(),
                            });
                        }

                        info!(client_id = %client_id, addr = %remote_addr, "[WebSocketChannel] client connected");

                        // Send welcome message
                        let welcome = ServerMessage::message(
                            "system",
                            format!("Connected to NemesisBot WebSocket channel. Client ID: {client_id}"),
                        );
                        if let Ok(json) = serde_json::to_string(&welcome) {
                            let _ = send_tx.send(json);
                        }

                        // Spawn writer task
                        tokio::spawn(async move {
                            let mut sink = sink;
                            while let Some(data) = send_rx.recv().await {
                                if sink.send(Message::Text(data.into())).await.is_err() {
                                    break;
                                }
                            }
                            let _ = sink.close().await;
                        });

                        // Spawn reader task
                        let read_active_conn = Arc::clone(&active_conn);
                        let read_bus = bus_sender.clone();
                        let read_send_tx = send_tx.clone();
                        let read_client_id = client_id.clone();
                        let read_base_name = base_name.clone();
                        let read_allow_list = base_allow_list.clone();
                        let read_base = Arc::clone(&base_for_reader);

                        let read_task = tokio::spawn(async move {
                            let mut stream = stream;
                            while let Some(msg_result) = stream.next().await {
                                match msg_result {
                                    Ok(Message::Text(text)) => {
                                        let sync_content = handle_text_message(
                                            &text,
                                            &read_client_id,
                                            &read_bus,
                                            &read_send_tx,
                                            &read_base_name,
                                            &read_allow_list,
                                        );
                                        // Matches Go's HandleMessage() which calls record_received()
                                        if sync_content.is_some() {
                                            read_base.record_received();
                                        }
                                        // Inbound sync: mirror to other channels (matches Go's SyncToTargets in handleConnection)
                                        if let Some(content) = sync_content {
                                            read_base.sync_to_targets(&content).await;
                                        }
                                    }
                                    Ok(Message::Close(_)) => {
                                        info!(client_id = %read_client_id, "[WebSocketChannel] close frame received");
                                        break;
                                    }
                                    Ok(Message::Binary(_)) => {
                                        warn!(client_id = %read_client_id, "[WebSocketChannel] received binary message, ignoring");
                                    }
                                    Ok(_) => {}
                                    Err(e) => {
                                        debug!(client_id = %read_client_id, error = %e, "[WebSocketChannel] read error");
                                        break;
                                    }
                                }
                            }

                            // Cleanup: clear active connection (matches Go's defer cleanup)
                            {
                                let mut guard = read_active_conn.lock();
                                if let Some(ref conn) = *guard {
                                    if conn.client_id == read_client_id {
                                        *guard = None;
                                    }
                                }
                            }
                            info!(client_id = %read_client_id, "[WebSocketChannel] client disconnected");
                        });

                        // Track reader task for clean shutdown
                        reader_tasks.lock().push(read_task);
                    }
                    Err(e) => {
                        warn!(error = %e, "[WebSocketChannel] accept error");
                    }
                }
            }
        });

        *self.accept_task.lock() = Some(accept_task);
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("[WebSocketChannel] stopping");

        // Step 1: Stop accepting new connections
        if let Some(handle) = self.accept_task.lock().take() {
            handle.abort();
        }

        // Step 2: Graceful shutdown — send close frame and wait for reader tasks to exit
        {
            let mut guard = self.active_conn.lock();
            if let Some(conn) = guard.take() {
                // Send WebSocket close frame through the send queue.
                // The writer task will send it to the client, then the reader task
                // will receive the close response and exit naturally.
                let close_msg = ServerMessage {
                    msg_type: "close",
                    role: None,
                    content: None,
                    error: None,
                    timestamp: ServerMessage::now_timestamp(),
                };
                if let Ok(json) = serde_json::to_string(&close_msg) {
                    let _ = conn.send_tx.send(json);
                }
                // Drop send_tx — writer task will flush remaining messages, then close the sink
            }
        }

        // Step 3: Wait for reader tasks to finish (graceful), with timeout fallback (abort)
        let handles: Vec<_> = self.reader_tasks.lock().drain(..).collect();
        if !handles.is_empty() {
            let timeout_dur = tokio::time::Duration::from_secs(3);
            for handle in handles {
                // Abort the handle if timeout fires (abort is harmless if task already finished)
                let abort_handle = handle.abort_handle();
                match tokio::time::timeout(timeout_dur, handle).await {
                    Ok(Ok(())) | Ok(Err(_)) => {} // task exited (normally or with error)
                    Err(_) => {
                        abort_handle.abort();
                    }
                }
            }
        }

        self.base.set_running(false);
        info!("[WebSocketChannel] stopped");
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !self.base.is_running() {
            return Err(NemesisError::Channel("websocket channel not running".to_string()));
        }

        let json = {
            let guard = self.active_conn.lock();
            match guard.as_ref() {
                Some(conn) => {
                    let server_msg = ServerMessage::message("assistant", msg.content.clone());
                    let json = serde_json::to_string(&server_msg)
                        .map_err(|e| NemesisError::Channel(format!("serialize failed: {e}")))?;
                    conn.send_tx.send(json.clone())
                        .map_err(|_| NemesisError::Channel("websocket send failed: client disconnected".to_string()))?;
                    self.base.record_sent();
                    json
                }
                None => {
                    return Err(NemesisError::Channel("no websocket client connected".to_string()));
                }
            }
        };
        let _ = json;

        // Outbound sync: forward to other channels (e.g., web dashboard).
        self.base.sync_to_targets(&msg.content).await;

        Ok(())
    }

    fn is_running(&self) -> bool {
        self.base.is_running()
    }

    fn is_allowed(&self, sender_id: &str) -> bool {
        self.base.is_allowed(sender_id)
    }

    fn add_sync_target(&self, name: &str, channel: Arc<dyn Channel>) -> Result<()> {
        self.base.add_sync_target(name, channel)
    }

    fn remove_sync_target(&self, name: &str) {
        self.base.remove_sync_target(name);
    }
}

// ---------------------------------------------------------------------------
// Message handling
// ---------------------------------------------------------------------------

/// Handles a single text message from a WebSocket client.
///
/// Returns `Some(content)` if the message was a valid "message" type that was
/// published to the bus (used for inbound sync_to_targets). Returns `None` otherwise.
fn handle_text_message(
    text: &str,
    client_id: &str,
    bus_sender: &tokio::sync::broadcast::Sender<InboundMessage>,
    send_tx: &tokio::sync::mpsc::UnboundedSender<String>,
    base_name: &str,
    allow_list: &[String],
) -> Option<String> {
    let client_msg: ClientMessage = match serde_json::from_str(text) {
        Ok(m) => m,
        Err(e) => {
            debug!(error = %e, data = %text, "[WebSocketChannel] failed to parse client message");
            send_error(send_tx, "Invalid message format");
            return None;
        }
    };

    match client_msg.msg_type.as_str() {
        "message" => {
            if client_msg.content.is_empty() {
                send_error(send_tx, "Message content cannot be empty");
                return None;
            }

            let chat_id = format!("websocket:{client_id}");
            info!(content = %client_msg.content, chat_id = %chat_id, "[WebSocketChannel] received message");

            // Allow-list check (matches Go's HandleMessage → IsAllowed)
            if !allow_list.is_empty() && !allow_list.iter().any(|a| a == &chat_id) {
                warn!(chat_id = %chat_id, "[WebSocketChannel] message blocked by allow-list");
                return None;
            }

            let inbound = InboundMessage {
                channel: base_name.to_string(),
                sender_id: chat_id.clone(),
                chat_id: chat_id.clone(),
                content: client_msg.content.clone(),
                media: Vec::new(),
                session_key: chat_id.clone(),
                correlation_id: String::new(),
                metadata: std::collections::HashMap::new(),
                voice_playback: None,
            };

            if let Err(e) = bus_sender.send(inbound) {
                warn!(error = %e, "[WebSocketChannel] failed to publish inbound message");
            }

            Some(client_msg.content)
        }
        "ping" => {
            let pong = ServerMessage::pong();
            if let Ok(json) = serde_json::to_string(&pong) {
                let _ = send_tx.send(json);
            }
            None
        }
        other => {
            send_error(send_tx, format!("Unknown message type: {other}"));
            None
        }
    }
}

/// Sends an error message to the client.
fn send_error(send_tx: &tokio::sync::mpsc::UnboundedSender<String>, msg: impl ToString) {
    let err = ServerMessage::error_msg(msg);
    if let Ok(json) = serde_json::to_string(&err) {
        let _ = send_tx.send(json);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

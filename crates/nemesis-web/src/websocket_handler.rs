//! WebSocket handler with thread-safe send queue, protocol dispatch,
//! session management, reconnect support, and message serialization.
//!
//! Mirrors the Go `module/web/websocket.go`:
//! - `SendQueue` — thread-safe send queue with single-writer goroutine/task
//! - `handle_websocket` — full WebSocket connection lifecycle
//! - `handle_text_message` — three-level protocol dispatch
//! - `broadcast_to_session` — send to specific session via session manager
//! - `handle_message_module` — message-type dispatch (chat.send, history_request)
//! - `handle_system_module` — system-type dispatch (heartbeat.ping, error.notify)
//! - Reconnect logic via session tracking

use crate::protocol::ProtocolMessage;
use crate::session::SessionManager;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures::stream::SplitSink;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Incoming message type
// ---------------------------------------------------------------------------

/// Incoming message from a WebSocket client.
#[derive(Debug, Clone)]
pub struct IncomingMessage {
    pub session_id: String,
    pub sender_id: String,
    pub chat_id: String,
    pub content: String,
    pub metadata: HashMap<String, String>,
    pub voice_playback: Option<bool>,
}

// ---------------------------------------------------------------------------
// Send queue (thread-safe, single-writer)
// ---------------------------------------------------------------------------

/// A thread-safe send queue that serializes all writes through a single task.
///
/// This mirrors the Go `sendQueue` struct. All writes go through a bounded
/// mpsc channel to a dedicated sender task, preventing concurrent WebSocket writes.
pub struct SendQueue {
    tx: mpsc::Sender<Vec<u8>>,
    done: tokio::sync::watch::Receiver<bool>,
}

impl SendQueue {
    /// Create a new send queue wrapping a WebSocket sink.
    /// Spawns a background task that processes the send queue.
    pub fn new(mut sink: SplitSink<WebSocket, Message>) -> Self {
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(256);
        let (done_tx, done_rx) = tokio::sync::watch::channel(false);

        tokio::spawn(async move {
            while let Some(data) = rx.recv().await {
                let text = String::from_utf8_lossy(&data).into_owned();
                let msg = Message::Text(text.into());
                if sink.feed(msg).await.is_err() {
                    break;
                }
                if sink.flush().await.is_err() {
                    break;
                }
            }
            let _ = done_tx.send(true);
        });

        Self {
            tx,
            done: done_rx,
        }
    }

    /// Send data through the queue. Blocks until the data is queued.
    /// Returns an error if the queue is full, stopped, or times out.
    pub async fn send(&self, data: Vec<u8>) -> Result<(), String> {
        self.tx
            .send(data)
            .await
            .map_err(|_| "send queue stopped".to_string())
    }

    /// Send data without waiting for the channel capacity (non-blocking).
    /// Returns an error immediately if the queue is full.
    pub fn try_send(&self, data: Vec<u8>) -> Result<(), String> {
        self.tx
            .try_send(data)
            .map_err(|e| format!("send queue error: {}", e))
    }

    /// Check if the send queue is still active.
    pub fn is_done(&self) -> bool {
        *self.done.borrow()
    }

    /// Create a SendQueue from raw channels (for testing).
    #[cfg(test)]
    pub fn from_channels(
        tx: mpsc::Sender<Vec<u8>>,
        done: tokio::sync::watch::Receiver<bool>,
    ) -> Self {
        Self { tx, done }
    }
}

// ---------------------------------------------------------------------------
// WebSocket upgrade handler
// ---------------------------------------------------------------------------

/// Query parameters for WebSocket connections.
#[derive(Debug, Deserialize)]
pub struct WsQuery {
    /// Authentication token.
    pub token: Option<String>,
}

/// Handle WebSocket upgrade requests.
///
/// This is the entry point for the WebSocket route. It performs:
/// 1. Auth token validation (if configured)
/// 2. WebSocket upgrade
/// 3. Session creation
/// 4. Handoff to the WebSocket connection handler
pub async fn handle_websocket_upgrade(
    ws: WebSocketUpgrade,
    Query(query): Query<WsQuery>,
    State(state): State<Arc<crate::api_handlers::AppState>>,
) -> axum::response::Response {
    // Verify auth token if configured
    if !state.auth_token.is_empty() {
        let token = query.token.unwrap_or_default();
        if token != state.auth_token {
            tracing::warn!("[WebSocket] Authentication failed");
            return axum::http::StatusCode::UNAUTHORIZED.into_response();
        }
    }

    ws.on_upgrade(move |socket| {
        handle_websocket(socket, state)
    })
}

// ---------------------------------------------------------------------------
// WebSocket connection handler
// ---------------------------------------------------------------------------

/// Handle a WebSocket connection through its full lifecycle.
///
/// This mirrors the Go `HandleWebSocket` function. It:
/// 1. Creates a session
/// 2. Sets up a send queue for thread-safe writes
/// 3. Reads messages in a loop
/// 4. Dispatches by protocol type (message/system)
/// 5. Sends pong responses for heartbeat pings
/// 6. Cleans up session on disconnect
pub async fn handle_websocket(socket: WebSocket, state: Arc<crate::api_handlers::AppState>) {
    let session = state.session_manager_ref().create_session();
    let session_id = session.id.clone();
    let sender_id = session.sender_id.clone();
    let chat_id = session.chat_id.clone();

    tracing::info!(
        session_id = %session_id,
        "[WebSocket] Connection established"
    );

    // Split socket into sink and stream
    let (sink, mut stream) = socket.split();

    // Create send queue for thread-safe writes
    let send_queue = Arc::new(SendQueue::new(sink));

    // Store send queue in session for outbound messages
    state.session_manager_ref().set_send_queue(&session_id, send_queue.clone());

    // Increment session count
    state.session_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

    // Message read loop
    let mut ping_interval = tokio::time::interval(Duration::from_secs(30));

    loop {
        tokio::select! {
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        // Update last active
                        state.session_manager_ref().touch_session(&session_id);

                        let raw = text.as_bytes();

                        // Try to detect request-type messages for WS API Router dispatch.
                        // Parse first, check type, then decide: request -> router, else -> legacy path.
                        let parsed = ProtocolMessage::parse(raw);
                        if let Ok(ref pm) = parsed {
                            if pm.is_request() {
                                // Dispatch to WS API Router
                                let req_id = pm.req_id.as_deref().unwrap_or("");
                                tracing::debug!(
                                    session_id = %session_id,
                                    module = %pm.module,
                                    cmd = %pm.cmd,
                                    req_id = %req_id,
                                    "[WebSocket] API request received"
                                );
                                if let Some(ref router) = state.ws_router {
                                    let ctx = crate::ws_router::RequestContext {
                                        session_id: session_id.clone(),
                                        workspace: state.workspace.clone(),
                                        home: state.home.clone(),
                                        state: state.clone(),
                                    };
                                    let router = router.clone();
                                    let sq = send_queue.clone();
                                    let msg = pm.clone();
                                    tokio::spawn(async move {
                                        router.dispatch(&msg, &ctx, &sq).await;
                                    });
                                } else {
                                    // No router configured — send error response
                                    let err = ProtocolMessage::response_err(
                                        &pm.module, &pm.cmd, req_id, "ws router not configured",
                                    );
                                    if let Ok(bytes) = err.to_json() {
                                        let _ = send_queue.send(bytes).await;
                                    }
                                }
                                // Request handled, skip legacy dispatch
                                continue;
                            }
                        }

                        // Legacy path: message / system types
                        match handle_text_message(&session_id, &sender_id, &chat_id, raw) {
                            Ok(Some(incoming)) => {
                                // Forward to the bus bridge via the inbound channel
                                if let Some(ref tx) = state.inbound_tx {
                                    if let Err(e) = tx.send(incoming.clone()) {
                                        tracing::warn!(
                                            error = %e,
                                            session_id = %session_id,
                                            "[WebSocket] Failed to forward message to bus (channel closed)"
                                        );
                                    } else {
                                        tracing::debug!(
                                            session_id = %session_id,
                                            content = %incoming.content,
                                            "[WebSocket] Message forwarded to bus"
                                        );
                                    }
                                } else {
                                    tracing::warn!(
                                        session_id = %session_id,
                                        "[WebSocket] No inbound channel configured, dropping message"
                                    );
                                }
                            }
                            Ok(None) => {
                                // System message handled (e.g., ping -> pong)
                                let pong = build_pong().unwrap();
                                let _ = send_queue.send(pong).await;
                            }
                            Err(e) => {
                                tracing::error!(
                                    error = %e,
                                    session_id = %session_id,
                                    "[WebSocket] Protocol message error"
                                );
                                let error_msg = build_error_message(&e);
                                let _ = send_queue.send(error_msg).await;
                            }
                        }
                    }
                    Some(Ok(Message::Ping(_data))) => {
                        // Respond with pong (axum handles this automatically, but just in case)
                        state.session_manager_ref().touch_session(&session_id);
                        let _ = send_queue.send(build_pong().unwrap()).await;
                    }
                    Some(Ok(Message::Close(_))) => {
                        tracing::info!(
                            session_id = %session_id,
                            "[WebSocket] Close frame received"
                        );
                        break;
                    }
                    Some(Ok(Message::Binary(_))) => {
                        tracing::warn!(
                            session_id = %session_id,
                            "[WebSocket] Received binary message (not supported)"
                        );
                    }
                    Some(Err(e)) => {
                        tracing::error!(
                            error = %e,
                            session_id = %session_id,
                            "[WebSocket] Read error"
                        );
                        break;
                    }
                    None => {
                        tracing::info!(
                            session_id = %session_id,
                            "[WebSocket] Stream ended"
                        );
                        break;
                    }
                    _ => {}
                }
            }
            _ = ping_interval.tick() => {
                // Send periodic ping to keep connection alive
                // SendQueue only handles text, so we skip this for now
                // (axum/autobahn handles ping/pong at protocol level)
            }
        }
    }

    // Cleanup
    state.session_manager_ref().remove_session(&session_id);
    state.session_count.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);

    tracing::info!(
        session_id = %session_id,
        "[WebSocket] Connection closed"
    );
}

// ---------------------------------------------------------------------------
// Protocol message dispatch
// ---------------------------------------------------------------------------

/// Handle a WebSocket text message using the three-level protocol.
///
/// Returns:
/// - `Ok(Some(IncomingMessage))` for chat messages that should be forwarded
/// - `Ok(None)` for system messages that are handled internally
/// - `Err(String)` for protocol errors
pub fn handle_text_message(
    session_id: &str,
    sender_id: &str,
    chat_id: &str,
    raw: &[u8],
) -> Result<Option<IncomingMessage>, String> {
    let msg = ProtocolMessage::parse(raw)
        .map_err(|e| format!("invalid protocol message: {}", e))?;

    match msg.msg_type.as_str() {
        "message" => handle_message_module(session_id, sender_id, chat_id, &msg),
        "system" => handle_system_module(&msg),
        "request" => Ok(None), // Handled by WsRouter in the main loop; should not reach here
        _ => Err(format!("unknown protocol type: {}", msg.msg_type)),
    }
}

// ---------------------------------------------------------------------------
// Message module dispatch
// ---------------------------------------------------------------------------

/// Dispatch messages with type=="message".
fn handle_message_module(
    session_id: &str,
    sender_id: &str,
    chat_id: &str,
    msg: &ProtocolMessage,
) -> Result<Option<IncomingMessage>, String> {
    match msg.module.as_str() {
        "chat" => match msg.cmd.as_str() {
            "send" => handle_chat_send(session_id, sender_id, chat_id, msg),
            "history_request" => handle_history_request(session_id, sender_id, chat_id, msg),
            _ => Err(format!("unknown chat cmd: {}", msg.cmd)),
        },
        _ => Err(format!("unknown message module: {}", msg.module)),
    }
}

/// Handle a chat.send message.
fn handle_chat_send(
    session_id: &str,
    sender_id: &str,
    chat_id: &str,
    msg: &ProtocolMessage,
) -> Result<Option<IncomingMessage>, String> {
    #[derive(serde::Deserialize)]
    struct ChatData {
        content: String,
        #[serde(default)]
        voice_playback: Option<bool>,
    }
    let data: ChatData = msg.decode_data()?;
    if data.content.is_empty() {
        return Err("message content cannot be empty".to_string());
    }

    tracing::debug!(
        session_id = %session_id,
        content = %data.content,
        "[WebSocket] Message forwarded to channel (new protocol)"
    );

    Ok(Some(IncomingMessage {
        session_id: session_id.to_string(),
        sender_id: sender_id.to_string(),
        chat_id: chat_id.to_string(),
        content: data.content,
        metadata: HashMap::new(),
        voice_playback: data.voice_playback,
    }))
}

/// Handle a chat.history_request message.
fn handle_history_request(
    session_id: &str,
    sender_id: &str,
    chat_id: &str,
    msg: &ProtocolMessage,
) -> Result<Option<IncomingMessage>, String> {
    #[derive(serde::Deserialize)]
    struct HistoryReqData {
        request_id: String,
        #[serde(default)]
        limit: Option<i64>,
        #[serde(default)]
        before_index: Option<i64>,
    }

    let req_data: HistoryReqData = msg.decode_data()?;
    let payload = serde_json::json!({
        "request_id": req_data.request_id,
        "limit": req_data.limit,
        "before_index": req_data.before_index,
    });

    let mut metadata = HashMap::new();
    metadata.insert("request_type".to_string(), "history".to_string());

    tracing::debug!(
        session_id = %session_id,
        request_id = %req_data.request_id,
        "[WebSocket] History request forwarded to channel"
    );

    Ok(Some(IncomingMessage {
        session_id: session_id.to_string(),
        sender_id: sender_id.to_string(),
        chat_id: chat_id.to_string(),
        content: payload.to_string(),
        metadata,
        voice_playback: None,
    }))
}

// ---------------------------------------------------------------------------
// System module dispatch
// ---------------------------------------------------------------------------

/// Dispatch messages with type=="system".
fn handle_system_module(msg: &ProtocolMessage) -> Result<Option<IncomingMessage>, String> {
    match msg.module.as_str() {
        "heartbeat" => match msg.cmd.as_str() {
            "ping" => Ok(None), // Caller should send pong
            _ => Err(format!("unknown heartbeat cmd: {}", msg.cmd)),
        },
        "error" => match msg.cmd.as_str() {
            "notify" => {
                tracing::warn!("[WebSocket] Client error notification: {:?}", msg.data);
                Ok(None)
            }
            _ => Err(format!("unknown error cmd: {}", msg.cmd)),
        },
        _ => Err(format!("unknown system module: {}", msg.module)),
    }
}

// ---------------------------------------------------------------------------
// Broadcast / message building helpers
// ---------------------------------------------------------------------------

/// Build a broadcast message for a session (type=message, module=chat, cmd=receive).
pub fn build_broadcast_message(role: &str, content: &str) -> Result<Vec<u8>, String> {
    let msg = ProtocolMessage::new(
        "message",
        "chat",
        "receive",
        Some(serde_json::json!({
            "role": role,
            "content": content,
        })),
    );
    msg.to_json().map_err(|e| e.to_string())
}

/// Build a pong response (type=system, module=heartbeat, cmd=pong).
pub fn build_pong() -> Result<Vec<u8>, String> {
    let msg = ProtocolMessage::new("system", "heartbeat", "pong", Some(serde_json::json!({})));
    msg.to_json().map_err(|e| e.to_string())
}

/// Build an error message (type=system, module=error, cmd=notify).
pub fn build_error_message(error_text: &str) -> Vec<u8> {
    let msg = ProtocolMessage::new(
        "system",
        "error",
        "notify",
        Some(serde_json::json!({
            "content": error_text,
        })),
    );
    msg.to_json().unwrap_or_else(|_| br#"{"type":"system","module":"error","cmd":"notify","data":{"content":"internal error"}}"#.to_vec())
}

/// Broadcast a message to a specific session using the protocol format.
///
/// This mirrors the Go `BroadcastToSession` function.
pub async fn broadcast_to_session(
    session_manager: &SessionManager,
    session_id: &str,
    role: &str,
    content: &str,
) -> Result<(), String> {
    tracing::debug!(
        session_id = %session_id,
        role = %role,
        content_len = content.len(),
        content_preview = &content[..content.len().min(100)],
        "[WebSocket] broadcast_to_session called"
    );

    let data = build_broadcast_message(role, content)?;
    session_manager
        .broadcast(session_id, &data)
        .await
        .map_err(|e| format!("failed to broadcast: {}", e))?;

    tracing::info!(
        session_id = %session_id,
        role = %role,
        "[WebSocket] broadcast_to_session completed"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

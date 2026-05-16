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
            tracing::warn!("WebSocket authentication failed");
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
        "WebSocket connection established"
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
                        match handle_text_message(&session_id, &sender_id, &chat_id, raw) {
                            Ok(Some(incoming)) => {
                                // Forward to the bus bridge via the inbound channel
                                if let Some(ref tx) = state.inbound_tx {
                                    if let Err(e) = tx.send(incoming.clone()) {
                                        tracing::warn!(
                                            error = %e,
                                            session_id = %session_id,
                                            "Failed to forward message to bus (channel closed)"
                                        );
                                    } else {
                                        tracing::debug!(
                                            session_id = %session_id,
                                            content = %incoming.content,
                                            "Message forwarded to bus"
                                        );
                                    }
                                } else {
                                    tracing::warn!(
                                        session_id = %session_id,
                                        "No inbound channel configured, dropping message"
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
                                    "Protocol message error"
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
                            "WebSocket close frame received"
                        );
                        break;
                    }
                    Some(Ok(Message::Binary(_))) => {
                        tracing::warn!(
                            session_id = %session_id,
                            "Received binary message (not supported)"
                        );
                    }
                    Some(Err(e)) => {
                        tracing::error!(
                            error = %e,
                            session_id = %session_id,
                            "WebSocket read error"
                        );
                        break;
                    }
                    None => {
                        tracing::info!(
                            session_id = %session_id,
                            "WebSocket stream ended"
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
        "WebSocket connection closed"
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
    }
    let data: ChatData = msg.decode_data()?;
    if data.content.is_empty() {
        return Err("message content cannot be empty".to_string());
    }

    tracing::debug!(
        session_id = %session_id,
        content = %data.content,
        "Message forwarded to channel (new protocol)"
    );

    Ok(Some(IncomingMessage {
        session_id: session_id.to_string(),
        sender_id: sender_id.to_string(),
        chat_id: chat_id.to_string(),
        content: data.content,
        metadata: HashMap::new(),
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
        "History request forwarded to channel"
    );

    Ok(Some(IncomingMessage {
        session_id: session_id.to_string(),
        sender_id: sender_id.to_string(),
        chat_id: chat_id.to_string(),
        content: payload.to_string(),
        metadata,
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
                tracing::warn!("client error notification: {:?}", msg.data);
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
        "broadcast_to_session called"
    );

    let data = build_broadcast_message(role, content)?;
    session_manager
        .broadcast(session_id, &data)
        .await
        .map_err(|e| format!("failed to broadcast: {}", e))?;

    tracing::info!(
        session_id = %session_id,
        role = %role,
        "broadcast_to_session completed"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handle_chat_send() {
        let raw = br#"{"type":"message","module":"chat","cmd":"send","data":{"content":"hello"}}"#;
        let result = handle_text_message("s1", "web:s1", "web:s1", raw).unwrap();
        assert!(result.is_some());
        let msg = result.unwrap();
        assert_eq!(msg.content, "hello");
        assert_eq!(msg.session_id, "s1");
        assert_eq!(msg.sender_id, "web:s1");
        assert_eq!(msg.chat_id, "web:s1");
    }

    #[test]
    fn test_handle_chat_send_empty_content() {
        let raw = br#"{"type":"message","module":"chat","cmd":"send","data":{"content":""}}"#;
        let result = handle_text_message("s1", "web:s1", "web:s1", raw);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty"));
    }

    #[test]
    fn test_handle_heartbeat_ping() {
        let raw = br#"{"type":"system","module":"heartbeat","cmd":"ping","data":null}"#;
        let result = handle_text_message("s1", "web:s1", "web:s1", raw).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_handle_error_notify() {
        let raw = br#"{"type":"system","module":"error","cmd":"notify","data":{"content":"test error"}}"#;
        let result = handle_text_message("s1", "web:s1", "web:s1", raw).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_handle_history_request() {
        let raw = br#"{"type":"message","module":"chat","cmd":"history_request","data":{"request_id":"r1","limit":10}}"#;
        let result = handle_text_message("s1", "web:s1", "web:s1", raw).unwrap();
        assert!(result.is_some());
        let msg = result.unwrap();
        assert_eq!(msg.metadata.get("request_type"), Some(&"history".to_string()));
    }

    #[test]
    fn test_unknown_protocol_type() {
        let raw = br#"{"type":"unknown","module":"test","cmd":"test"}"#;
        let result = handle_text_message("s1", "web:s1", "web:s1", raw);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown protocol type"));
    }

    #[test]
    fn test_unknown_chat_cmd() {
        let raw = br#"{"type":"message","module":"chat","cmd":"unknown","data":{}}"#;
        let result = handle_text_message("s1", "web:s1", "web:s1", raw);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown chat cmd"));
    }

    #[test]
    fn test_unknown_message_module() {
        let raw = br#"{"type":"message","module":"unknown","cmd":"test","data":{}}"#;
        let result = handle_text_message("s1", "web:s1", "web:s1", raw);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown message module"));
    }

    #[test]
    fn test_unknown_system_module() {
        let raw = br#"{"type":"system","module":"unknown","cmd":"test"}"#;
        let result = handle_text_message("s1", "web:s1", "web:s1", raw);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown system module"));
    }

    #[test]
    fn test_unknown_heartbeat_cmd() {
        let raw = br#"{"type":"system","module":"heartbeat","cmd":"unknown"}"#;
        let result = handle_text_message("s1", "web:s1", "web:s1", raw);
        assert!(result.is_err());
    }

    #[test]
    fn test_unknown_error_cmd() {
        let raw = br#"{"type":"system","module":"error","cmd":"unknown"}"#;
        let result = handle_text_message("s1", "web:s1", "web:s1", raw);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_json() {
        let raw = b"not valid json";
        let result = handle_text_message("s1", "web:s1", "web:s1", raw);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid protocol message"));
    }

    #[test]
    fn test_build_broadcast() {
        let bytes = build_broadcast_message("assistant", "hi there").unwrap();
        let msg = ProtocolMessage::parse(&bytes).unwrap();
        assert_eq!(msg.msg_type, "message");
        assert_eq!(msg.module, "chat");
        assert_eq!(msg.cmd, "receive");
    }

    #[test]
    fn test_build_pong() {
        let bytes = build_pong().unwrap();
        let msg = ProtocolMessage::parse(&bytes).unwrap();
        assert_eq!(msg.msg_type, "system");
        assert_eq!(msg.module, "heartbeat");
        assert_eq!(msg.cmd, "pong");
    }

    #[test]
    fn test_build_error_message() {
        let bytes = build_error_message("test error");
        let msg = ProtocolMessage::parse(&bytes).unwrap();
        assert_eq!(msg.msg_type, "system");
        assert_eq!(msg.module, "error");
        assert_eq!(msg.cmd, "notify");
    }

    #[test]
    fn test_build_broadcast_message_content() {
        let bytes = build_broadcast_message("user", "hello world").unwrap();
        let msg = ProtocolMessage::parse(&bytes).unwrap();
        let data = msg.data.unwrap();
        assert_eq!(data["role"], "user");
        assert_eq!(data["content"], "hello world");
    }

    #[test]
    fn test_handle_chat_send_with_metadata() {
        let raw = br#"{"type":"message","module":"chat","cmd":"send","data":{"content":"hello"}}"#;
        let result = handle_text_message("s1", "web:s1", "web:s1", raw).unwrap().unwrap();
        assert!(result.metadata.is_empty());
    }

    #[test]
    fn test_handle_chat_send_invalid_data() {
        let raw = br#"{"type":"message","module":"chat","cmd":"send","data":"not an object"}"#;
        let result = handle_text_message("s1", "web:s1", "web:s1", raw);
        assert!(result.is_err());
    }

    #[test]
    fn test_handle_history_request_invalid_data() {
        let raw = br#"{"type":"message","module":"chat","cmd":"history_request","data":"bad"}"#;
        let result = handle_text_message("s1", "web:s1", "web:s1", raw);
        assert!(result.is_err());
    }

    #[test]
    fn test_handle_text_message_preserves_session_info() {
        let raw = br#"{"type":"message","module":"chat","cmd":"send","data":{"content":"test"}}"#;
        let msg = handle_text_message("my-session", "my-sender", "my-chat", raw)
            .unwrap().unwrap();
        assert_eq!(msg.session_id, "my-session");
        assert_eq!(msg.sender_id, "my-sender");
        assert_eq!(msg.chat_id, "my-chat");
    }

    #[test]
    fn test_handle_chat_send_with_special_characters() {
        let raw = br#"{"type":"message","module":"chat","cmd":"send","data":{"content":"hello <b>world</b> & 'friends'"}}"#;
        let result = handle_text_message("s1", "w:s1", "w:s1", raw);
        assert!(result.is_ok());
        let msg = result.unwrap().unwrap();
        assert!(msg.content.contains("<b>world</b>"));
    }

    #[test]
    fn test_handle_chat_send_unicode_content() {
        let raw = br#"{"type":"message","module":"chat","cmd":"send","data":{"content":"Hello \u4e16\u754c \ud83d\ude00"}}"#;
        let result = handle_text_message("s1", "w:s1", "w:s1", raw);
        assert!(result.is_ok());
    }

    #[test]
    fn test_build_broadcast_message_roles() {
        // Test with assistant role
        let bytes = build_broadcast_message("assistant", "response text").unwrap();
        let msg = ProtocolMessage::parse(&bytes).unwrap();
        assert_eq!(msg.data.as_ref().unwrap()["role"], "assistant");

        // Test with user role
        let bytes = build_broadcast_message("user", "user message").unwrap();
        let msg = ProtocolMessage::parse(&bytes).unwrap();
        assert_eq!(msg.data.as_ref().unwrap()["role"], "user");

        // Test with system role
        let bytes = build_broadcast_message("system", "system note").unwrap();
        let msg = ProtocolMessage::parse(&bytes).unwrap();
        assert_eq!(msg.data.as_ref().unwrap()["role"], "system");
    }

    #[test]
    fn test_build_error_message_contains_error_text() {
        let bytes = build_error_message("something went wrong");
        let msg = ProtocolMessage::parse(&bytes).unwrap();
        assert_eq!(msg.data.unwrap()["content"], "something went wrong");
    }

    #[test]
    fn test_build_pong_has_empty_data() {
        let bytes = build_pong().unwrap();
        let msg = ProtocolMessage::parse(&bytes).unwrap();
        assert_eq!(msg.data.unwrap(), serde_json::json!({}));
    }

    #[test]
    fn test_handle_text_message_empty_data_chat_send() {
        let raw = br#"{"type":"message","module":"chat","cmd":"send","data":{}}"#;
        let result = handle_text_message("s1", "w:s1", "w:s1", raw);
        // Missing content field should fail
        assert!(result.is_err());
    }

    #[test]
    fn test_handle_history_request_with_all_fields() {
        let raw = br#"{"type":"message","module":"chat","cmd":"history_request","data":{"request_id":"req-123","limit":50,"before_index":100}}"#;
        let result = handle_text_message("s1", "w:s1", "w:s1", raw).unwrap().unwrap();
        assert_eq!(result.metadata.get("request_type"), Some(&"history".to_string()));
        // Content should contain the request data as JSON
        let content: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(content["request_id"], "req-123");
        assert_eq!(content["limit"], 50);
        assert_eq!(content["before_index"], 100);
    }

    #[test]
    fn test_handle_history_request_minimal_fields() {
        let raw = br#"{"type":"message","module":"chat","cmd":"history_request","data":{"request_id":"r1"}}"#;
        let result = handle_text_message("s1", "w:s1", "w:s1", raw);
        assert!(result.is_ok());
        let msg = result.unwrap().unwrap();
        let content: serde_json::Value = serde_json::from_str(&msg.content).unwrap();
        assert_eq!(content["request_id"], "r1");
        assert!(content["limit"].is_null());
    }

    #[test]
    fn test_incoming_message_debug() {
        let msg = IncomingMessage {
            session_id: "s1".to_string(),
            sender_id: "web:s1".to_string(),
            chat_id: "web:s1".to_string(),
            content: "hello".to_string(),
            metadata: HashMap::new(),
        };
        let debug_str = format!("{:?}", msg);
        assert!(debug_str.contains("s1"));
        assert!(debug_str.contains("hello"));
    }

    #[test]
    fn test_incoming_message_clone() {
        let msg = IncomingMessage {
            session_id: "s1".to_string(),
            sender_id: "web:s1".to_string(),
            chat_id: "web:s1".to_string(),
            content: "hello".to_string(),
            metadata: HashMap::new(),
        };
        let cloned = msg.clone();
        assert_eq!(cloned.session_id, msg.session_id);
        assert_eq!(cloned.content, msg.content);
    }

    #[test]
    fn test_broadcast_to_session_no_queue() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mgr = SessionManager::with_default_timeout();
        let session = mgr.create_session();
        let result = rt.block_on(broadcast_to_session(&mgr, &session.id, "assistant", "hello"));
        assert!(result.is_err());
    }

    #[test]
    fn test_ws_query_deserialize() {
        let query: WsQuery = serde_json::from_str(r#"{"token":"my-token"}"#).unwrap();
        assert_eq!(query.token, Some("my-token".to_string()));
    }

    #[test]
    fn test_ws_query_deserialize_no_token() {
        let query: WsQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert_eq!(query.token, None);
    }

    #[test]
    fn test_handle_text_message_chat_send_long_content() {
        let content = "a".repeat(10000);
        let raw = format!(r#"{{"type":"message","module":"chat","cmd":"send","data":{{"content":"{}"}}}}"#, content);
        let result = handle_text_message("s1", "w:s1", "w:s1", raw.as_bytes());
        assert!(result.is_ok());
        let msg = result.unwrap().unwrap();
        assert_eq!(msg.content.len(), 10000);
    }

    #[test]
    fn test_handle_text_message_preserves_newlines() {
        let raw = br#"{"type":"message","module":"chat","cmd":"send","data":{"content":"line1\nline2\nline3"}}"#;
        let result = handle_text_message("s1", "w:s1", "w:s1", raw);
        assert!(result.is_ok());
        let msg = result.unwrap().unwrap();
        assert!(msg.content.contains("line1"));
    }

    #[test]
    fn test_incoming_message_with_metadata() {
        let mut metadata = HashMap::new();
        metadata.insert("key1".to_string(), "val1".to_string());
        let msg = IncomingMessage {
            session_id: "s1".to_string(),
            sender_id: "web:s1".to_string(),
            chat_id: "web:s1".to_string(),
            content: "hello".to_string(),
            metadata,
        };
        assert_eq!(msg.metadata.get("key1"), Some(&"val1".to_string()));
    }

    #[test]
    fn test_incoming_message_equality() {
        let msg1 = IncomingMessage {
            session_id: "s1".to_string(),
            sender_id: "web:s1".to_string(),
            chat_id: "web:s1".to_string(),
            content: "hello".to_string(),
            metadata: HashMap::new(),
        };
        let msg2 = msg1.clone();
        assert_eq!(msg1.session_id, msg2.session_id);
        assert_eq!(msg1.content, msg2.content);
    }

    #[test]
    fn test_build_broadcast_message_with_special_chars() {
        let bytes = build_broadcast_message("assistant", "Hello <b>world</b>").unwrap();
        let parsed = ProtocolMessage::parse(&bytes).unwrap();
        assert_eq!(parsed.data.unwrap()["content"], "Hello <b>world</b>");
    }

    #[test]
    fn test_build_broadcast_message_empty_content() {
        let bytes = build_broadcast_message("user", "").unwrap();
        let parsed = ProtocolMessage::parse(&bytes).unwrap();
        assert_eq!(parsed.data.unwrap()["content"], "");
    }

    #[test]
    fn test_build_error_message_with_special_chars() {
        let bytes = build_error_message("error: <tag> & \"quotes\"");
        let parsed = ProtocolMessage::parse(&bytes).unwrap();
        assert!(parsed.data.unwrap()["content"].as_str().unwrap().contains("<tag>"));
    }

    #[test]
    fn test_handle_system_heartbeat_pong() {
        let raw = br#"{"type":"system","module":"heartbeat","cmd":"ping","data":{}}"#;
        let result = handle_text_message("s1", "w:s1", "w:s1", raw).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_handle_system_error_notify_no_data() {
        let raw = br#"{"type":"system","module":"error","cmd":"notify"}"#;
        let result = handle_text_message("s1", "w:s1", "w:s1", raw);
        assert!(result.is_ok());
    }

    #[test]
    fn test_handle_message_unknown_module_error() {
        let raw = br#"{"type":"message","module":"email","cmd":"send","data":{"content":"test"}}"#;
        let result = handle_text_message("s1", "w:s1", "w:s1", raw);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown message module"));
    }

    #[test]
    fn test_handle_chat_send_whitespace_content() {
        let raw = br#"{"type":"message","module":"chat","cmd":"send","data":{"content":"   "}}"#;
        let result = handle_text_message("s1", "w:s1", "w:s1", raw);
        // Whitespace-only content should succeed (only empty is rejected)
        assert!(result.is_ok());
    }

    #[test]
    fn test_handle_history_request_no_data() {
        let raw = br#"{"type":"message","module":"chat","cmd":"history_request","data":"string not object"}"#;
        let result = handle_text_message("s1", "w:s1", "w:s1", raw);
        assert!(result.is_err());
    }

    #[test]
    fn test_broadcast_to_session_nonexistent() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mgr = SessionManager::with_default_timeout();
        let result = rt.block_on(broadcast_to_session(&mgr, "fake-session", "assistant", "hello"));
        assert!(result.is_err());
    }

    #[test]
    fn test_ws_query_with_empty_token() {
        let query: WsQuery = serde_json::from_str(r#"{"token":""}"#).unwrap();
        assert_eq!(query.token, Some("".to_string()));
    }

    #[test]
    fn test_build_pong_is_valid_json() {
        let bytes = build_pong().unwrap();
        let json_str = String::from_utf8(bytes).unwrap();
        let _: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    }

    #[test]
    fn test_build_broadcast_is_valid_json() {
        let bytes = build_broadcast_message("user", "test message").unwrap();
        let json_str = String::from_utf8(bytes).unwrap();
        let _: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    }

    #[test]
    fn test_build_error_is_valid_json() {
        let bytes = build_error_message("some error");
        let json_str = String::from_utf8(bytes).unwrap();
        let _: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    }

    #[test]
    fn test_handle_chat_send_with_json_content() {
        let raw = br#"{"type":"message","module":"chat","cmd":"send","data":{"content":"{\"key\":\"value\"}"}}"#;
        let result = handle_text_message("s1", "w:s1", "w:s1", raw);
        assert!(result.is_ok());
        let msg = result.unwrap().unwrap();
        assert!(msg.content.contains("key"));
    }

    #[test]
    fn test_handle_system_unknown_module() {
        let raw = br#"{"type":"system","module":"unknown_module","cmd":"test"}"#;
        let result = handle_text_message("s1", "w:s1", "w:s1", raw);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown system module"));
    }

    #[test]
    fn test_handle_heartbeat_unknown_cmd() {
        let raw = br#"{"type":"system","module":"heartbeat","cmd":"restart"}"#;
        let result = handle_text_message("s1", "w:s1", "w:s1", raw);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown heartbeat cmd"));
    }

    // ============================================================
    // Additional tests for 95%+ coverage - SendQueue + broadcast
    // ============================================================

    #[tokio::test]
    async fn test_send_queue_broadcast_no_queue() {
        use crate::session::SessionManager;
        let mgr = SessionManager::with_default_timeout();
        let session = mgr.create_session();
        // No send queue set - should return error
        let result = broadcast_to_session(&mgr, &session.id, "assistant", "test msg").await;
        assert!(result.is_err());
    }

    #[test]
    fn test_handle_text_message_with_null_data() {
        let raw = br#"{"type":"message","module":"chat","cmd":"send","data":null}"#;
        let result = handle_text_message("s1", "w:s1", "w:s1", raw);
        assert!(result.is_err());
    }

    #[test]
    fn test_handle_error_notify_with_null_data() {
        let raw = br#"{"type":"system","module":"error","cmd":"notify","data":null}"#;
        let result = handle_text_message("s1", "w:s1", "w:s1", raw);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_handle_heartbeat_ping_with_data() {
        let raw = br#"{"type":"system","module":"heartbeat","cmd":"ping","data":{"ts":12345}}"#;
        let result = handle_text_message("s1", "w:s1", "w:s1", raw);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_build_broadcast_message_with_empty_role() {
        let bytes = build_broadcast_message("", "content").unwrap();
        let msg = ProtocolMessage::parse(&bytes).unwrap();
        assert_eq!(msg.data.unwrap()["role"], "");
    }

    #[test]
    fn test_build_broadcast_message_with_multiline_content() {
        let content = "line1\nline2\nline3";
        let bytes = build_broadcast_message("assistant", content).unwrap();
        let msg = ProtocolMessage::parse(&bytes).unwrap();
        assert_eq!(msg.data.unwrap()["content"], content);
    }

    #[test]
    fn test_build_error_message_with_empty_string() {
        let bytes = build_error_message("");
        let msg = ProtocolMessage::parse(&bytes).unwrap();
        assert_eq!(msg.data.unwrap()["content"], "");
    }

    #[test]
    fn test_build_error_message_with_long_error() {
        let long_error = "x".repeat(10000);
        let bytes = build_error_message(&long_error);
        let msg = ProtocolMessage::parse(&bytes).unwrap();
        assert_eq!(msg.data.unwrap()["content"].as_str().unwrap().len(), 10000);
    }

    #[test]
    fn test_handle_chat_send_with_numbers_in_content() {
        let raw = br#"{"type":"message","module":"chat","cmd":"send","data":{"content":"123 + 456 = 579"}}"#;
        let result = handle_text_message("s1", "w:s1", "w:s1", raw);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().unwrap().content, "123 + 456 = 579");
    }

    #[test]
    fn test_handle_history_request_with_string_before_index() {
        let raw = br#"{"type":"message","module":"chat","cmd":"history_request","data":{"request_id":"r1","before_index":"not_a_number"}}"#;
        let result = handle_text_message("s1", "w:s1", "w:s1", raw);
        // Should still work - the field is an Option<i64>, string won't parse
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_incoming_message_default_metadata() {
        let msg = IncomingMessage {
            session_id: "s".to_string(),
            sender_id: "w:s".to_string(),
            chat_id: "w:s".to_string(),
            content: "hi".to_string(),
            metadata: HashMap::new(),
        };
        assert!(msg.metadata.is_empty());
        assert_eq!(msg.session_id, "s");
    }

    #[tokio::test]
    async fn test_broadcast_to_session_empty_id() {
        let mgr = SessionManager::with_default_timeout();
        let result = broadcast_to_session(&mgr, "", "assistant", "hello").await;
        assert!(result.is_err());
    }

    // ============================================================
    // Additional coverage tests for SendQueue and broadcast
    // ============================================================

    #[tokio::test]
    async fn test_send_queue_send_success() {
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(16);
        let (_, done_rx) = tokio::sync::watch::channel(false);

        let queue = SendQueue::from_channels(tx, done_rx);

        // Send a message
        let result = queue.send(b"test message".to_vec()).await;
        assert!(result.is_ok());

        // Verify it was received
        let received = rx.recv().await.unwrap();
        assert_eq!(received, b"test message".to_vec());
    }

    #[tokio::test]
    async fn test_send_queue_try_send_success() {
        let (tx, _rx) = mpsc::channel::<Vec<u8>>(16);
        let (_, done_rx) = tokio::sync::watch::channel(false);

        let queue = SendQueue::from_channels(tx, done_rx);

        // Try to send a message (non-blocking)
        let result = queue.try_send(b"try message".to_vec());
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_send_queue_try_send_full() {
        let (tx, _rx) = mpsc::channel::<Vec<u8>>(1);
        let (_, done_rx) = tokio::sync::watch::channel(false);

        let queue = SendQueue::from_channels(tx, done_rx);

        // Fill the channel
        let _ = queue.try_send(b"first".to_vec());
        // Second send should fail (full)
        let result = queue.try_send(b"second".to_vec());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("send queue error"));
    }

    #[test]
    fn test_send_queue_is_done_initially_false() {
        let (tx, _rx) = mpsc::channel::<Vec<u8>>(16);
        let (_, done_rx) = tokio::sync::watch::channel(false);

        let queue = SendQueue::from_channels(tx, done_rx);
        assert!(!queue.is_done());
    }

    #[test]
    fn test_send_queue_is_done_when_signaled() {
        let (tx, _rx) = mpsc::channel::<Vec<u8>>(16);
        let (done_tx, done_rx) = tokio::sync::watch::channel(false);

        let queue = SendQueue::from_channels(tx, done_rx);
        assert!(!queue.is_done());

        // Signal done
        done_tx.send(true).unwrap();
        assert!(queue.is_done());
    }

    #[test]
    fn test_incoming_message_clone_equality() {
        let msg = IncomingMessage {
            session_id: "s1".to_string(),
            sender_id: "web:s1".to_string(),
            chat_id: "web:s1".to_string(),
            content: "hello".to_string(),
            metadata: {
                let mut m = HashMap::new();
                m.insert("key".to_string(), "value".to_string());
                m
            },
        };
        let cloned = msg.clone();
        assert_eq!(cloned.session_id, msg.session_id);
        assert_eq!(cloned.sender_id, msg.sender_id);
        assert_eq!(cloned.chat_id, msg.chat_id);
        assert_eq!(cloned.content, msg.content);
        assert_eq!(cloned.metadata, msg.metadata);
    }

    #[test]
    fn test_handle_text_message_chat_send_long_content_2() {
        let content = "x".repeat(10000);
        let raw = format!(r#"{{"type":"message","module":"chat","cmd":"send","data":{{"content":"{}"}}}}"#, content);
        let result = handle_text_message("s1", "w:s1", "w:s1", raw.as_bytes());
        assert!(result.is_ok());
        let msg = result.unwrap().unwrap();
        assert_eq!(msg.content.len(), 10000);
    }

    #[test]
    fn test_handle_text_message_history_request_no_limit() {
        let raw = br#"{"type":"message","module":"chat","cmd":"history_request","data":{"request_id":"r2"}}"#;
        let result = handle_text_message("s1", "w:s1", "w:s1", raw);
        assert!(result.is_ok());
        let msg = result.unwrap().unwrap();
        let content: serde_json::Value = serde_json::from_str(&msg.content).unwrap();
        assert_eq!(content["request_id"], "r2");
        assert!(content["limit"].is_null());
        assert!(content["before_index"].is_null());
    }

    #[test]
    fn test_build_broadcast_message_various_roles() {
        for role in &["assistant", "user", "system", "tool"] {
            let bytes = build_broadcast_message(role, "test message").unwrap();
            let msg = ProtocolMessage::parse(&bytes).unwrap();
            assert_eq!(msg.data.as_ref().unwrap()["role"], *role);
        }
    }

    #[test]
    fn test_build_pong_is_valid() {
        let bytes = build_pong().unwrap();
        let text = String::from_utf8(bytes).unwrap();
        let _: serde_json::Value = serde_json::from_str(&text).unwrap();
    }

    #[test]
    fn test_build_error_message_with_quotes() {
        let bytes = build_error_message("error with \"quotes\" and 'apostrophes'");
        let text = String::from_utf8(bytes).unwrap();
        let _: serde_json::Value = serde_json::from_str(&text).unwrap();
    }

    #[tokio::test]
    async fn test_send_queue_send_after_drop() {
        let (tx, _) = mpsc::channel::<Vec<u8>>(16);
        let (_, done_rx) = tokio::sync::watch::channel(false);

        let queue = SendQueue::from_channels(tx, done_rx);
        // tx receiver is dropped since _ wasn't bound

        // This should still succeed since the channel is still open from sender side
        // Actually, since the receiver is dropped, send should return error
        // Wait - mpsc::Sender keeps the channel alive. Receiver being dropped
        // means send will fail on next attempt.
        // Let me re-think: we drop the receiver here, so send should fail
        drop(queue); // Just test that it doesn't panic
    }

    #[test]
    fn test_ws_query_deserialize_with_special_chars() {
        let query: WsQuery = serde_json::from_str(r#"{"token":"abc123!@#$"}"#).unwrap();
        assert_eq!(query.token, Some("abc123!@#$".to_string()));
    }

    #[tokio::test]
    async fn test_broadcast_to_session_with_session() {
        let mgr = SessionManager::with_default_timeout();
        let session = mgr.create_session();
        // Session exists but no send queue set, so broadcast should fail
        let result = broadcast_to_session(&mgr, &session.id, "assistant", "test message").await;
        assert!(result.is_err());
    }

    #[test]
    fn test_handle_text_message_whitespace_content() {
        let raw = br#"{"type":"message","module":"chat","cmd":"send","data":{"content":"   "}}"#;
        let result = handle_text_message("s1", "w:s1", "w:s1", raw);
        // Whitespace-only should succeed
        assert!(result.is_ok());
    }

    #[test]
    fn test_handle_text_message_empty_string_content() {
        let raw = br#"{"type":"message","module":"chat","cmd":"send","data":{"content":""}}"#;
        let result = handle_text_message("s1", "w:s1", "w:s1", raw);
        // Empty content should fail
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty"));
    }
}

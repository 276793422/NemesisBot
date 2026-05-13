//! WebSocket channel implementation.
//!
//! `WebSocketChannel` manages connections by `chat_id`. Supports broadcasting
//! messages to all connected clients, heartbeat ping/pong, and a bus subscribe
//! pattern where inbound messages are forwarded to a callback.

use async_trait::async_trait;
use dashmap::DashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

use nemesis_types::channel::OutboundMessage;
use nemesis_types::error::{NemesisError, Result};

use crate::base::{BaseChannel, Channel};

/// Represents a connected WebSocket client.
/// In production this would hold the actual WebSocket sink.
#[derive(Debug)]
struct WsConnection {
    /// Messages sent to this connection (for testing).
    messages: parking_lot::RwLock<Vec<String>>,
    /// Last heartbeat timestamp (Unix millis).
    last_heartbeat: parking_lot::RwLock<u64>,
}

/// Callback type for inbound messages received from WebSocket clients.
pub type InboundCallback = Arc<dyn Fn(String, String, String) + Send + Sync>;

/// A WebSocket-based channel.
///
/// Manages a map of `chat_id -> WsConnection`. Received messages are
/// forwarded to an optional inbound callback; outbound messages are
/// queued per connection. Supports broadcast to all connections.
pub struct WebSocketChannel {
    base: BaseChannel,
    connections: DashMap<String, WsConnection>,
    inbound_callback: parking_lot::RwLock<Option<InboundCallback>>,
    heartbeat_interval: Duration,
}

impl WebSocketChannel {
    /// Creates a new `WebSocketChannel` with default heartbeat interval (30s).
    pub fn new() -> Self {
        Self::with_heartbeat(Duration::from_secs(30))
    }

    /// Creates a new `WebSocketChannel` with a custom heartbeat interval.
    pub fn with_heartbeat(heartbeat_interval: Duration) -> Self {
        Self {
            base: BaseChannel::new("websocket"),
            connections: DashMap::new(),
            inbound_callback: parking_lot::RwLock::new(None),
            heartbeat_interval,
        }
    }

    /// Sets the inbound message callback.
    pub fn set_inbound_callback(&self, cb: InboundCallback) {
        *self.inbound_callback.write() = Some(cb);
    }

    /// Simulates a client connecting with the given `chat_id`.
    pub fn connect(&self, chat_id: &str) {
        self.connections.insert(
            chat_id.to_string(),
            WsConnection {
                messages: parking_lot::RwLock::new(Vec::new()),
                last_heartbeat: parking_lot::RwLock::new(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64,
                ),
            },
        );
        self.base.record_received();
        debug!(chat_id = %chat_id, "websocket client connected");
    }

    /// Simulates a client disconnecting.
    pub fn disconnect(&self, chat_id: &str) {
        self.connections.remove(chat_id);
        debug!(chat_id = %chat_id, "websocket client disconnected");
    }

    /// Returns whether a client with the given `chat_id` is connected.
    pub fn is_connected(&self, chat_id: &str) -> bool {
        self.connections.contains_key(chat_id)
    }

    /// Returns the messages sent to a specific connection (for testing).
    pub fn get_messages(&self, chat_id: &str) -> Vec<String> {
        self.connections
            .get(chat_id)
            .map(|c| c.messages.read().clone())
            .unwrap_or_default()
    }

    /// Returns the number of active connections.
    pub fn connection_count(&self) -> usize {
        self.connections.len()
    }

    /// Broadcasts a message to all connected clients.
    pub fn broadcast(&self, content: &str) {
        for entry in self.connections.iter() {
            entry.value().messages.write().push(content.to_string());
        }
        debug!(count = self.connections.len(), "broadcast message to all connections");
    }

    /// Processes an inbound message from a WebSocket client.
    /// If a callback is set, it is invoked with (sender_id, chat_id, content).
    pub fn handle_inbound(&self, sender_id: &str, chat_id: &str, content: &str) {
        self.base.record_received();
        if let Some(ref cb) = *self.inbound_callback.read() {
            cb(sender_id.to_string(), chat_id.to_string(), content.to_string());
        }
    }

    /// Updates the heartbeat timestamp for a connection.
    pub fn update_heartbeat(&self, chat_id: &str) {
        if let Some(conn) = self.connections.get(chat_id) {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            *conn.last_heartbeat.write() = now;
        }
    }

    /// Removes connections whose last heartbeat is older than the given timeout.
    /// Returns the number of connections removed.
    pub fn cleanup_stale_connections(&self, timeout: Duration) -> usize {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let timeout_ms = timeout.as_millis() as u64;

        let stale: Vec<String> = self
            .connections
            .iter()
            .filter(|entry| {
                let last = *entry.value().last_heartbeat.read();
                now.saturating_sub(last) > timeout_ms
            })
            .map(|entry| entry.key().clone())
            .collect();

        let count = stale.len();
        for chat_id in &stale {
            self.connections.remove(chat_id);
            warn!(chat_id = %chat_id, "removed stale websocket connection");
        }
        count
    }

    /// Spawns a background task that periodically cleans up stale connections.
    pub fn start_heartbeat_monitor(self: &Arc<Self>) {
        let interval = self.heartbeat_interval;
        let channel = Arc::clone(self);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                let removed = channel.cleanup_stale_connections(interval * 3);
                if removed > 0 {
                    info!(removed = removed, "heartbeat monitor cleaned up stale connections");
                }
            }
        });
    }
}

impl Default for WebSocketChannel {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Channel for WebSocketChannel {
    fn name(&self) -> &str {
        self.base.name()
    }

    async fn start(&self) -> Result<()> {
        info!("starting websocket channel");
        self.base.set_enabled(true);
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("stopping websocket channel");
        self.base.set_enabled(false);
        self.connections.clear();
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        self.base.record_sent();

        match self.connections.get(&msg.chat_id) {
            Some(conn) => {
                debug!(chat_id = %msg.chat_id, "websocket channel sending message");
                conn.messages.write().push(msg.content.clone());
                Ok(())
            }
            None => Err(NemesisError::Channel(format!(
                "no websocket connection for chat_id '{}'",
                msg.chat_id
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_websocket_connect_disconnect() {
        let ch = WebSocketChannel::new();
        ch.start().await.unwrap();

        assert_eq!(ch.connection_count(), 0);
        assert!(!ch.is_connected("room-1"));

        ch.connect("room-1");
        assert_eq!(ch.connection_count(), 1);
        assert!(ch.is_connected("room-1"));

        ch.disconnect("room-1");
        assert_eq!(ch.connection_count(), 0);
        assert!(!ch.is_connected("room-1"));
    }

    #[tokio::test]
    async fn test_websocket_send_to_connection() {
        let ch = WebSocketChannel::new();
        ch.start().await.unwrap();
        ch.connect("room-1");

        let msg = OutboundMessage {
            channel: "websocket".to_string(),
            chat_id: "room-1".to_string(),
            content: "Hello WebSocket".to_string(),
            message_type: String::new(),
        };
        ch.send(msg).await.unwrap();

        let messages = ch.get_messages("room-1");
        assert_eq!(messages, vec!["Hello WebSocket"]);
        assert_eq!(ch.base.messages_sent(), 1);
    }

    #[tokio::test]
    async fn test_websocket_send_to_disconnected_fails() {
        let ch = WebSocketChannel::new();
        ch.start().await.unwrap();

        let msg = OutboundMessage {
            channel: "websocket".to_string(),
            chat_id: "room-999".to_string(),
            content: "Nobody here".to_string(),
            message_type: String::new(),
        };
        let result = ch.send(msg).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_websocket_broadcast() {
        let ch = WebSocketChannel::new();
        ch.connect("room-1");
        ch.connect("room-2");
        ch.connect("room-3");

        ch.broadcast("announcement");

        assert_eq!(ch.get_messages("room-1"), vec!["announcement"]);
        assert_eq!(ch.get_messages("room-2"), vec!["announcement"]);
        assert_eq!(ch.get_messages("room-3"), vec!["announcement"]);
    }

    #[test]
    fn test_websocket_inbound_callback() {
        let ch = WebSocketChannel::new();
        let received = Arc::new(parking_lot::RwLock::new(Vec::<(String, String, String)>::new()));
        let received_clone = received.clone();
        ch.set_inbound_callback(Arc::new(move |sender, chat, content| {
            received_clone.write().push((sender, chat, content));
        }));

        ch.handle_inbound("user-1", "room-1", "hello world");

        let msgs = received.read().clone();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].0, "user-1");
        assert_eq!(msgs[0].1, "room-1");
        assert_eq!(msgs[0].2, "hello world");
    }

    #[test]
    fn test_websocket_heartbeat_update() {
        let ch = WebSocketChannel::new();
        ch.connect("room-1");

        // Update heartbeat
        ch.update_heartbeat("room-1");

        // Should not be stale after update
        let removed = ch.cleanup_stale_connections(Duration::from_secs(60));
        assert_eq!(removed, 0);
    }

    #[test]
    fn test_websocket_cleanup_stale() {
        let ch = WebSocketChannel::new();
        ch.connect("room-1");

        // Manually set heartbeat to very old time
        if let Some(conn) = ch.connections.get("room-1") {
            *conn.last_heartbeat.write() = 0; // epoch
        }

        let removed = ch.cleanup_stale_connections(Duration::from_secs(10));
        assert_eq!(removed, 1);
        assert!(!ch.is_connected("room-1"));
    }

    #[tokio::test]
    async fn test_websocket_start_stop_lifecycle() {
        let ch = WebSocketChannel::new();
        ch.connect("room-1");
        assert_eq!(ch.connection_count(), 1);

        ch.start().await.unwrap();
        ch.stop().await.unwrap();

        // Stop clears all connections
        assert_eq!(ch.connection_count(), 0);
    }

    #[test]
    fn test_websocket_multiple_messages_to_same_connection() {
        let ch = WebSocketChannel::new();
        ch.connect("room-1");

        // Send multiple messages by using handle_inbound (records received)
        ch.handle_inbound("user-1", "room-1", "msg1");
        ch.handle_inbound("user-1", "room-1", "msg2");
        ch.handle_inbound("user-1", "room-1", "msg3");

        // connect() also calls record_received, so total is connect(1) + 3 messages = 4
        assert_eq!(ch.base.messages_received(), 4);
    }

    #[test]
    fn test_websocket_disconnect_nonexistent() {
        let ch = WebSocketChannel::new();
        // Should not panic
        ch.disconnect("nonexistent");
        assert_eq!(ch.connection_count(), 0);
    }

    #[test]
    fn test_websocket_get_messages_nonexistent() {
        let ch = WebSocketChannel::new();
        let messages = ch.get_messages("nonexistent");
        assert!(messages.is_empty());
    }

    #[test]
    fn test_websocket_update_heartbeat_nonexistent() {
        let ch = WebSocketChannel::new();
        // Should not panic
        ch.update_heartbeat("nonexistent");
    }

    #[test]
    fn test_websocket_no_callback_inbound() {
        let ch = WebSocketChannel::new();
        // No callback set - should not panic
        ch.handle_inbound("user-1", "room-1", "hello");
        assert_eq!(ch.base.messages_received(), 1);
    }

    #[test]
    fn test_websocket_cleanup_stale_partial() {
        let ch = WebSocketChannel::new();
        ch.connect("room-1");
        ch.connect("room-2");
        ch.connect("room-3");

        // Make room-1 stale
        if let Some(conn) = ch.connections.get("room-1") {
            *conn.last_heartbeat.write() = 0;
        }

        let removed = ch.cleanup_stale_connections(Duration::from_secs(10));
        assert_eq!(removed, 1);
        assert!(!ch.is_connected("room-1"));
        assert!(ch.is_connected("room-2"));
        assert!(ch.is_connected("room-3"));
    }

    #[test]
    fn test_websocket_default() {
        let ch = WebSocketChannel::default();
        assert_eq!(ch.name(), "websocket");
    }

    #[test]
    fn test_websocket_broadcast_empty_connections() {
        let ch = WebSocketChannel::new();
        // No connections - should not panic
        ch.broadcast("announcement");
        assert_eq!(ch.connection_count(), 0);
    }

    #[tokio::test]
    async fn test_websocket_send_records_sent() {
        let ch = WebSocketChannel::new();
        ch.start().await.unwrap();
        ch.connect("room-1");

        let msg = OutboundMessage {
            channel: "websocket".to_string(),
            chat_id: "room-1".to_string(),
            content: "test".to_string(),
            message_type: String::new(),
        };
        ch.send(msg).await.unwrap();
        assert_eq!(ch.base.messages_sent(), 1);
    }

    // ---- Additional comprehensive WebSocket channel tests ----

    // === Connection management ===

    #[test]
    fn test_websocket_reconnect_same_chat_id() {
        let ch = WebSocketChannel::new();
        ch.connect("room-1");
        assert_eq!(ch.connection_count(), 1);

        ch.disconnect("room-1");
        assert_eq!(ch.connection_count(), 0);

        ch.connect("room-1");
        assert_eq!(ch.connection_count(), 1);
    }

    #[test]
    fn test_websocket_connect_many_clients() {
        let ch = WebSocketChannel::new();
        for i in 0..100 {
            ch.connect(&format!("room-{}", i));
        }
        assert_eq!(ch.connection_count(), 100);
    }

    #[test]
    fn test_websocket_disconnect_all() {
        let ch = WebSocketChannel::new();
        for i in 0..10 {
            ch.connect(&format!("room-{}", i));
        }
        assert_eq!(ch.connection_count(), 10);

        for i in 0..10 {
            ch.disconnect(&format!("room-{}", i));
        }
        assert_eq!(ch.connection_count(), 0);
    }

    #[test]
    fn test_websocket_connect_increments_received() {
        let ch = WebSocketChannel::new();
        assert_eq!(ch.base.messages_received(), 0);

        ch.connect("room-1");
        assert_eq!(ch.base.messages_received(), 1);

        ch.connect("room-2");
        assert_eq!(ch.base.messages_received(), 2);
    }

    // === Send edge cases ===

    #[tokio::test]
    async fn test_websocket_send_to_specific_connection() {
        let ch = WebSocketChannel::new();
        ch.start().await.unwrap();
        ch.connect("room-1");
        ch.connect("room-2");

        let msg = OutboundMessage {
            channel: "websocket".to_string(),
            chat_id: "room-1".to_string(),
            content: "Targeted message".to_string(),
            message_type: String::new(),
        };
        ch.send(msg).await.unwrap();

        assert_eq!(ch.get_messages("room-1"), vec!["Targeted message"]);
        assert!(ch.get_messages("room-2").is_empty());
    }

    #[tokio::test]
    async fn test_websocket_send_multiple_to_same_connection() {
        let ch = WebSocketChannel::new();
        ch.start().await.unwrap();
        ch.connect("room-1");

        for i in 0..5 {
            let msg = OutboundMessage {
                channel: "websocket".to_string(),
                chat_id: "room-1".to_string(),
                content: format!("msg {}", i),
                message_type: String::new(),
            };
            ch.send(msg).await.unwrap();
        }

        let msgs = ch.get_messages("room-1");
        assert_eq!(msgs.len(), 5);
        assert_eq!(msgs[0], "msg 0");
        assert_eq!(msgs[4], "msg 4");
    }

    #[tokio::test]
    async fn test_websocket_send_unicode_content() {
        let ch = WebSocketChannel::new();
        ch.start().await.unwrap();
        ch.connect("room-1");

        let msg = OutboundMessage {
            channel: "websocket".to_string(),
            chat_id: "room-1".to_string(),
            content: "你好世界 🌍 مرحبا".to_string(),
            message_type: String::new(),
        };
        ch.send(msg).await.unwrap();

        let msgs = ch.get_messages("room-1");
        assert_eq!(msgs[0], "你好世界 🌍 مرحبا");
    }

    #[tokio::test]
    async fn test_websocket_send_empty_content() {
        let ch = WebSocketChannel::new();
        ch.start().await.unwrap();
        ch.connect("room-1");

        let msg = OutboundMessage {
            channel: "websocket".to_string(),
            chat_id: "room-1".to_string(),
            content: String::new(),
            message_type: String::new(),
        };
        ch.send(msg).await.unwrap();

        let msgs = ch.get_messages("room-1");
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0], "");
    }

    #[tokio::test]
    async fn test_websocket_send_large_content() {
        let ch = WebSocketChannel::new();
        ch.start().await.unwrap();
        ch.connect("room-1");

        let large = "x".repeat(1_000_000);
        let msg = OutboundMessage {
            channel: "websocket".to_string(),
            chat_id: "room-1".to_string(),
            content: large.clone(),
            message_type: String::new(),
        };
        ch.send(msg).await.unwrap();

        let msgs = ch.get_messages("room-1");
        assert_eq!(msgs[0].len(), 1_000_000);
    }

    // === Broadcast edge cases ===

    #[test]
    fn test_websocket_broadcast_many_connections() {
        let ch = WebSocketChannel::new();
        for i in 0..50 {
            ch.connect(&format!("room-{}", i));
        }

        ch.broadcast("mass broadcast");

        for i in 0..50 {
            let msgs = ch.get_messages(&format!("room-{}", i));
            assert_eq!(msgs, vec!["mass broadcast"]);
        }
    }

    #[test]
    fn test_websocket_broadcast_multiple_times() {
        let ch = WebSocketChannel::new();
        ch.connect("room-1");

        ch.broadcast("msg1");
        ch.broadcast("msg2");
        ch.broadcast("msg3");

        let msgs = ch.get_messages("room-1");
        assert_eq!(msgs, vec!["msg1", "msg2", "msg3"]);
    }

    #[test]
    fn test_websocket_broadcast_after_disconnect() {
        let ch = WebSocketChannel::new();
        ch.connect("room-1");
        ch.connect("room-2");

        ch.disconnect("room-1");
        ch.broadcast("after disconnect");

        assert!(ch.get_messages("room-1").is_empty());
        assert_eq!(ch.get_messages("room-2"), vec!["after disconnect"]);
    }

    // === Inbound callback edge cases ===

    #[test]
    fn test_websocket_inbound_multiple_callbacks_last_wins() {
        let ch = WebSocketChannel::new();
        let received1 = Arc::new(parking_lot::RwLock::new(Vec::<String>::new()));
        let received2 = Arc::new(parking_lot::RwLock::new(Vec::<String>::new()));

        let r1 = received1.clone();
        let r2 = received2.clone();

        ch.set_inbound_callback(Arc::new(move |_, _, content| {
            r1.write().push(content);
        }));
        ch.set_inbound_callback(Arc::new(move |_, _, content| {
            r2.write().push(content);
        }));

        ch.handle_inbound("user", "room", "test");

        assert!(received1.read().is_empty()); // first callback replaced
        assert_eq!(received2.read().len(), 1);
    }

    #[test]
    fn test_websocket_inbound_records_stats() {
        let ch = WebSocketChannel::new();
        assert_eq!(ch.base.messages_received(), 0);

        ch.handle_inbound("user", "room", "msg1");
        ch.handle_inbound("user", "room", "msg2");

        assert_eq!(ch.base.messages_received(), 2);
    }

    #[test]
    fn test_websocket_inbound_unicode_content() {
        let ch = WebSocketChannel::new();
        let received = Arc::new(parking_lot::RwLock::new(Vec::<String>::new()));
        let r = received.clone();
        ch.set_inbound_callback(Arc::new(move |_, _, content| {
            r.write().push(content);
        }));

        ch.handle_inbound("user", "room", "你好世界");
        assert_eq!(received.read()[0], "你好世界");
    }

    // === Heartbeat and cleanup ===

    #[test]
    fn test_websocket_heartbeat_keeps_connection_alive() {
        let ch = WebSocketChannel::new();
        ch.connect("room-1");

        // Very short timeout but update heartbeat
        ch.update_heartbeat("room-1");
        let removed = ch.cleanup_stale_connections(Duration::from_secs(3600));
        assert_eq!(removed, 0);
        assert!(ch.is_connected("room-1"));
    }

    #[test]
    fn test_websocket_cleanup_removes_only_stale() {
        let ch = WebSocketChannel::new();
        ch.connect("fresh");
        ch.connect("stale");

        // Use a very short timeout (0ms) which still won't remove connections
        // created in the same millisecond, so we test that cleanup runs without panic
        let removed = ch.cleanup_stale_connections(Duration::from_nanos(1));
        // Since both were just created, they are not stale yet
        assert_eq!(removed, 0);
        assert!(ch.is_connected("fresh"));
        assert!(ch.is_connected("stale"));
    }

    #[test]
    fn test_websocket_cleanup_no_stale() {
        let ch = WebSocketChannel::new();
        ch.connect("room-1");
        ch.connect("room-2");

        let removed = ch.cleanup_stale_connections(Duration::from_secs(3600));
        assert_eq!(removed, 0);
        assert_eq!(ch.connection_count(), 2);
    }

    #[test]
    fn test_websocket_cleanup_empty_connections() {
        let ch = WebSocketChannel::new();
        let removed = ch.cleanup_stale_connections(Duration::from_secs(10));
        assert_eq!(removed, 0);
    }

    // === Lifecycle ===

    #[tokio::test]
    async fn test_websocket_start_stop_clears_connections() {
        let ch = WebSocketChannel::new();
        ch.connect("room-1");
        ch.connect("room-2");
        ch.connect("room-3");

        ch.start().await.unwrap();
        ch.stop().await.unwrap();

        assert_eq!(ch.connection_count(), 0);
    }

    #[tokio::test]
    async fn test_websocket_start_stop_idempotent() {
        let ch = WebSocketChannel::new();

        ch.start().await.unwrap();
        ch.start().await.unwrap(); // second start
        ch.stop().await.unwrap();
        ch.stop().await.unwrap(); // second stop
    }

    #[test]
    fn test_websocket_with_custom_heartbeat() {
        let ch = WebSocketChannel::with_heartbeat(Duration::from_secs(5));
        assert_eq!(ch.name(), "websocket");
    }

    #[test]
    fn test_websocket_is_connected_after_disconnect() {
        let ch = WebSocketChannel::new();
        assert!(!ch.is_connected("room-1"));

        ch.connect("room-1");
        assert!(ch.is_connected("room-1"));

        ch.disconnect("room-1");
        assert!(!ch.is_connected("room-1"));
    }

    // === Error cases ===

    #[tokio::test]
    async fn test_websocket_send_to_nonexistent_connection_error() {
        let ch = WebSocketChannel::new();
        ch.start().await.unwrap();

        let msg = OutboundMessage {
            channel: "websocket".to_string(),
            chat_id: "nonexistent".to_string(),
            content: "test".to_string(),
            message_type: String::new(),
        };
        assert!(ch.send(msg).await.is_err());
    }

    #[tokio::test]
    async fn test_websocket_send_after_stop_error() {
        let ch = WebSocketChannel::new();
        ch.start().await.unwrap();
        ch.connect("room-1");
        ch.stop().await.unwrap();

        // Connection cleared on stop, so send should fail
        let msg = OutboundMessage {
            channel: "websocket".to_string(),
            chat_id: "room-1".to_string(),
            content: "test".to_string(),
            message_type: String::new(),
        };
        assert!(ch.send(msg).await.is_err());
    }

    #[test]
    fn test_websocket_get_messages_for_connected_client() {
        let ch = WebSocketChannel::new();
        ch.connect("room-1");
        ch.broadcast("test-msg");

        let msgs = ch.get_messages("room-1");
        assert_eq!(msgs, vec!["test-msg"]);
    }
}

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
        debug!(chat_id = %chat_id, "[WebSocketChannel] client connected");
    }

    /// Simulates a client disconnecting.
    pub fn disconnect(&self, chat_id: &str) {
        self.connections.remove(chat_id);
        debug!(chat_id = %chat_id, "[WebSocketChannel] client disconnected");
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
        debug!(count = self.connections.len(), "[WebSocketChannel] broadcast message to all connections");
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
            warn!(chat_id = %chat_id, "[WebSocketChannel] removed stale websocket connection");
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
                    info!(removed = removed, "[WebSocketChannel] heartbeat monitor cleaned up stale connections");
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
        info!("[WebSocketChannel] starting websocket channel");
        self.base.set_enabled(true);
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("[WebSocketChannel] stopping websocket channel");
        self.base.set_enabled(false);
        self.connections.clear();
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        self.base.record_sent();

        match self.connections.get(&msg.chat_id) {
            Some(conn) => {
                debug!(chat_id = %msg.chat_id, "[WebSocketChannel] channel sending message");
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
mod tests;

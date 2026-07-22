//! SSE Event Hub for server-sent events.

use std::sync::Arc;
use tokio::sync::broadcast;

/// Event type constants.
pub const EVENT_LOG: &str = "log";
pub const EVENT_STATUS: &str = "status";
pub const EVENT_SECURITY_ALERT: &str = "security-alert";
pub const EVENT_SCANNER_PROGRESS: &str = "scanner-progress";
pub const EVENT_CLUSTER_EVENT: &str = "cluster-event";
pub const EVENT_HEARTBEAT: &str = "heartbeat";
/// Chat streaming delta — published for each streamed LLM token chunk.
pub const EVENT_CHAT_STREAM: &str = "chat-stream";

/// A server-sent event.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Event {
    pub event_type: String,
    pub data: serde_json::Value,
}

/// Event hub that manages SSE subscribers and broadcasts events.
pub struct EventHub {
    sender: broadcast::Sender<Event>,
    subscriber_count: Arc<std::sync::atomic::AtomicUsize>,
}

impl EventHub {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(256);
        Self {
            sender,
            subscriber_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    /// Subscribe to events. Returns a receiver.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.subscriber_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.sender.subscribe()
    }

    /// Unsubscribe (decrement counter).
    pub fn unsubscribe(&self) {
        self.subscriber_count
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }

    /// Publish an event to all subscribers.
    pub fn publish(&self, event_type: &str, data: serde_json::Value) {
        let event = Event {
            event_type: event_type.to_string(),
            data,
        };
        // broadcast::send ignores errors when no receivers
        let _ = self.sender.send(event);
    }

    /// Get the number of active subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.subscriber_count
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Default for EventHub {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;

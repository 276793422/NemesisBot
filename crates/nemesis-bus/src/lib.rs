//! NemesisBot - Message Bus
//!
//! Central pub/sub system using tokio broadcast channels.
//! Replaces Go channels with tokio::sync::broadcast for multi-subscriber support.

use nemesis_types::channel::{InboundMessage, OutboundMessage};
use nemesis_types::constants::BUS_CHANNEL_CAPACITY;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::broadcast;
use tracing::warn;

/// Message bus for routing messages between channels and agents.
pub struct MessageBus {
    inbound_tx: broadcast::Sender<InboundMessage>,
    outbound_tx: broadcast::Sender<OutboundMessage>,
    closed: AtomicBool,
    /// Number of inbound messages dropped because no receivers or buffer full.
    inbound_dropped: AtomicU64,
    /// Number of outbound messages dropped because no receivers or buffer full.
    outbound_dropped: AtomicU64,
}

impl MessageBus {
    /// Create a new message bus with default capacity.
    pub fn new() -> Self {
        Self::with_capacity(BUS_CHANNEL_CAPACITY)
    }

    /// Create a new message bus with custom capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        let (inbound_tx, _) = broadcast::channel(capacity);
        let (outbound_tx, _) = broadcast::channel(capacity);
        Self {
            inbound_tx,
            outbound_tx,
            closed: AtomicBool::new(false),
            inbound_dropped: AtomicU64::new(0),
            outbound_dropped: AtomicU64::new(0),
        }
    }

    /// Publish an inbound message (from channel to agent).
    ///
    /// Unlike Go which blocks when the channel buffer is full, Rust's broadcast
    /// silently drops messages. This method logs a warning when the send fails
    /// (e.g., no receivers or buffer full) and increments a dropped counter.
    pub fn publish_inbound(&self, msg: InboundMessage) {
        if self.closed.load(Ordering::Relaxed) {
            return;
        }
        let receiver_count = self.inbound_tx.receiver_count();
        if receiver_count == 0 {
            warn!(
                "publish_inbound: no inbound receivers, message will be dropped (channel={}, chat_id={})",
                msg.channel, msg.chat_id
            );
            self.inbound_dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if let Err(err) = self.inbound_tx.send(msg) {
            self.inbound_dropped.fetch_add(1, Ordering::Relaxed);
            warn!("publish_inbound: failed to send inbound message: {}", err);
        }
    }

    /// Publish an outbound message (from agent to channel).
    ///
    /// Unlike Go which blocks when the channel buffer is full, Rust's broadcast
    /// silently drops messages. This method logs a warning when the send fails
    /// (e.g., no receivers or buffer full) and increments a dropped counter.
    pub fn publish_outbound(&self, msg: OutboundMessage) {
        if self.closed.load(Ordering::Relaxed) {
            return;
        }
        let receiver_count = self.outbound_tx.receiver_count();
        if receiver_count == 0 {
            warn!(
                "publish_outbound: no outbound receivers, message will be dropped (channel={}, chat_id={})",
                msg.channel, msg.chat_id
            );
            self.outbound_dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if let Err(err) = self.outbound_tx.send(msg) {
            self.outbound_dropped.fetch_add(1, Ordering::Relaxed);
            warn!("publish_outbound: failed to send outbound message: {}", err);
        }
    }

    /// Subscribe to inbound messages.
    ///
    /// Logs a warning if there are already existing subscribers, because
    /// broadcast is fan-out (every subscriber gets every message), unlike
    /// Go's point-to-point channels. Having multiple subscribers is usually
    /// unintentional and may indicate a bug where a component subscribes
    /// more than once.
    pub fn subscribe_inbound(&self) -> broadcast::Receiver<InboundMessage> {
        let existing = self.inbound_tx.receiver_count();
        if existing > 0 {
            warn!(
                existing_receivers = existing,
                "subscribe_inbound: additional subscriber added to broadcast channel; \
                 each subscriber receives every message (fan-out), which may be unintentional"
            );
        }
        self.inbound_tx.subscribe()
    }

    /// Subscribe to outbound messages.
    ///
    /// Logs a warning if there are already existing subscribers for the same
    /// fan-out concern as `subscribe_inbound`.
    pub fn subscribe_outbound(&self) -> broadcast::Receiver<OutboundMessage> {
        let existing = self.outbound_tx.receiver_count();
        if existing > 0 {
            warn!(
                existing_receivers = existing,
                "subscribe_outbound: additional subscriber added to broadcast channel; \
                 each subscriber receives every message (fan-out), which may be unintentional"
            );
        }
        self.outbound_tx.subscribe()
    }

    /// Get a sender for inbound messages (for direct publishing).
    pub fn inbound_sender(&self) -> broadcast::Sender<InboundMessage> {
        self.inbound_tx.clone()
    }

    /// Get a sender for outbound messages (for direct publishing).
    pub fn outbound_sender(&self) -> broadcast::Sender<OutboundMessage> {
        self.outbound_tx.clone()
    }

    /// Check if the bus is closed.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    /// Returns the number of active inbound subscribers.
    pub fn inbound_subscriber_count(&self) -> usize {
        self.inbound_tx.receiver_count()
    }

    /// Returns the number of active outbound subscribers.
    pub fn outbound_subscriber_count(&self) -> usize {
        self.outbound_tx.receiver_count()
    }

    /// Returns the total number of inbound messages dropped since bus creation.
    pub fn dropped_inbound(&self) -> u64 {
        self.inbound_dropped.load(Ordering::Relaxed)
    }

    /// Returns the total number of outbound messages dropped since bus creation.
    pub fn dropped_outbound(&self) -> u64 {
        self.outbound_dropped.load(Ordering::Relaxed)
    }

    /// Close the bus. No more messages can be published.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
    }
}

impl Default for MessageBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_publish_subscribe_inbound() {
        let bus = MessageBus::new();
        let mut rx = bus.subscribe_inbound();

        bus.publish_inbound(InboundMessage {
            channel: "test".to_string(),
            sender_id: "user1".to_string(),
            chat_id: "chat1".to_string(),
            content: "hello".to_string(),
            media: vec![],
            session_key: "test:chat1".to_string(),
            correlation_id: String::new(),
            metadata: std::collections::HashMap::new(),
        });

        let msg = rx.recv().await.unwrap();
        assert_eq!(msg.channel, "test");
        assert_eq!(msg.content, "hello");
    }

    #[tokio::test]
    async fn test_publish_subscribe_outbound() {
        let bus = MessageBus::new();
        let mut rx = bus.subscribe_outbound();

        bus.publish_outbound(OutboundMessage {
            channel: "test".to_string(),
            chat_id: "chat1".to_string(),
            content: "response".to_string(),
            message_type: String::new(),
        });

        let msg = rx.recv().await.unwrap();
        assert_eq!(msg.content, "response");
    }

    #[tokio::test]
    async fn test_multiple_subscribers() {
        let bus = MessageBus::new();
        let mut rx1 = bus.subscribe_inbound();
        let mut rx2 = bus.subscribe_inbound();

        bus.publish_inbound(InboundMessage {
            channel: "test".to_string(),
            sender_id: "u".to_string(),
            chat_id: "c".to_string(),
            content: "broadcast".to_string(),
            media: vec![],
            session_key: "t:c".to_string(),
            correlation_id: String::new(),
            metadata: std::collections::HashMap::new(),
        });

        let msg1 = rx1.recv().await.unwrap();
        let msg2 = rx2.recv().await.unwrap();
        assert_eq!(msg1.content, "broadcast");
        assert_eq!(msg2.content, "broadcast");
    }

    #[tokio::test]
    async fn test_close_bus() {
        let bus = MessageBus::new();
        assert!(!bus.is_closed());

        bus.close();
        assert!(bus.is_closed());

        // Publishing after close should be a no-op
        bus.publish_inbound(InboundMessage {
            channel: "test".to_string(),
            sender_id: "u".to_string(),
            chat_id: "c".to_string(),
            content: "after close".to_string(),
            media: vec![],
            session_key: "t:c".to_string(),
            correlation_id: String::new(),
            metadata: std::collections::HashMap::new(),
        });

        // Receiver should not get the message (timeout)
        let mut rx = bus.subscribe_inbound();
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            rx.recv(),
        ).await;
        assert!(result.is_err() || result.unwrap().is_err());
    }

    #[tokio::test]
    async fn test_concurrent_publish() {
        let bus = MessageBus::new();
        let mut rx = bus.subscribe_inbound();

        let bus_clone = std::sync::Arc::new(bus);
        let mut handles = vec![];

        for i in 0..10 {
            let b = bus_clone.clone();
            handles.push(tokio::spawn(async move {
                b.publish_inbound(InboundMessage {
                    channel: "test".to_string(),
                    sender_id: format!("user{}", i),
                    chat_id: "chat".to_string(),
                    content: format!("msg{}", i),
                    media: vec![],
                    session_key: "test:chat".to_string(),
                    correlation_id: String::new(),
                    metadata: std::collections::HashMap::new(),
                });
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        // Should receive all 10 messages
        let mut count = 0;
        for _ in 0..10 {
            if rx.try_recv().is_ok() {
                count += 1;
            }
        }
        assert_eq!(count, 10);
    }

    #[test]
    fn test_new_creates_bus() {
        let bus = MessageBus::new();
        assert!(!bus.is_closed());
    }

    #[test]
    fn test_default_creates_bus() {
        let bus = MessageBus::default();
        assert!(!bus.is_closed());
    }

    #[test]
    fn test_with_capacity_custom() {
        let bus = MessageBus::with_capacity(16);
        assert!(!bus.is_closed());
    }

    #[tokio::test]
    async fn test_close_multiple_times_no_panic() {
        let bus = MessageBus::new();
        bus.close();
        bus.close();
        bus.close();
        assert!(bus.is_closed());
    }

    #[tokio::test]
    async fn test_inbound_sender() {
        let bus = MessageBus::new();
        let mut rx = bus.subscribe_inbound();
        let sender = bus.inbound_sender();

        sender.send(InboundMessage {
            channel: "test".to_string(),
            sender_id: "u1".to_string(),
            chat_id: "c1".to_string(),
            content: "via sender".to_string(),
            media: vec![],
            session_key: "t:c".to_string(),
            correlation_id: String::new(),
            metadata: std::collections::HashMap::new(),
        }).unwrap();

        let msg = rx.recv().await.unwrap();
        assert_eq!(msg.content, "via sender");
    }

    #[tokio::test]
    async fn test_outbound_sender() {
        let bus = MessageBus::new();
        let mut rx = bus.subscribe_outbound();
        let sender = bus.outbound_sender();

        sender.send(OutboundMessage {
            channel: "test".to_string(),
            chat_id: "c1".to_string(),
            content: "via outbound sender".to_string(),
            message_type: String::new(),
        }).unwrap();

        let msg = rx.recv().await.unwrap();
        assert_eq!(msg.content, "via outbound sender");
    }

    #[tokio::test]
    async fn test_late_subscriber_misses_messages() {
        let bus = MessageBus::new();

        bus.publish_inbound(InboundMessage {
            channel: "test".to_string(),
            sender_id: "u".to_string(),
            chat_id: "c".to_string(),
            content: "early msg".to_string(),
            media: vec![],
            session_key: "t:c".to_string(),
            correlation_id: String::new(),
            metadata: std::collections::HashMap::new(),
        });

        // Subscribe after publishing - late subscriber should not get the message
        let mut rx = bus.subscribe_inbound();
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            rx.recv(),
        ).await;
        assert!(result.is_err() || result.unwrap().is_err());
    }

    #[tokio::test]
    async fn test_inbound_preserves_all_fields() {
        let bus = MessageBus::new();
        let mut rx = bus.subscribe_inbound();

        let mut metadata = std::collections::HashMap::new();
        metadata.insert("key1".to_string(), "value1".to_string());

        bus.publish_inbound(InboundMessage {
            channel: "rpc".to_string(),
            sender_id: "sender123".to_string(),
            chat_id: "chat456".to_string(),
            content: "test content".to_string(),
            media: vec![],
            session_key: "rpc:chat456".to_string(),
            correlation_id: "corr-123".to_string(),
            metadata: metadata.clone(),
        });

        let msg = rx.recv().await.unwrap();
        assert_eq!(msg.channel, "rpc");
        assert_eq!(msg.sender_id, "sender123");
        assert_eq!(msg.chat_id, "chat456");
        assert_eq!(msg.content, "test content");
        assert_eq!(msg.session_key, "rpc:chat456");
        assert_eq!(msg.correlation_id, "corr-123");
        assert_eq!(msg.metadata, metadata);
    }

    #[tokio::test]
    async fn test_outbound_preserves_fields() {
        let bus = MessageBus::new();
        let mut rx = bus.subscribe_outbound();

        bus.publish_outbound(OutboundMessage {
            channel: "web".to_string(),
            chat_id: "chat789".to_string(),
            content: "response text".to_string(),
            message_type: "text".to_string(),
        });

        let msg = rx.recv().await.unwrap();
        assert_eq!(msg.channel, "web");
        assert_eq!(msg.chat_id, "chat789");
        assert_eq!(msg.content, "response text");
        assert_eq!(msg.message_type, "text");
    }

    #[tokio::test]
    async fn test_concurrent_publish_outbound() {
        let bus = MessageBus::new();
        let mut rx = bus.subscribe_outbound();

        let bus_clone = std::sync::Arc::new(bus);
        let mut handles = vec![];

        for i in 0..5 {
            let b = bus_clone.clone();
            handles.push(tokio::spawn(async move {
                b.publish_outbound(OutboundMessage {
                    channel: "test".to_string(),
                    chat_id: format!("chat{}", i),
                    content: format!("outbound{}", i),
                    message_type: String::new(),
                });
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        let mut count = 0;
        for _ in 0..5 {
            if rx.try_recv().is_ok() {
                count += 1;
            }
        }
        assert_eq!(count, 5);
    }

    #[tokio::test]
    async fn test_sequential_inbound_messages() {
        let bus = MessageBus::new();
        let mut rx = bus.subscribe_inbound();

        for i in 0..5 {
            bus.publish_inbound(InboundMessage {
                channel: "test".to_string(),
                sender_id: "u".to_string(),
                chat_id: "c".to_string(),
                content: format!("msg{}", i),
                media: vec![],
                session_key: "t:c".to_string(),
                correlation_id: String::new(),
                metadata: std::collections::HashMap::new(),
            });
        }

        // Receive all 5 in order
        for i in 0..5 {
            let msg = rx.try_recv().unwrap();
            assert_eq!(msg.content, format!("msg{}", i));
        }
    }

    // --- Benchmark-style throughput test ---
    #[test]
    fn test_bus_publish_throughput() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let bus = std::sync::Arc::new(MessageBus::new());
            let count = 1_000;

            let start = std::time::Instant::now();
            for i in 0..count {
                bus.publish_inbound(InboundMessage {
                    channel: "bench".to_string(),
                    sender_id: "u".to_string(),
                    chat_id: "c".to_string(),
                    content: format!("msg{}", i),
                    media: vec![],
                    session_key: "t:c".to_string(),
                    correlation_id: String::new(),
                    metadata: std::collections::HashMap::new(),
                });
            }
            let elapsed = start.elapsed();
            // Should publish 1k messages in under 1 second
            assert!(elapsed < std::time::Duration::from_secs(1), "Bus publish too slow: {:?}", elapsed);
        });
    }

    #[test]
    fn test_publish_inbound_no_receivers_no_panic() {
        let bus = MessageBus::new();
        // Publishing with no receivers should not panic
        bus.publish_inbound(InboundMessage {
            channel: "test".to_string(),
            sender_id: "u".to_string(),
            chat_id: "c".to_string(),
            content: "orphan".to_string(),
            media: vec![],
            session_key: "t:c".to_string(),
            correlation_id: String::new(),
            metadata: std::collections::HashMap::new(),
        });
        // Should have incremented the dropped counter
        assert_eq!(bus.dropped_inbound(), 1);
        assert_eq!(bus.dropped_outbound(), 0);
    }

    #[test]
    fn test_publish_outbound_no_receivers_no_panic() {
        let bus = MessageBus::new();
        // Publishing with no receivers should not panic
        bus.publish_outbound(OutboundMessage {
            channel: "test".to_string(),
            chat_id: "c".to_string(),
            content: "orphan".to_string(),
            message_type: String::new(),
        });
        assert_eq!(bus.dropped_outbound(), 1);
        assert_eq!(bus.dropped_inbound(), 0);
    }

    #[test]
    fn test_dropped_inbound_counter_multiple() {
        let bus = MessageBus::new();
        assert_eq!(bus.dropped_inbound(), 0);

        // Publish 3 messages with no receivers
        for i in 0..3 {
            bus.publish_inbound(InboundMessage {
                channel: "test".to_string(),
                sender_id: "u".to_string(),
                chat_id: "c".to_string(),
                content: format!("msg{}", i),
                media: vec![],
                session_key: "t:c".to_string(),
                correlation_id: String::new(),
                metadata: std::collections::HashMap::new(),
            });
        }
        assert_eq!(bus.dropped_inbound(), 3);
    }

    #[test]
    fn test_dropped_outbound_counter_multiple() {
        let bus = MessageBus::new();
        assert_eq!(bus.dropped_outbound(), 0);

        for i in 0..4 {
            bus.publish_outbound(OutboundMessage {
                channel: "test".to_string(),
                chat_id: "c".to_string(),
                content: format!("out{}", i),
                message_type: String::new(),
            });
        }
        assert_eq!(bus.dropped_outbound(), 4);
    }

    #[tokio::test]
    async fn test_dropped_counter_resets_after_subscribe() {
        let bus = MessageBus::new();

        // Drop one message with no subscribers
        bus.publish_inbound(InboundMessage {
            channel: "test".to_string(),
            sender_id: "u".to_string(),
            chat_id: "c".to_string(),
            content: "dropped".to_string(),
            media: vec![],
            session_key: "t:c".to_string(),
            correlation_id: String::new(),
            metadata: std::collections::HashMap::new(),
        });
        assert_eq!(bus.dropped_inbound(), 1);

        // Subscribe, now messages should not be dropped
        let mut rx = bus.subscribe_inbound();
        bus.publish_inbound(InboundMessage {
            channel: "test".to_string(),
            sender_id: "u".to_string(),
            chat_id: "c".to_string(),
            content: "delivered".to_string(),
            media: vec![],
            session_key: "t:c".to_string(),
            correlation_id: String::new(),
            metadata: std::collections::HashMap::new(),
        });

        let msg = rx.recv().await.unwrap();
        assert_eq!(msg.content, "delivered");
        // Counter should still be 1 (only the first was dropped)
        assert_eq!(bus.dropped_inbound(), 1);
    }

    #[tokio::test]
    async fn test_inbound_subscriber_count() {
        let bus = MessageBus::new();
        assert_eq!(bus.inbound_subscriber_count(), 0);

        let rx1 = bus.subscribe_inbound();
        assert_eq!(bus.inbound_subscriber_count(), 1);

        let rx2 = bus.subscribe_inbound();
        assert_eq!(bus.inbound_subscriber_count(), 2);

        drop(rx1);
        assert_eq!(bus.inbound_subscriber_count(), 1);

        drop(rx2);
        assert_eq!(bus.inbound_subscriber_count(), 0);
    }

    #[tokio::test]
    async fn test_outbound_subscriber_count() {
        let bus = MessageBus::new();
        assert_eq!(bus.outbound_subscriber_count(), 0);

        let rx1 = bus.subscribe_outbound();
        assert_eq!(bus.outbound_subscriber_count(), 1);

        let rx2 = bus.subscribe_outbound();
        assert_eq!(bus.outbound_subscriber_count(), 2);

        drop(rx1);
        assert_eq!(bus.outbound_subscriber_count(), 1);

        drop(rx2);
        assert_eq!(bus.outbound_subscriber_count(), 0);
    }

    #[tokio::test]
    async fn test_publish_inbound_after_close_drops_silently() {
        let bus = MessageBus::new();
        let _rx = bus.subscribe_inbound();

        bus.close();
        bus.publish_inbound(InboundMessage {
            channel: "test".to_string(),
            sender_id: "u".to_string(),
            chat_id: "c".to_string(),
            content: "after close".to_string(),
            media: vec![],
            session_key: "t:c".to_string(),
            correlation_id: String::new(),
            metadata: std::collections::HashMap::new(),
        });

        // Closed bus should not increment dropped counter (it returns early)
        assert_eq!(bus.dropped_inbound(), 0);
    }

    #[tokio::test]
    async fn test_publish_outbound_after_close_drops_silently() {
        let bus = MessageBus::new();
        let _rx = bus.subscribe_outbound();

        bus.close();
        bus.publish_outbound(OutboundMessage {
            channel: "test".to_string(),
            chat_id: "c".to_string(),
            content: "after close".to_string(),
            message_type: String::new(),
        });

        assert_eq!(bus.dropped_outbound(), 0);
    }
}

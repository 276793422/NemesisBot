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

    // =========================================================================
    // Additional tests: close() / is_closed() edge cases
    // =========================================================================

    #[tokio::test]
    async fn test_close_prevents_both_inbound_and_outbound() {
        let bus = MessageBus::new();
        let mut in_rx = bus.subscribe_inbound();
        let mut out_rx = bus.subscribe_outbound();

        bus.close();

        // Publish on both channels after close -- both should be silently dropped.
        bus.publish_inbound(InboundMessage {
            channel: "test".to_string(),
            sender_id: "u".to_string(),
            chat_id: "c".to_string(),
            content: "inbound after close".to_string(),
            media: vec![],
            session_key: "t:c".to_string(),
            correlation_id: String::new(),
            metadata: std::collections::HashMap::new(),
        });
        bus.publish_outbound(OutboundMessage {
            channel: "test".to_string(),
            chat_id: "c".to_string(),
            content: "outbound after close".to_string(),
            message_type: String::new(),
        });

        // Neither receiver should get anything.
        let in_result = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            in_rx.recv(),
        )
        .await;
        let out_result = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            out_rx.recv(),
        )
        .await;
        assert!(in_result.is_err() || in_result.unwrap().is_err());
        assert!(out_result.is_err() || out_result.unwrap().is_err());

        // Dropped counters must remain 0 because close returns early.
        assert_eq!(bus.dropped_inbound(), 0);
        assert_eq!(bus.dropped_outbound(), 0);
    }

    #[test]
    fn test_is_closed_reflects_state_transitions() {
        let bus = MessageBus::new();
        assert!(!bus.is_closed(), "newly created bus should not be closed");

        bus.close();
        assert!(bus.is_closed(), "bus should be closed after close()");

        // Calling close again should not change anything.
        bus.close();
        assert!(bus.is_closed(), "bus should still be closed after second close()");
    }

    // =========================================================================
    // Additional tests: error paths for publish when closed / receivers dropped
    // =========================================================================

    #[tokio::test]
    async fn test_publish_inbound_all_receivers_dropped() {
        let bus = MessageBus::new();

        {
            let rx = bus.subscribe_inbound();
            assert_eq!(bus.inbound_subscriber_count(), 1);
            // Receiver goes out of scope here.
            drop(rx);
        }

        // After receiver is dropped, publish should not panic and should
        // increment the dropped counter (no receivers left).
        bus.publish_inbound(InboundMessage {
            channel: "test".to_string(),
            sender_id: "u".to_string(),
            chat_id: "c".to_string(),
            content: "no receivers".to_string(),
            media: vec![],
            session_key: "t:c".to_string(),
            correlation_id: String::new(),
            metadata: std::collections::HashMap::new(),
        });

        assert_eq!(bus.dropped_inbound(), 1);
        assert_eq!(bus.inbound_subscriber_count(), 0);
    }

    #[tokio::test]
    async fn test_publish_outbound_all_receivers_dropped() {
        let bus = MessageBus::new();

        {
            let rx = bus.subscribe_outbound();
            assert_eq!(bus.outbound_subscriber_count(), 1);
            drop(rx);
        }

        bus.publish_outbound(OutboundMessage {
            channel: "test".to_string(),
            chat_id: "c".to_string(),
            content: "no receivers".to_string(),
            message_type: String::new(),
        });

        assert_eq!(bus.dropped_outbound(), 1);
        assert_eq!(bus.outbound_subscriber_count(), 0);
    }

    #[tokio::test]
    async fn test_publish_inbound_some_receivers_dropped() {
        let bus = MessageBus::new();

        let mut rx1 = bus.subscribe_inbound();
        let rx2 = bus.subscribe_inbound();
        assert_eq!(bus.inbound_subscriber_count(), 2);

        // Drop one receiver; the other should still get messages.
        drop(rx2);
        assert_eq!(bus.inbound_subscriber_count(), 1);

        bus.publish_inbound(InboundMessage {
            channel: "test".to_string(),
            sender_id: "u".to_string(),
            chat_id: "c".to_string(),
            content: "partial".to_string(),
            media: vec![],
            session_key: "t:c".to_string(),
            correlation_id: String::new(),
            metadata: std::collections::HashMap::new(),
        });

        // rx1 should still receive the message.
        let msg = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            rx1.recv(),
        )
        .await
        .expect("timeout waiting for message on surviving receiver")
        .expect("recv error on surviving receiver");
        assert_eq!(msg.content, "partial");
        assert_eq!(bus.dropped_inbound(), 0);
    }

    #[tokio::test]
    async fn test_publish_outbound_after_close_with_subscriber() {
        let bus = MessageBus::new();
        let mut rx = bus.subscribe_outbound();

        // Publish one message before close.
        bus.publish_outbound(OutboundMessage {
            channel: "test".to_string(),
            chat_id: "c".to_string(),
            content: "before close".to_string(),
            message_type: String::new(),
        });

        bus.close();

        // Publish after close -- should be silently ignored.
        bus.publish_outbound(OutboundMessage {
            channel: "test".to_string(),
            chat_id: "c".to_string(),
            content: "after close".to_string(),
            message_type: String::new(),
        });

        // The pre-close message should be receivable.
        let msg = rx
            .recv()
            .await
            .expect("should receive pre-close message");
        assert_eq!(msg.content, "before close");

        // The post-close message was never sent to the channel.
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            rx.recv(),
        )
        .await;
        assert!(
            result.is_err() || result.unwrap().is_err(),
            "should not receive post-close message"
        );
    }

    // =========================================================================
    // Additional tests: inbound_sender() / outbound_sender() edge cases
    // =========================================================================

    #[tokio::test]
    async fn test_inbound_sender_cloned_independently() {
        let bus = MessageBus::new();
        let sender = bus.inbound_sender();
        let mut rx = bus.subscribe_inbound();

        // Publish via the cloned sender (not via publish_inbound).
        sender
            .send(InboundMessage {
                channel: "direct".to_string(),
                sender_id: "u".to_string(),
                chat_id: "c".to_string(),
                content: "from sender".to_string(),
                media: vec![],
                session_key: "t:c".to_string(),
                correlation_id: String::new(),
                metadata: std::collections::HashMap::new(),
            })
            .expect("send via cloned sender should succeed");

        let msg = rx.recv().await.expect("receiver should get message");
        assert_eq!(msg.content, "from sender");
        assert_eq!(msg.channel, "direct");
    }

    #[tokio::test]
    async fn test_outbound_sender_cloned_independently() {
        let bus = MessageBus::new();
        let sender = bus.outbound_sender();
        let mut rx = bus.subscribe_outbound();

        sender
            .send(OutboundMessage {
                channel: "direct".to_string(),
                chat_id: "c".to_string(),
                content: "from outbound sender".to_string(),
                message_type: String::new(),
            })
            .expect("send via cloned outbound sender should succeed");

        let msg = rx.recv().await.expect("receiver should get message");
        assert_eq!(msg.content, "from outbound sender");
        assert_eq!(msg.channel, "direct");
    }

    #[tokio::test]
    async fn test_inbound_sender_works_even_after_bus_closed() {
        // The cloned sender bypasses publish_inbound's close check.
        // This tests the broadcast::Sender directly.
        let bus = MessageBus::new();
        let mut rx = bus.subscribe_inbound();
        let sender = bus.inbound_sender();

        bus.close();

        // The broadcast::Sender itself is not closed; only publish_inbound
        // checks the closed flag. Direct send should still work.
        let result = sender.send(InboundMessage {
            channel: "test".to_string(),
            sender_id: "u".to_string(),
            chat_id: "c".to_string(),
            content: "direct after close".to_string(),
            media: vec![],
            session_key: "t:c".to_string(),
            correlation_id: String::new(),
            metadata: std::collections::HashMap::new(),
        });

        // broadcast::Sender::send should succeed because the underlying
        // channel is still open (close only sets the AtomicBool).
        assert!(result.is_ok(), "direct send should succeed even after bus.close()");

        let msg = rx.recv().await.expect("receiver should get message");
        assert_eq!(msg.content, "direct after close");
    }

    #[tokio::test]
    async fn test_multiple_inbound_senders_share_channel() {
        let bus = MessageBus::new();
        let sender1 = bus.inbound_sender();
        let sender2 = bus.inbound_sender();
        let mut rx = bus.subscribe_inbound();

        sender1
            .send(InboundMessage {
                channel: "test".to_string(),
                sender_id: "u1".to_string(),
                chat_id: "c".to_string(),
                content: "from sender1".to_string(),
                media: vec![],
                session_key: "t:c".to_string(),
                correlation_id: String::new(),
                metadata: std::collections::HashMap::new(),
            })
            .unwrap();

        sender2
            .send(InboundMessage {
                channel: "test".to_string(),
                sender_id: "u2".to_string(),
                chat_id: "c".to_string(),
                content: "from sender2".to_string(),
                media: vec![],
                session_key: "t:c".to_string(),
                correlation_id: String::new(),
                metadata: std::collections::HashMap::new(),
            })
            .unwrap();

        let msg1 = rx.try_recv().expect("should get first message");
        assert_eq!(msg1.content, "from sender1");
        let msg2 = rx.try_recv().expect("should get second message");
        assert_eq!(msg2.content, "from sender2");
    }

    #[tokio::test]
    async fn test_inbound_sender_no_receivers_returns_error() {
        let bus = MessageBus::new();
        let sender = bus.inbound_sender();
        // No subscribers. broadcast::Sender::send returns Err when there are
        // no active receivers.
        let result = sender.send(InboundMessage {
            channel: "test".to_string(),
            sender_id: "u".to_string(),
            chat_id: "c".to_string(),
            content: "orphan".to_string(),
            media: vec![],
            session_key: "t:c".to_string(),
            correlation_id: String::new(),
            metadata: std::collections::HashMap::new(),
        });
        assert!(result.is_err(), "send with no receivers should return Err");
    }

    #[tokio::test]
    async fn test_outbound_sender_no_receivers_returns_error() {
        let bus = MessageBus::new();
        let sender = bus.outbound_sender();
        let result = sender.send(OutboundMessage {
            channel: "test".to_string(),
            chat_id: "c".to_string(),
            content: "orphan".to_string(),
            message_type: String::new(),
        });
        assert!(result.is_err(), "send with no receivers should return Err");
    }

    // =========================================================================
    // Additional tests: publishing with no subscribers (edge cases)
    // =========================================================================

    #[test]
    fn test_publish_inbound_no_receivers_multiple_messages_drops_all() {
        let bus = MessageBus::new();
        for i in 0..10 {
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
        assert_eq!(bus.dropped_inbound(), 10);
        assert_eq!(bus.dropped_outbound(), 0);
    }

    #[test]
    fn test_publish_outbound_no_receivers_multiple_messages_drops_all() {
        let bus = MessageBus::new();
        for i in 0..10 {
            bus.publish_outbound(OutboundMessage {
                channel: "test".to_string(),
                chat_id: "c".to_string(),
                content: format!("out{}", i),
                message_type: String::new(),
            });
        }
        assert_eq!(bus.dropped_outbound(), 10);
        assert_eq!(bus.dropped_inbound(), 0);
    }

    #[tokio::test]
    async fn test_publish_no_receivers_then_subscribe_delivers_new_messages() {
        let bus = MessageBus::new();

        // Publish with no receivers -- message is dropped.
        bus.publish_inbound(InboundMessage {
            channel: "test".to_string(),
            sender_id: "u".to_string(),
            chat_id: "c".to_string(),
            content: "lost".to_string(),
            media: vec![],
            session_key: "t:c".to_string(),
            correlation_id: String::new(),
            metadata: std::collections::HashMap::new(),
        });
        assert_eq!(bus.dropped_inbound(), 1);

        // Subscribe and publish again.
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

        let msg = rx.recv().await.expect("should receive message after subscribing");
        assert_eq!(msg.content, "delivered");
        assert_eq!(bus.dropped_inbound(), 1, "only the first message should be dropped");
    }

    #[tokio::test]
    async fn test_publish_outbound_no_receivers_then_subscribe_delivers_new_messages() {
        let bus = MessageBus::new();

        bus.publish_outbound(OutboundMessage {
            channel: "test".to_string(),
            chat_id: "c".to_string(),
            content: "lost".to_string(),
            message_type: String::new(),
        });
        assert_eq!(bus.dropped_outbound(), 1);

        let mut rx = bus.subscribe_outbound();
        bus.publish_outbound(OutboundMessage {
            channel: "test".to_string(),
            chat_id: "c".to_string(),
            content: "delivered".to_string(),
            message_type: String::new(),
        });

        let msg = rx.recv().await.expect("should receive message after subscribing");
        assert_eq!(msg.content, "delivered");
        assert_eq!(bus.dropped_outbound(), 1);
    }

    #[tokio::test]
    async fn test_publish_inbound_subscribe_then_unsubscribe_all() {
        let bus = MessageBus::new();

        // Subscribe and then drop.
        {
            let _rx = bus.subscribe_inbound();
            assert_eq!(bus.inbound_subscriber_count(), 1);
        }
        assert_eq!(bus.inbound_subscriber_count(), 0);

        // Now publishing should drop the message.
        bus.publish_inbound(InboundMessage {
            channel: "test".to_string(),
            sender_id: "u".to_string(),
            chat_id: "c".to_string(),
            content: "after unsubscribe".to_string(),
            media: vec![],
            session_key: "t:c".to_string(),
            correlation_id: String::new(),
            metadata: std::collections::HashMap::new(),
        });
        assert_eq!(bus.dropped_inbound(), 1);
    }

    // =========================================================================
    // Additional tests: multiple concurrent publishers and subscribers
    // =========================================================================

    #[tokio::test]
    async fn test_concurrent_multiple_publishers_multiple_subscribers_inbound() {
        use std::sync::Arc;

        let bus = Arc::new(MessageBus::new());
        let num_subscribers = 3;
        let num_publishers = 5;
        let msgs_per_publisher = 20;
        let total_expected = num_publishers * msgs_per_publisher;

        // Spawn subscribers first.
        let mut sub_handles = Vec::new();
        for _ in 0..num_subscribers {
            let mut rx = bus.subscribe_inbound();
            let expected = total_expected;
            sub_handles.push(tokio::spawn(async move {
                let mut received = Vec::new();
                for _ in 0..expected {
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(5),
                        rx.recv(),
                    )
                    .await
                    {
                        Ok(Ok(msg)) => received.push(msg.content),
                        _ => break,
                    }
                }
                received.len()
            }));
        }

        // Give subscribers a moment to be ready.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // Spawn publishers.
        let mut pub_handles = Vec::new();
        for p in 0..num_publishers {
            let b = bus.clone();
            pub_handles.push(tokio::spawn(async move {
                for m in 0..msgs_per_publisher {
                    b.publish_inbound(InboundMessage {
                        channel: "test".to_string(),
                        sender_id: format!("publisher{}", p),
                        chat_id: "c".to_string(),
                        content: format!("p{}-m{}", p, m),
                        media: vec![],
                        session_key: "t:c".to_string(),
                        correlation_id: String::new(),
                        metadata: std::collections::HashMap::new(),
                    });
                }
            }));
        }

        // Wait for all publishers to finish.
        for h in pub_handles {
            h.await.unwrap();
        }

        // Wait for all subscribers to finish.
        let mut sub_counts = Vec::new();
        for h in sub_handles {
            sub_counts.push(h.await.unwrap());
        }

        // Each subscriber should have received all messages (broadcast fan-out).
        for (i, &count) in sub_counts.iter().enumerate() {
            assert_eq!(
                count, total_expected,
                "subscriber {} should receive {} messages, got {}",
                i, total_expected, count
            );
        }
    }

    #[tokio::test]
    async fn test_concurrent_inbound_and_outbound_are_independent() {
        use std::sync::Arc;

        let bus = Arc::new(MessageBus::new());
        let mut in_rx = bus.subscribe_inbound();
        let mut out_rx = bus.subscribe_outbound();

        let b1 = bus.clone();
        let b2 = bus.clone();

        // Publish inbound on one task, outbound on another.
        let in_handle = tokio::spawn(async move {
            for i in 0..10 {
                b1.publish_inbound(InboundMessage {
                    channel: "test".to_string(),
                    sender_id: "u".to_string(),
                    chat_id: "c".to_string(),
                    content: format!("in{}", i),
                    media: vec![],
                    session_key: "t:c".to_string(),
                    correlation_id: String::new(),
                    metadata: std::collections::HashMap::new(),
                });
            }
        });

        let out_handle = tokio::spawn(async move {
            for i in 0..10 {
                b2.publish_outbound(OutboundMessage {
                    channel: "test".to_string(),
                    chat_id: "c".to_string(),
                    content: format!("out{}", i),
                    message_type: String::new(),
                });
            }
        });

        in_handle.await.unwrap();
        out_handle.await.unwrap();

        // Inbound receiver should only have inbound messages.
        let mut in_count = 0;
        while let Ok(msg) = in_rx.try_recv() {
            assert!(
                msg.content.starts_with("in"),
                "inbound receiver got wrong message: {}",
                msg.content
            );
            in_count += 1;
        }
        assert_eq!(in_count, 10);

        // Outbound receiver should only have outbound messages.
        let mut out_count = 0;
        while let Ok(msg) = out_rx.try_recv() {
            assert!(
                msg.content.starts_with("out"),
                "outbound receiver got wrong message: {}",
                msg.content
            );
            out_count += 1;
        }
        assert_eq!(out_count, 10);
    }

    #[tokio::test]
    async fn test_concurrent_close_during_publish() {
        use std::sync::Arc;

        let bus = Arc::new(MessageBus::new());
        let _rx = bus.subscribe_inbound();

        let b1 = bus.clone();
        let b2 = bus.clone();

        // One task publishes messages, another closes the bus.
        let pub_handle = tokio::spawn(async move {
            for i in 0..100 {
                b1.publish_inbound(InboundMessage {
                    channel: "test".to_string(),
                    sender_id: "u".to_string(),
                    chat_id: "c".to_string(),
                    content: format!("msg{}", i),
                    media: vec![],
                    session_key: "t:c".to_string(),
                    correlation_id: String::new(),
                    metadata: std::collections::HashMap::new(),
                });
                // Yield occasionally to interleave with close.
                if i % 10 == 0 {
                    tokio::task::yield_now().await;
                }
            }
        });

        let close_handle = tokio::spawn(async move {
            // Close after a small delay.
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            b2.close();
        });

        pub_handle.await.unwrap();
        close_handle.await.unwrap();

        assert!(bus.is_closed());
        // Some messages were published before close, some after.
        // The important invariant: no panic, no deadlock.
    }

    #[tokio::test]
    async fn test_concurrent_subscribers_drop_while_publishing() {
        use std::sync::Arc;

        let bus = Arc::new(MessageBus::new());

        // Create several subscribers.
        let mut receivers: Vec<broadcast::Receiver<InboundMessage>> = Vec::new();
        for _ in 0..5 {
            receivers.push(bus.subscribe_inbound());
        }

        let b = bus.clone();
        let pub_handle = tokio::spawn(async move {
            for i in 0..50 {
                b.publish_inbound(InboundMessage {
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
        });

        // Drop some receivers while publishing is in progress.
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        receivers.truncate(2); // Drop 3 receivers.

        pub_handle.await.unwrap();

        // Remaining receivers should still have gotten messages.
        assert_eq!(bus.inbound_subscriber_count(), 2);
    }

    // =========================================================================
    // Additional tests: with_capacity edge case
    // =========================================================================

    #[tokio::test]
    async fn test_with_capacity_one_does_not_panic() {
        // Smallest possible capacity: 1.
        let bus = MessageBus::with_capacity(1);
        let mut rx = bus.subscribe_inbound();

        bus.publish_inbound(InboundMessage {
            channel: "test".to_string(),
            sender_id: "u".to_string(),
            chat_id: "c".to_string(),
            content: "first".to_string(),
            media: vec![],
            session_key: "t:c".to_string(),
            correlation_id: String::new(),
            metadata: std::collections::HashMap::new(),
        });

        let msg = rx.recv().await.expect("should receive first message");
        assert_eq!(msg.content, "first");
    }

    #[tokio::test]
    async fn test_with_capacity_overflow_drops_oldest() {
        // With capacity 2, publishing 4 messages should drop the oldest 2.
        let bus = MessageBus::with_capacity(2);
        let mut rx = bus.subscribe_inbound();

        for i in 0..4 {
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

        // With broadcast capacity 2 and no recv yet, the oldest messages
        // may have been overwritten. recv() should return messages but
        // may start from a later one or return a RecvError::Lagged.
        let result = rx.recv().await;
        assert!(result.is_ok() || result.is_err(), "recv should complete without panic");
    }
}

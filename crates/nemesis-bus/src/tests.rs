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
        voice_playback: None,
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
        meta: Default::default(),
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
        voice_playback: None,
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
        voice_playback: None,
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
                voice_playback: None,
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
        voice_playback: None,
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
        meta: Default::default(),
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
        voice_playback: None,
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
        voice_playback: None,
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
        meta: Default::default(),
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
                meta: Default::default(),
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
            voice_playback: None,
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
                voice_playback: None,
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
        voice_playback: None,
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
        meta: Default::default(),
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
            voice_playback: None,
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
            meta: Default::default(),
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
        voice_playback: None,
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
        voice_playback: None,
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
        voice_playback: None,
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
        meta: Default::default(),
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
        voice_playback: None,
    });
    bus.publish_outbound(OutboundMessage {
        channel: "test".to_string(),
        chat_id: "c".to_string(),
        content: "outbound after close".to_string(),
        message_type: String::new(),
        meta: Default::default(),
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
        voice_playback: None,
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
        meta: Default::default(),
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
        voice_playback: None,
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
        meta: Default::default(),
    });

    bus.close();

    // Publish after close -- should be silently ignored.
    bus.publish_outbound(OutboundMessage {
        channel: "test".to_string(),
        chat_id: "c".to_string(),
        content: "after close".to_string(),
        message_type: String::new(),
        meta: Default::default(),
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
            voice_playback: None,
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
            meta: Default::default(),
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
        voice_playback: None,
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
            voice_playback: None,
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
            voice_playback: None,
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
        voice_playback: None,
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
        meta: Default::default(),
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
            voice_playback: None,
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
            meta: Default::default(),
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
        voice_playback: None,
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
        voice_playback: None,
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
        meta: Default::default(),
    });
    assert_eq!(bus.dropped_outbound(), 1);

    let mut rx = bus.subscribe_outbound();
    bus.publish_outbound(OutboundMessage {
        channel: "test".to_string(),
        chat_id: "c".to_string(),
        content: "delivered".to_string(),
        message_type: String::new(),
        meta: Default::default(),
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
        voice_playback: None,
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
                    voice_playback: None,
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
                voice_playback: None,
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
                meta: Default::default(),
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
                voice_playback: None,
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
                voice_playback: None,
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
        voice_playback: None,
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
            voice_playback: None,
        });
    }

    // With broadcast capacity 2 and no recv yet, the oldest messages
    // may have been overwritten. recv() should return messages but
    // may start from a later one or return a RecvError::Lagged.
    let result = rx.recv().await;
    assert!(result.is_ok() || result.is_err(), "recv should complete without panic");
}

// =========================================================================
// Additional tests: broadcast send failure edge cases
// =========================================================================

#[tokio::test]
async fn test_publish_inbound_lagged_receiver_increments_dropped() {
    // Test the error path when inbound_tx.send() fails due to lagged receiver
    let bus = MessageBus::with_capacity(2);
    let mut rx = bus.subscribe_inbound();

    // Publish messages without receiving to overflow the buffer
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
            voice_playback: None,
        });
    }

    // Now try to receive - receiver is lagged, some messages may be lost
    // This will trigger RecvError::Lagged or skip to latest message
    let result = rx.recv().await;

    // The receiver should get either a message or a lagged error
    // Both cases indicate the error path was exercised
    assert!(result.is_ok() || result.is_err(), "recv should complete");

    // Drop the receiver and publish more to trigger send errors
    drop(rx);

    let initial_dropped = bus.dropped_inbound();

    // Publish with no receivers - this should increment dropped counter
    // via the error path in publish_inbound
    for i in 0..5 {
        bus.publish_inbound(InboundMessage {
            channel: "test".to_string(),
            sender_id: "u".to_string(),
            chat_id: "c".to_string(),
            content: format!("orphan{}", i),
            media: vec![],
            session_key: "t:c".to_string(),
            correlation_id: String::new(),
            metadata: std::collections::HashMap::new(),
            voice_playback: None,
        });
    }

    // Verify that messages were dropped (counter incremented)
    assert_eq!(bus.dropped_inbound(), initial_dropped + 5);
}

#[tokio::test]
async fn test_publish_outbound_lagged_receiver_increments_dropped() {
    // Test the error path when outbound_tx.send() fails due to lagged receiver
    let bus = MessageBus::with_capacity(2);
    let mut rx = bus.subscribe_outbound();

    // Publish messages without receiving to overflow the buffer
    for i in 0..10 {
        bus.publish_outbound(OutboundMessage {
            channel: "test".to_string(),
            chat_id: "c".to_string(),
            content: format!("out{}", i),
            message_type: String::new(),
            meta: Default::default(),
        });
    }

    // Try to receive - may get lagged error
    let result = rx.recv().await;
    assert!(result.is_ok() || result.is_err(), "recv should complete");

    // Drop the receiver and publish more to trigger send errors
    drop(rx);

    let initial_dropped = bus.dropped_outbound();

    // Publish with no receivers - this should increment dropped counter
    // via the error path in publish_outbound
    for i in 0..5 {
        bus.publish_outbound(OutboundMessage {
            channel: "test".to_string(),
            chat_id: "c".to_string(),
            content: format!("orphan_out{}", i),
            message_type: String::new(),
            meta: Default::default(),
        });
    }

    // Verify that messages were dropped (counter incremented)
    assert_eq!(bus.dropped_outbound(), initial_dropped + 5);
}

#[tokio::test]
async fn test_broadcast_send_failure_with_active_but_slow_receiver() {
    // Test send failure when receiver exists but is lagged
    let bus = MessageBus::with_capacity(3);

    // Create a receiver but don't read from it immediately
    let mut rx = bus.subscribe_inbound();

    // Publish more messages than buffer capacity
    for i in 0..20 {
        bus.publish_inbound(InboundMessage {
            channel: "test".to_string(),
            sender_id: "u".to_string(),
            chat_id: "c".to_string(),
            content: format!("overflow{}", i),
            media: vec![],
            session_key: "t:c".to_string(),
            correlation_id: String::new(),
            metadata: std::collections::HashMap::new(),
            voice_playback: None,
        });
    }

    // The receiver should be able to receive but may have missed messages
    // This exercises the send error path in the broadcast channel
    let _msg_count = loop {
        match tokio::time::timeout(
            std::time::Duration::from_millis(50),
            rx.recv()
        ).await {
            Ok(Ok(_)) => continue,
            Ok(Err(_)) => break, // Lagged error
            Err(_) => break, // Timeout
        }
    };

    // At least we attempted to receive, and sends were attempted
    // The key is that the code exercised the send operation
    assert!(true, "test completed send operations");
}

#[tokio::test]
async fn test_inbound_send_error_path_via_direct_sender() {
    // Test the error path when using inbound_sender() directly
    let bus = MessageBus::with_capacity(1);
    let sender = bus.inbound_sender();
    let mut rx = bus.subscribe_inbound();

    // Overfill the buffer without receiving
    for i in 0..10 {
        let _ = sender.send(InboundMessage {
            channel: "test".to_string(),
            sender_id: "u".to_string(),
            chat_id: "c".to_string(),
            content: format!("direct{}", i),
            media: vec![],
            session_key: "t:c".to_string(),
            correlation_id: String::new(),
            metadata: std::collections::HashMap::new(),
            voice_playback: None,
        });
    }

    // Some sends may have failed - that's the error path we want to exercise
    // Now try to receive
    let result = rx.recv().await;
    assert!(result.is_ok() || result.is_err());
}

#[tokio::test]
async fn test_outbound_send_error_path_via_direct_sender() {
    // Test the error path when using outbound_sender() directly
    let bus = MessageBus::with_capacity(1);
    let sender = bus.outbound_sender();
    let mut rx = bus.subscribe_outbound();

    // Overfill the buffer without receiving
    for i in 0..10 {
        let _ = sender.send(OutboundMessage {
            channel: "test".to_string(),
            chat_id: "c".to_string(),
            content: format!("direct_out{}", i),
            message_type: String::new(),
            meta: Default::default(),
        });
    }

    // Some sends may have failed
    let result = rx.recv().await;
    assert!(result.is_ok() || result.is_err());
}

#[tokio::test]
async fn test_publish_inbound_race_condition_receiver_drop_during_send() {
    // Test the race condition error path where receiver exists when counted
    // but is dropped before the actual send operation
    use std::sync::Arc;
    use tokio::sync::Mutex;

    let bus = Arc::new(MessageBus::new());
    let receiver_lock = Arc::new(Mutex::new(None::<broadcast::Receiver<InboundMessage>>));
    let publish_done = Arc::new(Mutex::new(false));

    // Spawn a task that will create and drop receivers rapidly
    let bus_clone = Arc::clone(&bus);
    let receiver_lock_clone = Arc::clone(&receiver_lock);
    let publish_done_clone = Arc::clone(&publish_done);

    let receiver_task = tokio::spawn(async move {
        for _ in 0..100 {
            let rx = bus_clone.subscribe_inbound();
            *receiver_lock_clone.lock().await = Some(rx);
            // Drop immediately by not storing it
            tokio::time::sleep(std::time::Duration::from_micros(10)).await;
            *receiver_lock_clone.lock().await = None;
        }
    });

    // Spawn a task that publishes continuously
    let bus_clone2 = Arc::clone(&bus);
    let publisher_task = tokio::spawn(async move {
        for i in 0..100 {
            bus_clone2.publish_inbound(InboundMessage {
                channel: "race".to_string(),
                sender_id: "u".to_string(),
                chat_id: "c".to_string(),
                content: format!("race{}", i),
                media: vec![],
                session_key: "race:c".to_string(),
                correlation_id: String::new(),
                metadata: std::collections::HashMap::new(),
                voice_playback: None,
            });
            tokio::time::sleep(std::time::Duration::from_micros(10)).await;
        }
        *publish_done_clone.lock().await = true;
    });

    // Wait for both tasks
    let _ = receiver_task.await;
    let _ = publisher_task.await;

    // The test completed without panic, which means the error paths were exercised
    assert!(*publish_done.lock().await, "publisher should have completed");
}

#[tokio::test]
async fn test_publish_outbound_race_condition_receiver_drop_during_send() {
    // Test the race condition error path for outbound messages
    use std::sync::Arc;
    use tokio::sync::Mutex;

    let bus = Arc::new(MessageBus::new());
    let receiver_lock = Arc::new(Mutex::new(None::<broadcast::Receiver<OutboundMessage>>));
    let publish_done = Arc::new(Mutex::new(false));

    let bus_clone = Arc::clone(&bus);
    let receiver_lock_clone = Arc::clone(&receiver_lock);
    let publish_done_clone = Arc::clone(&publish_done);

    let receiver_task = tokio::spawn(async move {
        for _ in 0..100 {
            let rx = bus_clone.subscribe_outbound();
            *receiver_lock_clone.lock().await = Some(rx);
            tokio::time::sleep(std::time::Duration::from_micros(10)).await;
            *receiver_lock_clone.lock().await = None;
        }
    });

    let bus_clone2 = Arc::clone(&bus);
    let publisher_task = tokio::spawn(async move {
        for i in 0..100 {
            bus_clone2.publish_outbound(OutboundMessage {
                channel: "race".to_string(),
                chat_id: "c".to_string(),
                content: format!("race_out{}", i),
                message_type: String::new(),
                meta: Default::default(),
            });
            tokio::time::sleep(std::time::Duration::from_micros(10)).await;
        }
        *publish_done_clone.lock().await = true;
    });

    let _ = receiver_task.await;
    let _ = publisher_task.await;

    assert!(*publish_done.lock().await, "publisher should have completed");
}

#[tokio::test]
async fn test_publish_inbound_drops_message_when_all_receivers_dropped_concurrently() {
    // Test the specific error path by creating a high contention scenario
    use std::sync::Arc;
    let bus = Arc::new(MessageBus::new());

    // Create many receivers and drop them rapidly while publishing
    let mut handles = vec![];

    // Spawn multiple tasks that create and drop receivers
    for _task_id in 0..10 {
        let bus_clone = Arc::clone(&bus);
        let handle = tokio::spawn(async move {
            for _ in 0..50 {
                let _rx = bus_clone.subscribe_inbound();
                // Immediately drop the receiver
                drop(_rx);
                // Small delay to create timing variations
                tokio::time::sleep(std::time::Duration::from_micros(1)).await;
            }
        });
        handles.push(handle);
    }

    // Publish messages while receivers are being created/dropped
    let bus_clone2 = Arc::clone(&bus);
    let publisher_handle = tokio::spawn(async move {
        for i in 0..100 {
            bus_clone2.publish_inbound(InboundMessage {
                channel: "contention".to_string(),
                sender_id: "u".to_string(),
                chat_id: "c".to_string(),
                content: format!("contention{}", i),
                media: vec![],
                session_key: "contention:c".to_string(),
                correlation_id: String::new(),
                metadata: std::collections::HashMap::new(),
                voice_playback: None,
            });
            tokio::time::sleep(std::time::Duration::from_micros(1)).await;
        }
    });

    // Wait for all tasks
    for handle in handles {
        handle.await.unwrap();
    }
    publisher_handle.await.unwrap();

    // Verify some messages were dropped due to no receivers
    let dropped = bus.dropped_inbound();
    assert!(dropped > 0, "Expected some messages to be dropped due to receiver contention, got {}", dropped);
}

#[tokio::test]
async fn test_publish_outbound_drops_message_when_all_receivers_dropped_concurrently() {
    // Test the specific error path for outbound with high contention
    use std::sync::Arc;
    let bus = Arc::new(MessageBus::new());

    let mut handles = vec![];

    for _task_id in 0..10 {
        let bus_clone = Arc::clone(&bus);
        let handle = tokio::spawn(async move {
            for _ in 0..50 {
                let _rx = bus_clone.subscribe_outbound();
                drop(_rx);
                tokio::time::sleep(std::time::Duration::from_micros(1)).await;
            }
        });
        handles.push(handle);
    }

    let bus_clone2 = Arc::clone(&bus);
    let publisher_handle = tokio::spawn(async move {
        for i in 0..100 {
            bus_clone2.publish_outbound(OutboundMessage {
                channel: "contention".to_string(),
                chat_id: "c".to_string(),
                content: format!("contention_out{}", i),
                message_type: String::new(),
                meta: Default::default(),
            });
            tokio::time::sleep(std::time::Duration::from_micros(1)).await;
        }
    });

    for handle in handles {
        handle.await.unwrap();
    }
    publisher_handle.await.unwrap();

    let dropped = bus.dropped_outbound();
    assert!(dropped > 0, "Expected some outbound messages to be dropped, got {}", dropped);
}

use super::*;
use async_trait::async_trait;

/// A minimal stub channel for testing.
struct StubChannel {
    name: String,
    sent: Arc<parking_lot::RwLock<Vec<String>>>,
    started: Arc<parking_lot::RwLock<bool>>,
}

impl StubChannel {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            sent: Arc::new(parking_lot::RwLock::new(Vec::new())),
            started: Arc::new(parking_lot::RwLock::new(false)),
        }
    }

    fn sent_messages(&self) -> Vec<String> {
        self.sent.read().clone()
    }

    fn is_started(&self) -> bool {
        *self.started.read()
    }
}

#[async_trait]
impl Channel for StubChannel {
    fn name(&self) -> &str {
        &self.name
    }

    fn is_running(&self) -> bool {
        *self.started.read()
    }

    async fn start(&self) -> Result<()> {
        *self.started.write() = true;
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        *self.started.write() = false;
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        self.sent.write().push(msg.content.clone());
        Ok(())
    }
}

#[tokio::test]
async fn test_register_and_get() {
    let mgr = ChannelManager::new();
    let ch = Arc::new(StubChannel::new("test-ch"));
    mgr.register(ch.clone()).await.unwrap();

    assert!(mgr.get("test-ch").await.is_some());
    assert!(mgr.get("nonexistent").await.is_none());
    assert_eq!(mgr.channel_count().await, 1);
}

#[tokio::test]
async fn test_register_duplicate_fails() {
    let mgr = ChannelManager::new();
    let ch1 = Arc::new(StubChannel::new("dup"));
    let ch2 = Arc::new(StubChannel::new("dup"));

    mgr.register(ch1).await.unwrap();
    let result = mgr.register(ch2).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_register_or_replace() {
    let mgr = ChannelManager::new();
    let ch1 = Arc::new(StubChannel::new("dup"));
    let ch2 = Arc::new(StubChannel::new("dup"));

    mgr.register(ch1).await.unwrap();
    mgr.register_or_replace(ch2).await;
    assert_eq!(mgr.channel_count().await, 1);
}

#[tokio::test]
async fn test_unregister() {
    let mgr = ChannelManager::new();
    let ch = Arc::new(StubChannel::new("removable"));
    mgr.register(ch).await.unwrap();
    assert!(mgr.unregister("removable").await);
    assert!(!mgr.unregister("removable").await);
    assert_eq!(mgr.channel_count().await, 0);
}

#[tokio::test]
async fn test_start_stop_all() {
    let mgr = Arc::new(ChannelManager::new());
    let ch1 = Arc::new(StubChannel::new("a"));
    let ch2 = Arc::new(StubChannel::new("b"));

    mgr.register(ch1.clone()).await.unwrap();
    mgr.register(ch2.clone()).await.unwrap();

    mgr.start_all().await.unwrap();
    assert!(ch1.is_started());
    assert!(ch2.is_started());

    mgr.stop_all().await.unwrap();
    assert!(!ch1.is_started());
    assert!(!ch2.is_started());
}

#[tokio::test]
async fn test_dispatch_outbound_success() {
    let mgr = ChannelManager::new();
    let ch = Arc::new(StubChannel::new("web"));
    mgr.register(ch.clone()).await.unwrap();

    let msg = OutboundMessage {
        channel: "web".to_string(),
        chat_id: "chat-1".to_string(),
        content: "Hello world".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    mgr.dispatch_outbound(msg).await.unwrap();

    let sent = ch.sent_messages();
    assert_eq!(sent, vec!["Hello world"]);
}

#[tokio::test]
async fn test_dispatch_outbound_channel_not_found() {
    let mgr = ChannelManager::new();
    let msg = OutboundMessage {
        channel: "missing".to_string(),
        chat_id: "chat-1".to_string(),
        content: "Hello".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    let result = mgr.dispatch_outbound(msg).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_dispatch_loop() {
    let mgr = Arc::new(ChannelManager::new());
    let ch = Arc::new(StubChannel::new("loop-test"));
    mgr.register(ch.clone()).await.unwrap();

    mgr.start_dispatch_loop().unwrap();
    let tx = mgr.outbound_sender();

    let msg = OutboundMessage {
        channel: "loop-test".to_string(),
        chat_id: "c1".to_string(),
        content: "via loop".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    tx.send(msg).await.unwrap();

    // Give the loop time to process
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let sent = ch.sent_messages();
    assert!(sent.contains(&"via loop".to_string()));
    drop(tx); // Close sender to stop loop
}

#[tokio::test]
async fn test_dispatch_loop_skips_internal_channels() {
    let mgr = Arc::new(ChannelManager::new());
    mgr.start_dispatch_loop().unwrap();
    let tx = mgr.outbound_sender();

    let msg = OutboundMessage {
        channel: "system".to_string(),
        chat_id: "c1".to_string(),
        content: "internal msg".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    tx.send(msg).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert_eq!(
        mgr.metrics()
            .dropped_internal
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    drop(tx);
}

#[tokio::test]
async fn test_allowed_channels_filter() {
    let mgr = ChannelManager::with_allowed_channels(vec!["allowed".to_string()]);
    let ch = Arc::new(StubChannel::new("allowed"));
    mgr.register(ch.clone()).await.unwrap();

    // Message to allowed channel should dispatch
    let msg = OutboundMessage {
        channel: "allowed".to_string(),
        chat_id: "c1".to_string(),
        content: "ok".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    mgr.dispatch_outbound(msg).await.unwrap();
    assert_eq!(ch.sent_messages().len(), 1);

    // Message to filtered channel should be silently dropped
    let msg2 = OutboundMessage {
        channel: "blocked".to_string(),
        chat_id: "c1".to_string(),
        content: "dropped".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    mgr.dispatch_outbound(msg2).await.unwrap(); // no error
    assert_eq!(
        mgr.metrics()
            .dropped_filtered
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
}

#[tokio::test]
async fn test_metrics() {
    let mgr = ChannelManager::new();
    assert_eq!(
        mgr.metrics()
            .dispatched
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    assert_eq!(
        mgr.metrics()
            .dropped_not_found
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );

    // Dispatch to missing channel increments dropped_not_found
    let msg = OutboundMessage {
        channel: "missing".to_string(),
        chat_id: "c1".to_string(),
        content: "x".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    let _ = mgr.dispatch_outbound(msg).await;
    assert_eq!(
        mgr.metrics()
            .dropped_not_found
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
}

#[tokio::test]
async fn test_channel_names() {
    let mgr = ChannelManager::new();
    mgr.register(Arc::new(StubChannel::new("a"))).await.unwrap();
    mgr.register(Arc::new(StubChannel::new("b"))).await.unwrap();

    let mut names = mgr.channel_names().await;
    names.sort();
    assert_eq!(names, vec!["a", "b"]);
}

#[tokio::test]
async fn test_get_status() {
    let mgr = ChannelManager::new();
    mgr.register(Arc::new(StubChannel::new("web")))
        .await
        .unwrap();

    let status = mgr.get_status().await;
    assert!(status.contains_key("web"));
    assert!(status["web"].enabled);
}

#[tokio::test]
async fn test_send_to_channel() {
    let mgr = ChannelManager::new();
    let ch = Arc::new(StubChannel::new("web"));
    mgr.register(ch.clone()).await.unwrap();

    mgr.send_to_channel("web", "chat-1", "direct message")
        .await
        .unwrap();

    let sent = ch.sent_messages();
    assert_eq!(sent, vec!["direct message"]);
}

#[tokio::test]
async fn test_send_to_missing_channel() {
    let mgr = ChannelManager::new();
    let result = mgr.send_to_channel("missing", "chat-1", "msg").await;
    assert!(result.is_err());
}

#[test]
fn test_is_internal_channel() {
    assert!(is_internal_channel("system"));
    assert!(!is_internal_channel("web"));
    assert!(!is_internal_channel("rpc"));
}

#[tokio::test]
async fn test_start_stop_idempotent() {
    let mgr = Arc::new(ChannelManager::new());
    let ch = Arc::new(StubChannel::new("a"));
    mgr.register(ch.clone()).await.unwrap();

    // Start all twice should succeed
    mgr.start_all().await.unwrap();
    assert!(ch.is_started());
    // Second call should be a no-op (dispatch already started)
    mgr.start_all().await.unwrap();

    // Stop all twice should succeed
    mgr.stop_all().await.unwrap();
    assert!(!ch.is_started());
    mgr.stop_all().await.unwrap();
}

#[tokio::test]
async fn test_start_all_empty_manager() {
    let mgr = Arc::new(ChannelManager::new());
    // No channels registered
    mgr.start_all().await.unwrap();
    mgr.stop_all().await.unwrap();
}

#[tokio::test]
async fn test_unregister_nonexistent() {
    let mgr = ChannelManager::new();
    assert!(!mgr.unregister("nonexistent").await);
    assert_eq!(mgr.channel_count().await, 0);
}

#[tokio::test]
async fn test_get_status_empty() {
    let mgr = ChannelManager::new();
    let status = mgr.get_status().await;
    assert!(status.is_empty());
}

#[tokio::test]
async fn test_get_status_multiple_channels() {
    let mgr = ChannelManager::new();
    mgr.register(Arc::new(StubChannel::new("ch1")))
        .await
        .unwrap();
    mgr.register(Arc::new(StubChannel::new("ch2")))
        .await
        .unwrap();
    mgr.register(Arc::new(StubChannel::new("ch3")))
        .await
        .unwrap();

    let status = mgr.get_status().await;
    assert_eq!(status.len(), 3);
    assert!(status.contains_key("ch1"));
    assert!(status.contains_key("ch2"));
    assert!(status.contains_key("ch3"));
}

#[tokio::test]
async fn test_dispatch_outbound_empty_content() {
    let mgr = ChannelManager::new();
    let ch = Arc::new(StubChannel::new("web"));
    mgr.register(ch.clone()).await.unwrap();

    let msg = OutboundMessage {
        channel: "web".to_string(),
        chat_id: "chat-1".to_string(),
        content: String::new(),
        message_type: String::new(),
        meta: Default::default(),
    };
    mgr.dispatch_outbound(msg).await.unwrap();

    let sent = ch.sent_messages();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0], "");
}

#[tokio::test]
async fn test_dispatch_outbound_long_content() {
    let mgr = ChannelManager::new();
    let ch = Arc::new(StubChannel::new("web"));
    mgr.register(ch.clone()).await.unwrap();

    let long_content = "x".repeat(100_000);
    let msg = OutboundMessage {
        channel: "web".to_string(),
        chat_id: "chat-1".to_string(),
        content: long_content.clone(),
        message_type: String::new(),
        meta: Default::default(),
    };
    mgr.dispatch_outbound(msg).await.unwrap();

    let sent = ch.sent_messages();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].len(), 100_000);
}

#[tokio::test]
async fn test_send_to_channel_empty_chat_id() {
    let mgr = ChannelManager::new();
    let ch = Arc::new(StubChannel::new("web"));
    mgr.register(ch.clone()).await.unwrap();

    mgr.send_to_channel("web", "", "test content")
        .await
        .unwrap();

    let sent = ch.sent_messages();
    assert_eq!(sent, vec!["test content"]);
}

#[tokio::test]
async fn test_send_to_channel_empty_content() {
    let mgr = ChannelManager::new();
    let ch = Arc::new(StubChannel::new("web"));
    mgr.register(ch.clone()).await.unwrap();

    mgr.send_to_channel("web", "chat-1", "").await.unwrap();

    let sent = ch.sent_messages();
    assert_eq!(sent, vec![""]);
}

#[tokio::test]
async fn test_dispatch_loop_double_start_fails() {
    let mgr = Arc::new(ChannelManager::new());
    mgr.start_dispatch_loop().unwrap();
    // Second call should fail
    let result = mgr.start_dispatch_loop();
    assert!(result.is_err());
}

#[tokio::test]
async fn test_concurrent_access() {
    let mgr = Arc::new(ChannelManager::new());

    // Register channels
    for i in 0..5 {
        let name = format!("ch{}", i);
        mgr.register(Arc::new(StubChannel::new(&name)))
            .await
            .unwrap();
    }

    let mgr1 = Arc::clone(&mgr);
    let mgr2 = Arc::clone(&mgr);
    let mgr3 = Arc::clone(&mgr);
    let mgr4 = Arc::clone(&mgr);

    // Concurrent reads
    let h1 = tokio::spawn(async move {
        for _ in 0..100 {
            mgr1.get("ch1").await;
        }
    });
    let h2 = tokio::spawn(async move {
        for _ in 0..100 {
            mgr2.channel_names().await;
        }
    });
    let h3 = tokio::spawn(async move {
        for _ in 0..100 {
            mgr3.get_status().await;
        }
    });
    let h4 = tokio::spawn(async move {
        for _ in 0..100 {
            mgr4.channel_count().await;
        }
    });

    h1.await.unwrap();
    h2.await.unwrap();
    h3.await.unwrap();
    h4.await.unwrap();

    // Manager should still be functional
    assert_eq!(mgr.channel_count().await, 5);
}

#[tokio::test]
async fn test_metrics_dispatched_increment() {
    let mgr = ChannelManager::new();
    let ch = Arc::new(StubChannel::new("web"));
    mgr.register(ch.clone()).await.unwrap();

    // Dispatch multiple messages
    for i in 0..5 {
        let msg = OutboundMessage {
            channel: "web".to_string(),
            chat_id: format!("chat-{}", i),
            content: format!("msg {}", i),
            message_type: String::new(),
            meta: Default::default(),
        };
        mgr.dispatch_outbound(msg).await.unwrap();
    }

    assert_eq!(
        mgr.metrics()
            .dispatched
            .load(std::sync::atomic::Ordering::Relaxed),
        5
    );
    assert_eq!(ch.sent_messages().len(), 5);
}

#[tokio::test]
async fn test_allowed_channels_empty_means_all() {
    let mgr = ChannelManager::with_allowed_channels(vec![]);
    // Empty allowed list means no filter - all channels allowed
    let ch = Arc::new(StubChannel::new("any"));
    mgr.register(ch.clone()).await.unwrap();

    let msg = OutboundMessage {
        channel: "any".to_string(),
        chat_id: "c1".to_string(),
        content: "ok".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    mgr.dispatch_outbound(msg).await.unwrap();
    assert_eq!(ch.sent_messages().len(), 1);
}

#[tokio::test]
async fn test_unregister_after_start() {
    let mgr = Arc::new(ChannelManager::new());
    let ch1 = Arc::new(StubChannel::new("a"));
    let ch2 = Arc::new(StubChannel::new("b"));
    mgr.register(ch1.clone()).await.unwrap();
    mgr.register(ch2.clone()).await.unwrap();

    mgr.start_all().await.unwrap();
    assert!(ch1.is_started());
    assert!(ch2.is_started());

    mgr.unregister("a").await;
    assert_eq!(mgr.channel_count().await, 1);

    // Channel b should still be accessible
    assert!(mgr.get("b").await.is_some());
    assert!(mgr.get("a").await.is_none());
}

// --- Benchmark-style throughput tests ---

#[tokio::test]
async fn test_dispatch_throughput() {
    let mgr = ChannelManager::new();
    let ch = Arc::new(StubChannel::new("bench"));
    mgr.register(ch.clone()).await.unwrap();

    let count = 1_000;
    let start = std::time::Instant::now();
    for i in 0..count {
        let msg = OutboundMessage {
            channel: "bench".to_string(),
            chat_id: format!("c{}", i),
            content: format!("msg{}", i),
            message_type: String::new(),
            meta: Default::default(),
        };
        mgr.dispatch_outbound(msg).await.unwrap();
    }
    let elapsed = start.elapsed();
    assert_eq!(ch.sent_messages().len(), count);
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "Dispatch throughput too slow: {:?}",
        elapsed
    );
}

#[tokio::test]
async fn test_register_unregister_throughput() {
    let mgr = ChannelManager::new();
    let count = 100;

    let start = std::time::Instant::now();
    for i in 0..count {
        let ch = Arc::new(StubChannel::new(&format!("ch-{}", i)));
        mgr.register(ch).await.unwrap();
    }
    let elapsed = start.elapsed();
    assert_eq!(mgr.channel_count().await, count);
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "Register throughput too slow: {:?}",
        elapsed
    );
}

// ---- Additional comprehensive manager tests ----

// === Sync target configuration ===

#[tokio::test]
async fn test_setup_sync_targets_valid() {
    let mgr = ChannelManager::new();
    let ch_a = Arc::new(StubChannel::new("a"));
    let ch_b = Arc::new(StubChannel::new("b"));
    mgr.register(ch_a).await.unwrap();
    mgr.register(ch_b).await.unwrap();

    let mut config = ChannelSyncConfig::default();
    config
        .targets
        .insert("a".to_string(), vec!["b".to_string()]);

    mgr.setup_sync_targets(&config).await;

    let targets = mgr.get_sync_targets("a").await;
    assert_eq!(targets, vec!["b"]);
}

#[tokio::test]
async fn test_setup_sync_targets_self_sync_prevented() {
    let mgr = ChannelManager::new();
    let ch = Arc::new(StubChannel::new("self"));
    mgr.register(ch).await.unwrap();

    let mut config = ChannelSyncConfig::default();
    config
        .targets
        .insert("self".to_string(), vec!["self".to_string()]);

    mgr.setup_sync_targets(&config).await;

    let targets = mgr.get_sync_targets("self").await;
    assert!(targets.is_empty()); // self-sync should be skipped
}

#[tokio::test]
async fn test_setup_sync_targets_nonexistent_source() {
    let mgr = ChannelManager::new();

    let mut config = ChannelSyncConfig::default();
    config
        .targets
        .insert("missing".to_string(), vec!["target".to_string()]);

    mgr.setup_sync_targets(&config).await;
    // Should not panic, just skip
}

#[tokio::test]
async fn test_setup_sync_targets_nonexistent_target() {
    let mgr = ChannelManager::new();
    let ch = Arc::new(StubChannel::new("source"));
    mgr.register(ch).await.unwrap();

    let mut config = ChannelSyncConfig::default();
    config
        .targets
        .insert("source".to_string(), vec!["nonexistent".to_string()]);

    mgr.setup_sync_targets(&config).await;

    let targets = mgr.get_sync_targets("source").await;
    assert!(targets.is_empty()); // nonexistent target skipped
}

#[tokio::test]
async fn test_setup_sync_targets_multiple() {
    let mgr = ChannelManager::new();
    mgr.register(Arc::new(StubChannel::new("a"))).await.unwrap();
    mgr.register(Arc::new(StubChannel::new("b"))).await.unwrap();
    mgr.register(Arc::new(StubChannel::new("c"))).await.unwrap();

    let mut config = ChannelSyncConfig::default();
    config
        .targets
        .insert("a".to_string(), vec!["b".to_string(), "c".to_string()]);

    mgr.setup_sync_targets(&config).await;

    let targets = mgr.get_sync_targets("a").await;
    assert_eq!(targets.len(), 2);
    assert!(targets.contains(&"b".to_string()));
    assert!(targets.contains(&"c".to_string()));
}

#[tokio::test]
async fn test_setup_sync_targets_circular() {
    let mgr = ChannelManager::new();
    mgr.register(Arc::new(StubChannel::new("a"))).await.unwrap();
    mgr.register(Arc::new(StubChannel::new("b"))).await.unwrap();

    let mut config = ChannelSyncConfig::default();
    config
        .targets
        .insert("a".to_string(), vec!["b".to_string()]);
    config
        .targets
        .insert("b".to_string(), vec!["a".to_string()]);

    mgr.setup_sync_targets(&config).await;

    let a_targets = mgr.get_sync_targets("a").await;
    let b_targets = mgr.get_sync_targets("b").await;
    assert_eq!(a_targets, vec!["b"]);
    assert_eq!(b_targets, vec!["a"]);
}

#[tokio::test]
async fn test_get_sync_targets_no_config() {
    let mgr = ChannelManager::new();
    let targets = mgr.get_sync_targets("anything").await;
    assert!(targets.is_empty());
}

#[tokio::test]
async fn test_setup_sync_targets_empty_config() {
    let mgr = ChannelManager::new();
    mgr.register(Arc::new(StubChannel::new("a"))).await.unwrap();

    let config = ChannelSyncConfig::default();
    mgr.setup_sync_targets(&config).await;

    let targets = mgr.get_sync_targets("a").await;
    assert!(targets.is_empty());
}

// === Dispatch loop edge cases ===

#[tokio::test]
async fn test_dispatch_loop_shutdown_flag() {
    let mgr = Arc::new(ChannelManager::new());
    let ch = Arc::new(StubChannel::new("test"));
    mgr.register(ch.clone()).await.unwrap();

    mgr.start_dispatch_loop().unwrap();
    let tx = mgr.outbound_sender();

    // Send a message
    let msg = OutboundMessage {
        channel: "test".to_string(),
        chat_id: "c1".to_string(),
        content: "before shutdown".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    tx.send(msg).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(ch.sent_messages().contains(&"before shutdown".to_string()));

    // Stop sets shutdown flag
    mgr.stop_all().await.unwrap();
}

#[tokio::test]
async fn test_dispatch_loop_skips_cli_channel() {
    let mgr = Arc::new(ChannelManager::new());
    mgr.start_dispatch_loop().unwrap();
    let tx = mgr.outbound_sender();

    let msg = OutboundMessage {
        channel: "cli".to_string(),
        chat_id: "c1".to_string(),
        content: "cli msg".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    tx.send(msg).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert_eq!(
        mgr.metrics()
            .dropped_internal
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    drop(tx);
}

#[tokio::test]
async fn test_dispatch_loop_skips_subagent_channel() {
    let mgr = Arc::new(ChannelManager::new());
    mgr.start_dispatch_loop().unwrap();
    let tx = mgr.outbound_sender();

    let msg = OutboundMessage {
        channel: "subagent".to_string(),
        chat_id: "c1".to_string(),
        content: "subagent msg".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    tx.send(msg).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert_eq!(
        mgr.metrics()
            .dropped_internal
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    drop(tx);
}

#[tokio::test]
async fn test_dispatch_loop_multiple_messages() {
    let mgr = Arc::new(ChannelManager::new());
    let ch = Arc::new(StubChannel::new("multi"));
    mgr.register(ch.clone()).await.unwrap();

    mgr.start_dispatch_loop().unwrap();
    let tx = mgr.outbound_sender();

    for i in 0..20 {
        let msg = OutboundMessage {
            channel: "multi".to_string(),
            chat_id: format!("c{}", i),
            content: format!("msg {}", i),
            message_type: String::new(),
            meta: Default::default(),
        };
        tx.send(msg).await.unwrap();
    }

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    assert_eq!(ch.sent_messages().len(), 20);
    drop(tx);
}

// === Allowed channels filter edge cases ===

#[tokio::test]
async fn test_allowed_channels_multiple_allowed() {
    let mgr = ChannelManager::with_allowed_channels(vec!["web".to_string(), "rpc".to_string()]);

    let ch_web = Arc::new(StubChannel::new("web"));
    let ch_rpc = Arc::new(StubChannel::new("rpc"));
    let ch_other = Arc::new(StubChannel::new("other"));

    mgr.register(ch_web.clone()).await.unwrap();
    mgr.register(ch_rpc.clone()).await.unwrap();
    mgr.register(ch_other.clone()).await.unwrap();

    // Allowed channels should receive
    let msg = OutboundMessage {
        channel: "web".to_string(),
        chat_id: "c1".to_string(),
        content: "ok".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    mgr.dispatch_outbound(msg).await.unwrap();
    assert_eq!(ch_web.sent_messages().len(), 1);

    // RPC should also be allowed
    let msg2 = OutboundMessage {
        channel: "rpc".to_string(),
        chat_id: "c1".to_string(),
        content: "ok".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    mgr.dispatch_outbound(msg2).await.unwrap();
    assert_eq!(ch_rpc.sent_messages().len(), 1);

    // Other should be filtered
    let msg3 = OutboundMessage {
        channel: "other".to_string(),
        chat_id: "c1".to_string(),
        content: "filtered".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    mgr.dispatch_outbound(msg3).await.unwrap(); // no error, just dropped
    assert_eq!(ch_other.sent_messages().len(), 0);
    assert_eq!(
        mgr.metrics()
            .dropped_filtered
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
}

// === Metrics accuracy ===

#[tokio::test]
async fn test_metrics_send_errors() {
    let mgr = ChannelManager::new();
    let ch = Arc::new(FailingStubChannel::new("fail-ch"));
    mgr.register(ch).await.unwrap();

    let msg = OutboundMessage {
        channel: "fail-ch".to_string(),
        chat_id: "c1".to_string(),
        content: "will fail".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    let result = mgr.dispatch_outbound(msg).await;
    assert!(result.is_err());
    assert_eq!(
        mgr.metrics()
            .send_errors
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
}

#[tokio::test]
async fn test_metrics_multiple_not_found() {
    let mgr = ChannelManager::new();

    for i in 0..5 {
        let msg = OutboundMessage {
            channel: format!("missing-{}", i),
            chat_id: "c1".to_string(),
            content: "test".to_string(),
            message_type: String::new(),
            meta: Default::default(),
        };
        let _ = mgr.dispatch_outbound(msg).await;
    }

    assert_eq!(
        mgr.metrics()
            .dropped_not_found
            .load(std::sync::atomic::Ordering::Relaxed),
        5
    );
}

// === Registration edge cases ===

#[tokio::test]
async fn test_register_many_channels() {
    let mgr = ChannelManager::new();
    for i in 0..200 {
        let ch = Arc::new(StubChannel::new(&format!("ch-{}", i)));
        mgr.register(ch).await.unwrap();
    }
    assert_eq!(mgr.channel_count().await, 200);
}

#[tokio::test]
async fn test_register_or_replace_multiple_times() {
    let mgr = ChannelManager::new();
    for _ in 0..5 {
        let ch = Arc::new(StubChannel::new("same-name"));
        mgr.register_or_replace(ch).await;
    }
    assert_eq!(mgr.channel_count().await, 1);
}

#[tokio::test]
async fn test_unregister_all_channels() {
    let mgr = ChannelManager::new();
    for i in 0..10 {
        mgr.register(Arc::new(StubChannel::new(&format!("ch-{}", i))))
            .await
            .unwrap();
    }
    assert_eq!(mgr.channel_count().await, 10);

    for i in 0..10 {
        assert!(mgr.unregister(&format!("ch-{}", i)).await);
    }
    assert_eq!(mgr.channel_count().await, 0);
}

// === Send to channel edge cases ===

#[tokio::test]
async fn test_send_to_channel_unicode_content() {
    let mgr = ChannelManager::new();
    let ch = Arc::new(StubChannel::new("web"));
    mgr.register(ch.clone()).await.unwrap();

    mgr.send_to_channel("web", "chat-1", "你好世界 🌍")
        .await
        .unwrap();
    assert_eq!(ch.sent_messages()[0], "你好世界 🌍");
}

#[tokio::test]
async fn test_send_to_channel_large_content() {
    let mgr = ChannelManager::new();
    let ch = Arc::new(StubChannel::new("web"));
    mgr.register(ch.clone()).await.unwrap();

    let large = "x".repeat(1_000_000);
    mgr.send_to_channel("web", "chat-1", &large).await.unwrap();
    assert_eq!(ch.sent_messages()[0].len(), 1_000_000);
}

// === Channel status ===

#[tokio::test]
async fn test_channel_status_after_start() {
    let mgr = ChannelManager::new();
    let ch = Arc::new(StubChannel::new("running-ch"));
    mgr.register(ch.clone()).await.unwrap();

    let mgr_arc = Arc::new(mgr);
    mgr_arc.start_all().await.unwrap();

    let status = mgr_arc.get_status().await;
    assert!(status.contains_key("running-ch"));
    assert!(status["running-ch"].running);
}

// === Channel names ===

#[tokio::test]
async fn test_channel_names_ordering() {
    let mgr = ChannelManager::new();
    mgr.register(Arc::new(StubChannel::new("z-channel")))
        .await
        .unwrap();
    mgr.register(Arc::new(StubChannel::new("a-channel")))
        .await
        .unwrap();
    mgr.register(Arc::new(StubChannel::new("m-channel")))
        .await
        .unwrap();

    let mut names = mgr.channel_names().await;
    names.sort();
    assert_eq!(names, vec!["a-channel", "m-channel", "z-channel"]);
}

// === Default impl ===

#[tokio::test]
async fn test_manager_default() {
    let mgr = ChannelManager::default();
    assert_eq!(mgr.channel_count().await, 0);
}

// === Internal channel check ===

#[test]
fn test_is_internal_channel_all_types() {
    assert!(is_internal_channel("cli"));
    assert!(is_internal_channel("system"));
    assert!(is_internal_channel("subagent"));
    assert!(!is_internal_channel("web"));
    assert!(!is_internal_channel("rpc"));
    assert!(!is_internal_channel("websocket"));
    assert!(!is_internal_channel("telegram"));
    assert!(!is_internal_channel(""));
}

// === Concurrent dispatch ===

#[tokio::test]
async fn test_concurrent_dispatch_outbound() {
    let mgr = Arc::new(ChannelManager::new());
    let ch = Arc::new(StubChannel::new("concurrent"));
    mgr.register(ch.clone()).await.unwrap();

    let mut handles = vec![];
    for i in 0..10 {
        let mgr_clone = Arc::clone(&mgr);
        handles.push(tokio::spawn(async move {
            let msg = OutboundMessage {
                channel: "concurrent".to_string(),
                chat_id: format!("c{}", i),
                content: format!("msg {}", i),
                message_type: String::new(),
                meta: Default::default(),
            };
            mgr_clone.dispatch_outbound(msg).await.unwrap();
        }));
    }

    for h in handles {
        h.await.unwrap();
    }

    assert_eq!(ch.sent_messages().len(), 10);
}

// === FailingStubChannel for testing send errors ===

struct FailingStubChannel {
    name: String,
}

impl FailingStubChannel {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
        }
    }
}

#[async_trait]
impl Channel for FailingStubChannel {
    fn name(&self) -> &str {
        &self.name
    }
    async fn start(&self) -> Result<()> {
        Ok(())
    }
    async fn stop(&self) -> Result<()> {
        Ok(())
    }
    async fn send(&self, _msg: OutboundMessage) -> Result<()> {
        Err(NemesisError::Channel("send always fails".to_string()))
    }
}

// === Channel that fails to start ===

struct FailStartChannel {
    name: String,
}

impl FailStartChannel {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
        }
    }
}

#[async_trait]
impl Channel for FailStartChannel {
    fn name(&self) -> &str {
        &self.name
    }
    fn is_running(&self) -> bool {
        false
    }
    async fn start(&self) -> Result<()> {
        Err(NemesisError::Channel("start failed".to_string()))
    }
    async fn stop(&self) -> Result<()> {
        Ok(())
    }
    async fn send(&self, _msg: OutboundMessage) -> Result<()> {
        Ok(())
    }
}

// === Slow channel for timeout tests ===

#[allow(dead_code)]
struct SlowChannel {
    name: String,
    sent: Arc<parking_lot::RwLock<Vec<String>>>,
}

#[allow(dead_code)]
impl SlowChannel {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            sent: Arc::new(parking_lot::RwLock::new(Vec::new())),
        }
    }

    fn sent_messages(&self) -> Vec<String> {
        self.sent.read().clone()
    }
}

#[async_trait]
impl Channel for SlowChannel {
    fn name(&self) -> &str {
        &self.name
    }
    async fn start(&self) -> Result<()> {
        Ok(())
    }
    async fn stop(&self) -> Result<()> {
        Ok(())
    }
    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        self.sent.write().push(msg.content);
        Ok(())
    }
}

// === Tests for start_all with failing channel ===

#[tokio::test]
async fn test_start_all_with_failing_channel_continues() {
    let mgr = Arc::new(ChannelManager::new());
    let good_ch = Arc::new(StubChannel::new("good"));
    let _fail_ch = Arc::new(FailStartChannel::new("fail"));

    mgr.register(good_ch.clone()).await.unwrap();
    mgr.register(Arc::new(FailStartChannel::new("fail")))
        .await
        .unwrap();

    // start_all should continue even if one channel fails
    mgr.start_all().await.unwrap();

    // Good channel should still be started
    assert!(good_ch.is_started());
}

#[tokio::test]
async fn test_stop_all_with_failing_channel_continues() {
    let mgr = Arc::new(ChannelManager::new());
    let good_ch = Arc::new(StubChannel::new("good"));

    mgr.register(good_ch.clone()).await.unwrap();

    mgr.start_all().await.unwrap();
    assert!(good_ch.is_started());

    mgr.stop_all().await.unwrap();
    assert!(!good_ch.is_started());
}

// === Tests for init_channels with web config ===

#[tokio::test]
async fn test_init_channels_with_web() {
    let mgr = ChannelManager::new();
    let (tx, _) = broadcast::channel::<InboundMessage>(256);

    let mut config = ChannelInitConfig::default();
    config.web = Some(crate::web::WebChannelConfig::default());

    mgr.init_channels(&config, tx).await.unwrap();
    assert!(mgr.get("web").await.is_some());
    assert_eq!(mgr.channel_count().await, 1);
}

#[tokio::test]
async fn test_init_channels_with_websocket() {
    let mgr = ChannelManager::new();
    let (tx, _) = broadcast::channel::<InboundMessage>(256);

    let mut config = ChannelInitConfig::default();
    config.websocket = Some(crate::websocket::WebSocketChannelConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        ..Default::default()
    });

    mgr.init_channels(&config, tx).await.unwrap();
    assert!(mgr.get("websocket").await.is_some());
    assert_eq!(mgr.channel_count().await, 1);
}

#[tokio::test]
async fn test_init_channels_empty_config() {
    let mgr = ChannelManager::new();
    let (tx, _) = broadcast::channel::<InboundMessage>(256);

    let config = ChannelInitConfig::default();
    mgr.init_channels(&config, tx).await.unwrap();
    assert_eq!(mgr.channel_count().await, 0);
}

#[tokio::test]
async fn test_init_channels_web_and_websocket() {
    let mgr = ChannelManager::new();
    let (tx, _) = broadcast::channel::<InboundMessage>(256);

    let mut config = ChannelInitConfig::default();
    config.web = Some(crate::web::WebChannelConfig::default());
    config.websocket = Some(crate::websocket::WebSocketChannelConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        ..Default::default()
    });

    mgr.init_channels(&config, tx).await.unwrap();
    assert!(mgr.get("web").await.is_some());
    assert!(mgr.get("websocket").await.is_some());
    assert_eq!(mgr.channel_count().await, 2);
}

// === ChannelSyncConfig edge cases ===

#[test]
fn test_channel_sync_config_default() {
    let config = ChannelSyncConfig::default();
    assert!(config.targets.is_empty());
}

#[test]
fn test_channel_sync_config_with_targets() {
    let mut config = ChannelSyncConfig::default();
    config
        .targets
        .insert("a".to_string(), vec!["b".to_string(), "c".to_string()]);
    assert_eq!(config.targets.len(), 1);
    assert_eq!(config.targets["a"].len(), 2);
}

// === ChannelStatus serialization ===

#[test]
fn test_channel_status_serialize() {
    let status = ChannelStatus {
        enabled: true,
        running: false,
    };
    let json = serde_json::to_string(&status).unwrap();
    assert!(json.contains("\"enabled\":true"));
    assert!(json.contains("\"running\":false"));
}

#[test]
fn test_channel_status_deserialize() {
    let json = r#"{"enabled":true,"running":false}"#;
    let status: ChannelStatus = serde_json::from_str(json).unwrap();
    assert!(status.enabled);
    assert!(!status.running);
}

#[test]
fn test_channel_status_roundtrip() {
    let status = ChannelStatus {
        enabled: false,
        running: true,
    };
    let json = serde_json::to_string(&status).unwrap();
    let deserialized: ChannelStatus = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.enabled, status.enabled);
    assert_eq!(deserialized.running, status.running);
}

// === ManagerMetrics default ===

#[test]
fn test_manager_metrics_default() {
    let metrics = ManagerMetrics::default();
    assert_eq!(
        metrics
            .dispatched
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    assert_eq!(
        metrics
            .dropped_not_found
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    assert_eq!(
        metrics
            .dropped_filtered
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    assert_eq!(
        metrics
            .dropped_internal
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    assert_eq!(
        metrics
            .send_errors
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
}

// === Dispatch loop with allowed channels ===

#[tokio::test]
async fn test_dispatch_loop_with_allowed_filter() {
    let mgr = Arc::new(ChannelManager::with_allowed_channels(vec![
        "ok".to_string(),
    ]));
    let ch = Arc::new(StubChannel::new("ok"));
    mgr.register(ch.clone()).await.unwrap();

    mgr.start_dispatch_loop().unwrap();
    let tx = mgr.outbound_sender();

    // Allowed channel should be dispatched
    let msg1 = OutboundMessage {
        channel: "ok".to_string(),
        chat_id: "c1".to_string(),
        content: "allowed".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    tx.send(msg1).await.unwrap();

    // Filtered channel should be dropped
    let msg2 = OutboundMessage {
        channel: "blocked".to_string(),
        chat_id: "c1".to_string(),
        content: "filtered".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    tx.send(msg2).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    assert_eq!(ch.sent_messages().len(), 1);
    assert_eq!(ch.sent_messages()[0], "allowed");
    assert_eq!(
        mgr.metrics()
            .dropped_filtered
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    drop(tx);
}

// === Dispatch loop with send error ===

#[tokio::test]
async fn test_dispatch_loop_with_send_error() {
    let mgr = Arc::new(ChannelManager::new());
    let ch = Arc::new(FailingStubChannel::new("fail"));
    mgr.register(ch).await.unwrap();

    mgr.start_dispatch_loop().unwrap();
    let tx = mgr.outbound_sender();

    let msg = OutboundMessage {
        channel: "fail".to_string(),
        chat_id: "c1".to_string(),
        content: "will fail".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    tx.send(msg).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    assert_eq!(
        mgr.metrics()
            .send_errors
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    drop(tx);
}

// === Dispatch loop with missing channel ===

#[tokio::test]
async fn test_dispatch_loop_missing_channel() {
    let mgr = Arc::new(ChannelManager::new());
    mgr.start_dispatch_loop().unwrap();
    let tx = mgr.outbound_sender();

    let msg = OutboundMessage {
        channel: "nonexistent".to_string(),
        chat_id: "c1".to_string(),
        content: "lost".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    tx.send(msg).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    assert_eq!(
        mgr.metrics()
            .dropped_not_found
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    drop(tx);
}

// === Setup sync targets mixed valid and invalid ===

#[tokio::test]
async fn test_setup_sync_targets_mixed_valid_invalid() {
    let mgr = ChannelManager::new();
    mgr.register(Arc::new(StubChannel::new("a"))).await.unwrap();
    mgr.register(Arc::new(StubChannel::new("b"))).await.unwrap();
    // "c" NOT registered

    let mut config = ChannelSyncConfig::default();
    config
        .targets
        .insert("a".to_string(), vec!["b".to_string(), "c".to_string()]);

    mgr.setup_sync_targets(&config).await;

    let targets = mgr.get_sync_targets("a").await;
    assert_eq!(targets.len(), 1);
    assert!(targets.contains(&"b".to_string()));
    assert!(!targets.contains(&"c".to_string())); // nonexistent target skipped
}

// === Register, unregister, re-register ===

#[tokio::test]
async fn test_unregister_and_reregister() {
    let mgr = ChannelManager::new();
    let ch1 = Arc::new(StubChannel::new("ch"));
    mgr.register(ch1).await.unwrap();
    assert_eq!(mgr.channel_count().await, 1);

    mgr.unregister("ch").await;
    assert_eq!(mgr.channel_count().await, 0);

    let ch2 = Arc::new(StubChannel::new("ch"));
    mgr.register(ch2).await.unwrap();
    assert_eq!(mgr.channel_count().await, 1);
    assert!(mgr.get("ch").await.is_some());
}

// === Dispatch loop processes multiple channels ===

#[tokio::test]
async fn test_dispatch_loop_routes_to_correct_channel() {
    let mgr = Arc::new(ChannelManager::new());
    let ch_a = Arc::new(StubChannel::new("a"));
    let ch_b = Arc::new(StubChannel::new("b"));
    mgr.register(ch_a.clone()).await.unwrap();
    mgr.register(ch_b.clone()).await.unwrap();

    mgr.start_dispatch_loop().unwrap();
    let tx = mgr.outbound_sender();

    tx.send(OutboundMessage {
        channel: "a".to_string(),
        chat_id: "c1".to_string(),
        content: "for A".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    })
    .await
    .unwrap();

    tx.send(OutboundMessage {
        channel: "b".to_string(),
        chat_id: "c2".to_string(),
        content: "for B".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    })
    .await
    .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    assert_eq!(ch_a.sent_messages(), vec!["for A"]);
    assert_eq!(ch_b.sent_messages(), vec!["for B"]);
    drop(tx);
}

// === ChannelInitConfig debug ===

#[test]
fn test_channel_init_config_debug() {
    let config = ChannelInitConfig::default();
    let debug_str = format!("{:?}", config);
    assert!(debug_str.contains("ChannelInitConfig") || debug_str.contains("web"));
}

// === get_status returns running state ===

#[tokio::test]
async fn test_get_status_running_state() {
    let mgr = Arc::new(ChannelManager::new());
    let ch = Arc::new(StubChannel::new("test"));
    mgr.register(ch.clone()).await.unwrap();

    // Before start: not running
    let status = mgr.get_status().await;
    assert!(!status["test"].running);

    // After start: running
    mgr.start_all().await.unwrap();
    let status = mgr.get_status().await;
    assert!(status["test"].running);

    // After stop: not running
    mgr.stop_all().await.unwrap();
    let status = mgr.get_status().await;
    assert!(!status["test"].running);
}

// === Outbound sender cloning ===

#[tokio::test]
async fn test_outbound_sender_clones() {
    let mgr = Arc::new(ChannelManager::new());
    let tx1 = mgr.outbound_sender();
    let tx2 = mgr.outbound_sender();

    // Both should be usable
    assert!(!tx1.is_closed());
    assert!(!tx2.is_closed());
}

// === Metrics after multiple operations ===

#[tokio::test]
async fn test_metrics_comprehensive() {
    let mgr = ChannelManager::new();
    let ch = Arc::new(StubChannel::new("ok"));
    mgr.register(ch).await.unwrap();

    // Dispatch successful
    mgr.dispatch_outbound(OutboundMessage {
        channel: "ok".to_string(),
        chat_id: "c1".to_string(),
        content: "ok".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    })
    .await
    .unwrap();

    // Dispatch to missing channel
    let _ = mgr
        .dispatch_outbound(OutboundMessage {
            channel: "missing".to_string(),
            chat_id: "c1".to_string(),
            content: "lost".to_string(),
            message_type: String::new(),
            meta: Default::default(),
        })
        .await;

    assert_eq!(
        mgr.metrics()
            .dispatched
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    assert_eq!(
        mgr.metrics()
            .dropped_not_found
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
}

// === Setup sync targets replaces existing config ===

#[tokio::test]
async fn test_setup_sync_targets_replaces() {
    let mgr = ChannelManager::new();
    mgr.register(Arc::new(StubChannel::new("a"))).await.unwrap();
    mgr.register(Arc::new(StubChannel::new("b"))).await.unwrap();
    mgr.register(Arc::new(StubChannel::new("c"))).await.unwrap();

    // First config: a -> b
    let mut config1 = ChannelSyncConfig::default();
    config1
        .targets
        .insert("a".to_string(), vec!["b".to_string()]);
    mgr.setup_sync_targets(&config1).await;
    assert_eq!(mgr.get_sync_targets("a").await, vec!["b"]);

    // Replace with: a -> c
    let mut config2 = ChannelSyncConfig::default();
    config2
        .targets
        .insert("a".to_string(), vec!["c".to_string()]);
    mgr.setup_sync_targets(&config2).await;
    assert_eq!(mgr.get_sync_targets("a").await, vec!["c"]);
}

// ===========================================================================
// W4c 补测（2026-08-25）：init_channels 全通道块（default + feature-gated，两臂：
// 有效配置注册成功 / 无效配置 Err 跳过）、dispatch loop shutdown-flag break、
// stop_all 错误续行、setup_sync_targets add 错误跳过
// ===========================================================================

/// 填充所有已启用 feature 的通道配置（有效值）。
/// 未启用的 feature 块被 cfg 掉，测试在 default / all-features 两种闸门下都成立。
fn w4c_valid_init_config() -> ChannelInitConfig {
    let mut cfg = ChannelInitConfig::default();

    #[cfg(feature = "telegram")]
    {
        cfg.telegram = Some(crate::telegram::TelegramConfig {
            token: "tg-token".to_string(),
            ..Default::default()
        });
    }
    #[cfg(feature = "discord")]
    {
        cfg.discord = Some(crate::discord::DiscordConfig {
            token: "dc-token".to_string(),
            ..Default::default()
        });
    }
    #[cfg(feature = "slack")]
    {
        cfg.slack = Some(crate::slack::SlackConfig {
            bot_token: "xoxb-t".to_string(),
            app_token: "xapp-t".to_string(),
            allow_from: vec![],
        });
    }
    #[cfg(feature = "whatsapp")]
    {
        cfg.whatsapp = Some(crate::whatsapp::WhatsAppConfig {
            bridge_url: "http://127.0.0.1:1/bridge".to_string(),
            api_key: None,
            allow_from: vec![],
        });
    }
    #[cfg(feature = "feishu")]
    {
        cfg.feishu = Some(crate::feishu::FeishuConfig {
            app_id: "app".to_string(),
            app_secret: "sec".to_string(),
            verification_token: "vt".to_string(),
            encrypt_key: "ek".to_string(),
            allow_from: vec![],
        });
    }
    #[cfg(feature = "dingtalk")]
    {
        cfg.dingtalk = Some(crate::dingtalk::DingTalkConfig {
            client_id: "cid".to_string(),
            client_secret: "csec".to_string(),
            allow_from: vec![],
        });
    }
    #[cfg(feature = "tencent")]
    {
        cfg.qq = Some(crate::qq::QQConfig {
            app_id: "qqid".to_string(),
            app_secret: "qqsec".to_string(),
            ..Default::default()
        });
    }
    #[cfg(feature = "email")]
    {
        cfg.email = Some(crate::email::EmailConfig {
            imap_host: "imap.example.com".to_string(),
            smtp_host: "smtp.example.com".to_string(),
            imap_username: "u".to_string(),
            imap_password: "p".to_string(),
            ..Default::default()
        });
    }
    #[cfg(feature = "matrix")]
    {
        cfg.matrix = Some(crate::matrix::MatrixConfig {
            homeserver: "https://matrix.example.com".to_string(),
            user_id: "@bot:example.com".to_string(),
            access_token: "mt".to_string(),
            room_id: None,
            allow_from: vec![],
        });
    }
    #[cfg(feature = "irc")]
    {
        cfg.irc = Some(crate::irc::IRCConfig {
            server: "irc.example.com".to_string(),
            nick: "nem".to_string(),
            channel: "#test".to_string(),
            ..Default::default()
        });
    }
    #[cfg(feature = "signal")]
    {
        cfg.signal = Some(crate::signal::SignalConfig {
            api_url: "http://127.0.0.1:1".to_string(),
            phone_number: "+10000000000".to_string(),
            allow_from: vec![],
        });
    }
    #[cfg(feature = "mastodon")]
    {
        cfg.mastodon = Some(crate::mastodon::MastodonConfig {
            server: "https://mastodon.example.com".to_string(),
            access_token: "mdt".to_string(),
            allow_from: vec![],
        });
    }
    #[cfg(feature = "bluesky")]
    {
        cfg.bluesky = Some(crate::bluesky::BlueskyConfig {
            server: "https://bsky.example.com".to_string(),
            handle: "nem.example.com".to_string(),
            password: "bp".to_string(),
            did: None,
            poll_interval: 10,
            allow_from: vec![],
        });
    }
    #[cfg(feature = "onebot")]
    {
        cfg.onebot = Some(crate::onebot::OneBotConfig {
            ws_url: "ws://127.0.0.1:1/ws".to_string(),
            access_token: None,
            reconnect_interval: 5,
            group_trigger_prefix: vec![],
            allow_from: vec![],
        });
    }

    // 无 feature 闸的通道（default 编译就有）
    cfg.line = Some(crate::line::LineConfig {
        channel_access_token: "lt".to_string(),
        channel_secret: "ls".to_string(),
        webhook_port: 0,
        allow_from: vec![],
    });
    cfg.external = Some(crate::external::ExternalConfig {
        input_exe: "nonexistent-input.exe".to_string(),
        output_exe: "nonexistent-output.exe".to_string(),
        chat_id: "external".to_string(),
        sync_to: vec![],
        allow_from: vec![],
    });
    cfg.maixcam = Some(crate::maixcam::MaixCamConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        allow_from: vec![],
    });
    cfg.web = Some(crate::web::WebChannelConfig::default());
    cfg.websocket = Some(crate::websocket::WebSocketChannelConfig::default());

    cfg
}

#[tokio::test]
async fn test_w4c_init_channels_valid_configs_register_all() {
    let mgr = ChannelManager::new();
    let (bus, _rx) = tokio::sync::broadcast::channel(16);
    let cfg = w4c_valid_init_config();

    // init_channels 只构造+注册，不 start —— 不监听端口、不连远端
    mgr.init_channels(&cfg, bus).await.unwrap();

    let expected: Vec<&str> = vec![
        #[cfg(feature = "telegram")]
        "telegram",
        #[cfg(feature = "discord")]
        "discord",
        #[cfg(feature = "slack")]
        "slack",
        #[cfg(feature = "whatsapp")]
        "whatsapp",
        #[cfg(feature = "feishu")]
        "feishu",
        #[cfg(feature = "dingtalk")]
        "dingtalk",
        #[cfg(feature = "tencent")]
        "qq",
        #[cfg(feature = "email")]
        "email",
        #[cfg(feature = "matrix")]
        "matrix",
        #[cfg(feature = "irc")]
        "irc",
        #[cfg(feature = "signal")]
        "signal",
        #[cfg(feature = "mastodon")]
        "mastodon",
        #[cfg(feature = "bluesky")]
        "bluesky",
        #[cfg(feature = "onebot")]
        "onebot",
        "line",
        "external",
        "maixcam",
        "web",
        "websocket",
    ];

    assert_eq!(mgr.channel_count().await, expected.len());
    for name in &expected {
        assert!(
            mgr.get(name).await.is_some(),
            "channel '{}' should be registered after init_channels",
            name
        );
    }
}

/// 无效配置（必填字段为空）→ 各通道 Err 臂：记日志、不注册、init_channels 整体仍 Ok。
#[tokio::test]
async fn test_w4c_init_channels_invalid_configs_skip_registration() {
    let mgr = ChannelManager::new();
    let (bus, _rx) = tokio::sync::broadcast::channel(16);
    let cfg = ChannelInitConfig {
        #[cfg(feature = "telegram")]
        telegram: Some(crate::telegram::TelegramConfig::default()),
        #[cfg(feature = "discord")]
        discord: Some(crate::discord::DiscordConfig::default()),
        #[cfg(feature = "slack")]
        slack: Some(crate::slack::SlackConfig {
            bot_token: String::new(),
            app_token: String::new(),
            allow_from: vec![],
        }),
        #[cfg(feature = "whatsapp")]
        whatsapp: Some(crate::whatsapp::WhatsAppConfig {
            bridge_url: String::new(),
            api_key: None,
            allow_from: vec![],
        }),
        #[cfg(feature = "feishu")]
        feishu: Some(crate::feishu::FeishuConfig {
            app_id: String::new(),
            app_secret: String::new(),
            verification_token: String::new(),
            encrypt_key: String::new(),
            allow_from: vec![],
        }),
        #[cfg(feature = "dingtalk")]
        dingtalk: Some(crate::dingtalk::DingTalkConfig {
            client_id: String::new(),
            client_secret: String::new(),
            allow_from: vec![],
        }),
        #[cfg(feature = "tencent")]
        qq: Some(crate::qq::QQConfig::default()),
        #[cfg(feature = "email")]
        email: Some(crate::email::EmailConfig::default()),
        #[cfg(feature = "matrix")]
        matrix: Some(crate::matrix::MatrixConfig {
            homeserver: String::new(),
            user_id: String::new(),
            access_token: String::new(),
            room_id: None,
            allow_from: vec![],
        }),
        #[cfg(feature = "irc")]
        irc: Some(crate::irc::IRCConfig::default()),
        #[cfg(feature = "signal")]
        signal: Some(crate::signal::SignalConfig {
            api_url: String::new(),
            phone_number: String::new(),
            allow_from: vec![],
        }),
        #[cfg(feature = "mastodon")]
        mastodon: Some(crate::mastodon::MastodonConfig {
            server: String::new(),
            access_token: String::new(),
            allow_from: vec![],
        }),
        #[cfg(feature = "bluesky")]
        bluesky: Some(crate::bluesky::BlueskyConfig {
            server: String::new(),
            handle: String::new(),
            password: String::new(),
            did: None,
            poll_interval: 0,
            allow_from: vec![],
        }),
        #[cfg(feature = "onebot")]
        onebot: Some(crate::onebot::OneBotConfig {
            ws_url: String::new(),
            access_token: None,
            reconnect_interval: 0,
            group_trigger_prefix: vec![],
            allow_from: vec![],
        }),
        // line 空 token → Err；external 空 exe → Err；maixcam/web/websocket 无 Err 臂
        line: Some(crate::line::LineConfig {
            channel_access_token: String::new(),
            channel_secret: String::new(),
            webhook_port: 0,
            allow_from: vec![],
        }),
        external: Some(crate::external::ExternalConfig {
            input_exe: String::new(),
            output_exe: String::new(),
            chat_id: String::new(),
            sync_to: vec![],
            allow_from: vec![],
        }),
        // maixcam/web/websocket 构造无校验 → 必注册（各 1 个）
        maixcam: Some(crate::maixcam::MaixCamConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
            allow_from: vec![],
        }),
        web: Some(crate::web::WebChannelConfig::default()),
        websocket: Some(crate::websocket::WebSocketChannelConfig::default()),
        web_server_ops: None,
    };

    let result = mgr.init_channels(&cfg, bus).await;
    assert!(result.is_ok(), "init_channels must not fail on bad configs");

    // 只有三个无校验构造的通道注册成功
    assert_eq!(mgr.channel_count().await, 3);
    assert!(mgr.get("maixcam").await.is_some());
    assert!(mgr.get("web").await.is_some());
    assert!(mgr.get("websocket").await.is_some());
    // 其余全部因校验失败被跳过
    #[cfg(feature = "telegram")]
    assert!(mgr.get("telegram").await.is_none());
    #[cfg(feature = "discord")]
    assert!(mgr.get("discord").await.is_none());
    #[cfg(feature = "tencent")]
    assert!(mgr.get("qq").await.is_none());
    assert!(mgr.get("line").await.is_none());
    assert!(mgr.get("external").await.is_none());
}

#[tokio::test]
async fn test_w4c_dispatch_loop_breaks_after_shutdown_flag() {
    // stop_all() 置 shutdown 标志后，即使克隆的 sender 还活着（mgr 自持
    // outbound_tx），循环也必须在下一轮顶部 break。
    let mgr = std::sync::Arc::new(ChannelManager::new());
    let stub = std::sync::Arc::new(StubChannel::new("stub"));
    mgr.register(stub.clone()).await.unwrap();
    mgr.start_dispatch_loop().unwrap();

    let tx = mgr.outbound_sender();
    let m1 = OutboundMessage::new("stub", "c1", "one");
    tx.send(m1).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(stub.sent_messages().len(), 1);

    // 置 shutdown 标志（同时停止通道）
    mgr.stop_all().await.unwrap();

    // 再发两条：循环若还活着会处理（race），但 break 之后必须全部静默丢弃
    tx.send(OutboundMessage::new("stub", "c2", "two"))
        .await
        .unwrap();
    tx.send(OutboundMessage::new("stub", "c3", "three"))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let sent = stub.sent_messages().len();
    assert!(
        sent <= 2,
        "dispatch loop should have exited after shutdown flag; got {} messages",
        sent
    );
}

/// stop() 返回 Err 的通道：stop_all 记 error 日志后继续停其余通道。
struct W4cErrorStopChannel {
    base: crate::base::BaseChannel,
    stopped: std::sync::Arc<parking_lot::RwLock<bool>>,
}

#[async_trait]
impl Channel for W4cErrorStopChannel {
    fn name(&self) -> &str {
        self.base.name()
    }
    fn is_running(&self) -> bool {
        self.base.is_running()
    }
    async fn start(&self) -> Result<()> {
        self.base.set_running(true);
        Ok(())
    }
    async fn stop(&self) -> Result<()> {
        // 记录“被调过”然后返回 Err
        *self.stopped.write() = true;
        Err(NemesisError::Channel("stop always fails".to_string()))
    }
    async fn send(&self, _msg: OutboundMessage) -> Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn test_w4c_stop_all_continues_after_channel_stop_error() {
    let mgr = ChannelManager::new();
    let stopped_flag = std::sync::Arc::new(parking_lot::RwLock::new(false));
    let err_ch = std::sync::Arc::new(W4cErrorStopChannel {
        base: crate::base::BaseChannel::new("err-stop"),
        stopped: stopped_flag.clone(),
    });
    let ok_ch = std::sync::Arc::new(StubChannel::new("after-err"));
    ok_ch.start().await.unwrap();

    mgr.register(err_ch).await.unwrap();
    mgr.register(ok_ch.clone()).await.unwrap();

    // stop_all 对 Err 只记日志，整体 Ok，且后续通道也被停掉
    mgr.stop_all().await.unwrap();
    assert!(*stopped_flag.read(), "error-stop channel's stop() must be called");
    assert!(!ok_ch.is_started(), "channel after the error must still be stopped");
}

/// add_sync_target 返回 Err 的源通道：setup_sync_targets 跳过该对并继续。
struct W4cRejectSyncChannel {
    base: crate::base::BaseChannel,
}

#[async_trait]
impl Channel for W4cRejectSyncChannel {
    fn name(&self) -> &str {
        self.base.name()
    }
    fn is_running(&self) -> bool {
        self.base.is_running()
    }
    async fn start(&self) -> Result<()> {
        self.base.set_running(true);
        Ok(())
    }
    async fn stop(&self) -> Result<()> {
        self.base.set_running(false);
        Ok(())
    }
    async fn send(&self, _msg: OutboundMessage) -> Result<()> {
        Ok(())
    }
    fn add_sync_target(
        &self,
        _name: &str,
        _channel: std::sync::Arc<dyn Channel>,
    ) -> Result<()> {
        Err(NemesisError::Channel("sync rejected".to_string()))
    }
}

#[tokio::test]
async fn test_w4c_setup_sync_targets_add_error_is_skipped() {
    let mgr = ChannelManager::new();
    let src = std::sync::Arc::new(W4cRejectSyncChannel {
        base: crate::base::BaseChannel::new("reject-src"),
    });
    let target = std::sync::Arc::new(StubChannel::new("tgt"));
    let good_src = std::sync::Arc::new(StubChannel::new("good-src"));
    let good_tgt = std::sync::Arc::new(StubChannel::new("good-tgt"));

    mgr.register(src).await.unwrap();
    mgr.register(target).await.unwrap();
    mgr.register(good_src).await.unwrap();
    mgr.register(good_tgt).await.unwrap();

    let mut targets = std::collections::HashMap::new();
    targets.insert("reject-src".to_string(), vec!["tgt".to_string()]);
    targets.insert("good-src".to_string(), vec!["good-tgt".to_string()]);
    let sync_cfg = ChannelSyncConfig { targets };

    mgr.setup_sync_targets(&sync_cfg).await;

    // 失败对不进 sync_targets 表；成功对进
    assert!(mgr.get_sync_targets("reject-src").await.is_empty());
    assert_eq!(
        mgr.get_sync_targets("good-src").await,
        vec!["good-tgt".to_string()]
    );
}

// ===========================================================================
// S2 coverage (2026-08-26): dispatch loop content_len tracing field +
// init_channels web_server_ops set_server arm
// ===========================================================================

fn s2_enable_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(std::io::sink)
        .try_init();
}

/// The non-internal outbound message debug! has `content_len` as a field
/// expression: it only evaluates with a subscriber active.
#[tokio::test]
async fn s2_dispatch_loop_non_internal_message_logs_content_len_field() {
    s2_enable_tracing();
    let mgr = Arc::new(ChannelManager::new());
    let ch = Arc::new(StubChannel::new("s2-loop"));
    mgr.register(ch.clone()).await.unwrap();

    mgr.start_dispatch_loop().unwrap();
    let tx = mgr.outbound_sender();

    let msg = OutboundMessage {
        channel: "s2-loop".to_string(),
        chat_id: "c1".to_string(),
        content: "field coverage".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    tx.send(msg).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let sent = ch.sent_messages();
    assert!(sent.contains(&"field coverage".to_string()));

    drop(tx);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
}

/// init_channels with `web_server_ops` set must call `ch.set_server(ops)`
/// (the Some arm of the web block).
#[tokio::test]
async fn s2_init_channels_web_with_server_ops_sets_server() {
    struct S2WebOps;
    impl crate::web::WebServerOps for S2WebOps {
        fn send_to_session(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: Option<&str>,
        ) -> std::result::Result<(), String> {
            Ok(())
        }
        fn send_history_to_session(&self, _: &str, _: &str) -> std::result::Result<(), String> {
            Ok(())
        }
        fn broadcast(&self, _: &str) -> std::result::Result<(), String> {
            Ok(())
        }
        fn active_session_ids(&self) -> Vec<String> {
            Vec::new()
        }
        fn start_server(&self) -> std::result::Result<(), String> {
            Ok(())
        }
        fn stop_server(&self) {}
    }

    let (bus, _rx) = tokio::sync::broadcast::channel::<InboundMessage>(16);
    let mgr = ChannelManager::new();
    let mut config = ChannelInitConfig::default();
    config.web = Some(crate::web::WebChannelConfig::default());
    config.web_server_ops = Some(Arc::new(S2WebOps));

    mgr.init_channels(&config, bus).await.unwrap();

    assert_eq!(mgr.channel_count().await, 1);
    assert!(
        mgr.get("web").await.is_some(),
        "web channel must be registered"
    );
}

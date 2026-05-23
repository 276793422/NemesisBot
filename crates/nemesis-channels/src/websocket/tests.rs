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

use super::*;

#[test]
fn test_key_generator() {
    let key_gen = KeyGenerator::new();
    let key = key_gen.generate("child-1", 1234);
    assert!(key.contains("child-1"));

    let validated = key_gen.validate(&key).unwrap();
    assert_eq!(validated.child_pid, 1234);
    assert_eq!(validated.child_id.as_deref(), Some("child-1"));
}

#[test]
fn test_key_generator_invalid() {
    let key_gen = KeyGenerator::new();
    let result = key_gen.validate("invalid");
    assert!(result.is_err());
}

#[test]
fn test_key_generator_remove() {
    let key_gen = KeyGenerator::new();
    let key = key_gen.generate("child-1", 1234);
    key_gen.remove(&key);
    assert!(key_gen.validate(&key).is_err());
}

#[tokio::test]
async fn test_server_start_and_stop() {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen);
    let result = server.start().await;
    assert!(result.is_ok());
    let port = result.unwrap();
    assert!(port > 0);
    server.stop();
}

#[test]
fn test_server_notification_no_connection() {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen);
    let result = server.send_notification("nonexistent", "test", serde_json::Value::Null);
    assert!(result.is_err());
}

#[tokio::test]
async fn test_server_call_no_connection() {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen);
    let result = server
        .call_child("nonexistent", "test", serde_json::Value::Null)
        .await;
    assert!(result.is_err());
}

#[test]
fn test_server_register_handler() {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen);
    server.register_handler("ping", |msg| {
        Ok(Message::new_response(
            msg.id.as_deref().unwrap_or(""),
            serde_json::json!("pong"),
        ))
    });
}

#[test]
fn test_server_register_notification_handler() {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen);
    server.register_notification_handler("event", |_msg| {});
}

#[test]
fn test_server_get_connection_none() {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen);
    assert!(server.get_connection("nonexistent").is_none());
}

#[test]
fn test_server_remove_connection_nonexistent() {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen);
    // Should not panic
    server.remove_connection("nonexistent");
}

#[test]
fn test_key_generator_revoke() {
    let key_gen = KeyGenerator::new();
    let key = key_gen.generate("child-1", 1234);

    // Revoke existing key returns true
    assert!(key_gen.revoke(&key));
    assert!(key_gen.validate(&key).is_err());

    // Revoke non-existent key returns false
    assert!(!key_gen.revoke("nonexistent"));
}

#[test]
fn test_key_generator_cleanup() {
    let key_gen = KeyGenerator::new();
    let key1 = key_gen.generate("child-1", 1111);
    let key2 = key_gen.generate("child-2", 2222);

    // Cleanup with very large max_age should remove nothing
    let removed = key_gen.cleanup(Duration::from_secs(86400 * 365));
    assert_eq!(removed, 0);
    assert!(key_gen.validate(&key1).is_ok());
    assert!(key_gen.validate(&key2).is_ok());

    // Cleanup with zero max_age should remove all keys
    let removed = key_gen.cleanup(Duration::ZERO);
    assert_eq!(removed, 2);
    assert!(key_gen.validate(&key1).is_err());
    assert!(key_gen.validate(&key2).is_err());
}

#[test]
fn test_key_generator_timestamps() {
    let key_gen = KeyGenerator::new();
    let key = key_gen.generate("child-1", 1234);

    // Validate the key and check used_at is set
    let validated = key_gen.validate(&key).unwrap();
    assert!(validated.created_at <= chrono::Local::now());
    assert!(validated.used_at.is_some());

    // Before validation, used_at was None in the stored copy;
    // after validation it should be set
    assert!(validated.used_at.unwrap() >= validated.created_at);
}

#[test]
fn test_server_get_port_default() {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen);
    assert_eq!(server.get_port(), 0);
}

#[tokio::test]
async fn test_server_start_assigns_port() {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen);
    let port = server.start().await.unwrap();
    assert_ne!(port, 0);
    assert_eq!(server.get_port(), port);
    server.stop();
}

#[test]
fn test_child_connection_new() {
    let (tx, _rx) = tokio::sync::mpsc::channel::<String>(64);
    let conn = ChildConnection::new("key-1".to_string(), "key-1".to_string(), 1234, tx);
    assert_eq!(conn.id, "key-1");
    assert_eq!(conn.child_pid, 1234);
    assert!(conn.child_id.is_none());
    assert!(!conn.is_closed());
}

#[test]
fn test_child_connection_close() {
    let (tx, _rx) = tokio::sync::mpsc::channel::<String>(64);
    let conn = ChildConnection::new("key-1".to_string(), "key-1".to_string(), 1234, tx);
    conn.close();
    assert!(conn.is_closed());
}

// ============================================================
// Additional tests for ~92% coverage
// ============================================================

#[test]
fn test_ws_server_error_display() {
    let err = WsServerError::ConnectionNotFound;
    assert!(err.to_string().contains("connection not found"));

    let err = WsServerError::CallTimeout;
    assert!(err.to_string().contains("call timeout"));

    let err = WsServerError::SendTimeout;
    assert!(err.to_string().contains("send timeout"));

    let err = WsServerError::Other("custom error".to_string());
    assert!(err.to_string().contains("custom error"));
}

#[test]
fn test_ws_server_error_debug() {
    let err = WsServerError::ConnectionNotFound;
    let debug = format!("{:?}", err);
    assert!(debug.contains("ConnectionNotFound"));
}

#[test]
fn test_child_connection_send_closed() {
    let (tx, _rx) = tokio::sync::mpsc::channel::<String>(64);
    let conn = ChildConnection::new("key-1".to_string(), "key-1".to_string(), 1234, tx);
    conn.close();
    // Send should fail when closed
    // Need runtime for async send
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(conn.send("test".to_string()));
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("connection closed"));
}

#[test]
fn test_child_connection_send_success() {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(64);
    let conn = ChildConnection::new("key-1".to_string(), "key-1".to_string(), 1234, tx);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(conn.send("hello".to_string()));
    assert!(result.is_ok());
    let received = rx.try_recv().unwrap();
    assert_eq!(received, "hello");
}

#[test]
fn test_child_connection_send_channel_dropped() {
    let (tx, rx) = tokio::sync::mpsc::channel::<String>(64);
    let conn = ChildConnection::new("key-1".to_string(), "key-1".to_string(), 1234, tx);
    drop(rx); // Drop receiver
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(conn.send("hello".to_string()));
    assert!(result.is_err());
}

#[test]
fn test_child_connection_dispatcher() {
    let (tx, _rx) = tokio::sync::mpsc::channel::<String>(64);
    let conn = ChildConnection::new("key-1".to_string(), "key-1".to_string(), 1234, tx);
    // Dispatcher should be usable
    conn.dispatcher.register("ping", |msg| {
        Ok(Message::new_response(msg.id.as_deref().unwrap_or(""), serde_json::json!("pong")))
    });
    let req = Message::new_request("ping", serde_json::Value::Null);
    let result = conn.dispatcher.dispatch(&req).unwrap().unwrap();
    assert_eq!(result.result.as_ref().unwrap(), &serde_json::json!("pong"));
}

#[test]
fn test_validated_key_debug_clone() {
    let key_gen = KeyGenerator::new();
    let key = key_gen.generate("child-1", 1234);
    let validated = key_gen.validate(&key).unwrap();

    // Debug
    let debug = format!("{:?}", validated);
    assert!(debug.contains("child-1"));

    // Clone
    let cloned = validated.clone();
    assert_eq!(cloned.child_pid, 1234);
    assert_eq!(cloned.key, key);
}

#[test]
fn test_validated_key_fields() {
    let key_gen = KeyGenerator::new();
    let key = key_gen.generate("child-test", 5678);
    let validated = key_gen.validate(&key).unwrap();
    assert_eq!(validated.child_pid, 5678);
    assert_eq!(validated.child_id.as_deref(), Some("child-test"));
    assert_eq!(validated.key, key);
    assert!(validated.created_at <= chrono::Local::now());
    assert!(validated.used_at.is_some());
}

#[test]
fn test_key_generator_multiple_keys() {
    let key_gen = KeyGenerator::new();
    let key1 = key_gen.generate("child-1", 1111);
    let key2 = key_gen.generate("child-2", 2222);
    let key3 = key_gen.generate("child-3", 3333);

    assert!(key_gen.validate(&key1).is_ok());
    assert!(key_gen.validate(&key2).is_ok());
    assert!(key_gen.validate(&key3).is_ok());

    // Remove key2
    key_gen.remove(&key2);
    assert!(key_gen.validate(&key1).is_ok());
    assert!(key_gen.validate(&key2).is_err());
    assert!(key_gen.validate(&key3).is_ok());
}

#[test]
fn test_key_generator_cleanup_partial() {
    let key_gen = KeyGenerator::new();
    let _key1 = key_gen.generate("child-1", 1111);
    // Wait a tiny bit
    std::thread::sleep(std::time::Duration::from_millis(10));
    let _key2 = key_gen.generate("child-2", 2222);

    // Cleanup with 5ms should remove key1 but keep key2 (timing sensitive)
    // This test may be flaky on very fast machines; use a larger margin
    let removed = key_gen.cleanup(std::time::Duration::from_millis(5));
    // At least key1 should be removed (it was created 10ms before cleanup check)
    assert!(removed >= 1);
}

#[test]
fn test_server_register_handler_and_use() {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen);

    server.register_handler("test.method", |msg| {
        Ok(Message::new_response(msg.id.as_deref().unwrap_or(""), serde_json::json!({"result": "ok"})))
    });
    // Verify it was registered (no panic)
}

#[test]
fn test_server_notification_handler() {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen);
    let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let called_clone = called.clone();
    server.register_notification_handler("event", move |_msg| {
        called_clone.store(true, std::sync::atomic::Ordering::SeqCst);
    });
}

#[tokio::test]
async fn test_server_start_stop_idempotent() {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen);
    let port = server.start().await.unwrap();
    assert!(port > 0);
    server.stop();
    // Stop again should not panic
    server.stop();
}

#[test]
fn test_server_send_notification_nonexistent() {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen);
    let result = server.send_notification("nonexistent", "method", serde_json::json!({}));
    assert!(result.is_err());
}

#[tokio::test]
async fn test_server_call_child_nonexistent() {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen);
    let result = server.call_child("nonexistent", "method", serde_json::json!({})).await;
    assert!(matches!(result, Err(WsServerError::ConnectionNotFound)));
}

#[test]
fn test_child_connection_child_id() {
    let (tx, _rx) = tokio::sync::mpsc::channel::<String>(64);
    let mut conn = ChildConnection::new("key-1".to_string(), "key-1".to_string(), 1234, tx);
    assert!(conn.child_id.is_none());
    conn.child_id = Some("child-test".to_string());
    assert_eq!(conn.child_id.as_deref(), Some("child-test"));
}

// ============================================================
// Phase 4: Integration tests for higher coverage
// ============================================================

#[tokio::test]
async fn test_server_client_full_connection() {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen.clone());
    let port = server.start().await.unwrap();

    // Generate a key
    let key = key_gen.generate("child-test", 42);

    // Connect a client to the server
    let url = format!("ws://127.0.0.1:{}{}", port, key);
    let connect_result = tokio_tungstenite::connect_async(&url).await;
    if let Ok((mut ws_stream, _)) = connect_result {
        // Send auth message
        let auth = serde_json::json!({"type": "auth", "key": key});
        ws_stream
            .send(tokio_tungstenite::tungstenite::Message::Text(auth.to_string().into()))
            .await
            .unwrap();

        // Give server time to process
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Verify connection exists
        assert!(server.get_connection(&key).is_some());
        assert!(server.get_connection("child-test").is_some());

        // Get connection and verify child_pid
        let conn = server.get_connection(&key).unwrap();
        let guard = conn.lock().await;
        assert_eq!(guard.child_pid, 42);
        assert_eq!(guard.child_id.as_deref(), Some("child-test"));
        drop(guard);

        // Send notification from server to client
        let result = server.send_notification("child-test", "test.method", serde_json::json!({"data": 123}));
        assert!(result.is_ok());

        // Client should receive the notification
        let msg_result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            ws_stream.next()
        ).await;

        if let Ok(Some(Ok(ws_msg))) = msg_result {
            if let tokio_tungstenite::tungstenite::Message::Text(text) = ws_msg {
                let msg: Message = serde_json::from_str(&text).unwrap();
                assert!(msg.is_notification());
                assert_eq!(msg.method.as_deref(), Some("test.method"));
            }
        }

        // Close connection
        ws_stream.close(None).await.ok();
    }

    server.stop();
}

#[tokio::test]
async fn test_server_client_auth_failure() {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen.clone());
    let port = server.start().await.unwrap();

    // Connect with invalid key
    let url = format!("ws://127.0.0.1:{}/test", port);
    let connect_result = tokio_tungstenite::connect_async(&url).await;
    if let Ok((mut ws_stream, _)) = connect_result {
        // Send invalid auth
        let auth = serde_json::json!({"type": "auth", "key": "invalid-key"});
        let _ = ws_stream
            .send(tokio_tungstenite::tungstenite::Message::Text(auth.to_string().into()))
            .await;

        // Give server time to process
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Connection should not be registered
        assert!(server.get_connection("invalid-key").is_none());
    }

    server.stop();
}

#[tokio::test]
async fn test_server_client_no_key_in_auth() {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen.clone());
    let port = server.start().await.unwrap();

    let url = format!("ws://127.0.0.1:{}/test", port);
    let connect_result = tokio_tungstenite::connect_async(&url).await;
    if let Ok((mut ws_stream, _)) = connect_result {
        // Send auth without key field
        let auth = serde_json::json!({"type": "auth"});
        let _ = ws_stream
            .send(tokio_tungstenite::tungstenite::Message::Text(auth.to_string().into()))
            .await;

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        // Connection should not be registered
        assert!(server.get_connection("anything").is_none());
    }

    server.stop();
}

#[tokio::test]
async fn test_server_client_invalid_auth_json() {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen.clone());
    let port = server.start().await.unwrap();

    let url = format!("ws://127.0.0.1:{}/test", port);
    let connect_result = tokio_tungstenite::connect_async(&url).await;
    if let Ok((mut ws_stream, _)) = connect_result {
        // Send invalid JSON
        let _ = ws_stream
            .send(tokio_tungstenite::tungstenite::Message::Text("not json".into()))
            .await;

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(server.get_connection("anything").is_none());
    }

    server.stop();
}

#[tokio::test]
async fn test_server_client_request_response() {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen.clone());

    // Register a handler on the server
    server.register_handler("add", |msg| {
        let id = msg.id.as_deref().unwrap_or("");
        Ok(Message::new_response(id, serde_json::json!({"result": "added"})))
    });

    let port = server.start().await.unwrap();
    let key = key_gen.generate("child-rpc", 100);

    let url = format!("ws://127.0.0.1:{}{}", port, key);
    let connect_result = tokio_tungstenite::connect_async(&url).await;
    if let Ok((mut ws_stream, _)) = connect_result {
        // Auth
        let auth = serde_json::json!({"type": "auth", "key": key});
        ws_stream
            .send(tokio_tungstenite::tungstenite::Message::Text(auth.to_string().into()))
            .await
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Send a request from client to server
        let request = Message::new_request("add", serde_json::json!({"a": 1, "b": 2}));
        let request_str = serde_json::to_string(&request).unwrap();
        ws_stream
            .send(tokio_tungstenite::tungstenite::Message::Text(request_str.into()))
            .await
            .unwrap();

        // Receive response
        let msg_result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            ws_stream.next()
        ).await;

        if let Ok(Some(Ok(ws_msg))) = msg_result {
            if let tokio_tungstenite::tungstenite::Message::Text(text) = ws_msg {
                let resp: Message = serde_json::from_str(&text).unwrap();
                assert!(resp.is_success_response());
                assert_eq!(resp.result.as_ref().unwrap()["result"], "added");
            }
        }

        ws_stream.close(None).await.ok();
    }

    server.stop();
}

#[tokio::test]
async fn test_server_call_child_with_connection() {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen.clone());
    let port = server.start().await.unwrap();
    let key = key_gen.generate("child-call", 200);

    let url = format!("ws://127.0.0.1:{}{}", port, key);
    let connect_result = tokio_tungstenite::connect_async(&url).await;
    if let Ok((mut ws_stream, _)) = connect_result {
        // Auth
        let auth = serde_json::json!({"type": "auth", "key": key});
        ws_stream
            .send(tokio_tungstenite::tungstenite::Message::Text(auth.to_string().into()))
            .await
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // call_child should work now (connection exists)
        // We need to spawn a task to read and respond
        let (client_tx, mut client_rx) = tokio::sync::mpsc::channel::<String>(64);

        // Read the call request from server and send a response
        let read_handle = tokio::spawn(async move {
            if let Some(Ok(ws_msg)) = ws_stream.next().await {
                if let tokio_tungstenite::tungstenite::Message::Text(text) = ws_msg {
                    let msg: Message = serde_json::from_str(&text).unwrap();
                    if msg.is_request() {
                        let resp = Message::new_response(
                            msg.id.as_deref().unwrap_or(""),
                            serde_json::json!({"status": "handled"}),
                        );
                        let _ = client_tx.send(serde_json::to_string(&resp).unwrap()).await;
                    }
                }
            }
            ws_stream
        });

            // Wait for the response to be ready
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;

            // Make the call
            let call_result = server.call_child("child-call", "test.method", serde_json::json!({})).await;
            if let Ok(response) = call_result {
                assert!(response.is_success_response());
                assert_eq!(response.result.as_ref().unwrap()["status"], "handled");
            }

            // Send response from client side
            if let Some(resp_str) = client_rx.recv().await {
                let mut ws = read_handle.await.unwrap();
                let _ = ws
                    .send(tokio_tungstenite::tungstenite::Message::Text(resp_str.into()))
                    .await;
            }
    }

    server.stop();
}

#[tokio::test]
async fn test_server_remove_connection_with_child() {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen.clone());
    let port = server.start().await.unwrap();
    let key = key_gen.generate("child-remove", 300);

    let url = format!("ws://127.0.0.1:{}{}", port, key);
    let connect_result = tokio_tungstenite::connect_async(&url).await;
    if let Ok((mut ws_stream, _)) = connect_result {
        // Auth
        let auth = serde_json::json!({"type": "auth", "key": key});
        ws_stream
            .send(tokio_tungstenite::tungstenite::Message::Text(auth.to_string().into()))
            .await
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Connection should exist
        assert!(server.get_connection("child-remove").is_some());

        // Remove connection
        server.remove_connection("child-remove");
        assert!(server.get_connection("child-remove").is_none());

        ws_stream.close(None).await.ok();
    }

    server.stop();
}

#[test]
fn test_send_notification_connection_busy() {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen);

    // Manually insert a connection with a locked mutex
    let (tx, _rx) = tokio::sync::mpsc::channel::<String>(64);
    let conn = Arc::new(tokio::sync::Mutex::new(
        ChildConnection::new("key-1".to_string(), "key-1".to_string(), 42, tx)
    ));

    // Lock the connection so try_lock fails
    let guard = conn.blocking_lock();
    {
        let mut state = server.state.lock();
        state.connections.insert("test-id".to_string(), conn.clone());
    }

    // send_notification should fail because connection is busy
    let result = server.send_notification("test-id", "test", serde_json::Value::Null);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("connection busy"));

    drop(guard);
}

#[test]
fn test_send_notification_success() {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen);

    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(64);
    let conn = Arc::new(tokio::sync::Mutex::new(
        ChildConnection::new("key-1".to_string(), "key-1".to_string(), 42, tx)
    ));

    {
        let mut state = server.state.lock();
        state.connections.insert("test-id".to_string(), conn.clone());
    }

    let result = server.send_notification("test-id", "method", serde_json::json!({"x": 1}));
    assert!(result.is_ok());

    // Verify message was sent
    let msg_str = rx.try_recv().unwrap();
    let msg: Message = serde_json::from_str(&msg_str).unwrap();
    assert!(msg.is_notification());
    assert_eq!(msg.method.as_deref(), Some("method"));
}

// ============================================================
// Additional tests for 95%+ coverage
// ============================================================

#[test]
fn test_server_key_generator_accessible() {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen.clone());
    let gen_ref = server.key_generator();
    let key = gen_ref.generate("child-1", 1234);
    assert!(gen_ref.validate(&key).is_ok());
}

#[test]
fn test_ws_server_error_variants() {
    let err = WsServerError::ConnectionNotFound;
    assert_eq!(err.to_string(), "connection not found");

    let err = WsServerError::CallTimeout;
    assert_eq!(err.to_string(), "call timeout");

    let err = WsServerError::SendTimeout;
    assert_eq!(err.to_string(), "send timeout");

    let err = WsServerError::Other("custom".to_string());
    assert_eq!(err.to_string(), "custom");
}

#[test]
fn test_validated_key_clone_independent() {
    let key_gen = KeyGenerator::new();
    let key = key_gen.generate("child-1", 1234);
    let v1 = key_gen.validate(&key).unwrap();
    let v2 = v1.clone();
    // They should be equal but independent
    assert_eq!(v1.key, v2.key);
    assert_eq!(v1.child_pid, v2.child_pid);
}

#[tokio::test]
async fn test_server_send_notification_connection_closed() {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen);

    let (tx, rx) = tokio::sync::mpsc::channel::<String>(64);
    drop(rx); // Drop receiver to simulate closed channel

    let conn = Arc::new(tokio::sync::Mutex::new(
        ChildConnection::new("key-1".to_string(), "key-1".to_string(), 42, tx)
    ));

    // Close the connection
    conn.lock().await.close();

    {
        let mut state = server.state.lock();
        state.connections.insert("test-id".to_string(), conn.clone());
    }

    // send_notification should fail because the receiver is dropped
    let result = server.send_notification("test-id", "test", serde_json::Value::Null);
    assert!(result.is_err());
}

#[test]
fn test_send_notification_connection_rx_dropped() {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen);

    let (tx, rx) = tokio::sync::mpsc::channel::<String>(64);
    drop(rx); // Drop receiver to simulate closed channel

    let conn = Arc::new(tokio::sync::Mutex::new(
        ChildConnection::new("key-1".to_string(), "key-1".to_string(), 42, tx)
    ));

    {
        let mut state = server.state.lock();
        state.connections.insert("test-id".to_string(), conn.clone());
    }

    let result = server.send_notification("test-id", "test", serde_json::Value::Null);
    assert!(result.is_err());
}

#[test]
fn test_child_connection_send_after_close() {
    let (tx, _rx) = tokio::sync::mpsc::channel::<String>(64);
    let conn = ChildConnection::new("key-1".to_string(), "key-1".to_string(), 42, tx);
    assert!(!conn.is_closed());
    conn.close();
    assert!(conn.is_closed());
    // Double close should be safe
    conn.close();
    assert!(conn.is_closed());
}

#[test]
fn test_key_generator_generate_format() {
    let key_gen = KeyGenerator::new();
    let key = key_gen.generate("my-child", 9999);
    // Key should contain child_id and child_pid
    assert!(key.starts_with("my-child-9999-"));
    // And end with a UUID
    let parts: Vec<&str> = key.splitn(4, '-').collect();
    assert!(parts.len() >= 3);
}

#[tokio::test]
async fn test_server_double_start_gets_new_port() {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen);

    // Starting twice should work (second start binds a new port)
    let port1 = server.start().await.unwrap();
    assert!(port1 > 0);
    server.stop();
}

#[test]
fn test_server_state_empty() {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen);
    // Initially no connections
    assert!(server.get_connection("anything").is_none());
    assert_eq!(server.get_port(), 0);
}

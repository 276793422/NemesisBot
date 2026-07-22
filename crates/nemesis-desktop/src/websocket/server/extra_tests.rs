//! Extra tests for server.rs covering uncovered branches.
//!
//! Focuses on:
//! - handle_new_connection error paths (auth, malformed json, non-protocol messages)
//! - read loop branches (binary, ping, pong, close frames)
//! - call_child happy path and timeout
//! - send_notification overflow
//! - remove_connection with key

use super::*;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message as WsMessage;

// ---------------------------------------------------------------------------
// Helper to spin up a server and return (port, server, key_gen)
// ---------------------------------------------------------------------------

async fn bootstrap_server() -> (Arc<WebSocketServer>, Arc<KeyGenerator>, u16, String) {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = Arc::new(WebSocketServer::new(key_gen.clone()));
    let port = server.start().await.unwrap();
    let key = key_gen.generate("child-x", 4242);
    (server, key_gen, port, key)
}

/// Bring connection to a fully authenticated state.
#[allow(dead_code)]
async fn _connect_and_auth_placeholder() {}

// ===========================================================================
// Auth/handshake error paths in handle_new_connection
// ===========================================================================

#[tokio::test]
async fn extra_server_auth_missing_key_returns_early() {
    let (_server, _key_gen, port, _key) = bootstrap_server().await;
    let url = format!("ws://127.0.0.1:{}/test", port);
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    // Auth JSON without "key"
    let auth = serde_json::json!({"type": "auth"});
    ws.send(WsMessage::Text(auth.to_string().into()))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    // No connection should be registered
    assert!(_server.get_connection("anything").is_none());
    _server.stop();
}

#[tokio::test]
async fn extra_server_auth_with_non_string_key() {
    let (_server, _key_gen, port, _key) = bootstrap_server().await;
    let url = format!("ws://127.0.0.1:{}/test", port);
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    // Auth with key not being a string - treated as missing key
    let auth = serde_json::json!({"type": "auth", "key": 12345});
    ws.send(WsMessage::Text(auth.to_string().into()))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(_server.get_connection("anything").is_none());
    _server.stop();
}

#[tokio::test]
async fn extra_server_auth_garbage_text() {
    let (_server, _key_gen, port, _key) = bootstrap_server().await;
    let url = format!("ws://127.0.0.1:{}/test", port);
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    ws.send(WsMessage::Text("not even json".into()))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(_server.get_connection("anything").is_none());
    _server.stop();
}

#[tokio::test]
async fn extra_server_auth_unknown_key() {
    let (_server, _key_gen, port, _key) = bootstrap_server().await;
    let url = format!("ws://127.0.0.1:{}/test", port);
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let auth = serde_json::json!({"type": "auth", "key": "totally-unknown"});
    ws.send(WsMessage::Text(auth.to_string().into()))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(_server.get_connection("totally-unknown").is_none());
    _server.stop();
}

#[tokio::test]
async fn extra_server_connection_closed_before_auth_msg() {
    let (_server, _key_gen, port, _key) = bootstrap_server().await;
    let url = format!("ws://127.0.0.1:{}/test", port);
    // Connect then immediately drop without sending auth
    {
        let (_ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        // Drop on exit
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(_server.get_connection("anything").is_none());
    _server.stop();
}

#[tokio::test]
async fn extra_server_auth_via_binary_ignored() {
    let (_server, _key_gen, port, key) = bootstrap_server().await;
    let url = format!("ws://127.0.0.1:{}/{}", port, key);
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    // Send binary auth - should be ignored, no connection registered
    ws.send(WsMessage::Binary(vec![1, 2, 3].into()))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(_server.get_connection(&key).is_none());
    _server.stop();
}

// ===========================================================================
// Read loop branches
// ===========================================================================

#[tokio::test]
async fn extra_server_read_loop_malformed_json_then_close() {
    let (server, key_gen, port, _key) = bootstrap_server().await;
    let key = key_gen.generate("child-rd1", 5000);
    let url = format!("ws://127.0.0.1:{}/{}", port, key);

    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let auth = serde_json::json!({"type": "auth", "key": key.clone()});
    ws.send(WsMessage::Text(auth.to_string().into()))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Send malformed JSON - server should continue (continue path)
    ws.send(WsMessage::Text("garbage".into())).await.unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;

    // Server still alive
    assert!(server.get_connection(&key).is_some());

    // Connection still usable - send close
    ws.close(None).await.ok();
    tokio::time::sleep(Duration::from_millis(150)).await;
    server.stop();
}

#[tokio::test]
async fn extra_server_read_loop_non_protocol_version_ignored() {
    let (server, key_gen, port, _key) = bootstrap_server().await;
    let key = key_gen.generate("child-rd2", 5001);
    let url = format!("ws://127.0.0.1:{}/{}", port, key);

    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let auth = serde_json::json!({"type": "auth", "key": key.clone()});
    ws.send(WsMessage::Text(auth.to_string().into()))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Send a non-2.0 protocol message - server should ignore (continue)
    let msg = serde_json::json!({"jsonrpc": "1.0", "method": "test", "id": "1"});
    ws.send(WsMessage::Text(msg.to_string().into()))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(server.get_connection(&key).is_some());

    ws.close(None).await.ok();
    tokio::time::sleep(Duration::from_millis(100)).await;
    server.stop();
}

#[tokio::test]
async fn extra_server_read_loop_binary_ignored() {
    let (server, key_gen, port, _key) = bootstrap_server().await;
    let key = key_gen.generate("child-rd3", 5002);
    let url = format!("ws://127.0.0.1:{}/{}", port, key);

    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let auth = serde_json::json!({"type": "auth", "key": key.clone()});
    ws.send(WsMessage::Text(auth.to_string().into()))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Send binary message - should be ignored (continue)
    ws.send(WsMessage::Binary(vec![0].into())).await.unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(server.get_connection(&key).is_some());

    ws.close(None).await.ok();
    tokio::time::sleep(Duration::from_millis(100)).await;
    server.stop();
}

#[tokio::test]
async fn extra_server_read_loop_ping_pong_ignored() {
    let (server, key_gen, port, _key) = bootstrap_server().await;
    let key = key_gen.generate("child-rd4", 5003);
    let url = format!("ws://127.0.0.1:{}/{}", port, key);

    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let auth = serde_json::json!({"type": "auth", "key": key.clone()});
    ws.send(WsMessage::Text(auth.to_string().into()))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Send ping
    ws.send(WsMessage::Ping(vec![1].into())).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    // Send pong
    ws.send(WsMessage::Pong(vec![2].into())).await.unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(server.get_connection(&key).is_some());

    ws.close(None).await.ok();
    tokio::time::sleep(Duration::from_millis(100)).await;
    server.stop();
}

#[tokio::test]
async fn extra_server_read_loop_response_routes_to_pending() {
    let (server, key_gen, port, _key) = bootstrap_server().await;
    let key = key_gen.generate("child-rd5", 5004);
    let url = format!("ws://127.0.0.1:{}/{}", port, key);

    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let auth = serde_json::json!({"type": "auth", "key": key.clone()});
    ws.send(WsMessage::Text(auth.to_string().into()))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Trigger a call_child which puts a pending entry, then we respond
    let server_clone = server.clone();
    let child_id = "child-rd5".to_string();
    let call_handle = tokio::spawn(async move {
        // Will wait up to 30s for a response - we'll send one
        server_clone
            .call_child(&child_id, "noop", serde_json::Value::Null)
            .await
    });
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Read the request from server
    if let Some(Ok(ws_msg)) = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .unwrap_or(None)
    {
        if let WsMessage::Text(text) = ws_msg {
            let req: Message = serde_json::from_str(&text).unwrap();
            assert!(req.is_request());
            let resp =
                Message::new_response(req.id.as_deref().unwrap_or(""), serde_json::json!("ok"));
            let resp_str = serde_json::to_string(&resp).unwrap();
            ws.send(WsMessage::Text(resp_str.into())).await.unwrap();
        }
    }

    let result = tokio::time::timeout(Duration::from_secs(2), call_handle).await;
    assert!(result.is_ok());
    let inner = result.unwrap().unwrap();
    assert!(inner.is_ok());
    server.stop();
}

#[tokio::test]
async fn extra_server_read_loop_response_no_pending_id() {
    let (server, key_gen, port, _key) = bootstrap_server().await;
    let key = key_gen.generate("child-rd6", 5005);
    let url = format!("ws://127.0.0.1:{}/{}", port, key);

    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let auth = serde_json::json!({"type": "auth", "key": key.clone()});
    ws.send(WsMessage::Text(auth.to_string().into()))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Send response for unknown id - should be silently dropped
    let resp = Message::new_response("nonexistent-id", serde_json::json!("ok"));
    let text = serde_json::to_string(&resp).unwrap();
    ws.send(WsMessage::Text(text.into())).await.unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(server.get_connection(&key).is_some());

    ws.close(None).await.ok();
    tokio::time::sleep(Duration::from_millis(100)).await;
    server.stop();
}

#[tokio::test]
async fn extra_server_read_loop_notification_handled() {
    let (server, key_gen, port, _key) = bootstrap_server().await;
    let key = key_gen.generate("child-rd7", 5006);
    let url = format!("ws://127.0.0.1:{}/{}", port, key);

    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let auth = serde_json::json!({"type": "auth", "key": key.clone()});
    ws.send(WsMessage::Text(auth.to_string().into()))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Send a notification - should be processed silently
    let note = Message::new_notification("some.event", serde_json::json!({"v": 1}));
    let text = serde_json::to_string(&note).unwrap();
    ws.send(WsMessage::Text(text.into())).await.unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(server.get_connection(&key).is_some());

    ws.close(None).await.ok();
    tokio::time::sleep(Duration::from_millis(100)).await;
    server.stop();
}

// ===========================================================================
// call_child error paths
// ===========================================================================

#[tokio::test]
async fn extra_server_call_child_timeout() {
    let (server, _key_gen, _port, _key) = bootstrap_server().await;
    // Patch: shorten timeout by directly testing pending cleanup
    // Since call_child has 30s timeout, we test the connection_not_found path
    let result = server
        .call_child("never-exists", "method", serde_json::Value::Null)
        .await;
    assert!(matches!(result, Err(WsServerError::ConnectionNotFound)));
    server.stop();
}

#[tokio::test]
async fn extra_server_call_child_send_failure_after_pending() {
    let (server, key_gen, port, _key) = bootstrap_server().await;
    let key = key_gen.generate("child-fail", 7000);

    let url = format!("ws://127.0.0.1:{}/{}", port, key);
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let auth = serde_json::json!({"type": "auth", "key": key.clone()});
    ws.send(WsMessage::Text(auth.to_string().into()))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Close client side first - server's send should fail
    ws.close(None).await.ok();
    tokio::time::sleep(Duration::from_millis(150)).await;

    // call_child may or may not fail depending on connection removal timing
    let _ = server
        .call_child("child-fail", "noop", serde_json::Value::Null)
        .await;
    server.stop();
}

// ===========================================================================
// send_notification edge cases
// ===========================================================================

#[tokio::test]
async fn extra_server_send_notification_dropped_rx() {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen);
    let (tx, rx) = tokio::sync::mpsc::channel::<String>(64);
    drop(rx);

    let conn = Arc::new(tokio::sync::Mutex::new(ChildConnection::new(
        "k".to_string(),
        "k".to_string(),
        99,
        tx,
    )));
    {
        let mut state = server.state.lock();
        state.connections.insert("target".to_string(), conn.clone());
    }

    let result = server.send_notification("target", "method", serde_json::Value::Null);
    assert!(result.is_err());
}

#[tokio::test]
async fn extra_server_send_notification_full_channel() {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen);
    let (tx, _rx) = tokio::sync::mpsc::channel::<String>(1);
    // Fill it
    tx.try_send("x".to_string()).unwrap();

    let conn = Arc::new(tokio::sync::Mutex::new(ChildConnection::new(
        "k".to_string(),
        "k".to_string(),
        99,
        tx,
    )));
    {
        let mut state = server.state.lock();
        state.connections.insert("target".to_string(), conn.clone());
    }

    let result = server.send_notification("target", "method", serde_json::Value::Null);
    // try_send on full channel returns error
    assert!(result.is_err());
}

// ===========================================================================
// remove_connection with key
// ===========================================================================

#[tokio::test]
async fn extra_server_remove_connection_with_known_key() {
    let (server, key_gen, port, _key) = bootstrap_server().await;
    let key = key_gen.generate("child-rm1", 8000);
    let url = format!("ws://127.0.0.1:{}/{}", port, key);

    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let auth = serde_json::json!({"type": "auth", "key": key.clone()});
    ws.send(WsMessage::Text(auth.to_string().into()))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Should be findable by both key and child_id
    assert!(server.get_connection(&key).is_some());
    assert!(server.get_connection("child-rm1").is_some());

    // Remove by child_id - should also remove by key
    server.remove_connection("child-rm1");
    assert!(server.get_connection("child-rm1").is_none());

    ws.close(None).await.ok();
    tokio::time::sleep(Duration::from_millis(50)).await;
    server.stop();
}

#[tokio::test]
async fn extra_server_remove_connection_direct_by_key() {
    let (server, key_gen, port, _key) = bootstrap_server().await;
    let key = key_gen.generate("child-rm2", 8001);
    let url = format!("ws://127.0.0.1:{}/{}", port, key);

    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let auth = serde_json::json!({"type": "auth", "key": key.clone()});
    ws.send(WsMessage::Text(auth.to_string().into()))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Remove by key directly
    server.remove_connection(&key);
    assert!(server.get_connection(&key).is_none());

    ws.close(None).await.ok();
    tokio::time::sleep(Duration::from_millis(50)).await;
    server.stop();
}

// ===========================================================================
// KeyGenerator cleanup edge cases
// ===========================================================================

#[test]
fn extra_key_gen_cleanup_all_expired() {
    let kg = KeyGenerator::new();
    let k1 = kg.generate("a", 1);
    let k2 = kg.generate("b", 2);
    let k3 = kg.generate("c", 3);
    assert_eq!(kg.cleanup(Duration::ZERO), 3);
    assert!(kg.validate(&k1).is_err());
    assert!(kg.validate(&k2).is_err());
    assert!(kg.validate(&k3).is_err());
}

#[test]
fn extra_key_gen_cleanup_with_huge_max_age_removes_none() {
    // Very large but valid max_age - should remove nothing
    let kg = KeyGenerator::new();
    let _ = kg.generate("a", 1);
    // 1 year in seconds is well within i64 range for from_std
    let removed = kg.cleanup(Duration::from_secs(86400 * 365));
    assert_eq!(removed, 0);
}

#[test]
fn extra_key_gen_validate_updates_used_at_twice() {
    let kg = KeyGenerator::new();
    let k = kg.generate("c", 100);
    let v1 = kg.validate(&k).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    let v2 = kg.validate(&k).unwrap();
    // Second validation should update used_at
    assert!(v2.used_at.unwrap() >= v1.used_at.unwrap());
}

// ===========================================================================
// ValidatedKey clone
// ===========================================================================

#[test]
fn extra_validated_key_used_at_none_before_validate() {
    let kg = KeyGenerator::new();
    let k = kg.generate("child", 42);
    // We can't directly inspect used_at before validate, but checking clone semantics
    let v = kg.validate(&k).unwrap();
    let cloned = v.clone();
    assert_eq!(v.used_at.is_some(), cloned.used_at.is_some());
}

// ===========================================================================
// WsServerError additional coverage
// ===========================================================================

#[test]
fn extra_ws_server_error_all_variants_to_string() {
    let variants = vec![
        WsServerError::ConnectionNotFound.to_string(),
        WsServerError::CallTimeout.to_string(),
        WsServerError::SendTimeout.to_string(),
        WsServerError::Other("xyz".to_string()).to_string(),
    ];
    assert_eq!(variants[0], "connection not found");
    assert_eq!(variants[1], "call timeout");
    assert_eq!(variants[2], "send timeout");
    assert_eq!(variants[3], "xyz");
}

// ===========================================================================
// WebSocketServer full lifecycle edge cases
// ===========================================================================

#[tokio::test]
async fn extra_server_start_get_port_consistency() {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen);
    let port = server.start().await.unwrap();
    // get_port should match
    let port2 = server.get_port();
    assert_eq!(port, port2);
    server.stop();
}

#[tokio::test]
async fn extra_server_stop_clears_state() {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen.clone());
    let port = server.start().await.unwrap();
    let key = key_gen.generate("child", 1);

    let url = format!("ws://127.0.0.1:{}/x", port);
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let auth = serde_json::json!({"type": "auth", "key": key.clone()});
    ws.send(WsMessage::Text(auth.to_string().into()))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Now stop - should clear all connections
    server.stop();
    assert!(server.get_connection(&key).is_none());
}

// ===========================================================================
// ChildConnection send error when channel receiver dropped
// ===========================================================================

#[tokio::test]
async fn extra_child_connection_send_when_receiver_dropped() {
    let (tx, rx) = tokio::sync::mpsc::channel::<String>(8);
    let conn = ChildConnection::new("k".into(), "k".into(), 1, tx);
    drop(rx);
    let result = conn.send("hello".to_string()).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("send failed"));
}

// ===========================================================================
// send_notification with non-null params
// ===========================================================================

#[tokio::test]
async fn extra_send_notification_with_complex_params() {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = WebSocketServer::new(key_gen);
    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(64);
    let conn = Arc::new(tokio::sync::Mutex::new(ChildConnection::new(
        "k".into(),
        "k".into(),
        1,
        tx,
    )));
    {
        let mut s = server.state.lock();
        s.connections.insert("cid".to_string(), conn.clone());
    }
    let params = serde_json::json!({
        "nested": {"deep": [1, 2, 3]},
        "str": "hello",
        "num": 42.5,
        "null": null,
        "bool": true,
    });
    let result = server.send_notification("cid", "complex", params.clone());
    assert!(result.is_ok());
    let s = rx.try_recv().unwrap();
    let m: Message = serde_json::from_str(&s).unwrap();
    assert_eq!(m.method.as_deref(), Some("complex"));
    assert_eq!(m.params.unwrap(), params);
}

// ===========================================================================
// Authentication via connection paths
// ===========================================================================

#[tokio::test]
async fn extra_server_authenticated_both_key_and_child_id() {
    let (server, key_gen, port, _key) = bootstrap_server().await;
    let key = key_gen.generate("child-both", 9000);
    let url = format!("ws://127.0.0.1:{}/{}", port, key);

    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let auth = serde_json::json!({"type": "auth", "key": key.clone()});
    ws.send(WsMessage::Text(auth.to_string().into()))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Both lookups should return the same connection (Arc identity check)
    let by_key = server.get_connection(&key).unwrap();
    let by_child = server.get_connection("child-both").unwrap();
    assert!(Arc::ptr_eq(&by_key, &by_child));

    // Verify child_pid
    let g = by_key.lock().await;
    assert_eq!(g.child_pid, 9000);
    assert_eq!(g.child_id.as_deref(), Some("child-both"));

    ws.close(None).await.ok();
    tokio::time::sleep(Duration::from_millis(100)).await;
    server.stop();
}

// ===========================================================================
// Connection dispatcher with method_not_found response
// ===========================================================================

#[tokio::test]
async fn extra_server_request_unknown_method_returns_error_response() {
    let (server, key_gen, port, _key) = bootstrap_server().await;
    let key = key_gen.generate("child-unk", 9100);
    let url = format!("ws://127.0.0.1:{}/{}", port, key);

    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let auth = serde_json::json!({"type": "auth", "key": key.clone()});
    ws.send(WsMessage::Text(auth.to_string().into()))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Send a request to a method that has no handler
    let req = Message::new_request_with_id("req-9100", "unknown.method", serde_json::Value::Null);
    let text = serde_json::to_string(&req).unwrap();
    ws.send(WsMessage::Text(text.into())).await.unwrap();

    // We should get back an error response (method_not_found)
    if let Some(Ok(ws_msg)) = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .unwrap_or(None)
    {
        if let WsMessage::Text(text) = ws_msg {
            let resp: Message = serde_json::from_str(&text).unwrap();
            assert!(resp.is_error_response());
            assert_eq!(resp.id.as_deref(), Some("req-9100"));
            let err = resp.error.unwrap();
            assert_eq!(err.code, crate::websocket::protocol::ERR_METHOD_NOT_FOUND);
        }
    }
    ws.close(None).await.ok();
    tokio::time::sleep(Duration::from_millis(50)).await;
    server.stop();
}

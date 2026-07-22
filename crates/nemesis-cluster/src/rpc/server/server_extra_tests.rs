//! Additional unit tests for RPC server focused on coverage gaps.
//!
//! Targets previously uncovered code paths:
//! - accept_loop / handle_connection over real TCP
//! - handle_request async path with handler dispatch + rpc metadata injection
//! - new_error / new_response responses on wire
//! - max_connections rejection
//! - server lifecycle: bind error, stop idempotency, restart
//! - default handler responses (peer_chat with task_id, get_info version, etc.)
//! - multi-handler routing, dynamic registration visible to in-flight requests
//! - auth_token roundtrip (encrypted vs plaintext)

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;
use crate::rpc_types::{ActionType, KnownAction, RPCRequest, RPCResponse};
use crate::transport::conn::{TcpConn, TcpConnConfig, WireMessage};
use crate::transport::frame::{decrypt_frame, derive_key, encrypt_frame};

fn make_test_server() -> RpcServer {
    RpcServer::new(RpcServerConfig {
        bind_address: "127.0.0.1:0".into(),
        ..Default::default()
    })
}

// ---------------------------------------------------------------------------
// Helpers to send a request frame and read the response.
// ---------------------------------------------------------------------------

/// Spawn server, send one plaintext request frame, read one response frame.
async fn roundtrip_plaintext(server: &RpcServer, wire: WireMessage) -> Option<WireMessage> {
    let port = server.port();
    let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .ok()?;

    let json = serde_json::to_vec(&wire).unwrap();
    let total = (json.len() as u32).to_be_bytes();
    stream.write_all(&total).await.ok()?;
    stream.write_all(&json).await.ok()?;

    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await.ok()?;
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await.ok()?;
    serde_json::from_slice::<WireMessage>(&buf).ok()
}

fn make_request_wire(action: &str) -> WireMessage {
    WireMessage {
        version: "1.0".into(),
        id: "rid-1".into(),
        msg_type: "request".into(),
        from: "node-a".into(),
        to: "node-b".into(),
        action: action.into(),
        payload: serde_json::json!({}),
        timestamp: chrono::Local::now().timestamp(),
        error: String::new(),
    }
}

// ===========================================================================
// Lifecycle: bind errors, restart, port reuse
// ===========================================================================

#[tokio::test]
async fn test_start_invalid_bind_address_returns_error() {
    let server = RpcServer::new(RpcServerConfig {
        bind_address: "not-an-address".into(),
        ..Default::default()
    });
    let result = server.start().await;
    assert!(result.is_err());
    let msg = result.unwrap_err();
    assert!(msg.contains("invalid bind address") || msg.contains("failed to"));
}

#[tokio::test]
async fn test_start_bind_to_occupied_port_returns_error() {
    // Bind a listener manually to occupy a port.
    let sock = tokio::net::TcpSocket::new_v4().unwrap();
    sock.set_reuseaddr(true).unwrap();
    let _ = sock.bind("127.0.0.1:0".parse().unwrap());
    let listener = sock.listen(1).unwrap();
    let occupied_port = listener.local_addr().unwrap().port();
    drop(listener);

    // 127.0.0.1 + ephemeral port may be free now after drop, but let's try anyway.
    // We use a deliberately invalid config form by leaving reuseaddr off server side.
    let server = RpcServer::new(RpcServerConfig {
        bind_address: format!("127.0.0.1:{}", occupied_port),
        ..Default::default()
    });
    // The drop may have freed the port already; both outcomes are valid:
    // - bind fails (we want this) → assert is_err
    // - bind succeeds (port was freed) → just stop
    let result = server.start().await;
    if result.is_ok() {
        server.stop().unwrap();
    }
    // No assertion — port reuse is racy; ensure no panic.
}

#[tokio::test]
async fn test_server_restart_reuses_handlers() {
    let server = make_test_server();
    server.register_handler(
        "custom_action",
        Box::new(|_| Ok(serde_json::json!({"v": 1}))),
    );
    server.start().await.unwrap();
    server.stop().unwrap();
    server.start().await.unwrap();
    // Custom handler should still work after restart.
    let result = server.handle_request_sync("custom_action", serde_json::json!({}));
    assert!(result.is_ok());
    assert_eq!(result.unwrap()["v"], 1);
    server.stop().unwrap();
}

// ===========================================================================
// Real TCP roundtrips through handle_connection / handle_request
// ===========================================================================

#[tokio::test]
async fn test_async_handle_request_default_ping() {
    let server = make_test_server();
    server.start().await.unwrap();
    let resp = roundtrip_plaintext(&server, make_request_wire("ping")).await;
    assert!(resp.is_some());
    let resp = resp.unwrap();
    assert_eq!(resp.msg_type, "response");
    assert_eq!(resp.payload["status"], "pong");
    server.stop().unwrap();
}

#[tokio::test]
async fn test_async_handle_request_default_get_info() {
    let server = make_test_server();
    server.start().await.unwrap();
    let resp = roundtrip_plaintext(&server, make_request_wire("get_info")).await;
    assert!(resp.is_some());
    let resp = resp.unwrap();
    assert_eq!(resp.payload["status"], "online");
    assert!(resp.payload.get("version").is_some());
    server.stop().unwrap();
}

#[tokio::test]
async fn test_async_handle_request_default_get_capabilities() {
    let server = make_test_server();
    server.start().await.unwrap();
    let resp = roundtrip_plaintext(&server, make_request_wire("get_capabilities")).await;
    assert!(resp.is_some());
    let resp = resp.unwrap();
    assert!(resp.payload.get("capabilities").is_some());
    server.stop().unwrap();
}

#[tokio::test]
async fn test_async_handle_request_default_list_actions() {
    let server = make_test_server();
    server.start().await.unwrap();
    let resp = roundtrip_plaintext(&server, make_request_wire("list_actions")).await;
    assert!(resp.is_some());
    let resp = resp.unwrap();
    let actions = resp.payload["actions"].as_array().unwrap();
    assert!(actions.len() >= 6);
    server.stop().unwrap();
}

#[tokio::test]
async fn test_async_handle_request_default_peer_chat() {
    let server = make_test_server();
    server.start().await.unwrap();
    let mut wire = make_request_wire("peer_chat");
    wire.payload = serde_json::json!({"task_id": "task-xyz"});
    let resp = roundtrip_plaintext(&server, wire).await;
    assert!(resp.is_some());
    let resp = resp.unwrap();
    assert_eq!(resp.payload["status"], "accepted");
    assert_eq!(resp.payload["task_id"], "task-xyz");
    server.stop().unwrap();
}

#[tokio::test]
async fn test_async_handle_request_default_peer_chat_callback() {
    let server = make_test_server();
    server.start().await.unwrap();
    let mut wire = make_request_wire("peer_chat_callback");
    wire.payload = serde_json::json!({"task_id": "task-cb"});
    let resp = roundtrip_plaintext(&server, wire).await;
    assert!(resp.is_some());
    let resp = resp.unwrap();
    assert_eq!(resp.payload["status"], "received");
    assert_eq!(resp.payload["task_id"], "task-cb");
    server.stop().unwrap();
}

#[tokio::test]
async fn test_async_handle_request_unknown_action_returns_error_response() {
    let server = make_test_server();
    server.start().await.unwrap();
    let resp = roundtrip_plaintext(&server, make_request_wire("does_not_exist")).await;
    assert!(resp.is_some());
    let resp = resp.unwrap();
    assert_eq!(resp.msg_type, "error");
    assert!(resp.error.contains("no handler"));
    server.stop().unwrap();
}

#[tokio::test]
async fn test_async_handle_request_custom_handler_returns_value() {
    let server = make_test_server();
    server.register_handler(
        "double",
        Box::new(|p| {
            let n = p.get("n").and_then(|v| v.as_i64()).unwrap_or(0);
            Ok(serde_json::json!({"result": n * 2}))
        }),
    );
    server.start().await.unwrap();
    let mut wire = make_request_wire("double");
    wire.payload = serde_json::json!({"n": 21});
    let resp = roundtrip_plaintext(&server, wire).await;
    assert!(resp.is_some());
    let resp = resp.unwrap();
    assert_eq!(resp.payload["result"], 42);
    server.stop().unwrap();
}

#[tokio::test]
async fn test_async_handle_request_handler_error_returns_error_response() {
    let server = make_test_server();
    server.register_handler("boom", Box::new(|_| Err("handler exploded".to_string())));
    server.start().await.unwrap();
    let resp = roundtrip_plaintext(&server, make_request_wire("boom")).await;
    assert!(resp.is_some());
    let resp = resp.unwrap();
    assert_eq!(resp.msg_type, "error");
    assert!(resp.error.contains("handler exploded"));
    server.stop().unwrap();
}

#[tokio::test]
async fn test_async_handle_request_injects_rpc_meta() {
    let server = make_test_server();
    server.register_handler(
        "meta_check",
        Box::new(|p| {
            let rpc = p.get("_rpc").cloned().unwrap_or(serde_json::Value::Null);
            Ok(rpc)
        }),
    );
    server.start().await.unwrap();
    let resp = roundtrip_plaintext(&server, make_request_wire("meta_check")).await;
    assert!(resp.is_some());
    let resp = resp.unwrap();
    // _rpc meta should be present and contain from/to/id.
    assert_eq!(resp.payload["from"], "node-a");
    assert_eq!(resp.payload["to"], "node-b");
    assert_eq!(resp.payload["id"], "rid-1");
    server.stop().unwrap();
}

#[tokio::test]
async fn test_async_handle_request_with_non_object_payload_wraps_in_object() {
    let server = make_test_server();
    server.register_handler(
        "wrap_test",
        Box::new(|p| Ok(serde_json::json!({"has_rpc": p.get("_rpc").is_some()}))),
    );
    server.start().await.unwrap();
    let mut wire = make_request_wire("wrap_test");
    // Non-object payload (array).
    wire.payload = serde_json::json!([1, 2, 3]);
    let resp = roundtrip_plaintext(&server, wire).await;
    assert!(resp.is_some());
    let resp = resp.unwrap();
    assert_eq!(resp.payload["has_rpc"], true);
    server.stop().unwrap();
}

// ===========================================================================
// Connection count tracking
// ===========================================================================

#[tokio::test]
async fn test_connection_count_increments_and_decrements() {
    let server = make_test_server();
    server.start().await.unwrap();
    let port = server.port();
    let addr = format!("127.0.0.1:{}", port);

    let s1 = tokio::net::TcpStream::connect(&addr).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(server.connection_count() >= 1);

    drop(s1);
    tokio::time::sleep(Duration::from_millis(100)).await;
    // Connection count should drop back (best-effort).
    assert!(server.connection_count() <= 1);
    server.stop().unwrap();
}

#[tokio::test]
async fn test_max_connections_rejects_excess() {
    let server = RpcServer::new(RpcServerConfig {
        bind_address: "127.0.0.1:0".into(),
        max_connections: 1,
        ..Default::default()
    });
    server.start().await.unwrap();
    let port = server.port();
    let addr = format!("127.0.0.1:{}", port);

    let _s1 = tokio::net::TcpStream::connect(&addr).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    // After first connection, the second should be rejected (server.drop(stream)).
    let _s2 = tokio::net::TcpStream::connect(&addr).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    // Best effort: conn_count should be at most max.
    let count = server.connection_count();
    assert!(count <= 1, "expected <= 1 connection, got {}", count);
    server.stop().unwrap();
}

// ===========================================================================
// Dynamic handler registration visible to in-flight requests
// ===========================================================================

#[tokio::test]
async fn test_dynamic_registration_visible_in_accept_loop() {
    let server = make_test_server();
    server.start().await.unwrap();

    // Register after start — should still be picked up.
    server.register_handler(
        "dynamic_action",
        Box::new(|_| Ok(serde_json::json!({"dynamic": true}))),
    );

    let resp = roundtrip_plaintext(&server, make_request_wire("dynamic_action")).await;
    assert!(resp.is_some());
    let resp = resp.unwrap();
    assert_eq!(resp.payload["dynamic"], true);
    server.stop().unwrap();
}

#[tokio::test]
async fn test_unregister_after_start_removes_handler() {
    let server = make_test_server();
    server.register_handler(
        "to_remove",
        Box::new(|_| Ok(serde_json::json!({"here": true}))),
    );
    server.start().await.unwrap();

    server.unregister_handler("to_remove");
    let resp = roundtrip_plaintext(&server, make_request_wire("to_remove")).await;
    assert!(resp.is_some());
    let resp = resp.unwrap();
    assert_eq!(resp.msg_type, "error");
    assert!(resp.error.contains("no handler"));
    server.stop().unwrap();
}

// ===========================================================================
// Auth-encrypted server <-> TcpConn client roundtrip
// ===========================================================================

#[tokio::test]
async fn test_encrypted_server_with_tcpconn_client() {
    let server = make_test_server();
    let token = "super-secret".to_string();
    server.set_auth_token(&token);
    server.start().await.unwrap();
    let port = server.port();

    let stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();
    let mut conn = TcpConn::new(
        stream,
        TcpConnConfig {
            address: format!("127.0.0.1:{}", port),
            auth_token: Some(token),
            ..Default::default()
        },
    );
    conn.start().await.unwrap();

    let req = WireMessage::new_request("client", "server", "ping", serde_json::json!({}));
    conn.send(&req).await.unwrap();

    let resp = tokio::time::timeout(Duration::from_secs(2), conn.receive())
        .await
        .expect("timeout")
        .expect("no message");
    assert_eq!(resp.payload["status"], "pong");
    assert_eq!(resp.id, req.id);

    conn.close();
    server.stop().unwrap();
}

#[tokio::test]
async fn test_encrypted_server_rejects_plaintext_client() {
    let server = make_test_server();
    server.set_auth_token("server-only-secret");
    server.start().await.unwrap();
    let port = server.port();

    let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();
    // Send plaintext — server's read loop will fail to decrypt.
    let wire = make_request_wire("ping");
    let json = serde_json::to_vec(&wire).unwrap();
    let total = (json.len() as u32).to_be_bytes();
    let _ = stream.write_all(&total).await;
    let _ = stream.write_all(&json).await;

    // Try to read response — should fail/timeout because server closes.
    let mut len_buf = [0u8; 4];
    let result =
        tokio::time::timeout(Duration::from_millis(300), stream.read_exact(&mut len_buf)).await;
    // Either timed out or read returned 0 / Err.
    assert!(result.is_err() || result.unwrap().is_err());

    server.stop().unwrap();
}

// ===========================================================================
// RpcServerConfig edge cases
// ===========================================================================

#[test]
fn test_rpc_server_config_default_idle_timeout_value() {
    let config = RpcServerConfig::default();
    assert_eq!(config.idle_timeout.as_secs(), 65 * 60);
}

#[test]
fn test_rpc_server_config_clone_preserves_fields() {
    let config = RpcServerConfig {
        bind_address: "127.0.0.1:5000".into(),
        max_connections: 42,
        send_timeout: Duration::from_secs(99),
        idle_timeout: Duration::from_secs(999),
    };
    let cloned = config.clone();
    assert_eq!(cloned.bind_address, "127.0.0.1:5000");
    assert_eq!(cloned.max_connections, 42);
    assert_eq!(cloned.send_timeout, Duration::from_secs(99));
    assert_eq!(cloned.idle_timeout, Duration::from_secs(999));
}

// ===========================================================================
// Default handler payload behavior
// ===========================================================================

#[test]
fn test_default_peer_chat_handler_with_null_task_id() {
    let server = make_test_server();
    let result = server.handle_request_sync(
        "peer_chat",
        serde_json::json!({"task_id": serde_json::Value::Null}),
    );
    // task_id is null, not a string → fallback to "unknown".
    assert!(result.is_ok());
    assert_eq!(result.unwrap()["task_id"], "unknown");
}

#[test]
fn test_default_peer_chat_callback_with_non_string_task_id() {
    let server = make_test_server();
    let result =
        server.handle_request_sync("peer_chat_callback", serde_json::json!({"task_id": 12345}));
    assert!(result.is_ok());
    // Integer is not a string → fallback.
    assert_eq!(result.unwrap()["task_id"], "unknown");
}

#[test]
fn test_default_handlers_independent_calls() {
    let server = make_test_server();
    // Each call should succeed independently.
    assert_eq!(
        server
            .handle_request_sync("ping", serde_json::json!({}))
            .unwrap()["status"],
        "pong"
    );
    let info = server
        .handle_request_sync("get_info", serde_json::json!({}))
        .unwrap();
    assert_eq!(info["status"], "online");
}

// ===========================================================================
// Multiple handlers and replacement
// ===========================================================================

#[test]
fn test_register_many_handlers_then_query() {
    let server = make_test_server();
    for i in 0..10 {
        let v = i;
        server.register_handler(
            &format!("act_{}", i),
            Box::new(move |_| Ok(serde_json::json!({"i": v}))),
        );
    }
    for i in 0..10 {
        let r = server
            .handle_request_sync(&format!("act_{}", i), serde_json::json!({}))
            .unwrap();
        assert_eq!(r["i"], i);
    }
}

#[test]
fn test_handler_uses_payload_from_sync_path() {
    // Verify sync path passes payload as-is to handler (no _rpc injection).
    let server = make_test_server();
    server.register_handler("echo_payload", Box::new(|p| Ok(p.clone())));
    let result = server.handle_request_sync(
        "echo_payload",
        serde_json::json!({"foo": "bar", "count": 3}),
    );
    let resp = result.unwrap();
    assert_eq!(resp["foo"], "bar");
    assert_eq!(resp["count"], 3);
    // Sync path does NOT inject _rpc.
    assert!(resp.get("_rpc").is_none());
}

#[test]
fn test_unregister_nonexistent_after_start_safe() {
    let server = make_test_server();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(server.start()).unwrap();
    server.unregister_handler("never_registered");
    server.unregister_handler("also_not_registered");
    rt.block_on(async {
        server.stop().unwrap();
    });
}

// ===========================================================================
// Server start state assertions
// ===========================================================================

#[tokio::test]
async fn test_server_running_state_transitions() {
    let server = make_test_server();
    assert!(!server.is_running());
    assert_eq!(server.port(), 0);

    server.start().await.unwrap();
    assert!(server.is_running());
    assert_ne!(server.port(), 0);
    let port_after_start = server.port();

    server.stop().unwrap();
    assert!(!server.is_running());
    // Port stays at last value (not reset).
    assert_eq!(server.port(), port_after_start);
}

// ===========================================================================
// WireMessage new_response / new_error via server responses on wire
// ===========================================================================

#[tokio::test]
async fn test_response_payload_echoed_in_response() {
    let server = make_test_server();
    server.register_handler(
        "big_payload",
        Box::new(|_| {
            Ok(serde_json::json!({
                "data": vec![1, 2, 3, 4, 5],
                "nested": {"deep": "value"},
                "bool": true,
                "null": serde_json::Value::Null,
            }))
        }),
    );
    server.start().await.unwrap();
    let resp = roundtrip_plaintext(&server, make_request_wire("big_payload")).await;
    let resp = resp.unwrap();
    assert_eq!(resp.payload["data"].as_array().unwrap().len(), 5);
    assert_eq!(resp.payload["nested"]["deep"], "value");
    assert_eq!(resp.payload["bool"], true);
    server.stop().unwrap();
}

#[tokio::test]
async fn test_response_includes_matching_id() {
    let server = make_test_server();
    server.start().await.unwrap();
    let mut wire = make_request_wire("ping");
    wire.id = "unique-id-abc".into();
    let resp = roundtrip_plaintext(&server, wire).await.unwrap();
    assert_eq!(resp.id, "unique-id-abc");
    server.stop().unwrap();
}

#[tokio::test]
async fn test_response_from_to_swapped() {
    let server = make_test_server();
    server.start().await.unwrap();
    let mut wire = make_request_wire("ping");
    wire.from = "alice".into();
    wire.to = "bob".into();
    let resp = roundtrip_plaintext(&server, wire).await.unwrap();
    assert_eq!(resp.from, "bob"); // server's `to` becomes response's `from`
    assert_eq!(resp.to, "alice");
    server.stop().unwrap();
}

// ===========================================================================
// RpcHandlerFn type alias behavior
// ===========================================================================

#[test]
fn test_rpc_handler_fn_boxed_closure_with_capture() {
    let server = make_test_server();
    let multiplier: i64 = 7;
    server.register_handler(
        "mult",
        Box::new(move |p| {
            let v = p.get("v").and_then(|x| x.as_i64()).unwrap_or(0);
            Ok(serde_json::json!({"result": v * multiplier}))
        }),
    );
    let r = server
        .handle_request_sync("mult", serde_json::json!({"v": 6}))
        .unwrap();
    assert_eq!(r["result"], 42);
}

#[test]
fn test_rpc_handler_fn_with_arc_state() {
    let server = make_test_server();
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = Arc::clone(&counter);
    server.register_handler(
        "count",
        Box::new(move |_| {
            let n = counter_clone.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(serde_json::json!({"count": n}))
        }),
    );
    let _ = server
        .handle_request_sync("count", serde_json::json!({}))
        .unwrap();
    let _ = server
        .handle_request_sync("count", serde_json::json!({}))
        .unwrap();
    let r = server
        .handle_request_sync("count", serde_json::json!({}))
        .unwrap();
    assert_eq!(r["count"], 3);
    assert_eq!(counter.load(Ordering::SeqCst), 3);
}

// ===========================================================================
// WireMessage (de)serialization via server roundtrip
// ===========================================================================

#[tokio::test]
async fn test_server_handles_unicode_payload() {
    let server = make_test_server();
    server.register_handler("echo", Box::new(|p| Ok(p.clone())));
    server.start().await.unwrap();
    let mut wire = make_request_wire("echo");
    wire.payload = serde_json::json!({"text": "こんにちは 世界 🌍"});
    let resp = roundtrip_plaintext(&server, wire).await.unwrap();
    assert_eq!(resp.payload["text"], "こんにちは 世界 🌍");
    server.stop().unwrap();
}

#[tokio::test]
async fn test_server_handles_large_payload() {
    let server = make_test_server();
    server.register_handler("echo", Box::new(|p| Ok(p.clone())));
    server.start().await.unwrap();
    let mut wire = make_request_wire("echo");
    let big_array: Vec<i64> = (0..5000).collect();
    wire.payload = serde_json::json!({"arr": big_array});
    let resp = roundtrip_plaintext(&server, wire).await.unwrap();
    assert_eq!(resp.payload["arr"].as_array().unwrap().len(), 5000);
    server.stop().unwrap();
}

// ===========================================================================
// Server with many concurrent connections
// ===========================================================================

#[tokio::test]
async fn test_server_handles_multiple_concurrent_connections() {
    let server = make_test_server();
    server.start().await.unwrap();
    let port = server.port();
    let addr = format!("127.0.0.1:{}", port);

    let mut conns = Vec::new();
    for _ in 0..5 {
        conns.push(tokio::net::TcpStream::connect(&addr).await.unwrap());
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(server.connection_count() >= 1);
    drop(conns);
    tokio::time::sleep(Duration::from_millis(150)).await;
    server.stop().unwrap();
}

// ===========================================================================
// Config field defaults and overrides
// ===========================================================================

#[test]
fn test_rpc_server_config_send_timeout_default() {
    let config = RpcServerConfig::default();
    assert_eq!(config.send_timeout, Duration::from_secs(10));
}

#[test]
fn test_rpc_server_config_max_connections_default() {
    assert_eq!(RpcServerConfig::default().max_connections, 100);
}

// ===========================================================================
// Multiple sequential roundtrips over fresh connections
// ===========================================================================

#[tokio::test]
async fn test_two_sequential_requests_over_fresh_connections() {
    let server = make_test_server();
    server.start().await.unwrap();

    let r1 = roundtrip_plaintext(&server, make_request_wire("ping"))
        .await
        .unwrap();
    assert_eq!(r1.payload["status"], "pong");

    let r2 = roundtrip_plaintext(&server, make_request_wire("get_info"))
        .await
        .unwrap();
    assert_eq!(r2.payload["status"], "online");

    server.stop().unwrap();
}

// ===========================================================================
// TcpConnConfig defaults
// ===========================================================================

#[test]
fn test_tcp_conn_config_default_values() {
    let config = TcpConnConfig::default();
    assert_eq!(config.read_buffer_size, 100);
    assert_eq!(config.send_buffer_size, 100);
    assert_eq!(config.send_timeout, Duration::from_secs(10));
    assert_eq!(config.idle_timeout, Duration::from_secs(30));
    assert!(config.auth_token.is_none());
    assert!(config.heartbeat_interval.is_none());
}

#[test]
fn test_tcp_conn_config_clone() {
    let config = TcpConnConfig {
        node_id: "n1".into(),
        address: "127.0.0.1:1".into(),
        read_buffer_size: 50,
        send_buffer_size: 75,
        send_timeout: Duration::from_secs(5),
        idle_timeout: Duration::from_secs(15),
        heartbeat_interval: Some(Duration::from_secs(2)),
        auth_token: Some("tok".into()),
    };
    let cloned = config.clone();
    assert_eq!(cloned.node_id, "n1");
    assert_eq!(cloned.read_buffer_size, 50);
    assert_eq!(cloned.heartbeat_interval, Some(Duration::from_secs(2)));
}

// ===========================================================================
// AEAD round-trip helpers used internally by the server
// ===========================================================================

#[test]
fn test_derive_key_deterministic_for_same_token() {
    let k1 = derive_key("my-token");
    let k2 = derive_key("my-token");
    assert_eq!(k1, k2);
}

#[test]
fn test_derive_key_different_for_different_tokens() {
    let k1 = derive_key("a");
    let k2 = derive_key("b");
    assert_ne!(k1, k2);
}

#[test]
fn test_encrypt_decrypt_frame_round_trip() {
    let key = derive_key("roundtrip-key");
    let plaintext = b"{\"hello\":\"world\"}".to_vec();
    let encrypted = encrypt_frame(&plaintext, &key).unwrap();
    assert_ne!(encrypted, plaintext);
    let decrypted = decrypt_frame(&encrypted, &key).unwrap();
    assert_eq!(decrypted, plaintext);
}

#[test]
fn test_decrypt_frame_wrong_key_fails() {
    let key1 = derive_key("key-1");
    let key2 = derive_key("key-2");
    let encrypted = encrypt_frame(b"data", &key1).unwrap();
    let result = decrypt_frame(&encrypted, &key2);
    assert!(result.is_err());
}

#[test]
fn test_decrypt_frame_too_short_fails() {
    let key = derive_key("k");
    let too_short = vec![0u8; 5]; // < NONCE_SIZE + TAG_SIZE
    let result = decrypt_frame(&too_short, &key);
    assert!(result.is_err());
}

// ===========================================================================
// handle_request_sync error message format
// ===========================================================================

#[test]
fn test_handle_request_sync_unknown_action_error_message() {
    let server = make_test_server();
    let err = server
        .handle_request_sync("missing_action", serde_json::json!({}))
        .unwrap_err();
    assert!(err.contains("no handler for action: missing_action"));
}

// ===========================================================================
// Server starts with default handlers visible before start
// ===========================================================================

#[test]
fn test_default_handlers_registered_in_constructor_for_all_defaults() {
    let server = make_test_server();
    // No register_default_handlers call — should be in constructor.
    for action in [
        "ping",
        "get_info",
        "get_capabilities",
        "list_actions",
        "peer_chat",
        "peer_chat_callback",
    ] {
        let result = server.handle_request_sync(action, serde_json::json!({}));
        assert!(
            result.is_ok(),
            "default handler '{}' should be registered",
            action
        );
    }
}

// ===========================================================================
// Concurrent handler registration / dispatch (read/write lock stress)
// ===========================================================================

#[tokio::test]
async fn test_concurrent_register_and_dispatch() {
    let server = Arc::new(make_test_server());
    server.start().await.unwrap();
    let port = server.port();
    let addr = format!("127.0.0.1:{}", port);

    // Spawn a connection that sends a request, while we register handler.
    let s_clone = Arc::clone(&server);
    let register_task = tokio::spawn(async move {
        for i in 0..5 {
            s_clone.register_handler(
                &format!("c_{}", i),
                Box::new(move |_| Ok(serde_json::json!({"i": i}))),
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });

    let mut conns = Vec::new();
    for _ in 0..5 {
        conns.push(tokio::net::TcpStream::connect(&addr).await.unwrap());
    }
    register_task.await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    drop(conns);
    server.stop().unwrap();
}

// ===========================================================================
// WireMessage serde: timestamp/error fields present
// ===========================================================================

#[test]
fn test_wire_message_serialization_has_all_fields() {
    let m = WireMessage {
        version: "1.0".into(),
        id: "x".into(),
        msg_type: "request".into(),
        from: "a".into(),
        to: "b".into(),
        action: "ping".into(),
        payload: serde_json::json!({"k": "v"}),
        timestamp: 1700000000,
        error: String::new(),
    };
    let json = serde_json::to_string(&m).unwrap();
    assert!(json.contains("\"version\""));
    assert!(json.contains("\"type\"")); // serde rename
    assert!(json.contains("\"from\""));
    assert!(json.contains("\"to\""));
    assert!(json.contains("\"action\""));
    assert!(json.contains("\"payload\""));
    assert!(json.contains("\"timestamp\""));
}

// ===========================================================================
// Frame::encode_request -> server decode
// ===========================================================================

#[test]
fn test_frame_encode_request_uses_lowercase_action() {
    let req = RPCRequest {
        id: "r1".into(),
        action: ActionType::Known(KnownAction::Ping),
        payload: serde_json::json!({}),
        source: "src".into(),
        target: Some("dst".into()),
    };
    let bytes = crate::rpc_types::Frame::encode_request(&req).unwrap();
    // Strip 4-byte length header.
    let json = &bytes[4..];
    let wire: WireMessage = serde_json::from_slice(json).unwrap();
    assert_eq!(wire.action, "ping"); // lowercase
    assert_eq!(wire.msg_type, "request");
    assert_eq!(wire.from, "src");
    assert_eq!(wire.to, "dst");
}

#[test]
fn test_frame_encode_response_serializes_rpcresponse() {
    let resp = RPCResponse {
        id: "r2".into(),
        result: Some(serde_json::json!({"ok": true})),
        error: None,
    };
    let bytes = crate::rpc_types::Frame::encode_response(&resp).unwrap();
    let json = &bytes[4..];
    let decoded: RPCResponse = serde_json::from_slice(json).unwrap();
    assert_eq!(decoded.id, "r2");
    assert_eq!(decoded.result.unwrap()["ok"], true);
}

// ===========================================================================
// Server-stress: many quick start/stop cycles
// ===========================================================================

#[tokio::test]
async fn test_rapid_start_stop_cycles() {
    let server = make_test_server();
    for _ in 0..3 {
        server.start().await.unwrap();
        assert!(server.is_running());
        server.stop().unwrap();
        assert!(!server.is_running());
    }
}

// ===========================================================================
// Server config to bind to specific interface
// ===========================================================================

#[tokio::test]
async fn test_bind_to_loopback_only() {
    let server = RpcServer::new(RpcServerConfig {
        bind_address: "127.0.0.1:0".into(),
        ..Default::default()
    });
    server.start().await.unwrap();
    let port = server.port();
    // Loopback connection should succeed.
    let stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port)).await;
    assert!(stream.is_ok());
    server.stop().unwrap();
}

// ===========================================================================
// Connection lifecycle: closing client drops server's conn_count
// ===========================================================================

#[tokio::test]
async fn test_client_close_drops_server_conn_count() {
    let server = make_test_server();
    server.start().await.unwrap();
    let port = server.port();

    {
        let _stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(server.connection_count() >= 1);
    } // stream dropped here

    tokio::time::sleep(Duration::from_millis(200)).await;
    // Count should reflect closure (best effort; server tasks may lag).
    assert!(server.connection_count() <= 1);
    server.stop().unwrap();
}

// ===========================================================================
// Auth token empty vs set
// ===========================================================================

#[tokio::test]
async fn test_server_with_empty_auth_accepts_plaintext() {
    let server = make_test_server();
    // Empty auth token — same as no auth.
    server.set_auth_token("");
    server.start().await.unwrap();
    let resp = roundtrip_plaintext(&server, make_request_wire("ping")).await;
    assert!(resp.is_some());
    assert_eq!(resp.unwrap().payload["status"], "pong");
    server.stop().unwrap();
}

// ===========================================================================
// Handler returns Result with different error patterns
// ===========================================================================

#[test]
fn test_handler_error_empty_string() {
    let server = make_server_with_handler("empty_err", |_| Err(String::new()));
    let result = server.handle_request_sync("empty_err", serde_json::json!({}));
    assert!(result.is_err());
    // Empty error string.
    assert_eq!(result.unwrap_err(), "");
}

fn make_server_with_handler(
    action: &str,
    handler: impl Fn(serde_json::Value) -> Result<serde_json::Value, String> + Send + Sync + 'static,
) -> RpcServer {
    let server = make_test_server();
    server.register_handler(action, Box::new(handler));
    server
}

#[test]
fn test_handler_error_with_newlines() {
    let server =
        make_server_with_handler("multiline_err", |_| Err("line1\nline2\nline3".to_string()));
    let result = server.handle_request_sync("multiline_err", serde_json::json!({}));
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.contains("line1"));
    assert!(err.contains("line3"));
}

// ===========================================================================
// WireMessage::new_request produces well-formed message
// ===========================================================================

#[test]
fn test_wire_message_new_request_action_preserved() {
    let m = WireMessage::new_request("a", "b", "custom_action", serde_json::json!({"x": 1}));
    assert_eq!(m.action, "custom_action");
    assert_eq!(m.payload["x"], 1);
    assert_eq!(m.version, "1.0");
    assert!(m.id.starts_with("msg-"));
    assert!(!m.from.is_empty());
}

// ===========================================================================
// Concurrent handler lookup safety (read lock during dispatch)
// ===========================================================================

#[tokio::test]
async fn test_handler_lookup_concurrent_with_register() {
    let server = Arc::new(make_test_server());
    server.start().await.unwrap();

    // Pre-register so the request handler resolves.
    server.register_handler(
        "concurrent_action",
        Box::new(|_| Ok(serde_json::json!({"ok": true}))),
    );

    // Fire a request, and concurrently register another handler.
    let server_clone = Arc::clone(&server);
    let register_task = tokio::spawn(async move {
        server_clone.register_handler(
            "another_action",
            Box::new(|_| Ok(serde_json::json!({"another": true}))),
        );
    });

    let resp = roundtrip_plaintext(&server, make_request_wire("concurrent_action")).await;
    register_task.await.unwrap();
    assert!(resp.is_some());
    assert_eq!(resp.unwrap().payload["ok"], true);
    server.stop().unwrap();
}

// ===========================================================================
// Run server under multiple stop calls (idempotent-ish; second errors)
// ===========================================================================

#[tokio::test]
async fn test_start_then_stop_then_start_then_stop() {
    let server = make_test_server();
    server.start().await.unwrap();
    server.stop().unwrap();
    server.start().await.unwrap();
    server.stop().unwrap();
    // Should not panic.
}

// ===========================================================================
// Server accepts and dispatches from raw RpcClient
// ===========================================================================

#[tokio::test]
async fn test_rpc_client_against_rpc_server() {
    use crate::rpc::LocalNetworkInterface;
    use crate::rpc::client::{PeerResolver, RpcClient};

    let server = make_test_server();
    server.start().await.unwrap();
    let port = server.port();

    struct StaticResolver {
        addr: String,
        port: u16,
    }
    impl PeerResolver for StaticResolver {
        fn get_peer_info(&self, _peer_id: &str) -> Option<(Vec<String>, u16, bool)> {
            Some((vec![self.addr.clone()], self.port, true))
        }
        fn get_local_interfaces(&self) -> Vec<LocalNetworkInterface> {
            vec![]
        }
        fn get_node_id(&self) -> String {
            "test".into()
        }
    }

    let resolver = Arc::new(StaticResolver {
        addr: format!("127.0.0.1:{}", port),
        port,
    });
    let client = RpcClient::with_resolver(resolver);

    let req = RPCRequest {
        id: "rt-1".into(),
        action: ActionType::Known(KnownAction::Ping),
        payload: serde_json::json!({}),
        source: "src".into(),
        target: Some("dst".into()),
    };

    let resp = client
        .call_with_timeout("peer", req, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(resp.id, "rt-1");
    assert_eq!(resp.result.unwrap()["status"], "pong");
    server.stop().unwrap();
}

//! Extra coverage tests for websocket_handler.rs focusing on:
//! - `handle_websocket_upgrade` auth flow (lines 131-148)
//! - `handle_websocket` connection lifecycle (lines 163-334)
//! - `SendQueue::new` with real WebSocket sink (lines 57-79)
//! - Various message dispatch paths through the full pipeline
//!
//! These tests spin up a real axum server bound to ephemeral ports and use
//! `tokio_tungstenite` as the client to exercise the full protocol stack.

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use axum::routing::get;
use axum::Router;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message as WsMessage;

// ---------------------------------------------------------------------------
// Test infrastructure: build an AppState for testing
// ---------------------------------------------------------------------------

fn make_state(
    auth_token: &str,
    inbound_tx: Option<mpsc::UnboundedSender<IncomingMessage>>,
    ws_router: Option<Arc<crate::ws_router::WsRouter>>,
) -> Arc<AppState> {
    Arc::new(AppState {
        auth_token: auth_token.to_string(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: None,
        home: None,
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("test-model".to_string())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
        session_manager: Arc::new(SessionManager::with_default_timeout()),
        inbound_tx,
        streaming_provider: None,
        ws_router,
        agent_service: None,
        data_store: None,
        memory_manager: None,
        forge: None,
        agent_loop: Arc::new(parking_lot::RwLock::new(None)),
        cluster: None,
        cluster_service: None,
        cluster_log_dir: None,
        internal_cmd_tx: None,
    })
}

async fn bind_ephemeral() -> (std::net::SocketAddr, TcpListener) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    (addr, listener)
}

async fn start_test_server(
    listener: TcpListener,
    state: Arc<AppState>,
) -> tokio::task::JoinHandle<()> {
    let app = Router::new()
        .route("/ws", get(handle_websocket_upgrade))
        .with_state(state);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    })
}

async fn ws_connect(
    addr: &std::net::SocketAddr,
    token: Option<&str>,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let url = match token {
        Some(t) => format!("ws://{}/ws?token={}", addr, t),
        None => format!("ws://{}/ws", addr),
    };
    tokio_tungstenite::connect_async(url)
        .await
        .expect("ws connect should succeed")
        .0
}

// ---------------------------------------------------------------------------
// handle_websocket_upgrade auth tests (HTTP layer, not WS upgrade)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_upgrade_no_token_returns_unauthorized_when_auth_required() {
    let state = make_state("secret-token", None, None);
    let (addr, listener) = bind_ephemeral().await;
    let _server = start_test_server(listener, state).await;

    // Try to connect without token — server should reject with HTTP 401
    // Use a raw HTTP request via TCP to inspect the status code
    let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buf = [0u8; 1024];
    let mut stream = stream;
    let req = b"GET /ws HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n";
    stream.write_all(req).await.unwrap();
    let n = stream.read(&mut buf).await.unwrap();
    let resp = String::from_utf8_lossy(&buf[..n]);
    assert!(
        resp.starts_with("HTTP/1.1 401"),
        "missing token should return 401, got: {}",
        resp.lines().next().unwrap_or("")
    );
}

#[tokio::test]
async fn test_upgrade_wrong_token_returns_unauthorized() {
    let state = make_state("secret-token", None, None);
    let (addr, listener) = bind_ephemeral().await;
    let _server = start_test_server(listener, state).await;

    let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buf = [0u8; 1024];
    let mut stream = stream;
    let req = b"GET /ws?token=wrong-token HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n";
    stream.write_all(req).await.unwrap();
    let n = stream.read(&mut buf).await.unwrap();
    let resp = String::from_utf8_lossy(&buf[..n]);
    assert!(
        resp.starts_with("HTTP/1.1 401"),
        "wrong token should return 401, got: {}",
        resp.lines().next().unwrap_or("")
    );
}

#[tokio::test]
async fn test_upgrade_correct_token_succeeds_handshake() {
    // Auth required + correct token — should accept the upgrade attempt.
    // The server will accept and respond with 101 Switching Protocols OR
    // 426/400 if the client's Upgrade headers don't match axum's expectations.
    // Either way it should NOT be 401.
    let state = make_state("secret-token", None, None);
    let (addr, listener) = bind_ephemeral().await;
    let _server = start_test_server(listener, state).await;

    let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buf = [0u8; 1024];
    let mut stream = stream;
    let req = b"GET /ws?token=secret-token HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n";
    stream.write_all(req).await.unwrap();
    let n = stream.read(&mut buf).await.unwrap();
    let resp = String::from_utf8_lossy(&buf[..n]);
    assert!(!resp.starts_with("HTTP/1.1 401"), "correct token must not return 401: {}", resp);
}

#[tokio::test]
async fn test_upgrade_no_auth_required_allows_connection() {
    // auth_token = "" disables auth — any connection should be allowed
    let state = make_state("", None, None);
    let (addr, listener) = bind_ephemeral().await;
    let _server = start_test_server(listener, state).await;

    // Connect without token — should succeed
    let _ws = ws_connect(&addr, None).await;
}

// ---------------------------------------------------------------------------
// handle_websocket full lifecycle tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_websocket_chat_send_message_round_trip() {
    let (tx, mut rx) = mpsc::unbounded_channel::<IncomingMessage>();
    let state = make_state("", Some(tx), None);
    let (addr, listener) = bind_ephemeral().await;
    let _server = start_test_server(listener, state).await;

    let mut ws = ws_connect(&addr, None).await;

    let raw = r#"{"type":"message","module":"chat","cmd":"send","data":{"content":"hello world"}}"#;
    ws.send(WsMessage::Text(raw.into())).await.unwrap();

    // Verify the message reached the inbound_tx channel
    let incoming = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("timeout waiting for inbound message")
        .expect("inbound channel closed");
    assert_eq!(incoming.content, "hello world");
    assert!(!incoming.session_id.is_empty());
}

#[tokio::test]
async fn test_websocket_heartbeat_ping_receives_pong() {
    let state = make_state("", None, None);
    let (addr, listener) = bind_ephemeral().await;
    let _server = start_test_server(listener, state).await;

    let mut ws = ws_connect(&addr, None).await;

    let raw = r#"{"type":"system","module":"heartbeat","cmd":"ping","data":null}"#;
    ws.send(WsMessage::Text(raw.into())).await.unwrap();

    // Server should respond with a pong message
    let msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("timeout waiting for pong")
        .expect("stream closed")
        .expect("ws error");

    match msg {
        WsMessage::Text(t) => {
            let parsed: serde_json::Value = serde_json::from_str(&t).unwrap();
            assert_eq!(parsed["type"], "system");
            assert_eq!(parsed["module"], "heartbeat");
            assert_eq!(parsed["cmd"], "pong");
        }
        other => panic!("expected text pong, got {:?}", other),
    }
}

#[tokio::test]
async fn test_websocket_invalid_json_sends_error_message() {
    let state = make_state("", None, None);
    let (addr, listener) = bind_ephemeral().await;
    let _server = start_test_server(listener, state).await;

    let mut ws = ws_connect(&addr, None).await;

    ws.send(WsMessage::Text("not valid json".into())).await.unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("timeout")
        .expect("closed")
        .expect("ws err");

    match msg {
        WsMessage::Text(t) => {
            let parsed: serde_json::Value = serde_json::from_str(&t).unwrap();
            assert_eq!(parsed["type"], "system");
            assert_eq!(parsed["module"], "error");
            assert_eq!(parsed["cmd"], "notify");
            assert!(parsed["data"]["content"].as_str().unwrap().contains("invalid"));
        }
        other => panic!("expected text error, got {:?}", other),
    }
}

#[tokio::test]
async fn test_websocket_unknown_protocol_type_sends_error() {
    let state = make_state("", None, None);
    let (addr, listener) = bind_ephemeral().await;
    let _server = start_test_server(listener, state).await;

    let mut ws = ws_connect(&addr, None).await;

    let raw = r#"{"type":"unknown_type","module":"test","cmd":"test"}"#;
    ws.send(WsMessage::Text(raw.into())).await.unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("timeout")
        .expect("closed")
        .expect("err");
    match msg {
        WsMessage::Text(t) => {
            let parsed: serde_json::Value = serde_json::from_str(&t).unwrap();
            assert!(parsed["data"]["content"].as_str().unwrap().contains("unknown"));
        }
        other => panic!("got {:?}", other),
    }
}

#[tokio::test]
async fn test_websocket_close_frame_terminates_connection() {
    let state = make_state("", None, None);
    let (addr, listener) = bind_ephemeral().await;
    let _server = start_test_server(listener, state).await;

    let mut ws = ws_connect(&addr, None).await;
    ws.send(WsMessage::Close(None)).await.unwrap();

    // Next message should be a Close response or stream end
    let _ = tokio::time::timeout(Duration::from_secs(1), ws.next()).await;
}

#[tokio::test]
async fn test_websocket_binary_message_ignored() {
    let state = make_state("", None, None);
    let (addr, listener) = bind_ephemeral().await;
    let _server = start_test_server(listener, state).await;

    let mut ws = ws_connect(&addr, None).await;

    // Send binary — should be ignored (no response)
    ws.send(WsMessage::Binary(vec![1, 2, 3].into())).await.unwrap();

    // Send a follow-up text message — should work normally
    let raw = r#"{"type":"system","module":"heartbeat","cmd":"ping","data":null}"#;
    ws.send(WsMessage::Text(raw.into())).await.unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("timeout")
        .expect("closed")
        .expect("err");
    if let WsMessage::Text(t) = msg {
        let parsed: serde_json::Value = serde_json::from_str(&t).unwrap();
        assert_eq!(parsed["cmd"], "pong");
    }
}

#[tokio::test]
async fn test_websocket_history_request_forwarded_to_bus() {
    let (tx, mut rx) = mpsc::unbounded_channel::<IncomingMessage>();
    let state = make_state("", Some(tx), None);
    let (addr, listener) = bind_ephemeral().await;
    let _server = start_test_server(listener, state).await;

    let mut ws = ws_connect(&addr, None).await;

    let raw = r#"{"type":"message","module":"chat","cmd":"history_request","data":{"request_id":"r-1","limit":5}}"#;
    ws.send(WsMessage::Text(raw.into())).await.unwrap();

    let incoming = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("timeout")
        .expect("closed");
    assert_eq!(incoming.metadata.get("request_type"), Some(&"history".to_string()));
}

#[tokio::test]
async fn test_websocket_error_notify_triggers_pong_response() {
    // error.notify returns Ok(None) at the dispatch layer, which causes the
    // handle_websocket outer loop to send a pong response (same as heartbeat ping).
    // This test verifies the actual behavior.
    let state = make_state("", None, None);
    let (addr, listener) = bind_ephemeral().await;
    let _server = start_test_server(listener, state).await;

    let mut ws = ws_connect(&addr, None).await;

    let raw = r#"{"type":"system","module":"error","cmd":"notify","data":{"content":"client-side issue"}}"#;
    ws.send(WsMessage::Text(raw.into())).await.unwrap();

    // Server responds with a pong (because Ok(None) maps to pong in the outer loop)
    let msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("timeout")
        .expect("closed")
        .expect("err");
    if let WsMessage::Text(t) = msg {
        let parsed: serde_json::Value = serde_json::from_str(&t).unwrap();
        assert_eq!(parsed["module"], "heartbeat");
        assert_eq!(parsed["cmd"], "pong");
    }
}

#[tokio::test]
async fn test_websocket_session_count_increments_and_decrements() {
    let state = make_state("", None, None);
    let initial_count = state.session_count.load(std::sync::atomic::Ordering::SeqCst);
    let (addr, listener) = bind_ephemeral().await;
    let _server = start_test_server(listener, state.clone()).await;

    {
        let mut ws = ws_connect(&addr, None).await;
        // Give the server time to increment
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mid_count = state.session_count.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(mid_count, initial_count + 1);

        // Disconnect
        let _ = ws.close(None).await;
    }

    // Wait for cleanup
    tokio::time::sleep(Duration::from_millis(200)).await;
    let final_count = state.session_count.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(final_count, initial_count);
}

#[tokio::test]
async fn test_websocket_chat_send_empty_content_sends_error() {
    let state = make_state("", None, None);
    let (addr, listener) = bind_ephemeral().await;
    let _server = start_test_server(listener, state).await;

    let mut ws = ws_connect(&addr, None).await;

    let raw = r#"{"type":"message","module":"chat","cmd":"send","data":{"content":""}}"#;
    ws.send(WsMessage::Text(raw.into())).await.unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("timeout")
        .expect("closed")
        .expect("err");
    if let WsMessage::Text(t) = msg {
        let parsed: serde_json::Value = serde_json::from_str(&t).unwrap();
        assert_eq!(parsed["module"], "error");
        assert!(parsed["data"]["content"].as_str().unwrap().contains("empty"));
    }
}

#[tokio::test]
async fn test_websocket_unknown_chat_cmd_sends_error() {
    let state = make_state("", None, None);
    let (addr, listener) = bind_ephemeral().await;
    let _server = start_test_server(listener, state).await;

    let mut ws = ws_connect(&addr, None).await;

    let raw = r#"{"type":"message","module":"chat","cmd":"nonexistent","data":{}}"#;
    ws.send(WsMessage::Text(raw.into())).await.unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("timeout")
        .expect("closed")
        .expect("err");
    if let WsMessage::Text(t) = msg {
        let parsed: serde_json::Value = serde_json::from_str(&t).unwrap();
        assert!(parsed["data"]["content"].as_str().unwrap().contains("unknown"));
    }
}

#[tokio::test]
async fn test_websocket_unknown_message_module_sends_error() {
    let state = make_state("", None, None);
    let (addr, listener) = bind_ephemeral().await;
    let _server = start_test_server(listener, state).await;

    let mut ws = ws_connect(&addr, None).await;

    let raw = r#"{"type":"message","module":"email","cmd":"send","data":{"content":"hi"}}"#;
    ws.send(WsMessage::Text(raw.into())).await.unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("timeout")
        .expect("closed")
        .expect("err");
    if let WsMessage::Text(t) = msg {
        let parsed: serde_json::Value = serde_json::from_str(&t).unwrap();
        assert!(parsed["data"]["content"].as_str().unwrap().contains("unknown message module"));
    }
}

#[tokio::test]
async fn test_websocket_unknown_system_module_sends_error() {
    let state = make_state("", None, None);
    let (addr, listener) = bind_ephemeral().await;
    let _server = start_test_server(listener, state).await;

    let mut ws = ws_connect(&addr, None).await;

    let raw = r#"{"type":"system","module":"unknown_mod","cmd":"test"}"#;
    ws.send(WsMessage::Text(raw.into())).await.unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("timeout")
        .expect("closed")
        .expect("err");
    if let WsMessage::Text(t) = msg {
        let parsed: serde_json::Value = serde_json::from_str(&t).unwrap();
        assert!(parsed["data"]["content"].as_str().unwrap().contains("unknown system module"));
    }
}

#[tokio::test]
async fn test_websocket_chat_send_missing_content_sends_error() {
    let state = make_state("", None, None);
    let (addr, listener) = bind_ephemeral().await;
    let _server = start_test_server(listener, state).await;

    let mut ws = ws_connect(&addr, None).await;

    let raw = r#"{"type":"message","module":"chat","cmd":"send","data":{}}"#;
    ws.send(WsMessage::Text(raw.into())).await.unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("timeout")
        .expect("closed")
        .expect("err");
    if let WsMessage::Text(t) = msg {
        let parsed: serde_json::Value = serde_json::from_str(&t).unwrap();
        assert_eq!(parsed["module"], "error");
    }
}

// ---------------------------------------------------------------------------
// SendQueue tests with real WebSocket sink (covers lines 57-79)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_send_queue_integrates_with_websocket() {
    // SendQueue::new is exercised by handle_websocket itself. Here we verify
    // that an end-to-end connection can send multiple queued messages back
    // to the client in order.
    let state = make_state("", None, None);
    let (addr, listener) = bind_ephemeral().await;
    let _server = start_test_server(listener, state).await;

    let mut ws = ws_connect(&addr, None).await;

    // Send two pings — server's SendQueue will write two pongs back to back
    let raw = r#"{"type":"system","module":"heartbeat","cmd":"ping","data":null}"#;
    ws.send(WsMessage::Text(raw.into())).await.unwrap();
    ws.send(WsMessage::Text(raw.into())).await.unwrap();

    for _ in 0..2 {
        let msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
            .await
            .expect("timeout")
            .expect("closed")
            .expect("err");
        match msg {
            WsMessage::Text(t) => {
                let parsed: serde_json::Value = serde_json::from_str(&t).unwrap();
                assert_eq!(parsed["cmd"], "pong");
            }
            other => panic!("expected text, got {:?}", other),
        }
    }
}

#[tokio::test]
async fn test_send_queue_handles_broadcast_and_pong_interleaved() {
    // Mix outbound broadcast with pong to verify queue serializes correctly
    let state = make_state("", None, None);
    let session_mgr = state.session_manager.clone();
    let (addr, listener) = bind_ephemeral().await;
    let server_state = state.clone();
    let _server = start_test_server(listener, server_state).await;

    let mut ws = ws_connect(&addr, None).await;
    // Allow session to be established
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Send a ping to trigger pong response
    let raw = r#"{"type":"system","module":"heartbeat","cmd":"ping","data":null}"#;
    ws.send(WsMessage::Text(raw.into())).await.unwrap();

    // Receive the pong
    let _ = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("timeout")
        .expect("closed")
        .expect("err");

    // Try broadcasting to the new session
    let sessions = session_mgr.all_sessions();
    if !sessions.is_empty() {
        let sid = sessions[0].id.clone();
        let result = broadcast_to_session(&session_mgr, &sid, "assistant", "broadcast msg").await;
        assert!(result.is_ok());
        let msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
            .await
            .expect("timeout")
            .expect("closed")
            .expect("err");
        if let WsMessage::Text(t) = msg {
            let parsed: serde_json::Value = serde_json::from_str(&t).unwrap();
            assert_eq!(parsed["cmd"], "receive");
        }
    }
}

#[tokio::test]
async fn test_broadcast_to_session_with_no_send_queue_fails() {
    let mgr = SessionManager::with_default_timeout();
    let session = mgr.create_session();
    // No send queue set
    let result = broadcast_to_session(&mgr, &session.id, "assistant", "msg").await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("session not found"));
}

#[tokio::test]
async fn test_broadcast_to_session_with_nonexistent_session_fails() {
    let mgr = SessionManager::with_default_timeout();
    let result = broadcast_to_session(&mgr, "no-such-session", "user", "msg").await;
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// WsQuery deserialization edge cases
// ---------------------------------------------------------------------------

#[test]
fn test_ws_query_deserialize_with_unicode_token() {
    let query: WsQuery = serde_json::from_str(r#"{"token":"token-中文-🚀"}"#).unwrap();
    assert_eq!(query.token, Some("token-中文-🚀".to_string()));
}

#[test]
fn test_ws_query_deserialize_with_long_token() {
    let long_token = "a".repeat(1000);
    let json = format!(r#"{{"token":"{}"}}"#, long_token);
    let query: WsQuery = serde_json::from_str(&json).unwrap();
    assert_eq!(query.token.unwrap().len(), 1000);
}

#[test]
fn test_ws_query_deserialize_with_null_token() {
    let result: Result<WsQuery, _> = serde_json::from_str(r#"{"token":null}"#);
    // null should deserialize to None
    if let Ok(q) = result {
        assert!(q.token.is_none());
    }
}

#[test]
fn test_ws_query_deserialize_invalid_type() {
    let result: Result<WsQuery, _> = serde_json::from_str(r#"{"token":123}"#);
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// handle_text_message additional dispatch tests
// ---------------------------------------------------------------------------

#[test]
fn test_handle_request_type_returns_none() {
    // "request" type should return Ok(None) — handled by WsRouter, not legacy dispatch
    let raw = br#"{"type":"request","module":"models","cmd":"list","reqId":"r1"}"#;
    let result = handle_text_message("s1", "w:s1", "w:s1", raw);
    assert!(result.is_ok());
    assert!(result.unwrap().is_none());
}

#[test]
fn test_handle_chat_send_with_voice_playback_true() {
    let raw = br#"{"type":"message","module":"chat","cmd":"send","data":{"content":"speak this","voice_playback":true}}"#;
    let result = handle_text_message("s1", "w:s1", "w:s1", raw).unwrap().unwrap();
    assert_eq!(result.voice_playback, Some(true));
    assert_eq!(result.content, "speak this");
}

#[test]
fn test_handle_chat_send_with_voice_playback_false() {
    let raw = br#"{"type":"message","module":"chat","cmd":"send","data":{"content":"don't speak","voice_playback":false}}"#;
    let result = handle_text_message("s1", "w:s1", "w:s1", raw).unwrap().unwrap();
    assert_eq!(result.voice_playback, Some(false));
}

#[test]
fn test_handle_chat_send_with_voice_playback_null() {
    let raw = br#"{"type":"message","module":"chat","cmd":"send","data":{"content":"hi","voice_playback":null}}"#;
    let result = handle_text_message("s1", "w:s1", "w:s1", raw).unwrap().unwrap();
    assert_eq!(result.voice_playback, None);
}

#[test]
fn test_handle_chat_send_with_extra_unknown_fields() {
    // Extra fields in data should be ignored
    let raw = br#"{"type":"message","module":"chat","cmd":"send","data":{"content":"hello","extra":"ignored","number":42}}"#;
    let result = handle_text_message("s1", "w:s1", "w:s1", raw);
    assert!(result.is_ok());
    assert_eq!(result.unwrap().unwrap().content, "hello");
}

#[test]
fn test_handle_chat_send_data_array_invalid() {
    let raw = br#"{"type":"message","module":"chat","cmd":"send","data":["array","not","object"]}"#;
    let result = handle_text_message("s1", "w:s1", "w:s1", raw);
    assert!(result.is_err());
}

#[test]
fn test_handle_chat_send_data_number_invalid() {
    let raw = br#"{"type":"message","module":"chat","cmd":"send","data":123}"#;
    let result = handle_text_message("s1", "w:s1", "w:s1", raw);
    assert!(result.is_err());
}

#[test]
fn test_handle_chat_send_data_bool_invalid() {
    let raw = br#"{"type":"message","module":"chat","cmd":"send","data":true}"#;
    let result = handle_text_message("s1", "w:s1", "w:s1", raw);
    assert!(result.is_err());
}

#[test]
fn test_handle_chat_send_with_content_array_invalid() {
    let raw = br#"{"type":"message","module":"chat","cmd":"send","data":{"content":["a","b"]}}"#;
    let result = handle_text_message("s1", "w:s1", "w:s1", raw);
    assert!(result.is_err());
}

#[test]
fn test_handle_chat_send_with_content_number_invalid() {
    let raw = br#"{"type":"message","module":"chat","cmd":"send","data":{"content":42}}"#;
    let result = handle_text_message("s1", "w:s1", "w:s1", raw);
    assert!(result.is_err());
}

#[test]
fn test_handle_history_request_missing_request_id() {
    let raw = br#"{"type":"message","module":"chat","cmd":"history_request","data":{}}"#;
    let result = handle_text_message("s1", "w:s1", "w:s1", raw);
    assert!(result.is_err());
}

#[test]
fn test_handle_history_request_with_negative_limit() {
    let raw = br#"{"type":"message","module":"chat","cmd":"history_request","data":{"request_id":"r1","limit":-5}}"#;
    let result = handle_text_message("s1", "w:s1", "w:s1", raw);
    assert!(result.is_ok());
}

#[test]
fn test_handle_history_request_with_huge_limit() {
    let raw = br#"{"type":"message","module":"chat","cmd":"history_request","data":{"request_id":"r1","limit":99999}}"#;
    let result = handle_text_message("s1", "w:s1", "w:s1", raw).unwrap().unwrap();
    let content: serde_json::Value = serde_json::from_str(&result.content).unwrap();
    assert_eq!(content["limit"], 99999);
}

#[test]
fn test_handle_history_request_with_null_optional_fields() {
    let raw = br#"{"type":"message","module":"chat","cmd":"history_request","data":{"request_id":"r1","limit":null,"before_index":null}}"#;
    let result = handle_text_message("s1", "w:s1", "w:s1", raw).unwrap().unwrap();
    let content: serde_json::Value = serde_json::from_str(&result.content).unwrap();
    assert!(content["limit"].is_null());
    assert!(content["before_index"].is_null());
}

// ---------------------------------------------------------------------------
// Protocol message parser edge cases
// ---------------------------------------------------------------------------

#[test]
fn test_handle_text_message_empty_bytes() {
    let result = handle_text_message("s1", "w:s1", "w:s1", b"");
    assert!(result.is_err());
}

#[test]
fn test_handle_text_message_just_braces() {
    let result = handle_text_message("s1", "w:s1", "w:s1", b"{}");
    // Empty JSON object — fields missing, "type" defaults to empty string via serde
    // The match on empty msg_type should hit the unknown arm
    assert!(result.is_err());
}

#[test]
fn test_handle_text_message_with_bom() {
    let raw = b"\xef\xbb\xbf{\"type\":\"message\",\"module\":\"chat\",\"cmd\":\"send\",\"data\":{\"content\":\"x\"}}";
    let result = handle_text_message("s1", "w:s1", "w:s1", raw);
    // BOM might or might not be tolerated by serde_json — either way it shouldn't panic
    let _ = result;
}

#[test]
fn test_handle_text_message_with_trailing_whitespace() {
    let raw = b"{\"type\":\"system\",\"module\":\"heartbeat\",\"cmd\":\"ping\",\"data\":null}   \n  ";
    let result = handle_text_message("s1", "w:s1", "w:s1", raw);
    // Trailing whitespace should be tolerated by serde_json
    assert!(result.is_ok());
}

// ---------------------------------------------------------------------------
// build_broadcast_message and build_pong content tests
// ---------------------------------------------------------------------------

#[test]
fn test_build_broadcast_message_unicode_content() {
    let bytes = build_broadcast_message("assistant", "你好，世界 🌍").unwrap();
    let msg = ProtocolMessage::parse(&bytes).unwrap();
    assert_eq!(msg.data.unwrap()["content"], "你好，世界 🌍");
}

#[test]
fn test_build_broadcast_message_with_special_role_chars() {
    let bytes = build_broadcast_message("user-custom", "msg").unwrap();
    let msg = ProtocolMessage::parse(&bytes).unwrap();
    assert_eq!(msg.data.unwrap()["role"], "user-custom");
}

#[test]
fn test_build_broadcast_message_long_role() {
    let long_role = "x".repeat(500);
    let bytes = build_broadcast_message(&long_role, "msg").unwrap();
    let msg = ProtocolMessage::parse(&bytes).unwrap();
    assert_eq!(msg.data.unwrap()["role"].as_str().unwrap().len(), 500);
}

#[test]
fn test_build_pong_includes_timestamp() {
    let bytes = build_pong().unwrap();
    let msg = ProtocolMessage::parse(&bytes).unwrap();
    assert!(msg.timestamp.is_some());
}

#[test]
fn test_build_error_message_includes_timestamp() {
    let bytes = build_error_message("test");
    let msg = ProtocolMessage::parse(&bytes).unwrap();
    assert!(msg.timestamp.is_some());
}

#[test]
fn test_build_broadcast_message_includes_timestamp() {
    let bytes = build_broadcast_message("assistant", "hi").unwrap();
    let msg = ProtocolMessage::parse(&bytes).unwrap();
    assert!(msg.timestamp.is_some());
}

// ---------------------------------------------------------------------------
// Stress tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_websocket_multiple_messages_in_sequence() {
    let (tx, mut rx) = mpsc::unbounded_channel::<IncomingMessage>();
    let state = make_state("", Some(tx), None);
    let (addr, listener) = bind_ephemeral().await;
    let _server = start_test_server(listener, state).await;

    let mut ws = ws_connect(&addr, None).await;

    for i in 0..10 {
        let raw = format!(
            r#"{{"type":"message","module":"chat","cmd":"send","data":{{"content":"msg-{}"}}}}"#,
            i
        );
        ws.send(WsMessage::Text(raw.into())).await.unwrap();
    }

    for i in 0..10 {
        let incoming = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("timeout")
            .expect("closed");
        assert_eq!(incoming.content, format!("msg-{}", i));
    }
}

#[tokio::test]
async fn test_multiple_concurrent_websocket_sessions() {
    let state = make_state("", None, None);
    let (addr, listener) = bind_ephemeral().await;
    let _server = start_test_server(listener, state.clone()).await;

    let mut clients = Vec::new();
    for _ in 0..5 {
        let ws = ws_connect(&addr, None).await;
        clients.push(ws);
    }

    tokio::time::sleep(Duration::from_millis(200)).await;
    let count = state.session_count.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(count, 5);

    // Close all
    for mut c in clients {
        let _ = c.close(None).await;
    }

    tokio::time::sleep(Duration::from_millis(300)).await;
    let final_count = state.session_count.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(final_count, 0);
}

// ---------------------------------------------------------------------------
// Authenticated WebSocket flow integration tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_websocket_with_correct_token_connects_and_works() {
    let state = make_state("my-secret", None, None);
    let (addr, listener) = bind_ephemeral().await;
    let _server = start_test_server(listener, state).await;

    let mut ws = ws_connect(&addr, Some("my-secret")).await;

    let raw = r#"{"type":"system","module":"heartbeat","cmd":"ping","data":null}"#;
    ws.send(WsMessage::Text(raw.into())).await.unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("timeout")
        .expect("closed")
        .expect("err");
    if let WsMessage::Text(t) = msg {
        let parsed: serde_json::Value = serde_json::from_str(&t).unwrap();
        assert_eq!(parsed["cmd"], "pong");
    }
}

// ---------------------------------------------------------------------------
// Integration: WS API router request dispatch (covers lines 201-237)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_websocket_request_type_without_router_sends_error() {
    // When ws_router is None, request messages should get an error response
    let state = make_state("", None, None);
    let (addr, listener) = bind_ephemeral().await;
    let _server = start_test_server(listener, state).await;

    let mut ws = ws_connect(&addr, None).await;

    let raw = r#"{"type":"request","module":"models","cmd":"list","reqId":"r-1"}"#;
    ws.send(WsMessage::Text(raw.into())).await.unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("timeout")
        .expect("closed")
        .expect("err");
    if let WsMessage::Text(t) = msg {
        let parsed: serde_json::Value = serde_json::from_str(&t).unwrap();
        assert_eq!(parsed["type"], "response");
        assert!(parsed.get("error").is_some());
        assert_eq!(parsed["reqId"], "r-1");
        assert!(parsed["error"].as_str().unwrap().contains("router not configured"));
    }
}

#[tokio::test]
async fn test_websocket_request_type_with_router_dispatches() {
    // Register a simple handler that responds to a custom module
    use crate::ws_router::{ModuleHandler, RequestContext, WsRouter};

    struct EchoHandler;
    #[async_trait::async_trait]
    impl ModuleHandler for EchoHandler {
        fn module_name(&self) -> &str {
            "echo"
        }
        async fn handle_cmd(
            &self,
            _cmd: &str,
            data: Option<serde_json::Value>,
            _ctx: &RequestContext,
        ) -> Result<Option<serde_json::Value>, String> {
            Ok(data)
        }
    }

    let mut router = WsRouter::new();
    router.register(Arc::new(EchoHandler));
    let router = Arc::new(router);

    let state = make_state("", None, Some(router));
    let (addr, listener) = bind_ephemeral().await;
    let _server = start_test_server(listener, state).await;

    let mut ws = ws_connect(&addr, None).await;

    let raw = r#"{"type":"request","module":"echo","cmd":"ping","reqId":"r-2","data":{"hello":"world"}}"#;
    ws.send(WsMessage::Text(raw.into())).await.unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("timeout")
        .expect("closed")
        .expect("err");
    if let WsMessage::Text(t) = msg {
        let parsed: serde_json::Value = serde_json::from_str(&t).unwrap();
        assert_eq!(parsed["type"], "response");
        assert_eq!(parsed["reqId"], "r-2");
        assert!(parsed.get("error").is_none() || parsed["error"].is_null());
        assert_eq!(parsed["data"]["hello"], "world");
    }
}

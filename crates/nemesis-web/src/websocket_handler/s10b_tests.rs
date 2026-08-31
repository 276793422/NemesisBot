//! S10b (quality-hardening goal 冲刺, web 批次 2): websocket_handler arms
//! that `extra_tests.rs` (auth/legacy happy paths) does not reach, all via a
//! real axum server on an ephemeral 127.0.0.1 port + tokio-tungstenite client:
//!
//! - `handle_websocket_upgrade` workflow-chat password arm: wrong pwd → 401,
//!   correct pwd → upgrade proceeds (163-173)
//! - request-type message with no WS router configured → `response_err`
//!   "ws router not configured" echoed on the socket (286-295)
//! - workflow_chat module message spawn block (304-320; unknown cmd → warn,
//!   loop stays responsive)
//! - legacy chat.send with `inbound_tx = None` → dropped-message warn (344-347)
//! - legacy chat.send with a CLOSED inbound channel → forward-failure warn
//!   (331-335)
//! - protocol-level Ping frame → text pong (366-370)
//! - abrupt client drop → stream-ended arm + session cleanup (392-397)
//! - direct: chat.send / history_request `session_id` metadata arms
//!
//! Not covered here (structural): SendQueue sink feed/flush failure arms
//! (68-79; need a sink that fails I/O deterministically) and the stream read
//! error arm (384-390; a compliant client cannot force a read error).

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use axum::Router;
use axum::routing::get;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message as WsMessage;

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn make_state(inbound_tx: Option<mpsc::UnboundedSender<IncomingMessage>>) -> Arc<AppState> {
    Arc::new(AppState {
        auth_token: String::new(),
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
        ws_router: None,
        agent_service: None,
        data_store: None,
        memory_manager: None,
        forge: None,
        agent_loop: Arc::new(parking_lot::RwLock::new(None)),
        cluster: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
        board: None,
    })
}

async fn start_server(state: Arc<AppState>) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = Router::new()
        .route("/ws", get(handle_websocket_upgrade))
        .with_state(state);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

async fn connect_url(addr: &std::net::SocketAddr, query: &str) -> WsStream {
    let url = format!("ws://{}/ws{}", addr, query);
    tokio_tungstenite::connect_async(url).await.expect("connect").0
}

/// Read the next TEXT message, skipping protocol-level Pong/Ping frames.
async fn next_text(ws: &mut WsStream) -> String {
    let deadline = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match ws.next().await.expect("stream open").expect("ws ok") {
                WsMessage::Text(t) => return t.to_string(),
                WsMessage::Pong(_) | WsMessage::Ping(_) => continue,
                other => panic!("expected text, got {:?}", other),
            }
        }
    });
    deadline.await.expect("timeout waiting for text message")
}

/// Send a heartbeat JSON ping and expect the pong reply — proves the read
/// loop is still alive after an arm that only warns.
async fn assert_loop_alive(ws: &mut WsStream) {
    ws.send(WsMessage::Text(
        r#"{"type":"system","module":"heartbeat","cmd":"ping","data":null}"#.into(),
    ))
    .await
    .unwrap();
    let pong = next_text(ws).await;
    let v: serde_json::Value = serde_json::from_str(&pong).unwrap();
    assert_eq!(v["cmd"], "pong", "loop must still respond, got: {}", pong);
}

// ---------------------------------------------------------------------------
// workflow-chat password auth (handle_websocket_upgrade)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn workflow_chat_wrong_password_rejects_upgrade_with_401() {
    let state = make_state(None);
    state
        .chat_secret_store
        .set_password("wfidx", "s3cret")
        .expect("set password");
    let addr = start_server(state).await;

    // Raw HTTP upgrade probe (same technique as extra_tests) to see the 401.
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let req = b"GET /ws?workflow_chat=wfidx&pwd=wrong HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n";
    stream.write_all(req).await.unwrap();
    let mut buf = [0u8; 1024];
    let n = stream.read(&mut buf).await.unwrap();
    let resp = String::from_utf8_lossy(&buf[..n]);
    assert!(
        resp.starts_with("HTTP/1.1 401"),
        "wrong workflow-chat pwd must 401, got: {}",
        resp.lines().next().unwrap_or("")
    );
}

#[tokio::test]
async fn workflow_chat_correct_password_allows_upgrade() {
    let state = make_state(None);
    state
        .chat_secret_store
        .set_password("wfidx", "s3cret")
        .expect("set password");
    let addr = start_server(state).await;

    let mut ws = connect_url(&addr, "?workflow_chat=wfidx&pwd=s3cret").await;
    // Upgrade succeeded; the session is a workflow-chat session. Round-trip
    // a heartbeat to prove the live loop works under this auth method.
    assert_loop_alive(&mut ws).await;
    ws.send(WsMessage::Close(None)).await.unwrap();
}

// ---------------------------------------------------------------------------
// request dispatch with no router + workflow_chat module spawn
// ---------------------------------------------------------------------------

#[tokio::test]
async fn request_without_router_gets_error_response() {
    let state = make_state(None); // ws_router = None
    let addr = start_server(state).await;
    let mut ws = connect_url(&addr, "").await;

    ws.send(WsMessage::Text(
        r#"{"type":"request","module":"chat","cmd":"list","id":"r1","data":{}}"#.into(),
    ))
    .await
    .unwrap();

    let resp = next_text(&mut ws).await;
    let v: serde_json::Value = serde_json::from_str(&resp).unwrap();
    assert_eq!(v["type"], "response", "got: {}", resp);
    assert!(
        resp.contains("ws router not configured"),
        "expected router-missing error, got: {}",
        resp
    );
    // Loop survives the error arm.
    assert_loop_alive(&mut ws).await;
}

#[tokio::test]
async fn workflow_chat_module_message_is_spawned_and_loop_survives() {
    let state = make_state(None); // workflow_engine = None
    let addr = start_server(state).await;
    let mut ws = connect_url(&addr, "").await;

    // Unknown cmd → the spawned handler returns Err → warn; no reply is sent.
    ws.send(WsMessage::Text(
        r#"{"type":"message","module":"workflow_chat","cmd":"bogus","data":{}}"#.into(),
    ))
    .await
    .unwrap();
    // Give the spawned task a beat to run, then prove the loop is responsive.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_loop_alive(&mut ws).await;
}

// ---------------------------------------------------------------------------
// legacy path inbound-channel arms
// ---------------------------------------------------------------------------

#[tokio::test]
async fn chat_send_with_no_inbound_channel_is_dropped_with_warning() {
    let state = make_state(None); // inbound_tx = None
    let addr = start_server(state).await;
    let mut ws = connect_url(&addr, "").await;

    ws.send(WsMessage::Text(
        r#"{"type":"message","module":"chat","cmd":"send","data":{"content":"dropped"}}"#.into(),
    ))
    .await
    .unwrap();
    // No response is expected; the loop must stay alive.
    assert_loop_alive(&mut ws).await;
}

#[tokio::test]
async fn chat_send_with_closed_inbound_channel_warns_and_survives() {
    let (tx, rx) = mpsc::unbounded_channel::<IncomingMessage>();
    drop(rx); // channel closed → tx.send fails
    let state = make_state(Some(tx));
    let addr = start_server(state).await;
    let mut ws = connect_url(&addr, "").await;

    ws.send(WsMessage::Text(
        r#"{"type":"message","module":"chat","cmd":"send","data":{"content":"to closed bus"}}"#
            .into(),
    ))
    .await
    .unwrap();
    assert_loop_alive(&mut ws).await;
}

// ---------------------------------------------------------------------------
// protocol frame arms
// ---------------------------------------------------------------------------

#[tokio::test]
async fn protocol_ping_frame_receives_text_pong() {
    let state = make_state(None);
    let addr = start_server(state).await;
    let mut ws = connect_url(&addr, "").await;

    ws.send(WsMessage::Ping(vec![1, 2, 3].into())).await.unwrap();
    let pong = next_text(&mut ws).await;
    let v: serde_json::Value = serde_json::from_str(&pong).unwrap();
    assert_eq!(v["module"], "heartbeat", "got: {}", pong);
    assert_eq!(v["cmd"], "pong", "got: {}", pong);
}

#[tokio::test]
async fn abrupt_client_drop_ends_stream_and_cleans_up_session() {
    let state = make_state(None);
    let addr = start_server(state.clone()).await;
    let mut ws = connect_url(&addr, "").await;

    // Connection is up → session count is 1.
    assert_eq!(state.session_count.load(std::sync::atomic::Ordering::SeqCst), 1);

    // Abrupt drop (no close frame) → server read returns None → stream-ended
    // arm → cleanup decrements the count.
    ws.send(WsMessage::Text(
        r#"{"type":"system","module":"heartbeat","cmd":"ping","data":null}"#.into(),
    ))
    .await
    .unwrap(); // ensure the server is fully inside the read loop
    drop(ws);

    let deadline = Instant::now() + Duration::from_secs(5);
    while state.session_count.load(std::sync::atomic::Ordering::SeqCst) != 0 {
        assert!(Instant::now() < deadline, "session count never returned to 0");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ---------------------------------------------------------------------------
// direct metadata arms (session_id passthrough)
// ---------------------------------------------------------------------------

#[test]
fn chat_send_and_history_request_pass_session_id_metadata() {
    let incoming = handle_text_message(
        "sess-1",
        "user-1",
        "chat-1",
        br#"{"type":"message","module":"chat","cmd":"send","data":{"content":"hi","session_id":"sess-x"}}"#,
    )
    .expect("chat.send parses")
    .expect("Some for chat.send");
    assert_eq!(incoming.metadata.get("session_id").unwrap(), "sess-x");
    assert_eq!(incoming.content, "hi");

    let hist = handle_text_message(
        "sess-1",
        "user-1",
        "chat-1",
        br#"{"type":"message","module":"chat","cmd":"history_request","data":{"request_id":"r9","session_id":"sess-y","limit":5,"before_index":2}}"#,
    )
    .expect("history_request parses")
    .expect("Some for history_request");
    assert_eq!(hist.metadata.get("request_type").unwrap(), "history");
    assert_eq!(hist.metadata.get("session_id").unwrap(), "sess-y");
}

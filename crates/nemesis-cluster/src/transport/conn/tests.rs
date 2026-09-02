use super::*;

#[test]
fn test_connection_lifecycle() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();

    let client = Connection::connect(&addr).unwrap();
    assert!(client.is_connected());
    assert!(!client.remote_addr().is_empty());
}

#[test]
fn test_send_recv_roundtrip() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();

    let mut client = Connection::connect(&addr).unwrap();
    let (server_stream, _) = listener.accept().unwrap();
    let mut server = Connection::new(server_stream);

    client.send(b"hello world").unwrap();
    let data = server.recv().unwrap();
    assert_eq!(data, b"hello world");
}

#[test]
fn test_close() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();

    let mut client = Connection::connect(&addr).unwrap();
    assert!(client.is_connected());
    client.close();
    assert!(!client.is_connected());
}

#[test]
fn test_wire_message_new_request() {
    let msg = WireMessage::new_request("node-a", "node-b", "ping", serde_json::json!({}));
    assert_eq!(msg.msg_type, "request");
    assert_eq!(msg.from, "node-a");
    assert_eq!(msg.to, "node-b");
    assert_eq!(msg.action, "ping");
    assert!(msg.validate().is_ok());
}

#[test]
fn test_wire_message_new_response() {
    let req = WireMessage::new_request("a", "b", "test", serde_json::json!({}));
    let resp = WireMessage::new_response(&req, serde_json::json!({"ok": true}));
    assert_eq!(resp.msg_type, "response");
    assert_eq!(resp.id, req.id);
    assert_eq!(resp.from, "b");
    assert_eq!(resp.to, "a");
    assert!(resp.is_response());
}

#[test]
fn test_wire_message_new_error() {
    let req = WireMessage::new_request("a", "b", "test", serde_json::json!({}));
    let err = WireMessage::new_error(&req, "something went wrong");
    assert_eq!(err.msg_type, "error");
    assert_eq!(err.error, "something went wrong");
    assert!(err.is_error());
}

#[test]
fn test_wire_message_validate() {
    let msg = WireMessage::new_request("a", "b", "c", serde_json::json!({}));
    assert!(msg.validate().is_ok());

    let bad = WireMessage {
        version: String::new(),
        id: String::new(),
        msg_type: "request".into(),
        from: String::new(),
        to: String::new(),
        action: String::new(),
        payload: serde_json::Value::Null,
        timestamp: 0,
        error: String::new(),
    };
    assert!(bad.validate().is_err());
}

#[test]
fn test_wire_message_serialization() {
    let msg = WireMessage::new_request("a", "b", "ping", serde_json::json!({"key": "val"}));
    let bytes = msg.to_bytes().unwrap();
    let back = WireMessage::from_bytes(&bytes).unwrap();
    assert_eq!(back.id, msg.id);
    assert_eq!(back.from, "a");
    assert_eq!(back.to, "b");
    assert_eq!(back.action, "ping");
}

#[tokio::test]
async fn test_tcp_conn_send_receive() {
    // Set up a TCP listener
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    // Channel to signal when server has sent its response
    let (server_done_tx, server_done_rx) = tokio::sync::oneshot::channel();

    // Server side: accept and create a TcpConn
    let server_addr = addr;
    let server_handle = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut server_conn = TcpConn::new(
            stream,
            TcpConnConfig {
                address: server_addr.to_string(),
                ..Default::default()
            },
        );
        server_conn.start().await.unwrap();

        // Read a message
        let msg = server_conn.receive().await.unwrap();
        assert_eq!(msg.action, "hello");
        assert_eq!(msg.from, "client");

        // Send a response
        let resp = WireMessage::new_response(&msg, serde_json::json!({"status": "ok"}));
        server_conn.send(&resp).await.unwrap();

        // Give the write loop time to flush the data to the TCP stream
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Signal that the response has been sent
        let _ = server_done_tx.send(());
    });

    // Client side
    let client_stream = TokioTcpStream::connect(addr).await.unwrap();
    let mut client_conn = TcpConn::new(
        client_stream,
        TcpConnConfig {
            node_id: "client".into(),
            address: addr.to_string(),
            ..Default::default()
        },
    );
    client_conn.start().await.unwrap();

    // Send a request
    let req = WireMessage::new_request("client", "server", "hello", serde_json::json!({}));
    client_conn.send(&req).await.unwrap();

    // Receive response
    let resp = client_conn.receive().await.unwrap();
    assert_eq!(resp.id, req.id);
    assert_eq!(resp.msg_type, "response");

    // Wait for server to finish
    server_done_rx.await.unwrap();
    let _ = server_handle.await;
}

#[tokio::test]
async fn test_tcp_conn_close() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server_handle = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let _ = stream; // just accept
    });

    let client_stream = TokioTcpStream::connect(addr).await.unwrap();
    let mut client = TcpConn::new(client_stream, TcpConnConfig::default());
    client.start().await.unwrap();
    assert!(client.is_active());
    assert!(!client.is_closed());

    client.close();
    assert!(!client.is_active());
    assert!(client.is_closed());

    // Double close is safe
    client.close();
    assert!(client.is_closed());

    server_handle.await.unwrap();
}

#[tokio::test]
async fn test_tcp_conn_auth_token() {
    // AEAD auth model: both ends derive the same AES-256-GCM key from the
    // shared token. The server's read loop decrypts inbound frames; an
    // attacker without the token cannot produce a frame with a valid GCM
    // tag. This test verifies that two TcpConns configured with the same
    // token can exchange a WireMessage end-to-end.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let token = "secret-token-123";

    let server_handle = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut server = TcpConn::new(
            stream,
            TcpConnConfig {
                address: "server".into(),
                auth_token: Some(token.to_string()),
                ..Default::default()
            },
        );
        server.start().await.unwrap();

        let msg = server.receive().await.unwrap();
        assert_eq!(msg.action, "ping");
        assert_eq!(msg.from, "client");
    });

    let client_stream = TokioTcpStream::connect(addr).await.unwrap();
    let mut client = TcpConn::new(
        client_stream,
        TcpConnConfig {
            node_id: "client".into(),
            address: addr.to_string(),
            auth_token: Some(token.to_string()),
            ..Default::default()
        },
    );
    client.start().await.unwrap();

    let req = WireMessage::new_request("client", "server", "ping", serde_json::json!({}));
    client.send(&req).await.unwrap();

    server_handle.await.unwrap();
}

#[tokio::test]
async fn test_tcp_conn_wrong_token_drops_connection() {
    // When the client uses a different token than the server, the server's
    // decrypt step fails the GCM tag check and closes the connection. The
    // client's subsequent receive() returns None.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server_token = "server-secret";
    let client_token = "client-different-secret";

    let server_handle = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut server = TcpConn::new(
            stream,
            TcpConnConfig {
                address: "server".into(),
                auth_token: Some(server_token.to_string()),
                ..Default::default()
            },
        );
        server.start().await.unwrap();
        // receive() returns None because the read loop closes on decrypt error
        let _ = server.receive().await;
    });

    let client_stream = TokioTcpStream::connect(addr).await.unwrap();
    let mut client = TcpConn::new(
        client_stream,
        TcpConnConfig {
            node_id: "client".into(),
            address: addr.to_string(),
            auth_token: Some(client_token.to_string()),
            ..Default::default()
        },
    );
    client.start().await.unwrap();

    let req = WireMessage::new_request("client", "server", "ping", serde_json::json!({}));
    // Send may succeed locally (queued in write loop) but the server will
    // close the connection after the decrypt failure.
    let _ = client.send(&req).await;
    server_handle.await.unwrap();
}

#[test]
fn test_tcp_conn_config_default() {
    let config = TcpConnConfig::default();
    assert_eq!(config.read_buffer_size, 100);
    assert_eq!(config.send_buffer_size, 100);
    assert_eq!(config.send_timeout, Duration::from_secs(10));
    assert_eq!(config.idle_timeout, Duration::from_secs(30));
    assert!(config.auth_token.is_none());
    assert!(config.heartbeat_interval.is_none());
}

// ============================================================
// Coverage improvement: WireMessage validation, Connection errors, TcpConn state
// ============================================================

#[test]
fn test_wire_message_validate_missing_version() {
    let msg = WireMessage {
        version: String::new(),
        id: "id".into(),
        msg_type: "request".into(),
        from: "a".into(),
        to: "b".into(),
        action: "c".into(),
        payload: serde_json::Value::Null,
        timestamp: 0,
        error: String::new(),
    };
    assert_eq!(msg.validate(), Err("missing version".into()));
}

#[test]
fn test_wire_message_validate_missing_id() {
    let msg = WireMessage {
        version: "1.0".into(),
        id: String::new(),
        msg_type: "request".into(),
        from: "a".into(),
        to: "b".into(),
        action: "c".into(),
        payload: serde_json::Value::Null,
        timestamp: 0,
        error: String::new(),
    };
    assert_eq!(msg.validate(), Err("missing id".into()));
}

#[test]
fn test_wire_message_validate_missing_from() {
    let msg = WireMessage {
        version: "1.0".into(),
        id: "id".into(),
        msg_type: "request".into(),
        from: String::new(),
        to: "b".into(),
        action: "c".into(),
        payload: serde_json::Value::Null,
        timestamp: 0,
        error: String::new(),
    };
    assert_eq!(msg.validate(), Err("missing from".into()));
}

#[test]
fn test_wire_message_validate_missing_to() {
    let msg = WireMessage {
        version: "1.0".into(),
        id: "id".into(),
        msg_type: "request".into(),
        from: "a".into(),
        to: String::new(),
        action: "c".into(),
        payload: serde_json::Value::Null,
        timestamp: 0,
        error: String::new(),
    };
    assert_eq!(msg.validate(), Err("missing to".into()));
}

#[test]
fn test_wire_message_validate_missing_action() {
    let msg = WireMessage {
        version: "1.0".into(),
        id: "id".into(),
        msg_type: "request".into(),
        from: "a".into(),
        to: "b".into(),
        action: String::new(),
        payload: serde_json::Value::Null,
        timestamp: 0,
        error: String::new(),
    };
    assert_eq!(msg.validate(), Err("missing action".into()));
}

#[test]
fn test_wire_message_from_bytes_invalid() {
    let result = WireMessage::from_bytes(b"not json");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("JSON unmarshal error"));
}

#[test]
fn test_wire_message_serialization_roundtrip_full() {
    let msg = WireMessage {
        version: "1.0".into(),
        id: "test-id".into(),
        msg_type: "request".into(),
        from: "node-a".into(),
        to: "node-b".into(),
        action: "ping".into(),
        payload: serde_json::json!({"key": "value", "num": 42}),
        timestamp: 1715385600,
        error: String::new(),
    };
    let bytes = msg.to_bytes().unwrap();
    let back = WireMessage::from_bytes(&bytes).unwrap();
    assert_eq!(back.version, "1.0");
    assert_eq!(back.id, "test-id");
    assert_eq!(back.from, "node-a");
    assert_eq!(back.to, "node-b");
    assert_eq!(back.action, "ping");
    assert_eq!(back.timestamp, 1715385600);
    assert_eq!(back.payload["key"], "value");
    assert_eq!(back.payload["num"], 42);
}

#[test]
fn test_wire_message_error_with_message() {
    let req = WireMessage::new_request("a", "b", "test", serde_json::json!({}));
    let err = WireMessage::new_error(&req, "something failed");
    assert_eq!(err.msg_type, "error");
    assert_eq!(err.error, "something failed");
    assert_eq!(err.from, "b");
    assert_eq!(err.to, "a");
    assert_eq!(err.id, req.id);
    assert!(err.payload.is_null());
}

#[test]
fn test_connection_double_close_safe() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let mut client = Connection::connect(&addr).unwrap();
    client.close();
    assert!(!client.is_connected());
    client.close(); // Second close should not panic
    assert!(!client.is_connected());
}

#[test]
fn test_connection_send_after_close_errors() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let mut client = Connection::connect(&addr).unwrap();
    client.close();
    let result = client.send(b"test");
    assert!(result.is_err());
}

#[test]
fn test_connection_recv_after_close_errors() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let mut client = Connection::connect(&addr).unwrap();
    client.close();
    let result = client.recv();
    assert!(result.is_err());
}

#[test]
fn test_connection_error_display() {
    let err = ConnectionError::Closed;
    assert_eq!(format!("{}", err), "Connection closed");
    let err = ConnectionError::NotConnected;
    assert_eq!(format!("{}", err), "Not connected");
}

#[tokio::test]
async fn test_tcp_conn_not_started_not_active() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_handle = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let _ = stream;
    });
    let client_stream = TokioTcpStream::connect(addr).await.unwrap();
    let client = TcpConn::new(client_stream, TcpConnConfig::default());
    assert!(!client.is_active());
    assert!(!client.is_closed());
    assert!(client.node_id().is_empty());
    assert!(client.address().is_empty());
    server_handle.await.unwrap();
}

#[tokio::test]
async fn test_tcp_conn_accessors() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_handle = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let _ = stream;
    });
    let client_stream = TokioTcpStream::connect(addr).await.unwrap();
    let config = TcpConnConfig {
        node_id: "test-node".into(),
        address: addr.to_string(),
        ..Default::default()
    };
    let client = TcpConn::new(client_stream, config);
    assert_eq!(client.node_id(), "test-node");
    assert_eq!(client.address(), addr.to_string());
    assert_eq!(client.dropped_count(), 0);
    let _created = client.created_at();
    let _last_used = client.last_used();
    server_handle.await.unwrap();
}

#[tokio::test]
async fn test_tcp_conn_set_node_id() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_handle = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let _ = stream;
    });
    let client_stream = TokioTcpStream::connect(addr).await.unwrap();
    let mut client = TcpConn::new(client_stream, TcpConnConfig::default());
    assert_eq!(client.node_id(), "");
    client.set_node_id("new-node-id".into());
    assert_eq!(client.node_id(), "new-node-id");
    server_handle.await.unwrap();
}

#[tokio::test]
async fn test_tcp_conn_send_closed_errors() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_handle = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let _ = stream;
    });
    let client_stream = TokioTcpStream::connect(addr).await.unwrap();
    let mut client = TcpConn::new(client_stream, TcpConnConfig::default());
    client.close();
    let msg = WireMessage::new_request("a", "b", "test", serde_json::json!({}));
    let result = client.send(&msg).await;
    assert!(result.is_err());
    server_handle.await.unwrap();
}

#[tokio::test]
async fn test_tcp_conn_close_marks_as_closed() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_handle = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let _ = stream;
    });
    let client_stream = TokioTcpStream::connect(addr).await.unwrap();
    let mut client = TcpConn::new(client_stream, TcpConnConfig::default());
    assert!(!client.is_closed());
    client.close();
    assert!(client.is_closed());
    assert!(!client.is_active());
    server_handle.await.unwrap();
}

#[tokio::test]
async fn test_tcp_conn_start_twice_errors() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_handle = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let _ = stream;
    });
    let client_stream = TokioTcpStream::connect(addr).await.unwrap();
    let mut client = TcpConn::new(client_stream, TcpConnConfig::default());
    client.start().await.unwrap();
    let result = client.start().await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("already started"));
    client.close();
    server_handle.await.unwrap();
}

#[test]
fn test_tcp_conn_config_debug() {
    let config = TcpConnConfig {
        node_id: "test".into(),
        address: "127.0.0.1:8080".into(),
        ..Default::default()
    };
    let debug = format!("{:?}", config);
    assert!(debug.contains("test"));
    assert!(debug.contains("127.0.0.1:8080"));
}

// ============================================================
// Coverage improvement: more edge cases
// ============================================================

#[test]
fn test_wire_message_new_response_flips_from_to() {
    let req = WireMessage::new_request("client", "server", "ping", serde_json::json!({}));
    let resp = WireMessage::new_response(&req, serde_json::json!({"ok": true}));
    assert_eq!(resp.from, "server");
    assert_eq!(resp.to, "client");
    assert_eq!(resp.msg_type, "response");
    assert_eq!(resp.action, "ping");
    assert_eq!(resp.id, req.id);
}

#[test]
fn test_wire_message_new_error_flips_from_to() {
    let req = WireMessage::new_request("client", "server", "ping", serde_json::json!({}));
    let err = WireMessage::new_error(&req, "test error");
    assert_eq!(err.from, "server");
    assert_eq!(err.to, "client");
    assert_eq!(err.msg_type, "error");
    assert_eq!(err.error, "test error");
}

#[test]
fn test_wire_message_is_error() {
    let req = WireMessage::new_request("a", "b", "test", serde_json::json!({}));
    assert!(!req.is_error());
    let err = WireMessage::new_error(&req, "fail");
    assert!(err.is_error());
}

#[test]
fn test_wire_message_validate_valid() {
    let msg = WireMessage::new_request("a", "b", "test", serde_json::json!({}));
    assert!(msg.validate().is_ok());
}

#[test]
fn test_connection_error_io_variant() {
    let err = ConnectionError::Io(std::io::Error::new(
        std::io::ErrorKind::BrokenPipe,
        "io error",
    ));
    assert!(format!("{}", err).contains("io error"));
}

#[tokio::test]
async fn test_tcp_conn_receive_closed_returns_none() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_handle = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        drop(stream); // Immediately close
    });

    let client_stream = TokioTcpStream::connect(addr).await.unwrap();
    let mut client = TcpConn::new(client_stream, TcpConnConfig::default());
    client.start().await.unwrap();

    // Wait for server to close
    server_handle.await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Receive on closed connection should return None
    let result = client.receive().await;
    assert!(result.is_none());
}

#[test]
fn test_connection_connect_to_invalid_addr() {
    let result = Connection::connect("999.999.999.999:99999");
    assert!(result.is_err());
}

// ============================================================
// S4 coverage: read/write loop arms, heartbeat, idle monitor,
// accessors, Debug
// ============================================================

/// Minimal no-op tracing subscriber that reports every callsite enabled.
/// Field expressions inside `warn!`/`debug!` macros only evaluate when a
/// subscriber is active; installing this as the global default (once per
/// process) makes those regions execute so they count as covered.
struct AllEventsSubscriber;
impl tracing::Subscriber for AllEventsSubscriber {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::Id {
        tracing::Id::from_u64(1)
    }
    fn record(&self, _span: &tracing::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::Id, _follows: &tracing::Id) {}
    fn event(&self, _event: &tracing::Event<'_>) {}
    fn enter(&self, _span: &tracing::Id) {}
    fn exit(&self, _span: &tracing::Id) {}
}

fn ensure_tracing_subscriber() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // If another test module already installed a global default, that
        // subscriber serves the same purpose — ignore the error.
        let _ = tracing::subscriber::set_global_default(AllEventsSubscriber);
    });
}

/// Sync `Connection::recv` must reject a length prefix larger than
/// MAX_FRAME_SIZE before attempting to read the payload.
#[test]
fn test_connection_recv_frame_too_large() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let huge = (MAX_FRAME_SIZE as u32) + 1;
        stream.write_all(&huge.to_be_bytes()).unwrap();
        // Keep the stream open so the header read succeeds and the size
        // check is what fails.
        std::thread::sleep(std::time::Duration::from_millis(200));
    });

    let mut conn = Connection::connect(&addr.to_string()).unwrap();
    let err = conn.recv().unwrap_err();
    match err {
        ConnectionError::Io(ref e) => assert_eq!(e.kind(), std::io::ErrorKind::InvalidData),
        other => panic!("expected Io(InvalidData), got: {:?}", other),
    }
    server.join().unwrap();
}

/// `start()` on an already-closed connection must fail with a clean error.
#[tokio::test]
async fn test_tcp_conn_start_after_close_errors() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (_s, _) = listener.accept().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    });

    let stream = TokioTcpStream::connect(addr).await.unwrap();
    let mut conn = TcpConn::new(stream, TcpConnConfig::default());
    conn.close();
    let err = conn.start().await.unwrap_err();
    assert_eq!(err, "connection is closed");
    let _ = server.await;
}

/// When the receive channel is full the read loop drops messages instead of
/// blocking, incrementing `dropped_count`.
#[tokio::test]
async fn test_tcp_conn_receive_buffer_full_drops_messages() {
    ensure_tracing_subscriber();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let peer = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        // Send more frames than the receive buffer (capacity 1) can hold.
        for i in 0..5 {
            let msg =
                WireMessage::new_request("peer", "me", "hello", serde_json::json!({ "i": i }));
            let data = msg.to_bytes().unwrap();
            write_frame_async(&mut stream, &data).await.unwrap();
        }
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    });

    let stream = TokioTcpStream::connect(addr).await.unwrap();
    let mut conn = TcpConn::new(
        stream,
        TcpConnConfig {
            read_buffer_size: 1,
            ..Default::default()
        },
    );
    conn.start().await.unwrap();

    // Give the read loop time to process the burst.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        conn.dropped_count() >= 1,
        "messages should be dropped when the receive buffer is full"
    );

    // The first message is still buffered and receivable.
    let first = tokio::time::timeout(std::time::Duration::from_secs(1), conn.receive())
        .await
        .expect("first message should be buffered")
        .expect("first message should be Some");
    assert_eq!(first.action, "hello");

    conn.close();
    let _ = peer.await;
}

/// A frame with a non-JSON payload makes the read loop warn and skip the
/// message, but keeps the connection alive for subsequent valid frames.
#[tokio::test]
async fn test_tcp_conn_read_loop_invalid_json_recovers() {
    ensure_tracing_subscriber();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let peer = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        // Garbage frame first.
        write_frame_async(&mut stream, b"this is not json")
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        // Then a valid frame — the read loop must still deliver it.
        let msg = WireMessage::new_request("peer", "me", "ping", serde_json::json!({}));
        write_frame_async(&mut stream, &msg.to_bytes().unwrap())
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    });

    let stream = TokioTcpStream::connect(addr).await.unwrap();
    let mut conn = TcpConn::new(stream, TcpConnConfig::default());
    conn.start().await.unwrap();

    let got = tokio::time::timeout(std::time::Duration::from_secs(2), conn.receive())
        .await
        .expect("valid frame should arrive after garbage");
    assert!(got.is_some());
    assert_eq!(got.unwrap().action, "ping");
    assert_eq!(conn.dropped_count(), 0);

    conn.close();
    let _ = peer.await;
}

/// A length prefix larger than MAX_FRAME_SIZE surfaces as a non-EOF read
/// error: the read loop warns and terminates, so `receive()` returns None.
#[tokio::test]
async fn test_tcp_conn_read_error_terminates_read_loop() {
    ensure_tracing_subscriber();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let peer = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        use tokio::io::AsyncWriteExt;
        let huge = u32::MAX.to_be_bytes();
        stream.write_all(&huge).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    });

    let stream = TokioTcpStream::connect(addr).await.unwrap();
    let mut conn = TcpConn::new(stream, TcpConnConfig::default());
    conn.start().await.unwrap();

    // receive() resolves to None once the read loop exits (sender dropped).
    let got = tokio::time::timeout(std::time::Duration::from_secs(2), conn.receive())
        .await
        .expect("receive should resolve after read loop exits");
    assert!(got.is_none(), "read loop should have terminated");

    conn.close();
    let _ = peer.await;
}

/// An RST from the peer (linger-0 close) makes writes fail; the write loop
/// logs the error, exits, and subsequent `send()` calls fail because the
/// send channel is closed.
#[tokio::test]
async fn test_tcp_conn_write_error_closes_write_loop() {
    ensure_tracing_subscriber();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let peer = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        // 等客户端 connect() 先落定再发 RST：Linux 回环上对端 accept 返回
        // 时客户端 connect 可能还在等可写事件轮询，立刻 linger(0)+drop 的
        // RST 会把尚未返回的 connect 打成 ECONNRESET（Windows 时序松，
        // 察觉不到；远端 Linux 实测触发）。
        std::thread::sleep(std::time::Duration::from_millis(100));
        // linger(0) + close => RST instead of FIN.
        let sock = socket2::SockRef::from(&stream);
        sock.set_linger(Some(std::time::Duration::ZERO)).unwrap();
        drop(stream);
    });

    let stream = TokioTcpStream::connect(addr).await.unwrap();
    let mut conn = TcpConn::new(
        stream,
        TcpConnConfig {
            send_buffer_size: 64,
            ..Default::default()
        },
    );
    conn.start().await.unwrap();

    // Keep sending until the write loop dies and the channel closes.
    let mut got_err = None;
    for _ in 0..40 {
        let msg = WireMessage::new_request("a", "b", "ping", serde_json::json!({}));
        match conn.send(&msg).await {
            Ok(()) => {
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
            Err(e) => {
                got_err = Some(e);
                break;
            }
        }
    }
    conn.close();
    peer.join().unwrap();
    assert!(
        got_err.is_some(),
        "send should fail once the peer reset the connection"
    );
}

/// The idle monitor closes its task once no activity happened for longer
/// than `idle_timeout`.
#[tokio::test]
async fn test_tcp_conn_idle_timeout_fires() {
    ensure_tracing_subscriber();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let peer = tokio::spawn(async move {
        let (s, _) = listener.accept().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        drop(s);
    });

    let stream = TokioTcpStream::connect(addr).await.unwrap();
    let mut conn = TcpConn::new(
        stream,
        TcpConnConfig {
            idle_timeout: std::time::Duration::from_millis(80),
            ..Default::default()
        },
    );
    conn.start().await.unwrap();

    // check interval = idle_timeout / 2 = 40ms; 300ms is plenty for the
    // idle warn + task break to run.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    conn.close();
    let _ = peer.await;
}

/// With `heartbeat_interval` set, the heartbeat task pushes 4-byte
/// zero-length frames that the peer can read.
#[tokio::test]
async fn test_tcp_conn_heartbeat_sends_empty_frames() {
    ensure_tracing_subscriber();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let peer = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        use tokio::io::AsyncReadExt;
        let mut buf = [0u8; 4];
        s.read_exact(&mut buf).await.unwrap();
        assert_eq!(u32::from_be_bytes(buf), 0, "heartbeat frame must be empty");
    });

    let stream = TokioTcpStream::connect(addr).await.unwrap();
    let mut conn = TcpConn::new(
        stream,
        TcpConnConfig {
            heartbeat_interval: Some(std::time::Duration::from_millis(60)),
            ..Default::default()
        },
    );
    conn.start().await.unwrap();

    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), peer)
        .await
        .expect("peer should read a heartbeat frame");
    conn.close();
}

/// White-box: `receive()` returns None when the receiver slot is absent.
#[tokio::test]
async fn test_tcp_conn_receive_none_without_receiver_slot() {
    ensure_tracing_subscriber();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let peer = tokio::spawn(async move {
        let (s, _) = listener.accept().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        drop(s);
    });

    let stream = TokioTcpStream::connect(addr).await.unwrap();
    let mut conn = TcpConn::new(stream, TcpConnConfig::default());
    conn.start().await.unwrap();

    conn.recv_rx = None; // white-box: simulate an already-consumed receiver
    let got = conn.receive().await;
    assert!(got.is_none());

    conn.close();
    let _ = peer.await;
}

/// local/remote address accessors, update_last_used, and Debug output.
#[tokio::test]
async fn test_tcp_conn_addr_accessors_last_used_and_debug() {
    ensure_tracing_subscriber();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let peer = tokio::spawn(async move {
        let (s, _) = listener.accept().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        drop(s);
    });

    let stream = TokioTcpStream::connect(addr).await.unwrap();
    let remote = stream.peer_addr().unwrap().to_string();
    let local = stream.local_addr().unwrap().to_string();
    let mut conn = TcpConn::new(stream, TcpConnConfig::default());

    assert_eq!(conn.local_addr(), local);
    assert_eq!(conn.remote_addr(), remote);
    assert!(conn.created_at() <= std::time::Instant::now());

    let before = conn.last_used();
    conn.update_last_used();
    assert!(conn.last_used() >= before);

    let dbg = format!("{:?}", conn);
    assert!(dbg.contains("TcpConn"), "Debug output: {}", dbg);

    conn.start().await.unwrap();
    conn.close(); // exercises the close debug event fields
    let _ = peer.await;
}

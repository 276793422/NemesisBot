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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let token = "secret-token-123";

    // Server: accept and read auth line (plain text with newline)
    let server_handle = tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;
        let (stream, _) = listener.accept().await.unwrap();
        let (read_half, _) = tokio::io::split(stream);
        let mut reader = tokio::io::BufReader::new(read_half);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        // The client sends "token\n"; trim the newline for comparison
        assert_eq!(line.trim(), token);
    });

    // Client: start with auth token (sent as plain text line)
    let client_stream = TokioTcpStream::connect(addr).await.unwrap();
    let mut client = TcpConn::new(
        client_stream,
        TcpConnConfig {
            auth_token: Some(token.to_string()),
            ..Default::default()
        },
    );
    client.start().await.unwrap();

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
    let err = ConnectionError::Io(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "io error"));
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

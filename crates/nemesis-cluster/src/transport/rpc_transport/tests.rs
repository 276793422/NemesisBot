use super::*;
use crate::rpc_types::ActionType;
use crate::transport::conn::{TcpConn, TcpConnConfig, WireMessage};
use crate::transport::pool::AsyncPoolConfig;

#[tokio::test]
async fn test_rpc_transport_roundtrip() {
    // Set up a mock server
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let server_addr = addr.clone();

    // Channel to signal when server has sent its response
    let (server_done_tx, server_done_rx) = tokio::sync::oneshot::channel();

    // Server: accept, read request, send response
    let server_handle = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut conn = TcpConn::new(
            stream,
            TcpConnConfig {
                address: server_addr.clone(),
                ..Default::default()
            },
        );
        conn.start().await.unwrap();

        // Read request
        let msg = conn.receive().await.unwrap();
        assert_eq!(msg.action, "Ping");

        // Send response
        let resp = WireMessage::new_response(&msg, serde_json::json!({"status": "ok"}));
        conn.send(&resp).await.unwrap();

        // Wait for the write loop to flush
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let _ = server_done_tx.send(());
    });

    // Client
    let pool = Pool::new(AsyncPoolConfig {
        max_conns: 10,
        max_conns_per_node: 2,
        dial_timeout: Duration::from_secs(5),
        ..Default::default()
    });
    let transport = RpcTransport::with_pool(pool);

    let request = RPCRequest {
        id: "test-req".into(),
        action: ActionType::Known(crate::rpc_types::KnownAction::Ping),
        payload: serde_json::json!({}),
        source: "client".into(),
        target: Some("server".into()),
    };

    let response = transport.call("server", &addr, request).await.unwrap();
    assert!(!response.id.is_empty());
    assert!(response.error.is_none());
    assert_eq!(response.result.unwrap()["status"], "ok");

    server_done_rx.await.unwrap();
    let _ = server_handle.await;
    transport.close();
}

// ============================================================
// Coverage improvement: error paths, config, stats
// ============================================================

#[test]
fn test_rpc_transport_default() {
    let transport = RpcTransport::default();
    let stats = transport.stats();
    assert_eq!(stats.active_conns, 0);
}

#[test]
fn test_rpc_transport_new() {
    let transport = RpcTransport::new();
    let stats = transport.stats();
    assert_eq!(stats.max_conns, 50);
}

#[test]
fn test_rpc_transport_with_config_custom_timeout() {
    let pool = Pool::new(AsyncPoolConfig {
        max_conns: 5,
        ..Default::default()
    });
    let transport = RpcTransport::with_config(pool, Duration::from_secs(30));
    let stats = transport.stats();
    assert_eq!(stats.max_conns, 5);
}

#[tokio::test]
async fn test_rpc_transport_call_pool_error() {
    let pool = Pool::new(AsyncPoolConfig {
        max_conns: 1,
        max_conns_per_node: 1,
        dial_timeout: Duration::from_millis(100),
        ..Default::default()
    });
    let transport = RpcTransport::with_config(pool, Duration::from_secs(5));

    // Connect to non-routable address -> pool error
    let request = RPCRequest {
        id: "test".into(),
        action: ActionType::Known(crate::rpc_types::KnownAction::Ping),
        payload: serde_json::json!({}),
        source: "client".into(),
        target: Some("server".into()),
    };

    let result = transport.call("node", "10.255.255.1:9999", request).await;
    assert!(result.is_err());
}

#[test]
fn test_rpc_transport_close() {
    let transport = RpcTransport::new();
    transport.close();
    let stats = transport.stats();
    assert_eq!(stats.active_conns, 0);
}

#[test]
fn test_rpc_transport_error_display() {
    let err = RpcTransportError::Connection("test conn error".to_string());
    assert!(err.to_string().contains("test conn error"));

    let err = RpcTransportError::Serialization("serde fail".to_string());
    assert!(err.to_string().contains("serde fail"));

    let err = RpcTransportError::RemoteError("remote said no".to_string());
    assert!(err.to_string().contains("remote said no"));

    let err = RpcTransportError::Timeout;
    assert!(err.to_string().contains("Timeout"));

    let err = RpcTransportError::Pool("pool exhausted".to_string());
    assert!(err.to_string().contains("pool exhausted"));
}

// ============================================================
// S4 coverage: error-response conversion, connection-closed
// during receive, and the receive timeout arm.
// ============================================================

fn s4_request() -> RPCRequest {
    RPCRequest {
        id: "s4-req".into(),
        action: ActionType::Known(crate::rpc_types::KnownAction::Ping),
        payload: serde_json::json!({}),
        source: "client".into(),
        target: Some("server".into()),
    }
}

fn s4_pool() -> Pool {
    Pool::new(AsyncPoolConfig {
        max_conns: 10,
        max_conns_per_node: 2,
        dial_timeout: Duration::from_secs(5),
        ..Default::default()
    })
}

/// Server replying with an error WireMessage: `call` converts it to
/// RPCResponse{error} and surfaces RemoteError (rpc_transport.rs 117-122,
/// 131-132).
#[tokio::test]
async fn test_rpc_transport_call_error_response() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let server_addr = addr.clone();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut conn = TcpConn::new(
            stream,
            TcpConnConfig {
                address: server_addr,
                ..Default::default()
            },
        );
        conn.start().await.unwrap();
        let msg = conn.receive().await.unwrap();
        let resp = WireMessage::new_error(&msg, "s4-boom");
        conn.send(&resp).await.unwrap();
        // Hold the connection open until the client finishes.
        tokio::time::sleep(Duration::from_millis(300)).await;
    });

    let transport = RpcTransport::with_pool(s4_pool());
    let err = transport
        .call("server", &addr, s4_request())
        .await
        .expect_err("error response must surface as RemoteError");
    match err {
        RpcTransportError::RemoteError(msg) => assert_eq!(msg, "s4-boom"),
        other => panic!("expected RemoteError, got {:?}", other),
    }

    transport.close();
    let _ = server.await;
}

/// Server closing the connection after reading the request: the receive loop
/// yields None → Connection("connection closed") (rpc_transport.rs 101-104,
/// 137).
#[tokio::test]
async fn test_rpc_transport_call_connection_closed() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut conn = TcpConn::new(stream, TcpConnConfig::default());
        conn.start().await.unwrap();
        let _ = conn.receive().await.unwrap();
        // Drop without responding.
        drop(conn);
    });

    let transport = RpcTransport::with_pool(s4_pool());
    let err = transport
        .call("server", &addr, s4_request())
        .await
        .expect_err("closed connection must fail");
    match err {
        RpcTransportError::Connection(msg) => assert!(msg.contains("connection closed")),
        other => panic!("expected Connection, got {:?}", other),
    }

    transport.close();
    let _ = server.await;
}

/// Server that never responds: the receive loop hits the configured timeout
/// (rpc_transport.rs 138).
#[tokio::test]
async fn test_rpc_transport_call_timeout() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let server_addr = addr.clone();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut conn = TcpConn::new(
            stream,
            TcpConnConfig {
                address: server_addr,
                ..Default::default()
            },
        );
        conn.start().await.unwrap();
        let _ = conn.receive().await.unwrap();
        // Read the request but never answer; hold the socket open.
        tokio::time::sleep(Duration::from_millis(600)).await;
    });

    let transport = RpcTransport::with_config(s4_pool(), Duration::from_millis(120));
    let err = transport
        .call("server", &addr, s4_request())
        .await
        .expect_err("silent server must time out");
    assert!(matches!(err, RpcTransportError::Timeout), "got {:?}", err);

    transport.close();
    let _ = server.await;
}

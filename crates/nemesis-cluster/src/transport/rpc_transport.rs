//! RPC transport - high-level RPC over framed TCP transport.
//!
//! Combines the connection pool with RPC request/response serialization
//! to provide a simple call interface.

use std::time::Duration;

use crate::rpc_types::{RPCRequest, RPCResponse};
use crate::transport::conn::WireMessage;
use crate::transport::pool::Pool;

/// Error type for RPC transport operations.
#[derive(Debug, thiserror::Error)]
pub enum RpcTransportError {
    #[error("Connection error: {0}")]
    Connection(String),
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("Response error from remote: {0}")]
    RemoteError(String),
    #[error("Timeout waiting for response")]
    Timeout,
    #[error("Pool error: {0}")]
    Pool(String),
}

/// High-level RPC transport using an async connection pool.
///
/// Mirrors Go's RPC client transport:
/// - Gets a connection from the pool
/// - Sends the request as a `WireMessage`
/// - Waits for the correlated response
/// - Returns the connection to the pool
pub struct RpcTransport {
    pool: Pool,
    timeout: Duration,
}

impl RpcTransport {
    /// Create a new RPC transport with the default pool and 60-second timeout.
    pub fn new() -> Self {
        Self {
            pool: Pool::with_defaults(),
            timeout: Duration::from_secs(60),
        }
    }

    /// Create a new RPC transport with a custom pool.
    pub fn with_pool(pool: Pool) -> Self {
        Self {
            pool,
            timeout: Duration::from_secs(60),
        }
    }

    /// Create a new RPC transport with a custom pool and timeout.
    pub fn with_config(pool: Pool, timeout: Duration) -> Self {
        Self { pool, timeout }
    }

    /// Send an RPC request and receive a response.
    ///
    /// Converts the `RPCRequest` to a `WireMessage`, sends it through the pool,
    /// waits for a response with the matching ID, and converts back.
    pub async fn call(
        &self,
        node_id: &str,
        address: &str,
        request: RPCRequest,
    ) -> Result<RPCResponse, RpcTransportError> {
        let (key, mut conn) = self
            .pool
            .get(node_id, address)
            .await
            .map_err(RpcTransportError::Pool)?;

        // Convert RPCRequest to WireMessage
        let wire_msg = WireMessage::new_request(
            &request.source,
            request.target.as_deref().unwrap_or(""),
            request.action.as_str(),
            request.payload,
        );
        let request_id = wire_msg.id.clone();

        // Send
        conn.send(&wire_msg)
            .await
            .map_err(RpcTransportError::Connection)?;

        // Receive with timeout
        let result = tokio::time::timeout(self.timeout, async {
            loop {
                match conn.receive().await {
                    Some(msg) => {
                        if msg.id == request_id {
                            return Ok(msg);
                        }
                        // Not our response, skip
                    }
                    None => {
                        return Err(RpcTransportError::Connection(
                            "connection closed".to_string(),
                        ));
                    }
                }
            }
        })
        .await;

        // Return connection to pool
        self.pool.return_connection(key, conn);

        match result {
            Ok(Ok(wire_response)) => {
                // Convert WireMessage to RPCResponse
                let response = if wire_response.is_error() {
                    RPCResponse {
                        id: wire_response.id,
                        result: None,
                        error: Some(wire_response.error),
                    }
                } else {
                    RPCResponse {
                        id: wire_response.id,
                        result: Some(wire_response.payload),
                        error: None,
                    }
                };

                if let Some(error) = &response.error {
                    return Err(RpcTransportError::RemoteError(error.clone()));
                }

                Ok(response)
            }
            Ok(Err(e)) => Err(e),
            Err(_) => Err(RpcTransportError::Timeout),
        }
    }

    /// Close all pooled connections.
    pub fn close(&self) {
        self.pool.close();
    }

    /// Get pool statistics.
    pub fn stats(&self) -> crate::transport::pool::PoolStats {
        self.pool.get_stats()
    }
}

impl Default for RpcTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
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
            let resp = WireMessage::new_response(
                &msg,
                serde_json::json!({"status": "ok"}),
            );
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
}

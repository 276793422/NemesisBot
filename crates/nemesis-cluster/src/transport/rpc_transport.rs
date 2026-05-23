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
mod tests;

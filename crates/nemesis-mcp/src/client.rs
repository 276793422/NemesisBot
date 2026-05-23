//! MCP client.
//!
//! Connects to an external MCP server process via a transport layer, using
//! newline-delimited JSON-RPC framing as defined by the MCP specification.

use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;

use crate::transport::{
    self, Transport, TransportError, TransportRequest,
};
use crate::types::*;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Error type for MCP client operations.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Server returned error: {0}")]
    Server(#[from] JSONRPCError),

    #[error("Transport error: {0}")]
    Transport(#[from] TransportError),

    #[error("Client is not connected")]
    NotConnected,

    #[error("Client has been closed")]
    Closed,

    #[error("Client is not initialized")]
    NotInitialized,

    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),
}

/// Result alias for client operations.
pub type ClientResult<T> = Result<T, ClientError>;

// ---------------------------------------------------------------------------
// Client trait
// ---------------------------------------------------------------------------

/// Trait defining the MCP client interface.
///
/// This mirrors the Go `Client` interface, allowing different implementations
/// for production (real transport) and testing (mock transport).
#[async_trait]
pub trait Client: Send + Sync {
    /// Perform the MCP initialization handshake.
    async fn initialize(&mut self) -> ClientResult<InitializeResult>;

    /// List available tools on the server.
    async fn list_tools(&mut self) -> ClientResult<Vec<McpTool>>;

    /// Invoke a tool on the server.
    async fn call_tool(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> ClientResult<ToolCallResult>;

    /// List available resources on the server.
    async fn list_resources(&mut self) -> ClientResult<Vec<Resource>>;

    /// Read a resource from the server.
    async fn read_resource(&mut self, uri: &str) -> ClientResult<ResourceContent>;

    /// List available prompts on the server.
    async fn list_prompts(&mut self) -> ClientResult<Vec<Prompt>>;

    /// Get a populated prompt from the server.
    async fn get_prompt(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> ClientResult<PromptResult>;

    /// Close the connection.
    async fn close(&mut self) -> ClientResult<()>;

    /// Return the server info obtained during initialization.
    fn server_info(&self) -> Option<&ServerInfo>;

    /// Return `true` if the client is connected and initialized.
    fn is_connected(&self) -> bool;
}

// ---------------------------------------------------------------------------
// McpClient
// ---------------------------------------------------------------------------

/// MCP client that communicates with a server via a pluggable transport.
///
/// The protocol uses newline-delimited JSON-RPC (each message is a single
/// line terminated by `\n`). Request/response correlation is done via
/// monotonically increasing integer IDs.
pub struct McpClient {
    /// The underlying transport (stdio, mock, etc.).
    transport: Box<dyn Transport>,
    /// Monotonic request id counter.
    next_id: AtomicU64,
    /// Whether the client has been closed.
    closed: bool,
    /// Whether initialization handshake has completed.
    initialized: bool,
    /// Server info obtained from the initialize handshake.
    server_info: Option<ServerInfo>,
    /// Capabilities obtained from the initialize handshake.
    capabilities: Option<ServerCapabilities>,
    /// Protocol version negotiated during initialization.
    protocol_version: Option<String>,
}

impl McpClient {
    /// Create a new client with the given transport.
    ///
    /// The client is not connected until `initialize()` is called.
    pub fn new(transport: Box<dyn Transport>) -> Self {
        Self {
            transport,
            next_id: AtomicU64::new(1),
            closed: false,
            initialized: false,
            server_info: None,
            capabilities: None,
            protocol_version: None,
        }
    }

    /// Create a new client from a server configuration using stdio transport.
    ///
    /// Convenience factory that builds a `StdioTransport` from the config and
    /// wraps it in a client.
    pub fn from_config(config: &ServerConfig) -> ClientResult<Self> {
        if config.command.is_empty() {
            return Err(ClientError::InvalidConfig(
                "server command cannot be empty".into(),
            ));
        }
        let stdio = crate::stdio_transport::StdioTransport::from_config(config);
        Ok(Self::new(Box::new(stdio)))
    }

    // -- Connection lifecycle ------------------------------------------------

    /// Connect the transport and perform the MCP initialization handshake.
    async fn do_initialize(&mut self) -> ClientResult<InitializeResult> {
        if self.closed {
            return Err(ClientError::Closed);
        }
        if self.initialized {
            return Err(ClientError::InvalidConfig(
                "client already initialized".into(),
            ));
        }

        // Connect the transport (starts subprocess for stdio).
        self.transport.connect().await?;

        // Build initialize request.
        let init_params = serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {
                "tools": {},
                "resources": {},
                "prompts": {}
            },
            "clientInfo": {
                "name": "nemesis-mcp-client",
                "version": env!("CARGO_PKG_VERSION")
            }
        });

        let resp = self.send_request("initialize", Some(init_params)).await?;

        if let Some(err) = &resp.error {
            return Err(ClientError::Server(err.clone()));
        }

        // Parse initialize result.
        let result: InitializeResult = resp
            .result
            .map(|v| serde_json::from_value(v))
            .transpose()?
            .ok_or_else(|| ClientError::InvalidConfig("empty initialize result".into()))?;

        // Update client state.
        self.protocol_version = Some(result.protocol_version.clone());
        self.server_info = Some(result.server_info.clone());
        self.capabilities = Some(result.capabilities.clone());
        self.initialized = true;

        // Send initialized notification (no response expected).
        self.send_notification("notifications/initialized", None)
            .await?;

        Ok(result)
    }

    // -- Low-level transport -------------------------------------------------

    /// Allocate the next request id.
    fn alloc_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Build a `TransportRequest` without sending it (for testing).
    pub fn build_request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> TransportRequest {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        TransportRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(serde_json::Value::Number(id.into())),
            method: method.to_string(),
            params,
        }
    }

    /// Send a JSON-RPC request via the transport and return the response.
    async fn send_request(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> ClientResult<JSONRPCResponse> {
        if self.closed {
            return Err(ClientError::Closed);
        }

        let id = self.alloc_id();
        let request = TransportRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(serde_json::Value::Number(id.into())),
            method: method.to_string(),
            params,
        };

        let transport_resp = self.transport.send(&request, 30_000).await?;
        let mcp_resp = transport::from_transport_response(&transport_resp);

        Ok(mcp_resp)
    }

    /// Send a JSON-RPC notification (no id, no response expected).
    async fn send_notification(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> ClientResult<()> {
        if self.closed {
            return Err(ClientError::Closed);
        }

        let notification = TransportRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: None, // notifications have no id
            method: method.to_string(),
            params,
        };

        // Send with a short timeout; we don't expect a response.
        // Some transports may return EOF or a timeout, which is fine for notifications.
        let result = self.transport.send(&notification, 1_000).await;
        // Ignore errors for notifications — they are fire-and-forget.
        let _ = result;

        Ok(())
    }

    /// Parse a JSON-RPC response from a raw string (for testing).
    pub fn parse_response(raw: &str) -> ClientResult<JSONRPCResponse> {
        Ok(serde_json::from_str(raw)?)
    }
}

#[async_trait]
impl Client for McpClient {
    async fn initialize(&mut self) -> ClientResult<InitializeResult> {
        self.do_initialize().await
    }

    async fn list_tools(&mut self) -> ClientResult<Vec<McpTool>> {
        if !self.initialized {
            return Err(ClientError::NotInitialized);
        }

        let resp = self.send_request("tools/list", None).await?;

        if let Some(err) = &resp.error {
            return Err(ClientError::Server(err.clone()));
        }

        let tools: Vec<McpTool> = resp
            .result
            .and_then(|r| r.get("tools").cloned())
            .map(|v| serde_json::from_value(v).unwrap_or_default())
            .unwrap_or_default();

        Ok(tools)
    }

    async fn call_tool(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> ClientResult<ToolCallResult> {
        if !self.initialized {
            return Err(ClientError::NotInitialized);
        }

        let params = serde_json::json!({
            "name": name,
            "arguments": arguments,
        });

        let resp = self.send_request("tools/call", Some(params)).await?;

        if let Some(err) = &resp.error {
            return Err(ClientError::Server(err.clone()));
        }

        let result: ToolCallResult = resp
            .result
            .map(|v| serde_json::from_value(v).unwrap_or_default())
            .unwrap_or_default();

        Ok(result)
    }

    async fn list_resources(&mut self) -> ClientResult<Vec<Resource>> {
        if !self.initialized {
            return Err(ClientError::NotInitialized);
        }

        let resp = self.send_request("resources/list", None).await?;

        if let Some(err) = &resp.error {
            return Err(ClientError::Server(err.clone()));
        }

        let resources: Vec<Resource> = resp
            .result
            .and_then(|r| r.get("resources").cloned())
            .map(|v| serde_json::from_value(v).unwrap_or_default())
            .unwrap_or_default();

        Ok(resources)
    }

    async fn read_resource(&mut self, uri: &str) -> ClientResult<ResourceContent> {
        if !self.initialized {
            return Err(ClientError::NotInitialized);
        }

        let params = serde_json::json!({ "uri": uri });

        let resp = self
            .send_request("resources/read", Some(params))
            .await?;

        if let Some(err) = &resp.error {
            return Err(ClientError::Server(err.clone()));
        }

        let content: ResourceContent = resp
            .result
            .and_then(|r| r.get("contents").and_then(|c| c.get(0)).cloned())
            .map(|v| serde_json::from_value(v).unwrap_or_default())
            .unwrap_or_default();

        Ok(content)
    }

    async fn list_prompts(&mut self) -> ClientResult<Vec<Prompt>> {
        if !self.initialized {
            return Err(ClientError::NotInitialized);
        }

        let resp = self.send_request("prompts/list", None).await?;

        if let Some(err) = &resp.error {
            return Err(ClientError::Server(err.clone()));
        }

        let prompts: Vec<Prompt> = resp
            .result
            .and_then(|r| r.get("prompts").cloned())
            .map(|v| serde_json::from_value(v).unwrap_or_default())
            .unwrap_or_default();

        Ok(prompts)
    }

    async fn get_prompt(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> ClientResult<PromptResult> {
        if !self.initialized {
            return Err(ClientError::NotInitialized);
        }

        let params = serde_json::json!({
            "name": name,
            "arguments": arguments,
        });

        let resp = self.send_request("prompts/get", Some(params)).await?;

        if let Some(err) = &resp.error {
            return Err(ClientError::Server(err.clone()));
        }

        let result: PromptResult = resp
            .result
            .map(|v| serde_json::from_value(v).unwrap_or_default())
            .unwrap_or_default();

        Ok(result)
    }

    async fn close(&mut self) -> ClientResult<()> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        self.initialized = false;

        self.transport.close().await?;

        Ok(())
    }

    fn server_info(&self) -> Option<&ServerInfo> {
        self.server_info.as_ref()
    }

    fn is_connected(&self) -> bool {
        !self.closed && self.initialized && self.transport.is_connected()
    }
}

impl Default for McpClient {
    fn default() -> Self {
        Self::new(Box::new(crate::transport::MockTransport::new()))
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests;

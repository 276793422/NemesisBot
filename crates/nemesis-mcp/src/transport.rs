//! MCP transport abstraction.
//!
//! Defines the `Transport` trait that all MCP communication mechanisms must
//! implement, along with a mock transport suitable for testing.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::types::*;

// ---------------------------------------------------------------------------
// Transport-level JSON-RPC types
// ---------------------------------------------------------------------------
// These mirror the MCP-level types but are kept separate so the transport
// layer does not depend on higher-level domain types. The conversion between
// the two is handled by the client.

/// Transport-level JSON-RPC request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportRequest {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// Transport-level JSON-RPC response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportResponse {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<TransportError>,
}

/// Transport-level JSON-RPC error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

/// Convert an MCP-level `JSONRPCRequest` into a transport-level request.
pub fn to_transport_request(req: &JSONRPCRequest) -> TransportRequest {
    TransportRequest {
        jsonrpc: req.jsonrpc.clone(),
        id: req.id.clone(),
        method: req.method.clone(),
        params: req.params.clone(),
    }
}

/// Convert a transport-level response into an MCP-level `JSONRPCResponse`.
pub fn from_transport_response(resp: &TransportResponse) -> JSONRPCResponse {
    let error = resp.error.as_ref().map(|e| JSONRPCError {
        code: e.code,
        message: e.message.clone(),
        data: e.data.clone(),
    });
    JSONRPCResponse {
        jsonrpc: resp.jsonrpc.clone(),
        id: resp.id.clone(),
        result: resp.result.clone(),
        error,
    }
}

// ---------------------------------------------------------------------------
// Transport trait
// ---------------------------------------------------------------------------

/// Transport trait for MCP communication.
///
/// Implementations handle the low-level details of communicating with an MCP
/// server (e.g., via subprocess stdin/stdout, HTTP, etc.).
#[async_trait]
pub trait Transport: Send + Sync {
    /// Connect to the MCP server.
    ///
    /// For stdio transports, this starts the subprocess.
    /// For HTTP transports, this establishes the connection.
    async fn connect(&mut self) -> Result<(), TransportError>;

    /// Close the connection to the MCP server.
    ///
    /// For stdio transports, this terminates the subprocess.
    async fn close(&mut self) -> Result<(), TransportError>;

    /// Send a JSON-RPC request and wait for the response.
    ///
    /// Blocks until a response matching the request id is received or the
    /// timeout elapses. The `timeout` parameter is in milliseconds; 0 means
    /// use an implementation-defined default.
    async fn send(
        &mut self,
        request: &TransportRequest,
        timeout_ms: u64,
    ) -> Result<TransportResponse, TransportError>;

    /// Returns `true` if the transport is currently connected.
    fn is_connected(&self) -> bool;

    /// Returns the transport type name (e.g., "stdio", "mock").
    fn name(&self) -> &str;
}

// ---------------------------------------------------------------------------
// Error helpers
// ---------------------------------------------------------------------------

impl TransportError {
    /// Create a new transport error.
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// Not-connected error.
    pub fn not_connected() -> Self {
        Self::new(-1, "transport is not connected")
    }

    /// Send-failed error.
    pub fn send_failed(msg: impl Into<String>) -> Self {
        Self::new(-2, msg)
    }

    /// Timeout error.
    pub fn timeout() -> Self {
        Self::new(-3, "request timed out")
    }
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "transport error {}: {}", self.code, self.message)
    }
}

impl std::error::Error for TransportError {}

// ---------------------------------------------------------------------------
// Mock transport (for testing)
// ---------------------------------------------------------------------------

/// A pre-scripted response entry for the mock transport.
#[derive(Debug, Clone)]
pub struct MockResponse {
    /// The method this response is associated with.
    pub method: String,
    /// The response to return.
    pub response: TransportResponse,
}

/// A recorded request captured by the mock transport.
#[derive(Debug, Clone)]
pub struct RecordedRequest {
    pub method: String,
    pub params: Option<serde_json::Value>,
}

/// Mock transport for unit testing.
///
/// Pre-load responses via `add_response()`. Each call to `send()` pops the
/// next matching response. All sent requests are recorded and can be
/// inspected via `requests()`.
pub struct MockTransport {
    connected: bool,
    was_connected: bool,
    closed: bool,
    responses: Vec<MockResponse>,
    requests: Vec<RecordedRequest>,
}

impl MockTransport {
    /// Create a new mock transport (disconnected by default).
    pub fn new() -> Self {
        Self {
            connected: false,
            was_connected: false,
            closed: false,
            responses: Vec::new(),
            requests: Vec::new(),
        }
    }

    /// Create a mock transport that is already connected.
    pub fn new_connected() -> Self {
        Self {
            connected: true,
            was_connected: true,
            closed: false,
            responses: Vec::new(),
            requests: Vec::new(),
        }
    }

    /// Queue a response for the given method.
    pub fn add_response(&mut self, method: impl Into<String>, response: TransportResponse) {
        self.responses.push(MockResponse {
            method: method.into(),
            response,
        });
    }

    /// Queue a success response for the given method with arbitrary result JSON.
    pub fn add_success(&mut self, method: impl Into<String>, result: serde_json::Value) {
        let id = serde_json::Value::Number(1.into());
        self.add_response(
            method,
            TransportResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id,
                result: Some(result),
                error: None,
            },
        );
    }

    /// Queue an error response for the given method.
    pub fn add_error(&mut self, method: impl Into<String>, code: i32, message: impl Into<String>) {
        let id = serde_json::Value::Number(1.into());
        self.add_response(
            method,
            TransportResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id,
                result: None,
                error: Some(TransportError::new(code, message)),
            },
        );
    }

    /// Return all recorded requests and clear the log.
    pub fn take_requests(&mut self) -> Vec<RecordedRequest> {
        std::mem::take(&mut self.requests)
    }

    /// Return the number of recorded requests.
    pub fn request_count(&self) -> usize {
        self.requests.len()
    }

    /// Return whether `connect()` was ever called (even if later closed).
    /// Mirrors Go's `WasConnected()`.
    pub fn was_connected(&self) -> bool {
        self.was_connected
    }

    /// Return whether `close()` was called.
    /// Mirrors Go's `IsClosed()`.
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Clear all recorded requests without consuming them.
    /// Mirrors Go's `ClearRequests()`.
    pub fn clear_requests(&mut self) {
        self.requests.clear();
    }
}

impl Default for MockTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Transport for MockTransport {
    async fn connect(&mut self) -> Result<(), TransportError> {
        self.connected = true;
        self.was_connected = true;
        self.closed = false;
        Ok(())
    }

    async fn close(&mut self) -> Result<(), TransportError> {
        self.connected = false;
        self.closed = true;
        Ok(())
    }

    async fn send(
        &mut self,
        request: &TransportRequest,
        _timeout_ms: u64,
    ) -> Result<TransportResponse, TransportError> {
        if !self.connected {
            return Err(TransportError::not_connected());
        }

        // Record the request.
        self.requests.push(RecordedRequest {
            method: request.method.clone(),
            params: request.params.clone(),
        });

        // Find the next matching response.
        let idx = self
            .responses
            .iter()
            .position(|r| r.method == request.method);

        match idx {
            Some(i) => {
                let mock = self.responses.remove(i);
                // Patch the response id to match the request id.
                let mut resp = mock.response;
                resp.id = request.id.clone().unwrap_or(serde_json::Value::Null);
                Ok(resp)
            }
            None => Err(TransportError::send_failed(format!(
                "no mock response for method: {}",
                request.method
            ))),
        }
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    fn name(&self) -> &str {
        "mock"
    }
}

/// Thread-safe wrapper around a mock transport for use in async tests.
#[derive(Clone)]
pub struct SharedMockTransport {
    inner: Arc<Mutex<MockTransport>>,
}

impl SharedMockTransport {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(MockTransport::new_connected())),
        }
    }

    pub async fn add_success(&self, method: impl Into<String>, result: serde_json::Value) {
        self.inner.lock().await.add_success(method, result);
    }

    pub async fn add_error(
        &self,
        method: impl Into<String>,
        code: i32,
        message: impl Into<String>,
    ) {
        self.inner.lock().await.add_error(method, code, message);
    }

    pub async fn take_requests(&self) -> Vec<RecordedRequest> {
        self.inner.lock().await.take_requests()
    }

    pub async fn request_count(&self) -> usize {
        self.inner.lock().await.request_count()
    }
}

impl Default for SharedMockTransport {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Mock response factory helpers (matching Go's CreateMock*Response functions)
// ---------------------------------------------------------------------------

/// Create a mock initialize response.
pub fn create_mock_initialize_response(
    server_name: &str,
    server_version: &str,
) -> TransportResponse {
    TransportResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: serde_json::Value::Number(1.into()),
        result: Some(serde_json::json!({
            "protocolVersion": "2024-11-05",
            "serverInfo": {
                "name": server_name,
                "version": server_version,
            },
            "capabilities": {
                "tools": { "listChanged": true },
            },
        })),
        error: None,
    }
}

/// Create a mock tools/list response.
pub fn create_mock_tools_list_response(
    tools: Vec<(&str, &str)>,
) -> TransportResponse {
    let tool_list: Vec<serde_json::Value> = tools
        .into_iter()
        .map(|(name, desc)| {
            serde_json::json!({
                "name": name,
                "description": desc,
                "inputSchema": { "type": "object", "properties": {} },
            })
        })
        .collect();

    TransportResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: serde_json::Value::Number(2.into()),
        result: Some(serde_json::json!({ "tools": tool_list })),
        error: None,
    }
}

/// Create a mock tool call response.
pub fn create_mock_tool_call_response(content: &str) -> TransportResponse {
    TransportResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: serde_json::Value::Number(3.into()),
        result: Some(serde_json::json!({
            "content": [{ "type": "text", "text": content }],
            "isError": false,
        })),
        error: None,
    }
}

/// Create a mock resources/list response.
pub fn create_mock_resources_list_response(
    resources: Vec<(&str, &str)>,
) -> TransportResponse {
    let resource_list: Vec<serde_json::Value> = resources
        .into_iter()
        .map(|(uri, name)| {
            serde_json::json!({
                "uri": uri,
                "name": name,
            })
        })
        .collect();

    TransportResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: serde_json::Value::Number(4.into()),
        result: Some(serde_json::json!({ "resources": resource_list })),
        error: None,
    }
}

/// Create a mock error response.
pub fn create_mock_error_response(id: i64, message: &str) -> TransportResponse {
    TransportResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: serde_json::Value::Number(id.into()),
        result: None,
        error: Some(TransportError::new(-32603, message)),
    }
}

/// Create a mock prompts/list response.
/// Mirrors Go's `CreateMockPromptsListResponse(prompts)`.
pub fn create_mock_prompts_list_response(
    prompts: Vec<(&str, &str)>,
) -> TransportResponse {
    let prompt_list: Vec<serde_json::Value> = prompts
        .into_iter()
        .map(|(name, description)| {
            serde_json::json!({
                "name": name,
                "description": description,
            })
        })
        .collect();

    TransportResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: serde_json::Value::Number(5.into()),
        result: Some(serde_json::json!({ "prompts": prompt_list })),
        error: None,
    }
}

/// Create a mock resources/read response.
/// Mirrors Go's `CreateMockReadResourceResponse(contents)`.
pub fn create_mock_read_resource_response(
    uri: &str,
    contents: &str,
) -> TransportResponse {
    TransportResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: serde_json::Value::Number(6.into()),
        result: Some(serde_json::json!({
            "contents": [{
                "uri": uri,
                "mimeType": "text/plain",
                "text": contents,
            }],
        })),
        error: None,
    }
}

/// Create a mock prompts/get response.
/// Mirrors Go's `CreateMockGetPromptResponse(messages)`.
pub fn create_mock_get_prompt_response(
    messages: Vec<(&str, &str)>,
) -> TransportResponse {
    let msg_list: Vec<serde_json::Value> = messages
        .into_iter()
        .map(|(role, content)| {
            serde_json::json!({
                "role": role,
                "content": { "type": "text", "text": content },
            })
        })
        .collect();

    TransportResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: serde_json::Value::Number(7.into()),
        result: Some(serde_json::json!({
            "description": "mock prompt",
            "messages": msg_list,
        })),
        error: None,
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests;

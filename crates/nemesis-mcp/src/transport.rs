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
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_transport_connect_close() {
        let mut t = MockTransport::new();
        assert!(!t.is_connected());

        t.connect().await.unwrap();
        assert!(t.is_connected());

        t.close().await.unwrap();
        assert!(!t.is_connected());
    }

    #[tokio::test]
    async fn mock_transport_send_receive() {
        let mut t = MockTransport::new_connected();

        t.add_success(
            "tools/list",
            serde_json::json!({ "tools": [{ "name": "echo" }] }),
        );

        let req = TransportRequest {
            jsonrpc: "2.0".into(),
            id: Some(serde_json::Value::Number(1.into())),
            method: "tools/list".into(),
            params: None,
        };

        let resp = t.send(&req, 0).await.unwrap();
        assert!(resp.error.is_none());
        assert!(resp.result.is_some());

        // Request should have been recorded.
        assert_eq!(t.request_count(), 1);
        let reqs = t.take_requests();
        assert_eq!(reqs[0].method, "tools/list");
    }

    #[tokio::test]
    async fn mock_transport_not_connected_error() {
        let mut t = MockTransport::new();
        let req = TransportRequest {
            jsonrpc: "2.0".into(),
            id: Some(serde_json::Value::Number(1.into())),
            method: "ping".into(),
            params: None,
        };
        let result = t.send(&req, 0).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn mock_transport_missing_response_error() {
        let mut t = MockTransport::new_connected();
        let req = TransportRequest {
            jsonrpc: "2.0".into(),
            id: Some(serde_json::Value::Number(1.into())),
            method: "tools/list".into(),
            params: None,
        };
        let result = t.send(&req, 0).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn conversion_helpers() {
        let mcp_req = JSONRPCRequest::new("tools/list", None);
        let t_req = to_transport_request(&mcp_req);
        assert_eq!(t_req.method, "tools/list");
        assert_eq!(t_req.jsonrpc, "2.0");

        let t_resp = TransportResponse {
            jsonrpc: "2.0".into(),
            id: serde_json::Value::Number(42.into()),
            result: Some(serde_json::json!({"ok": true})),
            error: None,
        };
        let mcp_resp = from_transport_response(&t_resp);
        assert!(!mcp_resp.is_error());
        assert_eq!(mcp_resp.id, serde_json::Value::Number(42.into()));
    }

    #[tokio::test]
    async fn mock_transport_add_error_response() {
        let mut t = MockTransport::new_connected();
        t.add_error("tools/call", -32602, "Invalid params");

        let req = TransportRequest {
            jsonrpc: "2.0".into(),
            id: Some(serde_json::Value::Number(1.into())),
            method: "tools/call".into(),
            params: None,
        };

        let resp = t.send(&req, 0).await.unwrap();
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32602);
    }

    #[tokio::test]
    async fn mock_transport_multiple_responses() {
        let mut t = MockTransport::new_connected();
        t.add_success("method1", serde_json::json!({"result": "first"}));
        t.add_success("method2", serde_json::json!({"result": "second"}));

        let req1 = TransportRequest {
            jsonrpc: "2.0".into(),
            id: Some(serde_json::Value::Number(1.into())),
            method: "method1".into(),
            params: None,
        };
        let resp1 = t.send(&req1, 0).await.unwrap();
        assert!(resp1.result.is_some());

        let req2 = TransportRequest {
            jsonrpc: "2.0".into(),
            id: Some(serde_json::Value::Number(2.into())),
            method: "method2".into(),
            params: None,
        };
        let resp2 = t.send(&req2, 0).await.unwrap();
        assert!(resp2.result.is_some());

        assert_eq!(t.request_count(), 2);
    }

    #[tokio::test]
    async fn mock_transport_double_close() {
        let mut t = MockTransport::new_connected();
        t.close().await.unwrap();
        assert!(!t.is_connected());
        t.close().await.unwrap();
        assert!(!t.is_connected());
    }

    #[tokio::test]
    async fn mock_transport_double_connect() {
        let mut t = MockTransport::new_connected();
        t.connect().await.unwrap();
        assert!(t.is_connected());
        t.connect().await.unwrap();
        assert!(t.is_connected());
    }

    #[tokio::test]
    async fn mock_transport_close_and_reconnect() {
        let mut t = MockTransport::new_connected();
        t.add_success("test", serde_json::json!({"ok": true}));
        t.close().await.unwrap();
        assert!(!t.is_connected());

        // Reconnect - responses are still queued (close doesn't clear them)
        t.connect().await.unwrap();
        let req = TransportRequest {
            jsonrpc: "2.0".into(),
            id: Some(serde_json::Value::Number(1.into())),
            method: "test".into(),
            params: None,
        };
        let result = t.send(&req, 0).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn mock_transport_take_requests_clears() {
        let mut t = MockTransport::new_connected();
        t.add_success("test", serde_json::json!({}));

        let req = TransportRequest {
            jsonrpc: "2.0".into(),
            id: Some(serde_json::Value::Number(1.into())),
            method: "test".into(),
            params: None,
        };
        t.send(&req, 0).await.unwrap();

        let reqs = t.take_requests();
        assert_eq!(reqs.len(), 1);

        let reqs2 = t.take_requests();
        assert!(reqs2.is_empty());
    }

    #[test]
    fn transport_request_with_params() {
        let mcp_req = JSONRPCRequest::new("tools/call", Some(serde_json::json!({"name": "echo"})));
        let t_req = to_transport_request(&mcp_req);
        assert!(t_req.params.is_some());
        let params = t_req.params.unwrap();
        assert_eq!(params["name"], "echo");
    }

    #[test]
    fn transport_response_error_conversion() {
        let t_resp = TransportResponse {
            jsonrpc: "2.0".into(),
            id: serde_json::Value::Number(1.into()),
            result: None,
            error: Some(TransportError {
                code: -32600,
                message: "Invalid Request".to_string(),
                data: None,
            }),
        };
        let mcp_resp = from_transport_response(&t_resp);
        assert!(mcp_resp.is_error());
        assert_eq!(mcp_resp.error.unwrap().code, -32600);
    }

    // ---- New tests ----

    #[test]
    fn transport_error_display() {
        let e = TransportError::new(-42, "something broke");
        assert_eq!(format!("{e}"), "transport error -42: something broke");
    }

    #[test]
    fn transport_error_std_error() {
        let e = TransportError::not_connected();
        let _: &dyn std::error::Error = &e;
    }

    #[test]
    fn transport_error_not_connected() {
        let e = TransportError::not_connected();
        assert_eq!(e.code, -1);
        assert!(e.message.contains("not connected"));
    }

    #[test]
    fn transport_error_send_failed() {
        let e = TransportError::send_failed("write failed");
        assert_eq!(e.code, -2);
        assert!(e.message.contains("write failed"));
    }

    #[test]
    fn transport_error_timeout() {
        let e = TransportError::timeout();
        assert_eq!(e.code, -3);
        assert!(e.message.contains("timed out"));
    }

    #[test]
    fn transport_error_with_data() {
        let e = TransportError {
            code: -100,
            message: "custom".into(),
            data: Some(serde_json::json!({"retry": true})),
        };
        let json = serde_json::to_string(&e).unwrap();
        let rt: TransportError = serde_json::from_str(&json).unwrap();
        assert!(rt.data.is_some());
    }

    #[test]
    fn transport_request_serialization() {
        let tr = TransportRequest {
            jsonrpc: "2.0".into(),
            id: Some(serde_json::Value::Number(99.into())),
            method: "ping".into(),
            params: None,
        };
        let json = serde_json::to_string(&tr).unwrap();
        assert!(json.contains("\"id\":99"));
    }

    #[test]
    fn transport_response_serialization() {
        let tr = TransportResponse {
            jsonrpc: "2.0".into(),
            id: serde_json::Value::Number(1.into()),
            result: Some(serde_json::json!({"ok": true})),
            error: None,
        };
        let json = serde_json::to_string(&tr).unwrap();
        assert!(json.contains("\"ok\":true"));
    }

    #[test]
    fn transport_error_serialization() {
        let te = TransportError::new(-100, "test");
        let json = serde_json::to_string(&te).unwrap();
        assert!(json.contains("\"code\":-100"));
    }

    #[test]
    fn to_transport_request_preserves_fields() {
        let req = JSONRPCRequest {
            jsonrpc: "2.0".into(),
            id: Some(serde_json::Value::String("abc".into())),
            method: "test".into(),
            params: Some(serde_json::json!({"x": 1})),
        };
        let tr = to_transport_request(&req);
        assert_eq!(tr.jsonrpc, "2.0");
        assert_eq!(tr.id, Some(serde_json::Value::String("abc".into())));
        assert_eq!(tr.method, "test");
        assert!(tr.params.is_some());
    }

    #[test]
    fn from_transport_response_with_error_data() {
        let tr = TransportResponse {
            jsonrpc: "2.0".into(),
            id: serde_json::Value::Null,
            result: None,
            error: Some(TransportError {
                code: -32603,
                message: "err".into(),
                data: Some(serde_json::json!({"info": "extra"})),
            }),
        };
        let mr = from_transport_response(&tr);
        let err = mr.error.unwrap();
        assert!(err.data.is_some());
    }

    #[tokio::test]
    async fn mock_transport_default_is_disconnected() {
        let t = MockTransport::default();
        assert!(!t.is_connected());
    }

    #[tokio::test]
    async fn mock_transport_was_connected_flag() {
        let mut t = MockTransport::new();
        assert!(!t.was_connected());
        t.connect().await.unwrap();
        assert!(t.was_connected());
        t.close().await.unwrap();
        assert!(t.was_connected());
    }

    #[tokio::test]
    async fn mock_transport_is_closed_flag() {
        let mut t = MockTransport::new_connected();
        assert!(!t.is_closed());
        t.close().await.unwrap();
        assert!(t.is_closed());
    }

    #[tokio::test]
    async fn mock_transport_clear_requests() {
        let mut t = MockTransport::new_connected();
        t.add_success("test", serde_json::json!({}));
        let req = TransportRequest {
            jsonrpc: "2.0".into(),
            id: Some(serde_json::Value::Number(1.into())),
            method: "test".into(),
            params: None,
        };
        t.send(&req, 0).await.unwrap();
        assert_eq!(t.request_count(), 1);
        t.clear_requests();
        assert_eq!(t.request_count(), 0);
    }

    #[tokio::test]
    async fn mock_transport_records_params() {
        let mut t = MockTransport::new_connected();
        t.add_success("method", serde_json::json!({}));
        let req = TransportRequest {
            jsonrpc: "2.0".into(),
            id: Some(serde_json::Value::Number(1.into())),
            method: "method".into(),
            params: Some(serde_json::json!({"key": "val"})),
        };
        t.send(&req, 0).await.unwrap();
        let reqs = t.take_requests();
        assert_eq!(reqs[0].params.as_ref().unwrap()["key"], "val");
    }

    #[test]
    fn create_mock_initialize_response_fields() {
        let resp = create_mock_initialize_response("test-srv", "1.0");
        assert!(resp.result.is_some());
        let result = resp.result.unwrap();
        assert_eq!(result["serverInfo"]["name"], "test-srv");
        assert_eq!(result["serverInfo"]["version"], "1.0");
    }

    #[test]
    fn create_mock_tools_list_response_fields() {
        let resp = create_mock_tools_list_response(vec![("echo", "Echo tool"), ("add", "Add tool")]);
        let binding = resp.result.unwrap();
        let tools = binding["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["name"], "echo");
    }

    #[test]
    fn create_mock_tool_call_response_fields() {
        let resp = create_mock_tool_call_response("result text");
        let result = resp.result.unwrap();
        assert_eq!(result["content"][0]["text"], "result text");
        assert_eq!(result["isError"], false);
    }

    #[test]
    fn create_mock_resources_list_response_fields() {
        let resp = create_mock_resources_list_response(vec![("file:///a.txt", "a.txt")]);
        let binding = resp.result.unwrap();
        let resources = binding["resources"].as_array().unwrap();
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0]["uri"], "file:///a.txt");
    }

    #[test]
    fn create_mock_error_response_fields() {
        let resp = create_mock_error_response(42, "bad");
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().message, "bad");
        assert_eq!(resp.id, serde_json::Value::Number(42.into()));
    }

    #[test]
    fn test_create_mock_prompts_list_response() {
        let resp = create_mock_prompts_list_response(vec![("greet", "Greet someone")]);
        let binding = resp.result.unwrap();
        let prompts = binding["prompts"].as_array().unwrap();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0]["name"], "greet");
        assert_eq!(prompts[0]["description"], "Greet someone");
    }

    #[test]
    fn test_create_mock_read_resource_response() {
        let resp = create_mock_read_resource_response("file:///x.txt", "hello");
        let binding = resp.result.unwrap();
        let contents = binding["contents"].as_array().unwrap();
        assert_eq!(contents[0]["uri"], "file:///x.txt");
        assert_eq!(contents[0]["text"], "hello");
    }

    #[test]
    fn test_create_mock_get_prompt_response() {
        let resp = create_mock_get_prompt_response(vec![
            ("user", "Hello"),
            ("assistant", "Hi there"),
        ]);
        let result = resp.result.unwrap();
        let msgs = result["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[1]["role"], "assistant");
    }

    #[tokio::test]
    async fn shared_mock_transport_add_success() {
        let t = SharedMockTransport::new();
        t.add_success("test", serde_json::json!({"ok": true})).await;
        assert_eq!(t.request_count().await, 0);
    }

    #[tokio::test]
    async fn shared_mock_transport_add_error() {
        let t = SharedMockTransport::new();
        t.add_error("test", -1, "fail").await;
        assert_eq!(t.request_count().await, 0);
    }

    #[tokio::test]
    async fn shared_mock_transport_default() {
        let t = SharedMockTransport::default();
        assert_eq!(t.request_count().await, 0);
    }

    #[test]
    fn mock_response_debug() {
        let mr = MockResponse {
            method: "test".into(),
            response: TransportResponse {
                jsonrpc: "2.0".into(),
                id: serde_json::Value::Null,
                result: None,
                error: None,
            },
        };
        let debug = format!("{:?}", mr);
        assert!(debug.contains("test"));
    }

    #[test]
    fn recorded_request_debug() {
        let rr = RecordedRequest {
            method: "m".into(),
            params: None,
        };
        let debug = format!("{:?}", rr);
        assert!(debug.contains("\"m\""));
    }
}

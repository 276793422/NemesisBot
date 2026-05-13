//! MCP protocol types.
//!
//! Defines all types used in the Model Context Protocol (MCP),
//! including JSON-RPC message structures, tool/resource definitions,
//! and server capability advertisements.

use serde::{Deserialize, Serialize};
use std::fmt;

/// MCP protocol version string.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// JSON-RPC protocol version string.
pub const JSONRPC_VERSION: &str = "2.0";

// ---------------------------------------------------------------------------
// JSON-RPC message types
// ---------------------------------------------------------------------------

/// A JSON-RPC 2.0 request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JSONRPCRequest {
    /// JSON-RPC version — always "2.0".
    pub jsonrpc: String,
    /// Request identifier. May be a number or a string; `None` signals a
    /// notification (no response expected).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    /// The method to invoke.
    pub method: String,
    /// Parameters for the method.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl JSONRPCRequest {
    /// Create a new request with an auto-generated string id.
    pub fn new(method: impl Into<String>, params: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(serde_json::Value::String(uuid::Uuid::new_v4().to_string())),
            method: method.into(),
            params,
        }
    }

    /// Create a notification (no id, no response expected).
    pub fn notification(method: impl Into<String>, params: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: None,
            method: method.into(),
            params,
        }
    }
}

/// A JSON-RPC 2.0 response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JSONRPCResponse {
    /// JSON-RPC version — always "2.0".
    pub jsonrpc: String,
    /// Identifier matching the original request.
    pub id: serde_json::Value,
    /// The result payload (present on success).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// The error payload (present on failure).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JSONRPCError>,
}

impl JSONRPCResponse {
    /// Build a success response.
    pub fn success(id: serde_json::Value, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// Build an error response.
    pub fn error(id: serde_json::Value, error: JSONRPCError) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: None,
            error: Some(error),
        }
    }

    /// Returns `true` when the response carries an error.
    pub fn is_error(&self) -> bool {
        self.error.is_some()
    }
}

/// A JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JSONRPCError {
    /// Numeric error code.
    pub code: i32,
    /// Human-readable error message.
    pub message: String,
    /// Optional additional data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl JSONRPCError {
    // Standard JSON-RPC error codes ----------------------------------------

    /// Invalid JSON was received.
    pub const PARSE_ERROR: i32 = -32700;
    /// The JSON sent is not a valid request object.
    pub const INVALID_REQUEST: i32 = -32600;
    /// The method does not exist / is not available.
    pub const METHOD_NOT_FOUND: i32 = -32601;
    /// Invalid method parameter(s).
    pub const INVALID_PARAMS: i32 = -32602;
    /// Internal JSON-RPC error.
    pub const INTERNAL_ERROR: i32 = -32603;

    /// Create a new error with the given code and message.
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// Convenience: method-not-found error.
    pub fn method_not_found(method: &str) -> Self {
        Self::new(Self::METHOD_NOT_FOUND, format!("Method not found: {method}"))
    }

    /// Convenience: invalid-params error.
    pub fn invalid_params(msg: impl Into<String>) -> Self {
        Self::new(Self::INVALID_PARAMS, msg)
    }

    /// Convenience: internal error.
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::new(Self::INTERNAL_ERROR, msg)
    }
}

impl fmt::Display for JSONRPCError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JSON-RPC error {}: {}", self.code, self.message)
    }
}

impl std::error::Error for JSONRPCError {}

// ---------------------------------------------------------------------------
// MCP domain types
// ---------------------------------------------------------------------------

/// Description of a tool exposed by an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    /// The tool name.
    pub name: String,
    /// Human-readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema describing the expected input.
    #[serde(rename = "inputSchema")]
    pub input_schema: serde_json::Value,
}

/// A single content item returned from a tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolContent {
    /// MIME type of the content (e.g. "text/plain").
    #[serde(rename = "type")]
    pub content_type: String,
    /// Text payload (used when `content_type` is text-based).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

impl ToolContent {
    /// Convenience constructor for plain-text content.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content_type: "text".to_string(),
            text: Some(text.into()),
        }
    }
}

/// The result of invoking a tool.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolCallResult {
    /// Content items returned by the tool.
    #[serde(default)]
    pub content: Vec<ToolContent>,
    /// Whether the tool execution resulted in an error.
    #[serde(default, rename = "isError")]
    pub is_error: bool,
}

impl ToolCallResult {
    /// Build a successful text-only result.
    pub fn ok(text: impl Into<String>) -> Self {
        Self {
            content: vec![ToolContent::text(text)],
            is_error: false,
        }
    }

    /// Build an error result.
    pub fn err(text: impl Into<String>) -> Self {
        Self {
            content: vec![ToolContent::text(text)],
            is_error: true,
        }
    }
}

/// Description of a resource exposed by an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resource {
    /// URI identifying the resource.
    pub uri: String,
    /// Human-readable name.
    pub name: String,
    /// Optional description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// MIME type (e.g. "text/plain").
    #[serde(skip_serializing_if = "Option::is_none", rename = "mimeType")]
    pub mime_type: Option<String>,
}

/// Contents of a resource returned by the server.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceContent {
    /// URI of the resource.
    #[serde(default)]
    pub uri: String,
    /// MIME type.
    #[serde(skip_serializing_if = "Option::is_none", rename = "mimeType")]
    pub mime_type: Option<String>,
    /// Text content (for text-based resources).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

// ---------------------------------------------------------------------------
// Capability types
// ---------------------------------------------------------------------------

/// Capabilities advertised by an MCP server during initialization.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolCapabilities>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourceCapabilities>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompts: Option<PromptCapabilities>,
}

/// Tool-related capabilities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCapabilities {
    #[serde(skip_serializing_if = "Option::is_none", rename = "listChanged")]
    pub list_changed: Option<bool>,
}

/// Resource-related capabilities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscribe: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "listChanged")]
    pub list_changed: Option<bool>,
}

/// Prompt-related capabilities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptCapabilities {
    #[serde(skip_serializing_if = "Option::is_none", rename = "listChanged")]
    pub list_changed: Option<bool>,
}

/// Server identity sent during the initialize handshake.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

/// Client identity sent during the initialize handshake.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

/// Client capabilities sent during the initialize handshake.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClientCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompts: Option<serde_json::Value>,
}

/// Parameters for the initialize request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeParams {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    pub capabilities: ClientCapabilities,
    #[serde(rename = "clientInfo")]
    pub client_info: ClientInfo,
}

/// Result of the initialize request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeResult {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    #[serde(rename = "serverInfo")]
    pub server_info: ServerInfo,
}

// ---------------------------------------------------------------------------
// Prompt API types
// ---------------------------------------------------------------------------

/// A prompt template available on the MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prompt {
    /// Name of the prompt.
    pub name: String,
    /// Human-readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Arguments that the prompt accepts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<PromptArgument>,
}

/// An argument for a prompt template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptArgument {
    /// Name of the argument.
    pub name: String,
    /// Human-readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Whether this argument is required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
}

/// A message in a prompt result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptMessage {
    /// Role of the message sender: "user", "assistant", or "system".
    pub role: String,
    /// Content of the message.
    pub content: PromptMessageContent,
}

/// Content within a prompt message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptMessageContent {
    /// Content type: "text", "image", or "resource".
    #[serde(rename = "type")]
    pub content_type: String,
    /// Text payload (for text content).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Base64-encoded data (for image/resource content).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
}

impl PromptMessageContent {
    /// Convenience constructor for text content.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content_type: "text".to_string(),
            text: Some(text.into()),
            data: None,
        }
    }
}

/// Result of getting a prompt from the server.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PromptResult {
    /// Messages that make up the prompt.
    #[serde(default)]
    pub messages: Vec<PromptMessage>,
    /// Optional description of the prompt result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for spawning an external MCP server process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Display name for this server.
    pub name: String,
    /// Executable command (resolved via $PATH unless absolute).
    pub command: String,
    /// Arguments passed to the command.
    #[serde(default)]
    pub args: Vec<String>,
    /// Optional environment variables ("KEY=VALUE").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<Vec<String>>,
    /// Timeout in seconds for requests to this server.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_timeout() -> u64 {
    30
}

impl ServerConfig {
    /// Create a new server configuration with default timeout.
    pub fn new(name: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
            args: Vec::new(),
            env: None,
            timeout_secs: 30,
        }
    }

    /// Add an argument.
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Add an environment variable.
    pub fn env(mut self, kv: impl Into<String>) -> Self {
        self.env.get_or_insert_with(Vec::new).push(kv.into());
        self
    }

    /// Set a custom timeout.
    pub fn timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_and_deserialize_jsonrpc_request() {
        let req = JSONRPCRequest::new("tools/list", None);
        let json = serde_json::to_string(&req).unwrap();

        // Must contain the required fields
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"method\":\"tools/list\""));
        assert!(json.contains("\"id\":"));

        let roundtrip: JSONRPCRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip.jsonrpc, "2.0");
        assert_eq!(roundtrip.method, "tools/list");
        assert!(roundtrip.id.is_some());
    }

    #[test]
    fn serialize_and_deserialize_jsonrpc_response() {
        let resp = JSONRPCResponse::success(
            serde_json::Value::String("abc".into()),
            serde_json::json!({"tools": []}),
        );
        let json = serde_json::to_string(&resp).unwrap();
        let roundtrip: JSONRPCResponse = serde_json::from_str(&json).unwrap();

        assert!(!roundtrip.is_error());
        assert_eq!(roundtrip.id, serde_json::Value::String("abc".into()));
        assert!(roundtrip.result.is_some());
        assert!(roundtrip.error.is_none());
    }

    #[test]
    fn jsonrpc_error_display_and_std_error() {
        let err = JSONRPCError::new(-32601, "Method not found: foo");
        assert_eq!(
            format!("{err}"),
            "JSON-RPC error -32601: Method not found: foo"
        );
        // Verify it satisfies std::error::Error
        let _: &dyn std::error::Error = &err;
    }

    #[test]
    fn mcp_tool_and_tool_call_result_roundtrip() {
        let tool = McpTool {
            name: "read_file".into(),
            description: Some("Read a file".into()),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }),
        };
        let json = serde_json::to_string(&tool).unwrap();
        let rt: McpTool = serde_json::from_str(&json).unwrap();
        assert_eq!(rt.name, "read_file");

        let result = ToolCallResult::ok("hello");
        let rj = serde_json::to_string(&result).unwrap();
        let rr: ToolCallResult = serde_json::from_str(&rj).unwrap();
        assert!(!rr.is_error);
        assert_eq!(rr.content[0].text.as_deref(), Some("hello"));
    }

    #[test]
    fn server_config_default_and_builder() {
        let cfg = ServerConfig::new("test", "node")
            .arg("server.js")
            .env("FOO=bar")
            .timeout(60);

        assert_eq!(cfg.name, "test");
        assert_eq!(cfg.command, "node");
        assert_eq!(cfg.args, vec!["server.js"]);
        assert_eq!(cfg.env.as_ref(), Some(&vec!["FOO=bar".to_string()]));
        assert_eq!(cfg.timeout_secs, 60);

        // Serialization round-trip
        let json = serde_json::to_string(&cfg).unwrap();
        let rt: ServerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(rt.name, cfg.name);
        assert_eq!(rt.timeout_secs, 60);
    }

    #[test]
    fn prompt_types_roundtrip() {
        let prompt = Prompt {
            name: "greet".into(),
            description: Some("Greet a person".into()),
            arguments: vec![
                PromptArgument {
                    name: "name".into(),
                    description: Some("Person's name".into()),
                    required: Some(true),
                },
            ],
        };
        let json = serde_json::to_string(&prompt).unwrap();
        let rt: Prompt = serde_json::from_str(&json).unwrap();
        assert_eq!(rt.name, "greet");
        assert_eq!(rt.arguments.len(), 1);
        assert_eq!(rt.arguments[0].name, "name");

        let result = PromptResult {
            messages: vec![PromptMessage {
                role: "user".into(),
                content: PromptMessageContent::text("Hello, {{name}}!"),
            }],
            description: Some("Greeting prompt".into()),
        };
        let rj = serde_json::to_string(&result).unwrap();
        let rr: PromptResult = serde_json::from_str(&rj).unwrap();
        assert_eq!(rr.messages.len(), 1);
        assert_eq!(rr.messages[0].role, "user");
        assert_eq!(
            rr.messages[0].content.text.as_deref(),
            Some("Hello, {{name}}!")
        );
    }

    #[test]
    fn initialize_params_and_result_roundtrip() {
        let params = InitializeParams {
            protocol_version: PROTOCOL_VERSION.to_string(),
            capabilities: ClientCapabilities {
                tools: Some(serde_json::json!({})),
                resources: Some(serde_json::json!({})),
                prompts: Some(serde_json::json!({})),
            },
            client_info: ClientInfo {
                name: "test-client".into(),
                version: "1.0.0".into(),
            },
        };
        let json = serde_json::to_string(&params).unwrap();
        let rt: InitializeParams = serde_json::from_str(&json).unwrap();
        assert_eq!(rt.protocol_version, PROTOCOL_VERSION);
        assert_eq!(rt.client_info.name, "test-client");

        let init_result = InitializeResult {
            protocol_version: PROTOCOL_VERSION.to_string(),
            capabilities: ServerCapabilities::default(),
            server_info: ServerInfo {
                name: "test-server".into(),
                version: "2.0.0".into(),
            },
        };
        let rj = serde_json::to_string(&init_result).unwrap();
        let rr: InitializeResult = serde_json::from_str(&rj).unwrap();
        assert_eq!(rr.server_info.name, "test-server");
    }

    #[test]
    fn prompt_message_content_helpers() {
        let content = PromptMessageContent::text("hello world");
        assert_eq!(content.content_type, "text");
        assert_eq!(content.text.as_deref(), Some("hello world"));
        assert!(content.data.is_none());
    }

    #[test]
    fn jsonrpc_request_with_params() {
        let params = serde_json::json!({"name": "test_tool"});
        let req = JSONRPCRequest::new("tools/call", Some(params.clone()));
        assert_eq!(req.method, "tools/call");
        assert_eq!(req.params, Some(params));
    }

    #[test]
    fn jsonrpc_request_with_string_id() {
        let req = JSONRPCRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::Value::String("test-id-123".to_string())),
            method: "tools/list".to_string(),
            params: None,
        };
        assert_eq!(req.id, Some(serde_json::Value::String("test-id-123".to_string())));
        assert_eq!(req.method, "tools/list");
    }

    #[test]
    fn jsonrpc_response_error() {
        let resp = JSONRPCResponse::error(
            serde_json::Value::String("err-id".into()),
            JSONRPCError::new(-32600, "Invalid Request"),
        );
        assert!(resp.is_error());
        assert!(resp.error.is_some());
        assert!(resp.result.is_none());
        assert_eq!(resp.id, serde_json::Value::String("err-id".into()));
    }

    #[test]
    fn jsonrpc_response_null_id() {
        let resp = JSONRPCResponse::error(
            serde_json::Value::Null,
            JSONRPCError::new(-32700, "Parse error"),
        );
        assert!(resp.is_error());
        assert_eq!(resp.id, serde_json::Value::Null);
    }

    #[test]
    fn jsonrpc_error_codes() {
        let err = JSONRPCError::new(-32700, "Parse error");
        assert_eq!(err.code, -32700);

        let err = JSONRPCError::new(-32600, "Invalid Request");
        assert_eq!(err.code, -32600);

        let err = JSONRPCError::new(-32601, "Method not found");
        assert_eq!(err.code, -32601);

        let err = JSONRPCError::new(-32602, "Invalid params");
        assert_eq!(err.code, -32602);

        let err = JSONRPCError::new(-32603, "Internal error");
        assert_eq!(err.code, -32603);
    }

    #[test]
    fn tool_call_result_error() {
        let result = ToolCallResult::err("file not found");
        assert!(result.is_error);
        assert_eq!(result.content[0].text.as_deref(), Some("file not found"));
    }

    #[test]
    fn tool_call_result_serialization() {
        let result = ToolCallResult::ok("success output");
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"isError\":false"));
        assert!(json.contains("\"content\":"));

        let parsed: ToolCallResult = serde_json::from_str(&json).unwrap();
        assert!(!parsed.is_error);
    }

    #[test]
    fn server_config_multiple_args() {
        let cfg = ServerConfig::new("multi", "python")
            .arg("server.py")
            .arg("--port")
            .arg("8080")
            .env("DEBUG=1")
            .env("LOG_LEVEL=info")
            .timeout(120);

        assert_eq!(cfg.args, vec!["server.py", "--port", "8080"]);
        assert_eq!(cfg.env.as_ref().map(|e| e.len()), Some(2));
        assert_eq!(cfg.timeout_secs, 120);
    }

    #[test]
    fn server_config_default_timeout() {
        let cfg = ServerConfig::new("test", "node");
        assert_eq!(cfg.timeout_secs, 30);
    }

    #[test]
    fn protocol_version_constant() {
        assert_eq!(PROTOCOL_VERSION, "2025-06-18");
    }

    #[test]
    fn client_capabilities_default() {
        let caps = ClientCapabilities::default();
        assert!(caps.tools.is_none());
        assert!(caps.resources.is_none());
        assert!(caps.prompts.is_none());
    }

    #[test]
    fn server_capabilities_default() {
        let caps = ServerCapabilities::default();
        assert!(caps.tools.is_none());
        assert!(caps.resources.is_none());
        assert!(caps.prompts.is_none());
    }

    #[test]
    fn mcp_tool_without_description() {
        let tool = McpTool {
            name: "simple_tool".into(),
            description: None,
            input_schema: serde_json::json!({"type": "object"}),
        };
        let json = serde_json::to_string(&tool).unwrap();
        let rt: McpTool = serde_json::from_str(&json).unwrap();
        assert_eq!(rt.name, "simple_tool");
        assert!(rt.description.is_none());
    }

    #[test]
    fn prompt_without_arguments() {
        let prompt = Prompt {
            name: "simple".into(),
            description: None,
            arguments: vec![],
        };
        let json = serde_json::to_string(&prompt).unwrap();
        let rt: Prompt = serde_json::from_str(&json).unwrap();
        assert_eq!(rt.arguments.len(), 0);
    }

    #[test]
    fn prompt_message_content_image() {
        let content = PromptMessageContent {
            content_type: "image".to_string(),
            text: None,
            data: Some("base64encodeddata".to_string()),
        };
        assert_eq!(content.content_type, "image");
        assert!(content.text.is_none());
        assert!(content.data.is_some());
    }

    #[test]
    fn jsonrpc_request_deserialize_with_null_params() {
        let json = r#"{"jsonrpc":"2.0","method":"ping","id":1,"params":null}"#;
        let req: JSONRPCRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, "ping");
        assert!(req.params.is_none());
    }

    #[test]
    fn initialize_params_serialization() {
        let params = InitializeParams {
            protocol_version: PROTOCOL_VERSION.to_string(),
            capabilities: ClientCapabilities::default(),
            client_info: ClientInfo {
                name: "test".into(),
                version: "1.0".into(),
            },
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("protocolVersion"));
        assert!(json.contains("capabilities"));
        assert!(json.contains("clientInfo"));
    }

    #[test]
    fn jsonrpc_request_notification_no_id() {
        let req = JSONRPCRequest::notification("some/event", Some(serde_json::json!({"data": 1})));
        assert!(req.id.is_none());
        assert_eq!(req.method, "some/event");
        assert!(req.params.is_some());
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("\"id\":"));
    }

    #[test]
    fn jsonrpc_error_convenience_methods() {
        let e1 = JSONRPCError::method_not_found("foo/bar");
        assert_eq!(e1.code, JSONRPCError::METHOD_NOT_FOUND);
        assert!(e1.message.contains("foo/bar"));

        let e2 = JSONRPCError::invalid_params("missing x");
        assert_eq!(e2.code, JSONRPCError::INVALID_PARAMS);

        let e3 = JSONRPCError::internal("boom");
        assert_eq!(e3.code, JSONRPCError::INTERNAL_ERROR);
    }

    #[test]
    fn tool_content_text_helper() {
        let tc = ToolContent::text("hello");
        assert_eq!(tc.content_type, "text");
        assert_eq!(tc.text.as_deref(), Some("hello"));
    }

    #[test]
    fn tool_call_result_default() {
        let tcr = ToolCallResult::default();
        assert!(tcr.content.is_empty());
        assert!(!tcr.is_error);
    }

    #[test]
    fn resource_serialization_roundtrip() {
        let r = Resource {
            uri: "file:///x.txt".into(),
            name: "x".into(),
            description: Some("desc".into()),
            mime_type: Some("text/plain".into()),
        };
        let json = serde_json::to_string(&r).unwrap();
        let rt: Resource = serde_json::from_str(&json).unwrap();
        assert_eq!(rt.uri, "file:///x.txt");
        assert_eq!(rt.name, "x");
        assert_eq!(rt.description.as_deref(), Some("desc"));
        assert_eq!(rt.mime_type.as_deref(), Some("text/plain"));
    }

    #[test]
    fn resource_content_default() {
        let rc = ResourceContent::default();
        assert!(rc.uri.is_empty());
        assert!(rc.mime_type.is_none());
        assert!(rc.text.is_none());
    }

    #[test]
    fn resource_content_roundtrip() {
        let rc = ResourceContent {
            uri: "mem://data".into(),
            mime_type: Some("application/json".into()),
            text: Some(r#"{"key":"val"}"#.into()),
        };
        let json = serde_json::to_string(&rc).unwrap();
        let rt: ResourceContent = serde_json::from_str(&json).unwrap();
        assert_eq!(rt.uri, "mem://data");
        assert_eq!(rt.text.as_deref(), Some(r#"{"key":"val"}"#));
    }

    #[test]
    fn server_info_serialization() {
        let si = ServerInfo { name: "srv".into(), version: "0.1".into() };
        let json = serde_json::to_string(&si).unwrap();
        let rt: ServerInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(rt.name, "srv");
        assert_eq!(rt.version, "0.1");
    }

    #[test]
    fn client_info_serialization() {
        let ci = ClientInfo { name: "cli".into(), version: "2.0".into() };
        let json = serde_json::to_string(&ci).unwrap();
        let rt: ClientInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(rt.name, "cli");
    }

    #[test]
    fn initialize_result_roundtrip() {
        let ir = InitializeResult {
            protocol_version: "2025-06-18".into(),
            capabilities: ServerCapabilities {
                tools: Some(ToolCapabilities { list_changed: Some(true) }),
                resources: None,
                prompts: None,
            },
            server_info: ServerInfo { name: "s".into(), version: "1".into() },
        };
        let json = serde_json::to_string(&ir).unwrap();
        let rt: InitializeResult = serde_json::from_str(&json).unwrap();
        assert!(rt.capabilities.tools.is_some());
        assert!(rt.capabilities.resources.is_none());
    }

    #[test]
    fn prompt_argument_optional_fields() {
        let pa = PromptArgument {
            name: "arg1".into(),
            description: None,
            required: None,
        };
        let json = serde_json::to_string(&pa).unwrap();
        let rt: PromptArgument = serde_json::from_str(&json).unwrap();
        assert!(rt.description.is_none());
        assert!(rt.required.is_none());
    }

    #[test]
    fn prompt_message_content_resource() {
        let pmc = PromptMessageContent {
            content_type: "resource".into(),
            text: None,
            data: Some("base64data".into()),
        };
        let json = serde_json::to_string(&pmc).unwrap();
        let rt: PromptMessageContent = serde_json::from_str(&json).unwrap();
        assert_eq!(rt.content_type, "resource");
        assert!(rt.text.is_none());
    }

    #[test]
    fn prompt_result_default() {
        let pr = PromptResult::default();
        assert!(pr.messages.is_empty());
        assert!(pr.description.is_none());
    }

    #[test]
    fn mcp_tool_input_schema_serialized_as_input_schema() {
        let tool = McpTool {
            name: "t".into(),
            description: None,
            input_schema: serde_json::json!({"type":"object"}),
        };
        let json = serde_json::to_string(&tool).unwrap();
        assert!(json.contains("\"inputSchema\""));
    }

    #[test]
    fn server_config_deserialize_with_defaults() {
        let json = r#"{"name":"n","command":"c"}"#;
        let cfg: ServerConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.name, "n");
        assert_eq!(cfg.command, "c");
        assert!(cfg.args.is_empty());
        assert!(cfg.env.is_none());
        assert_eq!(cfg.timeout_secs, 30);
    }

    #[test]
    fn jsonrpc_request_new_has_unique_ids() {
        let r1 = JSONRPCRequest::new("m1", None);
        let r2 = JSONRPCRequest::new("m2", None);
        assert_ne!(r1.id, r2.id);
    }

    #[test]
    fn jsonrpc_response_is_error_false_on_success() {
        let resp = JSONRPCResponse::success(serde_json::Value::Null, serde_json::json!({}));
        assert!(!resp.is_error());
    }

    #[test]
    fn jsonrpc_error_with_data() {
        let err = JSONRPCError {
            code: -32001,
            message: "custom".into(),
            data: Some(serde_json::json!({"detail":"info"})),
        };
        let json = serde_json::to_string(&err).unwrap();
        let rt: JSONRPCError = serde_json::from_str(&json).unwrap();
        assert!(rt.data.is_some());
        assert_eq!(rt.data.unwrap()["detail"], "info");
    }

    #[test]
    fn tool_capabilities_default() {
        let tc = ToolCapabilities { list_changed: None };
        let json = serde_json::to_string(&tc).unwrap();
        assert!(!json.contains("listChanged"));
    }

    #[test]
    fn resource_capabilities_roundtrip() {
        let rc = ResourceCapabilities {
            subscribe: Some(true),
            list_changed: Some(false),
        };
        let json = serde_json::to_string(&rc).unwrap();
        let rt: ResourceCapabilities = serde_json::from_str(&json).unwrap();
        assert_eq!(rt.subscribe, Some(true));
        assert_eq!(rt.list_changed, Some(false));
    }

    #[test]
    fn prompt_capabilities_default() {
        let pc = PromptCapabilities { list_changed: None };
        let json = serde_json::to_string(&pc).unwrap();
        assert!(!json.contains("listChanged"));
    }

    #[test]
    fn client_capabilities_with_all_fields() {
        let cc = ClientCapabilities {
            tools: Some(serde_json::json!({})),
            resources: Some(serde_json::json!({})),
            prompts: Some(serde_json::json!({})),
        };
        let json = serde_json::to_string(&cc).unwrap();
        let rt: ClientCapabilities = serde_json::from_str(&json).unwrap();
        assert!(rt.tools.is_some());
        assert!(rt.resources.is_some());
        assert!(rt.prompts.is_some());
    }
}

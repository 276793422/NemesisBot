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
        Self::new(
            Self::METHOD_NOT_FOUND,
            format!("Method not found: {method}"),
        )
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

/// Configuration for spawning an external MCP server process (or connecting
/// to an HTTP one).
///
/// **2026-08-31 单一真相源收敛**：本 crate 与 nemesis-config 曾各自定义一份
/// MCP 服务器配置结构（`env: Option<Vec<_>>` vs `Vec<_>`、`timeout_secs` vs
/// `timeout`）——同一份 config.mcp.json 出现两种解析结果，Dashboard 侧的
/// `"env": null` 直接解析崩溃而 runtime 一切正常。现在唯一定义在
/// [`nemesis_config::McpServerConfig`]（宽容反序列化 + builder 全套），本
/// 模块仅保留类型别名，存量 API（`ServerConfig::new/arg/env/timeout`）经
/// canonical 类型原样可用。Merge 方向说明：nemesis-mcp → nemesis-config
/// 单向依赖（反向会把 MCP 协议拖进所有 config 消费方）。
pub type ServerConfig = nemesis_config::McpServerConfig;

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests;

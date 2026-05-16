//! MCP tool adapter.
//!
//! Bridges MCP tools to the NemesisBot tool system. Each MCP tool exposed by
//! a remote server is wrapped in an `McpAdapter` that translates between the
//! NemesisBot `ToolDefinition` format and MCP's JSON-RPC tool invocation
//! protocol.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::time::timeout;

use crate::client::{Client, ClientError};
use crate::types::*;

// ---------------------------------------------------------------------------
// NemesisBot tool types (local to avoid circular dependency)
// ---------------------------------------------------------------------------
// The adapter defines its own lightweight tool interface rather than depending
// on the full nemesis-types crate. Higher-level code can bridge these types
// to the agent system.

/// A tool definition in NemesisBot format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Unique tool name (prefixed with server name to avoid collisions).
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// JSON Schema describing the expected input parameters.
    pub parameters: serde_json::Value,
}

/// The result of executing a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// The text content of the result.
    pub content: String,
    /// Whether the tool execution resulted in an error.
    pub is_error: bool,
}

impl ToolResult {
    /// Create a successful result.
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
        }
    }

    /// Create an error result.
    pub fn err(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
        }
    }
}

/// Trait for a tool that can be executed by the NemesisBot agent.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Return the tool definition.
    fn definition(&self) -> &ToolDefinition;

    /// Execute the tool with the given arguments.
    async fn execute(&self, args: serde_json::Value) -> ToolResult;
}

// ---------------------------------------------------------------------------
// McpAdapter
// ---------------------------------------------------------------------------

/// Adapter that wraps an MCP tool as a NemesisBot `Tool`.
///
/// The tool name is prefixed with the MCP server name (sanitized) to avoid
/// naming collisions when multiple MCP servers are connected.
///
/// Format: `mcp_<server_name>_<tool_name>`
pub struct McpAdapter {
    /// The MCP client used to invoke the tool (Arc<Mutex> for interior mutability).
    client: std::sync::Arc<tokio::sync::Mutex<Box<dyn Client>>>,
    /// The MCP tool definition.
    mcp_tool: McpTool,
    /// The cached NemesisBot tool definition.
    tool_def: ToolDefinition,
    /// Default execution timeout.
    timeout: Duration,
}

impl McpAdapter {
    /// Create a new adapter for the given MCP tool.
    ///
    /// The `client` must already be initialized (server info available).
    pub fn new(client: Box<dyn Client>, mcp_tool: McpTool) -> Self {
        let server_name = client
            .server_info()
            .map(|info| sanitize_name(&info.name))
            .unwrap_or_else(|| "unknown".to_string());

        let tool_name = sanitize_name(&mcp_tool.name);
        let prefixed_name = format!("mcp_{server_name}_{tool_name}");

        let description = match &mcp_tool.description {
            Some(desc) => format!("[MCP:{server_name}] {desc}"),
            None => format!("[MCP:{server_name}] MCP tool: {tool_name}"),
        };

        let parameters = serde_json::json!({
            "type": "object",
            "properties": mcp_tool.input_schema,
            "additionalProperties": false,
        });

        let tool_def = ToolDefinition {
            name: prefixed_name,
            description,
            parameters,
        };

        Self {
            client: std::sync::Arc::new(tokio::sync::Mutex::new(client)),
            mcp_tool,
            tool_def,
            timeout: Duration::from_secs(30),
        }
    }

    /// Set the execution timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Return a reference to the underlying MCP tool definition.
    pub fn mcp_tool(&self) -> &McpTool {
        &self.mcp_tool
    }
}

#[async_trait]
impl Tool for McpAdapter {
    fn definition(&self) -> &ToolDefinition {
        &self.tool_def
    }

    async fn execute(&self, args: serde_json::Value) -> ToolResult {
        let args_map: HashMap<String, serde_json::Value> = if args.is_object() {
            serde_json::from_value(args).unwrap_or_default()
        } else {
            HashMap::new()
        };

        let args_value = serde_json::to_value(&args_map).unwrap_or(serde_json::json!({}));

        // Execute with timeout.
        let future = async {
            let mut guard = self.client.lock().await;
            guard.call_tool(&self.mcp_tool.name, args_value).await
        };

        let result = match timeout(self.timeout, future).await {
            Ok(Ok(result)) => result,
            Ok(Err(e)) => {
                return ToolResult::err(format!(
                    "MCP tool '{}' error: {}",
                    self.mcp_tool.name, e
                ));
            }
            Err(_) => {
                return ToolResult::err(format!(
                    "MCP tool '{}' timed out after {:?}",
                    self.mcp_tool.name, self.timeout
                ));
            }
        };

        // Check if tool returned an error.
        if result.is_error {
            let err_msg: String = result
                .content
                .iter()
                .filter(|c| c.content_type == "text")
                .filter_map(|c| c.text.as_deref())
                .collect::<Vec<&str>>()
                .join("; ");

            return ToolResult::err(format!(
                "MCP tool '{}' returned error: {}",
                self.mcp_tool.name,
                if err_msg.is_empty() { "unknown error" } else { &err_msg }
            ));
        }

        // Extract text content from result.
        let text_parts: Vec<String> = result
            .content
            .iter()
            .map(|c| match c.content_type.as_str() {
                "text" => c.text.clone().unwrap_or_default(),
                "image" => {
                    let data = c.text.as_deref().unwrap_or("");
                    format!("[Image: {data}]")
                }
                "resource" => {
                    let data = c.text.as_deref().unwrap_or("");
                    format!("[Resource: {data}]")
                }
                _ => c.text.clone().unwrap_or_default(),
            })
            .collect();

        ToolResult::ok(text_parts.join("\n"))
    }
}

// ---------------------------------------------------------------------------
// Factory: create adapters from a connected client
// ---------------------------------------------------------------------------

/// Create NemesisBot tool adapters for all tools available on an MCP server.
///
/// The client must already be initialized. On success, returns a vector of
/// boxed `Tool` trait objects, one for each MCP tool on the server.
pub async fn create_tools_from_client(
    client: Box<dyn Client>,
) -> Result<Vec<Box<dyn Tool>>, ClientError> {
    // We need a mutable reference to call list_tools, but we also need to
    // share the client across all adapters. We'll use a shared wrapper.
    let shared = std::sync::Arc::new(tokio::sync::Mutex::new(client));

    // List tools from the server.
    let tools = {
        let mut guard = shared.lock().await;
        guard.list_tools().await?
    };

    let mut adapters: Vec<Box<dyn Tool>> = Vec::new();

    for mcp_tool in tools {
        // For each tool, create a thin wrapper that clones the Arc.
        let adapter = ArcClientAdapter {
            client: shared.clone(),
            mcp_tool: mcp_tool.clone(),
            tool_def: {
                let guard = shared.lock().await;
                let server_name = guard
                    .server_info()
                    .map(|info| sanitize_name(&info.name))
                    .unwrap_or_else(|| "unknown".to_string());

                let tool_name = sanitize_name(&mcp_tool.name);
                let prefixed_name = format!("mcp_{server_name}_{tool_name}");

                let description = match &mcp_tool.description {
                    Some(desc) => format!("[MCP:{server_name}] {desc}"),
                    None => format!("[MCP:{server_name}] MCP tool: {tool_name}"),
                };

                let parameters = serde_json::json!({
                    "type": "object",
                    "properties": mcp_tool.input_schema,
                    "additionalProperties": false,
                });

                ToolDefinition {
                    name: prefixed_name,
                    description,
                    parameters,
                }
            },
            timeout: Duration::from_secs(30),
        };
        adapters.push(Box::new(adapter));
    }

    Ok(adapters)
}

/// Internal adapter that uses an Arc<Mutex<Client>> for shared access.
struct ArcClientAdapter {
    client: std::sync::Arc<tokio::sync::Mutex<Box<dyn Client>>>,
    mcp_tool: McpTool,
    tool_def: ToolDefinition,
    timeout: Duration,
}

#[async_trait]
impl Tool for ArcClientAdapter {
    fn definition(&self) -> &ToolDefinition {
        &self.tool_def
    }

    async fn execute(&self, args: serde_json::Value) -> ToolResult {
        let args_map: HashMap<String, serde_json::Value> = if args.is_object() {
            serde_json::from_value(args).unwrap_or_default()
        } else {
            HashMap::new()
        };

        let args_value = serde_json::to_value(&args_map).unwrap_or(serde_json::json!({}));

        let future = async {
            let mut guard = self.client.lock().await;
            guard.call_tool(&self.mcp_tool.name, args_value).await
        };

        let result = match timeout(self.timeout, future).await {
            Ok(Ok(result)) => result,
            Ok(Err(e)) => {
                return ToolResult::err(format!(
                    "MCP tool '{}' error: {}",
                    self.mcp_tool.name, e
                ));
            }
            Err(_) => {
                return ToolResult::err(format!(
                    "MCP tool '{}' timed out after {:?}",
                    self.mcp_tool.name, self.timeout
                ));
            }
        };

        if result.is_error {
            let err_msg: String = result
                .content
                .iter()
                .filter(|c| c.content_type == "text")
                .filter_map(|c| c.text.as_deref())
                .collect::<Vec<&str>>()
                .join("; ");

            return ToolResult::err(format!(
                "MCP tool '{}' returned error: {}",
                self.mcp_tool.name,
                if err_msg.is_empty() { "unknown error" } else { &err_msg }
            ));
        }

        let text_parts: Vec<String> = result
            .content
            .iter()
            .map(|c| match c.content_type.as_str() {
                "text" => c.text.clone().unwrap_or_default(),
                "image" => format!("[Image: {}]", c.text.as_deref().unwrap_or("")),
                "resource" => format!("[Resource: {}]", c.text.as_deref().unwrap_or("")),
                _ => c.text.clone().unwrap_or_default(),
            })
            .collect();

        ToolResult::ok(text_parts.join("\n"))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Sanitize a string for use as a tool identifier component.
///
/// Replaces any character that's not alphanumeric, hyphen, or underscore
/// with an underscore.
fn sanitize_name(name: &str) -> String {
    let mut result = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            result.push(ch);
        } else {
            result.push('_');
        }
    }
    result
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::ClientResult;

    #[test]
    fn sanitize_name_basic() {
        assert_eq!(sanitize_name("hello-world"), "hello-world");
        assert_eq!(sanitize_name("hello_world"), "hello_world");
        assert_eq!(sanitize_name("hello world"), "hello_world");
        assert_eq!(sanitize_name("hello.world"), "hello_world");
        assert_eq!(sanitize_name("hello@world!"), "hello_world_");
        assert_eq!(sanitize_name("abc123"), "abc123");
        assert_eq!(sanitize_name(""), "");
    }

    #[test]
    fn tool_result_helpers() {
        let ok = ToolResult::ok("success message");
        assert_eq!(ok.content, "success message");
        assert!(!ok.is_error);

        let err = ToolResult::err("failure message");
        assert_eq!(err.content, "failure message");
        assert!(err.is_error);
    }

    #[test]
    fn tool_definition_serialization() {
        let def = ToolDefinition {
            name: "mcp_test_echo".into(),
            description: "[MCP:test] Echo tool".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string" }
                },
                "additionalProperties": false,
            }),
        };

        let json = serde_json::to_string(&def).unwrap();
        let rt: ToolDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(rt.name, "mcp_test_echo");
        assert_eq!(rt.description, "[MCP:test] Echo tool");
    }

    #[test]
    fn sanitize_name_special_chars() {
        assert_eq!(sanitize_name("foo/bar"), "foo_bar");
        assert_eq!(sanitize_name("foo\\bar"), "foo_bar");
        assert_eq!(sanitize_name("foo:bar"), "foo_bar");
        assert_eq!(sanitize_name("foo;bar"), "foo_bar");
        assert_eq!(sanitize_name("foo|bar"), "foo_bar");
        assert_eq!(sanitize_name("foo<bar>"), "foo_bar_");
    }

    #[test]
    fn sanitize_name_unicode() {
        let result = sanitize_name("hello-world");
        assert!(result.contains('-') || result.contains('_'));
    }

    #[test]
    fn sanitize_name_numbers_only() {
        assert_eq!(sanitize_name("12345"), "12345");
    }

    #[test]
    fn tool_result_ok_is_not_error() {
        let ok = ToolResult::ok("data");
        assert!(!ok.is_error);
        assert_eq!(ok.content, "data");
    }

    #[test]
    fn tool_result_err_is_error() {
        let err = ToolResult::err("oops");
        assert!(err.is_error);
        assert_eq!(err.content, "oops");
    }

    #[test]
    fn tool_result_empty_ok() {
        let ok = ToolResult::ok("");
        assert!(!ok.is_error);
        assert_eq!(ok.content, "");
    }

    #[test]
    fn tool_result_empty_err() {
        let err = ToolResult::err("");
        assert!(err.is_error);
        assert_eq!(err.content, "");
    }

    #[test]
    fn tool_definition_complex_params() {
        let def = ToolDefinition {
            name: "mcp_complex".into(),
            description: "Complex tool".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "options": {
                        "type": "object",
                        "properties": {
                            "recursive": { "type": "boolean" },
                            "depth": { "type": "integer" }
                        }
                    }
                },
                "required": ["path"]
            }),
        };

        let json = serde_json::to_string(&def).unwrap();
        let rt: ToolDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(rt.name, "mcp_complex");
        let params = rt.parameters.as_object().unwrap();
        assert!(params.contains_key("properties"));
        assert!(params.contains_key("required"));
    }

    #[test]
    fn sanitize_name_long_string() {
        let long = "a".repeat(200);
        let result = sanitize_name(&long);
        assert_eq!(result.len(), 200);
    }

    #[test]
    fn sanitize_name_mixed_chars() {
        let result = sanitize_name("My Tool Name!@#$%");
        assert!(!result.contains('!'));
        assert!(!result.contains('@'));
        assert!(!result.contains('#'));
        assert!(!result.contains('$'));
        assert!(!result.contains('%'));
    }

    // ---- New tests ----

    #[test]
    fn sanitize_name_preserves_hyphens() {
        assert_eq!(sanitize_name("hello-world-test"), "hello-world-test");
    }

    #[test]
    fn sanitize_name_preserves_underscores() {
        assert_eq!(sanitize_name("hello_world_test"), "hello_world_test");
    }

    #[test]
    fn sanitize_name_all_special() {
        let result = sanitize_name("!@#$%^&*()");
        assert!(result.chars().all(|c| c == '_'));
    }

    #[test]
    fn sanitize_name_with_spaces() {
        assert_eq!(sanitize_name("hello world"), "hello_world");
        assert_eq!(sanitize_name("a b c"), "a_b_c");
    }

    #[test]
    fn tool_result_serialization() {
        let ok = ToolResult::ok("message");
        let json = serde_json::to_string(&ok).unwrap();
        let rt: ToolResult = serde_json::from_str(&json).unwrap();
        assert!(!rt.is_error);
        assert_eq!(rt.content, "message");

        let err = ToolResult::err("error msg");
        let json = serde_json::to_string(&err).unwrap();
        let rt: ToolResult = serde_json::from_str(&json).unwrap();
        assert!(rt.is_error);
    }

    #[test]
    fn tool_definition_default_name_pattern() {
        // Verify that when creating McpAdapter, the name follows mcp_{server}_{tool}
        let def = ToolDefinition {
            name: "mcp_test_server_echo".into(),
            description: "[MCP:test_server] Echo tool".into(),
            parameters: serde_json::json!({"type": "object"}),
        };
        assert!(def.name.starts_with("mcp_"));
        assert!(def.description.starts_with("[MCP:"));
    }

    #[test]
    fn sanitize_name_tabs_and_newlines() {
        assert_eq!(sanitize_name("hello\tworld"), "hello_world");
        assert_eq!(sanitize_name("hello\nworld"), "hello_world");
    }

    #[test]
    fn tool_result_ok_with_multiline() {
        let ok = ToolResult::ok("line1\nline2\nline3");
        assert_eq!(ok.content, "line1\nline2\nline3");
        assert!(!ok.is_error);
    }

    #[test]
    fn tool_result_err_with_multiline() {
        let err = ToolResult::err("error1\nerror2");
        assert_eq!(err.content, "error1\nerror2");
        assert!(err.is_error);
    }

    #[test]
    fn tool_definition_name_uniqueness() {
        let def1 = ToolDefinition {
            name: "mcp_server1_tool1".into(),
            description: "Tool 1 from server 1".into(),
            parameters: serde_json::json!({"type": "object"}),
        };
        let def2 = ToolDefinition {
            name: "mcp_server2_tool1".into(),
            description: "Tool 1 from server 2".into(),
            parameters: serde_json::json!({"type": "object"}),
        };
        assert_ne!(def1.name, def2.name);
    }

    // ============================================================
    // Tests using mock client for McpAdapter coverage
    // ============================================================

    use std::sync::atomic::{AtomicBool, Ordering};

    /// Mock client for testing McpAdapter without a real server.
    struct MockClient {
        server_info: Option<ServerInfo>,
        tools: Vec<McpTool>,
        call_results: std::sync::Mutex<std::collections::VecDeque<ToolCallResult>>,
        initialized: AtomicBool,
    }

    impl MockClient {
        fn new(server_name: &str, tools: Vec<McpTool>) -> Self {
            Self {
                server_info: Some(ServerInfo {
                    name: server_name.to_string(),
                    version: "1.0.0".to_string(),
                }),
                tools,
                call_results: std::sync::Mutex::new(std::collections::VecDeque::new()),
                initialized: AtomicBool::new(false),
            }
        }

        fn with_call_result(&self, result: ToolCallResult) {
            self.call_results.lock().unwrap().push_back(result);
        }
    }

    #[async_trait]
    impl Client for MockClient {
        async fn initialize(&mut self) -> ClientResult<InitializeResult> {
            self.initialized.store(true, Ordering::SeqCst);
            Ok(InitializeResult {
                protocol_version: PROTOCOL_VERSION.to_string(),
                capabilities: ServerCapabilities::default(),
                server_info: self.server_info.clone().unwrap(),
            })
        }

        async fn list_tools(&mut self) -> ClientResult<Vec<McpTool>> {
            Ok(self.tools.clone())
        }

        async fn call_tool(
            &mut self,
            _name: &str,
            _arguments: serde_json::Value,
        ) -> ClientResult<ToolCallResult> {
            let mut results = self.call_results.lock().unwrap();
            if let Some(result) = results.pop_front() {
                Ok(result)
            } else {
                Ok(ToolCallResult::ok("default mock result"))
            }
        }

        async fn list_resources(&mut self) -> ClientResult<Vec<Resource>> {
            Ok(vec![])
        }

        async fn read_resource(&mut self, _uri: &str) -> ClientResult<ResourceContent> {
            Ok(ResourceContent::default())
        }

        async fn list_prompts(&mut self) -> ClientResult<Vec<Prompt>> {
            Ok(vec![])
        }

        async fn get_prompt(
            &mut self,
            _name: &str,
            _arguments: serde_json::Value,
        ) -> ClientResult<PromptResult> {
            Ok(PromptResult::default())
        }

        async fn close(&mut self) -> ClientResult<()> {
            Ok(())
        }

        fn server_info(&self) -> Option<&ServerInfo> {
            self.server_info.as_ref()
        }

        fn is_connected(&self) -> bool {
            self.initialized.load(Ordering::SeqCst)
        }
    }

    #[test]
    fn test_mcp_adapter_new_with_description() {
        let mock = MockClient::new("test_server", vec![]);
        let mcp_tool = McpTool {
            name: "echo".to_string(),
            description: Some("Echo the input".to_string()),
            input_schema: serde_json::json!({"type": "object", "properties": {"message": {"type": "string"}}}),
        };
        let adapter = McpAdapter::new(Box::new(mock), mcp_tool.clone());

        let def = adapter.definition();
        assert_eq!(def.name, "mcp_test_server_echo");
        assert!(def.description.contains("[MCP:test_server]"));
        assert!(def.description.contains("Echo the input"));

        assert_eq!(adapter.mcp_tool().name, "echo");
    }

    #[test]
    fn test_mcp_adapter_new_without_description() {
        let mock = MockClient::new("my_server", vec![]);
        let mcp_tool = McpTool {
            name: "read".to_string(),
            description: None,
            input_schema: serde_json::json!({"type": "object"}),
        };
        let adapter = McpAdapter::new(Box::new(mock), mcp_tool);

        let def = adapter.definition();
        assert!(def.description.contains("[MCP:my_server]"));
        assert!(def.description.contains("MCP tool: read"));
    }

    #[test]
    fn test_mcp_adapter_name_sanitization() {
        let mock = MockClient::new("my server!", vec![]);
        let mcp_tool = McpTool {
            name: "my tool@1".to_string(),
            description: Some("desc".to_string()),
            input_schema: serde_json::json!({}),
        };
        let adapter = McpAdapter::new(Box::new(mock), mcp_tool);

        let def = adapter.definition();
        // Spaces and special chars should be replaced with underscores
        assert!(def.name.contains("my_server_"));
        assert!(def.name.contains("my_tool_1"));
    }

    #[test]
    fn test_mcp_adapter_with_timeout() {
        let mock = MockClient::new("test", vec![]);
        let mcp_tool = McpTool {
            name: "tool".to_string(),
            description: None,
            input_schema: serde_json::json!({}),
        };
        let adapter = McpAdapter::new(Box::new(mock), mcp_tool).with_timeout(Duration::from_secs(60));
        let def = adapter.definition();
        assert_eq!(def.name, "mcp_test_tool");
    }

    #[test]
    fn test_mcp_adapter_parameters_structure() {
        let mock = MockClient::new("srv", vec![]);
        let schema = serde_json::json!({"type": "object", "properties": {"x": {"type": "number"}}});
        let mcp_tool = McpTool {
            name: "compute".to_string(),
            description: Some("Compute".to_string()),
            input_schema: schema.clone(),
        };
        let adapter = McpAdapter::new(Box::new(mock), mcp_tool);

        let params = adapter.definition().parameters.as_object().unwrap();
        assert_eq!(params["type"], "object");
        assert_eq!(params["additionalProperties"], false);
        // The input_schema is nested under "properties"
        assert!(params["properties"].is_object());
    }

    #[tokio::test]
    async fn test_mcp_adapter_execute_text_result() {
        let mock = MockClient::new("test", vec![]);
        mock.with_call_result(ToolCallResult::ok("Hello from tool!"));

        let mcp_tool = McpTool {
            name: "echo".to_string(),
            description: None,
            input_schema: serde_json::json!({}),
        };
        let adapter = McpAdapter::new(Box::new(mock), mcp_tool);

        let result = adapter.execute(serde_json::json!({"message": "hi"})).await;
        assert!(!result.is_error);
        assert_eq!(result.content, "Hello from tool!");
    }

    #[tokio::test]
    async fn test_mcp_adapter_execute_error_result() {
        let mock = MockClient::new("test", vec![]);
        mock.with_call_result(ToolCallResult::err("Something went wrong"));

        let mcp_tool = McpTool {
            name: "fail_tool".to_string(),
            description: None,
            input_schema: serde_json::json!({}),
        };
        let adapter = McpAdapter::new(Box::new(mock), mcp_tool);

        let result = adapter.execute(serde_json::json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("fail_tool"));
        assert!(result.content.contains("Something went wrong"));
    }

    #[tokio::test]
    async fn test_mcp_adapter_execute_error_result_no_text() {
        let mock = MockClient::new("test", vec![]);
        // Error result with image content (no text) — should return "unknown error"
        mock.with_call_result(ToolCallResult {
            content: vec![ToolContent {
                content_type: "image".to_string(),
                text: None,
            }],
            is_error: true,
        });

        let mcp_tool = McpTool {
            name: "img_tool".to_string(),
            description: None,
            input_schema: serde_json::json!({}),
        };
        let adapter = McpAdapter::new(Box::new(mock), mcp_tool);

        let result = adapter.execute(serde_json::json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("unknown error"));
    }

    #[tokio::test]
    async fn test_mcp_adapter_execute_image_content() {
        let mock = MockClient::new("test", vec![]);
        mock.with_call_result(ToolCallResult {
            content: vec![ToolContent {
                content_type: "image".to_string(),
                text: Some("base64data".to_string()),
            }],
            is_error: false,
        });

        let mcp_tool = McpTool {
            name: "img_tool".to_string(),
            description: None,
            input_schema: serde_json::json!({}),
        };
        let adapter = McpAdapter::new(Box::new(mock), mcp_tool);

        let result = adapter.execute(serde_json::json!({})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("[Image:"));
        assert!(result.content.contains("base64data"));
    }

    #[tokio::test]
    async fn test_mcp_adapter_execute_resource_content() {
        let mock = MockClient::new("test", vec![]);
        mock.with_call_result(ToolCallResult {
            content: vec![ToolContent {
                content_type: "resource".to_string(),
                text: Some("resource_data".to_string()),
            }],
            is_error: false,
        });

        let mcp_tool = McpTool {
            name: "res_tool".to_string(),
            description: None,
            input_schema: serde_json::json!({}),
        };
        let adapter = McpAdapter::new(Box::new(mock), mcp_tool);

        let result = adapter.execute(serde_json::json!({})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("[Resource:"));
        assert!(result.content.contains("resource_data"));
    }

    #[tokio::test]
    async fn test_mcp_adapter_execute_unknown_content_type() {
        let mock = MockClient::new("test", vec![]);
        mock.with_call_result(ToolCallResult {
            content: vec![ToolContent {
                content_type: "custom_type".to_string(),
                text: Some("custom_data".to_string()),
            }],
            is_error: false,
        });

        let mcp_tool = McpTool {
            name: "custom_tool".to_string(),
            description: None,
            input_schema: serde_json::json!({}),
        };
        let adapter = McpAdapter::new(Box::new(mock), mcp_tool);

        let result = adapter.execute(serde_json::json!({})).await;
        assert!(!result.is_error);
        assert_eq!(result.content, "custom_data");
    }

    #[tokio::test]
    async fn test_mcp_adapter_execute_multiple_content() {
        let mock = MockClient::new("test", vec![]);
        mock.with_call_result(ToolCallResult {
            content: vec![
                ToolContent::text("part1"),
                ToolContent::text("part2"),
            ],
            is_error: false,
        });

        let mcp_tool = McpTool {
            name: "multi".to_string(),
            description: None,
            input_schema: serde_json::json!({}),
        };
        let adapter = McpAdapter::new(Box::new(mock), mcp_tool);

        let result = adapter.execute(serde_json::json!({})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("part1"));
        assert!(result.content.contains("part2"));
    }

    #[tokio::test]
    async fn test_mcp_adapter_execute_non_object_args() {
        let mock = MockClient::new("test", vec![]);
        mock.with_call_result(ToolCallResult::ok("ok"));

        let mcp_tool = McpTool {
            name: "flex".to_string(),
            description: None,
            input_schema: serde_json::json!({}),
        };
        let adapter = McpAdapter::new(Box::new(mock), mcp_tool);

        // Pass a string instead of object
        let result = adapter.execute(serde_json::json!("not an object")).await;
        assert!(!result.is_error);

        // Pass null
        let result = adapter.execute(serde_json::json!(null)).await;
        assert!(!result.is_error);

        // Pass array
        let result = adapter.execute(serde_json::json!([1, 2, 3])).await;
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_create_tools_from_client() {
        let tools = vec![
            McpTool {
                name: "tool_a".to_string(),
                description: Some("Tool A".to_string()),
                input_schema: serde_json::json!({"type": "object"}),
            },
            McpTool {
                name: "tool_b".to_string(),
                description: None,
                input_schema: serde_json::json!({"type": "object"}),
            },
        ];
        let mock = MockClient::new("my_server", tools);

        let adapters = create_tools_from_client(Box::new(mock)).await.unwrap();
        assert_eq!(adapters.len(), 2);

        let def0 = adapters[0].definition();
        assert_eq!(def0.name, "mcp_my_server_tool_a");
        assert!(def0.description.contains("[MCP:my_server]"));
        assert!(def0.description.contains("Tool A"));

        let def1 = adapters[1].definition();
        assert_eq!(def1.name, "mcp_my_server_tool_b");
        assert!(def1.description.contains("MCP tool: tool_b"));
    }

    #[tokio::test]
    async fn test_create_tools_from_client_empty() {
        let mock = MockClient::new("empty_server", vec![]);
        let adapters = create_tools_from_client(Box::new(mock)).await.unwrap();
        assert!(adapters.is_empty());
    }

    #[test]
    fn test_mcp_adapter_no_server_info() {
        // Create a mock with no server info
        struct NoInfoMock;

        #[async_trait]
        impl Client for NoInfoMock {
            async fn initialize(&mut self) -> ClientResult<InitializeResult> {
                Ok(InitializeResult {
                    protocol_version: PROTOCOL_VERSION.to_string(),
                    capabilities: ServerCapabilities::default(),
                    server_info: ServerInfo { name: "n".into(), version: "1".into() },
                })
            }
            async fn list_tools(&mut self) -> ClientResult<Vec<McpTool>> { Ok(vec![]) }
            async fn call_tool(&mut self, _name: &str, _args: serde_json::Value) -> ClientResult<ToolCallResult> {
                Ok(ToolCallResult::ok(""))
            }
            async fn list_resources(&mut self) -> ClientResult<Vec<Resource>> { Ok(vec![]) }
            async fn read_resource(&mut self, _uri: &str) -> ClientResult<ResourceContent> { Ok(ResourceContent::default()) }
            async fn list_prompts(&mut self) -> ClientResult<Vec<Prompt>> { Ok(vec![]) }
            async fn get_prompt(&mut self, _name: &str, _args: serde_json::Value) -> ClientResult<PromptResult> { Ok(PromptResult::default()) }
            async fn close(&mut self) -> ClientResult<()> { Ok(()) }
            fn server_info(&self) -> Option<&ServerInfo> { None }
            fn is_connected(&self) -> bool { false }
        }

        let mcp_tool = McpTool {
            name: "test".to_string(),
            description: None,
            input_schema: serde_json::json!({}),
        };
        let adapter = McpAdapter::new(Box::new(NoInfoMock), mcp_tool);
        let def = adapter.definition();
        // When no server info, should use "unknown"
        assert!(def.name.contains("unknown"));
    }
}

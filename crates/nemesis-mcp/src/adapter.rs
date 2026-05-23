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
mod tests;

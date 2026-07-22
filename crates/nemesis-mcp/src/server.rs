//! MCP server (local).
//!
//! A simple in-process MCP server that exposes locally registered tools via
//! the JSON-RPC based MCP protocol. Designed for testing and for serving as
//! the foundation of a tool-host that other MCP clients connect to.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::RwLock;

use crate::types::*;

/// A handler function for a registered tool.
pub type ToolHandler = Arc<dyn Fn(serde_json::Value) -> ToolCallResult + Send + Sync>;

/// Error type for MCP server operations.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("Tool not found: {0}")]
    ToolNotFound(String),

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Internal error: {0}")]
    Internal(String),
}

/// Result alias for server operations.
pub type ServerResult<T> = Result<T, ServerError>;

/// A local MCP server that can register tools and handle JSON-RPC requests.
pub struct McpServer {
    /// Server identity.
    info: ServerInfo,
    /// Advertised capabilities.
    capabilities: ServerCapabilities,
    /// Registered tool definitions.
    tools: HashMap<String, McpTool>,
    /// Registered tool handlers.
    handlers: HashMap<String, ToolHandler>,
    /// Registered resource definitions.
    resources: HashMap<String, Resource>,
    /// Resource content providers.
    resource_content: HashMap<String, ResourceContent>,
}

impl McpServer {
    /// Create a new MCP server with the given name and version.
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            info: ServerInfo {
                name: name.into(),
                version: version.into(),
            },
            capabilities: ServerCapabilities {
                tools: Some(ToolCapabilities {
                    list_changed: Some(false),
                }),
                resources: Some(ResourceCapabilities {
                    subscribe: Some(false),
                    list_changed: Some(false),
                }),
                prompts: None,
            },
            tools: HashMap::new(),
            handlers: HashMap::new(),
            resources: HashMap::new(),
            resource_content: HashMap::new(),
        }
    }

    /// Register a tool with a handler function.
    pub fn register_tool(&mut self, tool: McpTool, handler: ToolHandler) -> ServerResult<()> {
        let name = tool.name.clone();
        if self.tools.contains_key(&name) {
            return Err(ServerError::InvalidRequest(format!(
                "Tool already registered: {name}"
            )));
        }
        self.tools.insert(name.clone(), tool);
        self.handlers.insert(name, handler);
        Ok(())
    }

    /// Register a static resource with pre-defined content.
    pub fn register_resource(&mut self, resource: Resource, content: ResourceContent) {
        let uri = resource.uri.clone();
        self.resources.insert(uri.clone(), resource);
        self.resource_content.insert(uri, content);
    }

    /// Handle a raw JSON-RPC request string and return the response string.
    ///
    /// This is the main entry point for feeding requests into the server.
    pub async fn handle_raw(&self, raw: &str) -> String {
        let request: JSONRPCRequest = match serde_json::from_str(raw) {
            Ok(r) => r,
            Err(e) => {
                let err = JSONRPCError::new(JSONRPCError::PARSE_ERROR, e.to_string());
                let resp = JSONRPCResponse::error(Value::Null, err);
                return serde_json::to_string(&resp).unwrap_or_default();
            }
        };

        let response = self.handle_request(&request).await;
        serde_json::to_string(&response).unwrap_or_default()
    }

    /// Handle a parsed JSON-RPC request and produce a response.
    pub async fn handle_request(&self, request: &JSONRPCRequest) -> JSONRPCResponse {
        let id = request.id.clone().unwrap_or(Value::Null);

        match request.method.as_str() {
            "initialize" => self.handle_initialize(id),
            "tools/list" => self.handle_tools_list(id),
            "tools/call" => self.handle_tools_call(id, &request.params).await,
            "resources/list" => self.handle_resources_list(id),
            "resources/read" => self.handle_resources_read(id, &request.params),
            "ping" => JSONRPCResponse::success(id, serde_json::json!({})),
            _ => JSONRPCResponse::error(id, JSONRPCError::method_not_found(&request.method)),
        }
    }

    // -- Handler methods -----------------------------------------------------

    fn handle_initialize(&self, id: Value) -> JSONRPCResponse {
        let result = serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": self.capabilities,
            "serverInfo": self.info,
        });
        JSONRPCResponse::success(id, result)
    }

    fn handle_tools_list(&self, id: Value) -> JSONRPCResponse {
        let tools: Vec<&McpTool> = self.tools.values().collect();
        JSONRPCResponse::success(id, serde_json::json!({ "tools": tools }))
    }

    async fn handle_tools_call(&self, id: Value, params: &Option<Value>) -> JSONRPCResponse {
        let params = match params {
            Some(p) => p,
            None => {
                return JSONRPCResponse::error(
                    id,
                    JSONRPCError::invalid_params("Missing parameters for tools/call"),
                );
            }
        };

        let tool_name = match params.get("name").and_then(|n| n.as_str()) {
            Some(n) => n.to_string(),
            None => {
                return JSONRPCResponse::error(
                    id,
                    JSONRPCError::invalid_params("Missing tool name"),
                );
            }
        };

        let handler = match self.handlers.get(&tool_name) {
            Some(h) => h,
            None => {
                return JSONRPCResponse::error(
                    id,
                    JSONRPCError::new(
                        JSONRPCError::INTERNAL_ERROR,
                        format!("Tool not found: {tool_name}"),
                    ),
                );
            }
        };

        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or(serde_json::json!({}));
        let result = handler(arguments);

        match serde_json::to_value(&result) {
            Ok(val) => JSONRPCResponse::success(id, val),
            Err(e) => JSONRPCResponse::error(
                id,
                JSONRPCError::internal(format!("Failed to serialize result: {e}")),
            ),
        }
    }

    fn handle_resources_list(&self, id: Value) -> JSONRPCResponse {
        let resources: Vec<&Resource> = self.resources.values().collect();
        JSONRPCResponse::success(id, serde_json::json!({ "resources": resources }))
    }

    fn handle_resources_read(&self, id: Value, params: &Option<Value>) -> JSONRPCResponse {
        let uri = params
            .as_ref()
            .and_then(|p| p.get("uri"))
            .and_then(|u| u.as_str());

        match uri {
            Some(uri) => match self.resource_content.get(uri) {
                Some(content) => {
                    JSONRPCResponse::success(id, serde_json::json!({ "contents": [content] }))
                }
                None => JSONRPCResponse::error(
                    id,
                    JSONRPCError::new(
                        JSONRPCError::INTERNAL_ERROR,
                        format!("Resource not found: {uri}"),
                    ),
                ),
            },
            None => {
                JSONRPCResponse::error(id, JSONRPCError::invalid_params("Missing uri parameter"))
            }
        }
    }

    /// Return the server info.
    pub fn info(&self) -> &ServerInfo {
        &self.info
    }

    /// Return the server capabilities.
    pub fn capabilities(&self) -> &ServerCapabilities {
        &self.capabilities
    }
}

/// Thread-safe wrapper around `McpServer` for use from multiple tasks.
pub struct SharedMcpServer {
    inner: Arc<RwLock<McpServer>>,
}

impl SharedMcpServer {
    /// Create a new shared server.
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(McpServer::new(name, version))),
        }
    }

    /// Register a tool.
    pub async fn register_tool(&self, tool: McpTool, handler: ToolHandler) -> ServerResult<()> {
        self.inner.write().await.register_tool(tool, handler)
    }

    /// Handle a raw request.
    pub async fn handle_raw(&self, raw: &str) -> String {
        self.inner.read().await.handle_raw(raw).await
    }

    /// Handle a parsed request.
    pub async fn handle_request(&self, request: &JSONRPCRequest) -> JSONRPCResponse {
        self.inner.read().await.handle_request(request).await
    }
}

impl Clone for SharedMcpServer {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests;

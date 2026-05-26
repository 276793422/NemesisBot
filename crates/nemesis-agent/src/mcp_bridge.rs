//! Bridge between `nemesis_mcp::adapter::Tool` and the agent's `Tool` trait.

/// Wrapper that adapts an `adapter::Tool` into the agent's `Tool` trait.
pub(crate) struct McpToolBridge {
    inner: Box<dyn nemesis_mcp::adapter::Tool>,
}

impl McpToolBridge {
    pub fn new(tool: Box<dyn nemesis_mcp::adapter::Tool>) -> Self {
        Self { inner: tool }
    }
}

#[async_trait::async_trait]
impl crate::r#loop::Tool for McpToolBridge {
    async fn execute(
        &self,
        args: &str,
        _context: &crate::context::RequestContext,
    ) -> Result<String, String> {
        let args_value = serde_json::from_str(args).unwrap_or(serde_json::json!({}));
        let result = self.inner.execute(args_value).await;
        if result.is_error {
            let msg = if result.content.is_empty() {
                "MCP tool returned an error".to_string()
            } else {
                result.content
            };
            Err(msg)
        } else {
            Ok(result.content)
        }
    }

    fn description(&self) -> String {
        self.inner.definition().description.clone()
    }

    fn parameters(&self) -> serde_json::Value {
        self.inner.definition().parameters.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::r#loop::Tool;
    use nemesis_mcp::adapter::{ToolDefinition, ToolResult};

    struct MockAdapterTool {
        definition: ToolDefinition,
        result: ToolResult,
    }

    #[async_trait::async_trait]
    impl nemesis_mcp::adapter::Tool for MockAdapterTool {
        fn definition(&self) -> &ToolDefinition {
            &self.definition
        }
        async fn execute(&self, _args: serde_json::Value) -> ToolResult {
            self.result.clone()
        }
    }

    fn make_context() -> crate::context::RequestContext {
        crate::context::RequestContext::new("test", "chat1", "session1", "corr1")
    }

    #[tokio::test]
    async fn test_bridge_success() {
        let mock = MockAdapterTool {
            definition: ToolDefinition {
                name: "test_tool".to_string(),
                description: "A test tool".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
            result: ToolResult::ok("hello world"),
        };
        let bridge = McpToolBridge::new(Box::new(mock));
        assert_eq!(bridge.description(), "A test tool");
        let result = bridge.execute("{}", &make_context()).await;
        assert_eq!(result.unwrap(), "hello world");
    }

    #[tokio::test]
    async fn test_bridge_error() {
        let mock = MockAdapterTool {
            definition: ToolDefinition {
                name: "fail_tool".to_string(),
                description: "A failing tool".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
            result: ToolResult::err("something went wrong"),
        };
        let bridge = McpToolBridge::new(Box::new(mock));
        let result = bridge.execute("{}", &make_context()).await;
        assert_eq!(result.unwrap_err(), "something went wrong");
    }

    #[tokio::test]
    async fn test_bridge_invalid_json_args() {
        let mock = MockAdapterTool {
            definition: ToolDefinition {
                name: "t".to_string(),
                description: "desc".to_string(),
                parameters: serde_json::json!({}),
            },
            result: ToolResult::ok("ok"),
        };
        let bridge = McpToolBridge::new(Box::new(mock));
        // Invalid JSON should be replaced with {} by the bridge
        let result = bridge.execute("not json{{{", &make_context()).await;
        assert_eq!(result.unwrap(), "ok");
    }

    #[tokio::test]
    async fn test_bridge_parameters_forwarded() {
        let params = serde_json::json!({"type": "object", "properties": {"q": {"type": "string"}}});
        let mock = MockAdapterTool {
            definition: ToolDefinition {
                name: "search".to_string(),
                description: "Search".to_string(),
                parameters: params.clone(),
            },
            result: ToolResult::ok("found"),
        };
        let bridge = McpToolBridge::new(Box::new(mock));
        assert_eq!(bridge.parameters(), params);
    }
}

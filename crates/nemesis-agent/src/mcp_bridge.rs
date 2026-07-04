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
mod tests;

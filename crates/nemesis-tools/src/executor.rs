//! Tool executor with security integration.

use crate::registry::ToolRegistry;
use crate::types::ToolResult;
use std::sync::Arc;

/// Tool executor configuration.
pub struct ExecutorConfig {
    pub max_concurrent: usize,
    pub timeout_secs: u64,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 10,
            timeout_secs: 300,
        }
    }
}

/// Tool executor.
pub struct ToolExecutor {
    registry: Arc<ToolRegistry>,
    config: ExecutorConfig,
}

impl ToolExecutor {
    pub fn new(registry: Arc<ToolRegistry>, config: ExecutorConfig) -> Self {
        Self { registry, config }
    }

    /// Execute a tool by name with the given arguments.
    pub async fn execute(&self, tool_name: &str, args: &serde_json::Value) -> ToolResult {
        let tool = match self.registry.get(tool_name) {
            Some(t) => t,
            None => {
                tracing::error!(tool = tool_name, "[Tools] Unknown tool requested");
                return ToolResult::error(&format!("unknown tool: {}", tool_name));
            }
        };

        let start = std::time::Instant::now();
        tracing::debug!(
            tool = tool_name,
            timeout_secs = self.config.timeout_secs,
            "[Tools] Executing via executor"
        );

        // Execute with timeout
        match tokio::time::timeout(
            std::time::Duration::from_secs(self.config.timeout_secs),
            tool.execute(args),
        )
        .await
        {
            Ok(result) => {
                let elapsed = start.elapsed();
                if result.is_error {
                    tracing::warn!(
                        tool = tool_name,
                        duration_ms = elapsed.as_millis() as u64,
                        "[Tools] Executor: tool failed"
                    );
                } else {
                    tracing::debug!(
                        tool = tool_name,
                        duration_ms = elapsed.as_millis() as u64,
                        "[Tools] Executor: tool completed"
                    );
                }
                result
            }
            Err(_) => {
                tracing::error!(
                    tool = tool_name,
                    timeout_secs = self.config.timeout_secs,
                    "[Tools] Executor: tool timed out"
                );
                ToolResult::error(&format!(
                    "tool {} timed out after {}s",
                    tool_name, self.config.timeout_secs
                ))
            }
        }
    }

    /// Execute multiple tool calls concurrently.
    pub async fn execute_batch(
        &self,
        calls: Vec<(String, serde_json::Value)>,
    ) -> Vec<ToolResult> {
        let futures: Vec<_> = calls
            .into_iter()
            .map(|(name, args)| {
                let registry = Arc::clone(&self.registry);
                let timeout = self.config.timeout_secs;
                async move {
                    let tool = match registry.get(&name) {
                        Some(t) => t,
                        None => return ToolResult::error(&format!("unknown tool: {}", name)),
                    };
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(timeout),
                        tool.execute(&args),
                    )
                    .await
                    {
                        Ok(result) => result,
                        Err(_) => ToolResult::error(&format!("tool {} timed out", name)),
                    }
                }
            })
            .collect();

        futures::future::join_all(futures).await
    }
}

#[cfg(test)]
mod tests;

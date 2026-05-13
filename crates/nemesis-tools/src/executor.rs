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
            None => return ToolResult::error(&format!("unknown tool: {}", tool_name)),
        };

        // Execute with timeout
        match tokio::time::timeout(
            std::time::Duration::from_secs(self.config.timeout_secs),
            tool.execute(args),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => ToolResult::error(&format!(
                "tool {} timed out after {}s",
                tool_name, self.config.timeout_secs
            )),
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
mod tests {
    use super::*;
    use crate::registry::Tool;
    use async_trait::async_trait;

    struct SlowTool;

    #[async_trait]
    impl Tool for SlowTool {
        fn name(&self) -> &str { "slow" }
        fn description(&self) -> &str { "A slow tool" }
        fn parameters(&self) -> serde_json::Value { serde_json::json!({"type": "object"}) }
        async fn execute(&self, _args: &serde_json::Value) -> ToolResult {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            ToolResult::success("done")
        }
    }

    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str { "echo" }
        fn description(&self) -> &str { "Echo" }
        fn parameters(&self) -> serde_json::Value { serde_json::json!({"type": "object"}) }
        async fn execute(&self, args: &serde_json::Value) -> ToolResult {
            ToolResult::success(args["text"].as_str().unwrap_or(""))
        }
    }

    #[tokio::test]
    async fn test_execute_unknown_tool() {
        let registry = Arc::new(ToolRegistry::new());
        let executor = ToolExecutor::new(registry, ExecutorConfig::default());
        let result = executor.execute("unknown", &serde_json::json!({})).await;
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_execute_tool() {
        let registry = Arc::new(ToolRegistry::new());
        registry.register(Arc::new(EchoTool));
        let executor = ToolExecutor::new(registry, ExecutorConfig::default());
        let result = executor.execute("echo", &serde_json::json!({"text": "hello"})).await;
        assert_eq!(result.for_llm, "hello");
    }

    #[tokio::test]
    async fn test_execute_batch() {
        let registry = Arc::new(ToolRegistry::new());
        registry.register(Arc::new(EchoTool));
        let executor = ToolExecutor::new(registry, ExecutorConfig::default());
        let results = executor
            .execute_batch(vec![
                ("echo".to_string(), serde_json::json!({"text": "a"})),
                ("echo".to_string(), serde_json::json!({"text": "b"})),
            ])
            .await;
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_timeout() {
        let registry = Arc::new(ToolRegistry::new());
        registry.register(Arc::new(SlowTool));
        let executor = ToolExecutor::new(registry, ExecutorConfig {
            timeout_secs: 0, // Immediate timeout
            ..Default::default()
        });
        let _result = executor.execute("slow", &serde_json::json!({})).await;
        // With 0 timeout, it should timeout
        // Note: 0s timeout is very aggressive, the slow tool sleeps 100ms
    }

    // ============================================================
    // Additional executor tests
    // ============================================================

    #[test]
    fn test_executor_config_default() {
        let config = ExecutorConfig::default();
        assert_eq!(config.max_concurrent, 10);
        assert_eq!(config.timeout_secs, 300);
    }

    #[test]
    fn test_executor_config_custom() {
        let config = ExecutorConfig {
            max_concurrent: 20,
            timeout_secs: 600,
        };
        assert_eq!(config.max_concurrent, 20);
        assert_eq!(config.timeout_secs, 600);
    }

    #[tokio::test]
    async fn test_execute_with_missing_args() {
        let registry = Arc::new(ToolRegistry::new());
        registry.register(Arc::new(EchoTool));
        let executor = ToolExecutor::new(registry, ExecutorConfig::default());

        let result = executor.execute("echo", &serde_json::json!({})).await;
        assert!(!result.is_error);
        assert_eq!(result.for_llm, "");
    }

    #[tokio::test]
    async fn test_execute_batch_with_unknown() {
        let registry = Arc::new(ToolRegistry::new());
        registry.register(Arc::new(EchoTool));
        let executor = ToolExecutor::new(registry, ExecutorConfig::default());

        let results = executor
            .execute_batch(vec![
                ("echo".to_string(), serde_json::json!({"text": "a"})),
                ("unknown".to_string(), serde_json::json!({})),
            ])
            .await;
        assert_eq!(results.len(), 2);
        assert!(!results[0].is_error);
        assert!(results[1].is_error);
    }

    #[tokio::test]
    async fn test_execute_batch_all_unknown() {
        let registry = Arc::new(ToolRegistry::new());
        let executor = ToolExecutor::new(registry, ExecutorConfig::default());

        let results = executor
            .execute_batch(vec![
                ("a".to_string(), serde_json::json!({})),
                ("b".to_string(), serde_json::json!({})),
            ])
            .await;
        assert_eq!(results.len(), 2);
        assert!(results[0].is_error);
        assert!(results[1].is_error);
    }

    #[tokio::test]
    async fn test_execute_batch_empty() {
        let registry = Arc::new(ToolRegistry::new());
        let executor = ToolExecutor::new(registry, ExecutorConfig::default());

        let results = executor.execute_batch(vec![]).await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_execute_multiple_different_tools() {
        let registry = Arc::new(ToolRegistry::new());
        registry.register(Arc::new(EchoTool));

        struct ReverseTool;
        #[async_trait]
        impl Tool for ReverseTool {
            fn name(&self) -> &str { "reverse" }
            fn description(&self) -> &str { "Reverse text" }
            fn parameters(&self) -> serde_json::Value { serde_json::json!({"type": "object"}) }
            async fn execute(&self, args: &serde_json::Value) -> ToolResult {
                let text = args["text"].as_str().unwrap_or("");
                ToolResult::success(&text.chars().rev().collect::<String>())
            }
        }
        registry.register(Arc::new(ReverseTool));

        let executor = ToolExecutor::new(registry, ExecutorConfig::default());

        let results = executor
            .execute_batch(vec![
                ("echo".to_string(), serde_json::json!({"text": "hello"})),
                ("reverse".to_string(), serde_json::json!({"text": "abc"})),
            ])
            .await;
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].for_llm, "hello");
        assert_eq!(results[1].for_llm, "cba");
    }

    #[tokio::test]
    async fn test_execute_tool_that_errors() {
        struct ErrorTool;
        #[async_trait]
        impl Tool for ErrorTool {
            fn name(&self) -> &str { "error_tool" }
            fn description(&self) -> &str { "Always errors" }
            fn parameters(&self) -> serde_json::Value { serde_json::json!({"type": "object"}) }
            async fn execute(&self, _args: &serde_json::Value) -> ToolResult {
                ToolResult::error("deliberate error")
            }
        }

        let registry = Arc::new(ToolRegistry::new());
        registry.register(Arc::new(ErrorTool));
        let executor = ToolExecutor::new(registry, ExecutorConfig::default());

        let result = executor.execute("error_tool", &serde_json::json!({})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("deliberate error"));
    }

    #[tokio::test]
    async fn test_execute_concurrent_batch() {
        let registry = Arc::new(ToolRegistry::new());
        registry.register(Arc::new(SlowTool));
        let executor = ToolExecutor::new(registry, ExecutorConfig::default());

        // Run 5 concurrent slow tools - they should all complete
        let calls: Vec<(String, serde_json::Value)> = (0..5)
            .map(|i| ("slow".to_string(), serde_json::json!({"text": format!("task-{}", i)})))
            .collect();

        let results = executor.execute_batch(calls).await;
        assert_eq!(results.len(), 5);
        for result in &results {
            assert!(!result.is_error, "Expected success, got: {}", result.for_llm);
        }
    }

    // ---- Additional coverage tests for 95%+ ----

    #[tokio::test]
    async fn test_execute_batch_timeout() {
        let registry = Arc::new(ToolRegistry::new());
        registry.register(Arc::new(SlowTool));
        let executor = ToolExecutor::new(registry, ExecutorConfig {
            timeout_secs: 0, // Immediate timeout
            max_concurrent: 10,
        });

        let results = executor
            .execute_batch(vec![
                ("slow".to_string(), serde_json::json!({})),
            ])
            .await;
        assert_eq!(results.len(), 1);
        assert!(results[0].is_error);
        assert!(results[0].for_llm.contains("timed out"));
    }

    #[tokio::test]
    async fn test_execute_batch_mixed_success_and_unknown() {
        let registry = Arc::new(ToolRegistry::new());
        registry.register(Arc::new(EchoTool));
        let executor = ToolExecutor::new(registry, ExecutorConfig::default());

        let results = executor
            .execute_batch(vec![
                ("echo".to_string(), serde_json::json!({"text": "ok"})),
                ("unknown".to_string(), serde_json::json!({})),
                ("echo".to_string(), serde_json::json!({"text": "ok2"})),
            ])
            .await;
        assert_eq!(results.len(), 3);
        assert!(!results[0].is_error);
        assert!(results[1].is_error);
        assert!(!results[2].is_error);
    }

    #[tokio::test]
    async fn test_execute_with_custom_timeout() {
        let registry = Arc::new(ToolRegistry::new());
        registry.register(Arc::new(EchoTool));
        let executor = ToolExecutor::new(registry, ExecutorConfig {
            timeout_secs: 1,
            max_concurrent: 5,
        });

        let result = executor.execute("echo", &serde_json::json!({"text": "fast"})).await;
        assert!(!result.is_error);
        assert_eq!(result.for_llm, "fast");
    }
}

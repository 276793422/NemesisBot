//! Tool loop - core LLM + tool call iteration engine.

use crate::registry::ToolRegistry;
use crate::types::ToolResult;
use std::sync::Arc;

/// Configuration for the tool execution loop.
pub struct ToolLoopConfig {
    /// Tool registry for looking up tools.
    pub tools: Arc<ToolRegistry>,
    /// Maximum number of iterations.
    pub max_iterations: usize,
    /// Per-iteration timeout in seconds.
    pub timeout_secs: u64,
}

impl Default for ToolLoopConfig {
    fn default() -> Self {
        Self {
            tools: Arc::new(ToolRegistry::new()),
            max_iterations: 10,
            timeout_secs: 300,
        }
    }
}

/// Result of running the tool loop.
#[derive(Debug, Clone)]
pub struct ToolLoopResult {
    /// Final text content from the LLM.
    pub content: String,
    /// Number of iterations executed.
    pub iterations: usize,
}

/// A simulated LLM response for the tool loop.
///
/// This is a simplified internal type used for testing and simulation.
/// The actual LLM response type used in production is
/// `nemesis_providers::types::LLMResponse`, which includes additional
/// fields like `finish_reason` and `usage`. This type is intentionally
/// kept separate to avoid coupling `nemesis-tools` to `nemesis-providers`.
///
/// When integrating with the real agent loop, the caller should convert
/// from `nemesis_providers::types::LLMResponse` to this type by mapping
/// `tool_calls` (from `Vec<ToolCall>` with `FunctionCall` to `LLMToolCall`).
#[derive(Debug, Clone)]
pub struct LLMResponse {
    /// Text content of the response.
    pub content: String,
    /// Tool calls requested by the LLM.
    pub tool_calls: Vec<LLMToolCall>,
}

/// A tool call from the LLM.
#[derive(Debug, Clone)]
pub struct LLMToolCall {
    /// Tool call ID.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Tool arguments as JSON.
    pub arguments: serde_json::Value,
}

/// Callback type for LLM responses.
///
/// The callback receives the accumulated messages/tool results from the
/// previous iteration and should return the LLM's response. Wrapped in
/// `Arc` so it can be cloned and shared across tasks (e.g., subagent spawns).
pub type LLMCallback = Arc<dyn Fn(Vec<serde_json::Value>) -> LLMResponse + Send + Sync>;

/// Run the tool loop with a simulated LLM callback.
///
/// The callback receives the accumulated tool results from the previous iteration
/// (empty on the first call) and should return the LLM's response.
/// The loop continues until the LLM returns no tool calls or max_iterations is reached.
pub async fn run_tool_loop(
    config: ToolLoopConfig,
    llm_callback: &LLMCallback,
    initial_messages: Vec<serde_json::Value>,
) -> ToolLoopResult {
    let mut iteration = 0;
    let mut tool_results: Vec<serde_json::Value> = Vec::new();
    let mut final_content = String::new();

    loop {
        if iteration >= config.max_iterations {
            break;
        }
        iteration += 1;

        // Call the LLM
        let response = if iteration == 1 {
            llm_callback(initial_messages.clone())
        } else {
            llm_callback(tool_results.clone())
        };

        // If no tool calls, we're done
        if response.tool_calls.is_empty() {
            final_content = response.content;
            break;
        }

        // Execute tool calls
        tool_results.clear();
        for tc in &response.tool_calls {
            let result = if config.tools.has(&tc.name) {
                // Execute with timeout
                match tokio::time::timeout(
                    std::time::Duration::from_secs(config.timeout_secs),
                    execute_tool(&config.tools, &tc.name, &tc.arguments),
                )
                .await
                {
                    Ok(r) => r,
                    Err(_) => ToolResult::error(&format!(
                        "tool {} timed out after {}s",
                        tc.name, config.timeout_secs
                    )),
                }
            } else {
                ToolResult::error(&format!("unknown tool: {}", tc.name))
            };

            tool_results.push(serde_json::json!({
                "tool_call_id": tc.id,
                "tool_name": tc.name,
                "result": result.for_llm,
                "is_error": result.is_error,
            }));
        }
    }

    ToolLoopResult {
        content: final_content,
        iterations: iteration,
    }
}

/// Execute a single tool call.
async fn execute_tool(
    registry: &ToolRegistry,
    name: &str,
    args: &serde_json::Value,
) -> ToolResult {
    match registry.get(name) {
        Some(tool) => tool.execute(args).await,
        None => ToolResult::error(&format!("unknown tool: {}", name)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Tool;
    use async_trait::async_trait;

    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str { "echo" }
        fn description(&self) -> &str { "Echo back the input" }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}})
        }
        async fn execute(&self, args: &serde_json::Value) -> ToolResult {
            ToolResult::success(args["text"].as_str().unwrap_or(""))
        }
    }

    #[tokio::test]
    async fn test_tool_loop_no_tool_calls() {
        let registry = Arc::new(ToolRegistry::new());
        let config = ToolLoopConfig {
            tools: registry,
            max_iterations: 5,
            timeout_secs: 10,
        };

        let callback: LLMCallback = Arc::new(|_msgs| LLMResponse {
            content: "Hello, world!".to_string(),
            tool_calls: vec![],
        });

        let result = run_tool_loop(config, &callback, vec![]).await;
        assert_eq!(result.content, "Hello, world!");
        assert_eq!(result.iterations, 1);
    }

    #[tokio::test]
    async fn test_tool_loop_with_tool_calls() {
        let registry = Arc::new(ToolRegistry::new());
        registry.register(Arc::new(EchoTool));
        let config = ToolLoopConfig {
            tools: registry,
            max_iterations: 5,
            timeout_secs: 10,
        };

        let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let call_count_clone = Arc::clone(&call_count);

        let callback: LLMCallback = Arc::new(move |_msgs| {
            let count = call_count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if count == 0 {
                LLMResponse {
                    content: String::new(),
                    tool_calls: vec![LLMToolCall {
                        id: "tc-1".to_string(),
                        name: "echo".to_string(),
                        arguments: serde_json::json!({"text": "hello"}),
                    }],
                }
            } else {
                LLMResponse {
                    content: "Done after echo".to_string(),
                    tool_calls: vec![],
                }
            }
        });

        let result = run_tool_loop(config, &callback, vec![]).await;
        assert_eq!(result.content, "Done after echo");
        assert_eq!(result.iterations, 2);
    }

    #[tokio::test]
    async fn test_tool_loop_max_iterations() {
        let registry = Arc::new(ToolRegistry::new());
        registry.register(Arc::new(EchoTool));
        let config = ToolLoopConfig {
            tools: registry,
            max_iterations: 3,
            timeout_secs: 10,
        };

        let callback: LLMCallback = Arc::new(|_msgs| LLMResponse {
            content: String::new(),
            tool_calls: vec![LLMToolCall {
                id: "tc-1".to_string(),
                name: "echo".to_string(),
                arguments: serde_json::json!({"text": "loop"}),
            }],
        });

        let result = run_tool_loop(config, &callback, vec![]).await;
        assert_eq!(result.iterations, 3);
        // Content should be empty since the LLM never returned without tool calls
        assert!(result.content.is_empty());
    }

    #[tokio::test]
    async fn test_tool_loop_unknown_tool() {
        let registry = Arc::new(ToolRegistry::new());
        let config = ToolLoopConfig {
            tools: registry,
            max_iterations: 3,
            timeout_secs: 10,
        };

        let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let call_count_clone = Arc::clone(&call_count);

        let callback: LLMCallback = Arc::new(move |_msgs| {
            let count = call_count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if count == 0 {
                LLMResponse {
                    content: String::new(),
                    tool_calls: vec![LLMToolCall {
                        id: "tc-1".to_string(),
                        name: "nonexistent_tool".to_string(),
                        arguments: serde_json::json!({}),
                    }],
                }
            } else {
                LLMResponse {
                    content: "Handled error".to_string(),
                    tool_calls: vec![],
                }
            }
        });

        let result = run_tool_loop(config, &callback, vec![]).await;
        assert_eq!(result.content, "Handled error");
        assert_eq!(result.iterations, 2);
    }

    // ============================================================
    // Additional tool loop tests
    // ============================================================

    #[test]
    fn test_tool_loop_config_default() {
        let config = ToolLoopConfig::default();
        assert_eq!(config.max_iterations, 10);
        assert_eq!(config.timeout_secs, 300);
    }

    #[tokio::test]
    async fn test_tool_loop_multiple_tool_calls_per_iteration() {
        let registry = Arc::new(ToolRegistry::new());
        registry.register(Arc::new(EchoTool));

        let config = ToolLoopConfig {
            tools: registry,
            max_iterations: 5,
            timeout_secs: 10,
        };

        let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let call_count_clone = Arc::clone(&call_count);

        let callback: LLMCallback = Arc::new(move |_msgs| {
            let count = call_count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if count == 0 {
                LLMResponse {
                    content: String::new(),
                    tool_calls: vec![
                        LLMToolCall {
                            id: "tc-1".to_string(),
                            name: "echo".to_string(),
                            arguments: serde_json::json!({"text": "first"}),
                        },
                        LLMToolCall {
                            id: "tc-2".to_string(),
                            name: "echo".to_string(),
                            arguments: serde_json::json!({"text": "second"}),
                        },
                    ],
                }
            } else {
                LLMResponse {
                    content: "Both done".to_string(),
                    tool_calls: vec![],
                }
            }
        });

        let result = run_tool_loop(config, &callback, vec![]).await;
        assert_eq!(result.content, "Both done");
        assert_eq!(result.iterations, 2);
    }

    #[tokio::test]
    async fn test_tool_loop_passes_initial_messages() {
        let registry = Arc::new(ToolRegistry::new());
        let config = ToolLoopConfig {
            tools: registry,
            max_iterations: 5,
            timeout_secs: 10,
        };

        let captured_msgs = Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_clone = captured_msgs.clone();

        let callback: LLMCallback = Arc::new(move |msgs| {
            if let Ok(mut guard) = captured_clone.try_lock() {
                *guard = msgs.clone();
            }
            LLMResponse {
                content: "received".to_string(),
                tool_calls: vec![],
            }
        });

        let initial = vec![
            serde_json::json!({"role": "user", "content": "hello"}),
            serde_json::json!({"role": "assistant", "content": "hi"}),
        ];

        let result = run_tool_loop(config, &callback, initial).await;
        assert_eq!(result.content, "received");
        assert_eq!(result.iterations, 1);

        let msgs = captured_msgs.lock().unwrap().clone();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["content"], "hello");
    }

    #[tokio::test]
    async fn test_tool_loop_result_type() {
        let registry = Arc::new(ToolRegistry::new());
        let config = ToolLoopConfig {
            tools: registry,
            max_iterations: 5,
            timeout_secs: 10,
        };

        let callback: LLMCallback = Arc::new(|_| LLMResponse {
            content: "final answer".to_string(),
            tool_calls: vec![],
        });

        let result = run_tool_loop(config, &callback, vec![]).await;
        assert_eq!(result.content, "final answer");
        assert_eq!(result.iterations, 1);
    }

    #[tokio::test]
    async fn test_tool_loop_unknown_tool_in_batch() {
        let registry = Arc::new(ToolRegistry::new());
        registry.register(Arc::new(EchoTool));

        let config = ToolLoopConfig {
            tools: registry,
            max_iterations: 5,
            timeout_secs: 10,
        };

        let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let call_count_clone = Arc::clone(&call_count);

        let callback: LLMCallback = Arc::new(move |_msgs| {
            let count = call_count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if count == 0 {
                LLMResponse {
                    content: String::new(),
                    tool_calls: vec![
                        LLMToolCall {
                            id: "tc-1".to_string(),
                            name: "echo".to_string(),
                            arguments: serde_json::json!({"text": "known"}),
                        },
                        LLMToolCall {
                            id: "tc-2".to_string(),
                            name: "unknown_tool".to_string(),
                            arguments: serde_json::json!({}),
                        },
                    ],
                }
            } else {
                LLMResponse {
                    content: "Mixed results handled".to_string(),
                    tool_calls: vec![],
                }
            }
        });

        let result = run_tool_loop(config, &callback, vec![]).await;
        assert_eq!(result.content, "Mixed results handled");
        assert_eq!(result.iterations, 2);
    }

    #[tokio::test]
    async fn test_tool_loop_zero_iterations() {
        let registry = Arc::new(ToolRegistry::new());
        let config = ToolLoopConfig {
            tools: registry,
            max_iterations: 0,
            timeout_secs: 10,
        };

        let callback: LLMCallback = Arc::new(|_| LLMResponse {
            content: "should not run".to_string(),
            tool_calls: vec![],
        });

        let result = run_tool_loop(config, &callback, vec![]).await;
        assert!(result.content.is_empty());
        assert_eq!(result.iterations, 0);
    }

    // --- Additional toolloop tests ---

    #[test]
    fn test_llm_response_clone() {
        let resp = LLMResponse {
            content: "test".into(),
            tool_calls: vec![LLMToolCall {
                id: "tc-1".into(),
                name: "echo".into(),
                arguments: serde_json::json!({"a": 1}),
            }],
        };
        let cloned = resp.clone();
        assert_eq!(cloned.content, "test");
        assert_eq!(cloned.tool_calls.len(), 1);
        assert_eq!(cloned.tool_calls[0].id, "tc-1");
    }

    #[test]
    fn test_llm_tool_call_debug() {
        let tc = LLMToolCall {
            id: "tc-1".into(),
            name: "echo".into(),
            arguments: serde_json::json!({}),
        };
        let debug_str = format!("{:?}", tc);
        assert!(debug_str.contains("tc-1"));
        assert!(debug_str.contains("echo"));
    }

    #[test]
    fn test_tool_loop_result_debug() {
        let result = ToolLoopResult {
            content: "done".into(),
            iterations: 3,
        };
        let debug_str = format!("{:?}", result);
        assert!(debug_str.contains("done"));
        assert!(debug_str.contains("3"));
    }

    #[tokio::test]
    async fn test_tool_loop_single_iteration_no_tools() {
        let registry = Arc::new(ToolRegistry::new());
        let config = ToolLoopConfig {
            tools: registry,
            max_iterations: 5,
            timeout_secs: 10,
        };
        let callback: LLMCallback = Arc::new(|_| LLMResponse {
            content: "immediate answer".into(),
            tool_calls: vec![],
        });
        let result = run_tool_loop(config, &callback, vec![]).await;
        assert_eq!(result.content, "immediate answer");
        assert_eq!(result.iterations, 1);
    }

    #[tokio::test]
    async fn test_tool_loop_empty_content() {
        let registry = Arc::new(ToolRegistry::new());
        let config = ToolLoopConfig {
            tools: registry,
            max_iterations: 5,
            timeout_secs: 10,
        };
        let callback: LLMCallback = Arc::new(|_| LLMResponse {
            content: String::new(),
            tool_calls: vec![],
        });
        let result = run_tool_loop(config, &callback, vec![]).await;
        assert!(result.content.is_empty());
        assert_eq!(result.iterations, 1);
    }

    #[test]
    fn test_llm_response_with_multiple_tool_calls() {
        let resp = LLMResponse {
            content: String::new(),
            tool_calls: vec![
                LLMToolCall { id: "1".into(), name: "a".into(), arguments: serde_json::json!({}) },
                LLMToolCall { id: "2".into(), name: "b".into(), arguments: serde_json::json!({}) },
                LLMToolCall { id: "3".into(), name: "c".into(), arguments: serde_json::json!({}) },
            ],
        };
        assert_eq!(resp.tool_calls.len(), 3);
    }

    #[tokio::test]
    async fn test_tool_loop_max_iterations_reached() {
        let registry = Arc::new(ToolRegistry::new());
        registry.register(Arc::new(EchoTool));
        let config = ToolLoopConfig {
            tools: registry,
            max_iterations: 3,
            timeout_secs: 10,
        };
        let callback: LLMCallback = Arc::new(|_| LLMResponse {
            content: String::new(),
            tool_calls: vec![LLMToolCall {
                id: "tc-loop".into(),
                name: "echo".into(),
                arguments: serde_json::json!({"text": "loop"}),
            }],
        });
        let result = run_tool_loop(config, &callback, vec![]).await;
        assert_eq!(result.iterations, 3);
    }
}

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

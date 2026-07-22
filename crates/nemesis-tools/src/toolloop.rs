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
    tracing::debug!(
        max_iterations = config.max_iterations,
        timeout_secs = config.timeout_secs,
        "[Tools/ToolLoop] Starting tool loop"
    );
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

    tracing::debug!(
        iterations = iteration,
        "[Tools/ToolLoop] Tool loop completed"
    );
    ToolLoopResult {
        content: final_content,
        iterations: iteration,
    }
}

/// Execute a single tool call.
async fn execute_tool(registry: &ToolRegistry, name: &str, args: &serde_json::Value) -> ToolResult {
    match registry.get(name) {
        Some(tool) => tool.execute(args).await,
        None => ToolResult::error(&format!("unknown tool: {}", name)),
    }
}

#[cfg(test)]
mod tests;

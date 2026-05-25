//! Agent types used within the agent engine.
//!
//! These types represent conversation turns, tool results, agent state,
//! and events emitted during the agent loop execution.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use tracing::debug;

/// Configuration for an agent instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// The LLM model identifier (e.g. "gpt-4", "claude-sonnet-4-6").
    pub model: String,
    /// System prompt injected at the start of every conversation.
    pub system_prompt: Option<String>,
    /// Maximum number of LLM tool-calling iterations per request.
    pub max_turns: u32,
    /// Names of tools available to this agent.
    pub tools: Vec<String>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: "gpt-4".to_string(),
            system_prompt: None,
            max_turns: 10,
            tools: Vec::new(),
        }
    }
}

/// A single conversation turn in the agent history.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConversationTurn {
    /// Role: "system", "user", "assistant", or "tool".
    pub role: String,
    /// Text content of the turn.
    pub content: String,
    /// Tool calls issued by the assistant in this turn.
    pub tool_calls: Vec<ToolCallInfo>,
    /// Tool call ID this turn responds to (set for role "tool").
    pub tool_call_id: Option<String>,
    /// Timestamp of the turn (ISO 8601).
    pub timestamp: String,
    /// Reasoning content from thinking-mode models (e.g., DeepSeek R1, GLM).
    /// Stored for passing back to the API in subsequent turns.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reasoning_content: Option<String>,
}

/// Information about a single tool call within a conversation turn.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCallInfo {
    /// Unique ID assigned by the LLM for this tool call.
    pub id: String,
    /// Name of the tool to invoke.
    pub name: String,
    /// JSON-encoded arguments for the tool.
    pub arguments: String,
}

/// Result returned after executing a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    /// Name of the tool that was executed.
    pub tool_name: String,
    /// The output string from the tool.
    pub result: String,
    /// Whether the tool execution resulted in an error.
    pub is_error: bool,
}

/// Options for LLM chat completion requests.
///
/// Mirrors the Go `options map[string]interface{}` passed to `Chat()`.
/// These control generation parameters like temperature and max output tokens.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatOptions {
    /// Maximum number of tokens to generate in the response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Sampling temperature (0.0 = deterministic, 1.0 = creative).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Nucleus sampling threshold.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// Stop sequences that end generation early.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
}

impl Default for ChatOptions {
    fn default() -> Self {
        Self {
            max_tokens: Some(8192),
            temperature: Some(0.7),
            top_p: None,
            stop: None,
        }
    }
}

/// Tool definition for LLM function calling.
///
/// Mirrors the OpenAI function calling format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Tool type (always "function").
    #[serde(rename = "type", default = "default_tool_type")]
    pub tool_type: String,
    /// Function definition.
    pub function: ToolFunctionDef,
}

fn default_tool_type() -> String {
    "function".to_string()
}

impl Default for ToolDefinition {
    fn default() -> Self {
        Self {
            tool_type: "function".to_string(),
            function: ToolFunctionDef::default(),
        }
    }
}

/// Function definition within a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolFunctionDef {
    /// Function name.
    pub name: String,
    /// Function description.
    pub description: String,
    /// JSON Schema for parameters.
    pub parameters: serde_json::Value,
}

impl Default for ToolFunctionDef {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }
    }
}

/// Current operational state of an agent instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentState {
    /// Agent is idle and ready to process a new request.
    Idle,
    /// Agent is waiting for an LLM response.
    Thinking,
    /// Agent is executing one or more tool calls.
    ExecutingTool,
    /// Agent is preparing the final response.
    Responding,
}

impl Default for AgentState {
    fn default() -> Self {
        Self::Idle
    }
}

/// Events emitted by the agent loop during execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentEvent {
    /// A text message was produced (intermediate or final).
    Message(String),
    /// The LLM requested one or more tool calls.
    ToolCall(Vec<ToolCallInfo>),
    /// A tool execution completed.
    ToolResult(ToolCallResult),
    /// An error occurred during execution.
    Error(String),
    /// The agent loop has finished processing.
    Done(String),
}

/// Fix orphaned tool message pairs in-place.
///
/// Guarantees two invariants:
/// 1. Every role="tool" message has a preceding role="assistant" with matching tool_call id
/// 2. Every assistant tool_call id has a corresponding tool response after it
///
/// When violated:
/// - Orphaned tool messages are removed
/// - Assistant tool_calls without responses are cleared
pub fn repair_tool_message_pairs(messages: &mut Vec<ConversationTurn>) {
    if messages.is_empty() {
        return;
    }

    // Pass 1: remove orphaned tool messages via retain.
    let mut seen_call_ids: HashSet<String> = HashSet::new();
    messages.retain(|msg| {
        let keep = if msg.role == "tool" {
            match msg.tool_call_id {
                Some(ref id) => seen_call_ids.contains(id),
                None => false,
            }
        } else {
            true
        };

        if msg.role == "assistant" {
            for tc in &msg.tool_calls {
                seen_call_ids.insert(tc.id.clone());
            }
        }

        if !keep {
            debug!("[repair_tool_message_pairs] Removing orphaned tool message");
        }
        keep
    });

    // Pass 2: clear incomplete assistant tool_calls.
    let n = messages.len();
    for i in 0..n {
        if messages[i].role == "assistant" && !messages[i].tool_calls.is_empty() {
            let call_ids: Vec<String> = messages[i].tool_calls.iter().map(|tc| tc.id.clone()).collect();
            let mut found_ids: HashSet<String> = HashSet::new();
            for j in (i + 1)..n {
                if messages[j].role == "tool" {
                    if let Some(ref tc_id) = messages[j].tool_call_id {
                        if call_ids.contains(tc_id) {
                            found_ids.insert(tc_id.clone());
                        }
                    }
                } else if messages[j].role == "assistant" {
                    break;
                }
            }
            if found_ids.len() < call_ids.len() {
                messages[i].tool_calls.retain(|tc| found_ids.contains(&tc.id));
            }
        }
    }
}

#[cfg(test)]
mod tests;

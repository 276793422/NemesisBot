//! Agent types used within the agent engine.
//!
//! These types represent conversation turns, tool results, agent state,
//! and events emitted during the agent loop execution.

use serde::{Deserialize, Serialize};

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

#[cfg(test)]
mod tests;

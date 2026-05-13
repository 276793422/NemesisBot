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
mod tests {
    use super::*;

    #[test]
    fn agent_config_default() {
        let config = AgentConfig::default();
        assert_eq!(config.model, "gpt-4");
        assert!(config.system_prompt.is_none());
        assert_eq!(config.max_turns, 10);
        assert!(config.tools.is_empty());
    }

    #[test]
    fn agent_config_serialization_roundtrip() {
        let config = AgentConfig {
            model: "claude-sonnet-4-6".to_string(),
            system_prompt: Some("You are helpful.".to_string()),
            max_turns: 5,
            tools: vec!["search".to_string(), "calculator".to_string()],
        };
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: AgentConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.model, config.model);
        assert_eq!(deserialized.system_prompt, config.system_prompt);
        assert_eq!(deserialized.max_turns, config.max_turns);
        assert_eq!(deserialized.tools, config.tools);
    }

    #[test]
    fn conversation_turn_serialization() {
        let turn = ConversationTurn {
            role: "user".to_string(),
            content: "Hello, world!".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: "2026-04-29T12:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&turn).unwrap();
        let parsed: ConversationTurn = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.role, "user");
        assert_eq!(parsed.content, "Hello, world!");
    }

    #[test]
    fn agent_event_variants() {
        let events = vec![
            AgentEvent::Message("hello".to_string()),
            AgentEvent::ToolCall(vec![ToolCallInfo {
                id: "tc_1".to_string(),
                name: "search".to_string(),
                arguments: "{}".to_string(),
            }]),
            AgentEvent::ToolResult(ToolCallResult {
                tool_name: "search".to_string(),
                result: "found".to_string(),
                is_error: false,
            }),
            AgentEvent::Error("something failed".to_string()),
            AgentEvent::Done("final answer".to_string()),
        ];

        // Verify serialization roundtrip for all variants
        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let parsed: AgentEvent = serde_json::from_str(&json).unwrap();
            let json2 = serde_json::to_string(&parsed).unwrap();
            assert_eq!(json, json2);
        }

        // Verify variant count
        assert_eq!(events.len(), 5);
    }

    #[test]
    fn conversation_turn_with_tool_calls() {
        let turn = ConversationTurn {
            role: "assistant".to_string(),
            content: String::new(),
            tool_calls: vec![
                ToolCallInfo {
                    id: "tc_1".to_string(),
                    name: "file_read".to_string(),
                    arguments: r#"{"path":"/tmp/test"}"#.to_string(),
                },
            ],
            tool_call_id: None,
            timestamp: "2026-04-29T12:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&turn).unwrap();
        let parsed: ConversationTurn = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.tool_calls.len(), 1);
        assert_eq!(parsed.tool_calls[0].name, "file_read");
    }

    #[test]
    fn tool_call_result_error() {
        let result = ToolCallResult {
            tool_name: "file_read".to_string(),
            result: "file not found".to_string(),
            is_error: true,
        };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: ToolCallResult = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_error);
        assert_eq!(parsed.result, "file not found");
    }

    #[test]
    fn agent_config_with_empty_tools() {
        let config = AgentConfig {
            model: "test".to_string(),
            system_prompt: None,
            max_turns: 1,
            tools: vec![],
        };
        assert!(config.tools.is_empty());
        let json = serde_json::to_string(&config).unwrap();
        let back: AgentConfig = serde_json::from_str(&json).unwrap();
        assert!(back.tools.is_empty());
    }

    #[test]
    fn conversation_turn_tool_call_id() {
        let turn = ConversationTurn {
            role: "tool".to_string(),
            content: "result data".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: Some("tc_123".to_string()),
            timestamp: "2026-04-29T12:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&turn).unwrap();
        let parsed: ConversationTurn = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.tool_call_id, Some("tc_123".to_string()));
    }

    // --- Additional types tests ---

    #[test]
    fn tool_call_info_equality() {
        let tc1 = ToolCallInfo {
            id: "tc_1".to_string(),
            name: "search".to_string(),
            arguments: r#"{"q":"test"}"#.to_string(),
        };
        let tc2 = ToolCallInfo {
            id: "tc_1".to_string(),
            name: "search".to_string(),
            arguments: r#"{"q":"test"}"#.to_string(),
        };
        assert_eq!(tc1, tc2);
    }

    #[test]
    fn tool_call_info_inequality() {
        let tc1 = ToolCallInfo {
            id: "tc_1".to_string(),
            name: "search".to_string(),
            arguments: "{}".to_string(),
        };
        let tc2 = ToolCallInfo {
            id: "tc_2".to_string(),
            name: "search".to_string(),
            arguments: "{}".to_string(),
        };
        assert_ne!(tc1, tc2);
    }

    #[test]
    fn tool_definition_serialization() {
        let def = ToolDefinition {
            tool_type: "function".to_string(),
            function: ToolFunctionDef {
                name: "calculator".to_string(),
                description: "Performs calculations".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "expr": {"type": "string"}
                    }
                }),
            },
        };
        let json = serde_json::to_string(&def).unwrap();
        let parsed: ToolDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.function.name, "calculator");
        assert_eq!(parsed.function.description, "Performs calculations");
    }

    #[test]
    fn tool_definition_default() {
        let def = ToolDefinition::default();
        assert_eq!(def.tool_type, "function");
        assert!(def.function.name.is_empty());
        assert!(def.function.description.is_empty());
        // Default parameters is a valid JSON schema object, not null
        assert!(def.function.parameters.is_object());
    }

    #[test]
    fn agent_state_variants() {
        assert_ne!(AgentState::Idle, AgentState::Thinking);
        assert_ne!(AgentState::Thinking, AgentState::ExecutingTool);
        assert_ne!(AgentState::ExecutingTool, AgentState::Responding);
        assert_ne!(AgentState::Responding, AgentState::Idle);
    }

    #[test]
    fn agent_state_serialization() {
        for state in &[AgentState::Idle, AgentState::Thinking, AgentState::ExecutingTool, AgentState::Responding] {
            let json = serde_json::to_string(&state).unwrap();
            let parsed: AgentState = serde_json::from_str(&json).unwrap();
            assert_eq!(*state, parsed);
        }
    }

    #[test]
    fn chat_options_default() {
        let opts = ChatOptions::default();
        assert_eq!(opts.max_tokens, Some(8192));
        assert_eq!(opts.temperature, Some(0.7));
    }

    #[test]
    fn chat_options_serialization() {
        let opts = ChatOptions {
            max_tokens: Some(4096),
            temperature: Some(0.5),
            top_p: None,
            stop: Some(vec!["\n".to_string()]),
        };
        let json = serde_json::to_string(&opts).unwrap();
        let parsed: ChatOptions = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.max_tokens, Some(4096));
        assert_eq!(parsed.temperature, Some(0.5));
        assert_eq!(parsed.stop, Some(vec!["\n".to_string()]));
    }

    #[test]
    fn tool_call_result_success() {
        let result = ToolCallResult {
            tool_name: "search".to_string(),
            result: "found it".to_string(),
            is_error: false,
        };
        assert!(!result.is_error);
        let json = serde_json::to_string(&result).unwrap();
        let parsed: ToolCallResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.tool_name, "search");
    }

    #[test]
    fn conversation_turn_clone() {
        let turn = ConversationTurn {
            role: "user".to_string(),
            content: "Hello".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: "2026-04-29T12:00:00Z".to_string(),
        };
        let cloned = turn.clone();
        assert_eq!(cloned.role, "user");
        assert_eq!(cloned.content, "Hello");
    }

    #[test]
    fn tool_call_info_clone() {
        let tc = ToolCallInfo {
            id: "tc_1".to_string(),
            name: "test".to_string(),
            arguments: "{}".to_string(),
        };
        let cloned = tc.clone();
        assert_eq!(cloned.id, "tc_1");
        assert_eq!(cloned.name, "test");
    }

    #[test]
    fn agent_event_done_matches() {
        let event = AgentEvent::Done("result".to_string());
        assert!(matches!(event, AgentEvent::Done(_)));

        let event = AgentEvent::Error("err".to_string());
        assert!(matches!(event, AgentEvent::Error(_)));

        let event = AgentEvent::Message("msg".to_string());
        assert!(matches!(event, AgentEvent::Message(_)));
    }

    #[test]
    fn tool_definition_custom() {
        let def = ToolDefinition {
            tool_type: "custom".to_string(),
            function: ToolFunctionDef {
                name: "my_tool".to_string(),
                description: "Custom tool".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
        };
        assert_eq!(def.tool_type, "custom");
        assert_eq!(def.function.name, "my_tool");
    }
}

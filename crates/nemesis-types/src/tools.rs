//! Tool-related types.

use serde::{Deserialize, Serialize};

/// Tool definition for the agent engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub required: Vec<String>,
}

/// Tool execution context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolContext {
    pub channel: String,
    pub chat_id: String,
    pub sender_id: String,
    pub session_key: String,
    pub correlation_id: Option<String>,
}

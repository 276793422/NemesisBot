//! Action schema definitions for cluster RPC actions.
//!
//! Defines the set of known actions that can be routed by the cluster's
//! default handler, along with their payload validation rules.

use serde::{Deserialize, Serialize};

/// Known action types in the cluster RPC protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Action {
    /// Bidirectional chat between peers.
    PeerChat,
    /// Callback for a completed peer_chat.
    PeerChatCallback,
    /// Share a forge reflection report with a remote node.
    ForgeShare,
    /// Request reflections from a remote node.
    ForgeGetReflections,
    /// Ping / health-check.
    Ping,
    /// Query remote node status.
    Status,
    /// LLM proxy: forward a chat completion request.
    LlmProxy,
    /// Query the capabilities of a remote node.
    GetCapabilities,
    /// Get detailed info about a remote node.
    GetInfo,
    /// List all available actions on a remote node.
    ListActions,
    /// Query the result of a previously submitted task.
    QueryTaskResult,
    /// Confirm delivery of a task result.
    ConfirmTaskDelivery,
    /// Custom action (user-defined).
    Custom,
}

impl std::fmt::Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PeerChat => write!(f, "peer_chat"),
            Self::PeerChatCallback => write!(f, "peer_chat_callback"),
            Self::ForgeShare => write!(f, "forge_share"),
            Self::ForgeGetReflections => write!(f, "forge_get_reflections"),
            Self::Ping => write!(f, "ping"),
            Self::Status => write!(f, "status"),
            Self::LlmProxy => write!(f, "llm_proxy"),
            Self::GetCapabilities => write!(f, "get_capabilities"),
            Self::GetInfo => write!(f, "get_info"),
            Self::ListActions => write!(f, "list_actions"),
            Self::QueryTaskResult => write!(f, "query_task_result"),
            Self::ConfirmTaskDelivery => write!(f, "confirm_task_delivery"),
            Self::Custom => write!(f, "custom"),
        }
    }
}

/// Schema definition for an action's expected payload fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionField {
    /// Field name.
    pub name: String,
    /// Whether the field is required.
    pub required: bool,
    /// Expected value type (e.g. "string", "number", "object").
    #[serde(rename = "type")]
    pub field_type: String,
}

/// Full schema for an action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionSchema {
    /// The action this schema describes.
    pub action: Action,
    /// Human-readable description.
    pub description: String,
    /// Expected fields in the payload.
    pub fields: Vec<ActionField>,
}

/// Returns the built-in action schemas for all known cluster actions.
pub fn builtin_schemas() -> Vec<ActionSchema> {
    vec![
        ActionSchema {
            action: Action::PeerChat,
            description: "Send a chat message to a remote peer for LLM processing".into(),
            fields: vec![
                ActionField { name: "message".into(), required: true, field_type: "string".into() },
                ActionField { name: "correlation_id".into(), required: true, field_type: "string".into() },
            ],
        },
        ActionSchema {
            action: Action::PeerChatCallback,
            description: "Callback with the result of a peer_chat".into(),
            fields: vec![
                ActionField { name: "task_id".into(), required: true, field_type: "string".into() },
                ActionField { name: "response".into(), required: true, field_type: "string".into() },
            ],
        },
        ActionSchema {
            action: Action::ForgeShare,
            description: "Share a forge reflection report".into(),
            fields: vec![
                ActionField { name: "report".into(), required: true, field_type: "object".into() },
                ActionField { name: "source_node".into(), required: true, field_type: "string".into() },
            ],
        },
        ActionSchema {
            action: Action::Ping,
            description: "Health check ping".into(),
            fields: vec![],
        },
        ActionSchema {
            action: Action::Status,
            description: "Query node status".into(),
            fields: vec![],
        },
        ActionSchema {
            action: Action::LlmProxy,
            description: "Proxy LLM chat completion to remote node".into(),
            fields: vec![
                ActionField { name: "messages".into(), required: true, field_type: "array".into() },
                ActionField { name: "model".into(), required: false, field_type: "string".into() },
            ],
        },
    ]
}

/// Parse an action string into an Action enum.
/// Returns `Action::Custom` for unknown actions.
pub fn parse_action(s: &str) -> Action {
    match s {
        "peer_chat" => Action::PeerChat,
        "peer_chat_callback" => Action::PeerChatCallback,
        "forge_share" => Action::ForgeShare,
        "forge_get_reflections" => Action::ForgeGetReflections,
        "ping" => Action::Ping,
        "status" => Action::Status,
        "llm_proxy" => Action::LlmProxy,
        "get_capabilities" => Action::GetCapabilities,
        "get_info" => Action::GetInfo,
        "list_actions" => Action::ListActions,
        "query_task_result" => Action::QueryTaskResult,
        "confirm_task_delivery" => Action::ConfirmTaskDelivery,
        _ => Action::Custom,
    }
}

#[cfg(test)]
mod tests;

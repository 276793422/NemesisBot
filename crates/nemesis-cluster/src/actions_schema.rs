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
mod tests {
    use super::*;

    #[test]
    fn test_action_display() {
        assert_eq!(Action::PeerChat.to_string(), "peer_chat");
        assert_eq!(Action::ForgeShare.to_string(), "forge_share");
        assert_eq!(Action::Custom.to_string(), "custom");
    }

    #[test]
    fn test_parse_action() {
        assert_eq!(parse_action("peer_chat"), Action::PeerChat);
        assert_eq!(parse_action("ping"), Action::Ping);
        assert_eq!(parse_action("unknown_action"), Action::Custom);
    }

    #[test]
    fn test_builtin_schemas_not_empty() {
        let schemas = builtin_schemas();
        assert!(!schemas.is_empty());
        assert!(schemas.iter().any(|s| s.action == Action::PeerChat));
    }

    #[test]
    fn test_action_schema_serialization() {
        let schemas = builtin_schemas();
        let json = serde_json::to_string(&schemas).unwrap();
        let back: Vec<ActionSchema> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.len(), schemas.len());
    }

    // -- Additional tests: actions schema edge cases --

    #[test]
    fn test_parse_all_known_actions() {
        assert_eq!(parse_action("peer_chat"), Action::PeerChat);
        assert_eq!(parse_action("peer_chat_callback"), Action::PeerChatCallback);
        assert_eq!(parse_action("forge_share"), Action::ForgeShare);
        assert_eq!(parse_action("forge_get_reflections"), Action::ForgeGetReflections);
        assert_eq!(parse_action("ping"), Action::Ping);
        assert_eq!(parse_action("status"), Action::Status);
        assert_eq!(parse_action("llm_proxy"), Action::LlmProxy);
        assert_eq!(parse_action("get_capabilities"), Action::GetCapabilities);
        assert_eq!(parse_action("get_info"), Action::GetInfo);
        assert_eq!(parse_action("list_actions"), Action::ListActions);
        assert_eq!(parse_action("query_task_result"), Action::QueryTaskResult);
        assert_eq!(parse_action("confirm_task_delivery"), Action::ConfirmTaskDelivery);
    }

    #[test]
    fn test_action_display_all_variants() {
        assert_eq!(Action::PeerChatCallback.to_string(), "peer_chat_callback");
        assert_eq!(Action::ForgeGetReflections.to_string(), "forge_get_reflections");
        assert_eq!(Action::LlmProxy.to_string(), "llm_proxy");
        assert_eq!(Action::GetCapabilities.to_string(), "get_capabilities");
        assert_eq!(Action::GetInfo.to_string(), "get_info");
        assert_eq!(Action::ListActions.to_string(), "list_actions");
        assert_eq!(Action::QueryTaskResult.to_string(), "query_task_result");
        assert_eq!(Action::ConfirmTaskDelivery.to_string(), "confirm_task_delivery");
        assert_eq!(Action::Custom.to_string(), "custom");
    }

    #[test]
    fn test_builtin_schemas_peer_chat_has_required_fields() {
        let schemas = builtin_schemas();
        let peer_chat = schemas.iter().find(|s| s.action == Action::PeerChat).unwrap();
        assert_eq!(peer_chat.fields.len(), 2);

        let message_field = peer_chat.fields.iter().find(|f| f.name == "message").unwrap();
        assert!(message_field.required);
        assert_eq!(message_field.field_type, "string");

        let correlation_field = peer_chat.fields.iter().find(|f| f.name == "correlation_id").unwrap();
        assert!(correlation_field.required);
    }

    #[test]
    fn test_builtin_schemas_ping_has_no_fields() {
        let schemas = builtin_schemas();
        let ping = schemas.iter().find(|s| s.action == Action::Ping).unwrap();
        assert!(ping.fields.is_empty());
    }

    #[test]
    fn test_builtin_schemas_llm_proxy_has_optional_model() {
        let schemas = builtin_schemas();
        let llm = schemas.iter().find(|s| s.action == Action::LlmProxy).unwrap();
        let model_field = llm.fields.iter().find(|f| f.name == "model").unwrap();
        assert!(!model_field.required);
    }
}

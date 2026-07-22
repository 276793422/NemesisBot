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
    assert_eq!(
        parse_action("forge_get_reflections"),
        Action::ForgeGetReflections
    );
    assert_eq!(parse_action("ping"), Action::Ping);
    assert_eq!(parse_action("status"), Action::Status);
    assert_eq!(parse_action("llm_proxy"), Action::LlmProxy);
    assert_eq!(parse_action("get_capabilities"), Action::GetCapabilities);
    assert_eq!(parse_action("get_info"), Action::GetInfo);
    assert_eq!(parse_action("list_actions"), Action::ListActions);
    assert_eq!(parse_action("query_task_result"), Action::QueryTaskResult);
    assert_eq!(
        parse_action("confirm_task_delivery"),
        Action::ConfirmTaskDelivery
    );
}

#[test]
fn test_action_display_all_variants() {
    assert_eq!(Action::PeerChatCallback.to_string(), "peer_chat_callback");
    assert_eq!(
        Action::ForgeGetReflections.to_string(),
        "forge_get_reflections"
    );
    assert_eq!(Action::LlmProxy.to_string(), "llm_proxy");
    assert_eq!(Action::GetCapabilities.to_string(), "get_capabilities");
    assert_eq!(Action::GetInfo.to_string(), "get_info");
    assert_eq!(Action::ListActions.to_string(), "list_actions");
    assert_eq!(Action::QueryTaskResult.to_string(), "query_task_result");
    assert_eq!(
        Action::ConfirmTaskDelivery.to_string(),
        "confirm_task_delivery"
    );
    assert_eq!(Action::Custom.to_string(), "custom");
}

#[test]
fn test_builtin_schemas_peer_chat_has_required_fields() {
    let schemas = builtin_schemas();
    let peer_chat = schemas
        .iter()
        .find(|s| s.action == Action::PeerChat)
        .unwrap();
    assert_eq!(peer_chat.fields.len(), 2);

    let message_field = peer_chat
        .fields
        .iter()
        .find(|f| f.name == "message")
        .unwrap();
    assert!(message_field.required);
    assert_eq!(message_field.field_type, "string");

    let correlation_field = peer_chat
        .fields
        .iter()
        .find(|f| f.name == "correlation_id")
        .unwrap();
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
    let llm = schemas
        .iter()
        .find(|s| s.action == Action::LlmProxy)
        .unwrap();
    let model_field = llm.fields.iter().find(|f| f.name == "model").unwrap();
    assert!(!model_field.required);
}

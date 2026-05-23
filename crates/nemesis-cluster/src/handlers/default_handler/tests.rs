use super::*;

#[test]
fn test_ping_action() {
    let handler = DefaultHandler::new("node-1".into());
    let result = handler.handle("ping", serde_json::json!({}));
    assert!(result.success);
    assert_eq!(result.response["status"], "ok");
}

#[test]
fn test_status_action() {
    let handler = DefaultHandler::new("node-1".into());
    let result = handler.handle("status", serde_json::json!({}));
    assert!(result.success);
    assert_eq!(result.response["node_id"], "node-1");
}

#[test]
fn test_unknown_action() {
    let handler = DefaultHandler::new("node-1".into());
    let result = handler.handle("unknown_xyz", serde_json::json!({}));
    assert!(!result.success);
    assert!(result.error.is_some());
}

#[test]
fn test_custom_action_registration() {
    let handler = DefaultHandler::new("node-1".into());
    handler.custom_handler().register(
        "my_action",
        std::sync::Arc::new(|_, p| Ok(p)),
    );

    let result = handler.handle("my_action", serde_json::json!({"key": "value"}));
    assert!(result.success);
    assert_eq!(result.response["key"], "value");
}

// -- Additional tests for uncovered actions --

struct MockNodeInfo {
    caps: Vec<String>,
    info: serde_json::Value,
}

impl NodeInfoProvider for MockNodeInfo {
    fn get_capabilities(&self) -> Vec<String> {
        self.caps.clone()
    }
    fn get_info(&self) -> serde_json::Value {
        self.info.clone()
    }
}

#[test]
fn test_with_node_info_construction() {
    let info = MockNodeInfo {
        caps: vec!["llm".into(), "tools".into()],
        info: serde_json::json!({"version": "1.0"}),
    };
    let handler = DefaultHandler::with_node_info("node-2".into(), Box::new(info));
    assert_eq!(handler.node_id, "node-2");
    assert!(handler.node_info.is_some());
}

#[test]
fn test_get_capabilities_without_node_info() {
    let handler = DefaultHandler::new("node-1".into());
    let result = handler.handle("get_capabilities", serde_json::json!({}));
    assert!(result.success);
    let caps = result.response["capabilities"].as_array().unwrap();
    assert!(caps.iter().any(|c| c == "peer_chat"));
    assert!(caps.iter().any(|c| c == "forge_share"));
}

#[test]
fn test_get_capabilities_with_node_info() {
    let info = MockNodeInfo {
        caps: vec!["custom_cap".into(), "llm".into()],
        info: serde_json::json!({}),
    };
    let handler = DefaultHandler::with_node_info("node-1".into(), Box::new(info));
    let result = handler.handle("get_capabilities", serde_json::json!({}));
    assert!(result.success);
    let caps = result.response["capabilities"].as_array().unwrap();
    assert!(caps.iter().any(|c| c == "custom_cap"));
    assert!(caps.iter().any(|c| c == "llm"));
    assert_eq!(result.response["node_id"], "node-1");
}

#[test]
fn test_get_info_without_node_info() {
    let handler = DefaultHandler::new("node-1".into());
    let result = handler.handle("get_info", serde_json::json!({}));
    assert!(result.success);
    assert_eq!(result.response["node_id"], "node-1");
    assert_eq!(result.response["status"], "online");
}

#[test]
fn test_get_info_with_node_info() {
    let info = MockNodeInfo {
        caps: vec![],
        info: serde_json::json!({"node_id": "n1", "version": "2.0", "uptime": 3600}),
    };
    let handler = DefaultHandler::with_node_info("n1".into(), Box::new(info));
    let result = handler.handle("get_info", serde_json::json!({}));
    assert!(result.success);
    assert_eq!(result.response["version"], "2.0");
    assert_eq!(result.response["uptime"], 3600);
}

#[test]
fn test_list_actions() {
    let handler = DefaultHandler::new("node-1".into());
    let result = handler.handle("list_actions", serde_json::json!({}));
    assert!(result.success);
    assert_eq!(result.response["node_id"], "node-1");
    let actions = result.response["actions"].as_array().unwrap();
    assert!(!actions.is_empty());
    // Verify each action entry has action and description fields
    for action in actions {
        assert!(action.get("action").is_some());
        assert!(action.get("description").is_some());
    }
}

#[test]
fn test_query_task_result_with_task_id() {
    let handler = DefaultHandler::new("node-1".into());
    let result = handler.handle(
        "query_task_result",
        serde_json::json!({"task_id": "task-123"}),
    );
    assert!(result.success);
    assert_eq!(result.response["task_id"], "task-123");
    assert_eq!(result.response["status"], "completed");
}

#[test]
fn test_query_task_result_without_task_id() {
    let handler = DefaultHandler::new("node-1".into());
    let result = handler.handle("query_task_result", serde_json::json!({}));
    assert!(result.success);
    assert_eq!(result.response["task_id"], "");
}

#[test]
fn test_confirm_task_delivery_with_task_id() {
    let handler = DefaultHandler::new("node-1".into());
    let result = handler.handle(
        "confirm_task_delivery",
        serde_json::json!({"task_id": "task-456"}),
    );
    assert!(result.success);
    assert_eq!(result.response["task_id"], "task-456");
    assert_eq!(result.response["confirmed"], true);
    assert!(result.error.is_none());
}

#[test]
fn test_confirm_task_delivery_without_task_id() {
    let handler = DefaultHandler::new("node-1".into());
    let result = handler.handle("confirm_task_delivery", serde_json::json!({}));
    assert!(result.success);
    assert_eq!(result.response["task_id"], "");
    assert_eq!(result.response["confirmed"], true);
}

#[test]
fn test_peer_chat_action() {
    let handler = DefaultHandler::new("node-1".into());
    let result = handler.handle(
        "peer_chat",
        serde_json::json!({"message": "hello", "correlation_id": "corr-1"}),
    );
    assert!(result.success);
    assert_eq!(result.response["status"], "accepted");
    assert!(result.error.is_none());
}

#[test]
fn test_peer_chat_callback_invalid_payload() {
    let handler = DefaultHandler::new("node-1".into());
    // Missing required fields for CallbackPayload
    let result = handler.handle("peer_chat_callback", serde_json::json!({"invalid": true}));
    assert!(!result.success);
    assert!(result.error.is_some());
}

#[test]
fn test_custom_action_execute_error() {
    let handler = DefaultHandler::new("node-1".into());
    handler.custom_handler().register(
        "failing_action",
        std::sync::Arc::new(|_, _| Err("action failed".to_string())),
    );
    let result = handler.handle("failing_action", serde_json::json!({}));
    assert!(!result.success);
    assert_eq!(result.error.unwrap(), "action failed");
}

#[test]
fn test_handle_result_fields() {
    let handler = DefaultHandler::new("node-1".into());
    let result = handler.handle("ping", serde_json::json!({}));
    assert!(result.success);
    assert!(result.error.is_none());
    assert!(result.response.is_object());
}

// ============================================================
// Coverage improvement: more action paths
// ============================================================

#[test]
fn test_forge_share_action() {
    let handler = DefaultHandler::new("node-1".into());
    let result = handler.handle(
        "forge_share",
        serde_json::json!({
            "report": {"insights": ["test"]},
            "source_node": "node-2"
        }),
    );
    assert!(result.success);
    assert_eq!(result.response["status"], "received");
}

#[test]
fn test_forge_share_missing_report() {
    let handler = DefaultHandler::new("node-1".into());
    let result = handler.handle(
        "forge_share",
        serde_json::json!({"source_node": "node-2"}),
    );
    assert!(!result.success);
}

#[test]
fn test_forge_get_reflections_action() {
    let handler = DefaultHandler::new("node-1".into());
    let result = handler.handle("forge_get_reflections", serde_json::json!({}));
    assert!(result.success);
    assert!(result.response.get("reflections").is_some());
}

#[test]
fn test_llm_proxy_action() {
    let handler = DefaultHandler::new("node-1".into());
    let result = handler.handle(
        "llm_proxy",
        serde_json::json!({"messages": [{"role": "user", "content": "hello"}]}),
    );
    // Without a real provider, returns success with a validation-only response
    assert!(result.success);
    assert!(result.response["content"].as_str().unwrap().contains("no provider configured"));
}

#[test]
fn test_peer_chat_callback_valid_payload() {
    let handler = DefaultHandler::new("node-1".into());
    let result = handler.handle(
        "peer_chat_callback",
        serde_json::json!({
            "task_id": "task-123",
            "success": true,
            "response": "hello",
        }),
    );
    assert!(result.success);
    assert_eq!(result.response["task_id"], "task-123");
}

#[test]
fn test_custom_handler_no_handler_registered() {
    let handler = DefaultHandler::new("node-1".into());
    // Custom action without registering a handler
    let result = handler.handle("custom_unknown_action", serde_json::json!({}));
    assert!(!result.success);
    assert!(result.error.is_some());
}

#[test]
fn test_custom_handler_success() {
    let handler = DefaultHandler::new("node-1".into());
    handler.custom_handler().register(
        "my_custom",
        std::sync::Arc::new(|_, p| {
            Ok(serde_json::json!({"echo": p}))
        }),
    );
    let result = handler.handle("my_custom", serde_json::json!({"data": 42}));
    assert!(result.success);
}

#[test]
fn test_get_info_default_response_fields() {
    let handler = DefaultHandler::new("node-test".into());
    let result = handler.handle("get_info", serde_json::json!({}));
    assert!(result.success);
    assert_eq!(result.response["node_id"], "node-test");
    assert_eq!(result.response["status"], "online");
}

#[test]
fn test_status_response() {
    let handler = DefaultHandler::new("node-status".into());
    let result = handler.handle("status", serde_json::json!({}));
    assert!(result.success);
    assert_eq!(result.response["node_id"], "node-status");
    assert_eq!(result.response["status"], "online");
}

#[test]
fn test_ping_response_node_id() {
    let handler = DefaultHandler::new("node-ping".into());
    let result = handler.handle("ping", serde_json::json!({}));
    assert!(result.success);
    assert_eq!(result.response["status"], "ok");
    assert_eq!(result.response["node_id"], "node-ping");
}

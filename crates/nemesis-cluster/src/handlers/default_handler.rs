//! Default action handler - routes actions to appropriate handlers.
//!
//! Acts as the main dispatcher for incoming RPC actions, routing them to
//! the correct specialized handler based on action type.

use crate::actions_schema::{self, Action};
use crate::handlers::callback::CallbackHandler;
use crate::handlers::custom::CustomHandler;
use crate::handlers::forge::ForgeHandler;
use crate::handlers::llm::LlmProxyHandler;

/// Result of handling an action.
#[derive(Debug, Clone)]
pub struct HandleResult {
    /// Whether the action was handled successfully.
    pub success: bool,
    /// Response payload.
    pub response: serde_json::Value,
    /// Error message if failed.
    pub error: Option<String>,
}

/// Node information provider for get_info/get_capabilities handlers.
pub trait NodeInfoProvider: Send + Sync {
    /// Get the capabilities of this node.
    fn get_capabilities(&self) -> Vec<String>;
    /// Get detailed info about this node.
    fn get_info(&self) -> serde_json::Value;
}

/// The default action handler that dispatches to specialized handlers.
pub struct DefaultHandler {
    node_id: String,
    callback: CallbackHandler,
    custom: CustomHandler,
    forge: ForgeHandler,
    llm: LlmProxyHandler,
    node_info: Option<Box<dyn NodeInfoProvider>>,
}

impl DefaultHandler {
    /// Create a new default handler with the given node ID.
    pub fn new(node_id: String) -> Self {
        Self {
            node_id: node_id.clone(),
            callback: CallbackHandler::new(node_id.clone()),
            custom: CustomHandler::new(),
            forge: ForgeHandler::new(node_id.clone()),
            llm: LlmProxyHandler::new(node_id),
            node_info: None,
        }
    }

    /// Create a handler with a node info provider for get_info/get_capabilities.
    pub fn with_node_info(node_id: String, info: Box<dyn NodeInfoProvider>) -> Self {
        Self {
            node_id: node_id.clone(),
            callback: CallbackHandler::new(node_id.clone()),
            custom: CustomHandler::new(),
            forge: ForgeHandler::new(node_id.clone()),
            llm: LlmProxyHandler::new(node_id),
            node_info: Some(info),
        }
    }

    /// Route and handle an incoming RPC action.
    pub fn handle(&self, action_str: &str, payload: serde_json::Value) -> HandleResult {
        let action = actions_schema::parse_action(action_str);

        match action {
            Action::PeerChatCallback => {
                match serde_json::from_value::<crate::handlers::callback::CallbackPayload>(payload) {
                    Ok(cb_payload) => {
                        if let Err(e) = self.callback.validate(&cb_payload) {
                            return HandleResult {
                                success: false,
                                response: serde_json::Value::Null,
                                error: Some(e),
                            };
                        }
                        let result = self.callback.handle(&cb_payload);
                        HandleResult {
                            success: result.accepted,
                            response: serde_json::json!({
                                "task_id": result.task_id,
                                "accepted": result.accepted,
                            }),
                            error: result.error,
                        }
                    }
                    Err(e) => HandleResult {
                        success: false,
                        response: serde_json::Value::Null,
                        error: Some(format!("Invalid callback payload: {}", e)),
                    },
                }
            }

            Action::ForgeShare | Action::ForgeGetReflections => {
                self.forge.handle(action_str, payload)
            }

            Action::LlmProxy => {
                self.llm.handle(payload)
            }

            Action::Ping => HandleResult {
                success: true,
                response: serde_json::json!({"status": "ok", "node_id": self.node_id}),
                error: None,
            },

            Action::Status => HandleResult {
                success: true,
                response: serde_json::json!({
                    "node_id": self.node_id,
                    "status": "online",
                }),
                error: None,
            },

            Action::PeerChat => HandleResult {
                success: true,
                response: serde_json::json!({"status": "accepted"}),
                error: None,
            },

            Action::GetCapabilities => {
                let caps = if let Some(ref info) = self.node_info {
                    info.get_capabilities()
                } else {
                    vec!["peer_chat".to_string(), "forge_share".to_string(), "forge_get_reflections".to_string()]
                };
                HandleResult {
                    success: true,
                    response: serde_json::json!({
                        "node_id": self.node_id,
                        "capabilities": caps,
                    }),
                    error: None,
                }
            }

            Action::GetInfo => {
                let info = if let Some(ref provider) = self.node_info {
                    provider.get_info()
                } else {
                    serde_json::json!({
                        "node_id": self.node_id,
                        "status": "online",
                    })
                };
                HandleResult {
                    success: true,
                    response: info,
                    error: None,
                }
            }

            Action::ListActions => {
                let schemas = actions_schema::builtin_schemas();
                let actions: Vec<serde_json::Value> = schemas.iter().map(|s| {
                    serde_json::json!({
                        "action": s.action.to_string(),
                        "description": s.description,
                    })
                }).collect();
                HandleResult {
                    success: true,
                    response: serde_json::json!({
                        "node_id": self.node_id,
                        "actions": actions,
                    }),
                    error: None,
                }
            }

            Action::QueryTaskResult => {
                // Return basic acknowledgment. Actual task result lookup
                // would be wired through TaskResultStore at the application layer.
                let task_id = payload.get("task_id").and_then(|v| v.as_str()).unwrap_or("");
                HandleResult {
                    success: true,
                    response: serde_json::json!({
                        "task_id": task_id,
                        "status": "completed",
                    }),
                    error: None,
                }
            }

            Action::ConfirmTaskDelivery => {
                let task_id = payload.get("task_id").and_then(|v| v.as_str()).unwrap_or("");
                tracing::info!(task_id = task_id, "Task delivery confirmed");
                HandleResult {
                    success: true,
                    response: serde_json::json!({
                        "task_id": task_id,
                        "confirmed": true,
                    }),
                    error: None,
                }
            }

            Action::Custom => {
                if self.custom.has_handler(action_str) {
                    match self.custom.execute(action_str, payload) {
                        Ok(result) => HandleResult {
                            success: true,
                            response: result,
                            error: None,
                        },
                        Err(e) => HandleResult {
                            success: false,
                            response: serde_json::Value::Null,
                            error: Some(e),
                        },
                    }
                } else {
                    HandleResult {
                        success: false,
                        response: serde_json::Value::Null,
                        error: Some(format!("Unknown action: {}", action_str)),
                    }
                }
            }
        }
    }

    /// Get a reference to the custom handler for registration.
    pub fn custom_handler(&self) -> &CustomHandler {
        &self.custom
    }
}

#[cfg(test)]
mod tests {
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
}

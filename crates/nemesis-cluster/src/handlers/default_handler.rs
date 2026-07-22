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
                match serde_json::from_value::<crate::handlers::callback::CallbackPayload>(payload)
                {
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

            Action::LlmProxy => self.llm.handle(payload),

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
                    vec![
                        "peer_chat".to_string(),
                        "forge_share".to_string(),
                        "forge_get_reflections".to_string(),
                    ]
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
                let actions: Vec<serde_json::Value> = schemas
                    .iter()
                    .map(|s| {
                        serde_json::json!({
                            "action": s.action.to_string(),
                            "description": s.description,
                        })
                    })
                    .collect();
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
                let task_id = payload
                    .get("task_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
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
                let task_id = payload
                    .get("task_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                tracing::info!(
                    task_id = task_id,
                    "[DefaultHandler] Task delivery confirmed"
                );
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
mod tests;

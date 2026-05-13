//! Callback handler - processes peer_chat_callback responses.
//!
//! When a remote node completes a peer_chat request, it sends back a callback.
//! This handler processes the callback, completing the associated task and
//! triggering the continuation flow.

use serde::{Deserialize, Serialize};

/// Interface for completing tasks when a callback is received.
///
/// Mirrors Go's `TaskCompleter` interface. The Cluster implements this
/// to trigger `handleTaskComplete()` which publishes the continuation message.
pub trait TaskCompleter: Send + Sync {
    /// Complete a task with the given result.
    fn complete_task(&self, task_id: &str, response: &str, success: bool, error: Option<&str>);
}

/// Callback payload for peer_chat responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallbackPayload {
    /// The original task ID.
    pub task_id: String,
    /// The LLM response content.
    pub response: String,
    /// Whether the processing succeeded.
    pub success: bool,
    /// Error message if failed.
    #[serde(default)]
    pub error: Option<String>,
}

/// Handler for peer_chat_callback actions.
pub struct CallbackHandler {
    node_id: String,
    task_completer: Option<Box<dyn TaskCompleter>>,
}

impl CallbackHandler {
    /// Create a new callback handler.
    pub fn new(node_id: String) -> Self {
        Self {
            node_id,
            task_completer: None,
        }
    }

    /// Create a new callback handler with a task completer.
    ///
    /// When a completer is set, `handle()` will actually complete the task
    /// in the TaskManager, triggering the continuation flow.
    /// Mirrors Go's `RegisterCallbackHandler()` with Cluster as TaskCompleter.
    pub fn with_completer(node_id: String, completer: Box<dyn TaskCompleter>) -> Self {
        Self {
            node_id,
            task_completer: Some(completer),
        }
    }

    /// Handle an incoming callback.
    ///
    /// Returns the task ID and whether the callback was successfully processed.
    /// If a `TaskCompleter` is set, also completes the task to trigger continuation.
    pub fn handle(&self, payload: &CallbackPayload) -> CallbackResult {
        tracing::info!(
            task_id = %payload.task_id,
            success = payload.success,
            node_id = %self.node_id,
            "Processing peer_chat callback"
        );

        if payload.success {
            // Complete the task if a completer is available.
            if let Some(ref completer) = self.task_completer {
                completer.complete_task(&payload.task_id, &payload.response, true, None);
            }

            CallbackResult {
                task_id: payload.task_id.clone(),
                accepted: true,
                error: None,
            }
        } else {
            tracing::warn!(
                task_id = %payload.task_id,
                error = ?payload.error,
                "Remote peer_chat failed"
            );

            // Mark task as failed if a completer is available.
            if let Some(ref completer) = self.task_completer {
                let err_msg = payload.error.as_deref().unwrap_or("unknown error");
                completer.complete_task(&payload.task_id, "", false, Some(err_msg));
            }

            CallbackResult {
                task_id: payload.task_id.clone(),
                accepted: true,
                error: payload.error.clone(),
            }
        }
    }

    /// Validate the callback payload.
    pub fn validate(&self, payload: &CallbackPayload) -> Result<(), String> {
        if payload.task_id.is_empty() {
            return Err("task_id is required".into());
        }
        if payload.success && payload.response.is_empty() {
            return Err("response is required for successful callbacks".into());
        }
        if !payload.success && payload.error.is_none() {
            return Err("error is required for failed callbacks".into());
        }
        Ok(())
    }
}

/// Result of processing a callback.
#[derive(Debug, Clone)]
pub struct CallbackResult {
    /// The task ID that was completed.
    pub task_id: String,
    /// Whether the callback was accepted.
    pub accepted: bool,
    /// Error message if processing failed.
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handle_success_callback() {
        let handler = CallbackHandler::new("node-a".into());
        let payload = CallbackPayload {
            task_id: "task-001".into(),
            response: "Hello from remote".into(),
            success: true,
            error: None,
        };

        let result = handler.handle(&payload);
        assert!(result.accepted);
        assert!(result.error.is_none());
    }

    #[test]
    fn test_handle_failure_callback() {
        let handler = CallbackHandler::new("node-a".into());
        let payload = CallbackPayload {
            task_id: "task-002".into(),
            response: String::new(),
            success: false,
            error: Some("timeout".into()),
        };

        let result = handler.handle(&payload);
        assert!(result.accepted);
        assert_eq!(result.error.as_deref(), Some("timeout"));
    }

    #[test]
    fn test_validate_payload() {
        let handler = CallbackHandler::new("node-a".into());

        // Valid success
        let valid = CallbackPayload {
            task_id: "t1".into(),
            response: "ok".into(),
            success: true,
            error: None,
        };
        assert!(handler.validate(&valid).is_ok());

        // Invalid: empty task_id
        let invalid = CallbackPayload {
            task_id: String::new(),
            response: "ok".into(),
            success: true,
            error: None,
        };
        assert!(handler.validate(&invalid).is_err());

        // Invalid: success but empty response
        let invalid2 = CallbackPayload {
            task_id: "t1".into(),
            response: String::new(),
            success: true,
            error: None,
        };
        assert!(handler.validate(&invalid2).is_err());
    }

    // -- Additional tests --

    #[test]
    fn test_callback_payload_serialization_roundtrip() {
        let payload = CallbackPayload {
            task_id: "task-123".into(),
            response: "result data".into(),
            success: true,
            error: None,
        };
        let json = serde_json::to_string(&payload).unwrap();
        let back: CallbackPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(back.task_id, "task-123");
        assert_eq!(back.response, "result data");
        assert!(back.success);
        assert!(back.error.is_none());
    }

    #[test]
    fn test_callback_payload_serialization_with_error() {
        let payload = CallbackPayload {
            task_id: "task-456".into(),
            response: String::new(),
            success: false,
            error: Some("connection refused".into()),
        };
        let json = serde_json::to_string(&payload).unwrap();
        let back: CallbackPayload = serde_json::from_str(&json).unwrap();
        assert!(!back.success);
        assert_eq!(back.error.unwrap(), "connection refused");
    }

    #[test]
    fn test_validate_failure_with_no_error() {
        let handler = CallbackHandler::new("node-a".into());
        let payload = CallbackPayload {
            task_id: "task-789".into(),
            response: String::new(),
            success: false,
            error: None,
        };
        // Failure with no error message should fail validation
        assert!(handler.validate(&payload).is_err());
    }

    #[test]
    fn test_with_completer_constructor() {
        use crate::handlers::callback::TaskCompleter;

        struct MockCompleter;
        impl TaskCompleter for MockCompleter {
            fn complete_task(&self, _task_id: &str, _response: &str, _success: bool, _error: Option<&str>) {
            }
        }

        let handler = CallbackHandler::with_completer(
            "node-b".into(),
            Box::new(MockCompleter),
        );
        // Just ensure it was constructed without panicking
        let payload = CallbackPayload {
            task_id: "task-1".into(),
            response: "ok".into(),
            success: true,
            error: None,
        };
        let result = handler.handle(&payload);
        assert!(result.accepted);
    }
}

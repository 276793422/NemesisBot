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
            "[CallbackHandler] Processing peer_chat callback"
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
                "[CallbackHandler] Remote peer_chat failed"
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
mod tests;

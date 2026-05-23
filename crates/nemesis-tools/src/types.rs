//! Tool result types.

use serde::{Deserialize, Serialize};

/// Result of a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// Text to send back to the LLM.
    pub for_llm: String,
    /// Text to show to the user (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub for_user: Option<String>,
    /// Whether this result should be silent (not shown to user).
    #[serde(default)]
    pub silent: bool,
    /// Whether this is an error result.
    #[serde(default)]
    pub is_error: bool,
    /// Whether this is an async result (task ID pending).
    #[serde(default)]
    pub is_async: bool,
    /// Task ID for async operations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

impl ToolResult {
    /// Create a successful result.
    pub fn success(for_llm: &str) -> Self {
        Self {
            for_llm: for_llm.to_string(),
            for_user: None,
            silent: false,
            is_error: false,
            is_async: false,
            task_id: None,
        }
    }

    /// Create an error result.
    pub fn error(for_llm: &str) -> Self {
        Self {
            for_llm: for_llm.to_string(),
            for_user: None,
            silent: false,
            is_error: true,
            is_async: false,
            task_id: None,
        }
    }

    /// Create an async result.
    pub fn async_result(task_id: &str) -> Self {
        Self {
            for_llm: format!("Task started with ID: {}", task_id),
            for_user: None,
            silent: false,
            is_error: false,
            is_async: true,
            task_id: Some(task_id.to_string()),
        }
    }

    /// Create a silent result (not shown to user).
    pub fn silent(for_llm: &str) -> Self {
        Self {
            for_llm: for_llm.to_string(),
            for_user: None,
            silent: true,
            is_error: false,
            is_async: false,
            task_id: None,
        }
    }

    /// Create a result that sends different content to the LLM and the user.
    ///
    /// Equivalent to Go's `UserResult()`.
    pub fn user_result(for_llm: &str, for_user: &str) -> Self {
        Self {
            for_llm: for_llm.to_string(),
            for_user: Some(for_user.to_string()),
            silent: false,
            is_error: false,
            is_async: false,
            task_id: None,
        }
    }

    /// Chain an error onto this result, preserving the original content.
    ///
    /// Equivalent to Go's `WithError()`. Returns a new error result with
    /// the chained error message.
    pub fn with_error(&self, err: &str) -> Self {
        let chained = if self.is_error {
            format!("{}; {}", self.for_llm, err)
        } else {
            format!("{} [error: {}]", self.for_llm, err)
        };
        Self {
            for_llm: chained,
            for_user: self.for_user.clone(),
            silent: self.silent,
            is_error: true,
            is_async: self.is_async,
            task_id: self.task_id.clone(),
        }
    }
}

/// Tool definition for registration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[cfg(test)]
mod tests;

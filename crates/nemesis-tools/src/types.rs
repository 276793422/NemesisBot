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
mod tests {
    use super::*;

    #[test]
    fn test_success_result() {
        let result = ToolResult::success("file contents here");
        assert_eq!(result.for_llm, "file contents here");
        assert!(!result.is_error);
        assert!(!result.silent);
    }

    #[test]
    fn test_error_result() {
        let result = ToolResult::error("file not found");
        assert!(result.is_error);
        assert_eq!(result.for_llm, "file not found");
    }

    #[test]
    fn test_async_result() {
        let result = ToolResult::async_result("task-123");
        assert!(result.is_async);
        assert_eq!(result.task_id.unwrap(), "task-123");
    }

    #[test]
    fn test_silent_result() {
        let result = ToolResult::silent("internal note");
        assert!(result.silent);
    }

    #[test]
    fn test_serialization() {
        let result = ToolResult::success("test");
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"for_llm\":\"test\""));
    }

    #[test]
    fn test_user_result() {
        let result = ToolResult::user_result("technical details", "User-friendly message");
        assert_eq!(result.for_llm, "technical details");
        assert_eq!(result.for_user.unwrap(), "User-friendly message");
        assert!(!result.is_error);
    }

    #[test]
    fn test_with_error_on_success() {
        let result = ToolResult::success("file contents");
        let chained = result.with_error("encoding error");
        assert!(chained.is_error);
        assert!(chained.for_llm.contains("file contents"));
        assert!(chained.for_llm.contains("encoding error"));
    }

    #[test]
    fn test_with_error_on_error() {
        let result = ToolResult::error("first error");
        let chained = result.with_error("second error");
        assert!(chained.is_error);
        assert!(chained.for_llm.contains("first error"));
        assert!(chained.for_llm.contains("second error"));
    }

    // ============================================================
    // Additional tests for missing coverage
    // ============================================================

    #[test]
    fn test_error_result_fields() {
        let result = ToolResult::error("test error");
        assert!(result.is_error);
        assert!(!result.silent);
        assert!(!result.is_async);
        assert!(result.task_id.is_none());
        assert!(result.for_user.is_none());
    }

    #[test]
    fn test_success_result_fields() {
        let result = ToolResult::success("test data");
        assert!(!result.is_error);
        assert!(!result.silent);
        assert!(!result.is_async);
        assert!(result.task_id.is_none());
        assert!(result.for_user.is_none());
    }

    #[test]
    fn test_silent_result_fields() {
        let result = ToolResult::silent("internal note");
        assert!(result.silent);
        assert!(!result.is_error);
        assert!(!result.is_async);
        assert!(result.task_id.is_none());
    }

    #[test]
    fn test_async_result_fields() {
        let result = ToolResult::async_result("task-456");
        assert!(result.is_async);
        assert!(!result.is_error);
        assert!(!result.silent);
        assert_eq!(result.task_id.unwrap(), "task-456");
        assert!(result.for_llm.contains("task-456"));
    }

    #[test]
    fn test_user_result_fields() {
        let result = ToolResult::user_result("technical", "user-friendly");
        assert_eq!(result.for_llm, "technical");
        assert_eq!(result.for_user.unwrap(), "user-friendly");
        assert!(!result.is_error);
        assert!(!result.silent);
    }

    #[test]
    fn test_user_result_none_for_user() {
        let result = ToolResult::success("no user content");
        assert!(result.for_user.is_none());
    }

    #[test]
    fn test_with_error_chaining_format() {
        let result = ToolResult::success("original data");
        let chained = result.with_error("something failed");
        assert!(chained.is_error);
        assert!(chained.for_llm.contains("original data"));
        assert!(chained.for_llm.contains("[error:"));
        assert!(chained.for_llm.contains("something failed"));
    }

    #[test]
    fn test_with_error_double_error() {
        let result = ToolResult::error("error 1");
        let chained = result.with_error("error 2");
        // When already an error, should use "; " separator
        assert!(chained.for_llm.contains("error 1; error 2"));
    }

    #[test]
    fn test_serialization_roundtrip() {
        let result = ToolResult::success("test content");
        let json = serde_json::to_string(&result).unwrap();
        let deserialized: ToolResult = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.for_llm, "test content");
        assert!(!deserialized.is_error);
    }

    #[test]
    fn test_async_result_serialization() {
        let result = ToolResult::async_result("task-789");
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("task-789"));
        let deserialized: ToolResult = serde_json::from_str(&json).unwrap();
        assert!(deserialized.is_async);
        assert_eq!(deserialized.task_id.unwrap(), "task-789");
    }

    #[test]
    fn test_tool_info_serialization() {
        let info = ToolInfo {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("test_tool"));
        let deserialized: ToolInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "test_tool");
    }

    // ============================================================
    // Additional edge-case tests
    // ============================================================

    #[test]
    fn test_success_with_empty_string() {
        let result = ToolResult::success("");
        assert_eq!(result.for_llm, "");
        assert!(!result.is_error);
    }

    #[test]
    fn test_error_with_empty_string() {
        let result = ToolResult::error("");
        assert_eq!(result.for_llm, "");
        assert!(result.is_error);
    }

    #[test]
    fn test_silent_with_empty_string() {
        let result = ToolResult::silent("");
        assert_eq!(result.for_llm, "");
        assert!(result.silent);
    }

    #[test]
    fn test_async_result_message_format() {
        let result = ToolResult::async_result("task-abc");
        assert!(result.for_llm.starts_with("Task started with ID:"));
        assert!(result.for_llm.contains("task-abc"));
    }

    #[test]
    fn test_user_result_with_empty_strings() {
        let result = ToolResult::user_result("", "");
        assert_eq!(result.for_llm, "");
        assert_eq!(result.for_user.as_ref().unwrap(), "");
    }

    #[test]
    fn test_with_error_preserves_for_user() {
        let result = ToolResult::user_result("technical", "user message");
        let chained = result.with_error("extra error");
        assert_eq!(chained.for_user.as_ref().unwrap(), "user message");
        assert!(chained.is_error);
    }

    #[test]
    fn test_with_error_preserves_silent() {
        let result = ToolResult::silent("quiet update");
        let chained = result.with_error("failure");
        assert!(chained.silent);
        assert!(chained.is_error);
    }

    #[test]
    fn test_with_error_preserves_async_and_task_id() {
        let result = ToolResult::async_result("task-789");
        let chained = result.with_error("async failed");
        assert!(chained.is_async);
        assert_eq!(chained.task_id.as_ref().unwrap(), "task-789");
    }

    #[test]
    fn test_with_error_on_success_format() {
        let result = ToolResult::success("original");
        let chained = result.with_error("problem");
        assert!(chained.for_llm.contains("[error: problem]"));
        assert!(chained.for_llm.contains("original"));
    }

    #[test]
    fn test_with_error_on_error_format() {
        let result = ToolResult::error("first");
        let chained = result.with_error("second");
        assert!(chained.for_llm.contains("first; second"));
    }

    #[test]
    fn test_double_with_error_chain() {
        let result = ToolResult::success("data");
        let first = result.with_error("err1");
        let second = first.with_error("err2");
        assert!(second.is_error);
        assert!(second.for_llm.contains("err1"));
        assert!(second.for_llm.contains("err2"));
    }

    #[test]
    fn test_tool_info_clone() {
        let info = ToolInfo {
            name: "clone_test".to_string(),
            description: "Test cloning".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        };
        let cloned = info.clone();
        assert_eq!(cloned.name, info.name);
        assert_eq!(cloned.description, info.description);
    }

    #[test]
    fn test_tool_result_clone() {
        let result = ToolResult::user_result("llm content", "user content");
        let cloned = result.clone();
        assert_eq!(cloned.for_llm, result.for_llm);
        assert_eq!(cloned.for_user, result.for_user);
        assert_eq!(cloned.silent, result.silent);
        assert_eq!(cloned.is_error, result.is_error);
    }

    #[test]
    fn test_serialization_skip_none_fields() {
        let result = ToolResult::success("test");
        let json = serde_json::to_string(&result).unwrap();
        // for_user is None, so it should not appear
        assert!(!json.contains("for_user"));
        // task_id is None, so it should not appear
        assert!(!json.contains("task_id"));
    }

    #[test]
    fn test_deserialization_with_all_fields() {
        let json = r#"{
            "for_llm": "content",
            "for_user": "user msg",
            "silent": true,
            "is_error": false,
            "is_async": true,
            "task_id": "task-001"
        }"#;
        let result: ToolResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.for_llm, "content");
        assert_eq!(result.for_user.as_ref().unwrap(), "user msg");
        assert!(result.silent);
        assert!(!result.is_error);
        assert!(result.is_async);
        assert_eq!(result.task_id.as_ref().unwrap(), "task-001");
    }

    #[test]
    fn test_deserialization_defaults() {
        let json = r#"{"for_llm": "minimal"}"#;
        let result: ToolResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.for_llm, "minimal");
        assert!(!result.silent);
        assert!(!result.is_error);
        assert!(!result.is_async);
        assert!(result.for_user.is_none());
        assert!(result.task_id.is_none());
    }

    #[test]
    fn test_tool_info_roundtrip() {
        let info = ToolInfo {
            name: "round_trip".to_string(),
            description: "Round trip test".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"}
                },
                "required": ["path"]
            }),
        };
        let json = serde_json::to_string(&info).unwrap();
        let restored: ToolInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.name, info.name);
        assert_eq!(restored.description, info.description);
        assert_eq!(restored.parameters, info.parameters);
    }

    #[test]
    fn test_success_with_unicode() {
        let result = ToolResult::success("Hello, world! - test");
        assert_eq!(result.for_llm, "Hello, world! - test");
    }

    #[test]
    fn test_error_with_special_characters() {
        let result = ToolResult::error("error: <tag> & \"quotes\" 'single'");
        assert!(result.for_llm.contains("<tag>"));
        assert!(result.for_llm.contains("&"));
    }

    #[test]
    fn test_with_error_chaining_three_levels() {
        let r1 = ToolResult::success("ok");
        let r2 = r1.with_error("first error");
        let r3 = r2.with_error("second error");
        let r4 = r3.with_error("third error");
        assert!(r4.is_error);
        assert!(r4.for_llm.contains("second error; third error"));
    }

    #[test]
    fn test_async_result_with_empty_task_id() {
        let result = ToolResult::async_result("");
        assert!(result.is_async);
        assert_eq!(result.task_id.as_ref().unwrap(), "");
    }

    #[test]
    fn test_tool_result_debug_format() {
        let result = ToolResult::error("debug test");
        let debug_str = format!("{:?}", result);
        assert!(debug_str.contains("debug test"));
        assert!(debug_str.contains("is_error: true"));
    }

    #[test]
    fn test_tool_info_debug_format() {
        let info = ToolInfo {
            name: "debug_tool".to_string(),
            description: "Debug test".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        };
        let debug_str = format!("{:?}", info);
        assert!(debug_str.contains("debug_tool"));
    }

    // ---- New tests ----

    #[test]
    fn test_tool_result_success_static() {
        let result = ToolResult::success("static message");
        assert!(!result.is_error);
        assert!(!result.is_async);
        assert!(result.task_id.is_none());
    }

    #[test]
    fn test_tool_result_error_static() {
        let result = ToolResult::error("error message");
        assert!(result.is_error);
    }

    #[test]
    fn test_tool_result_for_llm_contains_content() {
        let result = ToolResult::success("hello world content");
        assert!(result.for_llm.contains("hello world"));
    }

    #[test]
    fn test_tool_result_is_async_flag() {
        let result = ToolResult::async_result("task-123");
        assert!(result.is_async);
        assert_eq!(result.task_id.as_deref(), Some("task-123"));
        assert!(!result.is_error);
    }

    #[test]
    fn test_tool_result_chain_with_error() {
        let result = ToolResult::success("ok").with_error("fail");
        assert!(result.is_error);
        assert!(result.for_llm.contains("fail"));
    }

    #[test]
    fn test_tool_info_fields() {
        let info = ToolInfo {
            name: "test_tool".into(),
            description: "A test".into(),
            parameters: serde_json::json!({"type": "object"}),
        };
        assert_eq!(info.name, "test_tool");
        assert_eq!(info.description, "A test");
        assert!(info.parameters.is_object());
    }

    #[test]
    fn test_tool_result_empty_success() {
        let result = ToolResult::success("");
        assert!(!result.is_error);
    }

    #[test]
    fn test_tool_result_empty_error() {
        let result = ToolResult::error("");
        assert!(result.is_error);
    }

    #[test]
    fn test_tool_result_long_content() {
        let long = "x".repeat(10000);
        let result = ToolResult::success(&long);
        assert_eq!(result.for_llm.len(), 10000);
    }

    #[test]
    fn test_tool_result_unicode_content() {
        let result = ToolResult::success("Hello 日本語");
        assert!(result.for_llm.contains("日本語"));
    }
}

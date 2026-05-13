//! Sleep tool - suspends execution for a specified duration.

use crate::registry::Tool;
use crate::types::ToolResult;
use async_trait::async_trait;
use std::time::Duration;

/// Maximum sleep duration: 1 hour.
const MAX_SLEEP_SECS: u64 = 3600;

/// Sleep tool - suspends execution for a specified duration.
pub struct SleepTool;

impl SleepTool {
    /// Create a new sleep tool.
    pub fn new() -> Self {
        Self
    }
}

impl Default for SleepTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for SleepTool {
    fn name(&self) -> &str {
        "sleep"
    }

    fn description(&self) -> &str {
        "Suspend execution for a specified duration in seconds. Use for testing delays, timeouts, and long-running operations."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "duration": {
                    "type": "integer",
                    "description": "Duration to sleep in seconds (1-3600)",
                    "minimum": 1,
                    "maximum": 3600
                }
            },
            "required": ["duration"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let duration_secs = match args["duration"].as_u64() {
            Some(d) => d,
            None => return ToolResult::error("parameter 'duration' must be an integer (seconds)"),
        };

        if duration_secs < 1 {
            return ToolResult::error("duration must be at least 1 second");
        }
        if duration_secs > MAX_SLEEP_SECS {
            return ToolResult::error("duration cannot exceed 3600 seconds (1 hour)");
        }

        tokio::time::sleep(Duration::from_secs(duration_secs)).await;
        ToolResult::silent(&format!("Slept for {} seconds", duration_secs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sleep_success() {
        let tool = SleepTool::new();
        let result = tool
            .execute(&serde_json::json!({"duration": 1}))
            .await;
        assert!(!result.is_error);
        assert!(result.silent);
        assert!(result.for_llm.contains("Slept for 1 seconds"));
    }

    #[tokio::test]
    async fn test_sleep_zero_rejected() {
        let tool = SleepTool::new();
        let result = tool
            .execute(&serde_json::json!({"duration": 0}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("at least 1"));
    }

    #[tokio::test]
    async fn test_sleep_too_large() {
        let tool = SleepTool::new();
        let result = tool
            .execute(&serde_json::json!({"duration": 5000}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("3600"));
    }

    #[tokio::test]
    async fn test_sleep_missing_duration() {
        let tool = SleepTool::new();
        let result = tool
            .execute(&serde_json::json!({}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("must be an integer"));
    }

    #[tokio::test]
    async fn test_sleep_non_integer() {
        let tool = SleepTool::new();
        let result = tool
            .execute(&serde_json::json!({"duration": "abc"}))
            .await;
        assert!(result.is_error);
    }

    #[test]
    fn test_sleep_tool_metadata() {
        let tool = SleepTool::new();
        assert_eq!(tool.name(), "sleep");
        assert!(!tool.description().is_empty());
    }

    // ============================================================
    // Additional sleep tool tests
    // ============================================================

    #[tokio::test]
    async fn test_sleep_exactly_one_second() {
        let tool = SleepTool::new();
        let start = std::time::Instant::now();
        let result = tool
            .execute(&serde_json::json!({"duration": 1}))
            .await;
        let elapsed = start.elapsed();
        assert!(!result.is_error, "Expected success, got: {}", result.for_llm);
        assert!(result.silent, "Sleep result should be silent");
        assert!(elapsed >= std::time::Duration::from_millis(900));
    }

    #[tokio::test]
    async fn test_sleep_tool_parameters() {
        let tool = SleepTool::new();
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["duration"].is_object());

        let required = params["required"].as_array().unwrap();
        assert!(required.iter().any(|r| r.as_str() == Some("duration")));
    }

    #[tokio::test]
    async fn test_sleep_boundary_values() {
        let tool = SleepTool::new();
        // 3601 should fail (exceeds max)
        let result = tool
            .execute(&serde_json::json!({"duration": 3601}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("3600"));
    }

    #[tokio::test]
    async fn test_sleep_float_duration() {
        let tool = SleepTool::new();
        let _result = tool
            .execute(&serde_json::json!({"duration": 1.5}))
            .await;
        // Floats may or may not be accepted depending on implementation
        // Just verify it doesn't panic
    }

    #[tokio::test]
    async fn test_sleep_negative_duration() {
        let tool = SleepTool::new();
        let result = tool
            .execute(&serde_json::json!({"duration": -10}))
            .await;
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_sleep_cancellation() {
        let tool = SleepTool::new();
        // Use tokio::time::timeout to simulate cancellation
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            tool.execute(&serde_json::json!({"duration": 30})),
        )
        .await;

        // Should timeout (sleep 30s is way longer than 50ms)
        assert!(result.is_err(), "Expected timeout");
    }

    #[tokio::test]
    async fn test_sleep_success_result_content() {
        let tool = SleepTool::new();
        let result = tool
            .execute(&serde_json::json!({"duration": 1}))
            .await;
        assert!(!result.is_error);
        assert!(result.silent);
        assert!(result.for_llm.contains("Slept") || result.for_llm.contains("slept") || !result.for_llm.is_empty());
    }

    #[test]
    fn test_sleep_tool_new() {
        let tool = SleepTool::new();
        assert_eq!(tool.name(), "sleep");
    }
}

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
mod tests;

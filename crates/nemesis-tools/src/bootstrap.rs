//! Bootstrap completion tool - deletes BOOTSTRAP.md after initialization.

use crate::registry::Tool;
use crate::types::ToolResult;
use async_trait::async_trait;
use std::path::PathBuf;

/// Bootstrap completion tool - completes the bootstrap initialization process.
pub struct CompleteBootstrapTool {
    workspace: PathBuf,
}

impl CompleteBootstrapTool {
    /// Create a new bootstrap completion tool.
    pub fn new(workspace: &str) -> Self {
        Self {
            workspace: PathBuf::from(workspace),
        }
    }
}

#[async_trait]
impl Tool for CompleteBootstrapTool {
    fn name(&self) -> &str {
        "complete_bootstrap"
    }

    fn description(&self) -> &str {
        "Complete the bootstrap initialization by deleting BOOTSTRAP.md. Must confirm all initialization steps are done first."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "confirmed": {
                    "type": "boolean",
                    "description": "Confirm that initialization is complete and ready to delete BOOTSTRAP.md"
                }
            },
            "required": ["confirmed"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let confirmed = match args["confirmed"].as_bool() {
            Some(c) => c,
            None => return ToolResult::error("confirmed parameter must be a boolean"),
        };

        if !confirmed {
            return ToolResult::error(
                "Must confirm initialization is complete before deleting bootstrap file.",
            );
        }

        let bootstrap_path = self.workspace.join("BOOTSTRAP.md");

        // Check if file exists
        if !tokio::fs::metadata(&bootstrap_path).await.is_ok() {
            return ToolResult::success(
                "BOOTSTRAP.md has already been removed. Initialization is complete.",
            );
        }

        // Delete the file
        match tokio::fs::remove_file(&bootstrap_path).await {
            Ok(()) => ToolResult::success(
                "Bootstrap initialization complete! BOOTSTRAP.md has been deleted. The system will load configuration files on next startup.",
            ),
            Err(e) => ToolResult::error(&format!("Failed to delete BOOTSTRAP.md: {}", e)),
        }
    }
}

#[cfg(test)]
mod tests;

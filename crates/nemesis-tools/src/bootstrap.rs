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
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_bootstrap_not_confirmed() {
        let dir = TempDir::new().unwrap();
        let tool = CompleteBootstrapTool::new(&dir.path().to_string_lossy());

        let result = tool
            .execute(&serde_json::json!({"confirmed": false}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("confirm"));
    }

    #[tokio::test]
    async fn test_bootstrap_missing_param() {
        let dir = TempDir::new().unwrap();
        let tool = CompleteBootstrapTool::new(&dir.path().to_string_lossy());

        let result = tool.execute(&serde_json::json!({})).await;
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_bootstrap_file_not_found() {
        let dir = TempDir::new().unwrap();
        let tool = CompleteBootstrapTool::new(&dir.path().to_string_lossy());

        // No BOOTSTRAP.md file exists
        let result = tool
            .execute(&serde_json::json!({"confirmed": true}))
            .await;
        assert!(!result.is_error);
        assert!(result.for_llm.contains("already been removed"));
    }

    #[tokio::test]
    async fn test_bootstrap_success() {
        let dir = TempDir::new().unwrap();
        let bootstrap_path = dir.path().join("BOOTSTRAP.md");
        tokio::fs::write(&bootstrap_path, "# Bootstrap").await.unwrap();

        let tool = CompleteBootstrapTool::new(&dir.path().to_string_lossy());

        let result = tool
            .execute(&serde_json::json!({"confirmed": true}))
            .await;
        assert!(!result.is_error, "Expected success, got: {}", result.for_llm);
        assert!(result.for_llm.contains("complete"));

        // Verify file was deleted
        assert!(
            !tokio::fs::metadata(&bootstrap_path).await.is_ok(),
            "BOOTSTRAP.md should be deleted"
        );
    }

    #[tokio::test]
    async fn test_bootstrap_non_boolean_confirmed() {
        let dir = TempDir::new().unwrap();
        let tool = CompleteBootstrapTool::new(&dir.path().to_string_lossy());

        let result = tool
            .execute(&serde_json::json!({"confirmed": "yes"}))
            .await;
        assert!(result.is_error);
    }

    #[test]
    fn test_bootstrap_tool_metadata() {
        let dir = TempDir::new().unwrap();
        let tool = CompleteBootstrapTool::new(&dir.path().to_string_lossy());
        assert_eq!(tool.name(), "complete_bootstrap");
        assert!(!tool.description().is_empty());
    }

    // ---- New tests ----

    #[test]
    fn test_parameters_returns_valid_json() {
        let dir = TempDir::new().unwrap();
        let tool = CompleteBootstrapTool::new(&dir.path().to_string_lossy());
        let params = tool.parameters();
        assert!(params.is_object());
        assert!(params["properties"]["confirmed"].is_object());
    }

    #[test]
    fn test_new_stores_workspace() {
        let tool = CompleteBootstrapTool::new("/test/workspace");
        assert_eq!(tool.workspace, PathBuf::from("/test/workspace"));
    }

    #[tokio::test]
    async fn test_bootstrap_confirmed_number_instead_of_bool() {
        let dir = TempDir::new().unwrap();
        let tool = CompleteBootstrapTool::new(&dir.path().to_string_lossy());
        let result = tool
            .execute(&serde_json::json!({"confirmed": 1}))
            .await;
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_bootstrap_confirmed_null() {
        let dir = TempDir::new().unwrap();
        let tool = CompleteBootstrapTool::new(&dir.path().to_string_lossy());
        let result = tool
            .execute(&serde_json::json!({"confirmed": null}))
            .await;
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_bootstrap_double_execution() {
        let dir = TempDir::new().unwrap();
        let bootstrap_path = dir.path().join("BOOTSTRAP.md");
        tokio::fs::write(&bootstrap_path, "# Bootstrap").await.unwrap();

        let tool = CompleteBootstrapTool::new(&dir.path().to_string_lossy());

        // First execution deletes the file
        let result = tool
            .execute(&serde_json::json!({"confirmed": true}))
            .await;
        assert!(!result.is_error);

        // Second execution reports already removed
        let result2 = tool
            .execute(&serde_json::json!({"confirmed": true}))
            .await;
        assert!(!result2.is_error);
        assert!(result2.for_llm.contains("already been removed"));
    }
}

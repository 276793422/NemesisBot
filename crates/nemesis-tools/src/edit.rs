//! Edit and append file tools.

use crate::registry::Tool;
use crate::types::ToolResult;
use async_trait::async_trait;
use std::path::{Path, PathBuf};

/// Edit file tool - replaces exact text occurrences in a file.
pub struct EditFileTool {
    workspace: PathBuf,
    restrict: bool,
}

impl EditFileTool {
    /// Create a new edit file tool.
    pub fn new(workspace: &str, restrict: bool) -> Self {
        Self {
            workspace: PathBuf::from(workspace),
            restrict,
        }
    }

    fn resolve_path(&self, path: &str) -> Result<PathBuf, String> {
        let target = Path::new(path);
        let resolved = if target.is_absolute() {
            target.to_path_buf()
        } else {
            self.workspace.join(target)
        };

        if self.restrict {
            let ws = self.workspace.to_string_lossy();
            let res = resolved.to_string_lossy();
            if !res.starts_with(ws.as_ref()) {
                return Err(format!("path '{}' is outside workspace", path));
            }
        }

        Ok(resolved)
    }
}

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn description(&self) -> &str {
        "Edit a file by replacing old_text with new_text. The old_text must exist exactly in the file."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "The file path to edit"},
                "old_text": {"type": "string", "description": "The exact text to find and replace"},
                "new_text": {"type": "string", "description": "The text to replace with"}
            },
            "required": ["path", "old_text", "new_text"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let path = match args["path"].as_str() {
            Some(p) => p,
            None => return ToolResult::error("path is required"),
        };
        let old_text = match args["old_text"].as_str() {
            Some(t) => t,
            None => return ToolResult::error("old_text is required"),
        };
        let new_text = match args["new_text"].as_str() {
            Some(t) => t,
            None => return ToolResult::error("new_text is required"),
        };

        let resolved = match self.resolve_path(path) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(&e),
        };

        // Check file exists
        if !tokio::fs::metadata(&resolved).await.is_ok() {
            return ToolResult::error(&format!("file not found: {}", path));
        }

        // Read file
        let content = match tokio::fs::read_to_string(&resolved).await {
            Ok(c) => c,
            Err(e) => return ToolResult::error(&format!("failed to read file: {}", e)),
        };

        // Check old_text exists
        if !content.contains(old_text) {
            return ToolResult::error("old_text not found in file. Make sure it matches exactly");
        }

        // Check uniqueness
        let count = content.matches(old_text).count();
        if count > 1 {
            return ToolResult::error(&format!(
                "old_text appears {} times. Please provide more context to make it unique",
                count
            ));
        }

        // Replace and write
        let new_content = content.replacen(old_text, new_text, 1);
        match tokio::fs::write(&resolved, new_content).await {
            Ok(()) => ToolResult::silent(&format!("File edited: {}", path)),
            Err(e) => ToolResult::error(&format!("failed to write file: {}", e)),
        }
    }
}

/// Append file tool - appends content to the end of a file.
pub struct AppendFileTool {
    workspace: PathBuf,
    restrict: bool,
}

impl AppendFileTool {
    /// Create a new append file tool.
    pub fn new(workspace: &str, restrict: bool) -> Self {
        Self {
            workspace: PathBuf::from(workspace),
            restrict,
        }
    }

    fn resolve_path(&self, path: &str) -> Result<PathBuf, String> {
        let target = Path::new(path);
        let resolved = if target.is_absolute() {
            target.to_path_buf()
        } else {
            self.workspace.join(target)
        };

        if self.restrict {
            let ws = self.workspace.to_string_lossy();
            let res = resolved.to_string_lossy();
            if !res.starts_with(ws.as_ref()) {
                return Err(format!("path '{}' is outside workspace", path));
            }
        }

        Ok(resolved)
    }
}

#[async_trait]
impl Tool for AppendFileTool {
    fn name(&self) -> &str {
        "append_file"
    }

    fn description(&self) -> &str {
        "Append content to the end of a file"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "The file path to append to"},
                "content": {"type": "string", "description": "The content to append"}
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let path = match args["path"].as_str() {
            Some(p) => p,
            None => return ToolResult::error("path is required"),
        };
        let content = match args["content"].as_str() {
            Some(c) => c,
            None => return ToolResult::error("content is required"),
        };

        let resolved = match self.resolve_path(path) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(&e),
        };

        // Create parent directories if needed
        if let Some(parent) = resolved.parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                return ToolResult::error(&format!("failed to create directories: {}", e));
            }
        }

        // Use OpenOptions for append
        use tokio::io::AsyncWriteExt;
        let mut file = match tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&resolved)
            .await
        {
            Ok(f) => f,
            Err(e) => return ToolResult::error(&format!("failed to open file: {}", e)),
        };

        match file.write_all(content.as_bytes()).await {
            Ok(()) => ToolResult::silent(&format!("Appended to {}", path)),
            Err(e) => ToolResult::error(&format!("failed to append to file: {}", e)),
        }
    }
}

#[cfg(test)]
mod tests;

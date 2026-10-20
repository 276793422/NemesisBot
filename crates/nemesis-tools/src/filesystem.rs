//! Filesystem tools - read, write, list, etc.

use crate::registry::Tool;
use crate::types::ToolResult;
use async_trait::async_trait;
use nemesis_path::paths::canonicalize_for_compare;
use std::path::{Path, PathBuf};

/// 工作区路径解析 + restrict 边界校验（本模块工具族单一真相源）。
///
/// 相对路径相对 workspace 解析；绝对路径原样采纳。`restrict` 开启时双侧
/// 统一归一化（nemesis-path 单一真相源）：target 侧 resolve 防 symlink
/// escape；workspace 侧同步 canonicalize，防 Windows 8.3 短名失配（CI
/// runner 的 TEMP 是 C:\Users\RUNNER~1\...，target 经 canonicalize 成长名
/// 后与原始短名串永不相等 → 误拒，2026-09-01）。`Path::starts_with` 为
/// 组件级比较。
fn resolve_workspace_path(workspace: &Path, restrict: bool, path: &str) -> Result<PathBuf, String> {
    let target = Path::new(path);
    let canonical = if target.is_absolute() {
        target.to_path_buf()
    } else {
        workspace.join(target)
    };

    if restrict {
        let resolved = canonicalize_for_compare(&canonical);
        let ws = canonicalize_for_compare(workspace);
        if !resolved.starts_with(&ws) {
            return Err(format!("path '{}' is outside workspace", path));
        }
    }

    Ok(canonical)
}

/// File read tool.
pub struct ReadFileTool {
    workspace: PathBuf,
    restrict: bool,
}

impl ReadFileTool {
    pub fn new(workspace: &str, restrict: bool) -> Self {
        Self {
            workspace: PathBuf::from(workspace),
            restrict,
        }
    }

    fn validate_path(&self, path: &str) -> Result<PathBuf, String> {
        resolve_workspace_path(&self.workspace, self.restrict, path)
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }
    fn description(&self) -> &str {
        "Read the contents of a file"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "File path to read"}
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let path = match args["path"].as_str() {
            Some(p) => p,
            None => return ToolResult::error("missing 'path' argument"),
        };

        match self.validate_path(path) {
            Ok(validated) => match tokio::fs::read_to_string(&validated).await {
                Ok(content) => ToolResult::success(&content),
                Err(e) => ToolResult::error(&format!("failed to read file: {}", e)),
            },
            Err(e) => ToolResult::error(&e),
        }
    }
}

/// File write tool.
pub struct WriteFileTool {
    workspace: PathBuf,
    restrict: bool,
}

impl WriteFileTool {
    pub fn new(workspace: &str, restrict: bool) -> Self {
        Self {
            workspace: PathBuf::from(workspace),
            restrict,
        }
    }

    fn validate_path(&self, path: &str) -> Result<PathBuf, String> {
        resolve_workspace_path(&self.workspace, self.restrict, path)
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }
    fn description(&self) -> &str {
        "Write content to a file"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "File path to write"},
                "content": {"type": "string", "description": "Content to write"}
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let path = match args["path"].as_str() {
            Some(p) => p,
            None => return ToolResult::error("missing 'path' argument"),
        };
        let content = match args["content"].as_str() {
            Some(c) => c,
            None => return ToolResult::error("missing 'content' argument"),
        };

        let canonical = match self.validate_path(path) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(&e),
        };

        // Create parent directories if needed
        if let Some(parent) = canonical.parent()
            && let Err(e) = tokio::fs::create_dir_all(parent).await
        {
            return ToolResult::error(&format!("failed to create directories: {}", e));
        }

        match tokio::fs::write(&canonical, content).await {
            Ok(()) => ToolResult::success(&format!("wrote {} bytes to {}", content.len(), path)),
            Err(e) => ToolResult::error(&format!("failed to write file: {}", e)),
        }
    }
}

/// List directory tool.
pub struct ListDirTool {
    workspace: PathBuf,
    restrict: bool,
}

impl ListDirTool {
    pub fn new(workspace: &str, restrict: bool) -> Self {
        Self {
            workspace: PathBuf::from(workspace),
            restrict,
        }
    }
}

#[async_trait]
impl Tool for ListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }
    fn description(&self) -> &str {
        "List contents of a directory"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Directory path"}
            }
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let path = args["path"].as_str().unwrap_or(".");
        let target = match resolve_workspace_path(&self.workspace, self.restrict, path) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(&e),
        };

        match tokio::fs::read_dir(&target).await {
            Ok(mut entries) => {
                let mut listing = Vec::new();
                while let Ok(Some(entry)) = entries.next_entry().await {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let file_type = entry.file_type().await;
                    let suffix = match file_type {
                        Ok(ft) if ft.is_dir() => "/",
                        _ => "",
                    };
                    listing.push(format!("{}{}", name, suffix));
                }
                listing.sort();
                ToolResult::success(&listing.join("\n"))
            }
            Err(e) => ToolResult::error(&format!("failed to list directory: {}", e)),
        }
    }
}

/// File exists check tool.
pub struct FileExistsTool {
    workspace: PathBuf,
    restrict: bool,
}

impl FileExistsTool {
    pub fn new(workspace: &str, restrict: bool) -> Self {
        Self {
            workspace: PathBuf::from(workspace),
            restrict,
        }
    }
}

#[async_trait]
impl Tool for FileExistsTool {
    fn name(&self) -> &str {
        "file_exists"
    }
    fn description(&self) -> &str {
        "Check if a file or directory exists"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Path to check"}
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let path = match args["path"].as_str() {
            Some(p) => p,
            None => return ToolResult::error("missing 'path' argument"),
        };

        let target = match resolve_workspace_path(&self.workspace, self.restrict, path) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(&e),
        };

        match tokio::fs::metadata(&target).await {
            Ok(meta) => {
                let kind = if meta.is_dir() { "directory" } else { "file" };
                ToolResult::success(&format!("true ({}): {}", kind, path))
            }
            Err(_) => ToolResult::success(&format!("false: {}", path)),
        }
    }
}

/// Create directory tool.
pub struct CreateDirectoryTool {
    workspace: PathBuf,
    restrict: bool,
}

impl CreateDirectoryTool {
    pub fn new(workspace: &str, restrict: bool) -> Self {
        Self {
            workspace: PathBuf::from(workspace),
            restrict,
        }
    }

    fn validate_path(&self, path: &str) -> Result<PathBuf, String> {
        resolve_workspace_path(&self.workspace, self.restrict, path)
    }
}

#[async_trait]
impl Tool for CreateDirectoryTool {
    fn name(&self) -> &str {
        "create_dir"
    }
    fn description(&self) -> &str {
        "Create a directory (and all parent directories)"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Directory path to create"}
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let path = match args["path"].as_str() {
            Some(p) => p,
            None => return ToolResult::error("missing 'path' argument"),
        };

        let canonical = match self.validate_path(path) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(&e),
        };

        match tokio::fs::create_dir_all(&canonical).await {
            Ok(()) => ToolResult::success(&format!("created directory: {}", path)),
            Err(e) => ToolResult::error(&format!("failed to create directory: {}", e)),
        }
    }
}

/// Delete file tool.
pub struct DeleteFileTool {
    workspace: PathBuf,
    restrict: bool,
}

impl DeleteFileTool {
    pub fn new(workspace: &str, restrict: bool) -> Self {
        Self {
            workspace: PathBuf::from(workspace),
            restrict,
        }
    }

    fn validate_path(&self, path: &str) -> Result<PathBuf, String> {
        resolve_workspace_path(&self.workspace, self.restrict, path)
    }
}

#[async_trait]
impl Tool for DeleteFileTool {
    fn name(&self) -> &str {
        "delete_file"
    }
    fn description(&self) -> &str {
        "Delete a file"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "File path to delete"}
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let path = match args["path"].as_str() {
            Some(p) => p,
            None => return ToolResult::error("missing 'path' argument"),
        };

        let canonical = match self.validate_path(path) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(&e),
        };

        match tokio::fs::remove_file(&canonical).await {
            Ok(()) => ToolResult::success(&format!("deleted file: {}", path)),
            Err(e) => ToolResult::error(&format!("failed to delete file: {}", e)),
        }
    }
}

/// Delete directory tool.
///
/// Deletes a directory and all its contents (recursive). If `restrict` is
/// enabled, the target path must be within the workspace.
pub struct DeleteDirTool {
    workspace: PathBuf,
    restrict: bool,
}

impl DeleteDirTool {
    pub fn new(workspace: &str, restrict: bool) -> Self {
        Self {
            workspace: PathBuf::from(workspace),
            restrict,
        }
    }
}

#[async_trait]
impl Tool for DeleteDirTool {
    fn name(&self) -> &str {
        "delete_dir"
    }

    fn description(&self) -> &str {
        "Delete a directory and all its contents"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory path to delete"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let path = match args["path"].as_str() {
            Some(p) => p,
            None => return ToolResult::error("missing 'path' argument"),
        };

        let target = match resolve_workspace_path(&self.workspace, self.restrict, path) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(&e),
        };

        // Check if path exists and is a directory
        match tokio::fs::metadata(&target).await {
            Ok(meta) => {
                if !meta.is_dir() {
                    return ToolResult::error(&format!("'{}' is not a directory", path));
                }
            }
            Err(e) => {
                return ToolResult::error(&format!("failed to access directory '{}': {}", path, e));
            }
        }

        // Remove the directory and all its contents
        match tokio::fs::remove_dir_all(&target).await {
            Ok(()) => ToolResult {
                for_llm: format!("Directory deleted: {}", path),
                for_user: None,
                silent: true,
                is_error: false,
                is_async: false,
                task_id: None,
            },
            Err(e) => ToolResult::error(&format!("failed to delete directory: {}", e)),
        }
    }
}

#[cfg(test)]
mod tests;

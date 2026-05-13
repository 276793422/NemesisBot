//! Filesystem tools - read, write, list, etc.

use crate::registry::Tool;
use crate::types::ToolResult;
use async_trait::async_trait;
use std::path::{Path, PathBuf};

/// Resolve symlinks by walking up from the path until we find an existing ancestor,
/// then canonicalize that part and append the remaining components.
/// Mirrors Go's `resolveExistingAncestor(path)` + `filepath.EvalSymlinks`.
fn resolve_existing_ancestor(path: &Path) -> PathBuf {
    // First try to canonicalize the full path (resolves all symlinks)
    if path.exists() {
        if let Ok(canonical) = std::fs::canonicalize(path) {
            return canonical;
        }
    }

    // Walk up to find the deepest existing ancestor
    let mut ancestors = Vec::new();
    let mut current = path.to_path_buf();
    while !current.exists() {
        if let Some(parent) = current.parent() {
            ancestors.push(current.file_name().map(|n| PathBuf::from(n)));
            current = parent.to_path_buf();
        } else {
            // Reached root without finding existing path
            return path.to_path_buf();
        }
    }

    // Canonicalize the existing ancestor
    let base = match std::fs::canonicalize(&current) {
        Ok(c) => c,
        Err(_) => return path.to_path_buf(),
    };

    // Append non-existing components back
    let mut result = base;
    for component in ancestors.into_iter().rev() {
        if let Some(name) = component {
            result = result.join(name);
        }
    }

    result
}

/// Normalize a path for comparison purposes. Strips the Windows `\\?\` prefix
/// that `canonicalize` may return.
fn normalize_for_comparison(path: &Path) -> String {
    let s = path.to_string_lossy().to_string();
    // On Windows, canonicalize may return \\?\C:\... prefix
    if s.starts_with(r"\\?\") {
        s[4..].to_string()
    } else {
        s
    }
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
        let target = Path::new(path);
        let canonical = if target.is_absolute() {
            target.to_path_buf()
        } else {
            self.workspace.join(target)
        };

        if self.restrict {
            // Resolve symlinks to prevent symlink escape attacks.
            // Mirrors Go's resolveExistingAncestor + filepath.EvalSymlinks.
            let resolved = resolve_existing_ancestor(&canonical);
            let workspace_str = normalize_for_comparison(&self.workspace);
            let resolved_str = normalize_for_comparison(&resolved);
            if !resolved_str.starts_with(&*workspace_str) {
                return Err(format!("path '{}' is outside workspace", path));
            }
        }

        Ok(canonical)
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str { "read_file" }
    fn description(&self) -> &str { "Read the contents of a file" }
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
        let target = Path::new(path);
        let canonical = if target.is_absolute() {
            target.to_path_buf()
        } else {
            self.workspace.join(target)
        };

        if self.restrict {
            let resolved = resolve_existing_ancestor(&canonical);
            let workspace_str = normalize_for_comparison(&self.workspace);
            let resolved_str = normalize_for_comparison(&resolved);
            if !resolved_str.starts_with(&*workspace_str) {
                return Err(format!("access denied: path '{}' is outside the workspace", path));
            }
        }

        Ok(canonical)
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str { "write_file" }
    fn description(&self) -> &str { "Write content to a file" }
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
        if let Some(parent) = canonical.parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                return ToolResult::error(&format!("failed to create directories: {}", e));
            }
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
    #[allow(dead_code)]
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
    fn name(&self) -> &str { "list_dir" }
    fn description(&self) -> &str { "List contents of a directory" }
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
        let target = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            self.workspace.join(path)
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
    #[allow(dead_code)]
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
    fn name(&self) -> &str { "file_exists" }
    fn description(&self) -> &str { "Check if a file or directory exists" }
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

        let target = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            self.workspace.join(path)
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
        let target = Path::new(path);
        let canonical = if target.is_absolute() {
            target.to_path_buf()
        } else {
            self.workspace.join(target)
        };

        if self.restrict {
            let resolved = resolve_existing_ancestor(&canonical);
            let workspace_str = normalize_for_comparison(&self.workspace);
            let resolved_str = normalize_for_comparison(&resolved);
            if !resolved_str.starts_with(&*workspace_str) {
                return Err(format!("access denied: path '{}' is outside the workspace", path));
            }
        }

        Ok(canonical)
    }
}

#[async_trait]
impl Tool for CreateDirectoryTool {
    fn name(&self) -> &str { "create_dir" }
    fn description(&self) -> &str { "Create a directory (and all parent directories)" }
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
        let target = Path::new(path);
        let canonical = if target.is_absolute() {
            target.to_path_buf()
        } else {
            self.workspace.join(target)
        };

        if self.restrict {
            let resolved = resolve_existing_ancestor(&canonical);
            let workspace_str = normalize_for_comparison(&self.workspace);
            let resolved_str = normalize_for_comparison(&resolved);
            if !resolved_str.starts_with(&*workspace_str) {
                return Err(format!("access denied: path '{}' is outside the workspace", path));
            }
        }

        Ok(canonical)
    }
}

#[async_trait]
impl Tool for DeleteFileTool {
    fn name(&self) -> &str { "delete_file" }
    fn description(&self) -> &str { "Delete a file" }
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

        let target = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            self.workspace.join(path)
        };

        // Validate path is within workspace if restricted
        if self.restrict {
            let ws = normalize_for_comparison(&self.workspace);
            let target_str = normalize_for_comparison(&target);
            if !target_str.starts_with(&*ws) {
                return ToolResult::error(&format!(
                    "path '{}' is outside workspace",
                    path
                ));
            }
        }

        // Check if path exists and is a directory
        match tokio::fs::metadata(&target).await {
            Ok(meta) => {
                if !meta.is_dir() {
                    return ToolResult::error(&format!("'{}' is not a directory", path));
                }
            }
            Err(e) => {
                return ToolResult::error(&format!(
                    "failed to access directory '{}': {}",
                    path, e
                ));
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
mod tests {
    use super::*;
    
    use tempfile::TempDir;

    fn make_tools(dir: &TempDir) -> (ReadFileTool, WriteFileTool, ListDirTool) {
        let ws = dir.path().to_string_lossy().to_string();
        (ReadFileTool::new(&ws, false), WriteFileTool::new(&ws, false), ListDirTool::new(&ws, false))
    }

    #[tokio::test]
    async fn test_read_file() {
        let dir = TempDir::new().unwrap();
        let (read_tool, _, _) = make_tools(&dir);
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "hello world").unwrap();

        let result = read_tool
            .execute(&serde_json::json!({"path": file_path.to_string_lossy()}))
            .await;
        assert_eq!(result.for_llm, "hello world");
    }

    #[tokio::test]
    async fn test_read_missing_file() {
        let dir = TempDir::new().unwrap();
        let (read_tool, _, _) = make_tools(&dir);

        let result = read_tool
            .execute(&serde_json::json!({"path": "/nonexistent/file.txt"}))
            .await;
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_write_file() {
        let dir = TempDir::new().unwrap();
        let (_, write_tool, _) = make_tools(&dir);
        let file_path = dir.path().join("output.txt");

        let result = write_tool
            .execute(&serde_json::json!({
                "path": file_path.to_string_lossy(),
                "content": "test content"
            }))
            .await;
        assert!(!result.is_error);

        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "test content");
    }

    #[tokio::test]
    async fn test_list_directory() {
        let dir = TempDir::new().unwrap();
        let (_, _, list_tool) = make_tools(&dir);

        tokio::fs::write(dir.path().join("a.txt"), "a").await.unwrap();
        tokio::fs::create_dir(dir.path().join("subdir")).await.unwrap();

        let result = list_tool
            .execute(&serde_json::json!({"path": dir.path().to_string_lossy()}))
            .await;
        assert!(!result.is_error);
        assert!(result.for_llm.contains("a.txt"));
        assert!(result.for_llm.contains("subdir/"));
    }

    #[tokio::test]
    async fn test_path_restriction() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = ReadFileTool::new(&ws, true);

        let result = tool
            .execute(&serde_json::json!({"path": "/etc/passwd"}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("outside workspace"));
    }

    #[tokio::test]
    async fn test_file_exists() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = FileExistsTool::new(&ws, false);

        // Non-existent file.
        let result = tool
            .execute(&serde_json::json!({"path": dir.path().join("nope.txt").to_string_lossy()}))
            .await;
        assert!(!result.is_error);
        assert!(result.for_llm.contains("false"));

        // Create file and check again.
        tokio::fs::write(dir.path().join("exists.txt"), "data").await.unwrap();
        let result = tool
            .execute(&serde_json::json!({"path": dir.path().join("exists.txt").to_string_lossy()}))
            .await;
        assert!(result.for_llm.contains("true"));
    }

    #[tokio::test]
    async fn test_create_directory() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = CreateDirectoryTool::new(&ws, false);

        let new_dir = dir.path().join("a/b/c");
        let result = tool
            .execute(&serde_json::json!({"path": new_dir.to_string_lossy()}))
            .await;
        assert!(!result.is_error);
        assert!(new_dir.exists());
    }

    #[tokio::test]
    async fn test_delete_file() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = DeleteFileTool::new(&ws, false);

        let file_path = dir.path().join("to_delete.txt");
        tokio::fs::write(&file_path, "bye").await.unwrap();
        assert!(file_path.exists());

        let result = tool
            .execute(&serde_json::json!({"path": file_path.to_string_lossy()}))
            .await;
        assert!(!result.is_error);
        assert!(!file_path.exists());
    }

    #[tokio::test]
    async fn test_delete_nonexistent_file() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = DeleteFileTool::new(&ws, false);

        let result = tool
            .execute(&serde_json::json!({"path": dir.path().join("missing.txt").to_string_lossy()}))
            .await;
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_delete_dir_tool() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = DeleteDirTool::new(&ws, false);

        // Create a directory with content
        let subdir = dir.path().join("to_delete");
        tokio::fs::create_dir_all(&subdir).await.unwrap();
        tokio::fs::write(subdir.join("file.txt"), "content")
            .await
            .unwrap();
        tokio::fs::create_dir(subdir.join("nested")).await.unwrap();

        assert!(subdir.exists());

        let result = tool
            .execute(&serde_json::json!({"path": subdir.to_string_lossy()}))
            .await;
        assert!(
            !result.is_error,
            "Expected success, got: {}",
            result.for_llm
        );
        assert!(result.silent, "Result should be silent");
        assert!(!subdir.exists(), "Directory should be deleted");
    }

    #[tokio::test]
    async fn test_delete_dir_tool_not_a_directory() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = DeleteDirTool::new(&ws, false);

        // Create a file (not a directory)
        let file_path = dir.path().join("file.txt");
        tokio::fs::write(&file_path, "content").await.unwrap();

        let result = tool
            .execute(&serde_json::json!({"path": file_path.to_string_lossy()}))
            .await;
        assert!(result.is_error);
        assert!(
            result.for_llm.contains("not a directory"),
            "Expected 'not a directory' error, got: {}",
            result.for_llm
        );
    }

    #[tokio::test]
    async fn test_delete_dir_tool_nonexistent() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = DeleteDirTool::new(&ws, false);

        let result = tool
            .execute(&serde_json::json!({
                "path": dir.path().join("nonexistent_dir").to_string_lossy()
            }))
            .await;
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_delete_dir_tool_restricted() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = DeleteDirTool::new(&ws, true);

        // Try to delete a directory outside workspace
        let result = tool
            .execute(&serde_json::json!({"path": "/tmp/should_not_work"}))
            .await;
        assert!(result.is_error);
        assert!(
            result.for_llm.contains("outside workspace"),
            "Expected 'outside workspace' error, got: {}",
            result.for_llm
        );
    }

    #[tokio::test]
    async fn test_delete_dir_tool_missing_path() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = DeleteDirTool::new(&ws, false);

        let result = tool.execute(&serde_json::json!({})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("missing"));
    }

    // ============================================================
    // Additional tests for missing coverage
    // ============================================================

    #[tokio::test]
    async fn test_read_file_missing_path_arg() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = ReadFileTool::new(&ws, false);

        let result = tool.execute(&serde_json::json!({})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("missing"));
    }

    #[tokio::test]
    async fn test_read_file_relative_path() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = ReadFileTool::new(&ws, false);

        tokio::fs::write(dir.path().join("relative.txt"), "relative content")
            .await
            .unwrap();

        let result = tool
            .execute(&serde_json::json!({"path": "relative.txt"}))
            .await;
        assert!(!result.is_error, "Expected success, got: {}", result.for_llm);
        assert_eq!(result.for_llm, "relative content");
    }

    #[tokio::test]
    async fn test_read_file_empty_content() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = ReadFileTool::new(&ws, false);

        tokio::fs::write(dir.path().join("empty.txt"), "")
            .await
            .unwrap();

        let result = tool
            .execute(&serde_json::json!({"path": dir.path().join("empty.txt").to_string_lossy()}))
            .await;
        assert!(!result.is_error);
        assert_eq!(result.for_llm, "");
    }

    #[tokio::test]
    async fn test_write_file_creates_subdirs() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = WriteFileTool::new(&ws, false);

        let nested_path = dir.path().join("a/b/c/deep.txt");

        let result = tool
            .execute(&serde_json::json!({
                "path": nested_path.to_string_lossy(),
                "content": "nested content"
            }))
            .await;
        assert!(!result.is_error, "Expected success, got: {}", result.for_llm);
        assert!(nested_path.exists());

        let content = tokio::fs::read_to_string(&nested_path).await.unwrap();
        assert_eq!(content, "nested content");
    }

    #[tokio::test]
    async fn test_write_file_missing_path() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = WriteFileTool::new(&ws, false);

        let result = tool
            .execute(&serde_json::json!({"content": "test"}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("missing"));
    }

    #[tokio::test]
    async fn test_write_file_missing_content() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = WriteFileTool::new(&ws, false);

        let result = tool
            .execute(&serde_json::json!({"path": "test.txt"}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("missing"));
    }

    #[tokio::test]
    async fn test_write_file_overwrites() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = WriteFileTool::new(&ws, false);

        let file_path = dir.path().join("overwrite.txt");
        tokio::fs::write(&file_path, "old content").await.unwrap();

        let result = tool
            .execute(&serde_json::json!({
                "path": file_path.to_string_lossy(),
                "content": "new content"
            }))
            .await;
        assert!(!result.is_error);

        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "new content");
    }

    #[tokio::test]
    async fn test_list_directory_default_path() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = ListDirTool::new(&ws, false);

        tokio::fs::write(dir.path().join("file1.txt"), "a").await.unwrap();
        tokio::fs::write(dir.path().join("file2.txt"), "b").await.unwrap();

        // No path provided - should default to "." relative to workspace
        let result = tool.execute(&serde_json::json!({})).await;
        assert!(!result.is_error, "Expected success, got: {}", result.for_llm);
    }

    #[tokio::test]
    async fn test_list_directory_nonexistent() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = ListDirTool::new(&ws, false);

        let result = tool
            .execute(&serde_json::json!({"path": "/nonexistent/dir/12345"}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("failed to list"));
    }

    #[tokio::test]
    async fn test_file_exists_directory() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = FileExistsTool::new(&ws, false);

        tokio::fs::create_dir(dir.path().join("subdir")).await.unwrap();

        let result = tool
            .execute(&serde_json::json!({"path": dir.path().join("subdir").to_string_lossy()}))
            .await;
        assert!(!result.is_error);
        assert!(result.for_llm.contains("true"));
        assert!(result.for_llm.contains("directory"));
    }

    #[tokio::test]
    async fn test_create_directory_already_exists() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = CreateDirectoryTool::new(&ws, false);

        // First creation
        let new_dir = dir.path().join("exists_already");
        let result = tool
            .execute(&serde_json::json!({"path": new_dir.to_string_lossy()}))
            .await;
        assert!(!result.is_error);

        // Second creation (should succeed - idempotent)
        let result = tool
            .execute(&serde_json::json!({"path": new_dir.to_string_lossy()}))
            .await;
        assert!(!result.is_error);
        assert!(new_dir.exists());
    }

    #[tokio::test]
    async fn test_create_directory_missing_path() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = CreateDirectoryTool::new(&ws, false);

        let result = tool.execute(&serde_json::json!({})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("missing"));
    }

    #[tokio::test]
    async fn test_delete_file_missing_path() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = DeleteFileTool::new(&ws, false);

        let result = tool.execute(&serde_json::json!({})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("missing"));
    }

    #[tokio::test]
    async fn test_read_file_tool_interface() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = ReadFileTool::new(&ws, false);

        assert_eq!(tool.name(), "read_file");
        assert!(!tool.description().is_empty());
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
    }

    #[tokio::test]
    async fn test_write_file_tool_interface() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = WriteFileTool::new(&ws, false);

        assert_eq!(tool.name(), "write_file");
        assert!(!tool.description().is_empty());
    }

    #[tokio::test]
    async fn test_list_dir_tool_interface() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = ListDirTool::new(&ws, false);

        assert_eq!(tool.name(), "list_dir");
        assert!(!tool.description().is_empty());
    }

    #[tokio::test]
    async fn test_delete_file_tool_interface() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = DeleteFileTool::new(&ws, false);

        assert_eq!(tool.name(), "delete_file");
        assert!(!tool.description().is_empty());
    }

    #[tokio::test]
    async fn test_create_dir_tool_interface() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = CreateDirectoryTool::new(&ws, false);

        assert_eq!(tool.name(), "create_dir");
        assert!(!tool.description().is_empty());
    }

    #[tokio::test]
    async fn test_file_exists_tool_interface() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = FileExistsTool::new(&ws, false);

        assert_eq!(tool.name(), "file_exists");
        assert!(!tool.description().is_empty());
    }

    #[tokio::test]
    async fn test_delete_dir_tool_interface() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = DeleteDirTool::new(&ws, false);

        assert_eq!(tool.name(), "delete_dir");
        assert!(!tool.description().is_empty());
    }

    // ============================================================
    // Workspace restriction tests for write/create/delete tools
    // ============================================================

    #[tokio::test]
    async fn test_write_file_restricted_outside_workspace() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = WriteFileTool::new(&ws, true);

        let result = tool
            .execute(&serde_json::json!({
                "path": "/tmp/outside_workspace_test.txt",
                "content": "should fail"
            }))
            .await;
        assert!(result.is_error, "Expected error for write outside workspace, got: {}", result.for_llm);
        assert!(
            result.for_llm.contains("outside") || result.for_llm.contains("denied"),
            "Expected 'outside' or 'denied' error, got: {}",
            result.for_llm
        );
    }

    #[tokio::test]
    async fn test_create_directory_restricted_outside_workspace() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = CreateDirectoryTool::new(&ws, true);

        let result = tool
            .execute(&serde_json::json!({"path": "/tmp/outside_workspace_dir"}))
            .await;
        assert!(result.is_error, "Expected error for create_dir outside workspace, got: {}", result.for_llm);
        assert!(
            result.for_llm.contains("outside") || result.for_llm.contains("denied"),
            "Expected 'outside' or 'denied' error, got: {}",
            result.for_llm
        );
    }

    #[tokio::test]
    async fn test_delete_file_restricted_outside_workspace() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = DeleteFileTool::new(&ws, true);

        // Create a file outside workspace to try to delete
        let outside = std::env::temp_dir().join("nemesis_test_outside_delete.txt");
        std::fs::write(&outside, "test").ok();

        let result = tool
            .execute(&serde_json::json!({"path": outside.to_string_lossy()}))
            .await;
        assert!(result.is_error, "Expected error for delete outside workspace, got: {}", result.for_llm);

        // Cleanup
        std::fs::remove_file(&outside).ok();
    }

    #[tokio::test]
    async fn test_write_file_restricted_within_workspace() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = WriteFileTool::new(&ws, true);

        let file_path = dir.path().join("allowed_write.txt");
        let result = tool
            .execute(&serde_json::json!({
                "path": file_path.to_string_lossy(),
                "content": "allowed"
            }))
            .await;
        assert!(!result.is_error, "Expected success for write within workspace, got: {}", result.for_llm);
    }

    #[tokio::test]
    async fn test_create_directory_restricted_within_workspace() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = CreateDirectoryTool::new(&ws, true);

        let new_dir = dir.path().join("allowed_dir");
        let result = tool
            .execute(&serde_json::json!({"path": new_dir.to_string_lossy()}))
            .await;
        assert!(!result.is_error, "Expected success for create_dir within workspace, got: {}", result.for_llm);
        assert!(new_dir.exists());
    }

    // ============================================================
    // Additional filesystem edge-case tests
    // ============================================================

    #[tokio::test]
    async fn test_read_file_with_special_characters() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = ReadFileTool::new(&ws, false);

        let content = "Special: <tag> & \"quotes\" 'single' \n newlines \t tabs";
        tokio::fs::write(dir.path().join("special.txt"), content)
            .await
            .unwrap();

        let result = tool
            .execute(&serde_json::json!({"path": dir.path().join("special.txt").to_string_lossy()}))
            .await;
        assert!(!result.is_error);
        assert!(result.for_llm.contains("<tag>"));
        assert!(result.for_llm.contains("&"));
    }

    #[tokio::test]
    async fn test_write_file_unicode_content() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = WriteFileTool::new(&ws, false);

        let file_path = dir.path().join("unicode.txt");
        let result = tool
            .execute(&serde_json::json!({
                "path": file_path.to_string_lossy(),
                "content": "Hello! - Test"
            }))
            .await;
        assert!(!result.is_error);

        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert!(content.contains("Hello!"));
    }

    #[tokio::test]
    async fn test_list_directory_with_mixed_types() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = ListDirTool::new(&ws, false);

        // Create files and subdirs
        tokio::fs::write(dir.path().join("file.txt"), "a").await.unwrap();
        tokio::fs::create_dir(dir.path().join("subdir")).await.unwrap();
        tokio::fs::write(dir.path().join("subdir").join("nested.txt"), "b").await.unwrap();

        let result = tool
            .execute(&serde_json::json!({"path": dir.path().to_string_lossy()}))
            .await;
        assert!(!result.is_error);
        assert!(result.for_llm.contains("file.txt"));
        assert!(result.for_llm.contains("subdir"));
    }

    #[tokio::test]
    async fn test_file_exists_false() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = FileExistsTool::new(&ws, false);

        let result = tool
            .execute(&serde_json::json!({"path": dir.path().join("nonexistent.txt").to_string_lossy()}))
            .await;
        assert!(!result.is_error);
        assert!(result.for_llm.contains("false"));
    }

    #[tokio::test]
    async fn test_file_exists_missing_path() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = FileExistsTool::new(&ws, false);

        let result = tool.execute(&serde_json::json!({})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("missing"));
    }

    #[tokio::test]
    async fn test_read_file_restricted_outside_workspace() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = ReadFileTool::new(&ws, true);

        let result = tool
            .execute(&serde_json::json!({"path": "/etc/hosts"}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("outside workspace") || result.for_llm.contains("denied"));
    }

    #[tokio::test]
    async fn test_write_file_empty_content() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = WriteFileTool::new(&ws, false);

        let file_path = dir.path().join("empty_write.txt");
        let result = tool
            .execute(&serde_json::json!({
                "path": file_path.to_string_lossy(),
                "content": ""
            }))
            .await;
        assert!(!result.is_error);
        assert!(file_path.exists());
        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "");
    }

    #[tokio::test]
    async fn test_list_directory_empty_dir() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = ListDirTool::new(&ws, false);

        let empty_subdir = dir.path().join("empty_subdir");
        tokio::fs::create_dir(&empty_subdir).await.unwrap();

        let result = tool
            .execute(&serde_json::json!({"path": empty_subdir.to_string_lossy()}))
            .await;
        // Should succeed but show empty or no entries
        assert!(!result.is_error || result.for_llm.contains("empty") || result.for_llm.contains("no entries"));
    }

    #[tokio::test]
    async fn test_create_directory_single_level() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = CreateDirectoryTool::new(&ws, false);

        let new_dir = dir.path().join("single");
        let result = tool
            .execute(&serde_json::json!({"path": new_dir.to_string_lossy()}))
            .await;
        assert!(!result.is_error);
        assert!(new_dir.is_dir());
    }

    #[tokio::test]
    async fn test_delete_file_tool_restricted_within_workspace() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = DeleteFileTool::new(&ws, true);

        let file_path = dir.path().join("restricted_delete.txt");
        tokio::fs::write(&file_path, "content").await.unwrap();

        let result = tool
            .execute(&serde_json::json!({"path": file_path.to_string_lossy()}))
            .await;
        assert!(!result.is_error, "Should allow delete within workspace");
        assert!(!file_path.exists());
    }

    // ============================================================
    // Additional coverage tests for 95%+ target (round 2)
    // ============================================================

    #[test]
    fn test_resolve_existing_ancestor_current_dir() {
        // Current dir always exists
        let resolved = resolve_existing_ancestor(Path::new("."));
        assert!(resolved.exists() || resolved == Path::new("."));
    }

    #[test]
    fn test_normalize_for_comparison_regular_path() {
        let path = Path::new("C:\\Users\\test\\file.txt");
        let normalized = normalize_for_comparison(path);
        assert!(!normalized.starts_with(r"\\?\"));
    }

    #[test]
    fn test_normalize_for_comparison_unc_prefix() {
        // Simulate the Windows \\?\ prefix
        let path = Path::new(r"\\?\C:\Users\test\file.txt");
        let normalized = normalize_for_comparison(path);
        assert!(!normalized.starts_with(r"\\?\"));
        assert!(normalized.contains("Users"));
    }

    #[test]
    fn test_normalize_for_comparison_no_prefix() {
        let path = Path::new("/home/user/file.txt");
        let normalized = normalize_for_comparison(path);
        assert_eq!(normalized, "/home/user/file.txt");
    }

    #[tokio::test]
    async fn test_read_file_nonexistent_file() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = ReadFileTool::new(&ws, false);

        let result = tool
            .execute(&serde_json::json!({"path": dir.path().join("nonexistent_file.txt").to_string_lossy()}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("failed to read"));
    }

    #[tokio::test]
    async fn test_write_file_binary_content() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = WriteFileTool::new(&ws, false);

        let file_path = dir.path().join("binary.txt");
        let result = tool
            .execute(&serde_json::json!({
                "path": file_path.to_string_lossy(),
                "content": "binary\x00content"
            }))
            .await;
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_list_dir_with_files() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = ListDirTool::new(&ws, false);

        tokio::fs::write(dir.path().join("a.txt"), "a").await.unwrap();
        tokio::fs::write(dir.path().join("b.txt"), "b").await.unwrap();
        tokio::fs::create_dir(dir.path().join("subdir")).await.unwrap();

        let result = tool
            .execute(&serde_json::json!({"path": dir.path().to_string_lossy()}))
            .await;
        assert!(!result.is_error);
        assert!(result.for_llm.contains("a.txt"));
        assert!(result.for_llm.contains("b.txt"));
        assert!(result.for_llm.contains("subdir/"));
    }

    #[tokio::test]
    async fn test_file_exists_with_relative_path() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = FileExistsTool::new(&ws, false);

        tokio::fs::write(dir.path().join("exists.txt"), "yes").await.unwrap();

        let result = tool
            .execute(&serde_json::json!({"path": "exists.txt"}))
            .await;
        assert!(!result.is_error);
        assert!(result.for_llm.contains("true"));
    }

    #[tokio::test]
    async fn test_create_directory_nested() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = CreateDirectoryTool::new(&ws, false);

        let nested = dir.path().join("a/b/c");
        let result = tool
            .execute(&serde_json::json!({"path": nested.to_string_lossy()}))
            .await;
        assert!(!result.is_error);
        assert!(nested.is_dir());
    }

    #[tokio::test]
    async fn test_delete_file_nonexistent() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = DeleteFileTool::new(&ws, false);

        let result = tool
            .execute(&serde_json::json!({"path": dir.path().join("nonexistent.txt").to_string_lossy()}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("failed"));
    }

    #[tokio::test]
    async fn test_delete_dir_with_contents() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = DeleteDirTool::new(&ws, false);

        let sub = dir.path().join("to_delete");
        tokio::fs::create_dir_all(&sub).await.unwrap();
        tokio::fs::write(sub.join("file.txt"), "content").await.unwrap();

        let result = tool
            .execute(&serde_json::json!({"path": sub.to_string_lossy()}))
            .await;
        assert!(!result.is_error);
        assert!(!sub.exists());
    }

    #[tokio::test]
    async fn test_delete_dir_nonexistent() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = DeleteDirTool::new(&ws, false);

        let result = tool
            .execute(&serde_json::json!({"path": dir.path().join("no_such_dir").to_string_lossy()}))
            .await;
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_read_file_tool_non_string_path() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = ReadFileTool::new(&ws, false);

        let result = tool.execute(&serde_json::json!({"path": 42})).await;
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_write_file_tool_non_string_content() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = WriteFileTool::new(&ws, false);

        let result = tool
            .execute(&serde_json::json!({"path": "test.txt", "content": 123}))
            .await;
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_delete_dir_tool_nonexistent_path() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let tool = DeleteDirTool::new(&ws, false);

        let result = tool
            .execute(&serde_json::json!({"path": "/nonexistent/path/xyz123"}))
            .await;
        assert!(result.is_error);
    }
}

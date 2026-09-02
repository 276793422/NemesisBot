use super::*;

use tempfile::TempDir;

fn make_tools(dir: &TempDir) -> (ReadFileTool, WriteFileTool, ListDirTool) {
    let ws = dir.path().to_string_lossy().to_string();
    (
        ReadFileTool::new(&ws, false),
        WriteFileTool::new(&ws, false),
        ListDirTool::new(&ws, false),
    )
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

    tokio::fs::write(dir.path().join("a.txt"), "a")
        .await
        .unwrap();
    tokio::fs::create_dir(dir.path().join("subdir"))
        .await
        .unwrap();

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
    tokio::fs::write(dir.path().join("exists.txt"), "data")
        .await
        .unwrap();
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
    assert!(
        !result.is_error,
        "Expected success, got: {}",
        result.for_llm
    );
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
    assert!(
        !result.is_error,
        "Expected success, got: {}",
        result.for_llm
    );
    assert!(nested_path.exists());

    let content = tokio::fs::read_to_string(&nested_path).await.unwrap();
    assert_eq!(content, "nested content");
}

#[tokio::test]
async fn test_write_file_missing_path() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let tool = WriteFileTool::new(&ws, false);

    let result = tool.execute(&serde_json::json!({"content": "test"})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("missing"));
}

#[tokio::test]
async fn test_write_file_missing_content() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let tool = WriteFileTool::new(&ws, false);

    let result = tool.execute(&serde_json::json!({"path": "test.txt"})).await;
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

    tokio::fs::write(dir.path().join("file1.txt"), "a")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("file2.txt"), "b")
        .await
        .unwrap();

    // No path provided - should default to "." relative to workspace
    let result = tool.execute(&serde_json::json!({})).await;
    assert!(
        !result.is_error,
        "Expected success, got: {}",
        result.for_llm
    );
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

    tokio::fs::create_dir(dir.path().join("subdir"))
        .await
        .unwrap();

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
    assert!(
        result.is_error,
        "Expected error for write outside workspace, got: {}",
        result.for_llm
    );
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
    assert!(
        result.is_error,
        "Expected error for create_dir outside workspace, got: {}",
        result.for_llm
    );
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
    assert!(
        result.is_error,
        "Expected error for delete outside workspace, got: {}",
        result.for_llm
    );

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
    assert!(
        !result.is_error,
        "Expected success for write within workspace, got: {}",
        result.for_llm
    );
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
    assert!(
        !result.is_error,
        "Expected success for create_dir within workspace, got: {}",
        result.for_llm
    );
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
    tokio::fs::write(dir.path().join("file.txt"), "a")
        .await
        .unwrap();
    tokio::fs::create_dir(dir.path().join("subdir"))
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("subdir").join("nested.txt"), "b")
        .await
        .unwrap();

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
    assert!(
        !result.is_error
            || result.for_llm.contains("empty")
            || result.for_llm.contains("no entries")
    );
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
fn test_canonicalize_for_compare_current_dir_strips_verbatim() {
    // 替换旧局部 helper（resolve_existing_ancestor / normalize_for_comparison）
    // 的 4 个单测——helper 已收敛到 nemesis-path 单一真相源，其内部分支由该
    // crate 自己的测试覆盖。此处只锚消费方关心的行为：cwd 归一化为绝对路径；
    // Windows canonicalize 的 \\?\ verbatim 前缀必须被剥掉（否则 workspace
    // 前缀比较永远失配，CI RUNNER~1 家族，2026-09-01）。
    let resolved = canonicalize_for_compare(Path::new("."));
    assert!(resolved.is_absolute());

    let real = std::env::current_dir().unwrap();
    let s = canonicalize_for_compare(&real)
        .to_string_lossy()
        .to_string();
    assert!(
        !s.starts_with(r"\\?\"),
        "verbatim prefix must be stripped for comparison: {s}"
    );
}

#[tokio::test]
async fn test_read_file_nonexistent_file() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let tool = ReadFileTool::new(&ws, false);

    let result = tool
        .execute(
            &serde_json::json!({"path": dir.path().join("nonexistent_file.txt").to_string_lossy()}),
        )
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

    tokio::fs::write(dir.path().join("a.txt"), "a")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("b.txt"), "b")
        .await
        .unwrap();
    tokio::fs::create_dir(dir.path().join("subdir"))
        .await
        .unwrap();

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

    tokio::fs::write(dir.path().join("exists.txt"), "yes")
        .await
        .unwrap();

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
    tokio::fs::write(sub.join("file.txt"), "content")
        .await
        .unwrap();

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

// ============================================================
// W4a coverage gap closure (parameters() schemas, private
// resolve_existing_ancestor branches, restrict/validate arms,
// delete_dir metadata branches)
// ============================================================

#[test]
fn w4a_tool_parameters_schemas_all_tools() {
    // The tool_interface tests below only call name()/description();
    // parameters() bodies of all 7 tools are otherwise never executed.
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();

    let read = ReadFileTool::new(&ws, false);
    let p = read.parameters();
    assert_eq!(p["type"], "object");
    assert_eq!(p["properties"]["path"]["type"], "string");
    assert_eq!(p["required"][0], "path");

    let write = WriteFileTool::new(&ws, false);
    let p = write.parameters();
    assert_eq!(p["properties"]["path"]["type"], "string");
    assert_eq!(p["properties"]["content"]["type"], "string");
    assert_eq!(p["required"].as_array().unwrap().len(), 2);

    let list = ListDirTool::new(&ws, false);
    let p = list.parameters();
    assert!(p.get("properties").is_some());
    assert!(p.get("required").is_none(), "list has no required fields");

    let exists = FileExistsTool::new(&ws, false);
    let p = exists.parameters();
    assert_eq!(p["required"][0], "path");

    let create = CreateDirectoryTool::new(&ws, false);
    let p = create.parameters();
    assert_eq!(p["properties"]["path"]["type"], "string");
    assert_eq!(p["required"][0], "path");

    let delete_file = DeleteFileTool::new(&ws, false);
    let p = delete_file.parameters();
    assert_eq!(p["required"][0], "path");

    let delete_dir = DeleteDirTool::new(&ws, false);
    let p = delete_dir.parameters();
    assert_eq!(
        p["properties"]["path"]["description"],
        "Directory path to delete"
    );
    assert_eq!(p["required"][0], "path");
}

#[tokio::test]
async fn w4a_restrict_tolerates_uncanonicalized_workspace_form() {
    // 8.3 短名失配回归（Windows CI RUNNER~1 家族，2026-09-01）：CI runner 的
    // TEMP 在 C:\Users\RUNNER~1\...，workspace 以非 canonical 形式传入时，
    // 旧实现只对 target 侧 resolve（canonicalize 出真实形态），workspace 侧
    // 用原始串比较 → 恒不相等 → workspace 内读全部误拒 "outside workspace"。
    // 本测试用大小写失配模拟同一机制：Windows 大小写不敏感 → canonicalize
    // 出真实大小写；Linux 大小写敏感 → 两侧 fallback 同形，天然通过。
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("sub").join("f.txt"), b"data").unwrap();

    let upper = dir.path().to_string_lossy().to_uppercase();
    let tool = ReadFileTool::new(&upper, true);
    let validated = tool
        .validate_path("sub/f.txt")
        .expect("workspace in uncanonicalized form must not be rejected");
    assert!(validated.ends_with("f.txt"));
}

#[test]
fn w4a_canonicalize_for_compare_branches() {
    let dir = TempDir::new().unwrap();

    // 1. Existing path: canonicalize succeeds; the \\?\ verbatim prefix is
    //    stripped by the truth-source impl (Windows), no-op elsewhere.
    let existing = dir.path().join("real.txt");
    std::fs::write(&existing, b"x").unwrap();
    let resolved = canonicalize_for_compare(&existing);
    assert!(resolved.is_absolute());
    let s = resolved.to_string_lossy();
    assert!(
        !s.starts_with(r"\\?\") && s.ends_with("real.txt"),
        "got: {s}"
    );

    // 2. Nonexistent child under existing dir: walk up + append components.
    let ghost = dir.path().join("no_such_dir").join("leaf.txt");
    let resolved = canonicalize_for_compare(&ghost);
    let s = resolved.to_string_lossy();
    assert!(
        s.ends_with("no_such_dir\\leaf.txt") || s.ends_with("no_such_dir/leaf.txt"),
        "non-existing tail must be appended onto the canonical ancestor: {s}"
    );

    // 3. Absent drive (Z: not mounted on this host): walk-up hits the root
    //    without any existing component -> lexical normalization fallback
    //    (input has no `.`/`..`, so unchanged).
    let absent = Path::new("Z:\\w4a\\never\\exists.txt");
    if !Path::new("Z:\\").exists() {
        assert_eq!(canonicalize_for_compare(absent), absent.to_path_buf());
    }
}

#[tokio::test]
async fn w4a_read_file_restricted_outside_workspace_rejected() {
    // restrict=true + absolute path outside the workspace must be rejected by
    // validate_path (symlink-resolution + prefix comparison).
    let dir = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let tool = ReadFileTool::new(&ws, true);

    let victim = outside.path().join("secret.txt");
    std::fs::write(&victim, b"top secret").unwrap();
    let result = tool
        .execute(&serde_json::json!({"path": victim.to_string_lossy()}))
        .await;
    assert!(result.is_error);
    assert!(
        result.for_llm.contains("outside workspace"),
        "got: {}",
        result.for_llm
    );
    // and a file inside the workspace still reads fine under restrict
    let inside = dir.path().join("ok.txt");
    std::fs::write(&inside, b"fine").unwrap();
    let result = tool.execute(&serde_json::json!({"path": "ok.txt"})).await;
    assert!(
        !result.is_error,
        "inside file must read: {}",
        result.for_llm
    );
}

#[tokio::test]
async fn w4a_create_directory_relative_path_joins_workspace() {
    // CreateDirectoryTool validates via its own logic; relative paths join the
    // workspace root. Also covers the missing-'path' error arm.
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let tool = CreateDirectoryTool::new(&ws, false);

    let result = tool
        .execute(&serde_json::json!({"path": "w4a_new/sub"}))
        .await;
    assert!(
        !result.is_error,
        "create should succeed: {}",
        result.for_llm
    );
    assert!(dir.path().join("w4a_new").join("sub").is_dir());

    // missing path argument
    let result = tool.execute(&serde_json::json!({})).await;
    assert!(result.is_error);
}

#[tokio::test]
async fn w4a_delete_dir_restricted_outside_workspace_rejected() {
    let dir = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let tool = DeleteDirTool::new(&ws, true);

    let victim = outside.path().join("victim_dir");
    std::fs::create_dir_all(&victim).unwrap();
    let result = tool
        .execute(&serde_json::json!({"path": victim.to_string_lossy()}))
        .await;
    assert!(result.is_error);
    assert!(
        result.for_llm.contains("outside workspace"),
        "got: {}",
        result.for_llm
    );
    assert!(victim.exists(), "outside dir must NOT be deleted");
}

#[tokio::test]
async fn w4a_delete_dir_not_a_directory_is_rejected() {
    // Pointing delete_dir at a regular file hits the metadata is_dir()==false arm.
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let tool = DeleteDirTool::new(&ws, false);

    let file = dir.path().join("plain_file.txt");
    std::fs::write(&file, b"data").unwrap();
    let result = tool
        .execute(&serde_json::json!({"path": file.to_string_lossy()}))
        .await;
    assert!(result.is_error);
    assert!(
        result.for_llm.contains("is not a directory"),
        "got: {}",
        result.for_llm
    );
    assert!(file.exists(), "file must survive a delete_dir attempt");
}

#[tokio::test]
async fn w4a_delete_dir_success_is_silent() {
    // Successful delete_dir returns a silent ToolResult (for_llm message only,
    // for_user None, silent=true).
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let tool = DeleteDirTool::new(&ws, false);

    let victim = dir.path().join("doomed");
    std::fs::create_dir_all(victim.join("nested")).unwrap();
    std::fs::write(victim.join("nested").join("f.txt"), b"x").unwrap();

    let result = tool
        .execute(&serde_json::json!({"path": victim.to_string_lossy()}))
        .await;
    assert!(!result.is_error);
    assert!(result.silent, "successful delete_dir is silent");
    assert!(result.for_user.is_none());
    assert!(result.for_llm.contains("Directory deleted"));
    assert!(!victim.exists(), "directory tree must be gone");
}

// ===========================================================================
// S2 coverage (2026-08-26): DeleteFileTool relative-path validate arm.
//
// resolve_existing_ancestor canonicalize-failure arms (lines 16/35) are
// STRUCTURAL on Windows: an empirical rustc probe showed std reports
// exists() == false for every NUL-device path variant (C:\dir\nul, C:\nul,
// \\.\nul) and canonicalize fails with os error 87 — there is no
// deterministic Windows path with exists() == true && canonicalize() == Err,
// so the guard `p.exists()` never admits a canonicalize-failing path.
// ===========================================================================

/// DeleteFileTool::validate_path with a relative path joins the workspace
/// (the non-absolute arm of the target resolution).
#[test]
fn s2_delete_file_validate_path_relative_joins_workspace() {
    let dir = TempDir::new().unwrap();
    let tool = DeleteFileTool::new(&dir.path().to_string_lossy(), false);

    let resolved = tool.validate_path("rel/deep.txt").unwrap();
    assert_eq!(resolved, dir.path().join("rel").join("deep.txt"));
}

#[tokio::test]
async fn spill_locator_readable_under_workspace_restriction() {
    // U4 spill×restrict 契约（2026-08-31 迁移配套）：spill 根已迁回
    // <workspace>/logs/spill（nemesis-path 唯一拼接点）。restrict=true 的
    // read_file 必须能回读落在该根下的定位器全文——否则超大工具结果一旦
    // 外溢，agent 就永远拿不到完整内容。
    let dir = TempDir::new().unwrap();
    let workspace = nemesis_path::workspace_dir(dir.path());
    let spill_root = nemesis_path::resolve_spill_dir_in_workspace(&workspace);
    std::fs::create_dir_all(&spill_root).unwrap();

    let locator = spill_root.join("20260831_000000000_call_00_deadbeef.txt");
    std::fs::write(&locator, "FULL oversized tool result lives here").unwrap();

    let ws = workspace.to_string_lossy().to_string();
    let tool = ReadFileTool::new(&ws, true);
    let result = tool
        .execute(&serde_json::json!({ "path": locator.to_string_lossy() }))
        .await;
    assert!(
        !result.is_error,
        "spill locator inside workspace must be readable under restrict=true, got: {}",
        result.for_llm
    );
    assert!(result.for_llm.contains("FULL oversized tool result"));
}

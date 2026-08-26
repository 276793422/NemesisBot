use super::*;
use tempfile::TempDir;

fn make_tools(dir: &TempDir) -> (EditFileTool, AppendFileTool) {
    let ws = dir.path().to_string_lossy().to_string();
    (
        EditFileTool::new(&ws, false),
        AppendFileTool::new(&ws, false),
    )
}

#[tokio::test]
async fn test_edit_file_success() {
    let dir = TempDir::new().unwrap();
    let (edit_tool, _) = make_tools(&dir);
    let file_path = dir.path().join("test.txt");
    tokio::fs::write(&file_path, "hello world").await.unwrap();

    let path_str = file_path.to_string_lossy().to_string();
    let result = edit_tool
        .execute(&serde_json::json!({
            "path": path_str,
            "old_text": "world",
            "new_text": "rust"
        }))
        .await;
    assert!(
        !result.is_error,
        "Expected success, got: {}",
        result.for_llm
    );

    let content = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(content, "hello rust");
}

#[tokio::test]
async fn test_edit_file_old_text_not_found() {
    let dir = TempDir::new().unwrap();
    let (edit_tool, _) = make_tools(&dir);
    let file_path = dir.path().join("test.txt");
    tokio::fs::write(&file_path, "hello world").await.unwrap();

    let result = edit_tool
        .execute(&serde_json::json!({
            "path": file_path.to_string_lossy(),
            "old_text": "nonexistent",
            "new_text": "rust"
        }))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("not found"));
}

#[tokio::test]
async fn test_edit_file_multiple_occurrences() {
    let dir = TempDir::new().unwrap();
    let (edit_tool, _) = make_tools(&dir);
    let file_path = dir.path().join("test.txt");
    tokio::fs::write(&file_path, "aaa bbb aaa").await.unwrap();

    let result = edit_tool
        .execute(&serde_json::json!({
            "path": file_path.to_string_lossy(),
            "old_text": "aaa",
            "new_text": "ccc"
        }))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("2 times"));
}

#[tokio::test]
async fn test_edit_file_missing() {
    let dir = TempDir::new().unwrap();
    let (edit_tool, _) = make_tools(&dir);

    let result = edit_tool
        .execute(&serde_json::json!({
            "path": "/nonexistent/file.txt",
            "old_text": "a",
            "new_text": "b"
        }))
        .await;
    assert!(result.is_error);
}

#[tokio::test]
async fn test_append_file() {
    let dir = TempDir::new().unwrap();
    let (_, append_tool) = make_tools(&dir);
    let file_path = dir.path().join("output.txt");
    tokio::fs::write(&file_path, "hello ").await.unwrap();

    let result = append_tool
        .execute(&serde_json::json!({
            "path": file_path.to_string_lossy(),
            "content": "world"
        }))
        .await;
    assert!(!result.is_error);

    let content = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(content, "hello world");
}

#[tokio::test]
async fn test_append_file_creates_new() {
    let dir = TempDir::new().unwrap();
    let (_, append_tool) = make_tools(&dir);
    let file_path = dir.path().join("new_file.txt");

    let result = append_tool
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
async fn test_edit_path_restriction() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let tool = EditFileTool::new(&ws, true);

    let result = tool
        .execute(&serde_json::json!({
            "path": "/etc/passwd",
            "old_text": "a",
            "new_text": "b"
        }))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("outside workspace"));
}

// ============================================================
// Additional tests for missing coverage
// ============================================================

#[tokio::test]
async fn test_edit_file_missing_path_arg() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let tool = EditFileTool::new(&ws, false);

    let result = tool
        .execute(&serde_json::json!({
            "old_text": "a",
            "new_text": "b"
        }))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("path is required"));
}

#[tokio::test]
async fn test_edit_file_missing_old_text() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let tool = EditFileTool::new(&ws, false);

    let result = tool
        .execute(&serde_json::json!({
            "path": "test.txt",
            "new_text": "b"
        }))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("old_text is required"));
}

#[tokio::test]
async fn test_edit_file_missing_new_text() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let tool = EditFileTool::new(&ws, false);

    let result = tool
        .execute(&serde_json::json!({
            "path": "test.txt",
            "old_text": "a"
        }))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("new_text is required"));
}

#[tokio::test]
async fn test_edit_file_exact_replacement() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let tool = EditFileTool::new(&ws, false);
    let file_path = dir.path().join("exact.txt");
    tokio::fs::write(&file_path, "line1\nline2\nline3")
        .await
        .unwrap();

    let result = tool
        .execute(&serde_json::json!({
            "path": file_path.to_string_lossy(),
            "old_text": "line2",
            "new_text": "replaced"
        }))
        .await;
    assert!(
        !result.is_error,
        "Expected success, got: {}",
        result.for_llm
    );

    let content = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(content, "line1\nreplaced\nline3");
}

#[tokio::test]
async fn test_edit_file_multiline_replacement() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let tool = EditFileTool::new(&ws, false);
    let file_path = dir.path().join("multi.txt");
    tokio::fs::write(&file_path, "start\nmiddle\nend")
        .await
        .unwrap();

    let result = tool
        .execute(&serde_json::json!({
            "path": file_path.to_string_lossy(),
            "old_text": "middle",
            "new_text": "new_middle"
        }))
        .await;
    assert!(!result.is_error);

    let content = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(content, "start\nnew_middle\nend");
}

#[tokio::test]
async fn test_edit_tool_interface() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let tool = EditFileTool::new(&ws, false);

    assert_eq!(tool.name(), "edit_file");
    assert!(!tool.description().is_empty());
    let params = tool.parameters();
    assert_eq!(params["type"], "object");
}

#[tokio::test]
async fn test_append_tool_interface() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let tool = AppendFileTool::new(&ws, false);

    assert_eq!(tool.name(), "append_file");
    assert!(!tool.description().is_empty());
}

#[tokio::test]
async fn test_append_file_missing_path() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let tool = AppendFileTool::new(&ws, false);

    let result = tool.execute(&serde_json::json!({"content": "test"})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("path is required"));
}

#[tokio::test]
async fn test_append_file_missing_content() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let tool = AppendFileTool::new(&ws, false);

    let result = tool.execute(&serde_json::json!({"path": "test.txt"})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("content is required"));
}

#[tokio::test]
async fn test_append_file_creates_subdirs() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let tool = AppendFileTool::new(&ws, false);

    let nested_path = dir.path().join("deep/nested/append.txt");
    let result = tool
        .execute(&serde_json::json!({
            "path": nested_path.to_string_lossy(),
            "content": "deep content"
        }))
        .await;
    assert!(
        !result.is_error,
        "Expected success, got: {}",
        result.for_llm
    );
    assert!(nested_path.exists());

    let content = tokio::fs::read_to_string(&nested_path).await.unwrap();
    assert_eq!(content, "deep content");
}

#[tokio::test]
async fn test_append_file_multiple_appends() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let tool = AppendFileTool::new(&ws, false);
    let file_path = dir.path().join("multi_append.txt");

    // First append
    let result = tool
        .execute(&serde_json::json!({
            "path": file_path.to_string_lossy(),
            "content": "first"
        }))
        .await;
    assert!(!result.is_error);

    // Second append
    let result = tool
        .execute(&serde_json::json!({
            "path": file_path.to_string_lossy(),
            "content": "second"
        }))
        .await;
    assert!(!result.is_error);

    let content = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(content, "firstsecond");
}

#[tokio::test]
async fn test_append_file_result_is_silent() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let tool = AppendFileTool::new(&ws, false);
    let file_path = dir.path().join("silent_test.txt");

    let result = tool
        .execute(&serde_json::json!({
            "path": file_path.to_string_lossy(),
            "content": "content"
        }))
        .await;
    assert!(!result.is_error);
    assert!(result.silent, "Append result should be silent");
}

#[tokio::test]
async fn test_edit_file_result_is_silent() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let tool = EditFileTool::new(&ws, false);
    let file_path = dir.path().join("silent_edit.txt");
    tokio::fs::write(&file_path, "hello world").await.unwrap();

    let result = tool
        .execute(&serde_json::json!({
            "path": file_path.to_string_lossy(),
            "old_text": "world",
            "new_text": "rust"
        }))
        .await;
    assert!(!result.is_error);
    assert!(result.silent, "Edit result should be silent");
}

#[tokio::test]
async fn test_append_path_restriction() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let tool = AppendFileTool::new(&ws, true);

    let result = tool
        .execute(&serde_json::json!({
            "path": "/etc/passwd",
            "content": "should not work"
        }))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("outside workspace"));
}

// ============================================================
// Additional edit/append tool edge-case tests
// ============================================================

#[tokio::test]
async fn test_edit_file_old_text_not_found_v2() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let tool = EditFileTool::new(&ws, false);

    let result = tool
        .execute(&serde_json::json!({
            "path": dir.path().join("nonexistent.txt").to_string_lossy(),
            "old_text": "a",
            "new_text": "b"
        }))
        .await;
    assert!(result.is_error);
}

#[tokio::test]
async fn test_edit_file_empty_old_text() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let tool = EditFileTool::new(&ws, false);

    let result = tool
        .execute(&serde_json::json!({
            "path": "test.txt",
            "old_text": "",
            "new_text": "b"
        }))
        .await;
    // Empty old_text is technically valid (matches at every position)
    // The tool will try the operation but the file doesn't exist
    assert!(result.is_error);
}

#[tokio::test]
async fn test_edit_file_replace_with_empty() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let tool = EditFileTool::new(&ws, false);
    let file_path = dir.path().join("del.txt");
    tokio::fs::write(&file_path, "remove this text")
        .await
        .unwrap();

    let result = tool
        .execute(&serde_json::json!({
            "path": file_path.to_string_lossy(),
            "old_text": " this text",
            "new_text": ""
        }))
        .await;
    assert!(
        !result.is_error,
        "Expected success, got: {}",
        result.for_llm
    );

    let content = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(content, "remove");
}

#[tokio::test]
async fn test_edit_file_restricted_outside_workspace() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let tool = EditFileTool::new(&ws, true);

    let result = tool
        .execute(&serde_json::json!({
            "path": "/etc/hosts",
            "old_text": "localhost",
            "new_text": "modified"
        }))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("outside workspace"));
}

#[tokio::test]
async fn test_append_file_with_newlines() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let tool = AppendFileTool::new(&ws, false);
    let file_path = dir.path().join("newline.txt");

    let result = tool
        .execute(&serde_json::json!({
            "path": file_path.to_string_lossy(),
            "content": "line1\nline2\nline3"
        }))
        .await;
    assert!(!result.is_error);

    let content = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(content, "line1\nline2\nline3");
}

#[tokio::test]
async fn test_append_file_unicode_content() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let tool = AppendFileTool::new(&ws, false);
    let file_path = dir.path().join("unicode.txt");

    let result = tool
        .execute(&serde_json::json!({
            "path": file_path.to_string_lossy(),
            "content": "Hello! - test"
        }))
        .await;
    assert!(!result.is_error);

    let content = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert!(content.contains("Hello!"));
}

#[tokio::test]
async fn test_append_file_overwrite_existing() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let tool = AppendFileTool::new(&ws, false);
    let file_path = dir.path().join("existing.txt");

    // Pre-existing content
    tokio::fs::write(&file_path, "original").await.unwrap();

    let result = tool
        .execute(&serde_json::json!({
            "path": file_path.to_string_lossy(),
            "content": "_appended"
        }))
        .await;
    assert!(!result.is_error);

    let content = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(content, "original_appended");
}

#[tokio::test]
async fn test_edit_file_replace_entire_content() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let tool = EditFileTool::new(&ws, false);
    let file_path = dir.path().join("full.txt");
    tokio::fs::write(&file_path, "entire content")
        .await
        .unwrap();

    let result = tool
        .execute(&serde_json::json!({
            "path": file_path.to_string_lossy(),
            "old_text": "entire content",
            "new_text": "replaced entirely"
        }))
        .await;
    assert!(!result.is_error);

    let content = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(content, "replaced entirely");
}

// ============================================================
// W4a coverage gap closure (AppendFileTool::parameters and its
// restrict arm)
// ============================================================

#[test]
fn w4a_append_file_tool_parameters() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let tool = AppendFileTool::new(&ws, false);
    assert_eq!(tool.name(), "append_file");
    assert!(!tool.description().is_empty());
    let params = tool.parameters();
    assert_eq!(params["type"], "object");
    assert_eq!(params["properties"]["path"]["type"], "string");
    assert_eq!(params["properties"]["content"]["type"], "string");
    let required = params["required"].as_array().unwrap();
    assert!(required.contains(&serde_json::json!("path")));
    assert!(required.contains(&serde_json::json!("content")));
}

#[tokio::test]
async fn w4a_append_file_restrict_rejects_outside_workspace() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let tool = AppendFileTool::new(&ws, true);

    let result = tool
        .execute(&serde_json::json!({
            "path": "C:/Windows/w4a_target.txt",
            "content": "nope"
        }))
        .await;
    assert!(result.is_error);
    assert!(
        result.for_llm.contains("outside workspace"),
        "got: {}",
        result.for_llm
    );
}

// ===========================================================================
// S2 coverage (2026-08-26): resolve_path restrict=true with an in-workspace
// path (the non-rejected fall-through edge of both tools)
// ===========================================================================

#[test]
fn s2_edit_and_append_resolve_path_in_workspace_with_restrict() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let edit = EditFileTool::new(&ws, true);
    let append = AppendFileTool::new(&ws, true);

    // Relative in-workspace paths must pass the restrict check.
    let p1 = edit.resolve_path("notes/a.txt").unwrap();
    assert_eq!(p1, dir.path().join("notes").join("a.txt"));
    let p2 = append.resolve_path("logs/b.txt").unwrap();
    assert_eq!(p2, dir.path().join("logs").join("b.txt"));

    // An outside absolute path must still be rejected under restrict.
    let outside = TempDir::new().unwrap();
    let outside_file = outside.path().join("x.txt").to_string_lossy().to_string();
    assert!(edit.resolve_path(&outside_file).is_err());
    assert!(append.resolve_path(&outside_file).is_err());
}

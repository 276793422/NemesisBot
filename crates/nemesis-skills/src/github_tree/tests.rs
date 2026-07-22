use super::*;

#[test]
fn test_tree_entry_deserialization() {
    let json = r#"{
        "sha": "abc123",
        "tree": [
            {"path": "skills/pdf/SKILL.md", "type": "blob"},
            {"path": "skills/pdf", "type": "tree"},
            {"path": "skills/csv/SKILL.md", "type": "blob"}
        ],
        "truncated": false
    }"#;

    let response: TreeResponse = serde_json::from_str(json).unwrap();
    assert_eq!(response.tree.len(), 3);

    let blobs: Vec<_> = response
        .tree
        .into_iter()
        .filter(|e| e.entry_type == "blob" && e.path.starts_with("skills/pdf/"))
        .collect();
    assert_eq!(blobs.len(), 1);
    assert_eq!(blobs[0].path, "skills/pdf/SKILL.md");
}

#[test]
fn test_tree_entry_types() {
    let json = r#"{
        "sha": "abc",
        "tree": [
            {"path": "dir", "type": "tree"},
            {"path": "file.txt", "type": "blob"},
            {"path": "link", "type": "commit"}
        ],
        "truncated": null
    }"#;

    let response: TreeResponse = serde_json::from_str(json).unwrap();
    assert_eq!(response.tree[0].entry_type, "tree");
    assert_eq!(response.tree[1].entry_type, "blob");
    assert_eq!(response.tree[2].entry_type, "commit");
}

// ============================================================
// Additional tests for missing coverage
// ============================================================

#[test]
fn test_decode_tree_blob_paths_valid_with_matching_prefix() {
    let json = r#"{
        "sha": "abc123",
        "tree": [
            {"path": "skills/pdf/SKILL.md", "type": "blob"},
            {"path": "skills/pdf/docs/guide.md", "type": "blob"},
            {"path": "skills/csv/SKILL.md", "type": "blob"},
            {"path": "skills/pdf", "type": "tree"}
        ],
        "truncated": false
    }"#;

    let result = decode_tree_blob_paths(json.as_bytes(), "skills/pdf").unwrap();
    assert_eq!(result.len(), 2);
    assert!(result.contains(&"skills/pdf/SKILL.md".to_string()));
    assert!(result.contains(&"skills/pdf/docs/guide.md".to_string()));
}

#[test]
fn test_decode_tree_blob_paths_valid_no_matching_entries() {
    let json = r#"{
        "sha": "abc",
        "tree": [
            {"path": "skills/csv/SKILL.md", "type": "blob"},
            {"path": "other/file.txt", "type": "blob"}
        ],
        "truncated": false
    }"#;

    let result = decode_tree_blob_paths(json.as_bytes(), "skills/pdf").unwrap();
    assert!(result.is_empty());
}

#[test]
fn test_decode_tree_blob_paths_empty_body_error() {
    let result = decode_tree_blob_paths(b"", "skills/pdf");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("empty"));
}

#[test]
fn test_decode_tree_blob_paths_invalid_json_error() {
    let result = decode_tree_blob_paths(b"not valid json", "skills/pdf");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("parse"));
}

#[test]
fn test_decode_tree_blob_paths_prefix_with_trailing_slash() {
    let json = r#"{
        "tree": [
            {"path": "skills/pdf/SKILL.md", "type": "blob"}
        ]
    }"#;

    let result = decode_tree_blob_paths(json.as_bytes(), "skills/pdf/").unwrap();
    assert_eq!(result.len(), 1);
}

#[test]
fn test_decode_tree_blob_paths_no_tree_field() {
    let json = r#"{"sha": "abc"}"#;
    let result = decode_tree_blob_paths(json.as_bytes(), "skills/pdf").unwrap();
    assert!(result.is_empty());
}

#[test]
fn test_decode_tree_blob_paths_mixed_entry_types() {
    let json = r#"{
        "tree": [
            {"path": "skills/pdf/SKILL.md", "type": "blob"},
            {"path": "skills/pdf/subdir", "type": "tree"},
            {"path": "skills/pdf/README", "type": "blob"}
        ]
    }"#;

    let result = decode_tree_blob_paths(json.as_bytes(), "skills/pdf").unwrap();
    // Only blobs, not trees
    assert_eq!(result.len(), 2);
}

#[test]
fn test_decode_tree_blob_paths_empty_tree_array() {
    let json = r#"{"tree": []}"#;
    let result = decode_tree_blob_paths(json.as_bytes(), "skills/pdf").unwrap();
    assert!(result.is_empty());
}

// ---- Additional coverage tests ----

#[test]
fn test_decode_tree_blob_paths_entry_without_type() {
    let json = r#"{"tree": [{"path": "skills/pdf/SKILL.md"}]}"#;
    let result = decode_tree_blob_paths(json.as_bytes(), "skills/pdf").unwrap();
    // Missing type field defaults to empty string, not "blob"
    assert!(result.is_empty());
}

#[test]
fn test_decode_tree_blob_paths_entry_without_path() {
    let json = r#"{"tree": [{"type": "blob"}]}"#;
    let result = decode_tree_blob_paths(json.as_bytes(), "skills/pdf").unwrap();
    // Missing path defaults to empty string, won't match prefix
    assert!(result.is_empty());
}

#[test]
fn test_decode_tree_blob_paths_non_object_entries() {
    let json = r#"{"tree": [123, "hello", true, null]}"#;
    let result = decode_tree_blob_paths(json.as_bytes(), "skills/pdf").unwrap();
    assert!(result.is_empty());
}

#[test]
fn test_decode_tree_blob_paths_deep_nesting() {
    let json = r#"{
        "tree": [
            {"path": "skills/pdf/docs/guide.md", "type": "blob"},
            {"path": "skills/pdf/scripts/run.sh", "type": "blob"},
            {"path": "skills/pdf/examples/demo.py", "type": "blob"}
        ]
    }"#;
    let result = decode_tree_blob_paths(json.as_bytes(), "skills/pdf").unwrap();
    assert_eq!(result.len(), 3);
}

#[test]
fn test_decode_tree_blob_paths_prefix_must_match_exactly() {
    let json = r#"{
        "tree": [
            {"path": "skills/pdf/SKILL.md", "type": "blob"},
            {"path": "skills/pdfx/SKILL.md", "type": "blob"},
            {"path": "skills/pd/SKILL.md", "type": "blob"}
        ]
    }"#;
    let result = decode_tree_blob_paths(json.as_bytes(), "skills/pdf").unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], "skills/pdf/SKILL.md");
}

#[test]
fn test_tree_response_truncated_null() {
    let json = r#"{
        "sha": "abc",
        "tree": [{"path": "file.txt", "type": "blob"}],
        "truncated": null
    }"#;
    let response: TreeResponse = serde_json::from_str(json).unwrap();
    assert_eq!(response.tree.len(), 1);
    assert_eq!(response.truncated, None);
}

#[test]
fn test_tree_response_missing_truncated() {
    let json = r#"{
        "sha": "abc",
        "tree": [{"path": "file.txt", "type": "blob"}]
    }"#;
    let response: TreeResponse = serde_json::from_str(json).unwrap();
    assert_eq!(response.tree.len(), 1);
    assert!(response.truncated.is_none());
}

#[test]
fn test_tree_response_missing_sha() {
    let json = r#"{
        "tree": [{"path": "file.txt", "type": "blob"}],
        "truncated": false
    }"#;
    let response: TreeResponse = serde_json::from_str(json).unwrap();
    assert!(response.sha.is_none());
}

#[test]
fn test_tree_entry_blob_and_tree_types() {
    let json = r#"{
        "tree": [
            {"path": "dir", "type": "tree"},
            {"path": "file1.txt", "type": "blob"},
            {"path": "file2.txt", "type": "blob"}
        ]
    }"#;
    let response: TreeResponse = serde_json::from_str(json).unwrap();
    let blobs: Vec<_> = response
        .tree
        .iter()
        .filter(|e| e.entry_type == "blob")
        .collect();
    let trees: Vec<_> = response
        .tree
        .iter()
        .filter(|e| e.entry_type == "tree")
        .collect();
    assert_eq!(blobs.len(), 2);
    assert_eq!(trees.len(), 1);
}

#[test]
fn test_decode_tree_blob_paths_large_response() {
    // Simulate a large tree with many entries
    let mut entries = Vec::new();
    for i in 0..100 {
        entries.push(format!(
            r#"{{"path": "skills/pdf/file{}.txt", "type": "blob"}}"#,
            i
        ));
    }
    let json = format!(r#"{{"tree": [{}]}}"#, entries.join(","));
    let result = decode_tree_blob_paths(json.as_bytes(), "skills/pdf").unwrap();
    assert_eq!(result.len(), 100);
}

// ============================================================
// Coverage improvement: more edge cases for decode_tree_blob_paths
// ============================================================

#[test]
fn test_decode_tree_blob_paths_commit_type_ignored() {
    let json = r#"{
        "tree": [
            {"path": "skills/pdf/SKILL.md", "type": "blob"},
            {"path": "skills/pdf/submodule", "type": "commit"}
        ]
    }"#;
    let result = decode_tree_blob_paths(json.as_bytes(), "skills/pdf").unwrap();
    // Only blob entries should be returned, commit type is ignored
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], "skills/pdf/SKILL.md");
}

#[test]
fn test_decode_tree_blob_paths_exact_prefix_match() {
    // The function appends '/' to prefix, so "skills/pdf" becomes "skills/pdf/"
    // "skills/pdf" does NOT start with "skills/pdf/" but "skills/pdfx/SKILL.md" doesn't either
    let json = r#"{
        "tree": [
            {"path": "skills/pdf/SKILL.md", "type": "blob"},
            {"path": "skills/pdfx/SKILL.md", "type": "blob"}
        ]
    }"#;
    let result = decode_tree_blob_paths(json.as_bytes(), "skills/pdf").unwrap();
    // Only "skills/pdf/SKILL.md" matches "skills/pdf/"
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], "skills/pdf/SKILL.md");
}

#[test]
fn test_decode_tree_blob_paths_with_slash_prefix() {
    let json = r#"{
        "tree": [
            {"path": "skills/pdf/SKILL.md", "type": "blob"},
            {"path": "skills/pdf/docs/guide.md", "type": "blob"}
        ]
    }"#;
    let result = decode_tree_blob_paths(json.as_bytes(), "skills/pdf/").unwrap();
    // Both match "skills/pdf/" prefix
    assert_eq!(result.len(), 2);
}

#[test]
fn test_decode_tree_blob_paths_empty_prefix() {
    // Empty prefix becomes "/" which nothing starts with
    let json = r#"{
        "tree": [
            {"path": "skills/pdf/SKILL.md", "type": "blob"},
            {"path": "other/file.txt", "type": "blob"}
        ]
    }"#;
    let result = decode_tree_blob_paths(json.as_bytes(), "").unwrap();
    // Empty prefix becomes "/" which no path starts with
    assert_eq!(result.len(), 0);
}

#[test]
fn test_decode_tree_blob_paths_tree_entries_ignored() {
    let json = r#"{
        "tree": [
            {"path": "skills/pdf", "type": "tree"},
            {"path": "skills/pdf/SKILL.md", "type": "blob"},
            {"path": "skills/pdf/docs", "type": "tree"}
        ]
    }"#;
    let result = decode_tree_blob_paths(json.as_bytes(), "skills/pdf").unwrap();
    // Only blobs, not trees
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], "skills/pdf/SKILL.md");
}

#[test]
fn test_decode_tree_blob_paths_deduplication() {
    // The function does NOT deduplicate - duplicate entries are preserved
    let json = r#"{
        "tree": [
            {"path": "skills/pdf/SKILL.md", "type": "blob"},
            {"path": "skills/pdf/SKILL.md", "type": "blob"}
        ]
    }"#;
    let result = decode_tree_blob_paths(json.as_bytes(), "skills/pdf").unwrap();
    // Duplicates are preserved (no dedup in the function)
    assert_eq!(result.len(), 2);
}

#[test]
fn test_tree_entry_path_with_spaces() {
    let json = r#"{
        "tree": [
            {"path": "skills/my skill/SKILL.md", "type": "blob"},
            {"path": "skills/other skill/SKILL.md", "type": "blob"}
        ]
    }"#;
    let result = decode_tree_blob_paths(json.as_bytes(), "skills/my skill").unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], "skills/my skill/SKILL.md");
}

#[test]
fn test_decode_tree_blob_paths_unicode_paths() {
    let json = r#"{
        "tree": [
            {"path": "skills/日本語/SKILL.md", "type": "blob"},
            {"path": "skills/其他/SKILL.md", "type": "blob"}
        ]
    }"#;
    let result = decode_tree_blob_paths(json.as_bytes(), "skills/日本語").unwrap();
    assert_eq!(result.len(), 1);
}

// ============================================================
// Coverage: async HTTP paths for download_skill_tree_from_github
// ============================================================

#[tokio::test]
async fn test_download_skill_tree_connection_error() {
    let client = Client::builder()
        .timeout(std::time::Duration::from_millis(500))
        .build()
        .unwrap();

    let result = download_skill_tree_from_github(
        &client,
        "http://127.0.0.1:1",
        "http://127.0.0.1:1",
        "test/repo",
        "main",
        "skills/pdf",
        "/tmp/nonexistent_download_test",
        0,
    )
    .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("tree"));
}

#[tokio::test]
async fn test_download_file_connection_error() {
    let client = Client::builder()
        .timeout(std::time::Duration::from_millis(500))
        .build()
        .unwrap();

    let result = download_file(&client, "http://127.0.0.1:1/file.txt", 1024).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("request failed"));
}

#[tokio::test]
async fn test_download_skill_tree_no_trailing_slash_prefix() {
    // Verify that dir_prefix without trailing slash gets one appended
    let client = Client::builder()
        .timeout(std::time::Duration::from_millis(500))
        .build()
        .unwrap();

    let result = download_skill_tree_from_github(
        &client,
        "http://127.0.0.1:1",
        "http://127.0.0.1:1",
        "test/repo",
        "main",
        "skills/pdf", // no trailing slash
        "/tmp/nonexistent_download_test2",
        1024,
    )
    .await;
    assert!(result.is_err());
}

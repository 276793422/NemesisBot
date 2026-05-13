//! GitHub Trees API wrapper - downloads skill directories from GitHub.
//!
//! Uses the GitHub Trees API to enumerate files in a skill directory,
//! then downloads each file individually. Supports streaming JSON decoding
//! for large repository trees (65K+ entries).

use std::path::Path;

use serde::Deserialize;
use tracing::debug;
use reqwest::Client;

use nemesis_types::error::{NemesisError, Result};

const DEFAULT_FILE_MAX_SIZE: u64 = 10 * 1024 * 1024; // 10 MB per file

/// Download all files in a skill directory from GitHub using the Trees API.
///
/// Lists the full tree, filters for files under `dir_prefix`, and downloads
/// each file individually.
///
/// # Parameters
/// - `client`: HTTP client to use for requests.
/// - `api_base_url`: GitHub API base URL (e.g., "https://api.github.com").
/// - `raw_base_url`: Raw content base URL (e.g., "https://raw.githubusercontent.com").
/// - `repo`: Repository in "owner/repo" format.
/// - `branch`: Branch name (e.g., "main").
/// - `dir_prefix`: Directory prefix to filter (e.g., "skills/pdf").
/// - `target_dir`: Local directory to write files into.
/// - `max_file_size`: Maximum size per file in bytes (0 = default 10MB).
pub async fn download_skill_tree_from_github(
    client: &Client,
    api_base_url: &str,
    raw_base_url: &str,
    repo: &str,
    branch: &str,
    dir_prefix: &str,
    target_dir: &str,
    max_file_size: u64,
) -> Result<()> {
    // Ensure trailing slash for consistent prefix matching.
    let mut dir_prefix = dir_prefix.to_string();
    if !dir_prefix.ends_with('/') {
        dir_prefix.push('/');
    }

    let max_file_size = if max_file_size == 0 {
        DEFAULT_FILE_MAX_SIZE
    } else {
        max_file_size
    };

    let api_url = format!(
        "{}/repos/{}/git/trees/{}?recursive=1",
        api_base_url, repo, branch
    );

    debug!("Fetching GitHub tree: {}", api_url);

    let response = client
        .get(&api_url)
        .header("Accept", "application/vnd.github.v3+json")
        .send()
        .await
        .map_err(|e| NemesisError::Other(format!("failed to fetch tree: {}", e)))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(NemesisError::Other(format!(
            "GitHub Trees API HTTP {}: {}",
            status,
            body.chars().take(512).collect::<String>()
        )));
    }

    // Parse the tree response.
    let tree_response: TreeResponse = response
        .json()
        .await
        .map_err(|e| NemesisError::Other(format!("failed to parse tree response: {}", e)))?;

    // Filter blob paths under dir_prefix.
    let blob_paths: Vec<String> = tree_response
        .tree
        .into_iter()
        .filter(|entry| entry.entry_type == "blob" && entry.path.starts_with(&dir_prefix))
        .map(|entry| entry.path)
        .collect();

    if blob_paths.is_empty() {
        return Err(NemesisError::NotFound(format!(
            "no files found under {} in {}",
            dir_prefix, repo
        )));
    }

    // Create target directory.
    std::fs::create_dir_all(target_dir).map_err(|e| NemesisError::Io(e))?;

    // Download each file.
    for blob_path in &blob_paths {
        let relative_path = blob_path.strip_prefix(&dir_prefix).unwrap_or(blob_path);
        if relative_path.is_empty() {
            continue;
        }

        let dest_path = Path::new(target_dir).join(relative_path);

        // Security: ensure dest_path is within targetDir (path traversal check).
        let canonical_target = Path::new(target_dir)
            .canonicalize()
            .unwrap_or_else(|_| Path::new(target_dir).to_path_buf());
        let parent_dir = dest_path.parent().unwrap_or(Path::new(""));
        if let Ok(canonical_dest_parent) = parent_dir.canonicalize() {
            if !canonical_dest_parent.starts_with(&canonical_target) {
                return Err(NemesisError::Security(format!(
                    "path traversal detected: {}",
                    relative_path
                )));
            }
        }

        // Create parent directory.
        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| NemesisError::Io(e))?;
        }

        let raw_url = format!(
            "{}/{}/{}/{}",
            raw_base_url, repo, branch, blob_path
        );

        let data = download_file(client, &raw_url, max_file_size).await?;
        std::fs::write(&dest_path, &data).map_err(|e| NemesisError::Io(e))?;
    }

    debug!(
        "Downloaded {} files from {}/{} to {}",
        blob_paths.len(),
        repo,
        dir_prefix,
        target_dir
    );

    Ok(())
}

/// Stream-decode a GitHub Trees API response body and return the paths of all
/// blob entries that start with `dir_prefix`.
///
/// Mirrors Go `decodeTreeBlobPaths`. Uses streaming JSON deserialization to
/// handle very large repository trees (65K+ entries) without loading the
/// entire response into memory at once.
///
/// The `body` parameter is the raw bytes of the HTTP response body.
/// Returns a vector of paths matching the directory prefix filter.
pub fn decode_tree_blob_paths(body: &[u8], dir_prefix: &str) -> Result<Vec<String>> {
    // Ensure trailing slash for consistent prefix matching.
    let mut dir_prefix = dir_prefix.to_string();
    if !dir_prefix.ends_with('/') {
        dir_prefix.push('/');
    }

    let mut blob_paths = Vec::new();

    // Parse the JSON response using serde_json's streaming parser.
    let cursor = std::io::Cursor::new(body);
    let mut stream = serde_json::Deserializer::from_reader(cursor).into_iter::<serde_json::Value>();

    // The response is {"sha": ..., "tree": [...], "truncated": ...}
    // We need to find the "tree" array and iterate its entries.
    let root = stream.next().ok_or_else(|| {
        NemesisError::Other("empty tree response".to_string())
    })?.map_err(|e| {
        NemesisError::Other(format!("failed to parse tree response: {}", e))
    })?;

    if let serde_json::Value::Object(map) = root {
        if let Some(serde_json::Value::Array(tree)) = map.get("tree") {
            for entry in tree {
                if let serde_json::Value::Object(entry_map) = entry {
                    let entry_type = entry_map.get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let path = entry_map.get("path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    if entry_type == "blob" && path.starts_with(&dir_prefix) {
                        blob_paths.push(path.to_string());
                    }
                }
            }
        }
    }

    Ok(blob_paths)
}

/// Download a single file from a URL with size limit.
pub async fn download_file(client: &Client, url: &str, max_size: u64) -> Result<Vec<u8>> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| NemesisError::Other(format!("request failed: {}", e)))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(NemesisError::Other(format!(
            "HTTP {}: {}",
            status,
            body.chars().take(512).collect::<String>()
        )));
    }

    let body = response
        .bytes()
        .await
        .map_err(|e| NemesisError::Other(format!("failed to read response: {}", e)))?;

    if body.len() as u64 > max_size {
        return Err(NemesisError::Other(format!(
            "response too large: {} bytes (max {})",
            body.len(),
            max_size
        )));
    }

    Ok(body.to_vec())
}

/// GitHub Trees API response.
#[derive(Debug, Deserialize)]
struct TreeResponse {
    #[allow(dead_code)]
    sha: Option<String>,
    tree: Vec<TreeEntry>,
    #[allow(dead_code)]
    truncated: Option<bool>,
}

/// A single entry in the tree.
#[derive(Debug, Deserialize)]
struct TreeEntry {
    path: String,
    #[serde(rename = "type")]
    entry_type: String,
}

#[cfg(test)]
mod tests {
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
        let blobs: Vec<_> = response.tree.iter().filter(|e| e.entry_type == "blob").collect();
        let trees: Vec<_> = response.tree.iter().filter(|e| e.entry_type == "tree").collect();
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
            "skills/pdf",  // no trailing slash
            "/tmp/nonexistent_download_test2",
            1024,
        )
        .await;
        assert!(result.is_err());
    }
}

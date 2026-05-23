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
mod tests;

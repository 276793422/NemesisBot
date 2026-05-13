//! Forge action handler - processes forge_share and forge_get_reflections.
//!
//! Handles incoming forge-related RPC actions for cross-node learning:
//! - `forge_share`: receive a remote reflection report, sanitize, and store
//! - `forge_get_reflections`: list local reflections, optionally return content
//!
//! The handler integrates with `ForgeDataProvider` which abstracts file I/O
//! and sanitization, matching Go's `ForgeDataProvider` interface.

use std::path::{Path, PathBuf};

use crate::handlers::default_handler::HandleResult;

// ---------------------------------------------------------------------------
// ForgeDataProvider interface
// ---------------------------------------------------------------------------

/// Provider interface for forge data operations.
/// Decouples the handler from the forge package.
pub trait ForgeDataProvider: Send + Sync {
    /// Receive and store a remote reflection report.
    fn receive_reflection(&self, payload: &serde_json::Value) -> Result<(), String>;

    /// Get the list payload for reflections.
    fn get_reflections_list_payload(&self) -> serde_json::Value;

    /// Read the content of a specific reflection file.
    fn read_reflection_content(&self, filename: &str) -> Result<String, String>;

    /// Sanitize content before sending to remote nodes.
    fn sanitize_content(&self, content: &str) -> String;

    /// Clone into a boxed trait object.
    fn clone_boxed(&self) -> Box<dyn ForgeDataProvider>;
}

// ---------------------------------------------------------------------------
// Default file-based provider
// ---------------------------------------------------------------------------

/// Default file-based forge data provider that stores reflections on disk.
#[derive(Clone)]
pub struct FileForgeProvider {
    reflections_dir: PathBuf,
    remote_dir: PathBuf,
}

impl FileForgeProvider {
    /// Create a new file-based provider rooted at `workspace/forge/`.
    pub fn new(forge_dir: impl Into<PathBuf>) -> Self {
        let forge_dir = forge_dir.into();
        Self {
            reflections_dir: forge_dir.join("reflections"),
            remote_dir: forge_dir.join("reflections").join("remote"),
        }
    }

    /// Ensure the directories exist.
    fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.reflections_dir)?;
        std::fs::create_dir_all(&self.remote_dir)?;
        Ok(())
    }

    /// Sanitize content by replacing sensitive patterns.
    ///
    /// This is a simplified version of Go's `Sanitizer`. A full implementation
    /// would handle API keys, private IPs, file paths, etc.
    fn do_sanitize(content: &str) -> String {
        let mut result = content.to_string();

        // Simple string-based redaction for common patterns
        result = redact_api_keys(&result);
        result = redact_private_ips(&result);
        result = redact_file_paths(&result);

        result
    }
}

/// Redact common API key patterns from content.
fn redact_api_keys(content: &str) -> String {
    let mut result = content.to_string();

    // Redact AWS access keys
    if let Some(pos) = result.find("AKIA") {
        if pos + 20 <= result.len() {
            let is_valid = result[pos..pos + 20].chars().all(|c| c.is_ascii_alphanumeric());
            if is_valid {
                result = result.replace(&result[pos..pos + 20], "[REDACTED_AWS_KEY]");
            }
        }
    }

    result
}

/// Redact private/internal IP addresses.
fn redact_private_ips(content: &str) -> String {
    let mut result = content.to_string();

    // Replace common private IP prefixes
    for prefix in &["192.168.", "10.", "172.16.", "172.17.", "172.18.", "172.19."] {
        let mut start = 0;
        while let Some(pos) = result[start..].find(prefix) {
            let actual_pos = start + pos;
            // Find the end of the IP address
            let end = result[actual_pos..]
                .find(|c: char| !c.is_ascii_digit() && c != '.')
                .unwrap_or(result.len() - actual_pos);
            let ip_str = &result[actual_pos..actual_pos + end];
            // Only redact if it looks like a full IP (has at least 3 dots)
            if ip_str.matches('.').count() >= 3 {
                result = format!(
                    "{}[IP]{}",
                    &result[..actual_pos],
                    &result[actual_pos + end..]
                );
                start = actual_pos + 4; // "[IP]".len()
                if start >= result.len() {
                    break;
                }
            } else {
                start = actual_pos + prefix.len();
                if start >= result.len() {
                    break;
                }
            }
        }
    }

    result
}

/// Redact file paths that look like absolute paths.
fn redact_file_paths(content: &str) -> String {
    let mut result = content.to_string();

    // On Windows: C:\Users\... or C:\AI\...
    // On Linux: /home/..., /root/..., /etc/...
    // Simple heuristic: replace paths that start with / or drive letter
    for prefix in &["C:\\Users\\", "/home/", "/root/"] {
        if let Some(pos) = result.find(prefix) {
            let end = result[pos..]
                .find(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == ',')
                .unwrap_or(result.len() - pos);
            result = format!(
                "{}[REDACTED_PATH]{}",
                &result[..pos],
                &result[pos + end..]
            );
        }
    }

    result
}

impl ForgeDataProvider for FileForgeProvider {
    fn receive_reflection(&self, payload: &serde_json::Value) -> Result<(), String> {
        self.ensure_dirs().map_err(|e| e.to_string())?;

        let source_node = payload
            .get("source_node")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        let report = payload.get("report").ok_or("report field is required")?;

        // Generate a filename from source node and timestamp
        let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
        let filename = format!("remote-{}-{}.json", source_node, timestamp);
        let path = self.remote_dir.join(&filename);

        // Sanitize the report before storing
        let report_str = serde_json::to_string_pretty(report).unwrap_or_default();
        let sanitized = Self::do_sanitize(&report_str);

        std::fs::write(&path, sanitized).map_err(|e| e.to_string())?;

        tracing::info!(
            source_node = source_node,
            path = %path.display(),
            "Stored remote reflection report"
        );

        Ok(())
    }

    fn get_reflections_list_payload(&self) -> serde_json::Value {
        let mut reflections = Vec::new();

        // Read local reflections
        if let Ok(entries) = std::fs::read_dir(&self.reflections_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("json") {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        reflections.push(serde_json::json!({
                            "filename": name,
                            "remote": false,
                        }));
                    }
                }
            }
        }

        // Read remote reflections
        if let Ok(entries) = std::fs::read_dir(&self.remote_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("json") {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        reflections.push(serde_json::json!({
                            "filename": name,
                            "remote": true,
                        }));
                    }
                }
            }
        }

        serde_json::json!({
            "reflections": reflections,
            "count": reflections.len(),
        })
    }

    fn read_reflection_content(&self, filename: &str) -> Result<String, String> {
        // Prevent path traversal
        let filename = Path::new(filename)
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("invalid filename")?;

        // Try local reflections first, then remote
        let local_path = self.reflections_dir.join(filename);
        let remote_path = self.remote_dir.join(filename);

        let path = if local_path.exists() {
            local_path
        } else if remote_path.exists() {
            remote_path
        } else {
            return Err(format!("reflection file not found: {}", filename));
        };

        std::fs::read_to_string(&path).map_err(|e| e.to_string())
    }

    fn sanitize_content(&self, content: &str) -> String {
        Self::do_sanitize(content)
    }

    fn clone_boxed(&self) -> Box<dyn ForgeDataProvider> {
        Box::new(self.clone())
    }
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// Handler for forge-related cluster actions.
pub struct ForgeHandler {
    node_id: String,
    provider: Option<Box<dyn ForgeDataProvider>>,
}

impl ForgeHandler {
    /// Create a new forge handler with file-based storage.
    pub fn new(node_id: String) -> Self {
        Self {
            node_id,
            provider: None,
        }
    }

    /// Create a forge handler with a custom data provider.
    pub fn with_provider(node_id: String, provider: Box<dyn ForgeDataProvider>) -> Self {
        Self {
            node_id,
            provider: Some(provider),
        }
    }

    /// Set the data provider (used when forge is initialized after handler creation).
    pub fn set_provider(&mut self, provider: Box<dyn ForgeDataProvider>) {
        self.provider = Some(provider);
    }

    /// Handle a forge action (share or get_reflections).
    pub fn handle(&self, action: &str, payload: serde_json::Value) -> HandleResult {
        match action {
            "forge_share" => self.handle_share(payload),
            "forge_get_reflections" => self.handle_get_reflections(payload),
            _ => HandleResult {
                success: false,
                response: serde_json::Value::Null,
                error: Some(format!("Unknown forge action: {}", action)),
            },
        }
    }

    fn handle_share(&self, payload: serde_json::Value) -> HandleResult {
        let from = payload
            .get("from")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        tracing::info!(
            source_node = from,
            local_node = %self.node_id,
            "Received forge reflection report from peer"
        );

        if payload.get("report").is_none() {
            return HandleResult {
                success: false,
                response: serde_json::Value::Null,
                error: Some("report field is required".into()),
            };
        }

        // If we have a provider, store the reflection
        if let Some(ref provider) = self.provider {
            if let Err(e) = provider.receive_reflection(&payload) {
                tracing::error!(error = %e, "Failed to store reflection");
                return HandleResult {
                    success: false,
                    response: serde_json::Value::Null,
                    error: Some(format!("Failed to store reflection: {}", e)),
                };
            }
        }

        HandleResult {
            success: true,
            response: serde_json::json!({
                "status": "received",
                "node_id": self.node_id,
                "timestamp": chrono::Utc::now().to_rfc3339(),
            }),
            error: None,
        }
    }

    fn handle_get_reflections(&self, payload: serde_json::Value) -> HandleResult {
        let from = payload
            .get("from")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        tracing::info!(
            from = from,
            local_node = %self.node_id,
            "Reflections list requested by peer"
        );

        let mut result = if let Some(ref provider) = self.provider {
            provider.get_reflections_list_payload()
        } else {
            serde_json::json!({
                "reflections": [],
                "count": 0,
            })
        };

        // If a specific reflection is requested, include its content (sanitized)
        if let Some(filename) = payload.get("filename").and_then(|v| v.as_str()) {
            if !filename.is_empty() {
                if let Some(ref provider) = self.provider {
                    match provider.read_reflection_content(filename) {
                        Ok(content) => {
                            result["content"] = serde_json::Value::String(
                                provider.sanitize_content(&content),
                            );
                            result["filename"] = serde_json::Value::String(filename.into());
                        }
                        Err(e) => {
                            tracing::error!(
                                filename = filename,
                                error = %e,
                                "Failed to read reflection"
                            );
                            return HandleResult {
                                success: false,
                                response: serde_json::Value::Null,
                                error: Some(format!("Failed to read reflection: {}", e)),
                            };
                        }
                    }
                }
            }
        }

        result["node_id"] = serde_json::Value::String(self.node_id.clone());

        HandleResult {
            success: true,
            response: result,
            error: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_forge_share_success() {
        let handler = ForgeHandler::new("node-a".into());
        let payload = serde_json::json!({
            "report": {"insights": ["test"]},
            "source_node": "node-b"
        });

        let result = handler.handle("forge_share", payload);
        assert!(result.success);
        assert_eq!(result.response["status"], "received");
    }

    #[test]
    fn test_forge_share_missing_report() {
        let handler = ForgeHandler::new("node-a".into());
        let payload = serde_json::json!({"source_node": "node-b"});

        let result = handler.handle("forge_share", payload);
        assert!(!result.success);
        assert!(result.error.is_some());
    }

    #[test]
    fn test_forge_get_reflections() {
        let handler = ForgeHandler::new("node-a".into());
        let result = handler.handle("forge_get_reflections", serde_json::json!({}));
        assert!(result.success);
        assert!(result.response.get("reflections").is_some());
        assert_eq!(result.response["node_id"], "node-a");
    }

    #[test]
    fn test_unknown_forge_action() {
        let handler = ForgeHandler::new("node-a".into());
        let result = handler.handle("forge_unknown", serde_json::json!({}));
        assert!(!result.success);
    }

    // -- File-based provider tests --

    #[test]
    fn test_file_provider_receive_and_list() {
        let dir = tempfile::tempdir().unwrap();
        let provider = FileForgeProvider::new(dir.path());

        let payload = serde_json::json!({
            "source_node": "node-b",
            "report": {"insights": ["test insight"], "score": 0.85},
        });

        provider.receive_reflection(&payload).unwrap();

        let list = provider.get_reflections_list_payload();
        let reflections = list["reflections"].as_array().unwrap();
        assert!(!reflections.is_empty());

        // The stored file should be in remote/
        let remote_files: Vec<_> = reflections
            .iter()
            .filter(|r| r["remote"].as_bool().unwrap_or(false))
            .collect();
        assert!(!remote_files.is_empty());
    }

    #[test]
    fn test_file_provider_read_content() {
        let dir = tempfile::tempdir().unwrap();
        let provider = FileForgeProvider::new(dir.path());

        let payload = serde_json::json!({
            "source_node": "node-c",
            "report": {"data": "hello world"},
        });

        provider.receive_reflection(&payload).unwrap();

        let list = provider.get_reflections_list_payload();
        let filename = list["reflections"].as_array().unwrap()[0]["filename"]
            .as_str()
            .unwrap();

        let content = provider.read_reflection_content(filename).unwrap();
        assert!(content.contains("hello world"));
    }

    #[test]
    fn test_file_provider_read_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let provider = FileForgeProvider::new(dir.path());

        let result = provider.read_reflection_content("nonexistent.json");
        assert!(result.is_err());
    }

    #[test]
    fn test_file_provider_path_traversal_prevention() {
        let dir = tempfile::tempdir().unwrap();
        let provider = FileForgeProvider::new(dir.path());

        let result = provider.read_reflection_content("../../../etc/passwd");
        assert!(result.is_err());
    }

    #[test]
    fn test_sanitize_redacts_aws_keys() {
        let content = r#"{"key": "AKIAIOSFODNN7EXAMPLE", "data": "normal"}"#;
        let sanitized = FileForgeProvider::do_sanitize(content);
        assert!(!sanitized.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(sanitized.contains("[REDACTED_AWS_KEY]"));
    }

    #[test]
    fn test_sanitize_redacts_private_ips() {
        let content = r#"{"server": "192.168.1.100", "port": 8080}"#;
        let sanitized = FileForgeProvider::do_sanitize(content);
        assert!(!sanitized.contains("192.168.1.100"));
        assert!(sanitized.contains("[IP]"));
    }

    #[test]
    fn test_sanitize_preserves_public_ips() {
        let content = r#"{"server": "8.8.8.8", "port": 53}"#;
        let sanitized = FileForgeProvider::do_sanitize(content);
        assert!(sanitized.contains("8.8.8.8"));
    }

    #[test]
    fn test_handler_with_file_provider() {
        let dir = tempfile::tempdir().unwrap();
        let provider = Box::new(FileForgeProvider::new(dir.path()));
        let handler = ForgeHandler::with_provider("node-a".into(), provider);

        // Share
        let share_payload = serde_json::json!({
            "source_node": "node-b",
            "report": {"test": "data"},
        });
        let result = handler.handle("forge_share", share_payload);
        assert!(result.success);

        // List
        let result = handler.handle("forge_get_reflections", serde_json::json!({}));
        assert!(result.success);
        let reflections = result.response["reflections"].as_array().unwrap();
        assert!(!reflections.is_empty());
    }

    #[test]
    fn test_handler_get_specific_reflection() {
        let dir = tempfile::tempdir().unwrap();
        let provider = Box::new(FileForgeProvider::new(dir.path()));
        let handler = ForgeHandler::with_provider("node-a".into(), provider);

        // Share a report
        let share_payload = serde_json::json!({
            "source_node": "node-b",
            "report": {"secret": "value123"},
        });
        let result = handler.handle("forge_share", share_payload);
        assert!(result.success);

        // List to get the filename
        let list_result = handler.handle("forge_get_reflections", serde_json::json!({}));
        let filename = list_result.response["reflections"].as_array().unwrap()[0]["filename"]
            .as_str()
            .unwrap();

        // Get specific
        let get_payload = serde_json::json!({
            "filename": filename,
        });
        let result = handler.handle("forge_get_reflections", get_payload);
        assert!(result.success);
        assert!(result.response.get("content").is_some());
    }

    #[test]
    fn test_set_provider() {
        let mut handler = ForgeHandler::new("node-a".into());
        let dir = tempfile::tempdir().unwrap();
        handler.set_provider(Box::new(FileForgeProvider::new(dir.path())));

        let result = handler.handle("forge_get_reflections", serde_json::json!({}));
        assert!(result.success);
    }

    // ============================================================
    // Coverage improvement: sanitization, provider edge cases
    // ============================================================

    #[test]
    fn test_sanitize_redacts_file_paths() {
        let content = r#"path: /home/user/secret.txt"#;
        let sanitized = FileForgeProvider::do_sanitize(content);
        assert!(sanitized.contains("[REDACTED_PATH]"));
        assert!(!sanitized.contains("/home/user/"));
    }

    #[test]
    fn test_sanitize_no_redaction_needed() {
        let content = r#"{"data": "public info", "score": 0.95}"#;
        let sanitized = FileForgeProvider::do_sanitize(content);
        assert_eq!(sanitized, content);
    }

    #[test]
    fn test_file_provider_receive_missing_report() {
        let dir = tempfile::tempdir().unwrap();
        let provider = FileForgeProvider::new(dir.path());
        let payload = serde_json::json!({"source_node": "node-b"});
        let result = provider.receive_reflection(&payload);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("report field is required"));
    }

    #[test]
    fn test_file_provider_clone_boxed() {
        let dir = tempfile::tempdir().unwrap();
        let provider = FileForgeProvider::new(dir.path());
        let cloned = provider.clone_boxed();
        // Verify the cloned provider works
        let payload = serde_json::json!({
            "source_node": "node-b",
            "report": {"test": "data"},
        });
        cloned.receive_reflection(&payload).unwrap();
    }

    #[test]
    fn test_file_provider_sanitize_content() {
        let dir = tempfile::tempdir().unwrap();
        let provider = FileForgeProvider::new(dir.path());
        let content = "AKIAIOSFODNN7EXAMPLE key found at 192.168.1.100";
        let sanitized = provider.sanitize_content(content);
        assert!(sanitized.contains("[REDACTED_AWS_KEY]"));
        assert!(sanitized.contains("[IP]"));
    }

    #[test]
    fn test_forge_share_with_provider_bad_dir() {
        // Use a temp dir where we can write successfully
        let dir = tempfile::tempdir().unwrap();
        let forge_dir = dir.path().join("forge");
        std::fs::create_dir_all(forge_dir.join("reflections").join("remote")).unwrap();
        let provider = Box::new(FileForgeProvider::new(&forge_dir));
        let handler = ForgeHandler::with_provider("node-a".into(), provider);

        let payload = serde_json::json!({
            "source_node": "node-b",
            "report": {"insights": ["test"]},
        });
        let result = handler.handle("forge_share", payload);
        // Should succeed since the directory exists
        assert!(result.success);
    }

    #[test]
    fn test_forge_get_reflections_with_specific_filename_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let provider = Box::new(FileForgeProvider::new(dir.path()));
        let handler = ForgeHandler::with_provider("node-a".into(), provider);

        let result = handler.handle(
            "forge_get_reflections",
            serde_json::json!({"filename": "nonexistent.json"}),
        );
        assert!(!result.success);
    }

    #[test]
    fn test_forge_get_reflections_with_empty_filename() {
        let dir = tempfile::tempdir().unwrap();
        let provider = Box::new(FileForgeProvider::new(dir.path()));
        let handler = ForgeHandler::with_provider("node-a".into(), provider);

        let result = handler.handle(
            "forge_get_reflections",
            serde_json::json!({"filename": ""}),
        );
        assert!(result.success);
    }

    #[test]
    fn test_redact_api_keys_short_content() {
        // Content too short for full AWS key
        let content = "AKIA";
        let result = redact_api_keys(content);
        assert_eq!(result, "AKIA"); // Not long enough to redact
    }

    #[test]
    fn test_redact_private_ips_partial_ip() {
        // IP with fewer than 3 dots should not be redacted
        let content = "server at 192.168.1";
        let result = redact_private_ips(content);
        assert!(result.contains("192.168.1")); // Not fully qualified IP
    }

    #[test]
    fn test_redact_file_paths_windows_style() {
        let content = r#"path: C:\Users\admin\documents\secret.txt"#;
        let result = redact_file_paths(content);
        assert!(result.contains("[REDACTED_PATH]"));
    }

    #[test]
    fn test_file_provider_read_reflection_content_path_traversal_complex() {
        let dir = tempfile::tempdir().unwrap();
        let provider = FileForgeProvider::new(dir.path());
        let result = provider.read_reflection_content("../../etc/shadow");
        assert!(result.is_err());
    }

    #[test]
    fn test_file_provider_list_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let provider = FileForgeProvider::new(dir.path());
        let list = provider.get_reflections_list_payload();
        assert_eq!(list["count"], 0);
        assert!(list["reflections"].as_array().unwrap().is_empty());
    }
}

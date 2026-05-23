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
            "[ForgeHandler] Stored remote reflection report"
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
            "[ForgeHandler] Received forge reflection report from peer"
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
                tracing::error!(error = %e, "[ForgeHandler] Failed to store reflection");
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
            "[ForgeHandler] Reflections list requested by peer"
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
                                "[ForgeHandler] Failed to read reflection"
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
mod tests;

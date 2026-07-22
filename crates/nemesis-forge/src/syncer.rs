//! Syncer - cluster reflection sharing via bridge.
//!
//! Shares local reflection reports with online peers and receives
//! remote reports for cross-node learning.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::bridge::ClusterForgeBridge;
use crate::sanitizer::Sanitizer;

/// The syncer handles sharing reflection reports across cluster nodes.
pub struct Syncer {
    bridge: Arc<dyn ClusterForgeBridge>,
    sanitizer: Sanitizer,
    enabled: bool,
    forge_dir: PathBuf,
}

impl Syncer {
    /// Create a new syncer with the given bridge.
    pub fn new(bridge: Arc<dyn ClusterForgeBridge>) -> Self {
        Self {
            bridge,
            sanitizer: Sanitizer::new(),
            enabled: true,
            forge_dir: PathBuf::new(),
        }
    }

    /// Create a new syncer with a forge directory for file-based operations.
    pub fn with_forge_dir(bridge: Arc<dyn ClusterForgeBridge>, forge_dir: PathBuf) -> Self {
        Self {
            bridge,
            sanitizer: Sanitizer::new(),
            enabled: true,
            forge_dir,
        }
    }

    /// Replace the bridge after construction.
    ///
    /// This allows injecting or swapping the ClusterForgeBridge after the
    /// syncer has been created, matching the Go pattern where the bridge
    /// is set later via dependency injection.
    pub fn set_bridge(&mut self, bridge: Arc<dyn ClusterForgeBridge>) {
        self.bridge = bridge;
    }

    /// Check if the syncer is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Enable the syncer.
    pub fn enable(&mut self) {
        self.enabled = true;
    }

    /// Disable the syncer.
    pub fn disable(&mut self) {
        self.enabled = false;
    }

    /// Share a reflection report with all online peers.
    pub async fn share_reflection(&self, report_json: serde_json::Value) -> Result<usize, String> {
        if !self.enabled {
            return Err("Syncer is disabled".into());
        }

        // Sanitize the report before sharing
        let sanitized = self.sanitize_report(&report_json);

        // Share via bridge
        let count = self.bridge.share_reflection(sanitized).await?;

        tracing::info!(
            peers = count,
            node_id = %self.bridge.local_node_id(),
            "[Syncer] Shared reflection report with peers"
        );

        Ok(count)
    }

    /// Fetch remote reflection reports from online peers.
    pub async fn fetch_remote_reflections(&self) -> Result<Vec<serde_json::Value>, String> {
        if !self.enabled {
            return Err("Syncer is disabled".into());
        }

        self.bridge.get_remote_reflections().await
    }

    /// Receive a remote reflection and store it in the remote reflections directory.
    ///
    /// `payload` must contain "content" (string) and optionally "filename", "from", "timestamp".
    pub fn receive_reflection(&self, payload: &serde_json::Value) -> Result<(), String> {
        let content = payload
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or("invalid or missing 'content' in payload")?;

        let mut filename = payload
            .get("filename")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if filename.is_empty() {
            let now = chrono::Local::now().format("%Y-%m-%d_%H%M%S");
            filename = format!("remote_{}.md", now);
        }

        // Sanitize filename: strip any path separators to prevent directory traversal
        filename = Path::new(&filename)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| {
                format!(
                    "remote_{}.md",
                    chrono::Local::now().format("%Y-%m-%d_%H%M%S")
                )
            });

        let from = payload
            .get("from")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let from = sanitize_node_id(&from);

        let timestamp = payload
            .get("timestamp")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let remote_dir = self.forge_dir.join("reflections").join("remote");
        std::fs::create_dir_all(&remote_dir)
            .map_err(|e| format!("failed to create remote dir: {}", e))?;

        // Prefix filename with source node to avoid collisions
        let final_filename = if !from.is_empty() {
            format!("{}_{}", from, filename)
        } else {
            filename
        };

        // Add metadata header
        let header = format!(
            "<!-- Remote reflection from {} at {} -->\n",
            from, timestamp
        );
        let full_content = format!("{}{}", header, content);

        let dest_path = remote_dir.join(&final_filename);
        std::fs::write(&dest_path, full_content)
            .map_err(|e| format!("failed to write remote report: {}", e))?;

        tracing::info!(
            from = %from,
            filename = %final_filename,
            "[Syncer] Received remote reflection"
        );

        Ok(())
    }

    /// Get file paths of all local reflection reports (for sharing).
    pub fn get_local_reflections(&self) -> Result<Vec<PathBuf>, String> {
        let reflections_dir = self.forge_dir.join("reflections");
        read_md_files(&reflections_dir)
    }

    /// Get file paths of all remote reflection reports.
    pub fn get_remote_reflections_paths(&self) -> Result<Vec<PathBuf>, String> {
        let remote_dir = self.forge_dir.join("reflections").join("remote");
        read_md_files(&remote_dir)
    }

    /// Get a serializable list of available local reflections.
    pub fn get_reflections_list_payload(&self) -> serde_json::Value {
        match self.get_local_reflections() {
            Ok(paths) => {
                let filenames: Vec<String> = paths
                    .iter()
                    .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
                    .collect();
                serde_json::json!({
                    "reflections": filenames,
                    "count": filenames.len(),
                })
            }
            Err(e) => {
                serde_json::json!({
                    "reflections": [],
                    "error": e,
                })
            }
        }
    }

    /// Read a specific reflection report content by filename.
    ///
    /// Security: only allows reading from the reflections directory.
    pub fn read_reflection_content(&self, filename: &str) -> Result<String, String> {
        // Sanitize filename: strip path components
        let safe_name = Path::new(filename)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .ok_or_else(|| format!("invalid filename: {}", filename))?;

        if safe_name == "." || safe_name == ".." {
            return Err(format!("invalid filename: {}", filename));
        }

        let path = self.forge_dir.join("reflections").join(&safe_name);

        // Security: ensure the resolved path is within the reflections directory
        let reflections_dir = self.forge_dir.join("reflections");
        let abs_path = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        let abs_dir = std::fs::canonicalize(&reflections_dir).unwrap_or(reflections_dir.clone());

        if !abs_path.starts_with(&abs_dir) {
            return Err(format!("invalid path: {}", filename));
        }

        std::fs::read_to_string(&abs_path).map_err(|e| format!("failed to read reflection: {}", e))
    }

    /// Sanitize reflection content before sharing with remote peers.
    pub fn sanitize_content(&self, content: &str) -> String {
        let sanitized = self.sanitizer.sanitize(content);
        sanitized
    }

    /// Sanitize a report before sharing (remove sensitive data).
    /// Test-only wrapper for sanitize_report (F-M8 verification).
    #[cfg(test)]
    pub(crate) fn sanitize_report_for_test(&self, report: &serde_json::Value) -> serde_json::Value {
        self.sanitize_report(report)
    }

    fn sanitize_report(&self, report: &serde_json::Value) -> serde_json::Value {
        let report_str = serde_json::to_string(report).unwrap_or_default();
        let sanitized = self.sanitizer.sanitize(&report_str);
        serde_json::from_str(&sanitized).unwrap_or_else(|_| {
            // F-M8: NEVER fall back to the unsanitized report — a regex that
            // crossed JSON structure would otherwise leak the very secrets we
            // tried to scrub. Fail closed (drop the content) instead.
            tracing::warn!("[Syncer] sanitized report failed to re-parse; dropping content");
            serde_json::json!({})
        })
    }
}

/// Read all .md files from a directory.
fn read_md_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let entries = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_file() && path.extension().map(|e| e == "md").unwrap_or(false) {
            paths.push(path);
        }
    }
    Ok(paths)
}

/// Strip unsafe characters from a node ID used in filenames.
fn sanitize_node_id(id: &str) -> String {
    let sanitized: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests;

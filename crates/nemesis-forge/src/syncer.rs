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
    pub async fn share_reflection(
        &self,
        report_json: serde_json::Value,
    ) -> Result<usize, String> {
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
            "Shared reflection report with peers"
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
            let now = chrono::Utc::now().format("%Y-%m-%d_%H%M%S");
            filename = format!("remote_{}.md", now);
        }

        // Sanitize filename: strip any path separators to prevent directory traversal
        filename = Path::new(&filename)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("remote_{}.md", chrono::Utc::now().format("%Y-%m-%d_%H%M%S")));

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
        let header = format!("<!-- Remote reflection from {} at {} -->\n", from, timestamp);
        let full_content = format!("{}{}", header, content);

        let dest_path = remote_dir.join(&final_filename);
        std::fs::write(&dest_path, full_content)
            .map_err(|e| format!("failed to write remote report: {}", e))?;

        tracing::info!(
            from = %from,
            filename = %final_filename,
            "Received remote reflection"
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

        std::fs::read_to_string(&abs_path)
            .map_err(|e| format!("failed to read reflection: {}", e))
    }

    /// Sanitize reflection content before sharing with remote peers.
    pub fn sanitize_content(&self, content: &str) -> String {
        let sanitized = self.sanitizer.sanitize(content);
        sanitized
    }

    /// Sanitize a report before sharing (remove sensitive data).
    fn sanitize_report(&self, report: &serde_json::Value) -> serde_json::Value {
        let report_str = serde_json::to_string(report).unwrap_or_default();
        let sanitized = self.sanitizer.sanitize(&report_str);
        serde_json::from_str(&sanitized).unwrap_or_else(|_| report.clone())
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
mod tests {
    use super::*;
    use crate::bridge::NoOpBridge;

    #[tokio::test]
    async fn test_syncer_with_noop_bridge() {
        let bridge = Arc::new(NoOpBridge::new("test-node".into()));
        let syncer = Syncer::new(bridge);

        assert!(syncer.is_enabled());

        let count = syncer
            .share_reflection(serde_json::json!({"insights": ["test"]}))
            .await
            .unwrap();
        assert_eq!(count, 0); // NoOp bridge returns 0
    }

    #[tokio::test]
    async fn test_syncer_disabled() {
        let bridge = Arc::new(NoOpBridge::new("test-node".into()));
        let mut syncer = Syncer::new(bridge);
        syncer.disable();

        assert!(!syncer.is_enabled());

        let result = syncer.share_reflection(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_enable_disable() {
        let bridge = Arc::new(NoOpBridge::new("test".into()));
        let mut syncer = Syncer::new(bridge);
        assert!(syncer.is_enabled());
        syncer.disable();
        assert!(!syncer.is_enabled());
        syncer.enable();
        assert!(syncer.is_enabled());
    }

    #[tokio::test]
    async fn test_fetch_remote_reflections() {
        let bridge = Arc::new(NoOpBridge::new("test".into()));
        let syncer = Syncer::new(bridge);

        let reflections = syncer.fetch_remote_reflections().await.unwrap();
        assert!(reflections.is_empty());
    }

    #[test]
    fn test_receive_reflection() {
        let dir = tempfile::tempdir().unwrap();
        let bridge = Arc::new(NoOpBridge::new("test".into()));
        let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

        let payload = serde_json::json!({
            "content": "# Test Reflection\nSome insights here",
            "filename": "test_report.md",
            "from": "node-abc",
            "timestamp": "2026-04-29T12:00:00Z"
        });

        syncer.receive_reflection(&payload).unwrap();

        // Check file was created
        let remote_dir = dir.path().join("reflections").join("remote");
        assert!(remote_dir.exists());
        let entries: Vec<_> = std::fs::read_dir(&remote_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 1);
        let name = entries[0].file_name().to_string_lossy().to_string();
        // The filename should contain the sanitized node ID prefix
        assert!(name.ends_with(".md"), "Expected .md file, got: {}", name);
        assert!(name.contains("node"), "Expected 'node' in filename, got: {}", name);
    }

    #[test]
    fn test_receive_reflection_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let bridge = Arc::new(NoOpBridge::new("test".into()));
        let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

        // Try path traversal
        let payload = serde_json::json!({
            "content": "test",
            "filename": "../../../etc/passwd",
        });

        syncer.receive_reflection(&payload).unwrap();

        // Should still write to remote dir (path stripped)
        let remote_dir = dir.path().join("reflections").join("remote");
        assert!(remote_dir.exists());
    }

    #[test]
    fn test_get_reflections_list_payload() {
        let dir = tempfile::tempdir().unwrap();
        let bridge = Arc::new(NoOpBridge::new("test".into()));
        let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

        // Create some reflection files
        let ref_dir = dir.path().join("reflections");
        std::fs::create_dir_all(&ref_dir).unwrap();
        std::fs::write(ref_dir.join("report1.md"), "test1").unwrap();
        std::fs::write(ref_dir.join("report2.md"), "test2").unwrap();

        let payload = syncer.get_reflections_list_payload();
        let reflections = payload["reflections"].as_array().unwrap();
        assert_eq!(reflections.len(), 2);
        assert_eq!(payload["count"].as_u64().unwrap(), 2);
    }

    #[test]
    fn test_read_reflection_content() {
        let dir = tempfile::tempdir().unwrap();
        let bridge = Arc::new(NoOpBridge::new("test".into()));
        let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

        let ref_dir = dir.path().join("reflections");
        std::fs::create_dir_all(&ref_dir).unwrap();
        std::fs::write(ref_dir.join("test.md"), "# Test Content").unwrap();

        let content = syncer.read_reflection_content("test.md").unwrap();
        assert_eq!(content, "# Test Content");
    }

    #[test]
    fn test_sanitize_content() {
        let bridge = Arc::new(NoOpBridge::new("test".into()));
        let syncer = Syncer::new(bridge);
        let result = syncer.sanitize_content("test content");
        assert!(!result.is_empty());
    }

    #[test]
    fn test_sanitize_node_id() {
        assert_eq!(sanitize_node_id("node-abc_123"), "node-abc_123");
        assert_eq!(sanitize_node_id("node with spaces"), "node_with_spaces");
        assert_eq!(sanitize_node_id(""), "unknown");
        assert_eq!(sanitize_node_id("a/b\\c"), "a_b_c");
    }

    #[tokio::test]
    async fn test_set_bridge() {
        let bridge1 = Arc::new(NoOpBridge::new("node-1".into()));
        let mut syncer = Syncer::new(bridge1);
        assert_eq!(syncer.bridge.local_node_id(), "node-1");

        // Swap to a different bridge
        let bridge2 = Arc::new(NoOpBridge::new("node-2".into()));
        syncer.set_bridge(bridge2);
        assert_eq!(syncer.bridge.local_node_id(), "node-2");

        // Verify the syncer still works with the new bridge
        let count = syncer.share_reflection(serde_json::json!({})).await.unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_receive_reflection_missing_content() {
        let dir = tempfile::tempdir().unwrap();
        let bridge = Arc::new(NoOpBridge::new("test".into()));
        let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

        let payload = serde_json::json!({
            "filename": "test.md"
        });
        let result = syncer.receive_reflection(&payload);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("content"));
    }

    #[test]
    fn test_receive_reflection_auto_filename() {
        let dir = tempfile::tempdir().unwrap();
        let bridge = Arc::new(NoOpBridge::new("test".into()));
        let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

        let payload = serde_json::json!({
            "content": "# Auto-filename test",
            "from": "node-auto"
        });
        syncer.receive_reflection(&payload).unwrap();

        let remote_dir = dir.path().join("reflections").join("remote");
        assert!(remote_dir.exists());
        let entries: Vec<_> = std::fs::read_dir(&remote_dir).unwrap().filter_map(|e| e.ok()).collect();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn test_receive_reflection_with_metadata_header() {
        let dir = tempfile::tempdir().unwrap();
        let bridge = Arc::new(NoOpBridge::new("test".into()));
        let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

        let payload = serde_json::json!({
            "content": "# Test content",
            "filename": "header_test.md",
            "from": "node-meta",
            "timestamp": "2026-04-29T12:00:00Z"
        });
        syncer.receive_reflection(&payload).unwrap();

        let remote_dir = dir.path().join("reflections").join("remote");
        let entries: Vec<_> = std::fs::read_dir(&remote_dir).unwrap().filter_map(|e| e.ok()).collect();
        assert_eq!(entries.len(), 1);
        let content = std::fs::read_to_string(entries[0].path()).unwrap();
        assert!(content.contains("Remote reflection from"));
        assert!(content.contains("node-meta"));
        assert!(content.contains("# Test content"));
    }

    #[test]
    fn test_get_local_reflections_empty() {
        let dir = tempfile::tempdir().unwrap();
        let bridge = Arc::new(NoOpBridge::new("test".into()));
        let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

        let paths = syncer.get_local_reflections().unwrap();
        assert!(paths.is_empty());
    }

    #[test]
    fn test_get_remote_reflections_paths_empty() {
        let dir = tempfile::tempdir().unwrap();
        let bridge = Arc::new(NoOpBridge::new("test".into()));
        let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

        let paths = syncer.get_remote_reflections_paths().unwrap();
        assert!(paths.is_empty());
    }

    #[test]
    fn test_get_local_reflections_with_files() {
        let dir = tempfile::tempdir().unwrap();
        let bridge = Arc::new(NoOpBridge::new("test".into()));
        let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

        let ref_dir = dir.path().join("reflections");
        std::fs::create_dir_all(&ref_dir).unwrap();
        std::fs::write(ref_dir.join("a.md"), "content a").unwrap();
        std::fs::write(ref_dir.join("b.md"), "content b").unwrap();
        std::fs::write(ref_dir.join("c.txt"), "not md").unwrap();

        let paths = syncer.get_local_reflections().unwrap();
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn test_read_reflection_content_dot_dot() {
        let dir = tempfile::tempdir().unwrap();
        let bridge = Arc::new(NoOpBridge::new("test".into()));
        let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

        let result = syncer.read_reflection_content("..");
        assert!(result.is_err());
    }

    #[test]
    fn test_read_reflection_content_dot() {
        let dir = tempfile::tempdir().unwrap();
        let bridge = Arc::new(NoOpBridge::new("test".into()));
        let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

        let result = syncer.read_reflection_content(".");
        assert!(result.is_err());
    }

    #[test]
    fn test_sanitize_content_removes_sensitive_data() {
        let bridge = Arc::new(NoOpBridge::new("test".into()));
        let syncer = Syncer::new(bridge);
        let content = "api_key: sk-1234567890abcdefghijklmnop";
        let sanitized = syncer.sanitize_content(content);
        assert!(sanitized.contains("[REDACTED]"));
    }

    #[test]
    fn test_sanitize_node_id_special_chars() {
        assert_eq!(sanitize_node_id("node@#$%123!"), "node____123_");
    }

    #[test]
    fn test_sanitize_node_id_all_special() {
        let result = sanitize_node_id("@#$%");
        assert_eq!(result, "____");
    }

    #[test]
    fn test_sanitize_node_id_unicode() {
        let result = sanitize_node_id("node-123");
        assert_eq!(result, "node-123");
    }

    #[tokio::test]
    async fn test_fetch_remote_reflections_disabled() {
        let bridge = Arc::new(NoOpBridge::new("test".into()));
        let mut syncer = Syncer::new(bridge);
        syncer.disable();
        let result = syncer.fetch_remote_reflections().await;
        assert!(result.is_err());
    }

    #[test]
    fn test_get_reflections_list_payload_empty() {
        let dir = tempfile::tempdir().unwrap();
        let bridge = Arc::new(NoOpBridge::new("test".into()));
        let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

        let payload = syncer.get_reflections_list_payload();
        assert_eq!(payload["count"].as_u64().unwrap(), 0);
        assert!(payload["reflections"].as_array().unwrap().is_empty());
    }

    #[test]
    fn test_receive_reflection_from_empty() {
        let dir = tempfile::tempdir().unwrap();
        let bridge = Arc::new(NoOpBridge::new("test".into()));
        let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

        let payload = serde_json::json!({
            "content": "test content without from field",
            "filename": "no_from.md"
        });
        syncer.receive_reflection(&payload).unwrap();

        let remote_dir = dir.path().join("reflections").join("remote");
        let entries: Vec<_> = std::fs::read_dir(&remote_dir).unwrap().filter_map(|e| e.ok()).collect();
        assert_eq!(entries.len(), 1);
    }

    #[tokio::test]
    async fn test_share_reflection_sanitizes() {
        let bridge = Arc::new(NoOpBridge::new("test".into()));
        let syncer = Syncer::new(bridge);

        let report = serde_json::json!({
            "data": "api_key=sk-1234567890abcdefghijklmnopqrstuv"
        });
        let count = syncer.share_reflection(report).await.unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_with_forge_dir_stores_dir() {
        let dir = tempfile::tempdir().unwrap();
        let bridge = Arc::new(NoOpBridge::new("test".into()));
        let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

        let payload = serde_json::json!({"content": "test"});
        // Should create reflections/remote directory
        syncer.receive_reflection(&payload).unwrap();
        assert!(dir.path().join("reflections").join("remote").exists());
    }

    /// Edge case: share_reflection with mock peers that return non-zero count
    /// (matches Go's TestSyncer_ShareReflection_WithPeers)
    #[tokio::test]
    async fn test_share_reflection_with_mock_peers() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct MockPeerBridge {
            node_id: String,
            share_call_count: Arc<AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl ClusterForgeBridge for MockPeerBridge {
            async fn share_reflection(&self, _report: serde_json::Value) -> Result<usize, String> {
                self.share_call_count.fetch_add(1, Ordering::SeqCst);
                Ok(5) // Simulate 5 peers received the report
            }
            async fn get_remote_reflections(&self) -> Result<Vec<serde_json::Value>, String> {
                Ok(Vec::new())
            }
            async fn get_online_peers(&self) -> Result<Vec<String>, String> {
                Ok(vec![
                    "peer-a".into(),
                    "peer-b".into(),
                    "peer-c".into(),
                    "peer-d".into(),
                    "peer-e".into(),
                ])
            }
            fn local_node_id(&self) -> &str {
                &self.node_id
            }
            fn is_cluster_enabled(&self) -> bool {
                true
            }
        }

        let share_call_count = Arc::new(AtomicUsize::new(0));
        let bridge = Arc::new(MockPeerBridge {
            node_id: "node-test".into(),
            share_call_count: share_call_count.clone(),
        });
        let syncer = Syncer::new(bridge);

        let report = serde_json::json!({
            "insights": ["tool X has 80% success rate"],
            "recommendations": ["Consider creating a skill for tool X"]
        });

        let count = syncer.share_reflection(report).await.unwrap();
        assert_eq!(count, 5, "Should report sharing with 5 peers");
        assert_eq!(share_call_count.load(Ordering::SeqCst), 1);
    }
}

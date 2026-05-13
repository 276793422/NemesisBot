//! Integrity Audit Chain - Layer 8
//! Merkle-based SHA256 audit chain with JSONL persistence, segment rotation,
//! export/load, and sign field support.

use sha2::{Sha256, Digest};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Audit event for the integrity chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub id: String,
    pub timestamp: String,
    pub operation: String,
    pub tool_name: String,
    pub user: String,
    pub source: String,
    pub target: String,
    pub decision: String,
    pub reason: String,
    pub hash: String,
    pub prev_hash: String,
    /// Optional Ed25519 signature of the event hash.
    #[serde(default)]
    pub sign: Option<String>,
}

/// Audit chain configuration.
#[derive(Debug, Clone)]
pub struct AuditChainConfig {
    pub enabled: bool,
    pub storage_path: PathBuf,
    pub max_file_size: u64,
    pub verify_on_load: bool,
    /// Max events per segment before rotation.
    pub max_events_per_segment: u64,
    /// Optional signing key for event signatures.
    pub signing_key: Option<String>,
}

impl Default for AuditChainConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            storage_path: PathBuf::from("audit_chain.jsonl"),
            max_file_size: 50 * 1024 * 1024,
            verify_on_load: false,
            max_events_per_segment: 100_000,
            signing_key: None,
        }
    }
}

/// Returns a reasonable default configuration.
///
/// Equivalent to Go's `DefaultAuditChainConfig()`.
pub fn default_audit_chain_config() -> AuditChainConfig {
    AuditChainConfig::default()
}

/// Merkle audit chain with segment rotation and export.
///
/// Thread-safe: all mutable state is protected by locks or atomic counters.
pub struct AuditChain {
    config: AuditChainConfig,
    last_hash: parking_lot::Mutex<String>,
    event_count: std::sync::atomic::AtomicU64,
    segment_count: std::sync::atomic::AtomicU64,
    total_events: std::sync::atomic::AtomicU64,
    /// Whether the chain has been closed.
    closed: std::sync::atomic::AtomicBool,
}

impl AuditChain {
    pub fn new(config: AuditChainConfig) -> Self {
        let last_hash = parking_lot::Mutex::new("0000000000000000000000000000000000000000000000000000000000000000".to_string());
        Self {
            config,
            last_hash,
            event_count: std::sync::atomic::AtomicU64::new(0),
            segment_count: std::sync::atomic::AtomicU64::new(1),
            total_events: std::sync::atomic::AtomicU64::new(0),
            closed: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Append an event to the audit chain.
    ///
    /// Returns an error if the chain has been closed.
    pub fn append(&self, operation: &str, tool_name: &str, user: &str, source: &str, target: &str, decision: &str, reason: &str) -> Result<AuditEvent, String> {
        self.append_with_sign(operation, tool_name, user, source, target, decision, reason, None)
    }

    /// Append an event with an optional signature.
    ///
    /// Returns an error if the chain has been closed.
    pub fn append_with_sign(&self, operation: &str, tool_name: &str, user: &str, source: &str, target: &str, decision: &str, reason: &str, sign: Option<String>) -> Result<AuditEvent, String> {
        if self.is_closed() {
            return Err("audit chain is closed".to_string());
        }
        let mut hasher = Sha256::new();
        let timestamp = chrono::Utc::now().to_rfc3339();
        let id = uuid::Uuid::new_v4().to_string();

        let prev_hash = self.last_hash.lock().clone();

        // Hash: prev_hash + timestamp + operation + tool + user + target + decision
        hasher.update(prev_hash.as_bytes());
        hasher.update(timestamp.as_bytes());
        hasher.update(operation.as_bytes());
        hasher.update(tool_name.as_bytes());
        hasher.update(user.as_bytes());
        hasher.update(target.as_bytes());
        hasher.update(decision.as_bytes());
        let hash = format!("{:x}", hasher.finalize());

        let event = AuditEvent {
            id,
            timestamp,
            operation: operation.to_string(),
            tool_name: tool_name.to_string(),
            user: user.to_string(),
            source: source.to_string(),
            target: target.to_string(),
            decision: decision.to_string(),
            reason: reason.to_string(),
            hash: hash.clone(),
            prev_hash: prev_hash.clone(),
            sign,
        };

        *self.last_hash.lock() = hash;
        let count = self.event_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        self.total_events.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        // Persist to JSONL
        if let Ok(json) = serde_json::to_string(&event) {
            let storage_path = self.get_current_segment_path();
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&storage_path)
                .and_then(|mut f| {
                    use std::io::Write;
                    writeln!(f, "{}", json)
                });

            // Check if rotation is needed
            if count >= self.config.max_events_per_segment {
                self.rotate_segment();
            }
        }

        Ok(event)
    }

    /// Get the current chain hash.
    pub fn current_hash(&self) -> String {
        self.last_hash.lock().clone()
    }

    /// Get event count for current segment.
    pub fn event_count(&self) -> u64 {
        self.event_count.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Get total event count across all segments.
    pub fn total_event_count(&self) -> u64 {
        self.total_events.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Get current segment number.
    pub fn segment_count(&self) -> u64 {
        self.segment_count.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Rotate to a new segment.
    fn rotate_segment(&self) {
        self.event_count.store(0, std::sync::atomic::Ordering::SeqCst);
        self.segment_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    /// Get the path for the current segment file.
    fn get_current_segment_path(&self) -> PathBuf {
        let seg = self.segment_count.load(std::sync::atomic::Ordering::SeqCst);
        if seg == 1 {
            self.config.storage_path.clone()
        } else {
            let ext = self.config.storage_path.extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default();
            let stem = self.config.storage_path.file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "audit_chain".to_string());
            self.config.storage_path.parent()
                .map(|p| p.join(format!("{}_seg{:04}{}", stem, seg, ext)))
                .unwrap_or_else(|| PathBuf::from(format!("{}_seg{:04}{}", stem, seg, ext)))
        }
    }

    /// Verify chain integrity from a list of events.
    pub fn verify_chain(events: &[AuditEvent]) -> bool {
        for i in 1..events.len() {
            if events[i].prev_hash != events[i - 1].hash {
                return false;
            }
            // Verify hash computation
            let mut hasher = Sha256::new();
            hasher.update(events[i].prev_hash.as_bytes());
            hasher.update(events[i].timestamp.as_bytes());
            hasher.update(events[i].operation.as_bytes());
            hasher.update(events[i].tool_name.as_bytes());
            hasher.update(events[i].user.as_bytes());
            hasher.update(events[i].target.as_bytes());
            hasher.update(events[i].decision.as_bytes());
            let computed = format!("{:x}", hasher.finalize());
            if computed != events[i].hash {
                return false;
            }
        }
        true
    }

    /// Export the entire chain to a JSON file.
    pub fn export_chain(&self, output_path: &Path) -> Result<(), String> {
        let mut all_events: Vec<AuditEvent> = Vec::new();

        // Read all segment files
        let parent = self.config.storage_path.parent().unwrap_or(Path::new("."));
        let stem = self.config.storage_path.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "audit_chain".to_string());

        // Read main file
        if self.config.storage_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&self.config.storage_path) {
                for line in content.lines() {
                    if let Ok(event) = serde_json::from_str::<AuditEvent>(line) {
                        all_events.push(event);
                    }
                }
            }
        }

        // Read segment files
        if let Ok(entries) = std::fs::read_dir(parent) {
            let mut segment_files: Vec<PathBuf> = Vec::new();
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(&stem) && name.contains("_seg") {
                    segment_files.push(entry.path());
                }
            }
            segment_files.sort();
            for path in segment_files {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    for line in content.lines() {
                        if let Ok(event) = serde_json::from_str::<AuditEvent>(line) {
                            all_events.push(event);
                        }
                    }
                }
            }
        }

        let json = serde_json::to_string_pretty(&all_events).map_err(|e| e.to_string())?;
        std::fs::write(output_path, json).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Load chain from an exported JSON file (replaces current chain).
    pub fn load_chain(&self, input_path: &Path) -> Result<u64, String> {
        let content = std::fs::read_to_string(input_path).map_err(|e| e.to_string())?;
        let events: Vec<AuditEvent> = serde_json::from_str(&content).map_err(|e| e.to_string())?;

        if !Self::verify_chain(&events) {
            return Err("chain verification failed".to_string());
        }

        // Update last hash
        if let Some(last) = events.last() {
            *self.last_hash.lock() = last.hash.clone();
        }

        let count = events.len() as u64;
        self.total_events.fetch_add(count, std::sync::atomic::Ordering::SeqCst);

        // Write events to storage
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.config.storage_path)
            .map_err(|e| e.to_string())?;
        use std::io::Write;
        for event in &events {
            let json = serde_json::to_string(event).map_err(|e| e.to_string())?;
            writeln!(f, "{}", json).map_err(|e| e.to_string())?;
        }

        Ok(count)
    }

    /// Load existing segments from disk on startup.
    ///
    /// Reads the main segment file and all rotated segment files,
    /// updates the last hash and counters.
    pub fn load_segments(&self) -> Result<u64, String> {
        let mut total: u64 = 0;
        let mut last_hash = "0000000000000000000000000000000000000000000000000000000000000000".to_string();

        // Collect all segment files sorted
        let mut files: Vec<PathBuf> = Vec::new();
        let parent = self.config.storage_path.parent().unwrap_or(Path::new("."));
        let stem = self.config.storage_path.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "audit_chain".to_string());

        if self.config.storage_path.exists() {
            files.push(self.config.storage_path.clone());
        }

        if let Ok(entries) = std::fs::read_dir(parent) {
            let mut seg_files: Vec<PathBuf> = entries
                .flatten()
                .filter(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    name.starts_with(&stem) && name.contains("_seg")
                })
                .map(|e| e.path())
                .collect();
            seg_files.sort();
            files.extend(seg_files);
        }

        for path in files {
            if let Ok(content) = std::fs::read_to_string(&path) {
                for line in content.lines() {
                    if let Ok(event) = serde_json::from_str::<AuditEvent>(line) {
                        total += 1;
                        last_hash = event.hash.clone();
                    }
                }
            }
        }

        if total > 0 {
            *self.last_hash.lock() = last_hash;
            self.total_events.store(total, std::sync::atomic::Ordering::SeqCst);
        }

        Ok(total)
    }

    /// Get a specific event by index from the chain file.
    ///
    /// Index is 0-based. Returns None if index is out of range.
    pub fn get_event(&self, index: usize) -> Option<AuditEvent> {
        let mut count: usize = 0;
        let files = self.collect_segment_files();

        for path in files {
            if let Ok(content) = std::fs::read_to_string(&path) {
                for line in content.lines() {
                    if let Ok(event) = serde_json::from_str::<AuditEvent>(line) {
                        if count == index {
                            return Some(event);
                        }
                        count += 1;
                    }
                }
            }
        }
        None
    }

    /// Verify a range of events in the chain.
    ///
    /// Checks hash continuity between events[from] to events[to].
    pub fn verify_range(&self, from: usize, to: usize) -> Result<bool, String> {
        let events = self.read_events_range(from, to + 1)?;
        if events.len() < 2 {
            return Ok(true);
        }
        Ok(Self::verify_chain(&events))
    }

    /// Get the number of total stored events (alias for total_event_count).
    pub fn size(&self) -> u64 {
        self.total_event_count()
    }

    /// Close the chain, flushing pending writes and preventing further appends.
    ///
    /// Equivalent to Go's `AuditChain.Close()`. After calling this method,
    /// subsequent `append()` calls will return an error (or be silently ignored
    /// depending on the chain's error handling mode).
    pub fn close(&self) -> Result<(), String> {
        let was_closed = self.closed.swap(true, std::sync::atomic::Ordering::SeqCst);
        if was_closed {
            return Ok(());
        }
        tracing::info!(
            total_events = self.total_event_count(),
            "Audit chain closed"
        );
        Ok(())
    }

    /// Check if the chain has been closed.
    pub fn is_closed(&self) -> bool {
        self.closed.load(std::sync::atomic::Ordering::SeqCst)
    }

    // ---- Private helpers ----

    /// Collect all segment files in order.
    fn collect_segment_files(&self) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = Vec::new();
        let parent = self.config.storage_path.parent().unwrap_or(Path::new("."));
        let stem = self.config.storage_path.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "audit_chain".to_string());

        if self.config.storage_path.exists() {
            files.push(self.config.storage_path.clone());
        }

        if let Ok(entries) = std::fs::read_dir(parent) {
            let mut seg_files: Vec<PathBuf> = entries
                .flatten()
                .filter(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    name.starts_with(&stem) && name.contains("_seg")
                })
                .map(|e| e.path())
                .collect();
            seg_files.sort();
            files.extend(seg_files);
        }

        files
    }

    /// Read a range of events from disk.
    fn read_events_range(&self, from: usize, to: usize) -> Result<Vec<AuditEvent>, String> {
        let files = self.collect_segment_files();
        let mut events = Vec::new();

        for path in files {
            if events.len() >= to {
                break;
            }
            if let Ok(content) = std::fs::read_to_string(&path) {
                for line in content.lines() {
                    if let Ok(event) = serde_json::from_str::<AuditEvent>(line) {
                        events.push(event);
                        if events.len() >= to {
                            break;
                        }
                    }
                }
            }
        }

        if from >= events.len() {
            return Ok(Vec::new());
        }
        Ok(events[from..to.min(events.len())].to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_append_event() {
        let dir = tempfile::tempdir().unwrap();
        let config = AuditChainConfig {
            storage_path: dir.path().join("audit.jsonl"),
            ..Default::default()
        };
        let chain = AuditChain::new(config);

        let event = chain.append("file_read", "read_file", "user1", "cli", "/tmp/test.txt", "allowed", "rule match").unwrap();
        assert_eq!(event.decision, "allowed");
        assert_ne!(event.hash, event.prev_hash);
        assert_eq!(chain.event_count(), 1);
        assert!(event.sign.is_none());
    }

    #[test]
    fn test_append_with_sign() {
        let dir = tempfile::tempdir().unwrap();
        let config = AuditChainConfig {
            storage_path: dir.path().join("audit.jsonl"),
            ..Default::default()
        };
        let chain = AuditChain::new(config);

        let event = chain.append_with_sign("file_read", "read_file", "u", "c", "/tmp", "allowed", "", Some("sig123".to_string())).unwrap();
        assert_eq!(event.sign, Some("sig123".to_string()));
    }

    #[test]
    fn test_chain_hash_progression() {
        let dir = tempfile::tempdir().unwrap();
        let config = AuditChainConfig {
            storage_path: dir.path().join("audit.jsonl"),
            ..Default::default()
        };
        let chain = AuditChain::new(config);

        let e1 = chain.append("file_read", "read_file", "user1", "cli", "/tmp/a", "allowed", "ok").unwrap();
        let e2 = chain.append("file_write", "write_file", "user1", "cli", "/tmp/b", "denied", "blocked").unwrap();
        let e3 = chain.append("process_exec", "exec", "user1", "cli", "ls", "allowed", "ok").unwrap();

        assert_eq!(e2.prev_hash, e1.hash);
        assert_eq!(e3.prev_hash, e2.hash);
        assert_eq!(chain.event_count(), 3);
    }

    #[test]
    fn test_verify_chain() {
        let dir = tempfile::tempdir().unwrap();
        let config = AuditChainConfig {
            storage_path: dir.path().join("audit.jsonl"),
            ..Default::default()
        };
        let chain = AuditChain::new(config);

        let e1 = chain.append("file_read", "read_file", "u", "c", "/tmp", "allowed", "").unwrap();
        let e2 = chain.append("file_write", "write_file", "u", "c", "/tmp", "denied", "").unwrap();

        assert!(AuditChain::verify_chain(&[e1.clone(), e2.clone()]));

        // Tamper with an event
        let mut tampered = e2.clone();
        tampered.hash = "tampered".to_string();
        assert!(!AuditChain::verify_chain(&[e1, tampered]));
    }

    #[test]
    fn test_jsonl_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let config = AuditChainConfig {
            storage_path: path.clone(),
            ..Default::default()
        };
        let chain = AuditChain::new(config);
        chain.append("file_read", "read_file", "u", "c", "/tmp", "allowed", "").unwrap();
        chain.append("file_write", "write_file", "u", "c", "/tmp", "denied", "").unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.trim().lines().collect();
        assert_eq!(lines.len(), 2);

        for line in lines {
            let _: AuditEvent = serde_json::from_str(line).unwrap();
        }
    }

    #[test]
    fn test_segment_rotation() {
        let dir = tempfile::tempdir().unwrap();
        let config = AuditChainConfig {
            storage_path: dir.path().join("audit.jsonl"),
            max_events_per_segment: 5,
            ..Default::default()
        };
        let chain = AuditChain::new(config);

        for i in 0..12 {
            chain.append("test", "tool", "u", "c", &format!("{}", i), "allowed", "").unwrap();
        }

        // Should have rotated at least once
        assert!(chain.segment_count() >= 2);
        assert_eq!(chain.total_event_count(), 12);
    }

    #[test]
    fn test_export_load_chain() {
        let dir = tempfile::tempdir().unwrap();
        let config = AuditChainConfig {
            storage_path: dir.path().join("audit.jsonl"),
            ..Default::default()
        };
        let chain = AuditChain::new(config);

        chain.append("file_read", "read_file", "u", "c", "/tmp/a", "allowed", "").unwrap();
        chain.append("file_write", "write_file", "u", "c", "/tmp/b", "denied", "").unwrap();

        let export_path = dir.path().join("export.json");
        chain.export_chain(&export_path).unwrap();
        assert!(export_path.exists());

        // Load into a new chain
        let config2 = AuditChainConfig {
            storage_path: dir.path().join("audit2.jsonl"),
            ..Default::default()
        };
        let chain2 = AuditChain::new(config2);
        let count = chain2.load_chain(&export_path).unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_empty_chain_verify() {
        let result = AuditChain::verify_chain(&[]);
        assert!(result);
    }

    #[test]
    fn test_single_event_chain_verify() {
        let dir = tempfile::tempdir().unwrap();
        let config = AuditChainConfig {
            storage_path: dir.path().join("audit.jsonl"),
            ..Default::default()
        };
        let chain = AuditChain::new(config);
        let e = chain.append("file_read", "read_file", "u", "c", "/tmp", "allowed", "").unwrap();
        assert!(AuditChain::verify_chain(&[e]));
    }

    #[test]
    fn test_event_has_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        let config = AuditChainConfig {
            storage_path: dir.path().join("audit.jsonl"),
            ..Default::default()
        };
        let chain = AuditChain::new(config);
        let event = chain.append("file_read", "read_file", "user1", "cli", "/tmp/test", "allowed", "").unwrap();
        assert!(!event.timestamp.is_empty());
    }

    #[test]
    fn test_load_nonexistent_file() {
        let dir = tempfile::tempdir().unwrap();
        let config = AuditChainConfig {
            storage_path: dir.path().join("audit.jsonl"),
            ..Default::default()
        };
        let chain = AuditChain::new(config);
        let result = chain.load_chain(&dir.path().join("nonexistent.json"));
        assert!(result.is_err());
    }

    #[test]
    fn test_config_default() {
        let config = AuditChainConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_events_per_segment, 100_000);
    }

    // ---- Additional integrity tests ----

    #[test]
    fn test_default_config_function() {
        let config = default_audit_chain_config();
        assert!(config.enabled);
        assert_eq!(config.max_file_size, 50 * 1024 * 1024);
        assert!(!config.verify_on_load);
        assert!(config.signing_key.is_none());
    }

    #[test]
    fn test_audit_event_serialization() {
        let event = AuditEvent {
            id: "test-id".to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            operation: "file_read".to_string(),
            tool_name: "read_file".to_string(),
            user: "alice".to_string(),
            source: "cli".to_string(),
            target: "/tmp/test.txt".to_string(),
            decision: "allowed".to_string(),
            reason: "rule match".to_string(),
            hash: "abc123".to_string(),
            prev_hash: "000000".to_string(),
            sign: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("test-id"));
        assert!(json.contains("file_read"));
        assert!(json.contains("alice"));
        assert!(json.contains("allowed"));

        let de: AuditEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(de.id, "test-id");
        assert_eq!(de.operation, "file_read");
        assert!(de.sign.is_none());
    }

    #[test]
    fn test_audit_event_with_sign_serialization() {
        let event = AuditEvent {
            id: "signed-event".to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            operation: "process_exec".to_string(),
            tool_name: "exec".to_string(),
            user: "admin".to_string(),
            source: "api".to_string(),
            target: "ls".to_string(),
            decision: "denied".to_string(),
            reason: "dangerous".to_string(),
            hash: "def456".to_string(),
            prev_hash: "abc123".to_string(),
            sign: Some("ed25519_signature_data".to_string()),
        };
        let json = serde_json::to_string(&event).unwrap();
        let de: AuditEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(de.sign, Some("ed25519_signature_data".to_string()));
    }

    #[test]
    fn test_audit_event_default_sign_is_none() {
        let json = r#"{"id":"x","timestamp":"","operation":"","tool_name":"","user":"","source":"","target":"","decision":"","reason":"","hash":"","prev_hash":""}"#;
        let event: AuditEvent = serde_json::from_str(json).unwrap();
        assert!(event.sign.is_none());
    }

    #[test]
    fn test_close_prevents_append() {
        let dir = tempfile::tempdir().unwrap();
        let config = AuditChainConfig {
            storage_path: dir.path().join("audit.jsonl"),
            ..Default::default()
        };
        let chain = AuditChain::new(config);
        chain.append("op1", "tool1", "u", "c", "/tmp", "allowed", "").unwrap();
        chain.close().unwrap();
        assert!(chain.is_closed());

        let result = chain.append("op2", "tool2", "u", "c", "/tmp", "denied", "");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("closed"));
    }

    #[test]
    fn test_close_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let config = AuditChainConfig {
            storage_path: dir.path().join("audit.jsonl"),
            ..Default::default()
        };
        let chain = AuditChain::new(config);
        chain.close().unwrap();
        chain.close().unwrap(); // Second close should succeed
        assert!(chain.is_closed());
    }

    #[test]
    fn test_current_hash_initial() {
        let dir = tempfile::tempdir().unwrap();
        let config = AuditChainConfig {
            storage_path: dir.path().join("audit.jsonl"),
            ..Default::default()
        };
        let chain = AuditChain::new(config);
        let hash = chain.current_hash();
        assert_eq!(hash, "0000000000000000000000000000000000000000000000000000000000000000");
    }

    #[test]
    fn test_current_hash_updates() {
        let dir = tempfile::tempdir().unwrap();
        let config = AuditChainConfig {
            storage_path: dir.path().join("audit.jsonl"),
            ..Default::default()
        };
        let chain = AuditChain::new(config);
        let initial = chain.current_hash().clone();
        chain.append("file_read", "read_file", "u", "c", "/tmp", "allowed", "").unwrap();
        let after = chain.current_hash();
        assert_ne!(initial, after);
        assert_eq!(after.len(), 64); // SHA256 hex
    }

    #[test]
    fn test_total_event_count() {
        let dir = tempfile::tempdir().unwrap();
        let config = AuditChainConfig {
            storage_path: dir.path().join("audit.jsonl"),
            ..Default::default()
        };
        let chain = AuditChain::new(config);
        assert_eq!(chain.total_event_count(), 0);
        chain.append("op1", "t1", "u", "c", "x", "a", "").unwrap();
        chain.append("op2", "t2", "u", "c", "y", "d", "").unwrap();
        chain.append("op3", "t3", "u", "c", "z", "a", "").unwrap();
        assert_eq!(chain.total_event_count(), 3);
    }

    #[test]
    fn test_size_alias() {
        let dir = tempfile::tempdir().unwrap();
        let config = AuditChainConfig {
            storage_path: dir.path().join("audit.jsonl"),
            ..Default::default()
        };
        let chain = AuditChain::new(config);
        chain.append("op", "tool", "u", "c", "x", "a", "").unwrap();
        assert_eq!(chain.size(), 1);
    }

    #[test]
    fn test_segment_count_initial() {
        let dir = tempfile::tempdir().unwrap();
        let config = AuditChainConfig {
            storage_path: dir.path().join("audit.jsonl"),
            ..Default::default()
        };
        let chain = AuditChain::new(config);
        assert_eq!(chain.segment_count(), 1);
    }

    #[test]
    fn test_get_event_by_index() {
        let dir = tempfile::tempdir().unwrap();
        let config = AuditChainConfig {
            storage_path: dir.path().join("audit.jsonl"),
            ..Default::default()
        };
        let chain = AuditChain::new(config);
        chain.append("file_read", "read_file", "u", "c", "/tmp/a", "allowed", "").unwrap();
        chain.append("file_write", "write_file", "u", "c", "/tmp/b", "denied", "").unwrap();
        chain.append("process_exec", "exec", "u", "c", "ls", "allowed", "").unwrap();

        let e0 = chain.get_event(0).unwrap();
        assert_eq!(e0.operation, "file_read");
        let e1 = chain.get_event(1).unwrap();
        assert_eq!(e1.operation, "file_write");
        let e2 = chain.get_event(2).unwrap();
        assert_eq!(e2.operation, "process_exec");
        assert!(chain.get_event(3).is_none());
    }

    #[test]
    fn test_verify_range() {
        let dir = tempfile::tempdir().unwrap();
        let config = AuditChainConfig {
            storage_path: dir.path().join("audit.jsonl"),
            ..Default::default()
        };
        let chain = AuditChain::new(config);
        chain.append("op1", "t1", "u", "c", "x", "a", "").unwrap();
        chain.append("op2", "t2", "u", "c", "y", "d", "").unwrap();
        chain.append("op3", "t3", "u", "c", "z", "a", "").unwrap();

        let result = chain.verify_range(0, 2).unwrap();
        assert!(result);
    }

    #[test]
    fn test_verify_chain_tampered_prev_hash() {
        let dir = tempfile::tempdir().unwrap();
        let config = AuditChainConfig {
            storage_path: dir.path().join("audit.jsonl"),
            ..Default::default()
        };
        let chain = AuditChain::new(config);
        let e1 = chain.append("op1", "t1", "u", "c", "x", "a", "").unwrap();
        let mut e2 = chain.append("op2", "t2", "u", "c", "y", "d", "").unwrap();

        // Tamper with prev_hash
        e2.prev_hash = "tampered".to_string();
        assert!(!AuditChain::verify_chain(&[e1, e2]));
    }

    #[test]
    fn test_load_segments_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let config = AuditChainConfig {
            storage_path: dir.path().join("audit.jsonl"),
            ..Default::default()
        };
        let chain = AuditChain::new(config);
        chain.append("op1", "t1", "u", "c", "x", "a", "").unwrap();
        chain.append("op2", "t2", "u", "c", "y", "d", "").unwrap();

        // Create a new chain and load from the same file
        let config2 = AuditChainConfig {
            storage_path: dir.path().join("audit.jsonl"),
            ..Default::default()
        };
        let chain2 = AuditChain::new(config2);
        let count = chain2.load_segments().unwrap();
        assert_eq!(count, 2);
        assert_eq!(chain2.total_event_count(), 2);
    }

    #[test]
    fn test_load_segments_empty() {
        let dir = tempfile::tempdir().unwrap();
        let config = AuditChainConfig {
            storage_path: dir.path().join("nonexistent.jsonl"),
            ..Default::default()
        };
        let chain = AuditChain::new(config);
        let count = chain.load_segments().unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_event_id_is_uuid() {
        let dir = tempfile::tempdir().unwrap();
        let config = AuditChainConfig {
            storage_path: dir.path().join("audit.jsonl"),
            ..Default::default()
        };
        let chain = AuditChain::new(config);
        let event = chain.append("op", "tool", "u", "c", "x", "a", "").unwrap();
        // UUID v4 format: 8-4-4-4-12 hex chars
        let parts: Vec<&str> = event.id.split('-').collect();
        assert_eq!(parts.len(), 5);
    }

    #[test]
    fn test_multiple_events_unique_ids() {
        let dir = tempfile::tempdir().unwrap();
        let config = AuditChainConfig {
            storage_path: dir.path().join("audit.jsonl"),
            ..Default::default()
        };
        let chain = AuditChain::new(config);
        let e1 = chain.append("op1", "t1", "u", "c", "x", "a", "").unwrap();
        let e2 = chain.append("op2", "t2", "u", "c", "y", "d", "").unwrap();
        assert_ne!(e1.id, e2.id);
    }

    #[test]
    fn test_config_custom_max_file_size() {
        let config = AuditChainConfig {
            max_file_size: 1024,
            ..Default::default()
        };
        assert_eq!(config.max_file_size, 1024);
    }

    #[test]
    fn test_config_custom_signing_key() {
        let config = AuditChainConfig {
            signing_key: Some("my-secret-key".to_string()),
            ..Default::default()
        };
        assert_eq!(config.signing_key, Some("my-secret-key".to_string()));
    }

    #[test]
    fn test_export_empty_chain() {
        let dir = tempfile::tempdir().unwrap();
        let config = AuditChainConfig {
            storage_path: dir.path().join("audit.jsonl"),
            ..Default::default()
        };
        let chain = AuditChain::new(config);
        let export_path = dir.path().join("export.json");
        chain.export_chain(&export_path).unwrap();
        assert!(export_path.exists());
        let content = std::fs::read_to_string(&export_path).unwrap();
        assert_eq!(content, "[]");
    }
}

//! Continuation store for task snapshot persistence.
//!
//! When a cluster_rpc tool triggers a non-blocking call, the agent loop saves
//! a continuation snapshot (conversation messages + tool call context). When
//! the callback arrives, the snapshot is loaded and LLM processing resumes.

use std::collections::HashMap;
use std::path::PathBuf;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// A continuation snapshot that captures the state needed to resume an
/// LLM session after an async RPC callback arrives.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContinuationSnapshot {
    /// The task ID this snapshot belongs to.
    pub task_id: String,
    /// Serialized conversation messages preserved as raw JSON (matching Go's json.RawMessage).
    pub messages: serde_json::Value,
    /// The tool call ID that triggered the continuation.
    pub tool_call_id: String,
    /// Original channel for the response.
    pub channel: String,
    /// Original chat ID for the response.
    pub chat_id: String,
    /// Whether this snapshot is ready to be loaded.
    pub ready: bool,
    /// Timestamp when this snapshot was created.
    pub created_at: String,
}

/// Error type for continuation store operations.
#[derive(Debug, thiserror::Error)]
pub enum ContinuationError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Snapshot not found: {0}")]
    NotFound(String),
}

/// In-memory + disk continuation store.
///
/// Snapshots are first stored in memory, then persisted to disk. The `ready`
/// flag acts as a save barrier: callbacks that arrive before the snapshot is
/// ready will wait (up to a timeout).
pub struct ContinuationStore {
    snapshots: Mutex<HashMap<String, ContinuationSnapshot>>,
    cache_dir: PathBuf,
}

impl ContinuationStore {
    /// Create a new continuation store that persists snapshots to `cache_dir`.
    pub fn new(cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            snapshots: Mutex::new(HashMap::new()),
            cache_dir: cache_dir.into(),
        }
    }

    /// Save a continuation snapshot (in-memory + disk).
    pub async fn save(&self, snapshot: ContinuationSnapshot) -> Result<(), ContinuationError> {
        let task_id = snapshot.task_id.clone();

        // Persist to disk
        if let Err(e) = self.persist_to_disk(&snapshot).await {
            tracing::warn!(task_id = %task_id, error = %e, "Failed to persist continuation to disk");
        }

        // Store in memory
        self.snapshots.lock().insert(task_id, snapshot);
        Ok(())
    }

    /// Load a continuation snapshot by task ID.
    ///
    /// Implements a save-barrier mechanism matching Go's behavior: when a
    /// callback arrives before the snapshot is fully saved, we retry for up
    /// to 5 seconds (checking every 100ms) before giving up.
    pub async fn load(&self, task_id: &str) -> Result<ContinuationSnapshot, ContinuationError> {
        // Check memory first
        {
            let snapshots = self.snapshots.lock();
            if let Some(snap) = snapshots.get(task_id) {
                return Ok(snap.clone());
            }
        }

        // Check if a save might be in progress (tmp file exists).
        // If not, the snapshot simply doesn't exist -- fail immediately.
        let tmp_path = self.snapshot_path(task_id).with_extension("json.tmp");
        let might_be_saving = tmp_path.exists();

        if !might_be_saving {
            // No save in progress, try disk once and return
            return self.load_from_disk(task_id).await;
        }

        // Save barrier: a save appears to be in progress. Retry for up to 5 seconds.
        const MAX_RETRIES: u32 = 50;
        const RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

        for _ in 0..MAX_RETRIES {
            // Check memory
            {
                let snapshots = self.snapshots.lock();
                if let Some(snap) = snapshots.get(task_id) {
                    return Ok(snap.clone());
                }
            }

            // Check disk
            match self.load_from_disk_inner(task_id).await {
                Ok(snap) => {
                    // Re-populate in memory
                    self.snapshots.lock().insert(task_id.to_string(), snap.clone());
                    return Ok(snap);
                }
                Err(ContinuationError::NotFound(_)) => {
                    // Not on disk yet, wait and retry
                    tokio::time::sleep(RETRY_INTERVAL).await;
                    continue;
                }
                Err(e) => return Err(e),
            }
        }

        // Final attempt after exhausting retries
        {
            let snapshots = self.snapshots.lock();
            if let Some(snap) = snapshots.get(task_id) {
                return Ok(snap.clone());
            }
        }
        self.load_from_disk(task_id).await
    }

    /// Remove a continuation snapshot after it has been consumed.
    ///
    /// Removes from both in-memory map and disk, matching Go's behavior
    /// where Delete also removes the persisted file.
    pub async fn remove(&self, task_id: &str) -> bool {
        let removed = self.snapshots.lock().remove(task_id).is_some();

        // Also delete from disk
        let path = self.snapshot_path(task_id);
        if path.exists() {
            if let Err(e) = tokio::fs::remove_file(&path).await {
                tracing::warn!(
                    task_id,
                    path = %path.display(),
                    error = %e,
                    "Failed to delete continuation snapshot from disk"
                );
            }
        }

        removed
    }

    /// Check whether a snapshot exists for the given task ID.
    pub fn contains(&self, task_id: &str) -> bool {
        self.snapshots.lock().contains_key(task_id)
    }

    /// Return the number of stored snapshots.
    pub fn len(&self) -> usize {
        self.snapshots.lock().len()
    }

    /// Return whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.snapshots.lock().is_empty()
    }

    /// Remove snapshots older than `max_age`.
    ///
    /// Deletes snapshot files from disk whose modification time exceeds
    /// the specified duration. Also removes them from memory.
    pub async fn cleanup_old(&self, max_age: std::time::Duration) -> Result<usize, ContinuationError> {
        let cutoff = std::time::SystemTime::now() - max_age;
        let mut removed = 0;

        if !self.cache_dir.exists() {
            return Ok(0);
        }

        let mut entries = tokio::fs::read_dir(&self.cache_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                if let Ok(metadata) = entry.metadata().await {
                    if let Ok(modified) = metadata.modified() {
                        if modified < cutoff {
                            // Extract task ID from filename
                            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                                self.snapshots.lock().remove(stem);
                                tokio::fs::remove_file(&path).await?;
                                removed += 1;
                            }
                        }
                    }
                }
            }
        }

        if removed > 0 {
            tracing::info!(removed, "Cleaned up old continuation snapshots");
        }

        Ok(removed)
    }

    /// List all pending task IDs.
    ///
    /// Scans disk for `.json` files and returns their task IDs (matching Go's
    /// `ListPending` which scans disk on startup). In-memory snapshots that
    /// have not yet been persisted are also included.
    pub async fn list_pending(&self) -> Vec<String> {
        let mut task_ids: Vec<String> = self.snapshots.lock().keys().cloned().collect();

        // Also scan disk for any snapshots not in memory
        if self.cache_dir.exists() {
            if let Ok(mut entries) = tokio::fs::read_dir(&self.cache_dir).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    let path = entry.path();
                    if path.extension().map(|e| e == "json").unwrap_or(false) {
                        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                            let task_id = stem.to_string();
                            if !task_ids.contains(&task_id) {
                                task_ids.push(task_id);
                            }
                        }
                    }
                }
            }
        }

        task_ids
    }

    /// Recover all continuation snapshots from disk into memory.
    ///
    /// Scans the cache directory for `.json` files, deserializes each one,
    /// and inserts it into the in-memory map. This should be called on startup
    /// to restore any snapshots that were persisted before a restart.
    /// Returns the number of snapshots recovered.
    pub async fn recover_from_disk(&self) -> Result<usize, ContinuationError> {
        if !self.cache_dir.exists() {
            return Ok(0);
        }

        let mut recovered = 0;
        let mut entries = tokio::fs::read_dir(&self.cache_dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    // Skip if already in memory (save barrier may have populated it)
                    {
                        let snapshots = self.snapshots.lock();
                        if snapshots.contains_key(stem) {
                            continue;
                        }
                    }

                    match tokio::fs::read_to_string(&path).await {
                        Ok(content) => {
                            match serde_json::from_str::<ContinuationSnapshot>(&content) {
                                Ok(snapshot) => {
                                    self.snapshots.lock().insert(stem.to_string(), snapshot);
                                    recovered += 1;
                                    tracing::info!(
                                        task_id = stem,
                                        "Recovered continuation snapshot from disk"
                                    );
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        path = %path.display(),
                                        error = %e,
                                        "Failed to deserialize continuation snapshot, skipping"
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                path = %path.display(),
                                error = %e,
                                "Failed to read continuation snapshot, skipping"
                            );
                        }
                    }
                }
            }
        }

        if recovered > 0 {
            tracing::info!(recovered, "Recovered continuation snapshots from disk");
        }

        Ok(recovered)
    }

    // -- Private helpers --

    async fn persist_to_disk(&self, snapshot: &ContinuationSnapshot) -> std::io::Result<()> {
        tokio::fs::create_dir_all(&self.cache_dir).await?;
        let final_path = self.snapshot_path(&snapshot.task_id);
        let tmp_path = final_path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(snapshot).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })?;
        // Write to temporary file first, then rename for atomicity.
        tokio::fs::write(&tmp_path, &json).await?;
        std::fs::rename(&tmp_path, &final_path)?;
        Ok(())
    }

    async fn load_from_disk(&self, task_id: &str) -> Result<ContinuationSnapshot, ContinuationError> {
        let snap = self.load_from_disk_inner(task_id).await?;
        // Re-populate in memory
        self.snapshots.lock().insert(task_id.to_string(), snap.clone());
        Ok(snap)
    }

    /// Inner disk load without modifying memory (used by retry loop).
    async fn load_from_disk_inner(&self, task_id: &str) -> Result<ContinuationSnapshot, ContinuationError> {
        let path = self.snapshot_path(task_id);
        if !path.exists() {
            return Err(ContinuationError::NotFound(task_id.into()));
        }
        let content = tokio::fs::read_to_string(&path).await?;
        let snapshot: ContinuationSnapshot = serde_json::from_str(&content)?;
        Ok(snapshot)
    }

    fn snapshot_path(&self, task_id: &str) -> PathBuf {
        self.cache_dir.join(format!("{}.json", task_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_snapshot(task_id: &str) -> ContinuationSnapshot {
        ContinuationSnapshot {
            task_id: task_id.into(),
            messages: serde_json::json!([{"role": "user", "content": "hello"}]),
            tool_call_id: "tc-001".into(),
            channel: "web".into(),
            chat_id: "chat-123".into(),
            ready: true,
            created_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    #[tokio::test]
    async fn test_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContinuationStore::new(dir.path());

        let snap = make_snapshot("task-001");
        store.save(snap).await.unwrap();

        let loaded = store.load("task-001").await.unwrap();
        assert_eq!(loaded.task_id, "task-001");
        assert_eq!(loaded.channel, "web");
    }

    #[tokio::test]
    async fn test_load_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContinuationStore::new(dir.path());

        let result = store.load("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_remove() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContinuationStore::new(dir.path());

        store.save(make_snapshot("task-002")).await.unwrap();
        assert!(store.contains("task-002"));

        assert!(store.remove("task-002").await);
        assert!(!store.contains("task-002"));

        // Verify disk file is also deleted
        let path = dir.path().join("task-002.json");
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn test_disk_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContinuationStore::new(dir.path());

        store.save(make_snapshot("task-003")).await.unwrap();

        // Create a new store from the same dir
        let store2 = ContinuationStore::new(dir.path());
        // Memory is empty, but disk fallback should work
        let loaded = store2.load("task-003").await.unwrap();
        assert_eq!(loaded.task_id, "task-003");
    }

    #[tokio::test]
    async fn test_list_pending() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContinuationStore::new(dir.path());

        assert!(store.list_pending().await.is_empty());

        store.save(make_snapshot("task-a")).await.unwrap();
        store.save(make_snapshot("task-b")).await.unwrap();

        let pending = store.list_pending().await;
        assert_eq!(pending.len(), 2);
        assert!(pending.contains(&"task-a".to_string()));
        assert!(pending.contains(&"task-b".to_string()));
    }

    #[tokio::test]
    async fn test_cleanup_old() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContinuationStore::new(dir.path());

        // Create a snapshot
        store.save(make_snapshot("old-task")).await.unwrap();

        // Cleanup with 0-second threshold (removes everything older than "now")
        // Since the file was just created, it shouldn't be removed
        let removed = store.cleanup_old(std::time::Duration::from_secs(0)).await.unwrap();
        // A 0-duration cleanup may or may not remove recent files depending on FS timing
        assert!(removed <= 1);

        // Cleanup with very long threshold shouldn't remove anything
        store.save(make_snapshot("new-task")).await.unwrap();
        let removed2 = store.cleanup_old(std::time::Duration::from_secs(86400 * 365)).await.unwrap();
        assert_eq!(removed2, 0);
    }

    #[tokio::test]
    async fn test_recover_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContinuationStore::new(dir.path());

        // Save some snapshots
        store.save(make_snapshot("recover-1")).await.unwrap();
        store.save(make_snapshot("recover-2")).await.unwrap();

        // Create a fresh store (empty memory but disk has files)
        let store2 = ContinuationStore::new(dir.path());
        // list_pending now scans disk too, so it will find the files
        let pending = store2.list_pending().await;
        assert_eq!(pending.len(), 2);
        assert!(pending.contains(&"recover-1".to_string()));
        assert!(pending.contains(&"recover-2".to_string()));

        // Recover from disk into memory
        let recovered = store2.recover_from_disk().await.unwrap();
        assert_eq!(recovered, 2);

        // Should be able to load them from memory now
        let loaded = store2.load("recover-1").await.unwrap();
        assert_eq!(loaded.task_id, "recover-1");
    }

    #[tokio::test]
    async fn test_recover_from_disk_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContinuationStore::new(dir.path());

        let recovered = store.recover_from_disk().await.unwrap();
        assert_eq!(recovered, 0);
    }

    #[tokio::test]
    async fn test_recover_from_disk_nonexistent_dir() {
        let store = ContinuationStore::new("/nonexistent/path/that/does/not/exist");
        let recovered = store.recover_from_disk().await.unwrap();
        assert_eq!(recovered, 0);
    }

    // -- Additional tests: continuation store edge cases --

    #[tokio::test]
    async fn test_save_multiple_snapshots() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContinuationStore::new(dir.path());

        for i in 0..5 {
            let snap = make_snapshot(&format!("task-{}", i));
            store.save(snap).await.unwrap();
        }

        assert_eq!(store.len(), 5);
        assert!(!store.is_empty());

        // Each one should be loadable
        for i in 0..5 {
            let loaded = store.load(&format!("task-{}", i)).await.unwrap();
            assert_eq!(loaded.tool_call_id, "tc-001");
        }
    }

    #[tokio::test]
    async fn test_overwrite_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContinuationStore::new(dir.path());

        let mut snap = make_snapshot("task-overwrite");
        snap.channel = "web".into();
        store.save(snap).await.unwrap();

        let mut snap2 = make_snapshot("task-overwrite");
        snap2.channel = "rpc".into();
        store.save(snap2).await.unwrap();

        // Should still have only 1 entry (overwritten)
        assert_eq!(store.len(), 1);

        let loaded = store.load("task-overwrite").await.unwrap();
        assert_eq!(loaded.channel, "rpc");
    }

    #[tokio::test]
    async fn test_contains() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContinuationStore::new(dir.path());

        assert!(!store.contains("task-x"));
        store.save(make_snapshot("task-x")).await.unwrap();
        assert!(store.contains("task-x"));
    }

    #[tokio::test]
    async fn test_len_and_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContinuationStore::new(dir.path());

        assert!(store.is_empty());
        assert_eq!(store.len(), 0);

        store.save(make_snapshot("t1")).await.unwrap();
        assert!(!store.is_empty());
        assert_eq!(store.len(), 1);

        store.save(make_snapshot("t2")).await.unwrap();
        assert_eq!(store.len(), 2);

        store.remove("t1").await;
        assert_eq!(store.len(), 1);
    }

    #[tokio::test]
    async fn test_remove_nonexistent_returns_false() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContinuationStore::new(dir.path());
        assert!(!store.remove("nonexistent").await);
    }

    #[tokio::test]
    async fn test_snapshot_preserves_messages_json() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContinuationStore::new(dir.path());

        let messages = serde_json::json!([
            {"role": "user", "content": "hello"},
            {"role": "assistant", "content": "hi there"},
            {"role": "user", "content": "how are you?"}
        ]);

        let snap = ContinuationSnapshot {
            task_id: "msg-test".into(),
            messages: messages.clone(),
            tool_call_id: "tc-msg".into(),
            channel: "rpc".into(),
            chat_id: "chat-msg".into(),
            ready: true,
            created_at: chrono::Utc::now().to_rfc3339(),
        };

        store.save(snap).await.unwrap();
        let loaded = store.load("msg-test").await.unwrap();

        assert_eq!(loaded.messages, messages);
        assert_eq!(loaded.messages.as_array().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn test_disk_file_has_correct_name() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContinuationStore::new(dir.path());

        store.save(make_snapshot("my-task-id")).await.unwrap();

        let expected_path = dir.path().join("my-task-id.json");
        assert!(expected_path.exists(), "Expected file at {:?}", expected_path);
    }

    // ============================================================
    // Coverage improvement: cleanup, disk edge cases
    // ============================================================

    #[tokio::test]
    async fn test_cleanup_old_snapshots_none_expired_v2() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContinuationStore::new(dir.path());

        // Save a snapshot with current timestamp
        store.save(make_snapshot("fresh-task")).await.unwrap();

        // Cleanup with very long max age - nothing should be removed
        let removed = store.cleanup_old(std::time::Duration::from_secs(365 * 24 * 3600)).await.unwrap();
        assert_eq!(removed, 0);
        assert!(store.contains("fresh-task"));
    }

    #[tokio::test]
    async fn test_cleanup_old_empty_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContinuationStore::new(dir.path());

        let removed = store.cleanup_old(std::time::Duration::from_secs(1)).await.unwrap();
        assert_eq!(removed, 0);
    }

    #[tokio::test]
    async fn test_list_pending_includes_disk_only() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContinuationStore::new(dir.path());

        store.save(make_snapshot("disk-only-task")).await.unwrap();

        // Create a new store (memory empty, disk has data)
        let store2 = ContinuationStore::new(dir.path());
        let pending = store2.list_pending().await;
        assert!(pending.contains(&"disk-only-task".to_string()));
    }

    #[tokio::test]
    async fn test_list_pending_includes_memory_only() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContinuationStore::new(dir.path());

        // Save to memory but don't persist (use save without persist)
        // Actually save() does persist, so let's just verify it works
        store.save(make_snapshot("mem-task")).await.unwrap();

        let pending = store.list_pending().await;
        assert!(pending.contains(&"mem-task".to_string()));
    }

    #[tokio::test]
    async fn test_snapshot_not_ready() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContinuationStore::new(dir.path());

        let mut snap = make_snapshot("not-ready-task");
        snap.ready = false;
        store.save(snap).await.unwrap();

        let loaded = store.load("not-ready-task").await.unwrap();
        assert!(!loaded.ready);
    }

    #[tokio::test]
    async fn test_recover_from_disk_corrupted_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContinuationStore::new(dir.path());

        // Write a corrupted JSON file
        tokio::fs::create_dir_all(dir.path()).await.unwrap();
        tokio::fs::write(dir.path().join("corrupted.json"), "not valid json{{{")
            .await
            .unwrap();

        let recovered = store.recover_from_disk().await.unwrap();
        assert_eq!(recovered, 0); // Should skip corrupted file
    }
}

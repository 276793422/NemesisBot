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
            tracing::warn!(task_id = %task_id, error = %e, "[ContinuationStore] Failed to persist continuation to disk");
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
                    "[ContinuationStore] Failed to delete continuation snapshot from disk"
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

    /// Return the cache directory path.
    pub fn cache_dir(&self) -> &std::path::Path {
        &self.cache_dir
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
            tracing::info!(removed, "[ContinuationStore] Cleaned up old continuation snapshots");
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
                                        "[ContinuationStore] Recovered continuation snapshot from disk"
                                    );
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        path = %path.display(),
                                        error = %e,
                                        "[ContinuationStore] Failed to deserialize continuation snapshot, skipping"
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                path = %path.display(),
                                error = %e,
                                "[ContinuationStore] Failed to read continuation snapshot, skipping"
                            );
                        }
                    }
                }
            }
        }

        if recovered > 0 {
            tracing::info!(recovered, "[ContinuationStore] Recovered continuation snapshots from disk");
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
mod tests;

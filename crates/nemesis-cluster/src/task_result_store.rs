//! Task result store - persists task completion results.
//!
//! Stores the results of completed cluster tasks, allowing consumers to
//! retrieve them asynchronously. Supports optional disk persistence so that
//! results survive process restarts (matching the Go implementation).
//!
//! # Go-compatible types
//!
//! The module also provides [`GoTaskResultEntry`], [`GoTaskResultIndex`],
//! and [`GoTaskResultStore`] which mirror Go's `TaskResultEntry`,
//! `TaskResultIndex`, and `TaskResultStore` types. These track in-memory
//! "running" state and on-disk "done" state with atomic index persistence.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tracing;

/// A stored task result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    /// The task ID.
    pub task_id: String,
    /// The action that was performed.
    pub action: String,
    /// Result payload (JSON).
    pub result: serde_json::Value,
    /// Whether the task succeeded.
    pub success: bool,
    /// When the result was stored (RFC3339).
    pub stored_at: String,
}

/// In-memory task result store with optional disk persistence.
///
/// When disk persistence is enabled (via [`with_disk_persistence`]), each
/// result is written to `{cache_dir}/{task_id}.json` and can be restored on
/// startup with [`load_from_disk`].
pub struct TaskResultStore {
    results: Mutex<HashMap<String, TaskResult>>,
    max_size: usize,
    cache_dir: Option<PathBuf>,
}

impl TaskResultStore {
    /// Create a new in-memory result store with a maximum capacity.
    pub fn new(max_size: usize) -> Self {
        Self {
            results: Mutex::new(HashMap::new()),
            max_size,
            cache_dir: None,
        }
    }

    /// Create a new result store that persists results to disk.
    ///
    /// The `cache_dir` is created automatically if it does not exist.
    /// Each result is written to `{cache_dir}/{task_id}.json`.
    pub fn with_disk_persistence(max_size: usize, cache_dir: impl Into<PathBuf>) -> Self {
        let dir = cache_dir.into();
        // Create directory synchronously during construction so it's ready
        // for subsequent operations.
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!("failed to create cache_dir {:?}: {}", dir, e);
        }
        Self {
            results: Mutex::new(HashMap::new()),
            max_size,
            cache_dir: Some(dir),
        }
    }

    /// Store a successful task result.
    pub fn store_success(&self, task_id: &str, action: &str, result: serde_json::Value) {
        self.store(TaskResult {
            task_id: task_id.into(),
            action: action.into(),
            result,
            success: true,
            stored_at: chrono::Utc::now().to_rfc3339(),
        });
    }

    /// Store a failed task result.
    pub fn store_failure(&self, task_id: &str, action: &str, error: &str) {
        self.store(TaskResult {
            task_id: task_id.into(),
            action: action.into(),
            result: serde_json::json!({ "error": error }),
            success: false,
            stored_at: chrono::Utc::now().to_rfc3339(),
        });
    }

    /// Retrieve a task result by task ID.
    pub fn get(&self, task_id: &str) -> Option<TaskResult> {
        self.results.lock().get(task_id).cloned()
    }

    /// Remove a task result.
    pub fn remove(&self, task_id: &str) -> bool {
        let removed = self.results.lock().remove(task_id).is_some();
        if removed {
            self.delete_from_disk(task_id);
        }
        removed
    }

    /// Remove a result from both memory and disk after it has been delivered
    /// to the A-side (consumed). This is the preferred cleanup method after
    /// a result has been successfully handed off.
    pub fn cleanup_delivered(&self, task_id: &str) -> bool {
        self.remove(task_id)
    }

    /// Return the number of stored results.
    pub fn len(&self) -> usize {
        self.results.lock().len()
    }

    /// Return whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.results.lock().is_empty()
    }

    /// Clear all stored results from memory (disk files are **not** removed).
    pub fn clear(&self) {
        self.results.lock().clear();
    }

    /// Load all result files from the cache directory on startup.
    ///
    /// Reads every `*.json` file in `cache_dir` and inserts them into the
    /// in-memory map. Files that fail to parse are skipped with a warning.
    /// Returns the number of results successfully loaded.
    ///
    /// Does nothing if disk persistence is not enabled.
    pub fn load_from_disk(&self) -> usize {
        let dir = match &self.cache_dir {
            Some(d) => d,
            None => return 0,
        };

        let entries = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(e) => {
                tracing::warn!("failed to read cache_dir {:?}: {}", dir, e);
                return 0;
            }
        };

        let mut loaded = 0usize;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            match std::fs::read_to_string(&path) {
                Ok(data) => match serde_json::from_str::<TaskResult>(&data) {
                    Ok(result) => {
                        let mut results = self.results.lock();
                        // Respect max_size during restore
                        if results.len() >= self.max_size {
                            break;
                        }
                        results.insert(result.task_id.clone(), result);
                        loaded += 1;
                    }
                    Err(e) => {
                        tracing::warn!("skipping invalid result file {:?}: {}", path, e);
                    }
                },
                Err(e) => {
                    tracing::warn!("failed to read result file {:?}: {}", path, e);
                }
            }
        }
        loaded
    }

    // ---- Internal helpers ----

    fn store(&self, result: TaskResult) {
        let mut results = self.results.lock();
        // Evict oldest if at capacity
        if results.len() >= self.max_size {
            // Simple eviction: remove a random entry
            if let Some(key) = results.keys().next().cloned() {
                let evicted = results.remove(&key);
                // Delete evicted entry from disk as well
                if let Some(evicted) = evicted {
                    drop(results); // release lock before disk I/O
                    self.delete_from_disk(&evicted.task_id);
                    results = self.results.lock();
                }
            }
        }
        results.insert(result.task_id.clone(), result.clone());
        drop(results); // release lock before disk I/O
        self.write_to_disk(&result);
    }

    fn write_to_disk(&self, result: &TaskResult) {
        if let Some(dir) = &self.cache_dir {
            let path = dir.join(format!("{}.json", result.task_id));
            match serde_json::to_string_pretty(result) {
                Ok(json) => {
                    // Atomic write: write to temp file, then rename.
                    let tmp_path = path.with_extension("json.tmp");
                    if let Err(e) = std::fs::write(&tmp_path, &json) {
                        tracing::warn!(
                            "failed to write result temp file {:?}: {}",
                            tmp_path,
                            e
                        );
                        return;
                    }
                    if let Err(e) = std::fs::rename(&tmp_path, &path) {
                        tracing::warn!(
                            "failed to rename result file {:?} -> {:?}: {}",
                            tmp_path,
                            path,
                            e
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "failed to serialize result {}: {}",
                        result.task_id,
                        e
                    );
                }
            }
        }
    }

    fn delete_from_disk(&self, task_id: &str) {
        if let Some(dir) = &self.cache_dir {
            let path = dir.join(format!("{}.json", task_id));
            if path.exists() {
                if let Err(e) = std::fs::remove_file(&path) {
                    tracing::warn!("failed to delete result file {:?}: {}", path, e);
                }
            }
        }
    }
}

impl Default for TaskResultStore {
    fn default() -> Self {
        Self::new(1000)
    }
}

// ---------------------------------------------------------------------------
// AsyncTaskResultStore — async wrapper for tokio-based disk operations
// ---------------------------------------------------------------------------

/// Async wrapper around [`TaskResultStore`] that uses `tokio::fs` for all
/// disk I/O, keeping the calling tokio runtime unblocked.
///
/// The in-memory operations are identical to the sync version (they are fast
/// and lock-free enough to not warrant a dedicated async boundary). Disk
/// writes, reads, and deletes are offloaded to tokio's blocking thread pool.
pub struct AsyncTaskResultStore {
    inner: TaskResultStore,
}

impl AsyncTaskResultStore {
    /// Create a new async store backed by the given sync store.
    pub fn new(inner: TaskResultStore) -> Self {
        Self { inner }
    }

    /// Create an async store with disk persistence.
    pub fn with_disk_persistence(max_size: usize, cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            inner: TaskResultStore::with_disk_persistence(max_size, cache_dir),
        }
    }

    /// Store a successful task result (async — disk write happens in background).
    pub async fn store_success_async(
        &self,
        task_id: &str,
        action: &str,
        result: serde_json::Value,
    ) {
        let tr = TaskResult {
            task_id: task_id.into(),
            action: action.into(),
            result,
            success: true,
            stored_at: chrono::Utc::now().to_rfc3339(),
        };
        self.store_async(tr).await;
    }

    /// Store a failed task result (async — disk write happens in background).
    pub async fn store_failure_async(&self, task_id: &str, action: &str, error: &str) {
        let tr = TaskResult {
            task_id: task_id.into(),
            action: action.into(),
            result: serde_json::json!({ "error": error }),
            success: false,
            stored_at: chrono::Utc::now().to_rfc3339(),
        };
        self.store_async(tr).await;
    }

    /// Retrieve a task result by ID (memory-only, no disk read).
    pub fn get_async(&self, task_id: &str) -> Option<TaskResult> {
        self.inner.get(task_id)
    }

    /// Remove a delivered result from both memory and disk (async).
    pub async fn cleanup_delivered_async(&self, task_id: &str) -> bool {
        // Remove from memory synchronously
        let existed = self.inner.results.lock().remove(task_id).is_some();
        if existed {
            self.delete_from_disk_async(task_id).await;
        }
        existed
    }

    /// Load all results from disk into memory on startup (async).
    pub async fn load_from_disk_async(&self) -> usize {
        let dir = match &self.inner.cache_dir {
            Some(d) => d.clone(),
            None => return 0,
        };

        let mut read_dir = match fs::read_dir(&dir).await {
            Ok(rd) => rd,
            Err(e) => {
                tracing::warn!("failed to read cache_dir {:?}: {}", dir, e);
                return 0;
            }
        };

        let mut loaded = 0usize;
        while let Ok(Some(entry)) = read_dir.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            match fs::read_to_string(&path).await {
                Ok(data) => match serde_json::from_str::<TaskResult>(&data) {
                    Ok(result) => {
                        let mut results = self.inner.results.lock();
                        if results.len() >= self.inner.max_size {
                            break;
                        }
                        results.insert(result.task_id.clone(), result);
                        loaded += 1;
                    }
                    Err(e) => {
                        tracing::warn!("skipping invalid result file {:?}: {}", path, e);
                    }
                },
                Err(e) => {
                    tracing::warn!("failed to read result file {:?}: {}", path, e);
                }
            }
        }
        loaded
    }

    // ---- Internal helpers ----

    async fn store_async(&self, result: TaskResult) {
        // 1. Insert into memory (sync, fast)
        {
            let mut results = self.inner.results.lock();
            if results.len() >= self.inner.max_size {
                if let Some(key) = results.keys().next().cloned() {
                    let evicted = results.remove(&key);
                    if let Some(evicted) = evicted {
                        let dir = self.inner.cache_dir.clone();
                        let evicted_id = evicted.task_id.clone();
                        drop(results);
                        // Delete evicted entry from disk asynchronously
                        if let Some(dir) = dir {
                            let path = dir.join(format!("{}.json", evicted_id));
                            let _ = fs::remove_file(path).await;
                        }
                        results = self.inner.results.lock();
                    }
                }
            }
            results.insert(result.task_id.clone(), result.clone());
        }

        // 2. Write to disk asynchronously (result was cloned before move)
        self.write_to_disk_async(&result).await;
    }

    async fn write_to_disk_async(&self, result: &TaskResult) {
        if let Some(dir) = &self.inner.cache_dir {
            let path = dir.join(format!("{}.json", result.task_id));
            match serde_json::to_string_pretty(result) {
                Ok(json) => {
                    let tmp_path = path.with_extension("json.tmp");
                    match fs::write(&tmp_path, &json).await {
                        Ok(()) => {
                            if let Err(e) = fs::rename(&tmp_path, &path).await {
                                tracing::warn!(
                                    "failed to rename async result file {:?} -> {:?}: {}",
                                    tmp_path,
                                    path,
                                    e
                                );
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                "failed to write async result temp file {:?}: {}",
                                tmp_path,
                                e
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "failed to serialize result {}: {}",
                        result.task_id,
                        e
                    );
                }
            }
        }
    }

    async fn delete_from_disk_async(&self, task_id: &str) {
        if let Some(dir) = &self.inner.cache_dir {
            let path = dir.join(format!("{}.json", task_id));
            if Path::new(&path).exists() {
                if let Err(e) = fs::remove_file(&path).await {
                    tracing::warn!("failed to delete async result file {:?}: {}", path, e);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Go-compatible task result types (mirrors Go's TaskResultEntry/Index/Store)
// ---------------------------------------------------------------------------

/// A task result entry in the Go-compatible format.
///
/// Mirrors Go's `TaskResultEntry` struct. Tracks both "running" (in-memory)
/// and "done" (persisted to disk) task states. The B-side (worker node)
/// uses this to track task lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoTaskResultEntry {
    /// Task identifier.
    pub task_id: String,
    /// Current status: "running" or "done".
    pub status: String,
    /// Result status when done: "success" or "error".
    /// Only meaningful when `status == "done"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_status: Option<String>,
    /// Response content on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<String>,
    /// Error message on failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Source node that handled the task.
    pub source_node: String,
    /// When the entry was created (RFC3339).
    pub created_at: String,
    /// When the entry was last updated (RFC3339).
    pub updated_at: String,
}

/// On-disk index for Go-style task results.
///
/// Mirrors Go's `TaskResultIndex`. Maps task IDs to their result entries.
/// Only contains "done" entries; "running" entries are tracked in memory only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoTaskResultIndex {
    /// Map of task ID to result entry.
    pub tasks: HashMap<String, GoTaskResultEntry>,
}

impl Default for GoTaskResultIndex {
    fn default() -> Self {
        Self {
            tasks: HashMap::new(),
        }
    }
}

/// Go-compatible task result store.
///
/// Mirrors Go's `TaskResultStore`. Tracks:
/// - **Running** state: in-memory only (lost on process restart; the A-side
///   will re-query).
/// - **Done** state: persisted to disk (`{data_dir}/{task_id}.json`) with an
///   atomic index file (`{data_dir}/index.json`).
///
/// This is the B-side store for task results, used by worker nodes to
/// persist results that the A-side can later query.
pub struct GoTaskResultStore {
    data_dir: PathBuf,
    index_path: PathBuf,
    /// In-memory tracking of which tasks are currently running.
    running: Mutex<HashMap<String, bool>>,
    /// In-memory cache of the disk index.
    index: Mutex<GoTaskResultIndex>,
}

impl GoTaskResultStore {
    /// Creates a new `GoTaskResultStore` rooted at `{workspace}/cluster/task_results/`.
    ///
    /// The directory is created if it does not exist. The index is loaded from
    /// disk if present; otherwise an empty index is used.
    pub fn new(workspace: &Path) -> std::result::Result<Self, String> {
        let data_dir = workspace.join("cluster").join("task_results");
        std::fs::create_dir_all(&data_dir)
            .map_err(|e| format!("failed to create task_results directory: {e}"))?;

        let index_path = data_dir.join("index.json");

        let store = Self {
            data_dir,
            index_path,
            running: Mutex::new(HashMap::new()),
            index: Mutex::new(GoTaskResultIndex::default()),
        };

        // Load index from disk; failure is non-fatal
        if let Err(e) = store.load_index() {
            tracing::warn!("failed to load task result index (starting fresh): {e}");
            *store.index.lock() = GoTaskResultIndex::default();
        }

        Ok(store)
    }

    /// Marks a task as running (in-memory only).
    ///
    /// Mirrors Go's `TaskResultStore.SetRunning()`. Running state is not
    /// persisted; if the process restarts, the A-side will re-query and
    /// find the task missing.
    pub fn set_running(&self, task_id: &str, source_node: &str) {
        self.running.lock().insert(task_id.to_string(), true);
        let _ = source_node; // stored only on done
    }

    /// Writes a completed result (data file + index + disk).
    ///
    /// Mirrors Go's `TaskResultStore.SetResult()`. Also clears the running
    /// marker for this task.
    pub fn set_result(
        &self,
        task_id: &str,
        result_status: &str,
        response: &str,
        error_msg: &str,
        source_node: &str,
    ) -> std::result::Result<(), String> {
        let now = chrono::Utc::now().to_rfc3339();
        let entry = GoTaskResultEntry {
            task_id: task_id.to_string(),
            status: "done".to_string(),
            result_status: Some(result_status.to_string()),
            response: if response.is_empty() {
                None
            } else {
                Some(response.to_string())
            },
            error: if error_msg.is_empty() {
                None
            } else {
                Some(error_msg.to_string())
            },
            source_node: source_node.to_string(),
            created_at: now.clone(),
            updated_at: now,
        };

        // Write data file (atomic: tmp + rename)
        let json = serde_json::to_string_pretty(&entry)
            .map_err(|e| format!("failed to marshal task result: {e}"))?;
        let file_path = self.data_dir.join(format!("{task_id}.json"));
        let tmp_path = self.data_dir.join(format!("{task_id}.json.tmp"));
        std::fs::write(&tmp_path, &json)
            .map_err(|e| format!("failed to write tmp file: {e}"))?;
        std::fs::rename(&tmp_path, &file_path)
            .map_err(|e| format!("failed to rename tmp file: {e}"))?;

        // Update in-memory index
        self.index.lock().tasks.insert(task_id.to_string(), entry);

        // Clear running marker
        self.running.lock().remove(task_id);

        // Persist index to disk
        self.save_index_locked()
    }

    /// Retrieves a task result by ID.
    ///
    /// Mirrors Go's `TaskResultStore.Get()`. Checks running state first,
    /// then the index. Returns `None` if the task is unknown.
    pub fn get(&self, task_id: &str) -> Option<GoTaskResultEntry> {
        // Check running first
        if self.running.lock().contains_key(task_id) {
            return Some(GoTaskResultEntry {
                task_id: task_id.to_string(),
                status: "running".to_string(),
                result_status: None,
                response: None,
                error: None,
                source_node: String::new(),
                created_at: String::new(),
                updated_at: String::new(),
            });
        }

        // Check index
        self.index.lock().tasks.get(task_id).cloned()
    }

    /// Deletes a task result (data file + index entry + disk).
    ///
    /// Mirrors Go's `TaskResultStore.Delete()`.
    pub fn delete(&self, task_id: &str) -> std::result::Result<(), String> {
        // Delete data file (ignore errors, may not exist)
        let file_path = self.data_dir.join(format!("{task_id}.json"));
        let _ = std::fs::remove_file(&file_path);

        // Update index
        self.index.lock().tasks.remove(task_id);
        self.running.lock().remove(task_id);

        self.save_index_locked()
    }

    /// Returns the number of "done" entries in the index.
    pub fn done_count(&self) -> usize {
        self.index.lock().tasks.len()
    }

    /// Returns whether a task is currently in "running" state.
    pub fn is_running(&self, task_id: &str) -> bool {
        self.running.lock().contains_key(task_id)
    }

    // ---- Internal helpers ----

    fn load_index(&self) -> std::result::Result<(), String> {
        if !self.index_path.exists() {
            return Ok(());
        }

        let data = std::fs::read_to_string(&self.index_path)
            .map_err(|e| format!("failed to read index: {e}"))?;

        let idx: GoTaskResultIndex = serde_json::from_str(&data)
            .map_err(|e| format!("failed to parse index: {e}"))?;

        *self.index.lock() = idx;
        Ok(())
    }

    fn save_index_locked(&self) -> std::result::Result<(), String> {
        let json = serde_json::to_string_pretty(&*self.index.lock())
            .map_err(|e| format!("failed to marshal index: {e}"))?;
        let tmp_path = self.index_path.with_extension("json.tmp");
        std::fs::write(&tmp_path, &json)
            .map_err(|e| format!("failed to write index tmp: {e}"))?;
        std::fs::rename(&tmp_path, &self.index_path)
            .map_err(|e| format!("failed to rename index tmp: {e}"))?;
        Ok(())
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs as std_fs;

    // ---- Original tests (unchanged) ----

    #[test]
    fn test_store_and_get_success() {
        let store = TaskResultStore::new(100);
        store.store_success("task-1", "peer_chat", serde_json::json!("response"));

        let result = store.get("task-1").unwrap();
        assert_eq!(result.task_id, "task-1");
        assert!(result.success);
        assert_eq!(result.action, "peer_chat");
    }

    #[test]
    fn test_store_and_get_failure() {
        let store = TaskResultStore::new(100);
        store.store_failure("task-2", "forge_share", "connection refused");

        let result = store.get("task-2").unwrap();
        assert!(!result.success);
        assert_eq!(
            result.result.get("error").unwrap().as_str().unwrap(),
            "connection refused"
        );
    }

    #[test]
    fn test_max_size_eviction() {
        let store = TaskResultStore::new(2);
        store.store_success("task-1", "a", serde_json::json!(1));
        store.store_success("task-2", "b", serde_json::json!(2));
        store.store_success("task-3", "c", serde_json::json!(3));

        // One should have been evicted
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn test_remove_and_clear() {
        let store = TaskResultStore::new(100);
        store.store_success("task-x", "action", serde_json::json!(null));
        assert!(store.remove("task-x"));
        assert!(store.get("task-x").is_none());

        store.store_success("task-y", "action", serde_json::json!(null));
        store.clear();
        assert!(store.is_empty());
    }

    // ---- Disk persistence tests ----

    #[test]
    fn test_disk_persistence_write() {
        let tmp = tempfile::tempdir().unwrap();
        let store = TaskResultStore::with_disk_persistence(100, tmp.path());

        store.store_success("task-disk-1", "peer_chat", serde_json::json!("hello"));

        // Verify file exists on disk
        let file_path = tmp.path().join("task-disk-1.json");
        assert!(file_path.exists(), "result file should exist on disk");

        // Verify file content is valid JSON
        let data = std_fs::read_to_string(&file_path).unwrap();
        let parsed: TaskResult = serde_json::from_str(&data).unwrap();
        assert_eq!(parsed.task_id, "task-disk-1");
        assert!(parsed.success);
        assert_eq!(parsed.action, "peer_chat");
    }

    #[test]
    fn test_disk_persistence_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let store = TaskResultStore::with_disk_persistence(100, tmp.path());

        store.store_failure("task-disk-2", "forge_share", "timeout");

        let file_path = tmp.path().join("task-disk-2.json");
        assert!(file_path.exists());

        let data = std_fs::read_to_string(&file_path).unwrap();
        let parsed: TaskResult = serde_json::from_str(&data).unwrap();
        assert!(!parsed.success);
        assert_eq!(
            parsed.result.get("error").unwrap().as_str().unwrap(),
            "timeout"
        );
    }

    #[test]
    fn test_disk_persistence_atomic_write() {
        let tmp = tempfile::tempdir().unwrap();
        let store = TaskResultStore::with_disk_persistence(100, tmp.path());

        store.store_success("task-atomic", "action", serde_json::json!(42));

        // Main file should exist
        let file_path = tmp.path().join("task-atomic.json");
        assert!(file_path.exists());

        // Temp file should NOT remain
        let tmp_path = tmp.path().join("task-atomic.json.tmp");
        assert!(!tmp_path.exists(), "temp file should be cleaned up");
    }

    #[test]
    fn test_load_from_disk() {
        let tmp = tempfile::tempdir().unwrap();

        // Store 1: write results to disk
        {
            let store = TaskResultStore::with_disk_persistence(100, tmp.path());
            store.store_success("task-load-1", "peer_chat", serde_json::json!("response-1"));
            store.store_failure("task-load-2", "forge_share", "some error");
        }

        // Store 2: load from disk and verify
        let store = TaskResultStore::with_disk_persistence(100, tmp.path());
        let loaded = store.load_from_disk();
        assert_eq!(loaded, 2);

        let r1 = store.get("task-load-1").unwrap();
        assert_eq!(r1.task_id, "task-load-1");
        assert!(r1.success);

        let r2 = store.get("task-load-2").unwrap();
        assert_eq!(r2.task_id, "task-load-2");
        assert!(!r2.success);
        assert_eq!(
            r2.result.get("error").unwrap().as_str().unwrap(),
            "some error"
        );
    }

    #[test]
    fn test_load_from_disk_respects_max_size() {
        let tmp = tempfile::tempdir().unwrap();

        // Write 5 result files
        {
            let store = TaskResultStore::with_disk_persistence(100, tmp.path());
            for i in 0..5 {
                store.store_success(
                    &format!("task-max-{i}"),
                    "action",
                    serde_json::json!(i),
                );
            }
        }

        // Load with max_size = 2, should only load 2
        let store = TaskResultStore::with_disk_persistence(2, tmp.path());
        let loaded = store.load_from_disk();
        assert_eq!(loaded, 2);
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn test_load_from_disk_no_persistence() {
        let store = TaskResultStore::new(100);
        let loaded = store.load_from_disk();
        assert_eq!(loaded, 0);
    }

    #[test]
    fn test_load_from_disk_corrupt_file() {
        let tmp = tempfile::tempdir().unwrap();

        // Write a valid file and a corrupt file
        {
            let store = TaskResultStore::with_disk_persistence(100, tmp.path());
            store.store_success("task-good", "action", serde_json::json!("ok"));
        }
        // Write corrupt JSON
        std_fs::write(tmp.path().join("task-bad.json"), "not valid json{{{").unwrap();

        let store = TaskResultStore::with_disk_persistence(100, tmp.path());
        let loaded = store.load_from_disk();
        // Only the good file should be loaded
        assert_eq!(loaded, 1);
        assert!(store.get("task-good").is_some());
        assert!(store.get("task-bad").is_none());
    }

    #[test]
    fn test_cleanup_delivered() {
        let tmp = tempfile::tempdir().unwrap();
        let store = TaskResultStore::with_disk_persistence(100, tmp.path());

        store.store_success("task-cleanup", "action", serde_json::json!("data"));

        // File should exist
        let file_path = tmp.path().join("task-cleanup.json");
        assert!(file_path.exists());

        // Cleanup
        assert!(store.cleanup_delivered("task-cleanup"));
        assert!(store.get("task-cleanup").is_none());

        // File should be removed from disk
        assert!(!file_path.exists(), "file should be removed after cleanup");
    }

    #[test]
    fn test_cleanup_delivered_nonexistent() {
        let tmp = tempfile::tempdir().unwrap();
        let store = TaskResultStore::with_disk_persistence(100, tmp.path());

        // Should return false for non-existent task
        assert!(!store.cleanup_delivered("no-such-task"));
    }

    #[test]
    fn test_remove_deletes_disk_file() {
        let tmp = tempfile::tempdir().unwrap();
        let store = TaskResultStore::with_disk_persistence(100, tmp.path());

        store.store_success("task-remove", "action", serde_json::json!(1));
        let file_path = tmp.path().join("task-remove.json");
        assert!(file_path.exists());

        assert!(store.remove("task-remove"));
        assert!(!file_path.exists(), "file should be removed from disk");
    }

    #[test]
    fn test_eviction_deletes_disk_file() {
        let tmp = tempfile::tempdir().unwrap();
        let store = TaskResultStore::with_disk_persistence(2, tmp.path());

        store.store_success("evict-1", "a", serde_json::json!(1));
        store.store_success("evict-2", "b", serde_json::json!(2));
        // This should evict one of the previous entries
        store.store_success("evict-3", "c", serde_json::json!(3));

        assert_eq!(store.len(), 2);

        // At least one of the first two should be gone from disk
        let f1 = tmp.path().join("evict-1.json");
        let f2 = tmp.path().join("evict-2.json");
        let f3 = tmp.path().join("evict-3.json");
        // evict-3 must always exist
        assert!(f3.exists());
        // exactly one of evict-1/evict-2 should have been evicted
        assert_eq!(
            f1.exists() as u8 + f2.exists() as u8,
            1,
            "exactly one of the first two should remain on disk"
        );
    }

    #[test]
    fn test_no_persistence_no_files() {
        let tmp = tempfile::tempdir().unwrap();

        // Use plain new() — no disk persistence
        let store = TaskResultStore::new(100);
        store.store_success("no-disk", "action", serde_json::json!("data"));

        // No files should be written to tmp
        let count = std_fs::read_dir(tmp.path())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .map(|e| e.path().extension().and_then(|e| e.to_str()) == Some("json"))
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_restore_after_restart() {
        let tmp = tempfile::tempdir().unwrap();

        // Phase 1: write results
        {
            let store = TaskResultStore::with_disk_persistence(100, tmp.path());
            store.store_success("restart-1", "peer_chat", serde_json::json!("response-1"));
            store.store_failure("restart-2", "forge_share", "error-2");
        }

        // Phase 2: simulate restart — new store from same directory
        let store = TaskResultStore::with_disk_persistence(100, tmp.path());
        let loaded = store.load_from_disk();
        assert_eq!(loaded, 2);

        // Verify both results
        let r1 = store.get("restart-1").unwrap();
        assert!(r1.success);
        assert_eq!(r1.action, "peer_chat");

        let r2 = store.get("restart-2").unwrap();
        assert!(!r2.success);
        assert_eq!(r2.action, "forge_share");

        // Cleanup one, verify the other remains
        assert!(store.cleanup_delivered("restart-1"));
        assert!(store.get("restart-1").is_none());
        assert!(store.get("restart-2").is_some());

        // Phase 3: another restart — only restart-2 should load
        let store = TaskResultStore::with_disk_persistence(100, tmp.path());
        let loaded = store.load_from_disk();
        assert_eq!(loaded, 1);
        assert!(store.get("restart-1").is_none());
        assert!(store.get("restart-2").is_some());
    }

    // ---- Async tests ----

    #[tokio::test]
    async fn test_async_store_and_get() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AsyncTaskResultStore::with_disk_persistence(100, tmp.path());

        store
            .store_success_async("async-1", "peer_chat", serde_json::json!("hello"))
            .await;
        store
            .store_failure_async("async-2", "forge_share", "boom")
            .await;

        let r1 = store.get_async("async-1").unwrap();
        assert!(r1.success);
        assert_eq!(r1.action, "peer_chat");

        let r2 = store.get_async("async-2").unwrap();
        assert!(!r2.success);
        assert_eq!(
            r2.result.get("error").unwrap().as_str().unwrap(),
            "boom"
        );
    }

    #[tokio::test]
    async fn test_async_disk_write() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AsyncTaskResultStore::with_disk_persistence(100, tmp.path());

        store
            .store_success_async("async-disk", "action", serde_json::json!(42))
            .await;

        let file_path = tmp.path().join("async-disk.json");
        assert!(file_path.exists());

        let data = std_fs::read_to_string(&file_path).unwrap();
        let parsed: TaskResult = serde_json::from_str(&data).unwrap();
        assert_eq!(parsed.task_id, "async-disk");
    }

    #[tokio::test]
    async fn test_async_load_from_disk() {
        let tmp = tempfile::tempdir().unwrap();

        // Write with one store
        {
            let store = AsyncTaskResultStore::with_disk_persistence(100, tmp.path());
            store
                .store_success_async("async-load-1", "a", serde_json::json!(1))
                .await;
            store
                .store_failure_async("async-load-2", "b", "err")
                .await;
        }

        // Load with another
        let store = AsyncTaskResultStore::with_disk_persistence(100, tmp.path());
        let loaded = store.load_from_disk_async().await;
        assert_eq!(loaded, 2);

        assert!(store.get_async("async-load-1").unwrap().success);
        assert!(!store.get_async("async-load-2").unwrap().success);
    }

    #[tokio::test]
    async fn test_async_cleanup_delivered() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AsyncTaskResultStore::with_disk_persistence(100, tmp.path());

        store
            .store_success_async("async-clean", "action", serde_json::json!("data"))
            .await;

        let file_path = tmp.path().join("async-clean.json");
        assert!(file_path.exists());

        let existed = store.cleanup_delivered_async("async-clean").await;
        assert!(existed);
        assert!(store.get_async("async-clean").is_none());
        assert!(!file_path.exists(), "file should be deleted after cleanup");
    }

    #[tokio::test]
    async fn test_async_cleanup_nonexistent() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AsyncTaskResultStore::with_disk_persistence(100, tmp.path());

        let existed = store.cleanup_delivered_async("nope").await;
        assert!(!existed);
    }

    #[tokio::test]
    async fn test_async_eviction_deletes_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AsyncTaskResultStore::with_disk_persistence(2, tmp.path());

        store
            .store_success_async("ae-1", "a", serde_json::json!(1))
            .await;
        store
            .store_success_async("ae-2", "b", serde_json::json!(2))
            .await;
        store
            .store_success_async("ae-3", "c", serde_json::json!(3))
            .await;

        assert_eq!(store.get_async("ae-3").unwrap().task_id, "ae-3");

        let f3 = tmp.path().join("ae-3.json");
        assert!(f3.exists());
    }

    #[tokio::test]
    async fn test_async_full_lifecycle() {
        let tmp = tempfile::tempdir().unwrap();

        // Phase 1: write
        {
            let store = AsyncTaskResultStore::with_disk_persistence(100, tmp.path());
            store
                .store_success_async("life-1", "peer_chat", serde_json::json!("response"))
                .await;
            store
                .store_failure_async("life-2", "action", "failed")
                .await;

            // Cleanup life-1
            assert!(store.cleanup_delivered_async("life-1").await);
        }

        // Phase 2: restart — only life-2 should be on disk
        let store = AsyncTaskResultStore::with_disk_persistence(100, tmp.path());
        let loaded = store.load_from_disk_async().await;
        assert_eq!(loaded, 1);
        assert!(store.get_async("life-1").is_none());
        assert!(store.get_async("life-2").is_some());

        // Cleanup life-2
        assert!(store.cleanup_delivered_async("life-2").await);

        // Phase 3: another restart — nothing left
        let store = AsyncTaskResultStore::with_disk_persistence(100, tmp.path());
        let loaded = store.load_from_disk_async().await;
        assert_eq!(loaded, 0);
    }

    // ---- Go-compatible store tests ----

    #[test]
    fn test_go_store_new_creates_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("ws");
        let store = GoTaskResultStore::new(&workspace).unwrap();
        assert!(workspace.join("cluster").join("task_results").exists());
        let _ = store;
    }

    #[test]
    fn test_go_store_set_running_and_get() {
        let tmp = tempfile::tempdir().unwrap();
        let store = GoTaskResultStore::new(tmp.path()).unwrap();

        store.set_running("task-1", "node-A");

        let entry = store.get("task-1").unwrap();
        assert_eq!(entry.task_id, "task-1");
        assert_eq!(entry.status, "running");
        assert!(store.is_running("task-1"));
    }

    #[test]
    fn test_go_store_set_result_success() {
        let tmp = tempfile::tempdir().unwrap();
        let store = GoTaskResultStore::new(tmp.path()).unwrap();

        store.set_running("task-1", "node-A");
        store
            .set_result("task-1", "success", "hello world", "", "node-A")
            .unwrap();

        // Running should be cleared
        assert!(!store.is_running("task-1"));

        let entry = store.get("task-1").unwrap();
        assert_eq!(entry.status, "done");
        assert_eq!(entry.result_status.as_deref(), Some("success"));
        assert_eq!(entry.response.as_deref(), Some("hello world"));
        assert!(entry.error.is_none());
        assert_eq!(entry.source_node, "node-A");

        // Data file should exist
        let file_path = tmp.path().join("cluster/task_results/task-1.json");
        assert!(file_path.exists());
    }

    #[test]
    fn test_go_store_set_result_error() {
        let tmp = tempfile::tempdir().unwrap();
        let store = GoTaskResultStore::new(tmp.path()).unwrap();

        store
            .set_result("task-2", "error", "", "connection refused", "node-B")
            .unwrap();

        let entry = store.get("task-2").unwrap();
        assert_eq!(entry.status, "done");
        assert_eq!(entry.result_status.as_deref(), Some("error"));
        assert!(entry.response.is_none());
        assert_eq!(entry.error.as_deref(), Some("connection refused"));
    }

    #[test]
    fn test_go_store_get_unknown_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let store = GoTaskResultStore::new(tmp.path()).unwrap();
        assert!(store.get("nonexistent").is_none());
    }

    #[test]
    fn test_go_store_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let store = GoTaskResultStore::new(tmp.path()).unwrap();

        store
            .set_result("task-del", "success", "ok", "", "node-A")
            .unwrap();
        assert!(store.get("task-del").is_some());

        store.delete("task-del").unwrap();
        assert!(store.get("task-del").is_none());

        // Data file should be removed
        let file_path = tmp.path().join("cluster/task_results/task-del.json");
        assert!(!file_path.exists());
    }

    #[test]
    fn test_go_store_index_persistence() {
        let tmp = tempfile::tempdir().unwrap();

        // Write with one store
        {
            let store = GoTaskResultStore::new(tmp.path()).unwrap();
            store
                .set_result("persist-1", "success", "result-1", "", "node-A")
                .unwrap();
            store
                .set_result("persist-2", "error", "", "timeout", "node-B")
                .unwrap();
        }

        // Load with new store
        let store = GoTaskResultStore::new(tmp.path()).unwrap();
        assert_eq!(store.done_count(), 2);

        let r1 = store.get("persist-1").unwrap();
        assert_eq!(r1.status, "done");
        assert_eq!(r1.result_status.as_deref(), Some("success"));

        let r2 = store.get("persist-2").unwrap();
        assert_eq!(r2.status, "done");
        assert_eq!(r2.result_status.as_deref(), Some("error"));
    }

    #[test]
    fn test_go_store_restart_loses_running() {
        let tmp = tempfile::tempdir().unwrap();

        // Create store, set running + done
        {
            let store = GoTaskResultStore::new(tmp.path()).unwrap();
            store.set_running("running-task", "node-A");
            store
                .set_result("done-task", "success", "ok", "", "node-A")
                .unwrap();
        }

        // New store -- running state should be lost
        let store = GoTaskResultStore::new(tmp.path()).unwrap();
        assert!(!store.is_running("running-task"));
        assert!(store.get("running-task").is_none()); // Not in index either
        assert!(store.get("done-task").is_some());
    }

    // ============================================================
    // Additional TaskResultStore tests for missing coverage
    // ============================================================

    #[test]
    fn test_task_result_store_new() {
        let store = TaskResultStore::new(10);
        assert_eq!(store.len(), 0);
        assert!(store.is_empty());
    }

    #[test]
    fn test_task_result_store_store_and_get() {
        let store = TaskResultStore::new(10);
        store.store_success("task-1", "peer_chat", serde_json::json!({"response": "hello"}));

        let result = store.get("task-1").unwrap();
        assert_eq!(result.task_id, "task-1");
        assert_eq!(result.action, "peer_chat");
        assert!(result.success);
        assert!(!result.stored_at.is_empty());
    }

    #[test]
    fn test_task_result_store_failure() {
        let store = TaskResultStore::new(10);
        store.store_failure("task-2", "peer_chat", "connection refused");

        let result = store.get("task-2").unwrap();
        assert!(!result.success);
        assert_eq!(result.result["error"], "connection refused");
    }

    #[test]
    fn test_task_result_store_get_nonexistent() {
        let store = TaskResultStore::new(10);
        assert!(store.get("nonexistent").is_none());
    }

    #[test]
    fn test_task_result_store_remove() {
        let store = TaskResultStore::new(10);
        store.store_success("task-1", "action", serde_json::json!({}));
        assert!(store.remove("task-1"));
        assert!(store.get("task-1").is_none());
    }

    #[test]
    fn test_task_result_store_remove_nonexistent() {
        let store = TaskResultStore::new(10);
        assert!(!store.remove("nonexistent"));
    }

    #[test]
    fn test_task_result_store_cleanup_delivered() {
        let store = TaskResultStore::new(10);
        store.store_success("task-1", "action", serde_json::json!({}));
        assert!(store.cleanup_delivered("task-1"));
        assert!(store.get("task-1").is_none());
    }

    #[test]
    fn test_task_result_store_clear() {
        let store = TaskResultStore::new(10);
        store.store_success("task-1", "action", serde_json::json!({}));
        store.store_success("task-2", "action", serde_json::json!({}));
        store.clear();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn test_task_result_store_eviction() {
        let store = TaskResultStore::new(2);
        store.store_success("task-1", "a", serde_json::json!({}));
        store.store_success("task-2", "b", serde_json::json!({}));
        store.store_success("task-3", "c", serde_json::json!({}));

        // Should have at most 2 entries after eviction
        assert!(store.len() <= 2);
    }

    #[test]
    fn test_task_result_serialization() {
        let result = TaskResult {
            task_id: "test-123".to_string(),
            action: "peer_chat".to_string(),
            result: serde_json::json!({"message": "hello"}),
            success: true,
            stored_at: "2026-05-11T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: TaskResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.task_id, "test-123");
        assert!(parsed.success);
        assert_eq!(parsed.action, "peer_chat");
    }

    #[test]
    fn test_task_result_store_with_disk_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let store = TaskResultStore::with_disk_persistence(10, dir.path());
        store.store_success("disk-1", "action", serde_json::json!({"ok": true}));

        // Should be on disk
        let file_path = dir.path().join("disk-1.json");
        assert!(file_path.exists());
    }

    #[test]
    fn test_task_result_store_load_from_disk() {
        let dir = tempfile::tempdir().unwrap();

        // Write a result file
        let result = TaskResult {
            task_id: "loaded-1".to_string(),
            action: "test".to_string(),
            result: serde_json::json!({}),
            success: true,
            stored_at: "2026-05-11T00:00:00Z".to_string(),
        };
        let file_path = dir.path().join("loaded-1.json");
        std::fs::write(&file_path, serde_json::to_string(&result).unwrap()).unwrap();

        // Load from disk
        let store = TaskResultStore::with_disk_persistence(10, dir.path());
        let count = store.load_from_disk();
        assert_eq!(count, 1);
        assert!(store.get("loaded-1").is_some());
    }

    #[test]
    fn test_task_result_store_load_from_disk_no_dir() {
        let store = TaskResultStore::new(10); // No disk persistence
        let count = store.load_from_disk();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_task_result_store_load_from_disk_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        // Write an invalid JSON file
        std::fs::write(dir.path().join("bad.json"), "not valid json").unwrap();

        let store = TaskResultStore::with_disk_persistence(10, dir.path());
        let count = store.load_from_disk();
        assert_eq!(count, 0); // Should skip invalid files
    }

    #[test]
    fn test_task_result_store_load_from_disk_non_json() {
        let dir = tempfile::tempdir().unwrap();
        // Write a non-JSON file (should be ignored)
        std::fs::write(dir.path().join("readme.txt"), "hello").unwrap();

        let store = TaskResultStore::with_disk_persistence(10, dir.path());
        let count = store.load_from_disk();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_task_result_store_remove_deletes_disk() {
        let dir = tempfile::tempdir().unwrap();
        let store = TaskResultStore::with_disk_persistence(10, dir.path());
        store.store_success("disk-del", "action", serde_json::json!({}));

        let file_path = dir.path().join("disk-del.json");
        assert!(file_path.exists());

        store.remove("disk-del");
        assert!(!file_path.exists());
    }

    // ============================================================
    // Coverage improvement: more store edge cases
    // ============================================================

    #[test]
    fn test_task_result_store_default() {
        let store = TaskResultStore::default();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn test_task_result_store_clear_v2() {
        let store = TaskResultStore::new(10);
        store.store_success("task-1", "action", serde_json::json!({}));
        store.store_success("task-2", "action", serde_json::json!({}));
        assert_eq!(store.len(), 2);

        store.clear();
        assert!(store.is_empty());
    }

    #[test]
    fn test_task_result_store_store_failure() {
        let store = TaskResultStore::new(10);
        assert!(store.get("nonexistent").is_none());
    }

    #[test]
    fn test_task_result_store_remove_nonexistent_v2() {
        let store = TaskResultStore::new(10);
        assert!(!store.remove("nonexistent"));
    }

    #[test]
    fn test_task_result_store_cleanup_delivered_v2() {
        let store = TaskResultStore::new(10);
        store.store_success("delivered-task", "action", serde_json::json!({}));
        assert!(store.cleanup_delivered("delivered-task"));
        assert!(store.get("delivered-task").is_none());
    }

    #[test]
    fn test_task_result_store_cleanup_delivered_nonexistent() {
        let store = TaskResultStore::new(10);
        assert!(!store.cleanup_delivered("nonexistent"));
    }

    #[test]
    fn test_task_result_store_no_disk_persistence() {
        let store = TaskResultStore::new(10);
        // No cache_dir set, write_to_disk and delete_from_disk are no-ops
        store.store_success("no-disk", "action", serde_json::json!({}));
        assert!(store.get("no-disk").is_some());

        store.remove("no-disk");
        assert!(store.get("no-disk").is_none());
    }

    #[test]
    fn test_task_result_store_load_from_disk_no_cache() {
        let store = TaskResultStore::new(10);
        let count = store.load_from_disk();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_task_result_store_eviction_with_disk() {
        let dir = tempfile::tempdir().unwrap();
        let store = TaskResultStore::with_disk_persistence(2, dir.path());

        // Store 3 results, max_size is 2
        store.store_success("evict-1", "action", serde_json::json!({"n": 1}));
        store.store_success("evict-2", "action", serde_json::json!({"n": 2}));
        store.store_success("evict-3", "action", serde_json::json!({"n": 3}));

        // Should have at most 2
        assert!(store.len() <= 2);
        // The newest should still be there
        assert!(store.get("evict-3").is_some());
    }

    #[test]
    fn test_task_result_debug() {
        let result = TaskResult {
            task_id: "debug-test".into(),
            action: "test".into(),
            result: serde_json::json!({"key": "value"}),
            success: true,
            stored_at: "2026-01-01T00:00:00Z".into(),
        };
        let debug = format!("{:?}", result);
        assert!(debug.contains("debug-test"));
        assert!(debug.contains("test"));
    }

    #[test]
    fn test_task_result_serialization_roundtrip() {
        let result = TaskResult {
            task_id: "ser-test".into(),
            action: "act".into(),
            result: serde_json::json!({"x": 42}),
            success: true,
            stored_at: "2026-01-01T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: TaskResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.task_id, "ser-test");
        assert_eq!(parsed.action, "act");
        assert_eq!(parsed.result["x"], 42);
        assert!(parsed.success);
    }

    // -- Go-compatible store tests --

    #[test]
    fn test_go_task_result_entry_serialization() {
        let entry = GoTaskResultEntry {
            task_id: "go-task".into(),
            status: "running".into(),
            result_status: None,
            response: None,
            error: None,
            source_node: "node-a".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: GoTaskResultEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.task_id, "go-task");
        assert_eq!(parsed.status, "running");
        assert!(parsed.response.is_none());
    }

    #[test]
    fn test_go_task_result_entry_with_result() {
        let entry = GoTaskResultEntry {
            task_id: "go-done".into(),
            status: "done".into(),
            result_status: Some("success".into()),
            response: Some("task completed".into()),
            error: None,
            source_node: "node-b".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:01Z".into(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: GoTaskResultEntry = serde_json::from_str(&json).unwrap();
        assert!(parsed.response.is_some());
        assert_eq!(parsed.result_status.unwrap(), "success");
    }

    #[test]
    fn test_go_task_result_index_default() {
        let index = GoTaskResultIndex::default();
        assert!(index.tasks.is_empty());
    }

    #[test]
    fn test_go_task_result_index_serialization() {
        let mut index = GoTaskResultIndex::default();
        index.tasks.insert("t1".into(), GoTaskResultEntry {
            task_id: "t1".into(),
            status: "running".into(),
            result_status: None,
            response: None,
            error: None,
            source_node: "n1".into(),
            created_at: "2026-01-01".into(),
            updated_at: "2026-01-01".into(),
        });
        let json = serde_json::to_string(&index).unwrap();
        let parsed: GoTaskResultIndex = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.tasks.len(), 1);
    }

    // -- Async store tests --

    #[tokio::test]
    async fn test_async_store_success_and_get() {
        let dir = tempfile::tempdir().unwrap();
        let store = AsyncTaskResultStore::with_disk_persistence(10, dir.path());

        store.store_success_async("async-1", "action", serde_json::json!({"ok": true})).await;
        let result = store.get_async("async-1").unwrap();
        assert!(result.success);
        assert_eq!(result.result["ok"], true);
    }

    #[tokio::test]
    async fn test_async_store_failure_and_get() {
        let dir = tempfile::tempdir().unwrap();
        let store = AsyncTaskResultStore::with_disk_persistence(10, dir.path());

        store.store_failure_async("async-fail", "action", "error msg").await;
        let result = store.get_async("async-fail").unwrap();
        assert!(!result.success);
    }

    #[tokio::test]
    async fn test_async_store_cleanup_delivered() {
        let dir = tempfile::tempdir().unwrap();
        let store = AsyncTaskResultStore::with_disk_persistence(10, dir.path());

        store.store_success_async("async-del", "action", serde_json::json!({})).await;
        let existed = store.cleanup_delivered_async("async-del").await;
        assert!(existed);
        assert!(store.get_async("async-del").is_none());
    }

    #[tokio::test]
    async fn test_async_store_load_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let store = AsyncTaskResultStore::with_disk_persistence(10, dir.path());

        store.store_success_async("async-load", "action", serde_json::json!({})).await;
        // Give disk write time to complete
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Create new store and load from disk
        let store2 = AsyncTaskResultStore::with_disk_persistence(10, dir.path());
        let count = store2.load_from_disk_async().await;
        assert!(count >= 1);
        assert!(store2.get_async("async-load").is_some());
    }

    #[tokio::test]
    async fn test_async_store_load_no_cache_dir() {
        let inner = TaskResultStore::new(10);
        let store = AsyncTaskResultStore::new(inner);
        let count = store.load_from_disk_async().await;
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_async_store_cleanup_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let store = AsyncTaskResultStore::with_disk_persistence(10, dir.path());
        let existed = store.cleanup_delivered_async("nonexistent").await;
        assert!(!existed);
    }

    #[tokio::test]
    async fn test_async_store_get_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let store = AsyncTaskResultStore::with_disk_persistence(10, dir.path());
        assert!(store.get_async("nonexistent").is_none());
    }
}

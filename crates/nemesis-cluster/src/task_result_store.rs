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
            tracing::warn!("[TaskResultStore] failed to create cache_dir {:?}: {}", dir, e);
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
                tracing::warn!("[TaskResultStore] failed to read cache_dir {:?}: {}", dir, e);
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
                        tracing::warn!("[TaskResultStore] skipping invalid result file {:?}: {}", path, e);
                    }
                },
                Err(e) => {
                    tracing::warn!("[TaskResultStore] failed to read result file {:?}: {}", path, e);
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
                            "[TaskResultStore] failed to write result temp file {:?}: {}",
                            tmp_path,
                            e
                        );
                        return;
                    }
                    if let Err(e) = std::fs::rename(&tmp_path, &path) {
                        tracing::warn!(
                            "[TaskResultStore] failed to rename result file {:?} -> {:?}: {}",
                            tmp_path,
                            path,
                            e
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "[TaskResultStore] failed to serialize result {}: {}",
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
                    tracing::warn!("[TaskResultStore] failed to delete result file {:?}: {}", path, e);
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
                tracing::warn!("[TaskResultStore] failed to read cache_dir {:?}: {}", dir, e);
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
                        tracing::warn!("[TaskResultStore] skipping invalid result file {:?}: {}", path, e);
                    }
                },
                Err(e) => {
                    tracing::warn!("[TaskResultStore] failed to read result file {:?}: {}", path, e);
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
                                    "[TaskResultStore] failed to rename async result file {:?} -> {:?}: {}",
                                    tmp_path,
                                    path,
                                    e
                                );
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                "[TaskResultStore] failed to write async result temp file {:?}: {}",
                                tmp_path,
                                e
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "[TaskResultStore] failed to serialize result {}: {}",
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
                    tracing::warn!("[TaskResultStore] failed to delete async result file {:?}: {}", path, e);
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
            tracing::warn!("[TaskResultStore] failed to load task result index (starting fresh): {e}");
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
mod tests;

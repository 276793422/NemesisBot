//! Task result store - persists task completion results.
//!
//! Stores the results of completed cluster tasks, allowing consumers to
//! retrieve them asynchronously. Supports optional disk persistence so that
//! results survive process restarts (matching the Go implementation).
//!
//! T4（2026-10-17）：Go 兼容镜像类型（GoTaskResultEntry/Index/Store，
//! ~200 行）已整体删除——生产零调用方的死代码，与主 store 并存误导读者；
//! 历史形态见 git。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tracing;

/// SAN-02：task_id 可能来自远端对端（A 端可传任意串）——落盘路径成分前
/// 白名单消毒（`nemesis_utils::sanitize::sanitize_path_segment`，兜 `..` /
/// 路径分隔符 / 超长）；消毒改变原值时 WARN 留痕（内存索引仍用原 id，
/// 真实 `task_*`/`bg_*` id 映射不变）。
fn safe_task_file_name(task_id: &str) -> String {
    let safe = nemesis_utils::sanitize::sanitize_path_segment(task_id);
    if safe != task_id {
        tracing::warn!(
            "[TaskResultStore] task_id 含不安全字符，已消毒落盘: {task_id:?} -> {safe:?}"
        );
    }
    safe
}

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
            tracing::warn!(
                "[TaskResultStore] failed to create cache_dir {:?}: {}",
                dir,
                e
            );
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
            stored_at: chrono::Local::now().to_rfc3339(),
        });
    }

    /// Store a failed task result.
    pub fn store_failure(&self, task_id: &str, action: &str, error: &str) {
        self.store(TaskResult {
            task_id: task_id.into(),
            action: action.into(),
            result: serde_json::json!({ "error": error }),
            success: false,
            stored_at: chrono::Local::now().to_rfc3339(),
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

    /// 该任务是否处于「running 占位」态（worker 侧任务在跑的内存标记）。
    ///
    /// `TaskResultPersister::set_running` 落 `{"status":"running"}` 占位，
    /// `set_result` 换真结果、`delete` 送达后清理——三者之后本查询即 false。
    /// BUG-2026-10-15 D：exec 残留周期清扫以它跳过**进行中**任务的工作副本
    /// （活跃任务的 exec 目录不可触碰；真崩溃残留无占位、照常收拾）。
    pub fn is_running(&self, task_id: &str) -> bool {
        self.results
            .lock()
            .get(task_id)
            .map(|r| r.result.get("status").and_then(|v| v.as_str()) == Some("running"))
            .unwrap_or(false)
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
                tracing::warn!(
                    "[TaskResultStore] failed to read cache_dir {:?}: {}",
                    dir,
                    e
                );
                return 0;
            }
        };

        // T6③（2026-10-17）：显式排序——read_dir 顺序是平台实现细节
        //（NTFS 字典序 vs ext4 哈希序），max_size 限额加载时截哪部分
        // 必须确定性，不能跟着平台目录序漂。
        let mut paths: Vec<std::path::PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
            .collect();
        paths.sort();

        let mut loaded = 0usize;
        for path in paths {
            match std::fs::read_to_string(&path) {
                Ok(data) => match serde_json::from_str::<TaskResult>(&data) {
                    Ok(result) => {
                        // Bug D 修复（2026-09-08 真机 R1c 发现）：set_running
                        // 占位条目是运行时暂态 —— 它代表的在途工作随进程死亡
                        // 而消失（B 端 peer_chat 任务不进 ClusterTaskList，
                        // 重启后无人重跑）。占位若随盘复活，query_task_result
                        // 会永远回答 "running"，A 端恢复轮询被吊到 24h 安全
                        // 网，用户既拿不到结果也拿不到诚实报错。加载时丢弃
                        // 占位并删除残file，重启后的 B 诚实回答 not_found。
                        if result.result.get("status").and_then(|v| v.as_str()) == Some("running") {
                            let _ = std::fs::remove_file(&path);
                            tracing::info!(
                                task_id = %result.task_id,
                                "[TaskResultStore] dropped stale running placeholder from disk"
                            );
                            continue;
                        }
                        let mut results = self.results.lock();
                        // Respect max_size during restore
                        if results.len() >= self.max_size {
                            break;
                        }
                        results.insert(result.task_id.clone(), result);
                        loaded += 1;
                    }
                    Err(e) => {
                        tracing::warn!(
                            "[TaskResultStore] skipping invalid result file {:?}: {}",
                            path,
                            e
                        );
                    }
                },
                Err(e) => {
                    tracing::warn!(
                        "[TaskResultStore] failed to read result file {:?}: {}",
                        path,
                        e
                    );
                }
            }
        }
        loaded
    }

    /// Remove results older than `max_age` from both disk and memory (G1).
    ///
    /// Disk is the sweep authority: every `*.json` file in `cache_dir` whose
    /// `stored_at` parses and is older than the cutoff is deleted, then the
    /// in-memory map is pruned to match. Entries with an unparsable
    /// `stored_at` are kept (fail-open) so a clock skew or format change
    /// cannot wipe fresh data. Returns the number of removed entries.
    ///
    /// Without disk persistence this is a no-op (memory-only entries are not
    /// swept — they are bounded by `max_size` eviction instead).
    pub fn sweep_older_than(&self, max_age: chrono::Duration) -> usize {
        let dir = match &self.cache_dir {
            Some(d) => d,
            None => return 0,
        };
        let cutoff = chrono::Local::now() - max_age;
        let mut removed = 0usize;

        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                let keep = std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|data| serde_json::from_str::<TaskResult>(&data).ok())
                    .and_then(|r| chrono::DateTime::parse_from_rfc3339(&r.stored_at).ok())
                    .map(|stored| stored.with_timezone(&chrono::Local) >= cutoff)
                    .unwrap_or(true); // unparsable -> keep (fail-open)
                if !keep && std::fs::remove_file(&path).is_ok() {
                    removed += 1;
                }
            }
        }

        // Prune memory to match the disk sweep.
        let mut results = self.results.lock();
        let stale: Vec<String> = results
            .iter()
            .filter(|(_, r)| {
                chrono::DateTime::parse_from_rfc3339(&r.stored_at)
                    .map(|stored| stored.with_timezone(&chrono::Local) < cutoff)
                    .unwrap_or(false)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            results.remove(&id);
        }
        removed
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
            let path = dir.join(format!("{}.json", safe_task_file_name(&result.task_id)));
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
            let path = dir.join(format!("{}.json", safe_task_file_name(task_id)));
            if path.exists()
                && let Err(e) = std::fs::remove_file(&path)
            {
                tracing::warn!(
                    "[TaskResultStore] failed to delete result file {:?}: {}",
                    path,
                    e
                );
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
            stored_at: chrono::Local::now().to_rfc3339(),
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
            stored_at: chrono::Local::now().to_rfc3339(),
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
                tracing::warn!(
                    "[TaskResultStore] failed to read cache_dir {:?}: {}",
                    dir,
                    e
                );
                return 0;
            }
        };

        // T6③（2026-10-17）：显式排序——read_dir 顺序是平台实现细节，
        // max_size 限额加载时截哪部分必须确定性（与同步版同源纪律）。
        let mut paths: Vec<PathBuf> = Vec::new();
        while let Ok(Some(entry)) = read_dir.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            paths.push(path);
        }
        paths.sort();

        let mut loaded = 0usize;
        for path in paths {
            match fs::read_to_string(&path).await {
                Ok(data) => match serde_json::from_str::<TaskResult>(&data) {
                    Ok(result) => {
                        // Bug D 同款修复（T4，2026-10-17；同步版 2026-09-08
                        // 真机 R1c 发现）：set_running 占位条目是运行时暂态
                        // ——它代表的在途工作随进程死亡而消失，占位若随盘
                        // 复活，query_task_result 会永远回答 "running"，A 端
                        // 恢复轮询被吊到 24h 安全网。加载时丢弃占位并删除
                        // 残file，重启后的 B 诚实回答 not_found。
                        if result.result.get("status").and_then(|v| v.as_str()) == Some("running") {
                            let _ = fs::remove_file(&path).await;
                            tracing::info!(
                                task_id = %result.task_id,
                                "[TaskResultStore] dropped stale running placeholder from disk"
                            );
                            continue;
                        }
                        let mut results = self.inner.results.lock();
                        if results.len() >= self.inner.max_size {
                            break;
                        }
                        results.insert(result.task_id.clone(), result);
                        loaded += 1;
                    }
                    Err(e) => {
                        tracing::warn!(
                            "[TaskResultStore] skipping invalid result file {:?}: {}",
                            path,
                            e
                        );
                    }
                },
                Err(e) => {
                    tracing::warn!(
                        "[TaskResultStore] failed to read result file {:?}: {}",
                        path,
                        e
                    );
                }
            }
        }
        loaded
    }

    // ---- Internal helpers ----

    async fn store_async(&self, result: TaskResult) {
        // 1. Insert into memory (sync, fast). The evicted entry's file is
        //    deleted AFTER the lock scope — the parking_lot guard must not
        //    cross the async fs call (the old drop-guard/re-lock dance around
        //    the await was both hard to read and a clippy false-positive trap).
        let evicted_path = {
            let mut results = self.inner.results.lock();
            let mut evicted_path = None;
            if results.len() >= self.inner.max_size
                && let Some(key) = results.keys().next().cloned()
                && let Some(evicted) = results.remove(&key)
                && let Some(dir) = self.inner.cache_dir.clone()
            {
                evicted_path =
                    Some(dir.join(format!("{}.json", safe_task_file_name(&evicted.task_id))));
            }
            results.insert(result.task_id.clone(), result.clone());
            evicted_path
        };
        if let Some(path) = evicted_path {
            let _ = fs::remove_file(path).await;
        }

        // 2. Write to disk asynchronously (result was cloned before move)
        self.write_to_disk_async(&result).await;
    }

    async fn write_to_disk_async(&self, result: &TaskResult) {
        if let Some(dir) = &self.inner.cache_dir {
            let path = dir.join(format!("{}.json", safe_task_file_name(&result.task_id)));
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
            let path = dir.join(format!("{}.json", safe_task_file_name(task_id)));
            if Path::new(&path).exists()
                && let Err(e) = fs::remove_file(&path).await
            {
                tracing::warn!(
                    "[TaskResultStore] failed to delete async result file {:?}: {}",
                    path,
                    e
                );
            }
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod cov_tests;
#[cfg(test)]
mod tests;

//! Cluster task management: work queue and task list for the cluster agent.
//!
//! The cluster agent processes peer_chat requests asynchronously via a work queue
//! and task list. Tasks can be new requests or resumed tasks (after async RPC callbacks).

use std::path::{Path, PathBuf};

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Mutex};

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Status of a cluster task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Running,
    WaitingRemote,
    Completed,
    Failed,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Running => write!(f, "running"),
            Self::WaitingRemote => write!(f, "waiting_remote"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

/// Origin of a cluster task (who sent the request).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSource {
    /// Node ID of the request origin.
    pub node_id: String,
    /// RPC address of the origin node (for sending callbacks).
    pub rpc_address: String,
    /// Session key from the original request.
    pub session_key: String,
}

/// A task in the cluster agent's task list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterTask {
    /// Unique task identifier.
    pub task_id: String,
    /// Who sent this request.
    pub source: TaskSource,
    /// Current task status.
    pub status: TaskStatus,
    /// Original request content.
    pub content: String,
    /// Serialized conversation snapshot (saved when __ASYNC__ is detected).
    pub conversation: Option<serde_json::Value>,
    /// Child task ID we are waiting for (used for callback matching).
    pub waiting_for_task_id: Option<String>,
    /// Tool call ID that triggered __ASYNC__ (used for resume injection).
    pub waiting_tool_call_id: Option<String>,
    /// Callback result injected when the child task completes.
    pub callback_result: Option<String>,
}

// ---------------------------------------------------------------------------
// ClusterWorkQueue
// ---------------------------------------------------------------------------

/// FIFO work queue backed by an mpsc channel.
///
/// Producers (PeerChatHandler, callback handler) submit task IDs;
/// the cluster agent loop consumes them one at a time.
pub struct ClusterWorkQueue {
    tx: mpsc::Sender<String>,
    rx: Mutex<mpsc::Receiver<String>>,
}

impl ClusterWorkQueue {
    pub fn new(capacity: usize) -> Self {
        let (tx, rx) = mpsc::channel(capacity);
        Self {
            tx,
            rx: Mutex::new(rx),
        }
    }

    /// Submit a task ID to the queue (non-blocking).
    pub fn submit(&self, task_id: String) -> Result<(), String> {
        self.tx.try_send(task_id).map_err(|e| format!("work queue full: {}", e))
    }

    /// Get the sender handle for cloning.
    pub fn sender(&self) -> mpsc::Sender<String> {
        self.tx.clone()
    }

    /// Wait for the next task ID (async, zero-cost when idle).
    ///
    /// Returns `None` if the sender side is dropped (all producers gone).
    /// The caller should handle `None` appropriately (e.g., break the event loop).
    pub async fn next(&self) -> Option<String> {
        let mut guard = self.rx.lock().await;
        guard.recv().await
    }
}

// ---------------------------------------------------------------------------
// ClusterTaskList
// ---------------------------------------------------------------------------

/// Thread-safe task list backed by DashMap with optional disk persistence.
pub struct ClusterTaskList {
    tasks: DashMap<String, ClusterTask>,
    data_dir: PathBuf,
}

impl ClusterTaskList {
    /// Create a new task list. `data_dir` is the base directory for persistence.
    pub fn new<P: AsRef<Path>>(data_dir: P) -> Self {
        Self {
            tasks: DashMap::new(),
            data_dir: data_dir.as_ref().to_path_buf(),
        }
    }

    /// Create a new task and add it to the list.
    pub fn create_task(&self, task: ClusterTask) {
        tracing::info!(
            task_id = %task.task_id,
            status = %task.status,
            "[ClusterTaskList] Creating task"
        );
        self.tasks.insert(task.task_id.clone(), task);
    }

    /// Get a task by ID.
    pub fn get_task(&self, task_id: &str) -> Option<ClusterTask> {
        self.tasks.get(task_id).map(|r| r.value().clone())
    }

    /// Update the status of a task.
    pub fn update_status(&self, task_id: &str, status: TaskStatus) {
        if let Some(mut task) = self.tasks.get_mut(task_id) {
            task.status = status;
        }
    }

    /// Atomically save the async state when __ASYNC__ is detected.
    ///
    /// Sets conversation snapshot, waiting_for_task_id, waiting_tool_call_id,
    /// and status to WaitingRemote in a single DashMap entry update.
    pub fn save_async_state(
        &self,
        task_id: &str,
        child_task_id: String,
        tool_call_id: String,
        conversation: serde_json::Value,
    ) {
        // Update task state and persist conversation in a single DashMap entry lock.
        // IMPORTANT: the persist_to_disk() call must happen AFTER the get_mut guard
        // is dropped, because persist_to_disk() calls self.tasks.iter() internally.
        // Holding get_mut + calling iter on the same DashMap causes a deadlock.
        {
            if let Some(mut task) = self.tasks.get_mut(task_id) {
                task.conversation = Some(conversation);
                task.waiting_for_task_id = Some(child_task_id);
                task.waiting_tool_call_id = Some(tool_call_id);
                task.callback_result = None;
                task.status = TaskStatus::WaitingRemote;
                tracing::info!(
                    task_id = %task_id,
                    "[ClusterTaskList] Saved async state"
                );
                // Persist conversation snapshot to disk (single file for large payload).
                self.persist_conversation(task_id, task.conversation.as_ref().unwrap());
            }
        }
        // Guard is dropped here — safe to iterate DashMap now.

        // Persist task index to disk so crash recovery can find this task on restart.
        // Without this, a crash during WaitingRemote means the task is lost permanently —
        // the requesting node would never receive a callback.
        if let Err(e) = self.persist_to_disk() {
            tracing::warn!(
                "[ClusterTaskList] Failed to persist task index after save_async_state: {}", e
            );
        }
    }

    /// Find the parent task ID that is waiting for a given child task ID.
    /// Used by the callback handler to route callbacks.
    pub fn find_by_child_task_id(&self, child_task_id: &str) -> Option<String> {
        for entry in self.tasks.iter() {
            if entry.value().waiting_for_task_id.as_deref() == Some(child_task_id) {
                return Some(entry.key().clone());
            }
        }
        None
    }

    /// Atomically inject a callback result into a waiting task.
    ///
    /// Sets callback_result, clears waiting_for_task_id (we've received the callback),
    /// and sets status back to Pending so the work queue can pick it up.
    ///
    /// Note: `waiting_for_task_id` is intentionally cleared because:
    /// 1. The callback for this child task has arrived — we no longer wait for it.
    /// 2. If resume_execution triggers another __ASYNC__, save_async_state will set a
    ///    new waiting_for_task_id for the next hop (e.g., B→C→D chain).
    /// 3. Prevents a stale child_task_id from matching future callbacks with the same ID.
    pub fn inject_callback(&self, task_id: &str, response: &str) {
        if let Some(mut task) = self.tasks.get_mut(task_id) {
            task.callback_result = Some(response.to_string());
            task.waiting_for_task_id = None;
            task.status = TaskStatus::Pending;
            tracing::info!(
                task_id = %task_id,
                "[ClusterTaskList] Injected callback, status → Pending"
            );
        }
    }

    /// Mark a task as completed and clean up resources.
    ///
    /// Removes the conversation snapshot from memory and disk, removes the task entry,
    /// then updates the task index on disk so stale entries don't accumulate across restarts.
    pub fn complete_task(&self, task_id: &str) {
        // Delete conversation file from disk.
        let conv_path = self.conversation_path(task_id);
        if conv_path.exists() {
            if let Err(e) = std::fs::remove_file(&conv_path) {
                tracing::warn!(
                    path = %conv_path.display(),
                    error = %e,
                    "[ClusterTaskList] Failed to delete conversation file"
                );
            }
        }
        self.tasks.remove(task_id);
        tracing::info!(
            task_id = %task_id,
            "[ClusterTaskList] Task completed and removed"
        );

        // Update disk index after removal. If we skip this and the process crashes,
        // the next restart would restore a completed task that no longer has a
        // conversation file — it would fail immediately on resume, which is harmless
        // but noisy. Keeping the index clean avoids this.
        if let Err(e) = self.persist_to_disk() {
            tracing::warn!(
                "[ClusterTaskList] Failed to persist task index after complete_task: {}", e
            );
        }
    }

    // -- Persistence helpers ------------------------------------------------

    fn conversation_path(&self, task_id: &str) -> PathBuf {
        self.data_dir.join("cluster").join(format!("{}.json", task_id))
    }

    fn persist_conversation(&self, task_id: &str, conversation: &serde_json::Value) {
        let path = self.conversation_path(task_id);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(conversation) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "[ClusterTaskList] Failed to persist conversation"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    task_id = %task_id,
                    error = %e,
                    "[ClusterTaskList] Failed to serialize conversation"
                );
            }
        }
    }

    /// Restore tasks and conversations from disk.
    /// Called at startup to recover from crashes.
    pub fn restore_from_disk(&self) -> Result<(), String> {
        let tasks_path = self.data_dir.join("cluster").join("tasks.json");
        if !tasks_path.exists() {
            return Ok(());
        }

        let data = std::fs::read_to_string(&tasks_path)
            .map_err(|e| format!("Failed to read tasks.json: {}", e))?;

        let tasks: Vec<ClusterTask> = serde_json::from_str(&data)
            .map_err(|e| format!("Failed to parse tasks.json: {}", e))?;

        let mut restored = 0;
        for task in tasks {
            let task_id = task.task_id.clone();
            let status = task.status;

            // Try to restore conversation from disk if needed.
            let mut task = task;
            if task.conversation.is_none() && matches!(status, TaskStatus::WaitingRemote) {
                let conv_path = self.conversation_path(&task_id);
                if conv_path.exists() {
                    if let Ok(conv_data) = std::fs::read_to_string(&conv_path) {
                        task.conversation = serde_json::from_str(&conv_data).ok();
                    }
                }
            }

            // Only restore tasks that are still in progress.
            if matches!(status, TaskStatus::Pending | TaskStatus::WaitingRemote) {
                tracing::info!(
                    task_id = %task_id,
                    status = %status,
                    "[ClusterTaskList] Restoring task from disk"
                );
                self.tasks.insert(task_id, task);
                restored += 1;
            }
        }

        tracing::info!("[ClusterTaskList] Restored {} tasks from disk", restored);
        Ok(())
    }

    /// Return all Pending task IDs and reset WaitingRemote tasks to Pending.
    ///
    /// Called at startup after `restore_from_disk()` to re-submit recovered tasks
    /// into the work queue.
    ///
    /// **Why WaitingRemote becomes Pending**: if we crashed while waiting for a
    /// remote callback, that callback is lost — the remote node already sent it
    /// while we were down. By resetting to Pending, the cluster agent will pick
    /// the task back up. If the remote node hasn't sent the callback yet, it will
    /// arrive later via `inject_callback()` and the task will be re-queued then.
    /// In the worst case (callback truly lost), the task will time out on the
    /// requesting node, which is the correct degradation.
    pub fn recover_task_ids(&self) -> Vec<String> {
        let mut task_ids = Vec::new();
        for mut entry in self.tasks.iter_mut() {
            match entry.value().status {
                TaskStatus::Pending => {
                    tracing::info!(
                        task_id = %entry.key(),
                        "[ClusterTaskList] Recovering Pending task"
                    );
                    task_ids.push(entry.key().clone());
                }
                TaskStatus::WaitingRemote => {
                    tracing::info!(
                        task_id = %entry.key(),
                        "[ClusterTaskList] Recovering WaitingRemote task → Pending (callback may be lost)"
                    );
                    entry.value_mut().status = TaskStatus::Pending;
                    task_ids.push(entry.key().clone());
                }
                _ => {}
            }
        }

        // Persist the status changes (WaitingRemote → Pending) to disk.
        if !task_ids.is_empty() {
            if let Err(e) = self.persist_to_disk() {
                tracing::warn!(
                    "[ClusterTaskList] Failed to persist after recovery: {}", e
                );
            }
        }

        tracing::info!(
            count = task_ids.len(),
            "[ClusterTaskList] Recovered {} tasks for re-queuing",
            task_ids.len()
        );
        task_ids
    }

    /// Persist all active tasks to disk.
    pub fn persist_to_disk(&self) -> Result<(), String> {
        let dir = self.data_dir.join("cluster");
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create cluster dir: {}", e))?;

        let active_tasks: Vec<ClusterTask> = self.tasks.iter()
            .filter(|e| !matches!(e.value().status, TaskStatus::Completed | TaskStatus::Failed))
            .map(|e| e.value().clone())
            .collect();

        let json = serde_json::to_string_pretty(&active_tasks)
            .map_err(|e| format!("Failed to serialize tasks: {}", e))?;

        let path = dir.join("tasks.json");
        std::fs::write(&path, json)
            .map_err(|e| format!("Failed to write tasks.json: {}", e))?;

        tracing::info!(
            count = active_tasks.len(),
            "[ClusterTaskList] Persisted {} active tasks to disk",
            active_tasks.len()
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests;

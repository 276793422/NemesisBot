//! TaskManager - manages task lifecycle (create, assign, complete, fail).
//!
//! Callback-driven: an `on_task_complete` callback is invoked when a task
//! transitions to Completed or Failed.
//!
//! Mirrors Go's `TaskManager` with:
//! - `TaskStore` trait for pluggable storage (Phase 1: in-memory)
//! - `Start()`/`Stop()` lifecycle with background cleanup goroutine
//! - `cleanup_loop` that periodically removes completed tasks older than 2 hours
//! - `cleanup_completed` that also times out pending tasks older than 24 hours

use std::sync::Arc;

use dashmap::DashMap;
use parking_lot::Mutex;
use uuid::Uuid;

use nemesis_types::cluster::{Task, TaskStatus};

// ---------------------------------------------------------------------------
// TaskStore trait (Phase 2: can be replaced with persistent implementation)
// ---------------------------------------------------------------------------

/// Interface for task storage (Phase 2 can replace with persistent implementation).
///
/// Mirrors Go's `TaskStore` interface.
pub trait TaskStore: Send + Sync {
    /// Create a new task record. Returns error if the task already exists.
    fn create(&self, task: Task) -> Result<(), String>;

    /// Get a task by ID. Returns error if not found.
    fn get(&self, task_id: &str) -> Result<Task, String>;

    /// Update a task's result and status.
    fn update_result(
        &self,
        task_id: &str,
        status: TaskStatus,
        result: Option<serde_json::Value>,
    ) -> Result<(), String>;

    /// Delete a task by ID.
    fn delete(&self, task_id: &str) -> Result<(), String>;

    /// List all tasks with the given status.
    fn list_by_status(&self, status: TaskStatus) -> Vec<Task>;

    /// List all tasks.
    fn list_all(&self) -> Vec<Task>;
}

// ---------------------------------------------------------------------------
// InMemoryTaskStore (Phase 1 implementation)
// ---------------------------------------------------------------------------

/// In-memory task store backed by a DashMap.
///
/// Mirrors Go's `InMemoryTaskStore`.
pub struct InMemoryTaskStore {
    tasks: DashMap<String, Task>,
}

impl InMemoryTaskStore {
    /// Create a new empty in-memory task store.
    pub fn new() -> Self {
        Self {
            tasks: DashMap::new(),
        }
    }
}

impl Default for InMemoryTaskStore {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskStore for InMemoryTaskStore {
    fn create(&self, task: Task) -> Result<(), String> {
        if self.tasks.contains_key(&task.id) {
            return Err(format!("task already exists: {}", task.id));
        }
        self.tasks.insert(task.id.clone(), task);
        Ok(())
    }

    fn get(&self, task_id: &str) -> Result<Task, String> {
        self.tasks
            .get(task_id)
            .map(|r| r.value().clone())
            .ok_or_else(|| format!("task not found: {}", task_id))
    }

    fn update_result(
        &self,
        task_id: &str,
        status: TaskStatus,
        result: Option<serde_json::Value>,
    ) -> Result<(), String> {
        let mut task = self
            .tasks
            .get_mut(task_id)
            .ok_or_else(|| format!("task not found: {}", task_id))?;
        task.status = status;
        task.result = result;
        task.completed_at = Some(chrono::Local::now().to_rfc3339());
        Ok(())
    }

    fn delete(&self, task_id: &str) -> Result<(), String> {
        self.tasks
            .remove(task_id)
            .map(|_| ())
            .ok_or_else(|| format!("task not found: {}", task_id))
    }

    fn list_by_status(&self, status: TaskStatus) -> Vec<Task> {
        self.tasks
            .iter()
            .filter(|r| r.status == status)
            .map(|r| r.value().clone())
            .collect()
    }

    fn list_all(&self) -> Vec<Task> {
        self.tasks.iter().map(|r| r.value().clone()).collect()
    }
}

// ---------------------------------------------------------------------------
// OnComplete callback type
// ---------------------------------------------------------------------------

type OnCompleteCallback = Box<dyn Fn(&Task) + Send + Sync>;

// ---------------------------------------------------------------------------
// TaskManager
// ---------------------------------------------------------------------------

/// Manages the lifecycle of cluster tasks.
///
/// The TaskManager owns a `TaskStore` and a completion callback. When a task
/// transitions to Completed or Failed, the callback is fired.
///
/// If started with `start()`, a background cleanup loop runs periodically
/// to remove completed/failed tasks older than 2 hours, and to time out
/// pending tasks older than 24 hours.
pub struct TaskManager {
    store: Arc<dyn TaskStore>,
    cleanup_interval: std::time::Duration,
    on_complete: Mutex<Option<Arc<OnCompleteCallback>>>,
    stop_tx: Option<tokio::sync::broadcast::Sender<()>>,
}

impl TaskManager {
    /// Create a new task manager with in-memory storage and default cleanup interval.
    pub fn new() -> Self {
        Self::with_store_and_interval(
            Arc::new(InMemoryTaskStore::new()),
            std::time::Duration::from_secs(30),
        )
    }

    /// Create a task manager with a completion callback.
    pub fn with_callback(callback: Box<dyn Fn(&Task) + Send + Sync>) -> Self {
        let tm = Self::new();
        tm.set_callback(callback);
        tm
    }

    /// Create a task manager with a specific store and cleanup interval.
    ///
    /// Mirrors Go's `NewTaskManager(cleanupInterval time.Duration)`.
    pub fn with_store_and_interval(
        store: Arc<dyn TaskStore>,
        cleanup_interval: std::time::Duration,
    ) -> Self {
        Self {
            store,
            cleanup_interval,
            on_complete: Mutex::new(None),
            stop_tx: None,
        }
    }

    /// Create a task manager with a specific store.
    pub fn with_store(store: Arc<dyn TaskStore>) -> Self {
        Self::with_store_and_interval(store, std::time::Duration::from_secs(30))
    }

    // -- Lifecycle ---------------------------------------------------------------

    /// Start the background cleanup loop.
    ///
    /// Mirrors Go's `TaskManager.Start()`. Spawns a background tokio task
    /// that periodically calls `cleanup_completed()`.
    ///
    /// Only spawns if inside a tokio runtime. Safe to call multiple times
    /// (subsequent calls are no-ops).
    pub fn start(&mut self) {
        if self.stop_tx.is_some() {
            return; // Already started
        }

        let handle = match tokio::runtime::Handle::try_current() {
            Ok(h) => h,
            Err(_) => return, // No runtime available (e.g. in unit tests)
        };

        let (stop_tx, mut stop_rx) = tokio::sync::broadcast::channel(1);
        self.stop_tx = Some(stop_tx);

        let store = self.store.clone();
        let cleanup_interval = self.cleanup_interval;
        let on_complete = self.on_complete.lock().clone();

        handle.spawn(async move {
            let mut interval = tokio::time::interval(cleanup_interval);
            loop {
                tokio::select! {
                    _ = stop_rx.recv() => {
                        return;
                    }
                    _ = interval.tick() => {
                        cleanup_completed(&store, &on_complete);
                    }
                }
            }
        });
    }

    /// Stop the background cleanup loop.
    ///
    /// Mirrors Go's `TaskManager.Stop()`.
    pub fn stop(&mut self) {
        if let Some(stop_tx) = self.stop_tx.take() {
            let _ = stop_tx.send(());
        }
    }

    // -- Callback management ----------------------------------------------------

    /// Set or replace the completion callback.
    pub fn set_callback(&self, callback: Box<dyn Fn(&Task) + Send + Sync>) {
        *self.on_complete.lock() = Some(Arc::from(callback));
    }

    /// Set the completion callback that receives a task ID string.
    ///
    /// Mirrors Go's `SetOnComplete(fn func(taskID string))`.
    /// The wrapper fetches the task and calls the callback with its ID.
    pub fn set_on_complete(&self, on_complete: Box<dyn Fn(&str) + Send + Sync>) {
        let store = self.store.clone();
        let wrapper: Box<dyn Fn(&Task) + Send + Sync> = Box::new(move |task: &Task| {
            on_complete(&task.id);
            let _ = store; // keep store alive in closure
        });
        *self.on_complete.lock() = Some(Arc::from(wrapper));
    }

    // -- Task CRUD --------------------------------------------------------------

    /// Submit a pre-built task to the store.
    ///
    /// Mirrors Go's `TaskManager.Submit(task *Task) error`.
    pub fn submit(&self, task: Task) -> Result<(), String> {
        self.store.create(task)
    }

    /// Create a new task and insert it into the store.
    pub fn create_task(
        &self,
        action: &str,
        payload: serde_json::Value,
        original_channel: &str,
        original_chat_id: &str,
    ) -> Task {
        self.create_task_with_peer(action, payload, original_channel, original_chat_id, "")
    }

    /// Create a new task with a peer ID and insert it into the store.
    pub fn create_task_with_peer(
        &self,
        action: &str,
        payload: serde_json::Value,
        original_channel: &str,
        original_chat_id: &str,
        peer_id: &str,
    ) -> Task {
        let task = Task {
            id: Uuid::new_v4().to_string(),
            status: TaskStatus::Pending,
            action: action.to_string(),
            peer_id: peer_id.to_string(),
            payload,
            result: None,
            original_channel: original_channel.to_string(),
            original_chat_id: original_chat_id.to_string(),
            created_at: chrono::Local::now().to_rfc3339(),
            completed_at: None,
        };
        let _ = self.store.create(task.clone());
        task
    }

    /// Assign a task to a node, transitioning it to Running.
    /// Returns `false` if the task was not found or not in Pending state.
    pub fn assign_task(&self, task_id: &str, node_id: &str) -> bool {
        match self.store.get(task_id) {
            Ok(task) if task.status == TaskStatus::Pending => {
                let _ = self
                    .store
                    .update_result(task_id, TaskStatus::Running, task.result);
                crate::logger::log_task("assigned", task_id, node_id);
                true
            }
            _ => false,
        }
    }

    /// Complete a task with a result.
    /// Returns `false` if the task was not found.
    pub fn complete_task(&self, task_id: &str, result: serde_json::Value) -> bool {
        if self
            .store
            .update_result(task_id, TaskStatus::Completed, Some(result))
            .is_err()
        {
            return false;
        }

        if let Ok(task) = self.store.get(task_id) {
            self.fire_callback(&task);
        }

        true
    }

    /// Fail a task with an error message.
    /// Returns `false` if the task was not found.
    pub fn fail_task(&self, task_id: &str, error: &str) -> bool {
        if self
            .store
            .update_result(
                task_id,
                TaskStatus::Failed,
                Some(serde_json::json!({ "error": error })),
            )
            .is_err()
        {
            return false;
        }

        if let Ok(task) = self.store.get(task_id) {
            self.fire_callback(&task);
        }

        true
    }

    /// Get a task by ID.
    pub fn get_task(&self, task_id: &str) -> Option<Task> {
        self.store.get(task_id).ok()
    }

    /// Delete a task by ID.
    ///
    /// Mirrors Go's `TaskStore.Delete(taskID string) error`.
    pub fn delete_task(&self, task_id: &str) -> bool {
        let ok = self.store.delete(task_id).is_ok();
        if ok {
            crate::logger::log_task("cancelled", task_id, "");
        }
        ok
    }

    /// List all tasks.
    pub fn list_tasks(&self) -> Vec<Task> {
        self.store.list_all()
    }

    /// List all tasks that are still in Pending status (used by recoveryLoop).
    pub fn list_pending_tasks(&self) -> Vec<Task> {
        self.store.list_by_status(TaskStatus::Pending)
    }

    /// List all tasks with Completed status.
    pub fn list_completed_tasks(&self) -> Vec<Task> {
        self.store.list_by_status(TaskStatus::Completed)
    }

    /// List all tasks with Failed status.
    pub fn list_failed_tasks(&self) -> Vec<Task> {
        self.store.list_by_status(TaskStatus::Failed)
    }

    /// High-level callback that completes or fails a task based on status string.
    ///
    /// Mirrors the Go `CompleteCallback(taskID, status, response, errMsg)`:
    /// - If `status` is `"error"`, the task is failed with `errMsg`.
    /// - Otherwise, the task is completed with `response` as the JSON result.
    pub fn complete_callback(
        &self,
        task_id: &str,
        status: &str,
        response: &str,
        err_msg: &str,
    ) -> bool {
        if status == "error" {
            let error = if err_msg.is_empty() {
                "unknown error".to_string()
            } else {
                err_msg.to_string()
            };
            self.fail_task(task_id, &error)
        } else {
            let result = if response.is_empty() {
                serde_json::json!(null)
            } else {
                serde_json::json!(response)
            };
            self.complete_task(task_id, result)
        }
    }

    /// Return the number of tasks.
    pub fn len(&self) -> usize {
        self.list_tasks().len()
    }

    /// Return whether there are no tasks.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn fire_callback(&self, task: &Task) {
        if let Some(cb) = self.on_complete.lock().as_ref() {
            cb(task);
        }
    }
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Cleanup logic (free functions for use in background tasks)
// ---------------------------------------------------------------------------

/// Clean up completed/failed tasks older than 2 hours, and time out pending
/// tasks older than 24 hours.
///
/// Mirrors Go's `TaskManager.cleanupCompleted()`.
fn cleanup_completed(store: &Arc<dyn TaskStore>, on_complete: &Option<Arc<OnCompleteCallback>>) {
    // Clean up completed, failed, and cancelled tasks older than 2 hours
    let finished_statuses = [
        TaskStatus::Completed,
        TaskStatus::Failed,
        TaskStatus::Cancelled,
    ];

    let two_hours = chrono::Duration::hours(2);

    for status in &finished_statuses {
        let tasks = store.list_by_status(*status);
        for task in tasks {
            if let Some(ref completed_at) = task.completed_at {
                if let Ok(completed) = chrono::DateTime::parse_from_rfc3339(completed_at) {
                    let completed_utc = completed.with_timezone(&chrono::Local);
                    if chrono::Local::now() - completed_utc > two_hours {
                        let _ = store.delete(&task.id);
                    }
                }
            }
        }
    }

    // H4: Time out pending tasks older than 24 hours
    let twenty_four_hours = chrono::Duration::hours(24);
    let pending_tasks = store.list_by_status(TaskStatus::Pending);
    for task in pending_tasks {
        if let Ok(created) = chrono::DateTime::parse_from_rfc3339(&task.created_at) {
            let created_utc = created.with_timezone(&chrono::Local);
            if chrono::Local::now() - created_utc > twenty_four_hours {
                // Mark as failed with timeout error
                let _ = store.update_result(
                    &task.id,
                    TaskStatus::Failed,
                    Some(serde_json::json!({
                        "error": "task timed out: no response received within 24 hours"
                    })),
                );

                crate::logger::log_task(
                    "timeout",
                    &task.id,
                    &format!(
                        "age={}s",
                        (chrono::Local::now() - created_utc).num_seconds()
                    ),
                );

                // Fire callback if set
                if let Some(cb) = on_complete {
                    if let Ok(updated_task) = store.get(&task.id) {
                        cb(&updated_task);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

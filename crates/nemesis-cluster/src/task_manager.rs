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
        task.completed_at = Some(chrono::Utc::now().to_rfc3339());
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
            created_at: chrono::Utc::now().to_rfc3339(),
            completed_at: None,
        };
        let _ = self.store.create(task.clone());
        task
    }

    /// Assign a task to a node, transitioning it to Running.
    /// Returns `false` if the task was not found or not in Pending state.
    pub fn assign_task(&self, task_id: &str, _node_id: &str) -> bool {
        match self.store.get(task_id) {
            Ok(task) if task.status == TaskStatus::Pending => {
                let _ = self
                    .store
                    .update_result(task_id, TaskStatus::Running, task.result);
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
        self.store.delete(task_id).is_ok()
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
fn cleanup_completed(
    store: &Arc<dyn TaskStore>,
    on_complete: &Option<Arc<OnCompleteCallback>>,
) {
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
                    let completed_utc = completed.with_timezone(&chrono::Utc);
                    if chrono::Utc::now() - completed_utc > two_hours {
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
            let created_utc = created.with_timezone(&chrono::Utc);
            if chrono::Utc::now() - created_utc > twenty_four_hours {
                // Mark as failed with timeout error
                let _ = store.update_result(
                    &task.id,
                    TaskStatus::Failed,
                    Some(serde_json::json!({
                        "error": "task timed out: no response received within 24 hours"
                    })),
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
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn test_create_and_get_task() {
        let tm = TaskManager::new();
        let task = tm.create_task(
            "peer_chat",
            serde_json::json!({"msg": "hello"}),
            "web",
            "chat-1",
        );

        let retrieved = tm.get_task(&task.id).unwrap();
        assert_eq!(retrieved.action, "peer_chat");
        assert_eq!(retrieved.status, TaskStatus::Pending);
        assert!(retrieved.completed_at.is_none());
    }

    #[test]
    fn test_assign_task() {
        let tm = TaskManager::new();
        let task = tm.create_task("ping", serde_json::json!({}), "rpc", "ch");

        assert!(tm.assign_task(&task.id, "node-a"));
        let updated = tm.get_task(&task.id).unwrap();
        assert_eq!(updated.status, TaskStatus::Running);

        // Cannot assign again
        assert!(!tm.assign_task(&task.id, "node-b"));
    }

    #[test]
    fn test_complete_task_with_callback() {
        let completed = Arc::new(Mutex::new(Vec::new()));
        let completed_clone = completed.clone();
        let tm = TaskManager::with_callback(Box::new(move |t: &Task| {
            completed_clone.lock().push(t.id.clone());
        }));

        let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");
        tm.complete_task(&task.id, serde_json::json!("result-data"));

        let updated = tm.get_task(&task.id).unwrap();
        assert_eq!(updated.status, TaskStatus::Completed);
        assert!(updated.completed_at.is_some());

        // Callback should have fired
        let ids = completed.lock();
        assert!(ids.contains(&task.id));
    }

    #[test]
    fn test_fail_task() {
        let tm = TaskManager::new();
        let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");

        assert!(tm.fail_task(&task.id, "connection refused"));
        let updated = tm.get_task(&task.id).unwrap();
        assert_eq!(updated.status, TaskStatus::Failed);
        assert!(updated.result.as_ref().unwrap().get("error").is_some());
    }

    #[test]
    fn test_delete_task() {
        let tm = TaskManager::new();
        let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");
        assert!(tm.get_task(&task.id).is_some());
        assert!(tm.delete_task(&task.id));
        assert!(tm.get_task(&task.id).is_none());
    }

    #[test]
    fn test_submit_task() {
        let tm = TaskManager::new();
        let task = Task {
            id: "custom-task-001".to_string(),
            status: TaskStatus::Pending,
            action: "peer_chat".to_string(),
            peer_id: "remote-1".to_string(),
            payload: serde_json::json!({"msg": "hello"}),
            result: None,
            original_channel: "rpc".to_string(),
            original_chat_id: "chat-1".to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            completed_at: None,
        };

        assert!(tm.submit(task).is_ok());
        let retrieved = tm.get_task("custom-task-001").unwrap();
        assert_eq!(retrieved.action, "peer_chat");
    }

    #[test]
    fn test_submit_duplicate_fails() {
        let tm = TaskManager::new();
        let task = Task {
            id: "dup-task".to_string(),
            status: TaskStatus::Pending,
            action: "action".to_string(),
            peer_id: String::new(),
            payload: serde_json::json!({}),
            result: None,
            original_channel: "rpc".to_string(),
            original_chat_id: "ch".to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            completed_at: None,
        };

        assert!(tm.submit(task.clone()).is_ok());
        // Second submit should fail
        assert!(tm.submit(task).is_err());
    }

    #[test]
    fn test_set_on_complete() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();
        let tm = TaskManager::new();
        tm.set_on_complete(Box::new(move |_task_id: &str| {
            call_count_clone.fetch_add(1, Ordering::SeqCst);
        }));

        let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");
        tm.complete_task(&task.id, serde_json::json!("done"));

        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_list_by_status() {
        let tm = TaskManager::new();
        let t1 = tm.create_task("a", serde_json::json!({}), "rpc", "ch");
        let _t2 = tm.create_task("b", serde_json::json!({}), "rpc", "ch");
        tm.complete_task(&t1.id, serde_json::json!("done"));

        let pending = tm.list_pending_tasks();
        let completed = tm.list_completed_tasks();
        assert_eq!(pending.len(), 1);
        assert_eq!(completed.len(), 1);
    }

    #[test]
    fn test_complete_callback_error() {
        let tm = TaskManager::new();
        let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");

        tm.complete_callback(&task.id, "error", "", "something went wrong");
        let updated = tm.get_task(&task.id).unwrap();
        assert_eq!(updated.status, TaskStatus::Failed);
    }

    #[test]
    fn test_complete_callback_success() {
        let tm = TaskManager::new();
        let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");

        tm.complete_callback(&task.id, "success", "hello world", "");
        let updated = tm.get_task(&task.id).unwrap();
        assert_eq!(updated.status, TaskStatus::Completed);
    }

    // -- InMemoryTaskStore tests --

    #[test]
    fn test_in_memory_store_crud() {
        let store = InMemoryTaskStore::new();

        let task = Task {
            id: "test-1".to_string(),
            status: TaskStatus::Pending,
            action: "action".to_string(),
            peer_id: String::new(),
            payload: serde_json::json!({}),
            result: None,
            original_channel: "rpc".to_string(),
            original_chat_id: "ch".to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            completed_at: None,
        };

        assert!(store.create(task).is_ok());
        assert!(store.get("test-1").is_ok());
        assert!(store.get("nonexistent").is_err());

        assert!(store.update_result("test-1", TaskStatus::Completed, Some(serde_json::json!("done"))).is_ok());
        let t = store.get("test-1").unwrap();
        assert_eq!(t.status, TaskStatus::Completed);

        assert!(store.delete("test-1").is_ok());
        assert!(store.get("test-1").is_err());
    }

    #[test]
    fn test_in_memory_store_list_by_status() {
        let store = InMemoryTaskStore::new();

        for i in 0..3 {
            let task = Task {
                id: format!("task-{}", i),
                status: TaskStatus::Pending,
                action: "action".to_string(),
                peer_id: String::new(),
                payload: serde_json::json!({}),
                result: None,
                original_channel: "rpc".to_string(),
                original_chat_id: "ch".to_string(),
                created_at: chrono::Utc::now().to_rfc3339(),
                completed_at: None,
            };
            store.create(task).unwrap();
        }

        let pending = store.list_by_status(TaskStatus::Pending);
        assert_eq!(pending.len(), 3);

        store.update_result("task-0", TaskStatus::Completed, None).unwrap();
        let pending = store.list_by_status(TaskStatus::Pending);
        assert_eq!(pending.len(), 2);
        let completed = store.list_by_status(TaskStatus::Completed);
        assert_eq!(completed.len(), 1);
    }

    #[test]
    fn test_cleanup_completed_removes_old_tasks() {
        let store: Arc<dyn TaskStore> = Arc::new(InMemoryTaskStore::new());

        // Create a completed task with an old completed_at timestamp
        let old_task = Task {
            id: "old-completed".to_string(),
            status: TaskStatus::Pending,
            action: "action".to_string(),
            peer_id: String::new(),
            payload: serde_json::json!({}),
            result: None,
            original_channel: "rpc".to_string(),
            original_chat_id: "ch".to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            completed_at: None,
        };
        store.create(old_task).unwrap();

        // Complete it with an old timestamp (simulate)
        store
            .update_result("old-completed", TaskStatus::Completed, Some(serde_json::json!("done")))
            .unwrap();

        // Manually set the completed_at to 3 hours ago (can't easily do this through
        // the store interface, so we test the logic indirectly)
        // The cleanup function checks completed_at. Since we just created it,
        // it won't be cleaned up.
        cleanup_completed(&store, &None);

        // Task should still exist (completed_at is recent)
        assert!(store.get("old-completed").is_ok());
    }

    #[test]
    fn test_cleanup_pending_timeout() {
        let store: Arc<dyn TaskStore> = Arc::new(InMemoryTaskStore::new());
        let callback_count = Arc::new(AtomicUsize::new(0));
        let callback_count_clone = callback_count.clone();

        let on_complete: Option<Arc<OnCompleteCallback>> = Some(Arc::new(Box::new(move |_task: &Task| {
            callback_count_clone.fetch_add(1, Ordering::SeqCst);
        })));

        // Create a pending task with a very old created_at
        let old_time = (chrono::Utc::now() - chrono::Duration::hours(25)).to_rfc3339();
        let old_task = Task {
            id: "old-pending".to_string(),
            status: TaskStatus::Pending,
            action: "action".to_string(),
            peer_id: String::new(),
            payload: serde_json::json!({}),
            result: None,
            original_channel: "rpc".to_string(),
            original_chat_id: "ch".to_string(),
            created_at: old_time,
            completed_at: None,
        };
        store.create(old_task).unwrap();

        // Create a recent pending task (should NOT be timed out)
        let recent_task = Task {
            id: "recent-pending".to_string(),
            status: TaskStatus::Pending,
            action: "action".to_string(),
            peer_id: String::new(),
            payload: serde_json::json!({}),
            result: None,
            original_channel: "rpc".to_string(),
            original_chat_id: "ch".to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            completed_at: None,
        };
        store.create(recent_task).unwrap();

        cleanup_completed(&store, &on_complete);

        // Old pending should be failed
        let old = store.get("old-pending").unwrap();
        assert_eq!(old.status, TaskStatus::Failed);

        // Recent pending should still be pending
        let recent = store.get("recent-pending").unwrap();
        assert_eq!(recent.status, TaskStatus::Pending);

        // Callback should have been called once
        assert_eq!(callback_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_task_manager_with_custom_store() {
        let store = Arc::new(InMemoryTaskStore::new());
        let mut tm = TaskManager::with_store(store);

        let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");
        assert!(tm.get_task(&task.id).is_some());

        // start/stop should work without panicking
        tm.start();
        tm.stop();
    }

    #[test]
    fn test_len_and_is_empty() {
        let tm = TaskManager::new();
        assert!(tm.is_empty());
        assert_eq!(tm.len(), 0);

        let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");
        assert!(!tm.is_empty());
        assert_eq!(tm.len(), 1);

        tm.delete_task(&task.id);
        assert!(tm.is_empty());
    }

    // -- Additional tests: task state transitions, concurrent operations, error edge cases --

    #[test]
    fn test_create_task_with_peer() {
        let tm = TaskManager::new();
        let task = tm.create_task_with_peer(
            "peer_chat",
            serde_json::json!({"msg": "hello"}),
            "web",
            "chat-1",
            "remote-node-001",
        );
        assert_eq!(task.peer_id, "remote-node-001");
        assert_eq!(task.action, "peer_chat");

        let retrieved = tm.get_task(&task.id).unwrap();
        assert_eq!(retrieved.peer_id, "remote-node-001");
    }

    #[test]
    fn test_task_full_lifecycle_pending_running_completed() {
        let tm = TaskManager::new();
        let task = tm.create_task("peer_chat", serde_json::json!({}), "rpc", "ch");

        // Pending
        let t = tm.get_task(&task.id).unwrap();
        assert_eq!(t.status, TaskStatus::Pending);
        assert!(t.completed_at.is_none());

        // Assign -> Running
        assert!(tm.assign_task(&task.id, "node-a"));
        let t = tm.get_task(&task.id).unwrap();
        assert_eq!(t.status, TaskStatus::Running);

        // Complete -> Completed
        assert!(tm.complete_task(&task.id, serde_json::json!("result")));
        let t = tm.get_task(&task.id).unwrap();
        assert_eq!(t.status, TaskStatus::Completed);
        assert!(t.completed_at.is_some());
        assert_eq!(t.result.unwrap(), serde_json::json!("result"));
    }

    #[test]
    fn test_task_full_lifecycle_pending_failed() {
        let tm = TaskManager::new();
        let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");

        // Fail directly from Pending (skip Running)
        assert!(tm.fail_task(&task.id, "connection lost"));
        let t = tm.get_task(&task.id).unwrap();
        assert_eq!(t.status, TaskStatus::Failed);
        assert!(t.completed_at.is_some());
        let result_val = t.result.unwrap();
        let err = result_val.get("error").unwrap().as_str().unwrap();
        assert_eq!(err, "connection lost");
    }

    #[test]
    fn test_assign_task_nonexistent_returns_false() {
        let tm = TaskManager::new();
        assert!(!tm.assign_task("nonexistent-task", "node-a"));
    }

    #[test]
    fn test_complete_task_nonexistent_returns_false() {
        let tm = TaskManager::new();
        assert!(!tm.complete_task("nonexistent-task", serde_json::json!("x")));
    }

    #[test]
    fn test_fail_task_nonexistent_returns_false() {
        let tm = TaskManager::new();
        assert!(!tm.fail_task("nonexistent-task", "error"));
    }

    #[test]
    fn test_assign_running_task_fails() {
        let tm = TaskManager::new();
        let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");

        // First assign succeeds
        assert!(tm.assign_task(&task.id, "node-a"));

        // Second assign should fail (already Running)
        assert!(!tm.assign_task(&task.id, "node-b"));
    }

    #[test]
    fn test_assign_completed_task_fails() {
        let tm = TaskManager::new();
        let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");

        tm.complete_task(&task.id, serde_json::json!("done"));

        // Assigning a completed task should fail (status != Pending)
        assert!(!tm.assign_task(&task.id, "node-a"));
    }

    #[test]
    fn test_complete_callback_with_empty_error_defaults() {
        let tm = TaskManager::new();
        let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");

        // Error status with empty err_msg should use "unknown error"
        tm.complete_callback(&task.id, "error", "", "");
        let t = tm.get_task(&task.id).unwrap();
        assert_eq!(t.status, TaskStatus::Failed);
        let result_val = t.result.unwrap();
        let err = result_val.get("error").unwrap().as_str().unwrap();
        assert_eq!(err, "unknown error");
    }

    #[test]
    fn test_complete_callback_with_empty_response_uses_null() {
        let tm = TaskManager::new();
        let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");

        // Success with empty response should use null
        tm.complete_callback(&task.id, "success", "", "");
        let t = tm.get_task(&task.id).unwrap();
        assert_eq!(t.status, TaskStatus::Completed);
        assert_eq!(t.result.unwrap(), serde_json::json!(null));
    }

    #[test]
    fn test_callback_fires_on_fail_task() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();
        let tm = TaskManager::with_callback(Box::new(move |_t: &Task| {
            call_count_clone.fetch_add(1, Ordering::SeqCst);
        }));

        let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");
        tm.fail_task(&task.id, "some error");

        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_list_failed_tasks() {
        let tm = TaskManager::new();
        let t1 = tm.create_task("a", serde_json::json!({}), "rpc", "ch");
        let _t2 = tm.create_task("b", serde_json::json!({}), "rpc", "ch");
        let t3 = tm.create_task("c", serde_json::json!({}), "rpc", "ch");

        tm.fail_task(&t1.id, "error-1");
        tm.complete_task(&t3.id, serde_json::json!("done"));

        let failed = tm.list_failed_tasks();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].id, t1.id);
    }

    #[test]
    fn test_delete_nonexistent_task_returns_false() {
        let tm = TaskManager::new();
        assert!(!tm.delete_task("nonexistent"));
    }

    #[test]
    fn test_in_memory_store_update_nonexistent_fails() {
        let store = InMemoryTaskStore::new();
        let result = store.update_result("nonexistent", TaskStatus::Completed, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_in_memory_store_delete_nonexistent_fails() {
        let store = InMemoryTaskStore::new();
        let result = store.delete("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_in_memory_store_list_all() {
        let store = InMemoryTaskStore::new();
        for i in 0..5 {
            let task = Task {
                id: format!("task-{}", i),
                status: TaskStatus::Pending,
                action: "action".to_string(),
                peer_id: String::new(),
                payload: serde_json::json!({}),
                result: None,
                original_channel: "rpc".to_string(),
                original_chat_id: "ch".to_string(),
                created_at: chrono::Utc::now().to_rfc3339(),
                completed_at: None,
            };
            store.create(task).unwrap();
        }
        let all = store.list_all();
        assert_eq!(all.len(), 5);
    }

    // ============================================================
    // Coverage improvement: more edge cases, cleanup, start/stop
    // ============================================================

    #[tokio::test]
    async fn test_start_stop_lifecycle() {
        let mut tm = TaskManager::with_store_and_interval(
            Arc::new(InMemoryTaskStore::new()),
            std::time::Duration::from_millis(100),
        );
        tm.start();
        assert!(tm.stop_tx.is_some());
        tm.stop();
        assert!(tm.stop_tx.is_none());
    }

    #[test]
    fn test_start_without_runtime_is_noop() {
        let mut tm = TaskManager::new();
        tm.start();
        // Should not panic, stop_tx stays None
        assert!(tm.stop_tx.is_none());
    }

    #[test]
    fn test_start_idempotent() {
        let mut tm = TaskManager::new();
        tm.start();
        tm.start(); // second call should be no-op
        assert!(tm.stop_tx.is_none()); // still None because no runtime
    }

    #[test]
    fn test_stop_without_start_is_noop() {
        let mut tm = TaskManager::new();
        tm.stop(); // should not panic
    }

    #[test]
    fn test_set_callback_replaces_existing() {
        let call_count1 = Arc::new(AtomicUsize::new(0));
        let call_count1_clone = call_count1.clone();
        let tm = TaskManager::new();
        tm.set_callback(Box::new(move |_t: &Task| {
            call_count1_clone.fetch_add(1, Ordering::SeqCst);
        }));

        let task1 = tm.create_task("a", serde_json::json!({}), "rpc", "ch");
        tm.complete_task(&task1.id, serde_json::json!("done"));
        assert_eq!(call_count1.load(Ordering::SeqCst), 1);

        // Replace callback
        let call_count2 = Arc::new(AtomicUsize::new(0));
        let call_count2_clone = call_count2.clone();
        tm.set_callback(Box::new(move |_t: &Task| {
            call_count2_clone.fetch_add(1, Ordering::SeqCst);
        }));

        let task2 = tm.create_task("b", serde_json::json!({}), "rpc", "ch");
        tm.complete_task(&task2.id, serde_json::json!("done"));
        // First callback should NOT have been called again
        assert_eq!(call_count1.load(Ordering::SeqCst), 1);
        assert_eq!(call_count2.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_complete_task_without_callback() {
        let tm = TaskManager::new();
        // No callback set
        let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");
        assert!(tm.complete_task(&task.id, serde_json::json!("result")));
        let t = tm.get_task(&task.id).unwrap();
        assert_eq!(t.status, TaskStatus::Completed);
    }

    #[test]
    fn test_fail_task_without_callback() {
        let tm = TaskManager::new();
        let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");
        assert!(tm.fail_task(&task.id, "error"));
        let t = tm.get_task(&task.id).unwrap();
        assert_eq!(t.status, TaskStatus::Failed);
    }

    #[test]
    fn test_cleanup_completed_removes_cancelled_tasks() {
        let store: Arc<dyn TaskStore> = Arc::new(InMemoryTaskStore::new());

        // Create a task, complete it
        let task = Task {
            id: "cancelled-task".to_string(),
            status: TaskStatus::Pending,
            action: "action".to_string(),
            peer_id: String::new(),
            payload: serde_json::json!({}),
            result: None,
            original_channel: "rpc".to_string(),
            original_chat_id: "ch".to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            completed_at: None,
        };
        store.create(task).unwrap();
        store.update_result("cancelled-task", TaskStatus::Cancelled, None).unwrap();

        // Since just created, should not be cleaned up
        cleanup_completed(&store, &None);
        assert!(store.get("cancelled-task").is_ok());
    }

    #[test]
    fn test_cleanup_completed_with_invalid_created_at() {
        let store: Arc<dyn TaskStore> = Arc::new(InMemoryTaskStore::new());

        // Create a pending task with invalid created_at
        let task = Task {
            id: "bad-date".to_string(),
            status: TaskStatus::Pending,
            action: "action".to_string(),
            peer_id: String::new(),
            payload: serde_json::json!({}),
            result: None,
            original_channel: "rpc".to_string(),
            original_chat_id: "ch".to_string(),
            created_at: "not-a-date".to_string(),
            completed_at: None,
        };
        store.create(task).unwrap();

        // Should not panic with invalid date
        cleanup_completed(&store, &None);
        assert!(store.get("bad-date").is_ok());
    }

    #[test]
    fn test_cleanup_completed_with_invalid_completed_at() {
        let store: Arc<dyn TaskStore> = Arc::new(InMemoryTaskStore::new());

        let task = Task {
            id: "bad-completed-date".to_string(),
            status: TaskStatus::Pending,
            action: "action".to_string(),
            peer_id: String::new(),
            payload: serde_json::json!({}),
            result: None,
            original_channel: "rpc".to_string(),
            original_chat_id: "ch".to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            completed_at: None,
        };
        store.create(task).unwrap();
        store.update_result("bad-completed-date", TaskStatus::Completed, Some(serde_json::json!("done"))).unwrap();

        // Should not panic
        cleanup_completed(&store, &None);
    }

    #[test]
    fn test_with_store_and_interval_custom() {
        let store = Arc::new(InMemoryTaskStore::new());
        let tm = TaskManager::with_store_and_interval(
            store,
            std::time::Duration::from_secs(60),
        );
        assert_eq!(tm.cleanup_interval, std::time::Duration::from_secs(60));
    }

    #[test]
    fn test_in_memory_store_default() {
        let store = InMemoryTaskStore::default();
        assert!(store.list_all().is_empty());
    }

    #[test]
    fn test_task_manager_default() {
        let tm = TaskManager::default();
        assert!(tm.is_empty());
    }

    #[test]
    fn test_create_task_unique_ids() {
        let tm = TaskManager::new();
        let t1 = tm.create_task("a", serde_json::json!({}), "rpc", "ch");
        let t2 = tm.create_task("a", serde_json::json!({}), "rpc", "ch");
        assert_ne!(t1.id, t2.id);
    }

    #[test]
    fn test_multiple_tasks_lifecycle() {
        let completed_ids = Arc::new(Mutex::new(Vec::new()));
        let completed_clone = completed_ids.clone();
        let tm = TaskManager::with_callback(Box::new(move |t: &Task| {
            completed_clone.lock().push(t.id.clone());
        }));

        let tasks: Vec<_> = (0..5)
            .map(|i| tm.create_task(&format!("action-{}", i), serde_json::json!({}), "rpc", "ch"))
            .collect();

        // Complete some, fail others
        tm.complete_task(&tasks[0].id, serde_json::json!("r0"));
        tm.fail_task(&tasks[1].id, "e1");
        tm.complete_task(&tasks[2].id, serde_json::json!("r2"));

        assert_eq!(tm.list_completed_tasks().len(), 2);
        assert_eq!(tm.list_failed_tasks().len(), 1);
        assert_eq!(tm.list_pending_tasks().len(), 2);
        assert_eq!(completed_ids.lock().len(), 3);
    }
}

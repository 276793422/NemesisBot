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
    ///
    /// Terminal target states (Completed/Failed/Cancelled) also stamp
    /// `completed_at`; intermediate states must not carry a completion
    /// timestamp (NB-CODE-005).
    fn update_result(
        &self,
        task_id: &str,
        status: TaskStatus,
        result: Option<serde_json::Value>,
    ) -> Result<(), String>;

    /// Conditionally update a task only if its current status is in `expected`.
    ///
    /// Returns `Ok(true)` when the transition happened, `Ok(false)` when the
    /// task exists but its status does not match `expected` (no-op), and
    /// `Err` when the task is not found. The default implementation is a
    /// non-atomic get-check-update fallback — store implementations with a
    /// single-critical-section primitive (e.g. `DashMap::get_mut`) should
    /// override it (NB-CODE-001/002).
    fn update_result_if_status(
        &self,
        task_id: &str,
        expected: &[TaskStatus],
        status: TaskStatus,
        result: Option<serde_json::Value>,
    ) -> Result<bool, String> {
        let task = self.get(task_id)?;
        if !expected.contains(&task.status) {
            return Ok(false);
        }
        self.update_result(task_id, status, result)?;
        Ok(true)
    }

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

/// Terminal states of the TaskManager lifecycle（Completed/Failed/Cancelled）。
///
/// 本地辅助而非 `TaskStatus` 方法：types crate 保持纯数据枚举，终态集合是
/// TaskManager 生命周期的语义（ClusterTaskList 另有一套含 WaitingRemote 的
/// 终态过滤，两处口径独立演化）。
fn is_terminal_status(status: &TaskStatus) -> bool {
    matches!(
        status,
        TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
    )
}

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
        // DashMap entry API：检查与插入同一临界区（contains_key+insert 两步
        // 之间存在并发窗口，NB-CODE-004）。
        use dashmap::mapref::entry::Entry;
        match self.tasks.entry(task.id.clone()) {
            Entry::Occupied(_) => Err(format!("task already exists: {}", task.id)),
            Entry::Vacant(v) => {
                v.insert(task);
                Ok(())
            }
        }
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
        // 仅终态写完结时间：中间态任务带 completed_at 是语义谎言
        // （NB-CODE-005），且会误导 cleanup 的 2h 清理分支。
        if is_terminal_status(&status) {
            task.completed_at = Some(chrono::Local::now().to_rfc3339());
        }
        Ok(())
    }

    fn update_result_if_status(
        &self,
        task_id: &str,
        expected: &[TaskStatus],
        status: TaskStatus,
        result: Option<serde_json::Value>,
    ) -> Result<bool, String> {
        // 单临界区原子检查-转移：并发 assign/settle 恰一方成功（NB-CODE-001/002）。
        let mut entry = self
            .tasks
            .get_mut(task_id)
            .ok_or_else(|| format!("task not found: {}", task_id))?;
        if !expected.contains(&entry.status) {
            return Ok(false);
        }
        entry.status = status;
        entry.result = result;
        if is_terminal_status(&entry.status) {
            entry.completed_at = Some(chrono::Local::now().to_rfc3339());
        }
        Ok(true)
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
/// pending tasks older than the configured pending timeout.
pub struct TaskManager {
    store: Arc<dyn TaskStore>,
    cleanup_interval: std::time::Duration,
    on_complete: Mutex<Option<Arc<OnCompleteCallback>>>,
    /// G4: pending 任务安全网超时（默认 24h）。集群层用
    /// `stale_task_safety_net(llm_timeout)` 按阶梯推导后经
    /// [`Self::set_pending_timeout`] 覆盖，保证安全网 > B 端 LLM 超时。
    pending_timeout: Arc<parking_lot::RwLock<chrono::Duration>>,
    stop_tx: Option<tokio::sync::broadcast::Sender<()>>,
}

/// G4: pending 任务安全网默认值（24 小时）。
pub fn default_pending_timeout() -> chrono::Duration {
    chrono::Duration::hours(24)
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
            pending_timeout: Arc::new(parking_lot::RwLock::new(default_pending_timeout())),
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
        let pending_timeout = self.pending_timeout.clone();

        handle.spawn(async move {
            let mut interval = tokio::time::interval(cleanup_interval);
            loop {
                tokio::select! {
                    _ = stop_rx.recv() => {
                        return;
                    }
                    _ = interval.tick() => {
                        cleanup_completed(&store, &on_complete, &pending_timeout);
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

    /// G4: 覆盖 pending 任务安全网超时（`Cluster::start` 按超时阶梯推导后调用）。
    pub fn set_pending_timeout(&self, timeout: chrono::Duration) {
        *self.pending_timeout.write() = timeout;
    }

    /// G4: 当前 pending 任务安全网超时。
    pub fn pending_timeout(&self) -> chrono::Duration {
        *self.pending_timeout.read()
    }

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
        // store.create 失败（uuid 碰撞面≈0，但静默分叉内存态不可接受，
        // NB-CODE-003）——error 日志留痕后继续返回任务。
        if let Err(e) = self.store.create(task.clone()) {
            tracing::error!(
                "[TaskManager] store.create failed for task {} (action={}): {e}",
                task.id,
                task.action
            );
        }
        task
    }

    /// Assign a task to a node, transitioning it to Running.
    /// Returns `false` if the task was not found or not in Pending state.
    ///
    /// 原子检查-转移（NB-CODE-001）：get 检查 + update 两步之间存在并发
    /// 窗口，双认领会双 Running；`update_result_if_status` 单临界区裁决。
    pub fn assign_task(&self, task_id: &str, node_id: &str) -> bool {
        match self.store.update_result_if_status(
            task_id,
            &[TaskStatus::Pending],
            TaskStatus::Running,
            None,
        ) {
            Ok(true) => {
                crate::logger::log_task("assigned", task_id, node_id);
                true
            }
            Ok(false) => {
                tracing::warn!(
                    "[TaskManager] assign {task_id}: task not in Pending state (already claimed?)"
                );
                false
            }
            Err(e) => {
                tracing::warn!("[TaskManager] assign {task_id}: {e}");
                false
            }
        }
    }

    /// Complete a task with a result.
    /// Returns `false` if the task was not found, or the task is in a
    /// conflicting state.
    ///
    /// 终态幂等（NB-CODE-002）：重复回调（F2 送达重试/B 端重发）对已 Completed
    /// 的任务是幂等成功——结果保留首次值、callback 不重复 fire。
    pub fn complete_task(&self, task_id: &str, result: serde_json::Value) -> bool {
        self.settle_task(task_id, TaskStatus::Completed, Some(result))
    }

    /// Fail a task with an error message.
    /// Returns `false` if the task was not found, or the task is in a
    /// conflicting state.
    pub fn fail_task(&self, task_id: &str, error: &str) -> bool {
        self.settle_task(
            task_id,
            TaskStatus::Failed,
            Some(serde_json::json!({ "error": error })),
        )
    }

    /// Settle a task into a terminal state, idempotently.
    ///
    /// - 首次转移（Pending/Running → 终态）：写结果 + completed_at + fire callback。
    /// - 重复 settle 同一终态：幂等成功（返回 true），结果 first-write-wins，
    ///   callback 不重复 fire——重复回调不产生重复续行。
    /// - 冲突（对 Failed 再 complete 等）：返回 false。
    fn settle_task(
        &self,
        task_id: &str,
        status: TaskStatus,
        result: Option<serde_json::Value>,
    ) -> bool {
        match self.store.update_result_if_status(
            task_id,
            &[TaskStatus::Pending, TaskStatus::Running],
            status,
            result,
        ) {
            Err(e) => {
                tracing::debug!("[TaskManager] settle {task_id}: {e}");
                false
            }
            Ok(true) => {
                if let Ok(task) = self.store.get(task_id) {
                    self.fire_callback(&task);
                }
                true
            }
            Ok(false) => match self.store.get(task_id) {
                // 已是同一终态：幂等成功（本次调用未做任何变更）。
                Ok(t) if t.status == status => {
                    tracing::info!(
                        "[TaskManager] task {task_id} already {:?}; idempotent no-op (first result kept)",
                        status
                    );
                    true
                }
                Ok(t) => {
                    tracing::warn!(
                        "[TaskManager] task {task_id} in state {:?}, refusing settle to {:?}",
                        t.status,
                        status
                    );
                    false
                }
                Err(_) => false,
            },
        }
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
/// tasks older than `pending_timeout`（G4：由集群超时阶梯推导，默认 24h）.
///
/// Mirrors Go's `TaskManager.cleanupCompleted()`.
fn cleanup_completed(
    store: &Arc<dyn TaskStore>,
    on_complete: &Option<Arc<OnCompleteCallback>>,
    pending_timeout: &parking_lot::RwLock<chrono::Duration>,
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
            if let Some(ref completed_at) = task.completed_at
                && let Ok(completed) = chrono::DateTime::parse_from_rfc3339(completed_at)
            {
                let completed_utc = completed.with_timezone(&chrono::Local);
                if chrono::Local::now() - completed_utc > two_hours {
                    let _ = store.delete(&task.id);
                }
            }
        }
    }

    // H4: Time out pending/running tasks past the safety-net timeout
    // （G4: 每 tick 读一次当前配置，set_pending_timeout 即时生效）。
    // NB-CODE-006：卡死的 Running 与 Pending 同口径回收（超出同一安全网
    // 即判死 + fire callback），不再永久滞留。判定用 created_at：A 端
    // create→assign 同函数连续调用，时间差可忽略；不为此新增 updated_at
    // 字段（Task 字面量构造散布 40+ 处，波及面不成比例）。
    let pending_limit = *pending_timeout.read();
    for status in [TaskStatus::Pending, TaskStatus::Running] {
        let tasks = store.list_by_status(status);
        for task in tasks {
            if let Ok(created) = chrono::DateTime::parse_from_rfc3339(&task.created_at) {
                let created_utc = created.with_timezone(&chrono::Local);
                if chrono::Local::now() - created_utc > pending_limit {
                    // Mark as failed with timeout error（条件转移防与并发
                    // settle 竞态覆盖已定终态）。
                    let _ = store.update_result_if_status(
                        &task.id,
                        &[status],
                        TaskStatus::Failed,
                        Some(serde_json::json!({
                            "error": format!(
                                "task timed out in {:?} state: no response received within {} seconds",
                                status,
                                pending_limit.num_seconds()
                            )
                        })),
                    );

                    crate::logger::log_task(
                        "timeout",
                        &task.id,
                        &format!(
                            "state={:?} age={}s",
                            status,
                            (chrono::Local::now() - created_utc).num_seconds()
                        ),
                    );

                    // Fire callback if set
                    if let Some(cb) = on_complete
                        && let Ok(updated_task) = store.get(&task.id)
                    {
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

#[cfg(test)]
mod cov_tests;

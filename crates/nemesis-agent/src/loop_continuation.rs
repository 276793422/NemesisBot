//! Phase 2 cluster continuation system.
//!
//! When an async tool (e.g. `cluster_rpc`) is invoked, the agent loop cannot
//! wait synchronously for the result. Instead, it saves a "continuation
//! snapshot" (messages + tool call ID + channel/chat context) so that when the
//! async callback arrives, the loop can be resumed exactly where it left off.
//!
//! The save-barrier pattern ensures that `load_continuation` never reads
//! partially-written data: a `ready` `Notify` is closed only after both the
//! in-memory map and the on-disk snapshot have been fully written.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, Notify};
use tracing::{debug, info, warn};

use crate::context::RequestContext;
use crate::r#loop::{LlmMessage, LlmProvider, Tool};
use crate::types::ToolCallInfo;

/// Trait for looking up tools by name. Implemented for both
/// `HashMap<String, Box<dyn Tool>>` and `HashMap<String, Arc<dyn Tool>>`.
pub trait ToolLookup {
    fn get_tool(&self, name: &str) -> Option<&dyn Tool>;
}

impl ToolLookup for std::collections::HashMap<String, Box<dyn Tool>> {
    fn get_tool(&self, name: &str) -> Option<&dyn Tool> {
        self.get(name).map(|b| b.as_ref() as &dyn Tool)
    }
}

impl ToolLookup for std::collections::HashMap<String, std::sync::Arc<dyn Tool>> {
    fn get_tool(&self, name: &str) -> Option<&dyn Tool> {
        self.get(name).map(|a| a.as_ref() as &dyn Tool)
    }
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// In-memory continuation snapshot.
///
/// `ready` is a [`Notify`] that is **closed** (all waiters woken) once the
/// snapshot data has been fully populated and persisted to disk. Callers that
/// arrive before the data is ready will block on `ready` for up to 5 seconds.
/// `ready_flag` is an [`AtomicBool`] that is set to `true` once the data is
/// ready, providing a non-blocking check that works even if `notify_waiters()`
/// was already called before the waiter registered.
#[derive(Debug)]
pub struct ContinuationData {
    /// LLM message snapshot (up to the assistant's tool_call).
    pub messages: Vec<LlmMessage>,
    /// The tool call ID that triggered the async operation.
    pub tool_call_id: String,
    /// Original channel for sending the final response.
    pub channel: String,
    /// Original chat ID.
    pub chat_id: String,
    /// Save barrier: notified when data is fully written.
    pub ready: Arc<Notify>,
    /// Non-blocking ready flag: set to true when data is fully written.
    pub ready_flag: Arc<AtomicBool>,
}

/// On-disk continuation snapshot (serialized as JSON).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContinuationSnapshot {
    pub task_id: String,
    pub messages: String, // JSON-encoded Vec<LlmMessage>
    pub tool_call_id: String,
    pub channel: String,
    pub chat_id: String,
    pub created_at: String,
}

/// Result from a continuation tool execution.
#[derive(Debug, Clone)]
pub struct ContinuationToolResult {
    /// Content for the LLM to consume.
    pub for_llm: String,
    /// Content for the user to see immediately (if not silent).
    pub for_user: String,
    /// Whether the tool result should be silently passed to the LLM only.
    pub silent: bool,
    /// Whether this tool result is from an async operation.
    pub is_async: bool,
    /// Task ID for async tools.
    pub task_id: Option<String>,
    /// Error message, if any.
    pub error: Option<String>,
}

impl Default for ContinuationToolResult {
    fn default() -> Self {
        Self {
            for_llm: String::new(),
            for_user: String::new(),
            silent: true,
            is_async: false,
            task_id: None,
            error: None,
        }
    }
}

// ---------------------------------------------------------------------------
// ContinuationStore -- persists snapshots to disk
// ---------------------------------------------------------------------------

/// Manages on-disk continuation snapshots under `{workspace}/cluster/rpc_cache/`.
pub struct ContinuationStore {
    base_dir: PathBuf,
}

impl ContinuationStore {
    /// Create a new store rooted at the given workspace directory.
    pub fn new(workspace: &std::path::Path) -> Self {
        let base_dir = workspace.join("cluster").join("rpc_cache");
        Self { base_dir }
    }

    /// Ensure the storage directory exists.
    fn ensure_dir(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.base_dir)
    }

    /// Save a continuation snapshot to disk.
    pub fn save(&self, snapshot: &ContinuationSnapshot) -> std::io::Result<()> {
        self.ensure_dir()?;
        let path = self.snapshot_path(&snapshot.task_id);
        let json = serde_json::to_string_pretty(snapshot)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(&path, json)?;
        Ok(())
    }

    /// Load a continuation snapshot from disk.
    pub fn load(&self, task_id: &str) -> std::io::Result<ContinuationSnapshot> {
        let path = self.snapshot_path(task_id);
        let json = std::fs::read_to_string(&path)?;
        serde_json::from_str(&json)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    }

    /// Delete a continuation snapshot from disk.
    pub fn delete(&self, task_id: &str) {
        let path = self.snapshot_path(task_id);
        if path.exists() {
            if let Err(e) = std::fs::remove_file(&path) {
                warn!("Failed to delete continuation snapshot {}: {}", task_id, e);
            }
        }
    }

    /// List all pending task IDs on disk.
    ///
    /// Scans the cache directory for `.json` files and returns their task IDs
    /// (matching Go's `ListPending` which scans disk on startup).
    pub fn list_pending(&self) -> Vec<String> {
        let mut task_ids = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.base_dir) else {
            return task_ids;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    task_ids.push(stem.to_string());
                }
            }
        }
        task_ids
    }

    /// Recover all continuation snapshots from disk into the manager's
    /// in-memory map.
    ///
    /// Scans the cache directory for `.json` files, deserializes each one,
    /// and populates the in-memory continuations so they can be resumed.
    /// Returns the number of snapshots recovered.
    pub fn recover_to_manager(&self, manager: &ContinuationManager) -> usize {
        let task_ids = self.list_pending();
        let mut recovered = 0;

        for task_id in &task_ids {
            // Skip if already in memory
            if manager.has_continuation_sync(task_id) {
                continue;
            }

            match self.load(task_id) {
                Ok(snapshot) => {
                    let messages: Vec<LlmMessage> = match serde_json::from_str(&snapshot.messages) {
                        Ok(m) => m,
                        Err(e) => {
                            warn!(
                                "Failed to deserialize messages for snapshot {}: {}",
                                task_id, e
                            );
                            continue;
                        }
                    };

                    // Create ready continuation data (loaded from disk, so already complete)
                    let ready = Arc::new(Notify::new());
                    let ready_flag = Arc::new(AtomicBool::new(true));

                    let cont_data = Arc::new(ContinuationData {
                        messages,
                        tool_call_id: snapshot.tool_call_id,
                        channel: snapshot.channel,
                        chat_id: snapshot.chat_id,
                        ready,
                        ready_flag,
                    });

                    manager.insert_continuation_sync(task_id.clone(), cont_data);
                    recovered += 1;
                    info!("Recovered continuation snapshot from disk: task_id={}", task_id);
                }
                Err(e) => {
                    warn!(
                        "Failed to load continuation snapshot {}: {}",
                        task_id, e
                    );
                }
            }
        }

        if recovered > 0 {
            info!("Recovered {} continuation snapshots from disk", recovered);
        }

        recovered
    }

    fn snapshot_path(&self, task_id: &str) -> PathBuf {
        self.base_dir.join(format!("{}.json", task_id))
    }
}

// ---------------------------------------------------------------------------
// ContinuationManager -- in-memory + disk dual-write
// ---------------------------------------------------------------------------

/// Manages continuation snapshots with the save-barrier pattern.
///
/// This is the main entry point for the Phase 2 continuation system.
/// It holds an in-memory cache of active continuations and an optional
/// disk store for persistence across restarts.
pub struct ContinuationManager {
    /// In-memory continuation data: task_id -> data.
    continuations: Mutex<HashMap<String, Arc<ContinuationData>>>,
    /// Optional disk store for persistence.
    disk_store: Option<ContinuationStore>,
    /// Timeout for waiting on the save barrier.
    barrier_timeout: Duration,
}

impl ContinuationManager {
    /// Create a new continuation manager without disk persistence.
    pub fn new() -> Self {
        Self {
            continuations: Mutex::new(HashMap::new()),
            disk_store: None,
            barrier_timeout: Duration::from_secs(5),
        }
    }

    /// Create a new continuation manager with disk persistence.
    ///
    /// Automatically scans the disk cache directory for any persisted
    /// continuation snapshots and loads them into memory (matching Go's
    /// `ListPending` disk scan on startup).
    pub fn with_disk_store(workspace: &std::path::Path) -> Self {
        let disk_store = ContinuationStore::new(workspace);
        let manager = Self {
            continuations: Mutex::new(HashMap::new()),
            disk_store: Some(disk_store),
            barrier_timeout: Duration::from_secs(5),
        };
        // Recover any pending snapshots from disk
        if let Some(ref store) = manager.disk_store {
            store.recover_to_manager(&manager);
        }
        manager
    }

    /// Set the barrier timeout (default: 5 seconds).
    pub fn set_barrier_timeout(&mut self, timeout: Duration) {
        self.barrier_timeout = timeout;
    }

    /// Save a continuation snapshot (memory + disk dual-write).
    ///
    /// This method implements the save-barrier pattern:
    /// 1. Create `ContinuationData` with an open `ready` Notify.
    /// 2. Insert into the in-memory map (loaders will see the entry but wait on `ready`).
    /// 3. Persist to disk.
    /// 4. Close the `ready` Notify (waking any waiting loaders).
    pub async fn save_continuation(
        &self,
        task_id: &str,
        messages: Vec<LlmMessage>,
        tool_call_id: &str,
        channel: &str,
        chat_id: &str,
    ) {
        let ready = Arc::new(Notify::new());
        let ready_flag = Arc::new(AtomicBool::new(false));

        let cont_data = Arc::new(ContinuationData {
            messages: messages.clone(),
            tool_call_id: tool_call_id.to_string(),
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
            ready: ready.clone(),
            ready_flag: ready_flag.clone(),
        });

        // Step 1: Insert into memory (ready not yet notified).
        {
            let mut conts = self.continuations.lock().await;
            conts.insert(task_id.to_string(), cont_data);
        }

        // Step 2: Persist to disk.
        if let Some(ref store) = self.disk_store {
            let messages_json = serde_json::to_string(&messages)
                .unwrap_or_else(|e| {
                    warn!("Failed to serialize messages for continuation: {}", e);
                    "[]".to_string()
                });
            let snapshot = ContinuationSnapshot {
                task_id: task_id.to_string(),
                messages: messages_json,
                tool_call_id: tool_call_id.to_string(),
                channel: channel.to_string(),
                chat_id: chat_id.to_string(),
                created_at: chrono::Utc::now().to_rfc3339(),
            };
            if let Err(e) = store.save(&snapshot) {
                warn!("Failed to persist continuation snapshot to disk: {}", e);
            }
        }

        // Step 3: Mark as ready and notify waiters.
        ready_flag.store(true, Ordering::Release);
        ready.notify_waiters();
        info!(
            "Continuation snapshot saved (memory + disk): task_id={}",
            task_id
        );
    }

    /// Load a continuation snapshot, trying memory first (with save-barrier wait),
    /// then falling back to disk.
    pub async fn load_continuation(&self, task_id: &str) -> Option<ContinuationData> {
        // Try memory with save-barrier.
        if let Some(data) = self.wait_for_continuation(task_id).await {
            return Some(data);
        }

        // Fall back to disk.
        self.try_load_from_disk(task_id).await
    }

    /// Wait for a continuation to be ready in memory.
    ///
    /// If the entry exists but `ready` hasn't been notified yet, we wait
    /// up to `barrier_timeout` for the data to be populated.
    /// If the entry doesn't exist at all, we retry with short sleeps
    /// until the timeout expires (covers the race where the callback
    /// arrives before the snapshot is registered).
    async fn wait_for_continuation(&self, task_id: &str) -> Option<ContinuationData> {
        let deadline = tokio::time::Instant::now() + self.barrier_timeout;

        loop {
            {
                let conts = self.continuations.lock().await;
                if let Some(data) = conts.get(task_id) {
                    // Entry exists. Check if already ready (non-blocking).
                    if data.ready_flag.load(Ordering::Acquire) {
                        return Some(ContinuationData {
                            messages: data.messages.clone(),
                            tool_call_id: data.tool_call_id.clone(),
                            channel: data.channel.clone(),
                            chat_id: data.chat_id.clone(),
                            ready: data.ready.clone(),
                            ready_flag: data.ready_flag.clone(),
                        });
                    }

                    let ready = data.ready.clone();
                    let ready_flag = data.ready_flag.clone();
                    drop(conts); // Release lock before awaiting.

                    // Double-check after releasing lock (save might have completed).
                    if ready_flag.load(Ordering::Acquire) {
                        let conts = self.continuations.lock().await;
                        return conts.get(task_id).map(|arc| {
                            ContinuationData {
                                messages: arc.messages.clone(),
                                tool_call_id: arc.tool_call_id.clone(),
                                channel: arc.channel.clone(),
                                chat_id: arc.chat_id.clone(),
                                ready: arc.ready.clone(),
                                ready_flag: arc.ready_flag.clone(),
                            }
                        });
                    }

                    // Wait for ready with remaining timeout.
                    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                    if remaining.is_zero() {
                        warn!(
                            "Continuation ready timeout, falling back to disk: task_id={}",
                            task_id
                        );
                        return None;
                    }

                    // Use tokio::select! to wait with a timeout.
                    let notified = tokio::select! {
                        _ = ready.notified() => true,
                        _ = tokio::time::sleep(remaining) => false,
                    };

                    if notified || ready_flag.load(Ordering::Acquire) {
                        // Data is ready. Read it.
                        let conts = self.continuations.lock().await;
                        return conts.get(task_id).map(|arc| {
                            ContinuationData {
                                messages: arc.messages.clone(),
                                tool_call_id: arc.tool_call_id.clone(),
                                channel: arc.channel.clone(),
                                chat_id: arc.chat_id.clone(),
                                ready: arc.ready.clone(),
                                ready_flag: arc.ready_flag.clone(),
                            }
                        });
                    } else {
                        warn!(
                            "Continuation ready timeout, falling back to disk: task_id={}",
                            task_id
                        );
                        return None;
                    }
                }
            }

            // Entry doesn't exist yet. Short sleep and retry.
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return None;
            }

            let sleep_duration = remaining.min(Duration::from_millis(10));
            tokio::time::sleep(sleep_duration).await;
        }
    }

    /// Try to load a continuation snapshot from disk (restart recovery path).
    async fn try_load_from_disk(&self, task_id: &str) -> Option<ContinuationData> {
        let store = self.disk_store.as_ref()?;
        let snapshot = store.load(task_id).ok()?;

        let messages: Vec<LlmMessage> = serde_json::from_str(&snapshot.messages).ok()?;

        let ready_flag = Arc::new(AtomicBool::new(true)); // Already ready from disk
        Some(ContinuationData {
            messages,
            tool_call_id: snapshot.tool_call_id,
            channel: snapshot.channel,
            chat_id: snapshot.chat_id,
            ready: Arc::new(Notify::new()),
            ready_flag,
        })
    }

    /// Remove a continuation from memory and disk.
    /// Mirrors Go's cleanup in `handleClusterContinuation` which deletes
    /// both the in-memory map entry and the disk snapshot.
    pub async fn remove_continuation(&self, task_id: &str) {
        {
            let mut conts = self.continuations.lock().await;
            conts.remove(task_id);
        }
        // Delete the disk snapshot as well to prevent unbounded disk growth.
        // Mirrors Go's: store.Delete(taskID).
        if let Some(ref store) = self.disk_store {
            store.delete(task_id);
        }
    }

    /// Check whether a continuation exists in memory.
    pub async fn has_continuation(&self, task_id: &str) -> bool {
        let conts = self.continuations.lock().await;
        conts.contains_key(task_id)
    }

    /// Check whether a continuation exists in memory (synchronous).
    ///
    /// Uses `blocking_lock` — safe during initialisation before any async
    /// tasks are competing for the lock.
    pub fn has_continuation_sync(&self, task_id: &str) -> bool {
        self.continuations.blocking_lock().contains_key(task_id)
    }

    /// Insert a continuation into the in-memory map (synchronous).
    ///
    /// Used during disk recovery at startup. Uses `blocking_lock` — safe
    /// during initialisation before any async tasks are competing for the lock.
    pub fn insert_continuation_sync(
        &self,
        task_id: String,
        data: Arc<ContinuationData>,
    ) {
        self.continuations.blocking_lock().insert(task_id, data);
    }
}

impl Default for ContinuationManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// handle_cluster_continuation -- the core continuation handler
// ---------------------------------------------------------------------------

/// Handle a cluster continuation callback.
///
/// This function:
/// 1. Loads the continuation snapshot.
/// 2. Retrieves the task result.
/// 3. Appends the real tool result to the messages.
/// 4. Runs the LLM + tool loop to completion.
/// 5. Publishes the final response.
pub async fn handle_cluster_continuation<T: ToolLookup>(
    manager: &ContinuationManager,
    task_id: &str,
    task_response: &str,
    task_failed: bool,
    task_error: Option<&str>,
    provider: &dyn LlmProvider,
    model: &str,
    tools: &T,
    outbound_tx: &tokio::sync::mpsc::Sender<nemesis_types::channel::OutboundMessage>,
) {
    // 1. Load continuation snapshot.
    let cont_data = match manager.load_continuation(task_id).await {
        Some(data) => data,
        None => {
            warn!(
                "Continuation data not found for task_id={}",
                task_id
            );
            return;
        }
    };

    // 2. Build tool result content from task response.
    let tool_result_content = if task_failed {
        format!(
            "Error: {}",
            task_error.unwrap_or("Task failed with unknown error")
        )
    } else {
        task_response.to_string()
    };

    // 3. Remove the continuation now that we have the data.
    manager.remove_continuation(task_id).await;

    // 4. Build messages: snapshot + real tool result.
    let mut messages = cont_data.messages.clone();
    messages.push(LlmMessage {
        role: "tool".to_string(),
        content: tool_result_content,
        tool_calls: None,
        tool_call_id: Some(cont_data.tool_call_id.clone()),
    });

    // 5. Run the continuation LLM + tool loop.
    let max_iterations = 20;
    let mut final_content = String::new();

    for iteration in 1..=max_iterations {
        debug!(
            "Continuation LLM iteration {}/{}: task_id={}",
            iteration, max_iterations, task_id
        );

        let response = match provider.chat(model, messages.clone(), None, vec![]).await {
            Ok(resp) => resp,
            Err(e) => {
                warn!("Continuation LLM call failed: {}", e);
                final_content = format!("[LLM error: {}]", e);
                break;
            }
        };

        if response.tool_calls.is_empty() {
            final_content = response.content.clone();
            break;
        }

        // Build assistant message with tool calls.
        let assistant_msg = LlmMessage {
            role: "assistant".to_string(),
            content: response.content.clone(),
            tool_calls: Some(response.tool_calls.clone()),
            tool_call_id: None,
        };
        messages.push(assistant_msg);

        // Execute tool calls.
        for tc in &response.tool_calls {
            let tool_result = execute_tool_for_continuation(
                tools,
                tc,
                &cont_data.channel,
                &cont_data.chat_id,
            )
            .await;

            // Send ForUser content if not silent.
            if !tool_result.silent && !tool_result.for_user.is_empty() {
                let outbound = nemesis_types::channel::OutboundMessage {
                    channel: cont_data.channel.clone(),
                    chat_id: cont_data.chat_id.clone(),
                    content: tool_result.for_user.clone(),
                    message_type: String::new(),
                };
                if let Err(e) = outbound_tx.send(outbound).await {
                    warn!("Failed to send continuation tool output: {}", e);
                }
            }

            // Handle nested async: save a new continuation.
            if tool_result.is_async {
                if let Some(ref nested_task_id) = tool_result.task_id {
                    manager
                        .save_continuation(
                            nested_task_id,
                            messages.clone(),
                            &tc.id,
                            &cont_data.channel,
                            &cont_data.chat_id,
                        )
                        .await;
                }
            }

            // Determine content for LLM.
            let content_for_llm = if tool_result.for_llm.is_empty() {
                tool_result.error.unwrap_or_default()
            } else {
                tool_result.for_llm
            };

            messages.push(LlmMessage {
                role: "tool".to_string(),
                content: content_for_llm,
                tool_calls: None,
                tool_call_id: Some(tc.id.clone()),
            });
        }
    }

    // 6. Send final response.
    if !final_content.is_empty() {
        let outbound = nemesis_types::channel::OutboundMessage {
            channel: cont_data.channel.clone(),
            chat_id: cont_data.chat_id.clone(),
            content: final_content.clone(),
            message_type: String::new(),
        };
        if let Err(e) = outbound_tx.send(outbound).await {
            warn!("Failed to send continuation final response: {}", e);
        }

        info!(
            "Continuation response sent: task_id={}, content_len={}, target_channel={}",
            task_id,
            final_content.len(),
            cont_data.channel
        );
    }
}

/// Execute a single tool call during continuation processing.
async fn execute_tool_for_continuation<T: ToolLookup>(
    tools: &T,
    tc: &ToolCallInfo,
    channel: &str,
    chat_id: &str,
) -> ContinuationToolResult {
    let context = RequestContext::new(channel, chat_id, "continuation", "continuation_session");

    match tools.get_tool(&tc.name) {
        Some(tool) => match tool.execute(&tc.arguments, &context).await {
            Ok(output) => ContinuationToolResult {
                for_llm: output,
                ..Default::default()
            },
            Err(e) => ContinuationToolResult {
                error: Some(e.clone()),
                ..Default::default()
            },
        },
        None => ContinuationToolResult {
            error: Some(format!("Unknown tool '{}'", tc.name)),
            ..Default::default()
        },
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Helper to create test LlmMessages.
    fn make_message(role: &str, content: &str) -> LlmMessage {
        LlmMessage {
            role: role.to_string(),
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    #[tokio::test]
    async fn test_save_and_load_continuation() {
        let manager = ContinuationManager::new();

        let messages = vec![
            make_message("system", "You are helpful."),
            make_message("user", "Hello"),
        ];

        manager
            .save_continuation("task-1", messages.clone(), "tc_1", "web", "chat1")
            .await;

        let loaded = manager.load_continuation("task-1").await;
        assert!(loaded.is_some());
        let data = loaded.unwrap();
        assert_eq!(data.messages.len(), 2);
        assert_eq!(data.tool_call_id, "tc_1");
        assert_eq!(data.channel, "web");
        assert_eq!(data.chat_id, "chat1");
    }

    #[tokio::test]
    async fn test_load_nonexistent_continuation() {
        let manager = ContinuationManager::new();
        let loaded = manager.load_continuation("nonexistent").await;
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn test_remove_continuation() {
        let manager = ContinuationManager::new();

        manager
            .save_continuation("task-2", vec![make_message("user", "test")], "tc_2", "web", "chat1")
            .await;

        assert!(manager.has_continuation("task-2").await);
        manager.remove_continuation("task-2").await;
        assert!(!manager.has_continuation("task-2").await);
    }

    #[tokio::test]
    async fn test_disk_persistence_and_recovery() {
        let tmp = TempDir::new().unwrap();
        let manager = ContinuationManager::with_disk_store(tmp.path());

        let messages = vec![
            make_message("system", "System prompt"),
            make_message("user", "Query"),
        ];

        manager
            .save_continuation("task-disk", messages.clone(), "tc_d", "rpc", "chat2")
            .await;

        // Verify it can be loaded while still in memory.
        let loaded = manager.load_continuation("task-disk").await;
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().tool_call_id, "tc_d");

        // Remove should clear both memory and disk (mirrors Go behavior).
        manager.remove_continuation("task-disk").await;
        assert!(!manager.has_continuation("task-disk").await);

        // After removal, should not be loadable (disk was also deleted).
        let loaded = manager.load_continuation("task-disk").await;
        assert!(loaded.is_none());
    }

    #[test]
    fn test_disk_recovery_on_startup() {
        let tmp = TempDir::new().unwrap();

        // Write a snapshot to disk manually.
        let store = ContinuationStore::new(tmp.path());
        let messages_json = serde_json::to_string(&vec![
            make_message("system", "System prompt"),
            make_message("user", "Query"),
        ])
        .unwrap();
        let snapshot = ContinuationSnapshot {
            task_id: "task-recover".to_string(),
            messages: messages_json,
            tool_call_id: "tc_r".to_string(),
            channel: "rpc".to_string(),
            chat_id: "chat_r".to_string(),
            created_at: "2026-04-29T12:00:00Z".to_string(),
        };
        store.save(&snapshot).unwrap();

        // Create a manager with disk store -- it should recover the snapshot on startup.
        // Uses a synchronous test since with_disk_store uses blocking_lock internally.
        let manager = ContinuationManager::with_disk_store(tmp.path());
        assert!(manager.has_continuation_sync("task-recover"));
    }

    #[tokio::test]
    async fn test_disk_store_save_and_load() {
        let tmp = TempDir::new().unwrap();
        let store = ContinuationStore::new(tmp.path());

        let snapshot = ContinuationSnapshot {
            task_id: "task-100".to_string(),
            messages: r#"[{"role":"user","content":"hello"}]"#.to_string(),
            tool_call_id: "tc_100".to_string(),
            channel: "web".to_string(),
            chat_id: "chat100".to_string(),
            created_at: "2026-04-29T12:00:00Z".to_string(),
        };

        store.save(&snapshot).unwrap();
        let loaded = store.load("task-100").unwrap();
        assert_eq!(loaded.task_id, "task-100");
        assert_eq!(loaded.tool_call_id, "tc_100");
    }

    #[tokio::test]
    async fn test_disk_store_delete() {
        let tmp = TempDir::new().unwrap();
        let store = ContinuationStore::new(tmp.path());

        let snapshot = ContinuationSnapshot {
            task_id: "task-del".to_string(),
            messages: "[]".to_string(),
            tool_call_id: "tc_del".to_string(),
            channel: "web".to_string(),
            chat_id: "chat-del".to_string(),
            created_at: "2026-04-29T12:00:00Z".to_string(),
        };

        store.save(&snapshot).unwrap();
        store.delete("task-del");
        assert!(store.load("task-del").is_err());
    }

    #[tokio::test]
    async fn test_save_barrier_pattern() {
        let manager = ContinuationManager::new();

        // Spawn a task that delays saving.
        let mgr = Arc::new(manager);
        let mgr_clone = mgr.clone();

        let save_handle = tokio::spawn(async move {
            // Small delay before saving.
            tokio::time::sleep(Duration::from_millis(50)).await;
            mgr_clone
                .save_continuation(
                    "task-barrier",
                    vec![make_message("user", "delayed")],
                    "tc_b",
                    "web",
                    "chat_b",
                )
                .await;
        });

        // The load should wait for the save to complete.
        let load_handle = tokio::spawn(async move {
            mgr.load_continuation("task-barrier").await
        });

        save_handle.await.unwrap();
        let loaded = load_handle.await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().tool_call_id, "tc_b");
    }

    #[tokio::test]
    async fn test_overwrite_continuation() {
        let manager = ContinuationManager::new();

        manager
            .save_continuation(
                "task-overwrite",
                vec![make_message("user", "first")],
                "tc_1",
                "web",
                "chat1",
            )
            .await;

        manager
            .save_continuation(
                "task-overwrite",
                vec![make_message("user", "second")],
                "tc_2",
                "web",
                "chat1",
            )
            .await;

        let loaded = manager.load_continuation("task-overwrite").await.unwrap();
        // The last save should have overwritten.
        assert_eq!(loaded.messages[0].content, "second");
        assert_eq!(loaded.tool_call_id, "tc_2");
    }

    // --- Additional continuation tests ---

    #[test]
    fn test_continuation_snapshot_serialization() {
        let snapshot = ContinuationSnapshot {
            task_id: "task-ser".to_string(),
            messages: r#"[{"role":"user","content":"hello"}]"#.to_string(),
            tool_call_id: "tc_ser".to_string(),
            channel: "web".to_string(),
            chat_id: "chat_ser".to_string(),
            created_at: "2026-04-29T12:00:00Z".to_string(),
        };

        let json = serde_json::to_string(&snapshot).unwrap();
        let parsed: ContinuationSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.task_id, "task-ser");
        assert_eq!(parsed.tool_call_id, "tc_ser");
        assert_eq!(parsed.channel, "web");
    }

    #[test]
    fn test_continuation_data_debug() {
        let data = ContinuationData {
            messages: vec![make_message("user", "test")],
            tool_call_id: "tc_1".to_string(),
            channel: "web".to_string(),
            chat_id: "chat1".to_string(),
            ready: Arc::new(tokio::sync::Notify::new()),
            ready_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        let debug_str = format!("{:?}", data);
        assert!(debug_str.contains("tc_1"));
    }

    #[test]
    fn test_continuation_store_load_nonexistent() {
        let tmp = TempDir::new().unwrap();
        let store = ContinuationStore::new(tmp.path());

        let result = store.load("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_continuation_store_delete_nonexistent() {
        let tmp = TempDir::new().unwrap();
        let store = ContinuationStore::new(tmp.path());

        // Should not panic
        store.delete("nonexistent");
    }

    #[test]
    fn test_manager_has_continuation_sync() {
        let manager = ContinuationManager::new();

        assert!(!manager.has_continuation_sync("task-sync"));

        // Use synchronous insert
        let data = Arc::new(ContinuationData {
            messages: vec![make_message("user", "test")],
            tool_call_id: "tc_s".to_string(),
            channel: "web".to_string(),
            chat_id: "chat1".to_string(),
            ready: Arc::new(tokio::sync::Notify::new()),
            ready_flag: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        });
        manager.insert_continuation_sync("task-sync".to_string(), data);

        assert!(manager.has_continuation_sync("task-sync"));
    }

    #[tokio::test]
    async fn test_manager_multiple_continuations() {
        let manager = ContinuationManager::new();

        for i in 0..5 {
            manager
                .save_continuation(
                    &format!("task-multi-{}", i),
                    vec![make_message("user", &format!("msg {}", i))],
                    &format!("tc_{}", i),
                    "web",
                    &format!("chat_{}", i),
                )
                .await;
        }

        for i in 0..5 {
            assert!(manager.has_continuation(&format!("task-multi-{}", i)).await);
            let loaded = manager.load_continuation(&format!("task-multi-{}", i)).await;
            assert!(loaded.is_some());
            assert_eq!(loaded.unwrap().tool_call_id, format!("tc_{}", i));
        }
    }

    #[test]
    fn test_continuation_store_list_pending_empty() {
        let tmp = TempDir::new().unwrap();
        let store = ContinuationStore::new(tmp.path());

        let pending = store.list_pending();
        assert!(pending.is_empty());
    }

    #[test]
    fn test_continuation_store_list_pending_with_snapshots() {
        let tmp = TempDir::new().unwrap();
        let store = ContinuationStore::new(tmp.path());

        for i in 0..3 {
            let snapshot = ContinuationSnapshot {
                task_id: format!("task-list-{}", i),
                messages: "[]".to_string(),
                tool_call_id: format!("tc_{}", i),
                channel: "web".to_string(),
                chat_id: format!("chat_{}", i),
                created_at: "2026-04-29T12:00:00Z".to_string(),
            };
            store.save(&snapshot).unwrap();
        }

        let pending = store.list_pending();
        assert_eq!(pending.len(), 3);
        // Should contain the task IDs (stems of the filenames)
        assert!(pending.contains(&"task-list-0".to_string()));
        assert!(pending.contains(&"task-list-1".to_string()));
        assert!(pending.contains(&"task-list-2".to_string()));
    }

    #[test]
    fn test_continuation_snapshot_clone() {
        let snapshot = ContinuationSnapshot {
            task_id: "task-clone".to_string(),
            messages: r#"[]"#.to_string(),
            tool_call_id: "tc_c".to_string(),
            channel: "web".to_string(),
            chat_id: "chat_c".to_string(),
            created_at: "2026-04-29T12:00:00Z".to_string(),
        };
        let cloned = snapshot.clone();
        assert_eq!(cloned.task_id, "task-clone");
        assert_eq!(cloned.tool_call_id, "tc_c");
    }

    #[tokio::test]
    async fn test_save_barrier_timeout() {
        let manager = ContinuationManager::new();

        // Load without save should return None quickly (5s timeout in impl)
        // Use a short timeout approach: just verify it returns None
        let loaded = manager.load_continuation("task-noexist-barrier").await;
        assert!(loaded.is_none());
    }

    #[test]
    fn test_continuation_data_with_ready_notify() {
        let notify = Arc::new(tokio::sync::Notify::new());
        let data = ContinuationData {
            messages: vec![make_message("user", "test")],
            tool_call_id: "tc_1".to_string(),
            channel: "web".to_string(),
            chat_id: "chat1".to_string(),
            ready: notify,
            ready_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };

        assert!(!data.ready_flag.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn test_concurrent_save_and_load() {
        let manager = Arc::new(ContinuationManager::new());
        let mut handles = Vec::new();

        // Spawn multiple concurrent saves
        for i in 0..10 {
            let mgr = manager.clone();
            handles.push(tokio::spawn(async move {
                mgr.save_continuation(
                    &format!("task-concurrent-{}", i),
                    vec![make_message("user", &format!("msg {}", i))],
                    &format!("tc_{}", i),
                    "web",
                    &format!("chat_{}", i),
                ).await;
            }));
        }

        // Wait for all saves
        for handle in handles {
            handle.await.unwrap();
        }

        // Verify all can be loaded
        for i in 0..10 {
            let loaded = manager.load_continuation(&format!("task-concurrent-{}", i)).await;
            assert!(loaded.is_some());
            assert_eq!(loaded.unwrap().tool_call_id, format!("tc_{}", i));
        }
    }

    #[test]
    fn test_tool_lookup_trait() {
        use async_trait::async_trait;

        struct MockLookupTool;
        #[async_trait]
        impl Tool for MockLookupTool {
            async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
                Ok("mock".to_string())
            }
        }

        struct TestLookup {
            tool: MockLookupTool,
        }
        impl ToolLookup for TestLookup {
            fn get_tool(&self, name: &str) -> Option<&dyn Tool> {
                if name == "known_tool" {
                    Some(&self.tool)
                } else {
                    None
                }
            }
        }

        let lookup = TestLookup { tool: MockLookupTool };
        assert!(lookup.get_tool("known_tool").is_some());
        assert!(lookup.get_tool("unknown_tool").is_none());
    }

    #[test]
    fn test_continuation_store_save_overwrite() {
        let tmp = TempDir::new().unwrap();
        let store = ContinuationStore::new(tmp.path());

        let snapshot1 = ContinuationSnapshot {
            task_id: "task-ov".to_string(),
            messages: r#"[]"#.to_string(),
            tool_call_id: "tc_1".to_string(),
            channel: "web".to_string(),
            chat_id: "chat1".to_string(),
            created_at: "2026-04-29T12:00:00Z".to_string(),
        };
        store.save(&snapshot1).unwrap();

        let snapshot2 = ContinuationSnapshot {
            task_id: "task-ov".to_string(),
            messages: r#"[]"#.to_string(),
            tool_call_id: "tc_2".to_string(),
            channel: "web".to_string(),
            chat_id: "chat1".to_string(),
            created_at: "2026-04-29T12:00:00Z".to_string(),
        };
        store.save(&snapshot2).unwrap();

        let loaded = store.load("task-ov").unwrap();
        assert_eq!(loaded.tool_call_id, "tc_2");
    }

    #[tokio::test]
    async fn test_remove_nonexistent_continuation() {
        let manager = ContinuationManager::new();
        // Should not panic
        manager.remove_continuation("nonexistent").await;
    }

    #[test]
    fn test_disk_store_corrupted_file() {
        let tmp = TempDir::new().unwrap();
        let store = ContinuationStore::new(tmp.path());

        // Write corrupted JSON
        std::fs::write(tmp.path().join("task-corrupt.json"), "not valid json").unwrap();

        let result = store.load("task-corrupt");
        assert!(result.is_err());
    }

    // --- Additional continuation coverage tests ---

    #[test]
    fn test_continuation_tool_result_default() {
        let result = ContinuationToolResult::default();
        assert!(result.for_llm.is_empty());
        assert!(result.for_user.is_empty());
        assert!(result.silent);
        assert!(!result.is_async);
        assert!(result.task_id.is_none());
        assert!(result.error.is_none());
    }

    #[test]
    fn test_continuation_manager_default() {
        let manager = ContinuationManager::default();
        assert!(!manager.has_continuation_sync("anything"));
    }

    #[tokio::test]
    async fn test_set_barrier_timeout() {
        let mut manager = ContinuationManager::new();
        manager.set_barrier_timeout(Duration::from_secs(10));
        // Verify it works by checking load returns None quickly for non-existent
        let loaded = manager.load_continuation("nonexistent-timeout").await;
        assert!(loaded.is_none());
    }

    #[test]
    fn test_continuation_store_nonexistent_dir_list_pending() {
        let tmp = TempDir::new().unwrap();
        let nonexistent = tmp.path().join("does_not_exist");
        let store = ContinuationStore::new(&nonexistent);
        let pending = store.list_pending();
        assert!(pending.is_empty());
    }

    #[tokio::test]
    async fn test_continuation_manager_with_disk_store_empty() {
        let tmp = TempDir::new().unwrap();
        let manager = ContinuationManager::with_disk_store(tmp.path());
        assert!(!manager.has_continuation("nonexistent").await);
    }

    #[test]
    fn test_continuation_store_recover_skips_already_loaded() {
        let tmp = TempDir::new().unwrap();
        let store = ContinuationStore::new(tmp.path());

        // Save a snapshot
        let snapshot = ContinuationSnapshot {
            task_id: "task-skip".to_string(),
            messages: r#"[{"role":"user","content":"hello"}]"#.to_string(),
            tool_call_id: "tc_skip".to_string(),
            channel: "web".to_string(),
            chat_id: "chat_skip".to_string(),
            created_at: "2026-04-29T12:00:00Z".to_string(),
        };
        store.save(&snapshot).unwrap();

        // Create a manager and manually insert the key first
        let manager = ContinuationManager::new();
        let data = Arc::new(ContinuationData {
            messages: vec![make_message("user", "manual")],
            tool_call_id: "tc_manual".to_string(),
            channel: "web".to_string(),
            chat_id: "chat1".to_string(),
            ready: Arc::new(tokio::sync::Notify::new()),
            ready_flag: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        });
        manager.insert_continuation_sync("task-skip".to_string(), data);

        // Recovery should skip since it's already in memory
        let recovered = store.recover_to_manager(&manager);
        assert_eq!(recovered, 0);
    }

    #[test]
    fn test_continuation_store_recover_corrupted_messages() {
        let tmp = TempDir::new().unwrap();
        let store = ContinuationStore::new(tmp.path());

        // Write a snapshot with invalid messages JSON
        let snapshot = ContinuationSnapshot {
            task_id: "task-bad-msg".to_string(),
            messages: "not valid json array".to_string(),
            tool_call_id: "tc_bad".to_string(),
            channel: "web".to_string(),
            chat_id: "chat_bad".to_string(),
            created_at: "2026-04-29T12:00:00Z".to_string(),
        };
        store.save(&snapshot).unwrap();

        let manager = ContinuationManager::new();
        let recovered = store.recover_to_manager(&manager);
        assert_eq!(recovered, 0);
        assert!(!manager.has_continuation_sync("task-bad-msg"));
    }

    #[test]
    fn test_tool_lookup_hashmap_box() {
        use async_trait::async_trait;

        struct TestTool;
        #[async_trait]
        impl Tool for TestTool {
            async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
                Ok("test".to_string())
            }
        }

        let mut map: HashMap<String, Box<dyn Tool>> = HashMap::new();
        map.insert("tool1".to_string(), Box::new(TestTool));

        assert!(map.get_tool("tool1").is_some());
        assert!(map.get_tool("unknown").is_none());
    }

    #[test]
    fn test_tool_lookup_hashmap_arc() {
        use async_trait::async_trait;

        struct TestTool;
        #[async_trait]
        impl Tool for TestTool {
            async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
                Ok("test".to_string())
            }
        }

        let mut map: HashMap<String, Arc<dyn Tool>> = HashMap::new();
        map.insert("tool1".to_string(), Arc::new(TestTool));

        assert!(map.get_tool("tool1").is_some());
        assert!(map.get_tool("unknown").is_none());
    }

    // --- Additional coverage for continuation handling ---

    use crate::r#loop::LlmResponse;
    use async_trait::async_trait;

    #[tokio::test]
    async fn test_handle_cluster_continuation_no_data() {
        // When continuation data doesn't exist, should return early
        let manager = ContinuationManager::new();
        let (outbound_tx, _outbound_rx) = tokio::sync::mpsc::channel(16);

        // No continuation saved, so this should not panic
        handle_cluster_continuation(
            &manager,
            "nonexistent-task",
            "response",
            false,
            None,
            &MockContinuationProvider::new(vec![]),
            "test-model",
            &HashMap::<String, Arc<dyn Tool>>::new(),
            &outbound_tx,
        )
        .await;
        // No outbound should be sent
    }

    #[tokio::test]
    async fn test_handle_cluster_continuation_simple_response() {
        let manager = ContinuationManager::new();
        let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(16);

        // Save a continuation snapshot
        let messages = vec![make_message("user", "Hello")];
        manager
            .save_continuation("task-1", messages, "tc_1", "web", "chat1")
            .await;

        // Provider returns a simple text response (no tool calls)
        let provider = MockContinuationProvider::new(vec![LlmResponse {
            content: "Continuation result".to_string(),
            tool_calls: Vec::new(),
            finished: true,
        }]);

        handle_cluster_continuation(
            &manager,
            "task-1",
            "task response",
            false,
            None,
            &provider,
            "test-model",
            &HashMap::<String, Arc<dyn Tool>>::new(),
            &outbound_tx,
        )
        .await;

        let outbound = outbound_rx.try_recv();
        assert!(outbound.is_ok());
        let out = outbound.unwrap();
        assert_eq!(out.channel, "web");
        assert_eq!(out.chat_id, "chat1");
        assert!(out.content.contains("Continuation result"));
    }

    #[tokio::test]
    async fn test_handle_cluster_continuation_failed_task() {
        let manager = ContinuationManager::new();
        let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(16);

        let messages = vec![make_message("user", "Hello")];
        manager
            .save_continuation("task-fail", messages, "tc_1", "web", "chat1")
            .await;

        let provider = MockContinuationProvider::new(vec![LlmResponse {
            content: "Error handled".to_string(),
            tool_calls: Vec::new(),
            finished: true,
        }]);

        handle_cluster_continuation(
            &manager,
            "task-fail",
            "",
            true,
            Some("Task execution failed"),
            &provider,
            "test-model",
            &HashMap::<String, Arc<dyn Tool>>::new(),
            &outbound_tx,
        )
        .await;

        let outbound = outbound_rx.try_recv();
        assert!(outbound.is_ok());
        assert!(outbound.unwrap().content.contains("Error handled"));
    }

    #[tokio::test]
    async fn test_handle_cluster_continuation_with_tool_calls() {
        let manager = ContinuationManager::new();
        let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(16);

        let messages = vec![make_message("user", "Hello")];
        manager
            .save_continuation("task-tool", messages, "tc_1", "web", "chat1")
            .await;

        // First response has tool call, second response is final
        let provider = MockContinuationProvider::new(vec![
            LlmResponse {
                content: String::new(),
                tool_calls: vec![ToolCallInfo {
                    id: "tc_cont_1".to_string(),
                    name: "echo".to_string(),
                    arguments: r#"{"text":"hello"}"#.to_string(),
                }],
                finished: false,
            },
            LlmResponse {
                content: "Tool executed".to_string(),
                tool_calls: Vec::new(),
                finished: true,
            },
        ]);

        let mut tools: HashMap<String, Arc<dyn Tool>> = HashMap::new();
        struct EchoTool;
        #[async_trait]
        impl Tool for EchoTool {
            async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
                let val: serde_json::Value = serde_json::from_str(args).unwrap();
                Ok(val.get("text").unwrap().as_str().unwrap().to_string())
            }
        }
        tools.insert("echo".to_string(), Arc::new(EchoTool));

        handle_cluster_continuation(
            &manager,
            "task-tool",
            "task response",
            false,
            None,
            &provider,
            "test-model",
            &tools,
            &outbound_tx,
        )
        .await;

        let outbound = outbound_rx.try_recv();
        assert!(outbound.is_ok());
        assert!(outbound.unwrap().content.contains("Tool executed"));
    }

    #[tokio::test]
    async fn test_handle_cluster_continuation_llm_error() {
        let manager = ContinuationManager::new();
        let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(16);

        let messages = vec![make_message("user", "Hello")];
        manager
            .save_continuation("task-err", messages, "tc_1", "web", "chat1")
            .await;

        let provider = MockContinuationProvider::new_error("LLM connection failed".to_string());

        handle_cluster_continuation(
            &manager,
            "task-err",
            "task response",
            false,
            None,
            &provider,
            "test-model",
            &HashMap::<String, Arc<dyn Tool>>::new(),
            &outbound_tx,
        )
        .await;

        let outbound = outbound_rx.try_recv();
        assert!(outbound.is_ok());
        assert!(outbound.unwrap().content.contains("LLM error"));
    }

    #[tokio::test]
    async fn test_handle_cluster_continuation_unknown_tool() {
        let manager = ContinuationManager::new();
        let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(16);

        let messages = vec![make_message("user", "Hello")];
        manager
            .save_continuation("task-unknown", messages, "tc_1", "web", "chat1")
            .await;

        let provider = MockContinuationProvider::new(vec![
            LlmResponse {
                content: String::new(),
                tool_calls: vec![ToolCallInfo {
                    id: "tc_unk".to_string(),
                    name: "nonexistent_tool".to_string(),
                    arguments: "{}".to_string(),
                }],
                finished: false,
            },
            LlmResponse {
                content: "Handled unknown tool".to_string(),
                tool_calls: Vec::new(),
                finished: true,
            },
        ]);

        handle_cluster_continuation(
            &manager,
            "task-unknown",
            "task response",
            false,
            None,
            &provider,
            "test-model",
            &HashMap::<String, Arc<dyn Tool>>::new(),
            &outbound_tx,
        )
        .await;

        let outbound = outbound_rx.try_recv();
        assert!(outbound.is_ok());
        assert!(outbound.unwrap().content.contains("Handled unknown tool"));
    }

    #[tokio::test]
    async fn test_execute_tool_for_continuation_success() {
        struct OkTool;
        #[async_trait]
        impl Tool for OkTool {
            async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
                Ok("tool result".to_string())
            }
        }

        let mut tools: HashMap<String, Arc<dyn Tool>> = HashMap::new();
        tools.insert("my_tool".to_string(), Arc::new(OkTool));

        let tc = ToolCallInfo {
            id: "tc_1".to_string(),
            name: "my_tool".to_string(),
            arguments: "{}".to_string(),
        };

        let result = execute_tool_for_continuation(&tools, &tc, "web", "chat1").await;
        assert_eq!(result.for_llm, "tool result");
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn test_execute_tool_for_continuation_error() {
        struct ErrorTool;
        #[async_trait]
        impl Tool for ErrorTool {
            async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
                Err("tool error".to_string())
            }
        }

        let mut tools: HashMap<String, Arc<dyn Tool>> = HashMap::new();
        tools.insert("error_tool".to_string(), Arc::new(ErrorTool));

        let tc = ToolCallInfo {
            id: "tc_1".to_string(),
            name: "error_tool".to_string(),
            arguments: "{}".to_string(),
        };

        let result = execute_tool_for_continuation(&tools, &tc, "web", "chat1").await;
        assert!(result.error.is_some());
        assert_eq!(result.error.unwrap(), "tool error");
    }

    #[test]
    fn test_continuation_tool_result_fields() {
        let result = ContinuationToolResult::default();
        assert!(result.for_llm.is_empty());
        assert!(result.for_user.is_empty());
        assert!(result.error.is_none());
        assert!(result.silent); // Default is silent
        assert!(!result.is_async);
        assert!(result.task_id.is_none());
    }

    #[test]
    fn test_continuation_snapshot_created_at() {
        let snapshot = ContinuationSnapshot {
            task_id: "t1".to_string(),
            messages: "[]".to_string(),
            tool_call_id: "tc1".to_string(),
            channel: "web".to_string(),
            chat_id: "chat1".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
        };
        assert_eq!(snapshot.task_id, "t1");
        assert_eq!(snapshot.created_at, "2026-01-01T00:00:00Z");
    }

    // --- Mock LLM Provider for continuation tests ---

    struct MockContinuationProvider {
        responses: std::sync::Mutex<Vec<LlmResponse>>,
        error: std::sync::Mutex<Option<String>>,
    }

    impl MockContinuationProvider {
        fn new(responses: Vec<LlmResponse>) -> Self {
            Self {
                responses: std::sync::Mutex::new(responses),
                error: std::sync::Mutex::new(None),
            }
        }

        fn new_error(err: String) -> Self {
            Self {
                responses: std::sync::Mutex::new(Vec::new()),
                error: std::sync::Mutex::new(Some(err)),
            }
        }
    }

    #[async_trait]
    impl crate::r#loop::LlmProvider for MockContinuationProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<crate::types::ChatOptions>,
            _tools: Vec<crate::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            if let Some(ref err) = *self.error.lock().unwrap() {
                return Err(err.clone());
            }
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                Ok(LlmResponse {
                    content: "No more responses".to_string(),
                    tool_calls: Vec::new(),
                    finished: true,
                })
            } else {
                Ok(responses.remove(0))
            }
        }
    }

    // --- Additional coverage tests ---

    #[test]
    fn test_continuation_tool_result_debug() {
        let result = ContinuationToolResult {
            for_llm: "test data".to_string(),
            for_user: "user data".to_string(),
            silent: false,
            is_async: true,
            task_id: Some("task-1".to_string()),
            error: Some("some error".to_string()),
        };
        let debug = format!("{:?}", result);
        assert!(debug.contains("test data"));
        assert!(debug.contains("task-1"));
    }

    #[test]
    fn test_continuation_tool_result_with_all_fields() {
        let result = ContinuationToolResult {
            for_llm: "for llm".to_string(),
            for_user: "for user".to_string(),
            silent: false,
            is_async: true,
            task_id: Some("task-42".to_string()),
            error: None,
        };
        assert_eq!(result.for_llm, "for llm");
        assert_eq!(result.for_user, "for user");
        assert!(!result.silent);
        assert!(result.is_async);
        assert_eq!(result.task_id.unwrap(), "task-42");
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn test_execute_tool_for_continuation_unknown_tool() {
        let tools: HashMap<String, Arc<dyn Tool>> = HashMap::new();

        let tc = ToolCallInfo {
            id: "tc_unk".to_string(),
            name: "nonexistent".to_string(),
            arguments: "{}".to_string(),
        };

        let result = execute_tool_for_continuation(&tools, &tc, "web", "chat1").await;
        assert!(result.error.is_some());
        assert!(result.error.unwrap().contains("Unknown tool"));
    }

    #[test]
    fn test_continuation_snapshot_deserialization() {
        let json = r#"{
            "task_id": "task-json",
            "messages": "[{\"role\":\"user\",\"content\":\"hello\"}]",
            "tool_call_id": "tc_json",
            "channel": "rpc",
            "chat_id": "chat_json",
            "created_at": "2026-04-29T12:00:00Z"
        }"#;
        let snapshot: ContinuationSnapshot = serde_json::from_str(json).unwrap();
        assert_eq!(snapshot.task_id, "task-json");
        assert_eq!(snapshot.channel, "rpc");
    }

    #[test]
    fn test_disk_persistence_load_from_disk() {
        let tmp = TempDir::new().unwrap();
        let store = ContinuationStore::new(tmp.path());

        // Save a snapshot
        let messages = vec![make_message("user", "disk test")];
        let messages_json = serde_json::to_string(&messages).unwrap();
        let snapshot = ContinuationSnapshot {
            task_id: "task-disk-load".to_string(),
            messages: messages_json,
            tool_call_id: "tc_dl".to_string(),
            channel: "web".to_string(),
            chat_id: "chat_dl".to_string(),
            created_at: "2026-04-29T12:00:00Z".to_string(),
        };
        store.save(&snapshot).unwrap();

        // Create manager with disk store and verify recovery (sync test because with_disk_store uses blocking_lock)
        let manager = ContinuationManager::with_disk_store(tmp.path());
        assert!(manager.has_continuation_sync("task-disk-load"));
    }

    #[tokio::test]
    async fn test_disk_store_remove_and_verify() {
        let tmp = TempDir::new().unwrap();
        let store = ContinuationStore::new(tmp.path());

        let snapshot = ContinuationSnapshot {
            task_id: "task-rm".to_string(),
            messages: "[]".to_string(),
            tool_call_id: "tc_rm".to_string(),
            channel: "web".to_string(),
            chat_id: "chat_rm".to_string(),
            created_at: "2026-04-29T12:00:00Z".to_string(),
        };
        store.save(&snapshot).unwrap();
        assert!(store.load("task-rm").is_ok());

        store.delete("task-rm");
        assert!(store.load("task-rm").is_err());
    }

    #[tokio::test]
    async fn test_handle_cluster_continuation_failed_task_no_error_msg() {
        let manager = ContinuationManager::new();
        let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(16);

        let messages = vec![make_message("user", "Hello")];
        manager
            .save_continuation("task-fail-no-err", messages, "tc_1", "web", "chat1")
            .await;

        let provider = MockContinuationProvider::new(vec![LlmResponse {
            content: "Error handled".to_string(),
            tool_calls: Vec::new(),
            finished: true,
        }]);

        // task_failed = true but error is None
        handle_cluster_continuation(
            &manager,
            "task-fail-no-err",
            "",
            true,
            None, // No error message
            &provider,
            "test-model",
            &HashMap::<String, Arc<dyn Tool>>::new(),
            &outbound_tx,
        )
        .await;

        let outbound = outbound_rx.try_recv();
        assert!(outbound.is_ok());
        assert!(outbound.unwrap().content.contains("Error handled"));
    }
}

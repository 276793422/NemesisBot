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

/// Trait for looking up tools by name.
pub trait ToolLookup {
    fn get_tool(&self, name: &str) -> Option<Arc<dyn Tool>>;
}

impl ToolLookup for std::collections::HashMap<String, Arc<dyn Tool>> {
    fn get_tool(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.get(name).cloned()
    }
}

impl ToolLookup for parking_lot::RwLock<std::collections::HashMap<String, Arc<dyn Tool>>> {
    fn get_tool(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.read().get(name).cloned()
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
                warn!("[Continuation] Failed to delete continuation snapshot {}: {}", task_id, e);
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
                                "[Continuation] Failed to deserialize messages for snapshot {}: {}",
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
                    info!("[Continuation] Recovered continuation snapshot from disk: task_id={}", task_id);
                }
                Err(e) => {
                    warn!(
                        "[Continuation] Failed to load continuation snapshot {}: {}",
                        task_id, e
                    );
                }
            }
        }

        if recovered > 0 {
            info!("[Continuation] Recovered {} continuation snapshots from disk", recovered);
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
                    warn!("[Continuation] Failed to serialize messages for continuation: {}", e);
                    "[]".to_string()
                });
            let snapshot = ContinuationSnapshot {
                task_id: task_id.to_string(),
                messages: messages_json,
                tool_call_id: tool_call_id.to_string(),
                channel: channel.to_string(),
                chat_id: chat_id.to_string(),
                created_at: chrono::Local::now().to_rfc3339(),
            };
            if let Err(e) = store.save(&snapshot) {
                warn!("[Continuation] Failed to persist continuation snapshot to disk: {}", e);
            }
        }

        // Step 3: Mark as ready and notify waiters.
        ready_flag.store(true, Ordering::Release);
        ready.notify_waiters();
        info!(
            "[Continuation] Continuation snapshot saved (memory + disk): task_id={}",
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
                            "[Continuation] Continuation ready timeout, falling back to disk: task_id={}",
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
                            "[Continuation] Continuation ready timeout, falling back to disk: task_id={}",
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
    observer_manager: Option<Arc<nemesis_observer::Manager>>,
) {
    // Generate trace_id for observer event correlation.
    let trace_id = format!("continuation-{}-{}", task_id, chrono::Local::now().timestamp_nanos_opt().unwrap_or(0));
    let start_time = std::time::Instant::now();

    // Emit conversation_start observer event.
    if let Some(ref mgr) = observer_manager {
        let event = crate::loop_executor::ObserverEvent::ConversationStart {
            trace_id: trace_id.clone(),
            session_key: format!("continuation-{}", task_id),
            channel: String::new(),
            chat_id: String::new(),
            sender_id: "continuation".to_string(),
            content: format!("cluster_continuation:{}", task_id),
        }.to_conversation_event();
        mgr.emit_sync(event).await;
    }
    // 1. Load continuation snapshot.
    let cont_data = match manager.load_continuation(task_id).await {
        Some(data) => data,
        None => {
            warn!(
                "[Continuation] Continuation data not found for task_id={}",
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
        reasoning_content: None,
    });

    // 5. Run the continuation LLM + tool loop.
    let max_iterations = 20;
    let mut final_content = String::new();

    for iteration in 1..=max_iterations {
        debug!(
            "[Continuation] Continuation LLM iteration {}/{}: task_id={}",
            iteration, max_iterations, task_id
        );

        // Emit LLM request observer event.
        if let Some(ref mgr) = observer_manager {
            let msg_values: Vec<serde_json::Value> = messages.iter()
                .filter_map(|m| serde_json::to_value(m).ok())
                .collect();
            let event = crate::loop_executor::ObserverEvent::LlmRequest {
                trace_id: trace_id.clone(),
                round: iteration as u32,
                model: model.to_string(),
                messages_count: messages.len(),
                tools_count: 0,
                messages: msg_values,
                tools: vec![],
                provider_name: String::new(),
                api_key: String::new(),
                api_base: String::new(),
            }.to_conversation_event();
            let mgr = Arc::clone(mgr);
            tokio::spawn(async move { mgr.emit(event).await });
        }

        let round_start = std::time::Instant::now();
        let mut response = match provider.chat(model, messages.clone(), None, vec![]).await {
            Ok(resp) => resp,
            Err(e) => {
                warn!("[Continuation] Continuation LLM call failed: {}", e);
                final_content = format!("[LLM error: {}]", e);
                break;
            }
        };

        // Emit LLM response observer event.
        let round_duration = round_start.elapsed();
        if let Some(ref mgr) = observer_manager {
            let tc_values: Vec<serde_json::Value> = response.tool_calls.iter()
                .filter_map(|tc| serde_json::to_value(tc).ok())
                .collect();
            let tc_count = response.tool_calls.len();
            let event = crate::loop_executor::ObserverEvent::LlmResponse {
                trace_id: trace_id.clone(),
                round: iteration as u32,
                duration_ms: round_duration.as_millis() as u64,
                has_tool_calls: !response.tool_calls.is_empty(),
                content: response.content.clone(),
                tool_calls: tc_values,
                tool_calls_count: tc_count,
                finish_reason: if response.finished { Some("stop".to_string()) } else { None },
                usage: response.usage.clone(),
                raw_request_body: response.raw_request_body.take(),
                raw_response_body: response.raw_response_body.take(),
            }.to_conversation_event();
            let mgr = Arc::clone(mgr);
            tokio::spawn(async move { mgr.emit(event).await });
        }

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
            reasoning_content: response.reasoning_content.clone(),
        };
        messages.push(assistant_msg);

        // Execute tool calls.
        for tc in &response.tool_calls {
            let tool_start = std::time::Instant::now();
            let tool_result = execute_tool_for_continuation(
                tools,
                tc,
                &cont_data.channel,
                &cont_data.chat_id,
            )
            .await;
            let tool_duration = tool_start.elapsed();

            // Emit tool call observer event.
            if let Some(ref mgr) = observer_manager {
                let result_str = tool_result.error.clone()
                    .unwrap_or_else(|| tool_result.for_llm.clone());
                let event = crate::loop_executor::ObserverEvent::ToolCall {
                    trace_id: trace_id.clone(),
                    tool_name: tc.name.clone(),
                    success: tool_result.error.is_none(),
                    duration_ms: tool_duration.as_millis() as u64,
                    round: iteration as u32,
                    arguments: tc.arguments.clone(),
                    result: result_str,
                }.to_conversation_event();
                let mgr = Arc::clone(mgr);
                tokio::spawn(async move { mgr.emit(event).await });
            }

            // Send ForUser content if not silent.
            if !tool_result.silent && !tool_result.for_user.is_empty() {
                let outbound = nemesis_types::channel::OutboundMessage {
                    channel: cont_data.channel.clone(),
                    chat_id: cont_data.chat_id.clone(),
                    content: tool_result.for_user.clone(),
                    message_type: String::new(),
                };
                if let Err(e) = outbound_tx.send(outbound).await {
                    warn!("[Continuation] Failed to send continuation tool output: {}", e);
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
                reasoning_content: None,
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
            warn!("[Continuation] Failed to send continuation final response: {}", e);
        }

        info!(
            "[Continuation] Continuation response sent: task_id={}, content_len={}, target_channel={}",
            task_id,
            final_content.len(),
            cont_data.channel
        );
    }

    // Emit conversation_end observer event.
    let duration_ms = start_time.elapsed().as_millis() as u64;
    if let Some(ref mgr) = observer_manager {
        let event = crate::loop_executor::ObserverEvent::ConversationEnd {
            trace_id: trace_id.clone(),
            session_key: format!("continuation-{}", task_id),
            total_rounds: max_iterations as u32,
            duration_ms,
            content: final_content.clone(),
            channel: cont_data.channel.clone(),
            chat_id: cont_data.chat_id.clone(),
        }.to_conversation_event();
        mgr.emit_sync(event).await;
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
mod tests;

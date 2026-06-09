//! Peer chat handler - processes incoming peer_chat RPC requests.
//!
//! Handles the B-side of a peer_chat: receives the message, enqueues it to
//! the cluster agent's work queue, and the cluster agent processes it
//! asynchronously with full tool execution capability.
//!
//! Key behaviour:
//! - Immediately returns an ACK to the caller (non-blocking)
//! - Creates a ClusterTask and submits to the work queue
//! - The cluster agent loop picks up the task, runs it through AgentLoop
//! - Results are sent back via callback to the originating node

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::cluster_task::{ClusterTask, ClusterTaskList, ClusterWorkQueue, TaskSource, TaskStatus};
use crate::rpc::client::RpcClient;

/// Default LLM request timeout (2 hours).
/// This is the maximum time to wait for a single LLM API request to respond.
/// Configurable via `llm_timeout_secs` in config.cluster.json.
pub const DEFAULT_LLM_TIMEOUT: Duration = Duration::from_secs(2 * 3600);

/// Maximum callback retry attempts.
const MAX_CALLBACK_RETRIES: usize = 3;

/// Callback backoff base (multiplied by attempt number).
const CALLBACK_BACKOFF_SECS: u64 = 5;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Peer chat request payload (decoded from the RPC action payload).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerChatRequest {
    /// The type of peer chat (chat, request, task, query).
    /// Serialized as "type" in JSON to match Go's field name.
    #[serde(default = "default_request_type", rename = "type")]
    pub request_type: String,
    /// The message content to process.
    pub content: String,
    /// Additional context (chat_id, sender_id, etc.).
    #[serde(default)]
    pub context: serde_json::Value,
}

fn default_request_type() -> String {
    "request".into()
}

/// Immediate acknowledgment for a peer_chat request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerChatAck {
    /// Whether the request was accepted.
    pub status: String,
    /// Task ID for tracking the async processing.
    pub task_id: String,
}

/// Result of the peer chat callback.
#[derive(Debug, Clone)]
pub struct PeerChatResult {
    pub task_id: String,
    pub status: String,
    pub response: String,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// LLM channel interface (decoupled from channels module)
// ---------------------------------------------------------------------------

/// Interface for sending a message through the local LLM pipeline.
/// Implemented by whatever provides the RPC channel in the production system.
pub trait LlmChannel: Send + Sync {
    /// Submit a message for LLM processing. Returns a receiver for the response.
    fn submit(
        &self,
        session_key: &str,
        content: &str,
        correlation_id: &str,
    ) -> Result<oneshot::Receiver<String>, String>;
}

// ---------------------------------------------------------------------------
// Task result persistence interface (B-side)
// ---------------------------------------------------------------------------

/// Interface for persisting task results when callback fails.
pub trait TaskResultPersister: Send + Sync {
    /// Mark a task as running.
    fn set_running(&self, task_id: &str, source_node: &str);
    /// Store the final result.
    fn set_result(
        &self,
        task_id: &str,
        status: &str,
        response: &str,
        error: &str,
        source_node: &str,
    ) -> Result<(), String>;
    /// Delete a task result (after successful callback).
    fn delete(&self, task_id: &str) -> Result<(), String>;
}

// ---------------------------------------------------------------------------
// PeerChatHandler
// ---------------------------------------------------------------------------

/// Handler for peer_chat actions on the receiving (B) side.
///
/// The handler is created once and reused for multiple requests.
/// It enqueues tasks to the cluster agent's work queue for full AgentLoop processing.
pub struct PeerChatHandler {
    node_id: String,
    timeout: Duration,
    llm_channel: Option<Arc<dyn LlmChannel>>,
    rpc_client: Option<Arc<RpcClient>>,
    result_persister: Option<Arc<dyn TaskResultPersister>>,
    /// Cluster agent work queue (preferred over llm_channel).
    cluster_task_list: Option<Arc<ClusterTaskList>>,
    cluster_work_queue: Option<Arc<ClusterWorkQueue>>,
    /// Track active async tasks for graceful shutdown.
    active_tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}

impl PeerChatHandler {
    /// Create a new peer chat handler with default 59-minute timeout.
    pub fn new(node_id: String) -> Self {
        Self {
            node_id,
            timeout: DEFAULT_LLM_TIMEOUT,
            llm_channel: None,
            rpc_client: None,
            result_persister: None,
            cluster_task_list: None,
            cluster_work_queue: None,
            active_tasks: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Create a handler with a custom timeout.
    pub fn with_timeout(node_id: String, timeout: Duration) -> Self {
        Self {
            timeout,
            ..Self::new(node_id)
        }
    }

    /// Set the LLM channel (legacy, used when cluster queue is not available).
    pub fn set_llm_channel(&mut self, channel: Arc<dyn LlmChannel>) {
        self.llm_channel = Some(channel);
    }

    /// Set the cluster agent work queue (preferred over LLM channel).
    pub fn set_cluster_queue(
        &mut self,
        task_list: Arc<ClusterTaskList>,
        work_queue: Arc<ClusterWorkQueue>,
    ) {
        self.cluster_task_list = Some(task_list);
        self.cluster_work_queue = Some(work_queue);
    }

    /// Set the RPC client for callbacks.
    pub fn set_rpc_client(&mut self, client: Arc<RpcClient>) {
        self.rpc_client = Some(client);
    }

    /// Set the task result persister.
    pub fn set_result_persister(&mut self, persister: Arc<dyn TaskResultPersister>) {
        self.result_persister = Some(persister);
    }

    /// Set the LLM request timeout.
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// Return the configured timeout.
    pub fn timeout_secs(&self) -> u64 {
        self.timeout.as_secs()
    }

    /// Return the node ID.
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    // -- Main entry point -----------------------------------------------------

    /// Handle an incoming peer_chat request.
    ///
    /// Validates the payload, extracts task/source info, returns an immediate
    /// ACK, and spawns an async task for LLM processing.
    pub fn handle(
        &self,
        payload: serde_json::Value,
        rpc_meta: Option<RpcMeta>,
    ) -> PeerChatAck {
        // 1. Parse payload
        let req: PeerChatRequest = match serde_json::from_value(payload.clone()) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(error = %e, "[PeerChat] Invalid peer_chat payload");
                return PeerChatAck {
                    status: "error".into(),
                    task_id: String::new(),
                };
            }
        };

        // 2. Validate
        if req.content.is_empty() {
            tracing::error!("[PeerChat] Missing content in peer_chat request");
            return PeerChatAck {
                status: "error".into(),
                task_id: String::new(),
            };
        }

        // 3. Extract task_id
        let task_id = payload
            .get("task_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let task_id = if task_id.is_empty() {
            format!("auto-{}", Uuid::new_v4())
        } else {
            task_id
        };

        // 4. Extract source info
        let source_info = payload.get("_source").cloned();
        let source_node_id = source_info
            .as_ref()
            .and_then(|s| s.get("node_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // 5. Mark running in result persister
        if !source_node_id.is_empty() {
            if let Some(ref persister) = self.result_persister {
                persister.set_running(&task_id, &source_node_id);
            }
        }

        // 6. Determine sender ID
        let sender_id = rpc_meta
            .as_ref()
            .and_then(|m| m.from.as_deref())
            .unwrap_or_else(|| {
                req.context
                    .get("sender_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("remote-peer")
            })
            .to_string();

        // 7. Enqueue to cluster agent or fall back to legacy LLM channel
        let cluster_task_list = self.cluster_task_list.clone();
        let cluster_work_queue = self.cluster_work_queue.clone();
        if let (Some(ref task_list), Some(ref work_queue)) =
            (cluster_task_list, cluster_work_queue)
        {
            // Create a cluster task and enqueue to the work queue.
            let cluster_task = ClusterTask {
                task_id: task_id.clone(),
                source: TaskSource {
                    node_id: source_node_id.clone(),
                    rpc_address: String::new(), // Filled from discovery if needed.
                    session_key: format!("cluster_rpc:{}", sender_id),
                },
                status: TaskStatus::Pending,
                content: req.content.clone(),
                conversation: None,
                waiting_for_task_id: None,
                waiting_tool_call_id: None,
                callback_result: None,
            };
            task_list.create_task(cluster_task);
            if let Err(e) = work_queue.submit(task_id.clone()) {
                tracing::error!(
                    task_id = %task_id,
                    error = %e,
                    "[PeerChat] Failed to submit to work queue"
                );
                return PeerChatAck {
                    status: "error".into(),
                    task_id: String::new(),
                };
            }
            tracing::info!(
                task_id = %task_id,
                source_node = %source_node_id,
                "[PeerChat] Task enqueued to cluster agent work queue"
            );
        } else {
            // Legacy path: use LLM channel directly.
            let llm_channel = self.llm_channel.clone();
            let rpc_client = self.rpc_client.clone();
            let result_persister = self.result_persister.clone();
            let timeout = self.timeout;
            let node_id = self.node_id.clone();
            let source_info_clone = source_info.clone();
            let source_node_id_clone = source_node_id.clone();

            let task_id_clone = task_id.clone();
            let handle = tokio::spawn(async move {
                process_async(
                    &task_id_clone,
                    &req,
                    &sender_id,
                    &source_node_id_clone,
                    &source_info_clone,
                    llm_channel.as_deref(),
                    rpc_client.as_deref(),
                    result_persister.as_deref(),
                    timeout,
                    &node_id,
                )
                .await;
            });
            self.active_tasks.lock().push(handle);
            tracing::info!(
                task_id = %task_id,
                source_node = %source_node_id,
                "[PeerChat] Peer chat task accepted (legacy LLM path)"
            );
        }

        PeerChatAck {
            status: "accepted".into(),
            task_id,
        }
    }

    /// Persist a task result to disk (called when callback fails).
    ///
    /// Mirrors Go's `PeerChatHandler.persistResult(taskID, resultStatus, response, errMsg, sourceNodeID)`.
    pub fn persist_result(
        &self,
        task_id: &str,
        status: &str,
        response: &str,
        error: &str,
        source_node_id: &str,
    ) {
        if let Some(ref persister) = self.result_persister {
            if !source_node_id.is_empty() {
                if let Err(e) = persister.set_result(task_id, status, response, error, source_node_id) {
                    tracing::error!(task_id = %task_id, error = %e, "[PeerChat] Failed to persist task result");
                } else {
                    tracing::info!(task_id = %task_id, "[PeerChat] Task result persisted for recovery");
                }
            }
        }
    }

    /// Delete a persisted task result (called after successful callback).
    ///
    /// Mirrors Go's `PeerChatHandler.deleteResult(taskID)`.
    pub fn delete_result(&self, task_id: &str) {
        if let Some(ref persister) = self.result_persister {
            if let Err(e) = persister.delete(task_id) {
                tracing::error!(task_id = %task_id, error = %e, "[PeerChat] Failed to delete task result");
            }
        }
    }

    /// Wait for all active async tasks to complete.
    pub async fn wait_for_tasks(&self) {
        let handles: Vec<_> = self.active_tasks.lock().drain(..).collect();
        for handle in handles {
            let _ = handle.await;
        }
    }

    /// Validate a peer chat request.
    pub fn validate(&self, req: &PeerChatRequest) -> Result<(), String> {
        if req.content.is_empty() {
            return Err("content is required".into());
        }
        Ok(())
    }
}

/// RPC metadata injected by the server (source node info).
#[derive(Debug, Clone)]
pub struct RpcMeta {
    pub from: Option<String>,
}

// ---------------------------------------------------------------------------
// Async processing
// ---------------------------------------------------------------------------

async fn process_async(
    task_id: &str,
    req: &PeerChatRequest,
    sender_id: &str,
    source_node_id: &str,
    source_info: &Option<serde_json::Value>,
    llm_channel: Option<&dyn LlmChannel>,
    rpc_client: Option<&RpcClient>,
    result_persister: Option<&dyn TaskResultPersister>,
    timeout: Duration,
    _node_id: &str,
) {
    tracing::info!(task_id = %task_id, "[PeerChat] Async LLM processing started");

    // 1. Check LLM channel availability
    let llm_ch = match llm_channel {
        Some(ch) => ch,
        None => {
            tracing::error!("[PeerChat] RPC channel not available for peer_chat");
            send_callback_or_persist(
                rpc_client,
                result_persister,
                source_info,
                source_node_id,
                task_id,
                "error",
                "",
                "rpc channel not available",
            )
            .await;
            return;
        }
    };

    // 2. Build session key and correlation ID
    let session_key = format!("cluster_rpc:{}", sender_id);
    let correlation_id = format!(
        "peer-chat-{}-{:04}",
        chrono::Local::now().timestamp_nanos_opt().unwrap_or(0),
        rand::random::<u16>() % 10000
    );

    tracing::info!(
        session_key = %session_key,
        correlation_id = %correlation_id,
        "[PeerChat] Submitting to LLM channel"
    );

    // 3. Submit to LLM channel
    let mut rx = match llm_ch.submit(&session_key, &req.content, &correlation_id) {
        Ok(rx) => rx,
        Err(e) => {
            tracing::error!(error = %e, "[PeerChat] Failed to submit to LLM channel");
            send_callback_or_persist(
                rpc_client,
                result_persister,
                source_info,
                source_node_id,
                task_id,
                "error",
                "",
                &format!("failed to process: {}", e),
            )
            .await;
            return;
        }
    };

    // 4. Wait for response with timeout
    let response = match tokio::time::timeout(timeout, &mut rx).await {
        Ok(Ok(response)) => {
            tracing::info!(task_id = %task_id, "[PeerChat] LLM response received");
            response
        }
        Ok(Err(_)) => {
            tracing::error!(task_id = %task_id, "[PeerChat] Response channel closed");
            send_callback_or_persist(
                rpc_client,
                result_persister,
                source_info,
                source_node_id,
                task_id,
                "error",
                "",
                "response channel closed",
            )
            .await;
            return;
        }
        Err(_) => {
            tracing::error!(task_id = %task_id, "[PeerChat] LLM processing timeout");
            send_callback_or_persist(
                rpc_client,
                result_persister,
                source_info,
                source_node_id,
                task_id,
                "error",
                "",
                "LLM processing timeout",
            )
            .await;
            return;
        }
    };

    // 5. Send callback with success
    send_callback_or_persist(
        rpc_client,
        result_persister,
        source_info,
        source_node_id,
        task_id,
        "success",
        &response,
        "",
    )
    .await;
}

/// Attempt to send the callback to the source node. If all retries fail,
/// persist the result locally.
async fn send_callback_or_persist(
    rpc_client: Option<&RpcClient>,
    result_persister: Option<&dyn TaskResultPersister>,
    _source_info: &Option<serde_json::Value>,
    source_node_id: &str,
    task_id: &str,
    status: &str,
    response: &str,
    error: &str,
) {
    let callback_ok = if !source_node_id.is_empty() {
        send_callback(rpc_client, source_node_id, task_id, status, response, error).await
    } else {
        tracing::error!(task_id = %task_id, "[PeerChat] No source node_id, cannot callback");
        false
    };

    if callback_ok {
        // Clean up persisted result
        if let Some(persister) = result_persister {
            if let Err(e) = persister.delete(task_id) {
                tracing::error!(task_id = %task_id, error = %e, "[PeerChat] Failed to delete task result");
            }
        }
    } else {
        // Persist locally for later recovery
        if let Some(persister) = result_persister {
            if !source_node_id.is_empty() {
                if let Err(e) = persister.set_result(task_id, status, response, error, source_node_id) {
                    tracing::error!(task_id = %task_id, error = %e, "[PeerChat] Failed to persist task result");
                } else {
                    tracing::info!(task_id = %task_id, "[PeerChat] Task result persisted for recovery");
                }
            }
        }
    }
}

/// Send callback to source node with retries.
pub async fn send_callback(
    rpc_client: Option<&RpcClient>,
    source_node_id: &str,
    task_id: &str,
    status: &str,
    response: &str,
    error: &str,
) -> bool {
    let client = match rpc_client {
        Some(c) => c,
        None => return false,
    };

    let mut payload = serde_json::json!({
        "task_id": task_id,
        "status": status,
        "response": response,
    });
    if !error.is_empty() {
        payload["error"] = serde_json::Value::String(error.into());
    }

    for attempt in 0..MAX_CALLBACK_RETRIES {
        let timeout = Duration::from_secs(30);
        let request = crate::rpc_types::RPCRequest {
            id: uuid::Uuid::new_v4().to_string(),
            action: crate::rpc_types::ActionType::Known(crate::rpc_types::KnownAction::PeerChatCallback),
            payload: payload.clone(),
            source: String::new(), // filled by client
            target: Some(source_node_id.into()),
        };

        match client.call_with_timeout(source_node_id, request, timeout).await {
            Ok(_) => {
                tracing::info!(
                    task_id = %task_id,
                    source_node = %source_node_id,
                    "[PeerChat] Callback sent successfully"
                );
                return true;
            }
            Err(e) => {
                tracing::warn!(
                    task_id = %task_id,
                    attempt = attempt + 1,
                    max_retries = MAX_CALLBACK_RETRIES,
                    error = %e,
                    "[PeerChat] Callback attempt failed"
                );
                if attempt < MAX_CALLBACK_RETRIES - 1 {
                    let backoff = Duration::from_secs(CALLBACK_BACKOFF_SECS * (attempt as u64 + 1));
                    tokio::time::sleep(backoff).await;
                }
            }
        }
    }

    tracing::error!(
        task_id = %task_id,
        source_node = %source_node_id,
        "[PeerChat] All callback retries exhausted"
    );
    false
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

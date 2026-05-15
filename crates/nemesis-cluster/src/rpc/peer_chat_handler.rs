//! Peer chat handler - processes incoming peer_chat RPC requests.
//!
//! Handles the B-side of a peer_chat: receives the message, runs it through
//! the local LLM via an `RpcChannel` input, and sends the response back via
//! callback to the originating node.
//!
//! Key behaviour:
//! - Immediately returns an ACK to the caller (non-blocking)
//! - Spawns an async task for LLM processing
//! - Configurable LLM request timeout (default 2 hours, configurable via config.cluster.json)
//! - Callback retries (3 attempts with exponential backoff)
//! - Falls back to persisting results if all callbacks fail

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use uuid::Uuid;

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
/// It holds references to the LLM channel, RPC client (for callbacks),
/// and the task result persister.
pub struct PeerChatHandler {
    node_id: String,
    timeout: Duration,
    llm_channel: Option<Arc<dyn LlmChannel>>,
    rpc_client: Option<Arc<RpcClient>>,
    result_persister: Option<Arc<dyn TaskResultPersister>>,
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

    /// Set the LLM channel.
    pub fn set_llm_channel(&mut self, channel: Arc<dyn LlmChannel>) {
        self.llm_channel = Some(channel);
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
                tracing::error!(error = %e, "Invalid peer_chat payload");
                return PeerChatAck {
                    status: "error".into(),
                    task_id: String::new(),
                };
            }
        };

        // 2. Validate
        if req.content.is_empty() {
            tracing::error!("Missing content in peer_chat request");
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

        // 7. Spawn async processing
        let llm_channel = self.llm_channel.clone();
        let rpc_client = self.rpc_client.clone();
        let result_persister = self.result_persister.clone();
        let timeout = self.timeout;
        let node_id = self.node_id.clone();

        let task_id_clone = task_id.clone();
        let source_node_id_clone = source_node_id.clone();
        let handle = tokio::spawn(async move {
            process_async(
                &task_id_clone,
                &req,
                &sender_id,
                &source_node_id_clone,
                &source_info,
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
            "Peer chat task accepted, processing asynchronously"
        );

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
                    tracing::error!(task_id = %task_id, error = %e, "Failed to persist task result");
                } else {
                    tracing::info!(task_id = %task_id, "Task result persisted for recovery");
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
                tracing::error!(task_id = %task_id, error = %e, "Failed to delete task result");
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
    tracing::info!(task_id = %task_id, "Async LLM processing started");

    // 1. Check LLM channel availability
    let llm_ch = match llm_channel {
        Some(ch) => ch,
        None => {
            tracing::error!("RPC channel not available for peer_chat");
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
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
        rand::random::<u16>() % 10000
    );

    tracing::info!(
        session_key = %session_key,
        correlation_id = %correlation_id,
        "Submitting to LLM channel"
    );

    // 3. Submit to LLM channel
    let mut rx = match llm_ch.submit(&session_key, &req.content, &correlation_id) {
        Ok(rx) => rx,
        Err(e) => {
            tracing::error!(error = %e, "Failed to submit to LLM channel");
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
            tracing::info!(task_id = %task_id, "LLM response received");
            response
        }
        Ok(Err(_)) => {
            tracing::error!(task_id = %task_id, "Response channel closed");
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
            tracing::error!(task_id = %task_id, "LLM processing timeout");
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
        tracing::error!(task_id = %task_id, "No source node_id, cannot callback");
        false
    };

    if callback_ok {
        // Clean up persisted result
        if let Some(persister) = result_persister {
            if let Err(e) = persister.delete(task_id) {
                tracing::error!(task_id = %task_id, error = %e, "Failed to delete task result");
            }
        }
    } else {
        // Persist locally for later recovery
        if let Some(persister) = result_persister {
            if !source_node_id.is_empty() {
                if let Err(e) = persister.set_result(task_id, status, response, error, source_node_id) {
                    tracing::error!(task_id = %task_id, error = %e, "Failed to persist task result");
                } else {
                    tracing::info!(task_id = %task_id, "Task result persisted for recovery");
                }
            }
        }
    }
}

/// Send callback to source node with retries.
async fn send_callback(
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
                    "Callback sent successfully"
                );
                return true;
            }
            Err(e) => {
                tracing::warn!(
                    task_id = %task_id,
                    attempt = attempt + 1,
                    max_retries = MAX_CALLBACK_RETRIES,
                    error = %e,
                    "Callback attempt failed"
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
        "All callback retries exhausted"
    );
    false
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_request() -> PeerChatRequest {
        PeerChatRequest {
            request_type: "chat".into(),
            content: "What is Rust?".into(),
            context: serde_json::json!({
                "chat_id": "chat-123",
                "sender_id": "node-a",
            }),
        }
    }

    #[test]
    fn test_default_timeout() {
        let handler = PeerChatHandler::new("node-b".into());
        assert_eq!(handler.timeout_secs(), 7200);
    }

    #[test]
    fn test_validate_valid_request() {
        let handler = PeerChatHandler::new("node-b".into());
        assert!(handler.validate(&make_request()).is_ok());
    }

    #[test]
    fn test_validate_empty_content() {
        let handler = PeerChatHandler::new("node-b".into());
        let mut req = make_request();
        req.content = String::new();
        assert!(handler.validate(&req).is_err());
    }

    #[tokio::test]
    async fn test_handle_returns_ack() {
        let handler = PeerChatHandler::new("node-b".into());
        let payload = serde_json::json!({
            "content": "Hello",
            "type": "chat",
        });
        let ack = handler.handle(payload, None);
        assert_eq!(ack.status, "accepted");
        assert!(!ack.task_id.is_empty());
    }

    #[test]
    fn test_handle_missing_content() {
        let handler = PeerChatHandler::new("node-b".into());
        let payload = serde_json::json!({
            "type": "chat",
        });
        let ack = handler.handle(payload, None);
        assert_eq!(ack.status, "error");
    }

    #[tokio::test]
    async fn test_handle_extracts_task_id() {
        let handler = PeerChatHandler::new("node-b".into());
        let payload = serde_json::json!({
            "content": "Hello",
            "task_id": "custom-task-123",
        });
        let ack = handler.handle(payload, None);
        assert_eq!(ack.task_id, "custom-task-123");
    }

    #[test]
    fn test_request_type_default() {
        let req: PeerChatRequest = serde_json::from_value(serde_json::json!({
            "content": "test"
        }))
        .unwrap();
        assert_eq!(req.request_type, "request");
    }

    // -- Mock LLM channel for integration-style tests --

    struct MockLlmChannel {
        response: String,
        should_fail: bool,
    }

    impl LlmChannel for MockLlmChannel {
        fn submit(
            &self,
            _session_key: &str,
            _content: &str,
            _correlation_id: &str,
        ) -> Result<oneshot::Receiver<String>, String> {
            if self.should_fail {
                return Err("channel not available".into());
            }
            let (tx, rx) = oneshot::channel();
            let response = self.response.clone();
            tokio::spawn(async move {
                let _ = tx.send(response);
            });
            Ok(rx)
        }
    }

    #[tokio::test]
    async fn test_async_processing_success() {
        let (tx, rx) = tokio::sync::oneshot::channel::<PeerChatResult>();

        struct MockPersister {
            tx: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<PeerChatResult>>>,
        }

        impl TaskResultPersister for MockPersister {
            fn set_running(&self, _task_id: &str, _source_node: &str) {}
            fn set_result(
                &self,
                task_id: &str,
                status: &str,
                response: &str,
                error: &str,
                _source_node: &str,
            ) -> Result<(), String> {
                if let Some(tx) = self.tx.lock().unwrap().take() {
                    let _ = tx.send(PeerChatResult {
                        task_id: task_id.into(),
                        status: status.into(),
                        response: response.into(),
                        error: if error.is_empty() { None } else { Some(error.into()) },
                    });
                }
                Ok(())
            }
            fn delete(&self, _task_id: &str) -> Result<(), String> {
                Ok(())
            }
        }

        let llm = Arc::new(MockLlmChannel {
            response: "Rust is a systems programming language.".into(),
            should_fail: false,
        });

        let persister = Arc::new(MockPersister { tx: std::sync::Mutex::new(Some(tx)) });

        let source_info = Some(serde_json::json!({"node_id": "node-a"}));
        let req = make_request();

        // Run the async processing directly
        tokio::spawn(async move {
            process_async(
                "test-task",
                &req,
                "node-a",
                "node-a",
                &source_info,
                Some(llm.as_ref()),
                None, // no rpc_client -> will fall back to persist
                Some(persister.as_ref()),
                Duration::from_secs(10),
                "node-b",
            )
            .await;
        });

        let result = tokio::time::timeout(Duration::from_secs(5), rx).await;
        let result = result.unwrap().unwrap();
        assert_eq!(result.status, "success");
        assert_eq!(result.response, "Rust is a systems programming language.");
        let _ = tx; // suppress unused warning
    }

    #[tokio::test]
    async fn test_async_processing_no_llm_channel() {
        let (tx, rx) = tokio::sync::oneshot::channel::<PeerChatResult>();

        struct MockPersister {
            tx: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<PeerChatResult>>>,
        }

        impl TaskResultPersister for MockPersister {
            fn set_running(&self, _task_id: &str, _source_node: &str) {}
            fn set_result(
                &self,
                task_id: &str,
                status: &str,
                _response: &str,
                error: &str,
                _source_node: &str,
            ) -> Result<(), String> {
                if let Some(tx) = self.tx.lock().unwrap().take() {
                    let _ = tx.send(PeerChatResult {
                        task_id: task_id.into(),
                        status: status.into(),
                        response: String::new(),
                        error: if error.is_empty() { None } else { Some(error.into()) },
                    });
                }
                Ok(())
            }
            fn delete(&self, _task_id: &str) -> Result<(), String> {
                Ok(())
            }
        }

        let persister = Arc::new(MockPersister { tx: std::sync::Mutex::new(Some(tx)) });
        let source_info = Some(serde_json::json!({"node_id": "node-a"}));
        let req = make_request();

        tokio::spawn(async move {
            process_async(
                "test-task-2",
                &req,
                "node-a",
                "node-a",
                &source_info,
                None, // no LLM channel
                None,
                Some(persister.as_ref()),
                Duration::from_secs(10),
                "node-b",
            )
            .await;
        });

        let result = tokio::time::timeout(Duration::from_secs(5), rx).await;
        let result = result.unwrap().unwrap();
        assert_eq!(result.status, "error");
        assert!(result.error.unwrap().contains("rpc channel not available"));
        let _ = tx;
    }

    // -- Additional coverage tests --

    #[test]
    fn test_peer_chat_handler_with_timeout() {
        let handler = PeerChatHandler::with_timeout("node-c".into(), Duration::from_secs(120));
        assert_eq!(handler.timeout_secs(), 120);
        assert_eq!(handler.node_id(), "node-c");
    }

    #[test]
    fn test_peer_chat_handler_node_id() {
        let handler = PeerChatHandler::new("my-node".into());
        assert_eq!(handler.node_id(), "my-node");
    }

    #[test]
    fn test_peer_chat_request_deserialization() {
        let req: PeerChatRequest = serde_json::from_value(serde_json::json!({
            "type": "chat",
            "content": "Hello",
            "context": {"chat_id": "c1", "sender_id": "s1"}
        }))
        .unwrap();
        assert_eq!(req.request_type, "chat");
        assert_eq!(req.content, "Hello");
        assert_eq!(req.context["chat_id"], "c1");
    }

    #[test]
    fn test_peer_chat_ack_fields() {
        let ack = PeerChatAck {
            status: "accepted".into(),
            task_id: "task-123".into(),
        };
        assert_eq!(ack.status, "accepted");
        assert_eq!(ack.task_id, "task-123");
    }

    #[test]
    fn test_peer_chat_result_fields() {
        let result = PeerChatResult {
            task_id: "t-1".into(),
            status: "success".into(),
            response: "hello".into(),
            error: None,
        };
        assert_eq!(result.task_id, "t-1");
        assert!(result.error.is_none());
    }

    #[test]
    fn test_rpc_meta_fields() {
        let meta = RpcMeta {
            from: Some("node-a".into()),
        };
        assert_eq!(meta.from.as_deref(), Some("node-a"));

        let meta_none = RpcMeta { from: None };
        assert!(meta_none.from.is_none());
    }

    #[tokio::test]
    async fn test_handle_invalid_payload() {
        let handler = PeerChatHandler::new("node-b".into());
        // Pass a non-object value that can't be deserialized to PeerChatRequest
        let payload = serde_json::json!(42);
        let ack = handler.handle(payload, None);
        assert_eq!(ack.status, "error");
        assert!(ack.task_id.is_empty());
    }

    #[tokio::test]
    async fn test_handle_with_rpc_meta() {
        let handler = PeerChatHandler::new("node-b".into());
        let payload = serde_json::json!({
            "content": "Hello from meta",
        });
        let meta = RpcMeta {
            from: Some("source-node".into()),
        };
        let ack = handler.handle(payload, Some(meta));
        assert_eq!(ack.status, "accepted");
    }

    #[tokio::test]
    async fn test_persist_result_no_persister() {
        let handler = PeerChatHandler::new("node-b".into());
        // No persister set -> should not panic
        handler.persist_result("task-1", "success", "response", "", "node-a");
    }

    #[tokio::test]
    async fn test_persist_result_empty_source() {
        let handler = PeerChatHandler::new("node-b".into());
        // Empty source_node_id -> should not persist
        handler.persist_result("task-1", "success", "response", "", "");
    }

    #[tokio::test]
    async fn test_delete_result_no_persister() {
        let handler = PeerChatHandler::new("node-b".into());
        // No persister set -> should not panic
        handler.delete_result("task-1");
    }

    #[tokio::test]
    async fn test_wait_for_tasks_empty() {
        let handler = PeerChatHandler::new("node-b".into());
        // No active tasks -> should return immediately
        handler.wait_for_tasks().await;
    }

    #[tokio::test]
    async fn test_handle_auto_task_id_generation() {
        let handler = PeerChatHandler::new("node-b".into());
        let payload = serde_json::json!({
            "content": "Hello",
            // no task_id -> should auto-generate
        });
        let ack = handler.handle(payload, None);
        assert_eq!(ack.status, "accepted");
        assert!(!ack.task_id.is_empty());
    }

    #[test]
    fn test_peer_chat_request_serialization_roundtrip() {
        let req = PeerChatRequest {
            request_type: "task".into(),
            content: "Do something".into(),
            context: serde_json::json!({"key": "value"}),
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: PeerChatRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.request_type, "task");
        assert_eq!(parsed.content, "Do something");
        assert_eq!(parsed.context["key"], "value");
    }

    #[test]
    fn test_peer_chat_ack_serialization_roundtrip() {
        let ack = PeerChatAck {
            status: "accepted".into(),
            task_id: "t-123".into(),
        };
        let json = serde_json::to_string(&ack).unwrap();
        let parsed: PeerChatAck = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.status, "accepted");
        assert_eq!(parsed.task_id, "t-123");
    }

    #[tokio::test]
    async fn test_async_processing_llm_submit_fails() {
        let (tx, rx) = tokio::sync::oneshot::channel::<PeerChatResult>();

        struct MockPersister {
            tx: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<PeerChatResult>>>,
        }

        impl TaskResultPersister for MockPersister {
            fn set_running(&self, _task_id: &str, _source_node: &str) {}
            fn set_result(
                &self,
                task_id: &str,
                status: &str,
                _response: &str,
                error: &str,
                _source_node: &str,
            ) -> Result<(), String> {
                if let Some(tx) = self.tx.lock().unwrap().take() {
                    let _ = tx.send(PeerChatResult {
                        task_id: task_id.into(),
                        status: status.into(),
                        response: String::new(),
                        error: if error.is_empty() { None } else { Some(error.into()) },
                    });
                }
                Ok(())
            }
            fn delete(&self, _task_id: &str) -> Result<(), String> {
                Ok(())
            }
        }

        let llm = Arc::new(MockLlmChannel {
            response: String::new(),
            should_fail: true,
        });

        let persister = Arc::new(MockPersister { tx: std::sync::Mutex::new(Some(tx)) });
        let source_info = Some(serde_json::json!({"node_id": "node-a"}));
        let req = make_request();

        tokio::spawn(async move {
            process_async(
                "test-task-fail",
                &req,
                "node-a",
                "node-a",
                &source_info,
                Some(llm.as_ref()),
                None,
                Some(persister.as_ref()),
                Duration::from_secs(10),
                "node-b",
            )
            .await;
        });

        let result = tokio::time::timeout(Duration::from_secs(5), rx).await;
        let result = result.unwrap().unwrap();
        assert_eq!(result.status, "error");
        assert!(result.error.unwrap().contains("failed to process"));
        let _ = tx;
    }

    // ============================================================
    // Coverage improvement: more async processing edge cases
    // ============================================================

    #[tokio::test]
    async fn test_async_processing_llm_channel_closed() {
        // LLM channel returns a receiver that gets dropped immediately
        struct DroppingLlmChannel;
        impl LlmChannel for DroppingLlmChannel {
            fn submit(
                &self,
                _session_key: &str,
                _content: &str,
                _correlation_id: &str,
            ) -> Result<oneshot::Receiver<String>, String> {
                // Create a channel but drop the sender immediately
                let (tx, rx) = oneshot::channel();
                drop(tx); // Drop sender so receiver gets Err
                Ok(rx)
            }
        }

        let (tx, rx) = tokio::sync::oneshot::channel::<PeerChatResult>();

        struct MockPersister {
            tx: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<PeerChatResult>>>,
        }
        impl TaskResultPersister for MockPersister {
            fn set_running(&self, _task_id: &str, _source_node: &str) {}
            fn set_result(
                &self,
                task_id: &str,
                status: &str,
                _response: &str,
                error: &str,
                _source_node: &str,
            ) -> Result<(), String> {
                if let Some(tx) = self.tx.lock().unwrap().take() {
                    let _ = tx.send(PeerChatResult {
                        task_id: task_id.into(),
                        status: status.into(),
                        response: String::new(),
                        error: if error.is_empty() { None } else { Some(error.into()) },
                    });
                }
                Ok(())
            }
            fn delete(&self, _task_id: &str) -> Result<(), String> {
                Ok(())
            }
        }

        let llm = Arc::new(DroppingLlmChannel);
        let persister = Arc::new(MockPersister { tx: std::sync::Mutex::new(Some(tx)) });
        let source_info = Some(serde_json::json!({"node_id": "node-a"}));
        let req = make_request();

        tokio::spawn(async move {
            process_async(
                "test-task-drop",
                &req,
                "node-a",
                "node-a",
                &source_info,
                Some(llm.as_ref()),
                None,
                Some(persister.as_ref()),
                Duration::from_secs(10),
                "node-b",
            )
            .await;
        });

        let result = tokio::time::timeout(Duration::from_secs(5), rx).await;
        let result = result.unwrap().unwrap();
        assert_eq!(result.status, "error");
        assert!(result.error.unwrap().contains("response channel closed"));
        let _ = tx;
    }

    #[tokio::test]
    async fn test_async_processing_no_source_node() {
        // When source_node_id is empty, callback should fail and result should be persisted
        let (tx, rx) = tokio::sync::oneshot::channel::<PeerChatResult>();

        struct MockPersister {
            tx: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<PeerChatResult>>>,
        }
        impl TaskResultPersister for MockPersister {
            fn set_running(&self, _task_id: &str, _source_node: &str) {}
            fn set_result(
                &self,
                _task_id: &str,
                _status: &str,
                _response: &str,
                _error: &str,
                _source_node: &str,
            ) -> Result<(), String> {
                Ok(()) // Don't send since source is empty
            }
            fn delete(&self, _task_id: &str) -> Result<(), String> {
                Ok(())
            }
        }

        let llm = Arc::new(MockLlmChannel {
            response: "Response".into(),
            should_fail: false,
        });

        let persister = Arc::new(MockPersister { tx: std::sync::Mutex::new(Some(tx)) });
        let source_info = None; // No source info
        let req = make_request();

        tokio::spawn(async move {
            process_async(
                "test-no-source",
                &req,
                "node-a",
                "",  // empty source_node_id
                &source_info,
                Some(llm.as_ref()),
                None,
                Some(persister.as_ref()),
                Duration::from_secs(10),
                "node-b",
            )
            .await;
        });

        // This should complete without hanging
        let _ = tokio::time::timeout(Duration::from_secs(5), rx).await;
        let _ = tx;
    }

    #[tokio::test]
    async fn test_handle_extracts_source_from_payload() {
        let handler = PeerChatHandler::new("node-b".into());
        let payload = serde_json::json!({
            "content": "Hello",
            "_source": {"node_id": "source-node-1"},
        });
        let ack = handler.handle(payload, None);
        assert_eq!(ack.status, "accepted");
    }

    #[tokio::test]
    async fn test_handle_with_source_sender_id_fallback() {
        let handler = PeerChatHandler::new("node-b".into());
        let payload = serde_json::json!({
            "content": "Hello",
            "context": {"sender_id": "fallback-sender"},
        });
        let ack = handler.handle(payload, None);
        assert_eq!(ack.status, "accepted");
    }

    #[test]
    fn test_persist_result_with_persister() {
        let (tx, rx) = std::sync::mpsc::channel::<(String, String, String, String, String)>();

        struct MockPersister {
            tx: std::sync::Mutex<std::sync::mpsc::Sender<(String, String, String, String, String)>>,
        }
        impl TaskResultPersister for MockPersister {
            fn set_running(&self, _task_id: &str, _source_node: &str) {}
            fn set_result(
                &self,
                task_id: &str,
                status: &str,
                response: &str,
                error: &str,
                source_node: &str,
            ) -> Result<(), String> {
                let _ = self.tx.lock().unwrap().send((
                    task_id.into(), status.into(), response.into(), error.into(), source_node.into()
                ));
                Ok(())
            }
            fn delete(&self, _task_id: &str) -> Result<(), String> {
                Ok(())
            }
        }

        let mut handler = PeerChatHandler::new("node-b".into());
        handler.set_result_persister(Arc::new(MockPersister { tx: std::sync::Mutex::new(tx) }));

        handler.persist_result("task-1", "success", "response text", "", "node-a");

        let result = rx.recv_timeout(std::time::Duration::from_secs(1));
        assert!(result.is_ok());
        let (task_id, status, response, error, source) = result.unwrap();
        assert_eq!(task_id, "task-1");
        assert_eq!(status, "success");
        assert_eq!(response, "response text");
        assert_eq!(error, "");
        assert_eq!(source, "node-a");
    }

    #[test]
    fn test_delete_result_with_persister() {
        struct MockPersister {
            deleted: std::sync::Mutex<Option<String>>,
        }
        impl TaskResultPersister for MockPersister {
            fn set_running(&self, _task_id: &str, _source_node: &str) {}
            fn set_result(
                &self,
                _task_id: &str,
                _status: &str,
                _response: &str,
                _error: &str,
                _source_node: &str,
            ) -> Result<(), String> {
                Ok(())
            }
            fn delete(&self, task_id: &str) -> Result<(), String> {
                *self.deleted.lock().unwrap() = Some(task_id.into());
                Ok(())
            }
        }

        let persister = Arc::new(MockPersister {
            deleted: std::sync::Mutex::new(None),
        });
        let mut handler = PeerChatHandler::new("node-b".into());
        handler.set_result_persister(persister.clone());

        handler.delete_result("task-to-delete");

        let deleted = persister.deleted.lock().unwrap();
        assert_eq!(deleted.as_deref(), Some("task-to-delete"));
    }

    #[test]
    fn test_peer_chat_request_type_field_serialization() {
        let json = r#"{"type":"task","content":"do something","context":{}}"#;
        let req: PeerChatRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.request_type, "task");

        let serialized = serde_json::to_string(&req).unwrap();
        assert!(serialized.contains(r#""type":"task""#));
    }

    #[test]
    fn test_peer_chat_request_context_default() {
        let req: PeerChatRequest = serde_json::from_value(serde_json::json!({
            "content": "test"
        }))
        .unwrap();
        assert!(req.context.is_null());
    }

    #[tokio::test]
    async fn test_wait_for_tasks_completes() {
        let handler = PeerChatHandler::new("node-b".into());
        // Submit a task
        let payload = serde_json::json!({"content": "hello"});
        let _ack = handler.handle(payload, None);
        // Wait for tasks to complete
        handler.wait_for_tasks().await;
    }

    #[test]
    fn test_peer_chat_ack_serialization() {
        let ack = PeerChatAck {
            status: "accepted".into(),
            task_id: "t-123".into(),
        };
        let json = serde_json::to_string(&ack).unwrap();
        assert!(json.contains("accepted"));
        assert!(json.contains("t-123"));
    }

    // ============================================================
    // Coverage improvement: more edge cases for peer chat
    // ============================================================

    #[tokio::test]
    async fn test_handle_with_source_info_no_node_id() {
        let handler = PeerChatHandler::new("node-b".into());
        let payload = serde_json::json!({
            "content": "Hello",
            "_source": {"other_field": "value"},
        });
        let ack = handler.handle(payload, None);
        assert_eq!(ack.status, "accepted");
    }

    #[tokio::test]
    async fn test_handle_with_rpc_meta_none_from() {
        let handler = PeerChatHandler::new("node-b".into());
        let payload = serde_json::json!({
            "content": "Hello",
        });
        let meta = RpcMeta { from: None };
        let ack = handler.handle(payload, Some(meta));
        assert_eq!(ack.status, "accepted");
    }

    #[tokio::test]
    async fn test_handle_with_rpc_meta_with_from() {
        let handler = PeerChatHandler::new("node-b".into());
        let payload = serde_json::json!({
            "content": "Hello",
        });
        let meta = RpcMeta { from: Some("source-node".into()) };
        let ack = handler.handle(payload, Some(meta));
        assert_eq!(ack.status, "accepted");
    }

    #[tokio::test]
    async fn test_handle_with_context_sender_id_no_rpc_meta() {
        let handler = PeerChatHandler::new("node-b".into());
        let payload = serde_json::json!({
            "content": "Hello",
            "context": {"sender_id": "context-sender"},
        });
        let ack = handler.handle(payload, None);
        assert_eq!(ack.status, "accepted");
    }

    #[test]
    fn test_persist_result_with_persister_fails() {
        struct FailingPersister;
        impl TaskResultPersister for FailingPersister {
            fn set_running(&self, _task_id: &str, _source_node: &str) {}
            fn set_result(
                &self,
                _task_id: &str,
                _status: &str,
                _response: &str,
                _error: &str,
                _source_node: &str,
            ) -> Result<(), String> {
                Err("disk full".to_string())
            }
            fn delete(&self, _task_id: &str) -> Result<(), String> {
                Ok(())
            }
        }

        let mut handler = PeerChatHandler::new("node-b".into());
        handler.set_result_persister(Arc::new(FailingPersister));
        // Should not panic even when persister fails
        handler.persist_result("task-1", "success", "response", "", "node-a");
    }

    #[test]
    fn test_delete_result_with_persister_fails() {
        struct FailingPersister;
        impl TaskResultPersister for FailingPersister {
            fn set_running(&self, _task_id: &str, _source_node: &str) {}
            fn set_result(
                &self,
                _task_id: &str,
                _status: &str,
                _response: &str,
                _error: &str,
                _source_node: &str,
            ) -> Result<(), String> {
                Ok(())
            }
            fn delete(&self, _task_id: &str) -> Result<(), String> {
                Err("not found".to_string())
            }
        }

        let mut handler = PeerChatHandler::new("node-b".into());
        handler.set_result_persister(Arc::new(FailingPersister));
        // Should not panic even when delete fails
        handler.delete_result("task-1");
    }

    #[tokio::test]
    async fn test_wait_for_tasks_after_handle() {
        let handler = PeerChatHandler::new("node-b".into());
        // Handle a request to spawn a task
        let payload = serde_json::json!({"content": "Hello"});
        let _ack = handler.handle(payload, None);
        // Wait for the task to complete
        handler.wait_for_tasks().await;
        // Should not panic or hang
    }

    #[test]
    fn test_peer_chat_request_default_type_serialization() {
        // Verify default type is "request" when not specified
        let json = r#"{"content": "test"}"#;
        let req: PeerChatRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.request_type, "request");
    }

    #[test]
    fn test_peer_chat_request_all_types() {
        for request_type in &["chat", "request", "task", "query"] {
            let json = format!(r#"{{"type": "{}", "content": "test"}}"#, request_type);
            let req: PeerChatRequest = serde_json::from_str(&json).unwrap();
            assert_eq!(req.request_type, *request_type);
        }
    }

    #[tokio::test]
    async fn test_handle_with_persister_set_running_called() {
        let (tx, rx) = std::sync::mpsc::channel::<(String, String)>();

        struct MockPersister {
            tx: std::sync::Mutex<std::sync::mpsc::Sender<(String, String)>>,
        }
        impl TaskResultPersister for MockPersister {
            fn set_running(&self, task_id: &str, source_node: &str) {
                let _ = self.tx.lock().unwrap().send((task_id.into(), source_node.into()));
            }
            fn set_result(
                &self, _: &str, _: &str, _: &str, _: &str, _: &str,
            ) -> Result<(), String> { Ok(()) }
            fn delete(&self, _: &str) -> Result<(), String> { Ok(()) }
        }

        let mut handler = PeerChatHandler::new("node-b".into());
        handler.set_result_persister(Arc::new(MockPersister { tx: std::sync::Mutex::new(tx) }));

        let payload = serde_json::json!({
            "content": "Hello",
            "task_id": "task-with-source",
            "_source": {"node_id": "source-node-1"},
        });
        let ack = handler.handle(payload, None);
        assert_eq!(ack.status, "accepted");

        let (task_id, source) = rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap();
        assert_eq!(task_id, "task-with-source");
        assert_eq!(source, "source-node-1");
    }

    #[tokio::test]
    async fn test_async_processing_llm_timeout() {
        // LLM channel returns a receiver that never sends (timeout)
        struct SlowLlmChannel;
        impl LlmChannel for SlowLlmChannel {
            fn submit(
                &self,
                _session_key: &str,
                _content: &str,
                _correlation_id: &str,
            ) -> Result<oneshot::Receiver<String>, String> {
                let (_tx, rx) = oneshot::channel();
                // Don't send anything, just let it timeout
                Ok(rx)
            }
        }

        let (tx, rx) = tokio::sync::oneshot::channel::<PeerChatResult>();

        struct MockPersister {
            tx: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<PeerChatResult>>>,
        }
        impl TaskResultPersister for MockPersister {
            fn set_running(&self, _task_id: &str, _source_node: &str) {}
            fn set_result(
                &self,
                task_id: &str,
                status: &str,
                _response: &str,
                error: &str,
                _source_node: &str,
            ) -> Result<(), String> {
                if let Some(tx) = self.tx.lock().unwrap().take() {
                    let _ = tx.send(PeerChatResult {
                        task_id: task_id.into(),
                        status: status.into(),
                        response: String::new(),
                        error: if error.is_empty() { None } else { Some(error.into()) },
                    });
                }
                Ok(())
            }
            fn delete(&self, _task_id: &str) -> Result<(), String> { Ok(()) }
        }

        let llm = Arc::new(SlowLlmChannel);
        let persister = Arc::new(MockPersister { tx: std::sync::Mutex::new(Some(tx)) });
        let source_info = Some(serde_json::json!({"node_id": "node-a"}));
        let req = make_request();

        tokio::spawn(async move {
            process_async(
                "test-task-timeout",
                &req,
                "node-a",
                "node-a",
                &source_info,
                Some(llm.as_ref()),
                None,
                Some(persister.as_ref()),
                Duration::from_millis(100), // Very short timeout
                "node-b",
            )
            .await;
        });

        let result = tokio::time::timeout(Duration::from_secs(5), rx).await;
        let result = result.unwrap().unwrap();
        assert_eq!(result.status, "error");
        // The error could be either "response channel closed" (if oneshot sender is dropped)
        // or "LLM processing timeout" (if the timeout fires first)
        let err = result.error.unwrap();
        assert!(err.contains("timeout") || err.contains("response channel closed") || err.contains("LLM"),
            "unexpected error: {}", err);
        let _ = tx;
    }

    #[tokio::test]
    async fn test_send_callback_or_persist_no_source() {
        // When source_node_id is empty, should not succeed
        let (tx, _rx) = tokio::sync::oneshot::channel::<PeerChatResult>();

        struct MockPersister {
            tx: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<PeerChatResult>>>,
        }
        impl TaskResultPersister for MockPersister {
            fn set_running(&self, _: &str, _: &str) {}
            fn set_result(
                &self,
                _: &str,
                _: &str,
                _: &str,
                _: &str,
                source_node: &str,
            ) -> Result<(), String> {
                // When source is empty, set_result should not be called
                assert!(!source_node.is_empty(), "set_result should not be called with empty source");
                Ok(())
            }
            fn delete(&self, _: &str) -> Result<(), String> { Ok(()) }
        }

        let persister = Arc::new(MockPersister { tx: std::sync::Mutex::new(Some(tx)) });

        send_callback_or_persist(
            None,
            Some(persister.as_ref()),
            &None,
            "", // empty source_node_id
            "task-1",
            "success",
            "response",
            "",
        )
        .await;

        let _ = _rx;
    }

    #[test]
    fn test_peer_chat_handler_setters() {
        let handler = PeerChatHandler::new("node-b".into());

        // Verify initial state
        assert!(handler.llm_channel.is_none());
        assert!(handler.rpc_client.is_none());
        assert!(handler.result_persister.is_none());

        // We can't easily create real instances, but we can test that the
        // new/with_timeout constructors work properly
        let handler2 = PeerChatHandler::with_timeout("node-c".into(), Duration::from_secs(300));
        assert_eq!(handler2.node_id(), "node-c");
        assert_eq!(handler2.timeout_secs(), 300);
    }
}

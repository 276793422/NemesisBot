//! Observer that publishes workflow_chat replies to the originating WS session.
//!
//! When a user sends a message via `/workflow/chat/<index>`, the WS handler
//! starts a workflow execution tagged with
//! [`TriggerSource::WorkflowChat`]. This observer watches for the matching
//! `Completed`/`Failed`/`Cancelled` events, builds a textual reply, writes
//! it back into the session_store as role="assistant", and broadcasts it to
//! the WS session.
//!
//! It also releases the per-workflow serialization mutex (held since
//! `start_async`) by dropping the guard stored in
//! [`WorkflowChatState::take_guard`].
//!
//! Reply text rules (decision: agent-node-first with terminal fallback):
//! - Completed: walk workflow's terminal agent nodes; if any has output
//!   `{ "response": "..." }`, use that text. Else JSON-dump terminal
//!   NodeResults.
//! - Failed: `"[工作流失败] {error}"`
//! - Cancelled: `"[工作流已取消]"`

use crate::protocol::ProtocolMessage;
use crate::session::SessionManager;
use async_trait::async_trait;
use nemesis_workflow::engine::WorkflowEngine;
use nemesis_workflow::events::{WorkflowEvent, WorkflowObserver};
use nemesis_workflow::types::TriggerSource;
use std::sync::Arc;

pub struct WorkflowChatReplyObserver {
    session_manager: Arc<SessionManager>,
    engine: Arc<WorkflowEngine>,
}

impl WorkflowChatReplyObserver {
    pub fn new(session_manager: Arc<SessionManager>, engine: Arc<WorkflowEngine>) -> Self {
        Self {
            session_manager,
            engine,
        }
    }
}

#[async_trait]
impl WorkflowObserver for WorkflowChatReplyObserver {
    fn name(&self) -> &str {
        "workflow_chat_reply_observer"
    }

    async fn on_event(&self, event: WorkflowEvent) {
        let (execution_id, terminal_state) = match &event {
            WorkflowEvent::Completed { execution_id, .. } => (execution_id.clone(), "completed"),
            WorkflowEvent::Failed { execution_id, .. } => (execution_id.clone(), "failed"),
            WorkflowEvent::Cancelled { execution_id, .. } => (execution_id.clone(), "cancelled"),
            _ => return,
        };

        let exec = match self.engine.get_execution(&execution_id).await {
            Some(e) => e,
            None => {
                tracing::warn!(
                    execution_id = %execution_id,
                    "[workflow_chat_observer] execution not found (may have been evicted)"
                );
                return;
            }
        };

        let ts = match &exec.trigger_source {
            Some(TriggerSource::WorkflowChat { chat_id: _, session_id, workflow_name, index: _, session_key: _ }) => {
                // Borrow fields by cloning so we can use them after the
                // trigger_source borrow ends.
                let session_id = session_id.clone();
                let workflow_name = workflow_name.clone();
                (session_id, workflow_name)
            }
            _ => return, // Not a workflow_chat execution; ignore.
        };

        // Release the per-workflow serialization mutex. The guard is dropped
        // at the end of this scope; take_guard removes it from the map.
        let _guard = self
            .engine
            .workflow_chat_state()
            .take_guard(&execution_id);

        let reply = match terminal_state {
            "completed" => build_completed_reply(&self.engine, &exec).await,
            "failed" => format!(
                "[工作流失败] {}",
                exec.error.as_deref().unwrap_or("未知错误")
            ),
            _ => "[工作流已取消]".to_string(),
        };

        // Persist assistant message under the same session_key as the user
        // turns so subsequent history_request calls see this reply.
        let session_key = format!("wf_chat:{}", ts.1);
        nemesis_agent::chat_log::append_chat_log(&session_key, "assistant", &reply);

        // Broadcast the reply to the originating WS session.
        let msg = ProtocolMessage::new(
            "message",
            "workflow_chat",
            "receive",
            Some(serde_json::json!({
                "role": "assistant",
                "content": reply,
            })),
        );
        let bytes = match msg.to_json() {
            Ok(b) => b,
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "[workflow_chat_observer] failed to encode reply"
                );
                return;
            }
        };
        if let Err(e) = self.session_manager.broadcast(&ts.0, &bytes).await {
            tracing::warn!(
                error = %e,
                session_id = %ts.0,
                "[workflow_chat_observer] broadcast failed"
            );
        }

        tracing::info!(
            workflow_name = %ts.1,
            execution_id = %execution_id,
            reply_len = reply.len(),
            "[workflow_chat_observer] reply delivered"
        );
    }
}

/// Build the reply text for a Completed execution.
///
/// 1. Look up the workflow def. If it has any terminal agent nodes (node_type
///    == "agent" && (is_terminal || is_leaf)), use the first one's
///    `output.response` text.
/// 2. Otherwise JSON-dump the merged terminal NodeResult outputs (same merge
///    logic as `Workflow::compute_output`).
async fn build_completed_reply(
    engine: &WorkflowEngine,
    exec: &nemesis_workflow::types::Execution,
) -> String {
    let workflow = match engine.get_workflow(&exec.workflow_name) {
        Some(w) => w,
        None => {
            // Workflow def removed mid-flight — fall back to JSON-dumping the
            // node_results that exist on the execution record.
            return serde_json::to_string_pretty(&exec.node_results)
                .unwrap_or_else(|_| "[工作流完成，但工作流定义已丢失]".to_string());
        }
    };

    // Use Workflow::compute_output for terminal merging semantics — this
    // matches what workflow_run agent tool returns to its caller.
    let merged = workflow.compute_output(&exec.node_results);

    // Agent node output signature is {"response": "..."}. If merged is an
    // object with a "response" string field, prefer that as the natural
    // language reply.
    if let Some(obj) = merged.as_object() {
        if let Some(resp) = obj.get("response").and_then(|v| v.as_str()) {
            if !resp.is_empty() {
                return resp.to_string();
            }
        }
    }

    // Fallback: JSON-dump the merged output. Keeps the workflow_chat page
    // useful for workflows that have no agent node (just code/start/etc.).
    if merged.is_null() {
        return "[工作流完成，无输出]".to_string();
    }
    serde_json::to_string_pretty(&merged).unwrap_or_else(|_| "[工作流完成]".to_string())
}

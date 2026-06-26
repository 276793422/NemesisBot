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
    build_completed_reply_with_workflow(&workflow, exec)
}

/// Pure reply-builder — split from [`build_completed_reply`] so tests can
/// drive it without spinning up a full WorkflowEngine. The engine-aware
/// wrapper handles the "workflow def missing" case; this function handles
/// everything else.
fn build_completed_reply_with_workflow(
    workflow: &nemesis_workflow::types::Workflow,
    exec: &nemesis_workflow::types::Execution,
) -> String {
    // Walk terminal/leaf nodes; if any produced a bare JSON string, treat it
    // as the user-facing reply. This is the path used by transform nodes
    // configured with `output_type: text|markdown|xml` which unwrap the
    // `{text: ...}` envelope into a bare string so the chat UI sees clean
    // text instead of JSON.
    for id in terminal_node_ids(workflow) {
        if let Some(nr) = exec.node_results.get(&id) {
            if let Some(s) = nr.output.as_str() {
                if !s.is_empty() {
                    return s.to_string();
                }
            }
        }
    }

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

/// Mirror of `Workflow::compute_output`'s terminal-node selection: prefer
/// nodes explicitly marked `is_terminal`, otherwise pick leaves (nodes that
/// aren't any edge's source). Used to inspect each terminal's raw output
/// before merging — needed because `compute_output` wraps bare-string
/// outputs into `{node_id: "..."}` objects, hiding the shape signal.
fn terminal_node_ids(workflow: &nemesis_workflow::types::Workflow) -> Vec<String> {
    let explicit: Vec<String> = workflow
        .nodes
        .iter()
        .filter(|n| n.is_terminal)
        .map(|n| n.id.clone())
        .collect();
    if !explicit.is_empty() {
        return explicit;
    }
    let downstream: Vec<&str> = workflow.edges.iter().map(|e| e.from_node.as_str()).collect();
    workflow
        .nodes
        .iter()
        .filter(|n| !downstream.contains(&n.id.as_str()))
        .map(|n| n.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Local;
    use nemesis_workflow::types::{
        Edge, Execution, ExecutionState, NodeDef, NodeResult, Workflow,
    };
    use std::collections::HashMap;

    fn make_node(id: &str, is_terminal: bool) -> NodeDef {
        NodeDef {
            id: id.to_string(),
            node_type: "transform".to_string(),
            config: HashMap::new(),
            depends_on: Vec::new(),
            retry_count: 0,
            timeout: None,
            is_terminal,
        }
    }

    fn make_result(id: &str, output: serde_json::Value) -> NodeResult {
        let now = Local::now();
        NodeResult {
            node_id: id.to_string(),
            output,
            error: None,
            state: ExecutionState::Completed,
            started_at: now,
            ended_at: now,
            metadata: HashMap::new(),
        }
    }

    fn make_execution(node_results: Vec<(&str, serde_json::Value)>) -> Execution {
        let mut nr_map = HashMap::new();
        for (id, v) in node_results {
            nr_map.insert(id.to_string(), make_result(id, v));
        }
        let mut exec = Execution::new("test_workflow".to_string(), HashMap::new());
        exec.node_results = nr_map;
        exec
    }

    /// Single terminal node with bare string output → returned as-is.
    /// This is the canonical output_type=text/markdown/xml path.
    #[test]
    fn test_bare_string_terminal_output_returned_directly() {
        let workflow = Workflow {
            name: "test_workflow".to_string(),
            description: String::new(),
            version: "1.0.0".to_string(),
            triggers: Vec::new(),
            nodes: vec![make_node("format_reply", true)],
            edges: Vec::new(),
            variables: HashMap::new(),
            metadata: HashMap::new(),
        };
        let exec = make_execution(vec![(
            "format_reply",
            serde_json::Value::String("hello world".to_string()),
        )]);
        let reply = build_completed_reply_with_workflow(&workflow, &exec);
        assert_eq!(reply, "hello world");
    }

    /// Bare string output on a leaf node (no explicit is_terminal) — same
    /// behavior via the fallback path.
    #[test]
    fn test_bare_string_leaf_output_returned_directly() {
        let workflow = Workflow {
            name: "test_workflow".to_string(),
            description: String::new(),
            version: "1.0.0".to_string(),
            triggers: Vec::new(),
            nodes: vec![
                make_node("upstream", false),
                make_node("leaf", false),
            ],
            edges: vec![Edge {
                from_node: "upstream".to_string(),
                to_node: "leaf".to_string(),
                condition: None,
            }],
            variables: HashMap::new(),
            metadata: HashMap::new(),
        };
        let exec = make_execution(vec![
            (
                "upstream",
                serde_json::json!({"text": "ignored"}),
            ),
            (
                "leaf",
                serde_json::Value::String("**译文**：hello".to_string()),
            ),
        ]);
        let reply = build_completed_reply_with_workflow(&workflow, &exec);
        assert_eq!(reply, "**译文**：hello");
    }

    /// Legacy agent-node path: terminal output is an object with a
    /// `response` field. Should still work via the merged-output branch.
    #[test]
    fn test_agent_response_field_still_works() {
        let workflow = Workflow {
            name: "test_workflow".to_string(),
            description: String::new(),
            version: "1.0.0".to_string(),
            triggers: Vec::new(),
            nodes: vec![make_node("agent_1", true)],
            edges: Vec::new(),
            variables: HashMap::new(),
            metadata: HashMap::new(),
        };
        let exec = make_execution(vec![(
            "agent_1",
            serde_json::json!({"response": "I am the agent reply"}),
        )]);
        let reply = build_completed_reply_with_workflow(&workflow, &exec);
        assert_eq!(reply, "I am the agent reply");
    }

    /// Empty bare string falls through the bare-string priority path (the
    /// `!s.is_empty()` guard treats empty as "no real reply") and ends up
    /// JSON-dumped via the merged-output fallback. This is intentional — an
    /// empty transform output shouldn't be served to webchat as a blank
    /// reply just because it has the right shape.
    #[test]
    fn test_empty_bare_string_falls_through() {
        let workflow = Workflow {
            name: "test_workflow".to_string(),
            description: String::new(),
            version: "1.0.0".to_string(),
            triggers: Vec::new(),
            nodes: vec![make_node("format_reply", true)],
            edges: Vec::new(),
            variables: HashMap::new(),
            metadata: HashMap::new(),
        };
        let exec = make_execution(vec![(
            "format_reply",
            serde_json::Value::String(String::new()),
        )]);
        let reply = build_completed_reply_with_workflow(&workflow, &exec);
        // compute_output wraps the empty string as {format_reply: ""} since
        // it's a non-object, non-null value — JSON-dump shows that shape.
        assert!(reply.contains("\"format_reply\""));
        assert!(reply.contains("\"\""));
    }

    /// JSON envelope output (no output_type configured) → JSON-dumped.
    /// This is the legacy behavior, preserved for backward compat.
    #[test]
    fn test_json_envelope_output_dumped_as_json() {
        let workflow = Workflow {
            name: "test_workflow".to_string(),
            description: String::new(),
            version: "1.0.0".to_string(),
            triggers: Vec::new(),
            nodes: vec![make_node("code_1", true)],
            edges: Vec::new(),
            variables: HashMap::new(),
            metadata: HashMap::new(),
        };
        let exec = make_execution(vec![(
            "code_1",
            serde_json::json!({"lines": ["a", "b", "c"]}),
        )]);
        let reply = build_completed_reply_with_workflow(&workflow, &exec);
        assert!(reply.contains("\"lines\""));
        assert!(reply.contains("\"a\""));
    }

    /// Multiple terminal nodes, first one with bare string wins. Order
    /// matches the workflow.nodes declaration order.
    #[test]
    fn test_first_terminal_with_bare_string_wins() {
        let workflow = Workflow {
            name: "test_workflow".to_string(),
            description: String::new(),
            version: "1.0.0".to_string(),
            triggers: Vec::new(),
            nodes: vec![
                make_node("first", true),
                make_node("second", true),
            ],
            edges: Vec::new(),
            variables: HashMap::new(),
            metadata: HashMap::new(),
        };
        let exec = make_execution(vec![
            ("first", serde_json::Value::String("first reply".to_string())),
            (
                "second",
                serde_json::json!({"response": "second reply"}),
            ),
        ]);
        let reply = build_completed_reply_with_workflow(&workflow, &exec);
        assert_eq!(reply, "first reply");
    }

    /// No node results at all → "[工作流完成，无输出]".
    #[test]
    fn test_no_node_results_returns_no_output_message() {
        let workflow = Workflow {
            name: "test_workflow".to_string(),
            description: String::new(),
            version: "1.0.0".to_string(),
            triggers: Vec::new(),
            nodes: vec![make_node("only", true)],
            edges: Vec::new(),
            variables: HashMap::new(),
            metadata: HashMap::new(),
        };
        let exec = make_execution(Vec::new());
        let reply = build_completed_reply_with_workflow(&workflow, &exec);
        assert_eq!(reply, "[工作流完成，无输出]");
    }
}

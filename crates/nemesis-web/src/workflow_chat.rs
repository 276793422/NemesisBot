//! WebSocket `workflow_chat` module — dedicated chat page for testing workflows.
//!
//! URL contract: being on `/#/workflow/chat/<index>` runs that workflow.
//! Each user message triggers a fresh execution; the workflow's terminal
//! output (or agent node reply) is broadcast back to the originating WS
//! session by [`crate::workflow_chat_reply_observer`].
//!
//! Two cmds live here:
//! - `send` — resolves `<index>` → workflow_name, serializes per-workflow,
//!   writes user msg to chat_log (`wf_chat:{name}` jsonl; the assistant
//!   side lands there via `workflow_chat_reply_observer`), calls
//!   `engine.start_async`. Workflow-chat sessions do NOT keep a
//!   SessionStore copy — chat_log is their single store (2026-08-25
//!   audit note; this line used to claim session_store, which was stale).
//! - `history_request` — reads prior turns from chat_log and replies
//!   on the same WS connection (bypasses the bus entirely).
//!
//! The chat_id passed in from the WebSocket layer is the transport chat_id
//! (`web:<session_id>`) — that's what the outbound dispatcher requires, so
//! we never rewrite it. The *logical* identifiers (`wf:<index>`,
//! `wf_chat:<workflow_name>`) live inside [`TriggerSource::WorkflowChat`]
//! and the session_key namespace only.

use crate::api_handlers::AppState;
use crate::protocol::ProtocolMessage;
use nemesis_workflow::types::TriggerSource;
use std::collections::HashMap;
use std::sync::Arc;

/// Top-level dispatch for `module == "workflow_chat"` messages.
///
/// `bound_workflow` is the workflow a workflow-chat session was bound to at
/// WS upgrade time (2026-10-16 hardening). When set, the client-supplied
/// `index` is ignored — the session can only ever talk to its own workflow.
/// Dashboard-originated sessions pass `None` and keep resolving `index`.
///
/// Returns `Ok(Some(IncomingMessage))` is never produced here — workflow_chat
/// never forwards to the bus. Returns `Ok(())` on successful handling (whether
/// or not the workflow has finished — that's async via the observer) and
/// `Err(String)` on protocol errors so the caller can surface them.
pub async fn handle_workflow_chat_message(
    state: Arc<AppState>,
    session_id: String,
    chat_id: String,
    pm: ProtocolMessage,
    bound_workflow: Option<String>,
) -> Result<(), String> {
    match pm.cmd.as_str() {
        "send" => handle_send(state, session_id, chat_id, pm, bound_workflow).await,
        "history_request" => handle_history_request(state, session_id, pm, bound_workflow).await,
        other => Err(format!("unknown workflow_chat cmd: {}", other)),
    }
}

async fn handle_send(
    state: Arc<AppState>,
    session_id: String,
    chat_id: String,
    pm: ProtocolMessage,
    bound_workflow: Option<String>,
) -> Result<(), String> {
    #[derive(serde::Deserialize)]
    struct SendData {
        // Optional so bound workflow-chat sessions may omit it; resolved
        // from the bound workflow when absent/ignored.
        index: Option<String>,
        content: String,
    }
    let data: SendData = pm
        .decode_data()
        .map_err(|e| format!("invalid workflow_chat.send data: {}", e))?;
    if data.content.trim().is_empty() {
        return Err("workflow_chat.send content cannot be empty".to_string());
    }

    let engine = match state.workflow_engine.clone() {
        Some(e) => e,
        None => {
            let _ = send_error(&state, &session_id, "工作流引擎未启用").await;
            return Ok(());
        }
    };

    // Resolve the target workflow: a bound workflow-chat session is pinned to
    // its own workflow; dashboard sessions resolve the client-supplied index.
    let (workflow_name, index_for_record) = match &bound_workflow {
        Some(w) => {
            let idx = nemesis_workflow::engine::WorkflowEngine::chat_index(w);
            (w.clone(), idx)
        }
        None => {
            let index = data.index.clone().unwrap_or_default();
            match engine.workflow_by_chat_index(&index) {
                Some(n) => (n, index),
                None => {
                    log_send_error(&state, &session_id, "未找到工作流").await;
                    return Ok(());
                }
            }
        }
    };

    // Re-check chat_eligible in case the workflow def was edited after the
    // client's resolve_chat_target call. human_review workflows pause in
    // Waiting state, which would hang the chat UI — reject up front instead.
    let wf = match engine.get_workflow(&workflow_name) {
        Some(w) => w,
        None => {
            log_send_error(&state, &session_id, "工作流定义加载失败").await;
            return Ok(());
        }
    };
    if wf.nodes.iter().any(|n| n.node_type == "human_review") {
        log_send_error(
            &state,
            &session_id,
            "工作流包含 human_review 节点，聊天测试不支持",
        )
        .await;
        return Ok(());
    }
    drop(wf);

    let session_key = format!("wf_chat:{}", workflow_name);

    // Acquire per-workflow mutex. Held across the workflow's lifetime — the
    // reply observer releases it when the workflow reaches a terminal state.
    let guard = engine.workflow_chat_state().acquire(&workflow_name).await;

    let trigger_source = TriggerSource::WorkflowChat {
        chat_id: chat_id.clone(),
        session_id: session_id.clone(),
        workflow_name: workflow_name.clone(),
        index: index_for_record,
        session_key: session_key.clone(),
    };

    let mut input: HashMap<String, serde_json::Value> = HashMap::new();
    input.insert(
        "content".to_string(),
        serde_json::Value::String(data.content.clone()),
    );
    // Unified `input` field — the "main input string" for this trigger.
    // Lets workflow authors write `{{input}}` in node prompts regardless
    // of which trigger source fired the workflow. For workflow_chat the
    // main input is the user-typed chat content.
    input.insert(
        "input".to_string(),
        serde_json::Value::String(data.content.clone()),
    );
    input.insert(
        "chat_id".to_string(),
        serde_json::Value::String(chat_id.clone()),
    );
    input.insert(
        "session_key".to_string(),
        serde_json::Value::String(session_key.clone()),
    );
    input.insert(
        "workflow_name".to_string(),
        serde_json::Value::String(workflow_name.clone()),
    );

    let exec_id = match engine
        .clone()
        .start_async(&workflow_name, input, Some(trigger_source))
        .await
    {
        Ok(id) => id,
        Err(e) => {
            // Release the mutex immediately — no observer will fire for this.
            drop(guard);
            log_send_error(&state, &session_id, &format!("启动工作流失败: {}", e)).await;
            return Ok(());
        }
    };

    // Persist user message only AFTER start_async succeeded. If we wrote it
    // before and start_async failed, the JSONL log would contain a user turn
    // with no matching assistant reply, leaving a confusing gap on reload.
    nemesis_agent::chat_log::append_chat_log(&session_key, "user", &data.content);

    engine.workflow_chat_state().store_guard(&exec_id, guard);

    tracing::info!(
        session_id = %session_id,
        workflow_name = %workflow_name,
        execution_id = %exec_id,
        "[workflow_chat] send: workflow started"
    );

    Ok(())
}

async fn handle_history_request(
    state: Arc<AppState>,
    session_id: String,
    pm: ProtocolMessage,
    bound_workflow: Option<String>,
) -> Result<(), String> {
    #[derive(serde::Deserialize)]
    struct HistoryReq {
        // Optional so bound workflow-chat sessions may omit it; resolved
        // from the bound workflow when absent/ignored.
        index: Option<String>,
        request_id: String,
        #[serde(default)]
        limit: Option<usize>,
        #[serde(default)]
        before_index: Option<usize>,
    }
    let req: HistoryReq = pm
        .decode_data()
        .map_err(|e| format!("invalid workflow_chat.history_request data: {}", e))?;

    let engine = match state.workflow_engine.clone() {
        Some(e) => e,
        None => {
            log_send_error(&state, &session_id, "工作流引擎未启用").await;
            return Ok(());
        }
    };

    // Same binding rule as send: a workflow-chat session only ever reads its
    // own workflow's history — the client-supplied index is ignored.
    let workflow_name = match &bound_workflow {
        Some(w) => w.clone(),
        None => {
            let index = req.index.clone().unwrap_or_default();
            match engine.workflow_by_chat_index(&index) {
                Some(n) => n,
                None => {
                    log_send_error(&state, &session_id, "未找到工作流").await;
                    return Ok(());
                }
            }
        }
    };

    let session_key = format!("wf_chat:{}", workflow_name);
    let limit = clamp_history_limit(req.limit);
    // chat_log::read_chat_log_async returns (page, total, has_more, oldest_index)
    // — same shape we need to forward to the client. _async（BUG 2026-09-22）：
    // 阻塞读移出 tokio worker。
    let (page, total_count, has_more, oldest_index) =
        nemesis_agent::chat_log::read_chat_log_async(&session_key, limit, req.before_index).await;

    let response = ProtocolMessage::new(
        "message",
        "workflow_chat",
        "history_response",
        Some(serde_json::json!({
            "request_id": req.request_id,
            "messages": page,
            "has_more": has_more,
            "oldest_index": oldest_index,
            "total_count": total_count,
        })),
    );
    let bytes = response
        .to_json()
        .map_err(|e| format!("failed to encode history response: {}", e))?;
    state
        .session_manager
        .broadcast(&session_id, &bytes)
        .await
        .map_err(|e| format!("failed to broadcast history response: {}", e))?;

    Ok(())
}

/// Protocol-level hard cap for `history_request` page size (2026-10-16
/// hardening): chat_log switches to whole-file mode at limit >= 100_000, so
/// an unbounded client value let any chat client force a full-log scan + a
/// giant page allocation. Missing or 0 → default 50; anything above 200 is
/// clamped to 200.
pub(crate) fn clamp_history_limit(limit: Option<usize>) -> usize {
    match limit {
        None | Some(0) => 50usize,
        Some(n) => n.clamp(1, 200),
    }
}

/// Emit a `workflow_chat.error` message to the client so the UI can surface
/// it (e.g., "工作流未找到"). Returns the broadcast result so callers can
/// log failures.
async fn send_error(state: &AppState, session_id: &str, error: &str) -> Result<(), String> {
    let msg = ProtocolMessage::new(
        "message",
        "workflow_chat",
        "error",
        Some(serde_json::json!({ "content": error })),
    );
    let bytes = msg
        .to_json()
        .map_err(|e| format!("failed to encode error: {}", e))?;
    state
        .session_manager
        .broadcast(session_id, &bytes)
        .await
        .map_err(|e| format!("failed to broadcast error: {}", e))
}

/// Like [`send_error`] but logs a warning if the broadcast itself fails
/// (e.g., WebSocket already closed). Use this from fire-and-forget paths
/// where the caller has no way to react to a send failure.
async fn log_send_error(state: &AppState, session_id: &str, error: &str) {
    if let Err(e) = send_error(state, session_id, error).await {
        tracing::warn!(
            error = %e,
            session_id = %session_id,
            "[workflow_chat] failed to deliver error to client"
        );
    }
}

//! Cluster agent event loop.
//!
//! Processes cluster tasks from the work queue using a full AgentLoop.
//! Supports new tasks (run_with_trace) and resumed tasks (resume_execution
//! after async callback). Detects __ASYNC__ results for multi-hop chain calls.

use std::sync::Arc;

use nemesis_agent::instance::AgentInstance;
use nemesis_agent::types::AgentConfig;
use nemesis_agent::r#loop::AgentLoop;
use nemesis_agent::types::AgentEvent;
use nemesis_agent::context::RequestContext;
use nemesis_cluster::cluster_task::{ClusterTaskList, ClusterWorkQueue, TaskStatus};
use nemesis_cluster::rpc::client::RpcClient;
use nemesis_cluster::rpc::peer_chat_handler::send_callback;

use crate::cluster_request_logger_observer::ClusterRequestLoggerObserver;

/// Run the cluster agent event loop.
///
/// This is the main entry point for the cluster agent. It loops forever,
/// taking tasks from the work queue and processing them one at a time.
///
/// **`config` vs `agent_loop`'s config**: these serve different purposes.
/// - `agent_loop`'s config: controls the LLM loop behavior (max_turns, provider, model).
/// - `config` parameter: used to create each `AgentInstance`, controlling per-task identity
///   (system_prompt, etc.). Currently system_prompt is None (placeholder), but will be
///   customized per task when "identity switching" is implemented (e.g., different prompts
///   for different source nodes).
pub async fn cluster_agent_loop(
    agent_loop: Arc<AgentLoop>,
    config: AgentConfig,
    work_queue: Arc<ClusterWorkQueue>,
    task_list: Arc<ClusterTaskList>,
    rpc_client: Option<Arc<RpcClient>>,
    cluster_observer: Option<Arc<ClusterRequestLoggerObserver>>,
    mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
) {
    tracing::info!("[ClusterAgent] Event loop started");

    loop {
        let task_id = tokio::select! {
            task = work_queue.next() => {
                match task {
                    Some(id) => id,
                    None => {
                        tracing::warn!("[ClusterAgent] Work queue closed, exiting event loop");
                        break;
                    }
                }
            }
            _ = shutdown_rx.recv() => {
                tracing::info!("[ClusterAgent] Shutdown signal received, exiting event loop");
                break;
            }
        };
        let task = match task_list.get_task(&task_id) {
            Some(t) => t,
            None => {
                tracing::warn!(
                    task_id = %task_id,
                    "[ClusterAgent] Task not found in task list, skipping"
                );
                continue;
            }
        };

        tracing::info!(
            task_id = %task_id,
            status = %task.status,
            "[ClusterAgent] Processing task"
        );

        task_list.update_status(&task_id, TaskStatus::Running);

        if task.conversation.is_some() && task.callback_result.is_some() {
            // Resume a task that was waiting for a remote callback.
            match resume_task(
                &agent_loop,
                &config,
                &task_list,
                rpc_client.as_deref(),
                cluster_observer.as_deref(),
                &task,
            )
            .await
            {
                Ok(()) => {}
                Err(e) => {
                    tracing::error!(
                        task_id = %task_id,
                        error = %e,
                        "[ClusterAgent] Resume task failed"
                    );
                    handle_task_error(&task_list, rpc_client.as_deref(), &task, &e.to_string())
                        .await;
                }
            }
        } else {
            // New task — execute from scratch.
            match execute_new_task(
                &agent_loop,
                &config,
                &task_list,
                rpc_client.as_deref(),
                cluster_observer.as_deref(),
                &task,
            )
            .await
            {
                Ok(()) => {}
                Err(e) => {
                    tracing::error!(
                        task_id = %task_id,
                        error = %e,
                        "[ClusterAgent] Execute task failed"
                    );
                    handle_task_error(&task_list, rpc_client.as_deref(), &task, &e.to_string())
                        .await;
                }
            }
        }
    }
}

/// Execute a new task using run_with_trace().
async fn execute_new_task(
    agent_loop: &AgentLoop,
    config: &AgentConfig,
    task_list: &ClusterTaskList,
    rpc_client: Option<&RpcClient>,
    cluster_observer: Option<&ClusterRequestLoggerObserver>,
    task: &nemesis_cluster::cluster_task::ClusterTask,
) -> Result<(), String> {
    let content_preview = truncate_str(&task.content, 200);
    nemesis_cluster::logger::log_task("exec_start", &task.task_id, &content_preview);
    let context = build_context(task);
    let trace_id = format!("cluster-{}", &task.task_id);
    // Per-task AgentInstance. The config controls this instance's identity (system_prompt, model).
    // Currently uses the shared cluster agent config, but will be customized per task
    // when "identity switching" is implemented (e.g., per-source-node system prompt).
    let instance = AgentInstance::new(config.clone());

    let token = tokio_util::sync::CancellationToken::new();
    if let Some(ref obs) = cluster_observer {
        obs.set_task_context(task.task_id.clone(), task.source.node_id.clone());
        obs.emit_conversation_start(
            &trace_id,
            "cluster",
            &task.task_id,
            &task.source.node_id,
            &task.content,
        );
    }
    let events = agent_loop
        .run_with_trace(&instance, &task.content, &context, &trace_id, false, &token)
        .await;
    if let Some(ref obs) = cluster_observer {
        let final_msg = extract_final_message(&events);
        let rounds = count_llm_rounds(&events);
        obs.emit_conversation_end(&trace_id, "cluster", &task.task_id, rounds, &final_msg, false);
        obs.clear_task_context();
    }

    let conversation = instance.get_history();
    if is_async_done(&conversation) {
        let conversation_json = serde_json::to_value(&conversation)
            .map_err(|e| format!("Failed to serialize conversation: {}", e))?;
        let (child_task_id, tool_call_id) =
            extract_async_info(&conversation).ok_or("Failed to extract async info")?;

        tracing::info!(
            task_id = %task.task_id,
            child_task_id = %child_task_id,
            "[ClusterAgent] Task went async, saving state"
        );

        nemesis_cluster::logger::log_task("exec_async", &task.task_id, &format!("child={}", child_task_id));

        task_list.save_async_state(
            &task.task_id,
            child_task_id,
            tool_call_id,
            conversation_json,
        );
        return Ok(());
    }

    let result = extract_final_message(&events);
    send_task_callback(rpc_client, task, "success", &result, "").await;
    task_list.complete_task(&task.task_id);
    nemesis_cluster::logger::log_task("exec_done", &task.task_id, &format!("events={}", events.len()));
    Ok(())
}

/// Resume a task after receiving a callback from a remote node.
async fn resume_task(
    agent_loop: &AgentLoop,
    config: &AgentConfig,
    task_list: &ClusterTaskList,
    rpc_client: Option<&RpcClient>,
    cluster_observer: Option<&ClusterRequestLoggerObserver>,
    task: &nemesis_cluster::cluster_task::ClusterTask,
) -> Result<(), String> {
    nemesis_cluster::logger::log_task("exec_resume", &task.task_id, "");
    // Per-task AgentInstance. Same rationale as execute_new_task — see its comment.
    let instance = AgentInstance::new(config.clone());

    // Restore conversation history.
    let conversation_json = task
        .conversation
        .as_ref()
        .ok_or("No conversation snapshot")?;
    let conversation: Vec<nemesis_agent::types::ConversationTurn> =
        serde_json::from_value(conversation_json.clone())
            .map_err(|e| format!("Failed to deserialize conversation: {}", e))?;
    instance.set_history(conversation);

    // Inject the callback result as a tool result.
    // Use replace (not add): the snapshot already contains an async placeholder
    // tool message with the same tool_call_id. Appending would create duplicate
    // tool_call_ids and LLM APIs reject that with HTTP 400.
    let tool_call_id = task
        .waiting_tool_call_id
        .as_deref()
        .ok_or("No waiting_tool_call_id")?;
    let callback_result = task
        .callback_result
        .as_deref()
        .ok_or("No callback_result")?;
    instance.replace_tool_result(tool_call_id, callback_result);

    let context = build_context(task);
    let trace_id = format!("cluster-resume-{}", &task.task_id);

    if let Some(ref obs) = cluster_observer {
        obs.set_task_context(task.task_id.clone(), task.source.node_id.clone());
        obs.emit_conversation_start(
            &trace_id,
            "cluster",
            &task.task_id,
            &task.source.node_id,
            "(resume)",
        );
    }
    let events = agent_loop
        .resume_execution(&instance, &context, &trace_id)
        .await;
    if let Some(ref obs) = cluster_observer {
        let final_msg = extract_final_message(&events);
        let rounds = count_llm_rounds(&events);
        obs.emit_conversation_end(&trace_id, "cluster", &task.task_id, rounds, &final_msg, false);
        obs.clear_task_context();
    }

    let conversation = instance.get_history();
    if is_async_done(&conversation) {
        let conversation_json = serde_json::to_value(&conversation)
            .map_err(|e| format!("Failed to serialize conversation: {}", e))?;
        let (child_task_id, new_tool_call_id) =
            extract_async_info(&conversation).ok_or("Failed to extract async info")?;

        tracing::info!(
            task_id = %task.task_id,
            child_task_id = %child_task_id,
            "[ClusterAgent] Resumed task went async again"
        );

        nemesis_cluster::logger::log_task("exec_async", &task.task_id, &format!("child={}", child_task_id));

        task_list.save_async_state(
            &task.task_id,
            child_task_id,
            new_tool_call_id,
            conversation_json,
        );
        return Ok(());
    }

    let result = extract_final_message(&events);
    send_task_callback(rpc_client, task, "success", &result, "").await;
    task_list.complete_task(&task.task_id);
    nemesis_cluster::logger::log_task("exec_done", &task.task_id, &format!("events={}", events.len()));
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Truncate a string to at most `max_len` characters, respecting char boundaries.
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }
    match s.char_indices().nth(max_len) {
        Some((idx, _)) => {
            let mut truncated = s[..idx].to_string();
            truncated.push_str("...");
            truncated
        }
        None => s.to_string(),
    }
}

/// Build a RequestContext for a cluster task.
fn build_context(task: &nemesis_cluster::cluster_task::ClusterTask) -> RequestContext {
    RequestContext::new(
        "cluster",
        &format!("{}:{}", task.source.node_id, task.task_id),
        &task.source.node_id,
        &task.source.session_key,
    )
}

/// Check if the last execution ended in an async cluster_rpc by inspecting
/// the conversation history for the `__CLUSTER_ASYNC__` marker.
///
/// Prior implementation matched the user-facing message text
/// ("已发送请求到远程节点"), which coupled detection to a specific wording.
/// After Plan C changed that template to "老爷，去找 X 帮个忙了，稍等哈~",
/// the detection broke. We replaced it with a structured check on the
/// tool_result marker — the marker is the load-bearing signal and is
/// independent of how the message is phrased for the user.
fn is_async_done(conversation: &[nemesis_agent::types::ConversationTurn]) -> bool {
    conversation.iter().rev().any(|t| {
        t.role == "tool" && t.content.contains("__CLUSTER_ASYNC__")
    })
}

/// Extract child_task_id and tool_call_id from the conversation history.
///
/// Looks for the tool result containing `__CLUSTER_ASYNC__` JSON marker
/// (written by run_llm_loop when __ASYNC__ is detected) and the
/// preceding assistant turn's tool_calls[0].id.
///
/// Falls back to text-based "Task ID: " parsing for older format compatibility.
fn extract_async_info(
    conversation: &[nemesis_agent::types::ConversationTurn],
) -> Option<(String, String)> {
    let mut child_task_id = None;
    let mut tool_call_id = None;

    // Scan conversation in reverse to find the async markers.
    for (i, turn) in conversation.iter().enumerate().rev() {
        if turn.role == "tool" {
            // Try structured JSON marker first.
            if let Some(marker_start) = turn.content.find("__CLUSTER_ASYNC__") {
                let json_str = &turn.content[marker_start + "__CLUSTER_ASYNC__".len()..];
                if let Ok(info) = serde_json::from_str::<serde_json::Value>(json_str) {
                    child_task_id = info.get("task_id").and_then(|v| v.as_str()).map(String::from);
                }
            }

            // Fallback: text-based "Task ID: " parsing.
            if child_task_id.is_none() && turn.content.contains("Task ID:") {
                if let Some(pos) = turn.content.rfind("Task ID: ") {
                    let rest = &turn.content[pos + "Task ID: ".len()..];
                    child_task_id = rest.split_whitespace().next().map(String::from);
                }
            }

            if child_task_id.is_some() {
                // Look at the preceding assistant turn for the tool_call_id.
                if i > 0 {
                    if let Some(prev) = conversation.get(i - 1) {
                        if prev.role == "assistant" {
                            if let Some(tc) = prev.tool_calls.first() {
                                tool_call_id = Some(tc.id.clone());
                            }
                        }
                    }
                }
                break;
            }
        }
    }

    match (child_task_id, tool_call_id) {
        (Some(ct), Some(tc)) => Some((ct, tc)),
        _ => None,
    }
}

/// Extract the final text message from agent events.
fn extract_final_message(events: &[AgentEvent]) -> String {
    events
        .iter()
        .rev()
        .find_map(|e| match e {
            AgentEvent::Done(msg) => Some(msg.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

/// Count LLM rounds from agent events (mirrors main agent's formula in loop.rs).
fn count_llm_rounds(events: &[AgentEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolCall(_)))
        .count()
        + 1
}

/// Send a callback for a completed task.
async fn send_task_callback(
    rpc_client: Option<&RpcClient>,
    task: &nemesis_cluster::cluster_task::ClusterTask,
    status: &str,
    response: &str,
    error: &str,
) {
    tracing::info!(
        task_id = %task.task_id,
        status = %status,
        target_node = %task.source.node_id,
        "[ClusterAgent] Sending callback"
    );

    send_callback(
        rpc_client,
        &task.source.node_id,
        &task.task_id,
        status,
        response,
        error,
    )
    .await;
}

/// Handle task execution error: mark as failed and send error callback.
async fn handle_task_error(
    task_list: &ClusterTaskList,
    rpc_client: Option<&RpcClient>,
    task: &nemesis_cluster::cluster_task::ClusterTask,
    error_msg: &str,
) {
    let error_preview = truncate_str(error_msg, 200);
    nemesis_cluster::logger::log_task("exec_failed", &task.task_id, &error_preview);
    task_list.update_status(&task.task_id, TaskStatus::Failed);
    send_task_callback(rpc_client, task, "error", "", error_msg).await;
    task_list.complete_task(&task.task_id);
}

#[cfg(test)]
mod tests;

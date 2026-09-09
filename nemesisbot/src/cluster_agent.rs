//! Cluster agent event loop.
//!
//! Processes cluster tasks from the work queue using a full AgentLoop.
//! Supports new tasks (run_with_trace) and resumed tasks (resume_execution
//! after async callback). Detects __ASYNC__ results for multi-hop chain calls.
//!
//! Swarm M3（G4 worker 被动响应）：第三 select 臂消费看板讨论唤醒事件
//! （[`nemesis_types::cluster::DiscussionEvent`]）——任务间隙跑一轮轻量
//! agent，[SILENT] 判定沉默，非静默回帖 master。讨论是分钟级不是秒级，
//! 忙碌时排队天然成立（诚实定位「任务间隙参与讨论」）。

use std::sync::Arc;

use nemesis_agent::context::RequestContext;
use nemesis_agent::instance::AgentInstance;
use nemesis_agent::r#loop::AgentLoop;
use nemesis_agent::types::AgentConfig;
use nemesis_agent::types::AgentEvent;
use nemesis_cluster::cluster_task::{ClusterTaskList, ClusterWorkQueue, TaskStatus};
use nemesis_cluster::envelope::{self, Envelope, EnvelopeResponse};
use nemesis_cluster::rpc::client::RpcClient;
use nemesis_cluster::rpc::peer_chat_handler::{send_callback_or_persist, TaskResultPersister};
use nemesis_cluster::rpc_types::{ActionType, RPCRequest};
use nemesis_types::cluster::DiscussionEvent;

use crate::cluster_request_logger_observer::ClusterRequestLoggerObserver;

// ---------------------------------------------------------------------------
// DiscussionInbox — 讨论事件入站箱（worker 生产者 → cluster agent loop）
// ---------------------------------------------------------------------------

/// 讨论事件入站箱：`nb_bus` handler / board.sync 补拉任务从这边投递，
/// cluster agent loop 的第三 select 臂从这边消费。
///
/// 为什么不直接传 Sender：cluster stop/start 周期会重新 spawn agent loop，
/// mpsc Receiver 无法二次取得；Inbox 里保存「当前活跃 loop 的 Sender」，
/// 每次 start 换入新的。stop 之后旧 Sender 随 loop 退出自然失联——send
/// 诚实失败（事件丢弃，board.sync 兜底补拉），不缓存不过期。
pub struct DiscussionInbox {
    tx: std::sync::Mutex<
        Option<tokio::sync::mpsc::UnboundedSender<DiscussionEvent>>,
    >,
}

impl Default for DiscussionInbox {
    fn default() -> Self {
        Self::new()
    }
}

impl DiscussionInbox {
    pub fn new() -> Self {
        Self {
            tx: std::sync::Mutex::new(None),
        }
    }

    /// 换入当前活跃 loop 的 sender（每次 adapter start 调用；旧 sender 随之丢弃）。
    pub fn set_sender(&self, tx: tokio::sync::mpsc::UnboundedSender<DiscussionEvent>) {
        *self
            .tx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(tx);
    }

    /// 投递事件。无活跃 loop（cluster 停了）/ 接收端已关闭（loop 刚退出）
    /// → Err，调用方记日志不重试（board.sync 是兜底）。
    // 消费方是 board_bus 的 worker handler（all(board, cluster) 门控）——
    // cluster-only 编译形态下无调用点，属预期裁剪而非死代码。
    #[cfg_attr(not(feature = "board"), allow(dead_code))]
    pub fn send(&self, event: DiscussionEvent) -> Result<(), String> {
        let guard = self
            .tx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match &*guard {
            Some(tx) => tx
                .send(event)
                .map_err(|_| "discussion channel closed (loop exited)".to_string()),
            None => Err("no active cluster agent loop (cluster stopped)".to_string()),
        }
    }
}

/// Run the cluster agent event loop.
///
/// This is the main entry point for the cluster agent. It loops forever,
/// taking tasks from the work queue and processing them one at a time
/// (讨论唤醒事件同队列串行——见模块注释).
///
/// **`config` vs `agent_loop`'s config**: these serve different purposes.
/// - `agent_loop`'s config: controls the LLM loop behavior (max_turns, provider, model).
/// - `config` parameter: used to create each `AgentInstance`, controlling per-task identity
///   (system_prompt, etc.). Currently system_prompt is None (placeholder), but will be
///   customized per task when "identity switching" is implemented (e.g., different prompts
///   for different source nodes).
#[allow(clippy::too_many_arguments)]
pub async fn cluster_agent_loop(
    agent_loop: Arc<AgentLoop>,
    config: AgentConfig,
    work_queue: Arc<ClusterWorkQueue>,
    task_list: Arc<ClusterTaskList>,
    rpc_client: Option<Arc<RpcClient>>,
    cluster_observer: Option<Arc<ClusterRequestLoggerObserver>>,
    // G1 收口（2026-09-08）：回调失败 → persister.set_result 落盘真实结果
    // （A 端恢复轮询可查）；回调成功 → persister.delete 清理占位。生产装配
    // 恒为 Some（gateway 传入与 peer_chat_handler 同一份 adapter）。
    result_persister: Option<Arc<dyn TaskResultPersister>>,
    // Swarm M3（G4）：看板讨论唤醒通道。None = 本节点不参与讨论（master
    // 本尊走主持人裁决 / board 裁剪 / cluster 未启用）。通道关闭（stop/start
    // 周期）时置 None 禁用本臂，防 recv() 返回 None 的自旋。
    mut discussion_rx: Option<tokio::sync::mpsc::UnboundedReceiver<DiscussionEvent>>,
    // 本节点 id（回复 board.comment.post 的 sender；来自 cluster.node_id()）。
    self_node_id: String,
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
            event = async {
                match discussion_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    // 无通道：永久挂起（select 分支即被有效禁用）。
                    None => std::future::pending().await,
                }
            } => {
                match event {
                    Some(ev) => {
                        handle_discussion(
                            &agent_loop,
                            &config,
                            rpc_client.as_deref(),
                            cluster_observer.as_deref(),
                            &self_node_id,
                            &ev,
                        )
                        .await;
                    }
                    None => {
                        // 通道关闭（cluster stop/start 换了新管道）——禁用本臂。
                        discussion_rx = None;
                    }
                }
                continue;
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

        // 取消检查（W2 P4 per-task cancel）：排队期间被 task_cancel 下行取消
        // 的任务直接报错回收，不再执行（cancel_task 只能标终态，回收靠出队）。
        if task.status == TaskStatus::Cancelled {
            tracing::info!(
                task_id = %task_id,
                "[ClusterAgent] Dequeued task was cancelled, dropping"
            );
            handle_task_error(
                &task_list,
                rpc_client.as_deref(),
                result_persister.as_deref(),
                &task,
                "cancelled",
            )
            .await;
            continue;
        }

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
                result_persister.as_deref(),
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
                    handle_task_error(
                        &task_list,
                        rpc_client.as_deref(),
                        result_persister.as_deref(),
                        &task,
                        &e.to_string(),
                    )
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
                result_persister.as_deref(),
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
                    handle_task_error(
                        &task_list,
                        rpc_client.as_deref(),
                        result_persister.as_deref(),
                        &task,
                        &e.to_string(),
                    )
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
    result_persister: Option<&dyn TaskResultPersister>,
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

    // Restore history from SessionStore (same pattern as main AgentLoop).
    // Without this, every peer_chat starts with empty context, so "give me the
    // code from last time" can't be answered — Alex would have no memory of
    // the previous task and would rewrite from scratch.
    let restored = restore_session_history(agent_loop, &instance, &task.source.session_key);
    if restored > 0 {
        tracing::info!(
            task_id = %task.task_id,
            session_key = %task.source.session_key,
            restored_msgs = restored,
            "[ClusterAgent] Restored history from SessionStore"
        );
    }

    let token = tokio_util::sync::CancellationToken::new();
    // W2 P4 per-task cancel：执行窗口内注册令牌，cancel_task 下行命中 Running
    // 时 token.cancel() 打断 LLM 循环；run 结束必须注销（防 DashMap 泄漏）。
    task_list.register_cancel_token(&task.task_id, token.clone());
    if let Some(obs) = cluster_observer {
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
        .run_with_trace(
            &instance,
            &task.content,
            &context,
            &trace_id,
            false,
            &token,
            None,
            &[],
        )
        .await;
    task_list.unregister_cancel_token(&task.task_id);
    if let Some(obs) = cluster_observer {
        let final_msg = extract_final_message(&events);
        let rounds = count_llm_rounds(&events);
        obs.emit_conversation_end(
            &trace_id,
            "cluster",
            &task.task_id,
            rounds,
            &final_msg,
            false,
        );
        obs.clear_task_context();
    }

    // 取消检查（W2 P4）：cancel_task 在执行窗口命中 → 中断后不走正常完成/
    // async 路径，发 error 回调（"cancelled"）回收任务。A 侧 dispatch 已终态，
    // 其写回会按 state != dispatched 幂等跳过。
    if token.is_cancelled() {
        tracing::info!(
            task_id = %task.task_id,
            "[ClusterAgent] Task was cancelled during execution"
        );
        nemesis_cluster::logger::log_task("exec_cancelled", &task.task_id, "");
        send_task_callback(rpc_client, result_persister, task, "error", "", "cancelled").await;
        task_list.complete_task(&task.task_id);
        return Ok(());
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

        nemesis_cluster::logger::log_task(
            "exec_async",
            &task.task_id,
            &format!("child={}", child_task_id),
        );

        task_list.save_async_state(
            &task.task_id,
            child_task_id,
            tool_call_id,
            conversation_json,
        );
        return Ok(());
    }

    let result = extract_final_message(&events);
    // Persist the full instance history + cache to SessionStore before sending
    // the callback. Async-path tasks skip this; they'll be persisted by
    // resume_task when the callback comes back and the task actually completes.
    persist_session_history(agent_loop, &instance, &task.source.session_key);
    send_task_callback(rpc_client, result_persister, task, "success", &result, "").await;
    task_list.complete_task(&task.task_id);
    nemesis_cluster::logger::log_task(
        "exec_done",
        &task.task_id,
        &format!("events={}", events.len()),
    );
    Ok(())
}

/// Resume a task after receiving a callback from a remote node.
async fn resume_task(
    agent_loop: &AgentLoop,
    config: &AgentConfig,
    task_list: &ClusterTaskList,
    rpc_client: Option<&RpcClient>,
    cluster_observer: Option<&ClusterRequestLoggerObserver>,
    result_persister: Option<&dyn TaskResultPersister>,
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

    // W2 P4 per-task cancel：resume 路径同样注册令牌（token 透传给
    // resume_execution_with_token，cancel_task 可中断续行中的 LLM 循环）。
    let token = tokio_util::sync::CancellationToken::new();
    task_list.register_cancel_token(&task.task_id, token.clone());
    if let Some(obs) = cluster_observer {
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
        .resume_execution_with_token(&instance, &context, &trace_id, &token)
        .await;
    task_list.unregister_cancel_token(&task.task_id);
    if let Some(obs) = cluster_observer {
        let final_msg = extract_final_message(&events);
        let rounds = count_llm_rounds(&events);
        obs.emit_conversation_end(
            &trace_id,
            "cluster",
            &task.task_id,
            rounds,
            &final_msg,
            false,
        );
        obs.clear_task_context();
    }

    // 取消检查（W2 P4）：续行被中断 → error 回调回收，不走 async/完成路径。
    if token.is_cancelled() {
        tracing::info!(
            task_id = %task.task_id,
            "[ClusterAgent] Resumed task was cancelled during execution"
        );
        nemesis_cluster::logger::log_task("exec_cancelled", &task.task_id, "");
        send_task_callback(rpc_client, result_persister, task, "error", "", "cancelled").await;
        task_list.complete_task(&task.task_id);
        return Ok(());
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

        nemesis_cluster::logger::log_task(
            "exec_async",
            &task.task_id,
            &format!("child={}", child_task_id),
        );

        task_list.save_async_state(
            &task.task_id,
            child_task_id,
            new_tool_call_id,
            conversation_json,
        );
        return Ok(());
    }

    let result = extract_final_message(&events);
    // Persist the full instance history + cache. The original user request and
    // the resumed turn's tool result / final response are all in the instance
    // history already, so no separate content args are needed.
    persist_session_history(agent_loop, &instance, &task.source.session_key);
    send_task_callback(rpc_client, result_persister, task, "success", &result, "").await;
    task_list.complete_task(&task.task_id);
    nemesis_cluster::logger::log_task(
        "exec_done",
        &task.task_id,
        &format!("events={}", events.len()),
    );
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
///
/// `chat_id` 用稳定的 `session_key`（而不是 `node_id:task_id`），让同一对端节点的多次
/// peer_chat 共享 chat_id。这样下游工具（cluster_rpc 多跳传播、cron、spawn）拿到的
/// chat_id 不会每次变化，避免历史断裂和路由失效。
fn build_context(task: &nemesis_cluster::cluster_task::ClusterTask) -> RequestContext {
    RequestContext::new(
        "cluster",
        &task.source.session_key,
        &task.source.node_id,
        &task.source.session_key,
    )
}

/// Restore conversation history from SessionStore into the given AgentInstance.
///
/// Mirrors `AgentLoop::get_or_create_instance`:
/// - Reads `StoredSession` by `session_key`
/// - If non-empty, converts messages to `ConversationTurn` and calls `set_history`
/// - Restores the summary cache (text + covers_up_to); legacy files (field
///   absent) map covers_up_to so all loaded messages are sent verbatim
///
/// Failures are silent (no panic) — cluster peer_chat must degrade gracefully
/// if SessionStore is unavailable or corrupt. Returns the number of restored
/// messages for logging.
fn restore_session_history(
    agent_loop: &AgentLoop,
    instance: &AgentInstance,
    session_key: &str,
) -> usize {
    let store = match agent_loop.session_store() {
        Some(s) => s,
        None => return 0,
    };
    let stored = store.get_or_create(session_key);
    if stored.messages.is_empty() {
        return 0;
    }
    let covers = store.get_summary_covers_up_to(session_key);
    let history: Vec<nemesis_agent::types::ConversationTurn> =
        stored.messages.into_iter().map(|m| m.into()).collect();
    let count = history.len();
    instance.set_history(history);
    if !stored.summary.is_empty() {
        let history = instance.get_history();
        if !history.is_empty() {
            let c = match covers {
                Some(c) => c.clamp(1, history.len()),
                None => history
                    .iter()
                    .take_while(|t| t.role == "system")
                    .count()
                    .max(1),
            };
            instance.set_summary_cache(Some(nemesis_agent::instance::SummaryCache {
                covers_up_to: c,
                text: stored.summary,
            }));
        }
    }
    count
}

/// Persist the full instance history + summary cache for a session to
/// SessionStore.
///
/// Mirrors `AgentLoop::run_agent_loop_internal`'s save path (post-refactor):
/// the store holds the FULL instance history (not just a user/assistant log)
/// so the summary cache's covers_up_to index stays coherent across turns.
///
/// Used by both `execute_new_task` and `resume_task` on their sync-completion paths.
/// Async paths (task went async again) skip this — the next resume will write the
/// final response when it eventually completes. The user/assistant content is
/// already in the instance history (added by run_with_trace / the resume
/// snapshot), so no separate content args are needed.
fn persist_session_history(agent_loop: &AgentLoop, instance: &AgentInstance, session_key: &str) {
    let store = match agent_loop.session_store() {
        Some(s) => s,
        None => return,
    };
    store.get_or_create(session_key);
    // Set the cache BEFORE set_history: trim_to_limit reads covers_up_to to
    // decide which oldest messages are safe to drop and adjusts it downward;
    // setting it first keeps the store coherent (mirrors AgentLoop's save path).
    match instance.get_summary_cache() {
        Some(cache) => {
            store.set_summary(session_key, &cache.text);
            store.set_summary_covers_up_to(session_key, Some(cache.covers_up_to));
        }
        None => {
            store.set_summary(session_key, "");
            store.set_summary_covers_up_to(session_key, None);
        }
    }
    let stored: Vec<nemesis_agent::session::StoredMessage> = instance
        .get_history()
        .iter()
        .map(nemesis_agent::session::StoredMessage::from)
        .collect();
    store.set_history(session_key, stored);
    if let Err(e) = store.save(session_key) {
        tracing::warn!(
            session_key = %session_key,
            error = %e,
            "[ClusterAgent] Failed to persist session history"
        );
    }
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
    conversation
        .iter()
        .rev()
        .any(|t| t.role == "tool" && t.content.contains("__CLUSTER_ASYNC__"))
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
                    child_task_id = info
                        .get("task_id")
                        .and_then(|v| v.as_str())
                        .map(String::from);
                }
            }

            // Fallback: text-based "Task ID: " parsing.
            if child_task_id.is_none()
                && turn.content.contains("Task ID:")
                && let Some(pos) = turn.content.rfind("Task ID: ")
            {
                let rest = &turn.content[pos + "Task ID: ".len()..];
                child_task_id = rest.split_whitespace().next().map(String::from);
            }

            if child_task_id.is_some() {
                // Look at the preceding assistant turn for the tool_call_id.
                if i > 0
                    && let Some(prev) = conversation.get(i - 1)
                    && prev.role == "assistant"
                    && let Some(tc) = prev.tool_calls.first()
                {
                    tool_call_id = Some(tc.id.clone());
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
///
/// G1 收口（2026-09-08）：走 [`send_callback_or_persist`] 单一真相源 ——
/// 回调成功 → `persister.delete(task_id)` 清理 set_running 占位；回调失败
/// （对端宕机等）→ `persister.set_result` 落盘真实结果，A 端重启后的恢复
/// 轮询（query_task_result）才能查到。此前直连裸 send_callback，真实结果
/// 从不落盘 → G5 恢复链在生产 work-queue 路径下失效（真机 R1a 发现）。
async fn send_task_callback(
    rpc_client: Option<&RpcClient>,
    result_persister: Option<&dyn TaskResultPersister>,
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

    send_callback_or_persist(
        rpc_client,
        result_persister,
        &None,
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
    result_persister: Option<&dyn TaskResultPersister>,
    task: &nemesis_cluster::cluster_task::ClusterTask,
    error_msg: &str,
) {
    let error_preview = truncate_str(error_msg, 200);
    nemesis_cluster::logger::log_task("exec_failed", &task.task_id, &error_preview);
    task_list.update_status(&task.task_id, TaskStatus::Failed);
    send_task_callback(rpc_client, result_persister, task, "error", "", error_msg).await;
    task_list.complete_task(&task.task_id);
}

// ---------------------------------------------------------------------------
// Board discussion（Swarm M3 G4：worker 被动响应）
// ---------------------------------------------------------------------------

/// 消费一条讨论唤醒事件：线程上下文 + 节点身份 + 唤醒原因 → 轻量
/// AgentInstance 跑一轮 → [SILENT] 判定沉默 / 非静默回帖 master。
///
/// estop 覆盖：跑在与 work queue 同一个 agent loop 上（build 阶段已
/// set_estop）——estop 冻结时 LLM 调用被拒/中断，最终回复为空，静默丢弃。
async fn handle_discussion(
    agent_loop: &AgentLoop,
    config: &AgentConfig,
    rpc_client: Option<&RpcClient>,
    cluster_observer: Option<&ClusterRequestLoggerObserver>,
    self_node_id: &str,
    event: &DiscussionEvent,
) {
    let thread_key = format!("{}:{}", event.thread_kind, event.thread_id);
    tracing::info!(
        target: "board_bus",
        thread = %thread_key,
        seq = event.seq,
        wake = %event.event,
        from = %event.from_node,
        "[ClusterAgent] Discussion wake, running one agent round"
    );

    // Per-round AgentInstance：与 work queue 任务同款（吃集群身份 system
    // prompt）。会话不落 SessionStore——讨论状态真相源在 master 台账，每轮
    // 上下文由 wake 包随带（幂等重放安全：同 seq 不重复入队）。
    let instance = AgentInstance::new(config.clone());
    let prompt = build_discussion_prompt(self_node_id, event);
    let session_key = format!("board:discussion:{thread_key}");
    let trace_id = format!("board-disc-{}-{}", event.thread_id, event.seq);
    let context = RequestContext::new("board", &session_key, &event.from_node, &session_key);
    let token = tokio_util::sync::CancellationToken::new();
    // Swarm G13：讨论轮同样落 cluster_logs 4 件套。讨论没有 cluster task，
    // 观察者需要合成 task 上下文才能构造路径（目录按唤醒来源节点归组，
    // task_id 带线程与 seq 可检索）；并补 start/end——run_with_trace 只发
    // LlmRequest/LlmResponse，缺 start 则全部事件被静默丢弃（与 worker
    // 任务同一处置）。
    if let Some(obs) = cluster_observer {
        obs.set_task_context(
            format!(
                "discussion-{}-{}-seq{}",
                event.thread_kind, event.thread_id, event.seq
            ),
            event.from_node.clone(),
        );
        obs.emit_conversation_start(&trace_id, "board", &thread_key, &event.from_node, &prompt);
    }
    let events = agent_loop
        .run_with_trace(
            &instance, &prompt, &context, &trace_id, false, &token, None, &[],
        )
        .await;

    // 先闭合观察者 trace（所有退出路径都要收尾，防 active 表泄漏），
    // 再走取消/沉默判定。
    let reply = extract_final_message(&events);
    if let Some(obs) = cluster_observer {
        let rounds = count_llm_rounds(&events);
        obs.emit_conversation_end(&trace_id, "board", &thread_key, rounds, &reply, false);
        obs.clear_task_context();
    }

    if token.is_cancelled() {
        tracing::debug!(target: "board_bus", thread = %thread_key,
            "[ClusterAgent] Discussion round cancelled, dropping");
        return;
    }

    let trimmed = reply.trim();
    if trimmed.is_empty() {
        tracing::debug!(target: "board_bus", thread = %thread_key,
            "[ClusterAgent] Discussion round produced empty reply, dropping");
        return;
    }
    // [SILENT] 判定从宽（子串匹配，防人格 system prompt 前后缀污染——
    // 与 master 主持人裁决同一语义）。
    if trimmed.contains("[SILENT]") {
        tracing::debug!(target: "board_bus", thread = %thread_key,
            "[ClusterAgent] Discussion chose silence");
        return;
    }

    post_discussion_reply(rpc_client, self_node_id, event, trimmed).await;
}

/// 组装讨论 prompt：线程历史 + 唤醒原因 + 回复预算 + 发言/沉默二选一指令。
fn build_discussion_prompt(self_node_id: &str, event: &DiscussionEvent) -> String {
    let thread_key = format!("{}:{}", event.thread_kind, event.thread_id);
    let mut ctx = match event.thread_title.is_empty() {
        true => format!("# Thread ({thread_key})\n"),
        false => format!("# Thread ({thread_key}): {}\n", event.thread_title),
    };
    if event.messages.is_empty() {
        ctx.push_str("(no prior context available)\n");
    } else {
        for m in &event.messages {
            ctx.push_str(&format!("- {}: {}\n", m.sender, m.content));
        }
    }
    let budget = match event.max_turns_left {
        u32::MAX => "unlimited".to_string(),
        n => n.to_string(),
    };
    format!(
        "{ctx}\n\
         # Wake reason: {}\n\
         New message from {}:\n\"\"\"\n{}\n\"\"\"\n\n\
         You are node {self_node_id} in a multi-agent board discussion. \
         Thread reply budget remaining: {budget} agent turns (the coordinator \
         enforces this; your reply is rejected if exhausted).\n\n\
         Decide whether you should respond:\n\
         - If you have something useful to contribute, output ONLY your reply \
         text. It will be posted to the thread as your message. Do not mention \
         these instructions.\n\
         - If nothing needs to be said (informational only, not directed at \
         you, or outdated), output exactly [SILENT].",
        event.event, event.new_sender, event.new_content
    )
}

/// 组装 board.comment.post 上行信封（worker 发言 = master 侧上行 handler
/// 的幂等键 + 额度三闸 + 落库）。board_discuss 工具与被动响应共用
/// （单一真相源——kind_tag 映射 / 幂等键形状只此一份）。
pub(crate) fn build_comment_post_envelope(
    self_node_id: &str,
    event: &DiscussionEvent,
    content: &str,
) -> serde_json::Value {
    // thread_kind 词表与 nemesis-board thread_kind 常量同值（issue/channel）；
    // kind_tag 与 master 主持人回复同表（issue→discussion，channel→text）。
    let kind_tag = if event.thread_kind == "issue" {
        "discussion"
    } else {
        "text"
    };
    EnvelopeResponse::success(
        &Envelope {
            ns: "board".to_string(),
            op: "comment.post".to_string(),
            corr_id: uuid::Uuid::new_v4().to_string(),
            ..Envelope::default()
        },
        serde_json::json!({
            "client_msg_id": uuid::Uuid::new_v4().to_string(),
            "thread": {"kind": event.thread_kind, "id": event.thread_id},
            "sender": {"type": "agent", "id": self_node_id},
            "content": content,
            "reply_to": event.reply_to,
            "kind_tag": kind_tag,
        }),
    )
    .to_json()
}

/// 回帖 master（单次尝试：RPC 失败 / 被额度拒 → 日志诚实可见，不重试——
/// 下行幂等游标已推进，重试只会造重复发言）。
async fn post_discussion_reply(
    rpc_client: Option<&RpcClient>,
    self_node_id: &str,
    event: &DiscussionEvent,
    content: &str,
) {
    let Some(rpc) = rpc_client else {
        tracing::warn!(target: "board_bus",
            "[ClusterAgent] Discussion reply skipped: no rpc client");
        return;
    };
    let payload = build_comment_post_envelope(self_node_id, event, content);
    let thread_key = format!("{}:{}", event.thread_kind, event.thread_id);
    match send_nb_bus(rpc, self_node_id, &event.from_node, payload).await {
        Ok(body) => {
            // 信封响应：ok=false 时错误码诚实可见（quota_exhausted 等）。
            let ok = body
                .get("ok")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if ok {
                tracing::debug!(target: "board_bus", thread = %thread_key,
                    "[ClusterAgent] Discussion reply posted");
            } else {
                let code = body
                    .pointer("/error/code")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let msg = body
                    .pointer("/error/message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                tracing::warn!(target: "board_bus", code, message = %msg,
                    thread = %thread_key,
                    "[ClusterAgent] Discussion reply rejected by master");
            }
        }
        Err(e) => {
            tracing::warn!(target: "board_bus", error = %e, thread = %thread_key,
                "[ClusterAgent] Discussion reply delivery failed");
        }
    }
}

/// nb_bus 单次 RPC 往返（信封 payload 进、信封响应 JSON 出）。
/// 被动回帖与 board_discuss 工具共用（单一真相源——action 名 / 幂等不做 /
/// result 解空只此一份）。
pub(crate) async fn send_nb_bus(
    rpc: &RpcClient,
    source: &str,
    target: &str,
    payload: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let request = RPCRequest {
        id: uuid::Uuid::new_v4().to_string(),
        action: ActionType::Custom(envelope::NB_BUS_ACTION.to_string()),
        payload,
        source: source.to_string(),
        target: Some(target.to_string()),
    };
    let resp = rpc
        .call(target, request)
        .await
        .map_err(|e| format!("nb_bus rpc to {target}: {e}"))?;
    Ok(resp.result.unwrap_or(serde_json::Value::Null))
}

#[cfg(test)]
mod tests;

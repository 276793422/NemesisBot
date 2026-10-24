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
use std::time::Duration;

use nemesis_agent::context::RequestContext;
use nemesis_agent::instance::AgentInstance;
use nemesis_agent::r#loop::AgentLoop;
use nemesis_agent::turn_guard::ESCALATION_MARKER;
use nemesis_agent::types::AgentConfig;
use nemesis_agent::types::AgentEvent;
use nemesis_cluster::cluster_task::{ClusterTaskList, ClusterWorkQueue, TaskStatus};
use nemesis_cluster::envelope::{self, Envelope, EnvelopeResponse};
use nemesis_cluster::rpc::client::RpcClient;
use nemesis_cluster::rpc::peer_chat_handler::{TaskResultPersister, send_callback_or_persist};
use nemesis_cluster::rpc_types::{ActionType, RPCRequest};
use nemesis_types::cluster::DiscussionEvent;

use crate::cluster_request_logger_observer::ClusterRequestLoggerObserver;
use crate::delivery_gate::{capture_task_delivery, delivery_base_tree};

/// turn_guard 退化三判 GiveUp 的兜底告知文案（`turn_guard::check_final_answer`
/// 在空答复重试耗尽时构造、以 Done 终局）。nemesis-agent 不导出该常量（crate
/// 边界），此处按值对齐；单真相源仍为 turn_guard——改文案须同步此处，否则
/// 空交付改判臂失配（wave_d 空交付测试会红，即回归锚）。
const DEGENERATE_GIVEUP_NOTICE: &str =
    "（模型多次未给出有效答复，已停止重试。请重试或换一种问法。）";

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
    tx: std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedSender<DiscussionEvent>>>,
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
/// - `config` parameter: 稳定前缀模板——system_prompt 由 agent_factory 装配
///   （IDENTITY/SOUL/EXPERTISE/worker 契约/Workspace 行，跨任务字节不变）；
///   每个任务经 [`task_system_prompt`] 在其上追加任务职能后缀
///   （集群专业职能框架 M3）后构造 per-task `AgentInstance`。
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
                &self_node_id,
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

        // D0b（goal P2）：出队开跑上报——master 把 dispatch 标 running
        // （queued vs executing 分界；重平衡只挪 queued 单）。fire-and-forget：
        // 失败只 debug（master 保守方向=不挪已标记的单，安全侧）。
        notify_task_started(rpc_client.as_deref(), &task, &self_node_id).await;

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
                &self_node_id,
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
                        &self_node_id,
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
                &self_node_id,
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
                        &self_node_id,
                        &e.to_string(),
                    )
                    .await;
                }
            }
        }
    }
}

/// Execute a new task using run_with_trace().
/// D0b（goal P2）：出队开跑上报（nb_bus ns="task" op="started"，目标=
/// 任务来源 master）。master 据此把 dispatch 标 running——重平衡只挪
/// queued 单。fire-and-forget：RPC 失败只 debug（重平衡保守方向=不挪）。
///
/// claim 竞速重试（T43 实证）：master 的派发行在 submit ACK 之后才落库
/// （board.rs try_claim_dispatch 在 submit_peer_chat 之后），空闲 worker
/// 取即跑，started 上报可跑赢插行 → master 回 UNAVAILABLE。此时任务正在
/// 执行，退避 500ms 重报（上限 2 次重试；正常首试即中，最坏 +1s）；其余
/// 失败（transport / VALIDATION）维持 fire-and-forget 语义不重试。
async fn notify_task_started(
    rpc_client: Option<&RpcClient>,
    task: &nemesis_cluster::cluster_task::ClusterTask,
    self_node_id: &str,
) {
    let Some(rpc) = rpc_client else {
        return;
    };
    for attempt in 0..=2u32 {
        let payload = serde_json::json!({
            "v": nemesis_cluster::envelope::ENVELOPE_VERSION,
            "ns": "task",
            "op": "started",
            "corr_id": uuid::Uuid::new_v4().to_string(),
            "body": { "task_id": task.task_id },
        });
        let request = RPCRequest {
            id: uuid::Uuid::new_v4().to_string(),
            action: ActionType::Custom(nemesis_cluster::envelope::NB_BUS_ACTION.to_string()),
            payload,
            source: self_node_id.to_string(),
            target: Some(task.source.node_id.clone()),
        };
        match rpc.call(&task.source.node_id, request).await {
            Ok(resp) if started_reply_retryable(&resp) => {
                tracing::debug!(task_id = %task.task_id, attempt,
                    "[ClusterAgent] task.started 撞上 claim 竞速（UNAVAILABLE），退避重报");
            }
            Ok(_) => return,
            Err(e) => {
                tracing::debug!(task_id = %task.task_id, error = %e,
                    "[ClusterAgent] task.started notify skipped");
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    tracing::debug!(task_id = %task.task_id,
        "[ClusterAgent] task.started 重试耗尽（claim 竞速未解，放弃上报）");
}

/// master 对 started 上报的 UNAVAILABLE 回复（派发行未落库，可重试）。
fn started_reply_retryable(resp: &nemesis_cluster::rpc_types::RPCResponse) -> bool {
    resp.result.as_ref().map(|v| {
        v.get("ok").and_then(|b| b.as_bool()) == Some(false)
            && v.pointer("/error/code").and_then(|c| c.as_str())
                == Some(nemesis_cluster::envelope::error_code::UNAVAILABLE)
    }) == Some(true)
}

/// P8（集群无人值守硬化 A2）：会话 token 预算拒接判定（D3 重派入口）。
/// `agents.defaults.session_token_budget`（0 = 关，默认）读法经 loop 侧
/// `current_session_token_budget`（config.json 单一真相源 fresh-read）；
/// 用量取 `aggregate_session_usage` absolute（口径 = 完整输入（prompt +
/// cache_creation + cache_read）+ output，与 loop 侧累计器同账本同公式）。
/// `None` = 关/无账/未超限（放行）。
fn session_budget_rejection(agent_loop: &AgentLoop, session_key: &str) -> Option<String> {
    let budget = agent_loop.current_session_token_budget();
    if budget == 0 {
        return None;
    }
    let ds = agent_loop.data_store()?;
    let agg = ds.aggregate_session_usage(session_key).ok()?;
    let used = u64::try_from(agg.input_tokens.max(0))
        .unwrap_or(u64::MAX)
        .saturating_add(u64::try_from(agg.output_tokens.max(0)).unwrap_or(u64::MAX));
    if used < budget {
        return None;
    }
    Some(nemesis_agent::token_budget::entry_reject_notice(
        used, budget,
    ))
}

async fn execute_new_task(
    agent_loop: &AgentLoop,
    config: &AgentConfig,
    task_list: &ClusterTaskList,
    rpc_client: Option<&RpcClient>,
    cluster_observer: Option<&ClusterRequestLoggerObserver>,
    result_persister: Option<&dyn TaskResultPersister>,
    task: &nemesis_cluster::cluster_task::ClusterTask,
    self_node_id: &str,
) -> Result<(), String> {
    let content_preview = truncate_str(&task.content, 200);
    nemesis_cluster::logger::log_task("exec_start", &task.task_id, &content_preview);
    let context = build_context(agent_loop, task);
    let trace_id = format!("cluster-{}", &task.task_id);
    // Per-task AgentInstance：稳定前缀（装配期 system_prompt）+ 任务职能
    // 后缀（M3）。职能文本从不跨 RPC——slug 随任务走，契约本节点本地渲染。
    let instance = AgentInstance::new(task_system_prompt(config, agent_loop, task));

    // P8（集群无人值守硬化 A2）：会话 token 预算入口拒接（NB-17 根因的正解
    // 位——不进 loop 就不烧）。重派收敛到同一会话且带全量旧历史重入，预算
    // 耗尽后放进来每轮 prefill 都是纯消耗；此处早退 error 回调（classify
    // 识别 token_budget）→ master board_review 转人工不自动重派。
    if let Some(notice) = session_budget_rejection(agent_loop, &task.source.session_key) {
        tracing::info!(
            task_id = %task.task_id,
            session_key = %task.source.session_key,
            "[ClusterAgent] session token budget exhausted; task rejected at entry"
        );
        nemesis_cluster::logger::log_task("exec_budget_blocked", &task.task_id, &notice);
        send_task_callback(
            rpc_client,
            result_persister,
            task,
            self_node_id,
            "error",
            "",
            &notice,
            None,
            None,
            None,
        )
        .await;
        task_list.complete_task(&task.task_id);
        return Ok(());
    }

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
    // E1 二期：执行前快照 session 用量（成功回调时取差值 = 本任务消耗）。
    let usage_before = snapshot_session_usage(agent_loop, &task.source.session_key);
    // P4 交付门：执行前快照基线 tree（git 形态才有；成功收口时按此 diff
    // ——先执行后快照会把任务自己的改动算进基线，diff 恒空证据失真）。
    let delivery_base = delivery_base_tree(agent_loop);
    // ②typed checks 块：任务窗口检查捕获开槽（`checks_capture` 模块文档是
    // 语义真相源）。证据窗口与交付 diff 窗口一致 = 本轮 run；async 首跳的
    // 记录随窗口丢弃，与 delivery_base 续行重照基线同语义（最终交付证据
    // = 最后一轮）。
    agent_loop.begin_checks_capture(&task.source.session_key);
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
    // ②typed checks：窗口关闭必摘槽（cancel/async/error/success 各臂共用
    // 这一份记录；只有 success 臂的交付捕获真正消费，其余臂诚实丢弃）。
    let checks = agent_loop.take_checks_capture(&task.source.session_key);
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
        send_task_callback(
            rpc_client,
            result_persister,
            task,
            self_node_id,
            "error",
            "",
            "cancelled",
            None,
            None,
            None,
        )
        .await;
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

        // NB-CODE-008：磁盘恢复面静默失效必须可观测——内存态已就绪、任务继续
        // 异步挂起，但崩溃后该任务无法恢复。
        // E1 二期多跳账（2026-10-17 根修）：async 挂起前已消耗的首跳用量随
        // usage_before 局部变量一起丢失——暂存进任务记录，resume 完成时
        // 合并上报（否则多跳任务的 A 端记账只含末跳差值）。
        let first_leg = extract_task_usage(agent_loop, &task.source.session_key, &usage_before);
        task_list.set_async_usage_stash(&task.task_id, first_leg);
        if let Err(e) = task_list.save_async_state(
            &task.task_id,
            child_task_id,
            tool_call_id,
            conversation_json,
        ) {
            tracing::error!(
                task_id = %task.task_id,
                error = %e,
                "[ClusterAgent] save_async_state disk persist failed — task continues in memory but will be lost on restart"
            );
        }
        return Ok(());
    }

    let mut result = extract_final_message(&events);
    // P1 同族（2026-09-25）：turn_guard 退化三判 GiveUp 的兜底告知不是工作
    // 交付——置空后走下方 (None, true) 空交付改判臂（error「worker 返回空
    // 结果…」），否则放弃告知会被当 success 交付给 A 端（同 P2A 升级硬停
    // 的改判理由）。文案对齐见 DEGENERATE_GIVEUP_NOTICE。
    if result.trim() == DEGENERATE_GIVEUP_NOTICE {
        result.clear();
    }
    // Persist the full instance history + cache to SessionStore before sending
    // the callback. Async-path tasks skip this; they'll be persisted by
    // resume_task when the callback comes back and the task actually completes.
    persist_session_history(agent_loop, &instance, &task.source.session_key);
    let usage = extract_task_usage(agent_loop, &task.source.session_key, &usage_before);
    // P1（2026-09-11 真机日志分析）：失败终止的 turn（校验预算耗尽 / LLM 重试
    // 耗尽 / 升级硬停，末事件为 Error）必须发 error 回调，不得包装成 success
    // ——此前 7/7 真机样本都是空交付 success，A 端重派链拿不到失败信号。
    // 成功终止但最终文本为空同样按 error 上报（无内容可交付）。
    // P2A（2026-09-12）：turn_guard 升级硬停以 Done 终止（P1 语义——文案带
    // 「已完成的工作已保存」），但交付内容只是停止告知，不是工作交付——按
    // error 上报（classify 为 escalation），A 端据此转人工而非把停止告知当
    // 交付评审后盲重派（同节点重放同一任务大概率复现同一循环）。
    let error_text = terminal_error(&events);
    let escalation_stop = error_text.is_none() && result.contains(ESCALATION_MARKER);
    let status = if escalation_stop {
        (result.clone(), "error")
    } else {
        match (&error_text, result.trim().is_empty()) {
            (Some(err), _) => (err.clone(), "error"),
            (None, true) => (
                "worker 返回空结果：turn 正常结束但没有产出任何最终回复文本".to_string(),
                "error",
            ),
            (None, false) => (String::new(), "success"),
        }
    };
    if status.1 == "error" {
        nemesis_cluster::logger::log_task("exec_failed", &task.task_id, &status.0);
    }
    // P4 交付门：成功交付才捕获分支（error/取消没有交付物可捕获）。
    // 失败 = None（交付门是增强证据面，绝不阻断回调主链路）。
    let delivery = if status.1 == "success" {
        capture_task_delivery(
            agent_loop,
            &task.task_id,
            delivery_base.as_deref(),
            self_node_id,
            checks,
        )
    } else {
        None
    };
    send_task_callback(
        rpc_client,
        result_persister,
        task,
        self_node_id,
        status.1,
        &result,
        &status.0,
        usage,
        delivery,
        context.tool_path_base.as_deref(),
    )
    .await;
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
    self_node_id: &str,
) -> Result<(), String> {
    nemesis_cluster::logger::log_task("exec_resume", &task.task_id, "");
    // Per-task AgentInstance：与 execute_new_task 同源渲染（稳定前缀 + 任务
    // 职能后缀）——续行恢复不丢职能身份（M3：ClusterTask 带着职能字段走）。
    let instance = AgentInstance::new(task_system_prompt(config, agent_loop, task));

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

    let context = build_context(agent_loop, task);
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
    // E1 二期：续行前快照（差值 = 本续行轮消耗）。旧注释「原任务用量已在
    // execute_new_task 成功回调时报过」是假的——async 挂起路径没有成功回调，
    // 首跳用量自 2026-10-17 起由 async_usage_stash 随任务记录暂存，最终
    // 回调时合并上报（此前多跳任务的首跳消耗从未回传 A 端）。
    let carried_usage = task.async_usage_stash.clone();
    let usage_before = snapshot_session_usage(agent_loop, &task.source.session_key);
    // P4 交付门：续行同样先记基线（本续行轮的改动才是本轮交付）。
    let delivery_base = delivery_base_tree(agent_loop);
    // ②typed checks 块：续行窗口同 execute_new_task（开槽→run→摘槽，
    // 证据窗口与本轮交付 diff 窗口一致）。
    agent_loop.begin_checks_capture(&task.source.session_key);
    let events = agent_loop
        .resume_execution_with_token(&instance, &context, &trace_id, &token)
        .await;
    // ②typed checks：窗口关闭必摘槽（cancel/async/error/success 各臂共用）。
    let checks = agent_loop.take_checks_capture(&task.source.session_key);
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
        send_task_callback(
            rpc_client,
            result_persister,
            task,
            self_node_id,
            "error",
            "",
            "cancelled",
            None,
            None,
            None,
        )
        .await;
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

        // 二次 async：暂存更新为 已承接首跳 + 本续行轮 的累计（逐跳累加，
        // 最终回调一次合并上报）。本轮差值取不到（未装配 data_store）时
        // 保留已有暂存，不吞账。
        let merged = match extract_task_usage(agent_loop, &task.source.session_key, &usage_before) {
            Some(leg) => Some(task_usage_merge(carried_usage.as_ref(), &leg)),
            None => carried_usage.clone(),
        };
        task_list.set_async_usage_stash(&task.task_id, merged);
        if let Err(e) = task_list.save_async_state(
            &task.task_id,
            child_task_id,
            new_tool_call_id,
            conversation_json,
        ) {
            tracing::error!(
                task_id = %task.task_id,
                error = %e,
                "[ClusterAgent] save_async_state disk persist failed (resumed task) — task continues in memory but will be lost on restart"
            );
        }
        return Ok(());
    }

    let mut result = extract_final_message(&events);
    // P1 同族（2026-09-25）：续行轮同样把 turn_guard GiveUp 兜底告知按空交付
    // 处理（与 execute_new_task 同一段注释，此处走「续行正常结束」文案臂）。
    if result.trim() == DEGENERATE_GIVEUP_NOTICE {
        result.clear();
    }
    // Persist the full instance history + cache. The original user request and
    // the resumed turn's tool result / final response are all in the instance
    // history already, so no separate content args are needed.
    persist_session_history(agent_loop, &instance, &task.source.session_key);
    // E1 二期多跳账：本续行轮差值 + async 暂存的此前各跳累计 = 全程用量。
    let usage = extract_task_usage(agent_loop, &task.source.session_key, &usage_before)
        .map(|leg| task_usage_merge(carried_usage.as_ref(), &leg));
    // P1：续行轮同样按终态判定（与 execute_new_task 同一映射，见其注释）。
    // P2A：升级硬停（Done 终止 + 稳定前缀交付文本）同样改判 error——
    // 停止告知不是工作交付（详见 execute_new_task 同段注释）。
    let error_text = terminal_error(&events);
    let escalation_stop = error_text.is_none() && result.contains(ESCALATION_MARKER);
    let status = if escalation_stop {
        (result.clone(), "error")
    } else {
        match (&error_text, result.trim().is_empty()) {
            (Some(err), _) => (err.clone(), "error"),
            (None, true) => (
                "worker 返回空结果：续行正常结束但没有产出任何最终回复文本".to_string(),
                "error",
            ),
            (None, false) => (String::new(), "success"),
        }
    };
    if status.1 == "error" {
        nemesis_cluster::logger::log_task("exec_failed", &task.task_id, &status.0);
    }
    // P4 交付门：同 execute_new_task——成功才捕获（diff 基线 = 本轮续行前
    // 快照）；失败 = None 不阻断回调主链路。
    let delivery = if status.1 == "success" {
        capture_task_delivery(
            agent_loop,
            &task.task_id,
            delivery_base.as_deref(),
            self_node_id,
            checks,
        )
    } else {
        None
    };
    send_task_callback(
        rpc_client,
        result_persister,
        task,
        self_node_id,
        status.1,
        &result,
        &status.0,
        usage,
        delivery,
        context.tool_path_base.as_deref(),
    )
    .await;
    task_list.complete_task(&task.task_id);
    nemesis_cluster::logger::log_task(
        "exec_done",
        &task.task_id,
        &format!("events={}", events.len()),
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// 职能渲染（集群专业职能框架 M3：per-task system prompt = 稳定前缀 + 职能后缀）
// ---------------------------------------------------------------------------

/// 本节点宣告的职能清单（peers.toml `[node].professions`，身份编辑热改写；
/// 经 loop 持有的 cluster 引用读取）。cluster 引用缺席（测试/降级装配）=
/// 空清单——一切带职能任务按 fallback 注记诚实处理。
fn self_declared_professions(agent_loop: &AgentLoop) -> Vec<String> {
    agent_loop
        .get_cluster()
        .and_then(|c| c.downcast_ref::<nemesis_cluster::cluster::Cluster>())
        .map(nemesis_cluster::cluster::Cluster::professions)
        .unwrap_or_default()
}

/// B2（profession-capability-evolution 2026-10-14）：讨论唤醒轮的工具围栏
/// 收窄。DiscussionEvent 无职能字段——讨论轮不是单一职能任务，按**本节点
/// 宣告职能的契约围栏交集**收窄（D 裁决）：讨论是节点以自己的专业身份参
/// 与，能力面 = 宣告职能围栏的公共部分。
///
/// - 无宣告职能 / 宣告职能契约都无围栏块 = 幺元跳过（None，不围）；
/// - 交集为空 = **不围**（fail-open，D 裁决——空交集是宣告组合的偶然形
///   态，不是契约作者的显式全禁意志，与任务装配的 Some(空) 语义刻意分
///   流）；讨论轮没有单一职能来源，也没有 Some(空) 哨兵形态；
/// - 总开关 `agents.profession_tool_fence` fresh-read 关 = 不围。
///
/// 返回 `Some(list)` 非空 = 讨论轮可用工具白名单（写 [`AgentConfig::tools`]，
/// `tool_fence_profession` 留 None——派发拒绝文案走无职能的通用措辞）。
fn discussion_tool_fence(agent_loop: &AgentLoop) -> Option<Vec<String>> {
    if !agent_loop.current_profession_tool_fence() {
        return None;
    }
    let declared = self_declared_professions(agent_loop);
    if declared.is_empty() {
        return None;
    }
    let ws = agent_loop.workspace_root();
    let mut acc: Option<Vec<String>> = None;
    for p in &declared {
        let slug = nemesis_prompts::professions::meta::normalize_slug(p);
        let (contract, _) = load_profession_assets(ws.as_deref(), &slug);
        let Some(fence) = contract
            .as_deref()
            .and_then(nemesis_prompts::professions::fence::parse_tool_fence)
        else {
            continue; // 幺元：该宣告职能无围栏块，不参与收窄
        };
        acc = Some(match acc {
            None => fence,
            Some(prev) => prev.into_iter().filter(|t| fence.contains(t)).collect(),
        });
    }
    let fence = acc.filter(|f| !f.is_empty())?;
    Some(fence)
}

/// 职能资产解析：内置目录优先（契约+专业方法论编译期内嵌），用户工作区
/// 扩展兜底（`<workspace>/cluster/professions/<family>/<spec>.md` 单文件，
/// 正文即契约+方法论）。内置命中时用户同名档案 WARN 忽略（内置赢）。
///
/// 安全闸：slug 先过 `validate_slug`（`^[a-z0-9_-]+(:[a-z0-9_-]+)?$`），
/// 非法（含 `..`/路径分隔符）一律落未知臂——绝不拼路径。
///
/// 优先级（A1 翻转，2026-10-14 D5）：**用户同名档案赢过内置**——提示词
/// 可编辑、部署后可热改（改完下个任务生效，与用户自定义职能同链路）。
/// 路径约定：`cluster/professions/<family>/<spec>.md`；裸 family（无
/// spec 段）用 family 名作文件名（如 `product` → `product/product.md`）。
/// 遮蔽 WARN：提示遮蔽事实 + 基线注记对照（档案首部 `<!-- base: … -->`
/// vs 内置当前基线），删除用户文件即回内置；不做自动合并（诚实边界：
/// 版本漂移靠人）。
///
/// 返回 (契约, 方法论)；契约 None = 未知 slug（渲染端诚实注记）。
/// 用户档案遮蔽内置 slug 时方法论仍取内置（专业知识与角色契约正交，
/// 用户要换方法论直接写进档案正文）。
fn load_profession_assets(
    workspace: Option<&std::path::Path>,
    slug: &str,
) -> (Option<String>, Option<String>) {
    use nemesis_prompts::professions::meta::find_builtin;
    use nemesis_prompts::professions::meta::split_slug;
    use nemesis_prompts::professions::meta::validate_slug;
    if validate_slug(slug).is_err() {
        return (None, None);
    }
    // 统一的用户档案查找：内置 slug（遮蔽臂）与非内置 slug（自定义臂）
    // 走同一条读盘路径，一次读盘定优先级。
    if let Some(ws) = workspace
        && let Some((family, spec)) = split_slug(slug)
    {
        let file_name = format!("{}.md", spec.unwrap_or(family));
        let user_file = ws
            .join("cluster")
            .join("professions")
            .join(family)
            .join(file_name);
        let content = std::fs::read_to_string(&user_file).ok();
        if let Some(content) = content
            && !content.trim().is_empty()
        {
            if find_builtin(slug).is_some() {
                let user_base =
                    nemesis_prompts::professions::render::extract_base_annotation(&content)
                        .unwrap_or("未注记");
                tracing::warn!(
                    slug,
                    file = %user_file.display(),
                    user_base,
                    builtin_base = nemesis_prompts::professions::render::BUILTIN_BASE_ANNOTATION,
                    "[ClusterAgent] 用户职能档案遮蔽内置（删除该文件即回内置）；若基于旧内置副本定制，请对照内置最新版核对差异"
                );
            }
            return (
                Some(content),
                nemesis_prompts::professions::render::builtin_method(slug).map(str::to_string),
            );
        }
    }
    match find_builtin(slug) {
        Some(meta) => (
            Some(meta.contract.to_string()),
            nemesis_prompts::professions::render::builtin_method(slug).map(str::to_string),
        ),
        None => (None, None),
    }
}

/// per-task system prompt：稳定前缀（`config.system_prompt`，装配期字节
/// 不变）+ 任务职能后缀（M3）。无职能子单原样克隆 config（现状兼容，
/// 零成本路径）。
///
/// 渲染臂（诚实注记三臂，见 professions::render）：内置/用户契约 → 契约
/// 正文（未宣告节点加 fallback 注记）；未知 slug → 契约缺席 + 通用纪律，
/// 绝不空串。
///
/// B2（profession-capability-evolution 2026-10-14）：职能工具围栏装配——
/// 契约文本内的 ` ```nemesis-tool-fence ` 块（D3：能力边界跟职能文本放一
/// 处，用户自定义档案天然同款）解析为该任务的实例级工具白名单，写进
/// [`AgentConfig::tools`]（非空 = 供给/派发双闸的权威判据）。纪律：
/// **先 parse（原文）后 strip（渲染用）**——围栏块是装配元数据不是提示词，
/// yaml 永不进 LLM 上下文（开关关也剥）。开关 `agents.profession_tool_fence`
/// fresh-read（缺省 true）；`Some(空)` = 显式全禁，落成 `tools=["question"]`
/// 哨兵形态（豁免通道之外全禁的最小可表示集——AgentConfig 以「非空白名单」
/// 表达「围了」，空表是「没围」，两者不可混同）。无围栏块的职能（dev:*/
/// test-* fail-open）不写 tools = 不围，零行为变化。
fn task_system_prompt(
    config: &AgentConfig,
    agent_loop: &AgentLoop,
    task: &nemesis_cluster::cluster_task::ClusterTask,
) -> AgentConfig {
    let Some(raw) = task
        .required_profession
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return config.clone();
    };
    let slug = nemesis_prompts::professions::meta::normalize_slug(raw);
    let (contract, method) = load_profession_assets(agent_loop.workspace_root().as_deref(), &slug);
    let declared = self_declared_professions(agent_loop)
        .iter()
        .any(|p| nemesis_prompts::professions::meta::normalize_slug(p) == slug);
    tracing::info!(
        task_id = %task.task_id,
        slug = %slug,
        builtin = contract.is_some() && nemesis_prompts::professions::meta::find_builtin(&slug).is_some(),
        declared,
        "[ClusterAgent] Rendering profession suffix"
    );
    // B2：围栏先解析（原文），渲染用剥除后的正文（yaml 永不进 prompt）。
    let fence_tools = contract
        .as_deref()
        .and_then(nemesis_prompts::professions::fence::parse_tool_fence);
    let contract_rendered = contract
        .as_deref()
        .map(nemesis_prompts::professions::fence::strip_tool_fence);
    let suffix = nemesis_prompts::professions::render::render_profession_suffix(
        nemesis_prompts::professions::render::SuffixRender {
            slug: &slug,
            contract: contract_rendered.as_deref(),
            method: method.as_deref(),
            declared,
        },
    );
    let mut cfg = config.clone();
    cfg.system_prompt = Some(match config.system_prompt.as_deref() {
        Some(prefix) if !prefix.trim().is_empty() => format!("{prefix}\n\n---\n\n{suffix}"),
        _ => suffix,
    });
    // B2：围栏写入（fresh-read 开关；白名单是闸的权威判据，职能标注只管
    // 拒绝文案）。显式全禁（Some(空)）→ question 哨兵形态，见 fn 纪律注。
    if let Some(allowed) = fence_tools
        && agent_loop.current_profession_tool_fence()
    {
        cfg.tools = if allowed.is_empty() {
            vec![nemesis_agent::r#loop::FENCE_EXEMPT_TOOLS[0].to_string()]
        } else {
            allowed
        };
        cfg.tool_fence_profession = Some(slug);
    }
    cfg
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

/// F-U3-2（U3 实证）路径基准判定：档案管线任务（工作副本有基线 sidecar）
/// → Some(exec_dir)，否则 None。`build_context`（dispatch 层文件工具/执行
/// cwd 的相对路径基准）与 `deliver_files_back`（交付回传相对路径解析）
/// 共用——两处口径必须一致，否则 worker 按 tool_path_base 写对的相对
/// 路径，回传时却按进程 cwd 读不到（2026-10-17 根修：交付回传面此前
/// 对相对路径形同虚设）。
fn cluster_task_path_base(
    agent_loop: &AgentLoop,
    task: &nemesis_cluster::cluster_task::ClusterTask,
) -> Option<std::path::PathBuf> {
    let ws = agent_loop.workspace_root()?;
    let exec_dir = nemesis_cluster::exec_workspace::exec_dir_for(&ws, &task.task_id);
    (exec_dir
        .join(nemesis_cluster::exec_workspace::BASELINE_SIDECAR_NAME)
        .exists())
    .then_some(exec_dir)
}

/// Build a RequestContext for a cluster task.
///
/// `chat_id` 用稳定的 `session_key`（而不是 `node_id:task_id`），让同一对端节点的多次
/// peer_chat 共享 chat_id。这样下游工具（cluster_rpc 多跳传播、cron、spawn）拿到的
/// chat_id 不会每次变化，避免历史断裂和路由失效。
fn build_context(
    agent_loop: &AgentLoop,
    task: &nemesis_cluster::cluster_task::ClusterTask,
) -> RequestContext {
    let mut ctx = RequestContext::new(
        "cluster",
        &task.source.session_key,
        &task.source.node_id,
        &task.source.session_key,
    );
    // F-U3-2（UAT U3 实证）：档案管线任务（工作副本有基线 sidecar）→ dispatch
    // 层文件工具/执行 cwd 的相对路径基准 = 工作副本目录。worker 误用相对路径
    // 不再静默落到工作副本之外（变更集只扫 exec 目录——落外面=成果丢失）。
    // 非档案任务（无 sidecar）与普通会话一样 base 为空，零行为变化。
    ctx.tool_path_base = cluster_task_path_base(agent_loop, task);
    ctx
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

/// P1（2026-09-11 真机日志分析）：turn 终止原因 → 回调 status 映射。
///
/// 若事件流以 `Error` 终止（校验预算耗尽 / LLM 重试耗尽 / 升级硬停等
/// force_stop 路径），返回该错误文本——这类 turn 是失败终止，不能包装成
/// success 回调（真机 7/7 样本都是 0.6~1.5ms 内 success + 空交付）。
/// 只有末事件是 `Done`（或流为空但无 Error）时返回 None。
fn terminal_error(events: &[AgentEvent]) -> Option<String> {
    events.last().and_then(|e| match e {
        AgentEvent::Error(msg) => Some(msg.clone()),
        _ => None,
    })
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
    self_node_id: &str,
    status: &str,
    response: &str,
    error: &str,
    usage: Option<serde_json::Value>,
    delivery: Option<serde_json::Value>,
    path_base: Option<&std::path::Path>,
) {
    // P2A（2026-09-12 NB-15）：终结失败分类随 error 回调上行（success =
    // None，字段不落），A 端验收重派决策据此避免同 worker 同模型盲重派。
    let fail_class = classify_terminal_failure(status, response, error);
    tracing::info!(
        task_id = %task.task_id,
        status = %status,
        fail_class = ?fail_class,
        has_delivery = delivery.is_some(),
        target_node = %task.source.node_id,
        "[ClusterAgent] Sending callback"
    );

    send_callback_or_persist(
        rpc_client,
        result_persister,
        &None,
        &task.source.node_id,
        self_node_id,
        &task.task_id,
        status,
        response,
        error,
        usage,
        fail_class,
        delivery,
    )
    .await;

    // G（goal P3）：成功交付 → 解析交付文本中的文件路径回传 master
    // （失败/取消不回传；解析/读取失败诚实降级——交付评论内联源码兜底）。
    // 相对路径按 path_base（= dispatch 层 tool_path_base 同源）解析，
    // 绝对路径原样。
    if status == "success" {
        deliver_files_back(rpc_client, task, self_node_id, response, path_base).await;
    }
}

/// G（goal P3）交付文件回传：解析交付文本反引号里的路径（五段式「改动
/// 文件路径」清单），逐个（≤20 个、单文件 ≤8MB、需存在）base64 后经
/// nb_bus ns="task" op="delivery.files" 推给 master 落资产。fire-and-forget：
/// 失败只 debug（master 侧溯源线索=交付评论里的内联源码与路径）。
/// 相对路径按 path_base 解析（与 dispatch 层 tool_path_base 同源——
/// [`cluster_task_path_base`]；绝对路径原样），否则档案管线里 worker 按
/// 工作副本根写的相对路径全部落空（metadata 失败被静默 continue）。
async fn deliver_files_back(
    rpc_client: Option<&RpcClient>,
    task: &nemesis_cluster::cluster_task::ClusterTask,
    self_node_id: &str,
    delivery_text: &str,
    path_base: Option<&std::path::Path>,
) {
    use base64::Engine as _;
    const MAX_FILES: usize = 20;
    const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;

    let mut paths: Vec<String> = Vec::new();
    for seg in delivery_text.split('`') {
        let t = seg.trim();
        // 路径形态启发：含分隔符、无空白、非纯词（回退靠内联源码）。
        if (t.contains('/') || t.contains('\\'))
            && !t.contains(' ')
            && t.len() > 3
            && !paths.iter().any(|x| x == t)
        {
            paths.push(t.to_string());
            if paths.len() >= MAX_FILES {
                break;
            }
        }
    }
    if paths.is_empty() {
        return;
    }
    let Some(rpc) = rpc_client else {
        return;
    };

    let mut files = Vec::new();
    for p in &paths {
        // 相对路径 → path_base 拼接（与 dispatch 层工具同一基准）；绝对路径原样。
        let resolved = match path_base {
            Some(base) if !std::path::Path::new(p).is_absolute() => base.join(p),
            _ => std::path::PathBuf::from(p),
        };
        let Ok(meta) = std::fs::metadata(&resolved) else {
            continue;
        };
        if !meta.is_file() || meta.len() as usize > MAX_FILE_BYTES {
            continue;
        }
        let Ok(bytes) = std::fs::read(&resolved) else {
            continue;
        };
        let name = p.rsplit(['/', '\\']).next().unwrap_or("file").to_string();
        files.push(serde_json::json!({
            "name": name,
            "content_b64": base64::engine::general_purpose::STANDARD.encode(&bytes),
        }));
    }
    if files.is_empty() {
        return;
    }

    let payload = serde_json::json!({
        "v": nemesis_cluster::envelope::ENVELOPE_VERSION,
        "ns": "task",
        "op": "delivery.files",
        "corr_id": uuid::Uuid::new_v4().to_string(),
        "body": { "task_id": task.task_id, "files": files },
    });
    let request = RPCRequest {
        id: uuid::Uuid::new_v4().to_string(),
        action: ActionType::Custom(nemesis_cluster::envelope::NB_BUS_ACTION.to_string()),
        payload,
        source: self_node_id.to_string(),
        target: Some(task.source.node_id.clone()),
    };
    if let Err(e) = rpc.call(&task.source.node_id, request).await {
        tracing::debug!(task_id = %task.task_id, error = %e,
            "[ClusterAgent] 交付文件回传失败（交付评论内联源码兜底）");
    } else {
        tracing::info!(task_id = %task.task_id, count = files.len(),
            "[ClusterAgent] 交付文件已回传 master");
    }
}

/// P2A（2026-09-12 双端真机 NB-15 根修）：终结失败分类——error 回调负载
/// 携带结构化 `fail_class`，A 端验收重派决策据此避免同 worker 同模型盲目
/// 重派（能力类失败重派同节点大概率原样复现：NB-15 校验预算耗尽连烧两轮
/// 重派预算）。分类真相源 = 各终结路径的稳定文本前缀（loop.rs 生成侧与此
/// 处消费侧同步演化，tests 钉死契约）：
/// - `validation_budget`：工具参数校验连续失败（loop.rs 校验预算耗尽停轮）
/// - `escalation`：turn_guard 升级硬停（稳定前缀
///   [`nemesis_agent::turn_guard::ESCALATION_MARKER`]）
/// - `llm_timeout` / `llm_failure`：LLM 调用终结失败（transient 重试耗尽 /
///   非瞬态错误），超时形态单列（撞墙检测的跨端可观测面；尽力匹配——
///   provider 错误文案不受本侧控制）
/// - `llm_rate_limit`（2026-09-17 BUG 文档裁决⑤）：上游 429 限流重试耗尽
///   （loop.rs 重试环终局文案含「rate limited」），评审重派决策据此避免
///   同 worker 同模型盲重派
/// - `empty_result`：turn 正常结束但无交付文本
/// - `token_budget`（P8 集群无人值守硬化 A2）：会话 token 预算耗尽——
///   loop 硬停与入口拒接两类文案同前缀
///   [`nemesis_agent::token_budget::SESSION_TOKEN_BUDGET_MARKER`]；master
///   board_review 识别后转人工不重派
/// - `exec_failed`：其它失败（兜底）。success 回调返回 None（不落字段）。
fn classify_terminal_failure(status: &str, response: &str, error: &str) -> Option<&'static str> {
    if status != "error" {
        return None;
    }
    let text = if error.is_empty() { response } else { error };
    if text.starts_with("工具参数校验连续失败") {
        Some("validation_budget")
    } else if text.starts_with(nemesis_agent::turn_guard::ESCALATION_MARKER) {
        Some("escalation")
    } else if text.starts_with(nemesis_agent::token_budget::SESSION_TOKEN_BUDGET_MARKER) {
        Some("token_budget")
    } else if text.starts_with("worker 返回空结果") {
        Some("empty_result")
    } else if text.starts_with("Error:") {
        let lower = text.to_ascii_lowercase();
        if lower.contains("timed out") || lower.contains("timeout") || text.contains("超时") {
            Some("llm_timeout")
        } else if lower.contains("rate limited") || lower.contains("429") {
            Some("llm_rate_limit")
        } else {
            Some("llm_failure")
        }
    } else {
        Some("exec_failed")
    }
}

/// 本轮 token 用量提取（E1 二期）：run 前后各取一次 session 聚合，差值 =
/// 本任务消耗（饱和减法防下溢——并发清扫/rollup 可能削行）。设计裁决：
/// AgentEvent 事件流目前不携带 usage，走 goal 钦定的 fallback 查
/// data_store（两次聚合查询代价可忽略）；未装配 data_store = None。
fn extract_task_usage(
    agent_loop: &AgentLoop,
    session_key: &str,
    before: &nemesis_data::SessionUsageAgg,
) -> Option<serde_json::Value> {
    let ds = agent_loop.data_store()?;
    let after = ds.aggregate_session_usage(session_key).ok()?;
    Some(task_usage_delta(before, &after))
}

/// 纯差值（单测钉死）：after - before 逐字段饱和减法（防下溢——retention
/// 清扫 / rollup 可能在任务执行窗口削行）。
fn task_usage_delta(
    before: &nemesis_data::SessionUsageAgg,
    after: &nemesis_data::SessionUsageAgg,
) -> serde_json::Value {
    serde_json::json!({
        // 负差钳 0（saturating_sub 只防 i64 下溢，不防语义负值——retention
        // 清扫/rollup 削行时 after 可能小于 before）。
        "input_tokens": (after.input_tokens - before.input_tokens).max(0),
        "output_tokens": (after.output_tokens - before.output_tokens).max(0),
        "requests": (after.requests - before.requests).max(0),
        "cost_usd": (after.total_cost_usd - before.total_cost_usd).max(0.0),
    })
}

/// 多跳用量合并（2026-10-17 异步多跳根修）：async 暂存的此前各跳累计 +
/// 本轮差值，逐字段饱和加法。`carried` 缺失/字段缺失按 0 处理（旧任务
/// 记录无 stash、旧 delta 形态缺字段都兼容）。
fn task_usage_merge(
    carried: Option<&serde_json::Value>,
    leg: &serde_json::Value,
) -> serde_json::Value {
    let num = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_i64()).unwrap_or(0);
    let fnum = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0);
    let empty = serde_json::Value::Null;
    let c = carried.unwrap_or(&empty);
    let sat_add = |a: i64, b: i64| a.saturating_add(b);
    serde_json::json!({
        "input_tokens": sat_add(num(c, "input_tokens"), num(leg, "input_tokens")),
        "output_tokens": sat_add(num(c, "output_tokens"), num(leg, "output_tokens")),
        "requests": sat_add(num(c, "requests"), num(leg, "requests")),
        "cost_usd": fnum(c, "cost_usd") + fnum(leg, "cost_usd"),
    })
}

/// 执行前快照 session 用量（[`extract_task_usage`] 的前半；未装配 = 全零）。
fn snapshot_session_usage(
    agent_loop: &AgentLoop,
    session_key: &str,
) -> nemesis_data::SessionUsageAgg {
    agent_loop
        .data_store()
        .and_then(|ds| ds.aggregate_session_usage(session_key).ok())
        .unwrap_or_default()
}

/// Handle task execution error: mark as failed and send error callback.
async fn handle_task_error(
    task_list: &ClusterTaskList,
    rpc_client: Option<&RpcClient>,
    result_persister: Option<&dyn TaskResultPersister>,
    task: &nemesis_cluster::cluster_task::ClusterTask,
    self_node_id: &str,
    error_msg: &str,
) {
    let error_preview = truncate_str(error_msg, 200);
    nemesis_cluster::logger::log_task("exec_failed", &task.task_id, &error_preview);
    task_list.update_status(&task.task_id, TaskStatus::Failed);
    send_task_callback(
        rpc_client,
        result_persister,
        task,
        self_node_id,
        "error",
        "",
        error_msg,
        None,
        None,
        None,
    )
    .await;
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
    // B2 工具围栏（profession-capability-evolution 2026-10-14）：讨论轮按
    // 本节点宣告职能的契约围栏交集收窄工具面（D 裁决）。tool_fence_
    // profession 留 None——派发拒绝文案走无职能的通用措辞（讨论无单一职
    // 能来源）。
    let mut cfg = config.clone();
    if let Some(allowed) = discussion_tool_fence(agent_loop) {
        cfg.tools = allowed;
    }
    let instance = AgentInstance::new(cfg);
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
            &instance,
            &prompt,
            &context,
            &trace_id,
            false,
            &token,
            None,
            &[],
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

/// 发送者身份标签：board 开 = [`crate::board_bus::actor_label`] 权威实现；
/// 关 = 裸 id 降级（board_bus 模块随 all(board, cluster) 闸不编译，而讨论
/// 子系统在 board 关时本就不装配——此臂只为 cluster-only 裁剪形态编译完整）。
#[cfg(all(feature = "board", feature = "cluster"))]
fn actor_label(kind: &str, id: &str) -> String {
    crate::board_bus::actor_label(kind, id)
}
#[cfg(not(all(feature = "board", feature = "cluster")))]
fn actor_label(kind: &str, id: &str) -> String {
    let _ = kind;
    id.to_string()
}

/// 组装讨论 prompt：线程历史 + 唤醒原因 + 回复预算 + 发言/沉默二选一指令。
/// 发送者身份经 [`actor_label`] 渲染（admin=human user /
/// agent=agent node——2026-10-23 身份透传：LLM 由此知道说话的是人还是节点，
/// 不再对着人类只能 @ 自己）。
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
            ctx.push_str(&format!(
                "- {}: {}\n",
                actor_label(&m.kind, &m.sender),
                m.content
            ));
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
        event.event,
        actor_label(&event.sender_kind, &event.new_sender),
        event.new_content
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

/// D2 读取臂：`board.comment.list` 上行信封（线程纯读——无幂等键、无
/// 额度记账；limit 缺省由 master 侧收敛）。唯一消费点 board_discuss_tool
/// 在 board+cluster 门内——随门裁剪（board 关时常量留场 = dead_code，
/// 同 STALL_MARKER 家族先例）。
#[cfg(all(feature = "board", feature = "cluster"))]
pub(crate) fn build_comment_list_envelope(
    thread_kind: &str,
    thread_id: i64,
    limit: i64,
) -> serde_json::Value {
    EnvelopeResponse::success(
        &Envelope {
            ns: "board".to_string(),
            op: "comment.list".to_string(),
            corr_id: uuid::Uuid::new_v4().to_string(),
            ..Envelope::default()
        },
        serde_json::json!({
            "thread": {"kind": thread_kind, "id": thread_id},
            "limit": limit,
        }),
    )
    .to_json()
}

/// 找在线 coordinator（≠本节点）。master 地址每次现查——节点表鲜活，
/// 重选主 / 离线切换不需要重启调用方。board_discuss 与 board_claim 两
/// 工具同源（单一真相源）。
#[cfg(all(feature = "board", feature = "cluster"))]
pub(crate) fn find_online_coordinator(
    cluster: &nemesis_cluster::cluster::Cluster,
    self_node_id: &str,
) -> Result<String, String> {
    cluster
        .list_nodes()
        .into_iter()
        .find(|n| {
            n.base.role == nemesis_types::cluster::NodeRole::Coordinator
                && n.base.id != self_node_id
                && n.is_online()
        })
        .map(|n| n.base.id)
        .ok_or_else(|| {
            "no online coordinator known (cluster not started, or master offline \
             — the board.sync backfill channel will retry later)"
                .to_string()
        })
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
            let ok = body.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
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

// ---------------------------------------------------------------------------
// M5-3（2026-10-10 机制加固）：cluster ask 看板可见化——审批等待不再无声
// ---------------------------------------------------------------------------

/// 从审批 ask 的 chat_id 提取看板单号。worker 侧 cluster 会话键形态
/// `cluster_rpc:{source_node}/board:{n}`（peer_chat_handler 原样拼接；
/// board 派发 chat_id = `board:{number}`，board.rs / board_review.rs 两个
/// 派发点同源）；`board:NB-{n}` 形态无生产产出方，容忍兼容不信任其存在。
/// 非看板会话（dashboard peer_chat 等）/单号非法 → None（调用方静默跳过）。
#[cfg_attr(not(all(feature = "security", feature = "cluster")), allow(dead_code))]
pub(crate) fn issue_number_from_cluster_chat_id(chat_id: &str) -> Option<i64> {
    let board = chat_id.rsplit("/").next()?.trim();
    let n = board.strip_prefix("board:")?;
    let n = n.strip_prefix("NB-").unwrap_or(n);
    n.parse::<i64>().ok().filter(|v| *v > 0)
}

/// M5-3 主入口：cluster ask 触发时向 master 权威看板 issue 线程投递系统
/// 注记（审批等待可见化半步）。非看板会话静默跳过；投递单次尝试，失败
/// WARN 不重试（同 `post_discussion_reply` 语义——审批等待不依赖注记到达）。
/// 同步入口（Composite 观察者在审批等待的同步上下文里触发）：有 tokio
/// runtime 则 spawn 后台投递，无 runtime（纯同步测试环境）诚实记日志跳过。
#[cfg(all(feature = "security", feature = "cluster"))]
pub(crate) fn spawn_board_ask_notice(
    cluster: Arc<nemesis_cluster::cluster::Cluster>,
    chat_id: &str,
    request_id: &str,
    operation: &str,
    target: &str,
    risk_level: &str,
    reason: &str,
    timeout_secs: u64,
) {
    let Some(issue_number) = issue_number_from_cluster_chat_id(chat_id) else {
        return; // 非看板 cluster 会话（无 issue 可注记），静默。
    };
    let self_node_id = cluster.node_id().to_string();
    let request_id = request_id.to_string();
    let operation = operation.to_string();
    let target = target.to_string();
    let risk_level = risk_level.to_string();
    let reason = reason.to_string();
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        tracing::info!(
            "[BoardAsk] request {request_id}（{operation} → {target}）无可投递 runtime，注记跳过"
        );
        return;
    };
    handle.spawn(async move {
        if let Err(e) = post_board_ask_notice(
            cluster,
            &self_node_id,
            issue_number,
            &request_id,
            &operation,
            &target,
            &risk_level,
            &reason,
            timeout_secs,
        )
        .await
        {
            tracing::warn!("[BoardAsk] issue NB-{issue_number} 审批注记投递失败（不重试）：{e}");
        }
    });
}

/// 注记投递本体：找在线 coordinator（≠本节点）→ board.comment.post 信封
/// → nb_bus RPC。master 额度三闸拒绝 / RPC 失败都算投递失败（诚实 Err）。
#[cfg(all(feature = "security", feature = "cluster"))]
async fn post_board_ask_notice(
    cluster: Arc<nemesis_cluster::cluster::Cluster>,
    self_node_id: &str,
    issue_number: i64,
    request_id: &str,
    operation: &str,
    target: &str,
    risk_level: &str,
    reason: &str,
    timeout_secs: u64,
) -> Result<(), String> {
    let coordinator = cluster
        .list_nodes()
        .into_iter()
        .find(|n| {
            n.base.role == nemesis_types::cluster::NodeRole::Coordinator
                && n.base.id != self_node_id
                && n.is_online()
        })
        .map(|n| n.base.id)
        .ok_or_else(|| "no online coordinator known（注记无投递目标）".to_string())?;
    let rpc = cluster
        .rpc_client_arc()
        .ok_or_else(|| "rpc client unavailable（cluster 未启动）".to_string())?;
    let short_id: String = request_id.chars().take(8).collect();
    // 策略理由截断（防长策略刷屏；120 字符足够定位是哪条规则）。
    let reason_short: String = reason.chars().take(120).collect();
    let content = format!(
        "⚠ [审批等待] 执行中遇到需人工放行的操作（审批单 {short_id}，风险 {risk_level}）\n\
         - 操作：{operation} → {target}\n\
         - 策略理由：{reason_short}\n\
         - {timeout_secs}s 内未裁决将自动拒绝，任务可能因此失败/重派\n\
         - 处理入口：节点 {self_node_id} 的 Dashboard 审批卡（或其 approval_rules 预放行）"
    );
    let event = DiscussionEvent {
        thread_kind: "issue".to_string(),
        thread_id: issue_number,
        ..DiscussionEvent::default()
    };
    let payload = build_comment_post_envelope(self_node_id, &event, &content);
    let body = send_nb_bus(&rpc, self_node_id, &coordinator, payload).await?;
    if body.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        tracing::info!(
            "[BoardAsk] issue NB-{issue_number} 审批注记已投递（request {short_id}，{operation}）"
        );
        Ok(())
    } else {
        Err(format!(
            "master 拒绝注记（额度/校验）：{}",
            body.get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
        ))
    }
}

#[cfg(test)]
mod tests;

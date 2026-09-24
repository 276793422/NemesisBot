//! 入站门控：ConcurrentMode/GateOutcome/TurnAdmission/SessionBusyState/SentInRoundTracker、parse_approval_reply、route/gate/process_direct/heartbeat/inbound_message、会话忙/取消/收件箱族、persist_busy_refusal。
//!
//! P1 自 `loop.rs` 物理搬迁（docs/PLAN/2026-09-23_agentloop-god-object-decomposition.md §3.2）；语义零变化。
use super::prelude::*;
use super::*;

/// Concurrent request handling mode.
///
/// D (2026-09-23 多会话并行清账)：本枚举**只决定忙时会话处置语义**，不再
/// 决定泵调度——泵已统一为「gate 内联 + turn spawn」（跨会话并发、同会话
/// 保序，任何模式下成立；`run_bus_impl`）。历史版本里 Reject 曾意味着泵级
/// 串行（上一条 turn 不结束下一条消息不出队），导致主 loop 上一个长 turn
/// 堵死所有会话——那个耦合正是本轮清账的根因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConcurrentMode {
    /// Session busy → 立即回执拒绝（诚实留痕：user 行 + busy 回实行均落
    /// chat_log，见 gate_inbound 忙分支）。
    #[default]
    Reject,
    /// Queue messages when session is busy — processed after the current turn.
    Queue,
    /// Queue + steer: `!`-prefixed messages are injected into the RUNNING
    /// turn before its next LLM call (I1 / U7).
    Steer,
}

/// V5 (2026-08-23): outcome of the synchronous inbound gate (`gate_inbound`).
#[derive(Debug)]
pub(crate) enum GateOutcome {
    /// `cluster_continuation` marker — the pump handles it inline via
    /// `dispatch_continuation` (serial, unchanged from the legacy loop).
    Continuation(String),
    /// Short-circuit reply (busy receipt / busy bounce / queue-full / slash
    /// command response). No session was acquired.
    Immediate { agent_id: String, response: String },
    /// E6 (2026-09-05): `/compact` / `/clear` — 会话维护命令。gate 的同步
    /// 短路点拿不到 instance（LLM 摘要是 async），但会话已在此获取（防
    /// 并发回合与压缩互相踩摘要推进）；pump/serial tail 派发到
    /// `handle_maintenance`，回执经 `finish_message` 走正常出站路径。
    Maintenance {
        kind: SessionMaintenance,
        session_key: String,
        receipt: String,
    },
    /// K4 (devtool-upgrade 阶段 7): 用户直发远程编码任务派发——
    /// `/build <task> repo:<node>`。会话已在 gate 获取（Maintenance 同款，
    /// 防并发回合踩会话）；tail 走 `process_user_dispatch`：经
    /// `handle_tool_call` 全管线（安全 8 层 / estop / Plan 闸）提交
    /// `cluster_rpc`，ACK 后存自足续行快照，完成回调复用
    /// `handle_cluster_continuation` 呈现结果并回原通道。
    UserDispatch {
        task_text: String,
        node_id: String,
        session_key: String,
    },
    /// System (non-continuation) / history-request passthrough — async
    /// handling, no session semantics.
    Ungated,
    /// Normal chat message: the session is already acquired and the cancel
    /// token minted. The tail (`process_admitted`) owns release + drain.
    Admitted(TurnAdmission),
}

/// V5: admission minted by the gate — everything the turn tail needs that
/// the gate acquired on the message's behalf.
#[derive(Debug)]
pub(crate) struct TurnAdmission {
    pub(crate) agent_id: String,
    pub(crate) session_key: String,
    pub(crate) cancel_token: tokio_util::sync::CancellationToken,
    /// E3：本消息在 `turn_preamble` 里 begin 的 checkpoint turn 序号（无
    /// store 挂载时 None）。随 admission 穿针到行落盘点——不能落盘时反查
    /// `cur`（整 turn 的 await 间隙里可能被其他会话/steer 的 begin 翻掉）。
    cp_turn: Option<usize>,
}
/// K4 (b): IM 审批卡回执语法——`/approve <id>` / `/deny <id>`（id 为审批
/// 卡给出的短编号，6~12 位十六进制字符）。命中返回静态确认文案（gate
/// Immediate 短路，不进 agent）；格式不符返回 None 落普通轮。真实裁决与
/// 无效 id 的诚实提示由 gateway 审批回执 watcher 负责。
fn parse_approval_reply(content: &str) -> Option<String> {
    let trimmed = content.trim();
    let (verb, id) = if let Some(rest) = trimmed.strip_prefix("/approve ") {
        ("approve", rest.trim())
    } else if let Some(rest) = trimmed.strip_prefix("/deny ") {
        ("deny", rest.trim())
    } else {
        return None;
    };
    let id = id.trim();
    if id.is_empty() || id.len() > 64 || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    Some(match verb {
        "approve" => format!("✓ 已收到审批回复（同意 {id}），结果另行通知。"),
        _ => format!("✓ 已收到审批回复（拒绝 {id}），结果另行通知。"),
    })
}
/// Per-session busy state with queue length.
#[derive(Debug, Clone, Default)]
pub(crate) struct SessionBusyState {
    pub(crate) busy: bool,
    pub(crate) queue_length: usize,
}
/// Tracks whether a message has already been sent in the current LLM round.
/// This prevents double-sending when the agent loop also publishes outbound.
#[derive(Debug, Default)]
pub(crate) struct SentInRoundTracker {
    /// session_key -> whether a tool already sent a message this round.
    sent: parking_lot::Mutex<std::collections::HashSet<String>>,
}

impl SentInRoundTracker {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Mark that a message was sent for the given session key this round.
    pub(crate) fn mark_sent(&self, session_key: &str) {
        self.sent.lock().insert(session_key.to_string());
    }

    /// Check if a message was already sent for the given session key.
    pub(crate) fn has_sent_in_round(&self, session_key: &str) -> bool {
        self.sent.lock().contains(session_key)
    }

    /// Clear the sent flag for a session (start of new round).
    pub(crate) fn clear(&self, session_key: &str) {
        self.sent.lock().remove(session_key);
    }

    /// Clear all sent flags.
    #[allow(dead_code)]
    pub(crate) fn clear_all(&self) {
        self.sent.lock().clear();
    }
}

impl AgentLoop {
    /// Clear all session busy states.
    ///
    /// Called after a forced stop (task abort) to release sessions that were
    /// mid-processing when the agent was killed.  Without this, those sessions
    /// remain permanently locked ("busy") and all subsequent messages for them
    /// are rejected.
    pub fn clear_session_busy(&self) {
        let mut map = self.session_busy.lock();
        let count = map.len();
        map.clear();
        if count > 0 {
            tracing::warn!(
                "[AgentLoop] Cleared {} session busy states (agent was stopped mid-processing)",
                count
            );
        }
    }

    // -----------------------------------------------------------------------
    // Observer event emission helpers
    // -----------------------------------------------------------------------

    /// Process a direct message without the bus.
    /// Mirrors Go's `ProcessDirect()`.
    pub async fn process_direct(&self, content: &str, session_key: &str) -> Result<String, String> {
        self.process_direct_with_channel(content, session_key, "cli", "direct")
            .await
    }

    /// Process a direct message with explicit channel/chat ID.
    /// Mirrors Go's `ProcessDirectWithChannel()`.
    pub async fn process_direct_with_channel(
        &self,
        content: &str,
        session_key: &str,
        channel: &str,
        chat_id: &str,
    ) -> Result<String, String> {
        let trace_id = format!(
            "direct-{}-{}",
            session_key,
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let start_time = std::time::Instant::now();

        // Emit conversation_start observer event.
        self.emit_observer_sync(crate::loop_executor::ObserverEvent::ConversationStart {
            trace_id: trace_id.clone(),
            session_key: session_key.to_string(),
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
            sender_id: "direct".to_string(),
            content: content.to_string(),
        })
        .await;

        let instance = self.get_or_create_instance(session_key);
        let context = RequestContext::new(channel, chat_id, "cron", session_key);

        let token = tokio_util::sync::CancellationToken::new();
        let events = self
            .run_with_trace(
                &instance,
                content,
                &context,
                &trace_id,
                false,
                &token,
                None,
                &[],
            )
            .await;

        // Extract final response for the conversation end event.
        let final_response = events
            .iter()
            .rev()
            .find_map(|e| {
                if let AgentEvent::Done(msg) = e {
                    Some(msg.clone())
                } else {
                    None
                }
            })
            .unwrap_or_default();

        // Emit conversation_end observer event.
        let duration_ms = start_time.elapsed().as_millis() as u64;
        let rounds = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::ToolCall(_)))
            .count() as u32
            + 1;
        self.emit_observer_sync(crate::loop_executor::ObserverEvent::ConversationEnd {
            trace_id: trace_id.clone(),
            session_key: session_key.to_string(),
            total_rounds: rounds,
            duration_ms,
            content: final_response,
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
        })
        .await;

        // Extract final response from events.
        for event in events.iter().rev() {
            if let AgentEvent::Done(msg) = event {
                return Ok(msg.clone());
            }
        }
        for event in events.iter().rev() {
            if let AgentEvent::Error(msg) = event {
                return Err(msg.clone());
            }
        }
        Ok(String::new())
    }

    /// Process a heartbeat request without session history.
    /// Each heartbeat is independent and doesn't accumulate context.
    /// Mirrors Go's `ProcessHeartbeat()`.
    pub async fn process_heartbeat(
        &self,
        content: &str,
        channel: &str,
        chat_id: &str,
    ) -> Result<String, String> {
        let trace_id = format!(
            "heartbeat-{}-{}",
            chat_id,
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let start_time = std::time::Instant::now();

        // Emit conversation_start observer event.
        self.emit_observer_sync(crate::loop_executor::ObserverEvent::ConversationStart {
            trace_id: trace_id.clone(),
            session_key: "heartbeat".to_string(),
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
            sender_id: "heartbeat".to_string(),
            content: content.to_string(),
        })
        .await;

        // Heartbeat uses a fresh temporary instance, no history.
        let config = AgentConfig {
            model: self.active_model.read().clone(),
            system_prompt: self.config.system_prompt.clone(),
            max_turns: self.config.max_turns,
            tools: self.config.tools.clone(),
            models: self.config.models.clone(),
        };
        let instance = AgentInstance::new(config);
        let context = RequestContext::new(channel, chat_id, "heartbeat", "heartbeat");

        let token = tokio_util::sync::CancellationToken::new();
        let events = self
            .run_with_trace(
                &instance,
                content,
                &context,
                &trace_id,
                false,
                &token,
                None,
                &[],
            )
            .await;

        // Extract final response for the conversation end event.
        let final_response = events
            .iter()
            .rev()
            .find_map(|e| {
                if let AgentEvent::Done(msg) = e {
                    Some(msg.clone())
                } else {
                    None
                }
            })
            .unwrap_or_default();

        // Emit conversation_end observer event.
        let duration_ms = start_time.elapsed().as_millis() as u64;
        let rounds = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::ToolCall(_)))
            .count() as u32
            + 1;
        self.emit_observer_sync(crate::loop_executor::ObserverEvent::ConversationEnd {
            trace_id: trace_id.clone(),
            session_key: "heartbeat".to_string(),
            total_rounds: rounds,
            duration_ms,
            content: final_response,
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
        })
        .await;

        for event in events.iter().rev() {
            if let AgentEvent::Done(msg) = event {
                return Ok(msg.clone());
            }
        }
        Ok("I've completed processing but have no response to give.".to_string())
    }

    // -----------------------------------------------------------------------
    // Inbound message processing (bus mode)
    // -----------------------------------------------------------------------

    /// Process an inbound message from the bus.
    ///
    /// Returns (agent_id, response_content, optional_error).
    /// Mirrors Go's `processMessage()`.
    pub(crate) async fn process_inbound_message(
        &self,
        msg: &nemesis_types::channel::InboundMessage,
    ) -> (String, String, Option<String>) {
        // 自定义 slash 命令改写（改写型，见 rewrite_custom_command）：在闸门
        // 之前原地展开为提示词，让消息以最终形态走正常会话/LLM 流程。内置
        // 命令不受影响（rewrite 跳过内置名，gate 内的短路检查照常先行）。
        // K3：async 化（`` !`cmd` `` 注入 + 技能回落），模板注入命令在闸门
        // 前执行完毕——进闸的消息永远是最终形态。
        let mut msg = msg.clone();
        self.rewrite_custom_command(&mut msg).await;
        // V5 (2026-08-23): gate first (sync classification + session
        // acquire), then the matching tail. D (2026-09-23)：bus 泵已统一为
        // gate+spawn（`run_bus_impl`），本入口不再服务泵——仅存量的直调方
        // （tests、process_admitted 无 reinject 时的 inline 排队回退）继续
        // 经此进入，行为与 gate 语义一致（含 D-2 忙回绝留痕）。
        match self.gate_inbound(&msg) {
            GateOutcome::Continuation(task_id) => ("__continuation__".to_string(), task_id, None),
            GateOutcome::Immediate { agent_id, response } => (agent_id, response, None),
            GateOutcome::Maintenance {
                kind,
                session_key,
                receipt,
            } => {
                // 串行路径内联执行：⏳ 回执先发（会话已获取），干活，✓ 回执
                // 随后；返回空串让调用方的 finish_message 早退（不重复发布）。
                self.finish_message(&msg, receipt, None, false).await;
                let response = self.handle_maintenance(kind, &session_key).await;
                self.release_session(&session_key);
                self.finish_message(&msg, response, None, false).await;
                (String::new(), String::new(), None)
            }
            GateOutcome::UserDispatch {
                task_text,
                node_id,
                session_key,
            } => {
                // K4: 用户直发远程编码任务派发——串行路径内联执行（ACK 等
                // 待可阻塞泵，与 Maintenance 的 LLM 摘要同理由）。收据/快照/
                // 错误回复都在 process_user_dispatch 内部完成；返回空串让
                // 调用方的 finish_message 早退。
                self.process_user_dispatch(&msg, &task_text, &node_id, &session_key)
                    .await;
                (String::new(), String::new(), None)
            }
            GateOutcome::Ungated => self.process_ungated(&msg).await,
            GateOutcome::Admitted(admission) => self.process_admitted(&msg, admission).await,
        }
    }

    /// Turn preamble shared by every gated path (V5: extracted verbatim from
    /// the head of the pre-split `process_inbound_message`, so gate-time
    /// short-circuits — busy receipts, slash replies — behave exactly as
    /// before: the monolith opened the checkpoint turn and logged BEFORE any
    /// classification).
    ///
    /// E3：返回本消息 begin 的 checkpoint turn 序号（无 store 挂载时
    /// None）——gate 捕获后装进 [`TurnAdmission`]，随链穿针到行落盘点做
    /// `checkpoint_turn` 行标记（消息级回退的行→turn 定位锚）。
    fn turn_preamble(&self, msg: &nemesis_types::channel::InboundMessage) -> Option<usize> {
        // Open a checkpoint turn for the edit safety net (so writer-tool changes
        // during this message can be rewound). No-op when no store is attached.
        let cp_turn = self
            .turn_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let attached = {
            let cp = self.security.checkpoint_store.read().as_ref().cloned();
            if let Some(cp) = cp {
                cp.begin(cp_turn, &msg.content);
                true
            } else {
                false
            }
        };

        info!(
            "[AgentLoop] Processing message from {}:{}: {}",
            msg.channel,
            msg.sender_id,
            truncate(&msg.content, 80)
        );
        attached.then_some(cp_turn)
    }

    /// Resolve (agent_id, session_key) for a message (V5: extracted verbatim
    /// from the routing block of the pre-split `process_inbound_message`).
    pub(crate) fn route_message(
        &self,
        msg: &nemesis_types::channel::InboundMessage,
    ) -> (String, String) {
        // Resolve agent and session via route resolver.
        // Mirrors Go's processMessage: al.registry.ResolveRoute(RouteInput{...})
        let (agent_id, session_key) = if let Some(ref resolver) = self.route_resolver {
            // Build the routing input from message metadata, matching Go's extractPeer/extractParentPeer.
            let peer_kind = msg.metadata.get("peer_kind").cloned();
            let peer_id = msg.metadata.get("peer_id").cloned().or_else(|| {
                // Fallback: if peer_kind is "direct" use sender_id, else use chat_id
                if let Some(kind) = &peer_kind {
                    if kind == "direct" {
                        Some(msg.sender_id.clone())
                    } else {
                        Some(msg.chat_id.clone())
                    }
                } else {
                    None
                }
            });
            let parent_peer_kind = msg.metadata.get("parent_peer_kind").cloned();
            let parent_peer_id = msg.metadata.get("parent_peer_id").cloned();

            let route_input = RoutingRouteInput {
                channel: msg.channel.clone(),
                account_id: msg.metadata.get("account_id").cloned().unwrap_or_default(),
                peer_kind,
                peer_id,
                parent_peer_kind,
                parent_peer_id,
                guild_id: msg.metadata.get("guild_id").cloned(),
                team_id: msg.metadata.get("team_id").cloned(),
                identity_links: std::collections::HashMap::new(),
            };
            let route = resolver.resolve(&route_input);

            // Use routed session key, but honor pre-set agent-scoped keys
            // (mirrors Go's logic for ProcessDirect/cron).
            let session_key =
                if !msg.session_key.is_empty() && msg.session_key.starts_with("agent:") {
                    msg.session_key.clone()
                } else {
                    route.session_key.clone()
                };

            info!(
                "[AgentLoop] Routed message: agent_id={}, session_key={}, matched_by={}",
                route.agent_id, session_key, route.matched_by
            );

            (route.agent_id, session_key)
        } else {
            // Fallback when no route resolver is configured (standalone mode).
            let agent_id = self
                .registry
                .as_ref()
                .and_then(|r| r.default_agent_id())
                .unwrap_or_else(|| "main".to_string());

            let peer = extract_peer(msg);
            let session_key =
                if !msg.session_key.is_empty() && msg.session_key.starts_with("agent:") {
                    msg.session_key.clone()
                } else {
                    format!("{}:{}", msg.channel, peer)
                };

            info!(
                "[AgentLoop] Routed message (no resolver): agent_id={}, session_key={}",
                agent_id, session_key
            );

            (agent_id, session_key)
        };
        (agent_id, session_key)
    }

    /// Synchronous inbound gate (V5 / B5 真机揭的接线 bug，2026-08-23):
    /// 生产 pump（`run_bus_*`）原是纯串行消费者——`process_inbound_message`
    /// 把整个回合 await 到底才 recv 下一条消息，busy 闸门对用户消息永远
    /// 不可达（U7 inbox 因此在真机上从未生效，只有单测手动占住 session
    /// 验证过）。Queue/Steer 模式下 pump 现在同步跑这个闸门（同 session
    /// 保序：忙时的第二条消息必见 busy → 入队/回执），回合在独立 task
    /// 里并发跑；Reject 模式维持原串行路径（行为零变化）。
    ///
    /// D-2（2026-09-23 多会话并行清账）：忙时回绝的诚实留痕。被弹回
    /// （Reject）或因 inbox 满被拒（Queue/Steer）的消息此前零落盘——前端
    /// 切会话即「消失」，违反诚实失败契约。user 行 + 回绝回实行**成对**
    /// 落盘（无悬空 user 行，jsonl 轮次完整；重建上下文时模型能看到「这
    /// 条消息当时收到了、被回绝」）。同 process_admitted 早落盘块的物化
    /// 语义：首行落盘 = 会话物化 → emit_session_created。gate 是同步
    /// 分类点，落盘是同步文件写，不引入 async 面。
    fn persist_busy_refusal(
        &self,
        msg: &nemesis_types::channel::InboundMessage,
        session_key: &str,
        bounce: &str,
    ) {
        let log_existed = Self::session_log_exists_before_append(session_key);
        let cron_job_id = msg.metadata.get("cron_job_id").map(|s| s.as_str());
        let cron_job_name = msg.metadata.get("cron_job_name").map(|s| s.as_str());
        crate::chat_log::append_chat_log_meta(
            session_key,
            "user",
            &msg.content,
            &crate::chat_log::ChatLogMeta {
                model: None,
                cron_job_id,
                cron_job_name,
                images: &[],
                file_changes: &[],
                checkpoint_turn: None, // 回绝消息不开始 turn，无 checkpoint 标记
            },
        );
        crate::chat_log::append_chat_log_meta(
            session_key,
            "assistant",
            bounce,
            &crate::chat_log::ChatLogMeta {
                model: Some(&self.current_display_model()),
                cron_job_id,
                cron_job_name,
                images: &[],
                file_changes: &[],
                checkpoint_turn: None,
            },
        );
        if !log_existed {
            self.emit_session_created(session_key);
        }
        if let Some(ref store) = self.session_store {
            store.add_message(session_key, "user", &msg.content);
            store.add_message(session_key, "assistant", bounce);
        }
    }

    /// 闸门只做同步分类 + session 获取；async 处理（system/history/回合
    /// 本体）留给 tail。slash 命令在这里同步执行并短路（原路径在 busy
    /// 检查前同步返回；且命令可能有副作用，tail 不得重跑）。
    pub(crate) fn gate_inbound(&self, msg: &nemesis_types::channel::InboundMessage) -> GateOutcome {
        // E3：begin 的 turn 序号在此捕获（admitted 链穿针用；其余 outcome
        // 不产生带 checkpoint_turn 标记的行，值自然丢弃）。
        let cp_turn = self.turn_preamble(msg);

        // Route system messages.
        if msg.channel == "system" {
            // Cluster continuation — the pump handles via dispatch_continuation.
            if msg
                .sender_id
                .starts_with(nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX)
            {
                let task_id =
                    &msg.sender_id[nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX.len()..];
                debug!(
                    "[AgentLoop] Cluster continuation message intercepted, task_id={}",
                    task_id
                );
                return GateOutcome::Continuation(task_id.to_string());
            }
            // G4 (devtool-upgrade 阶段 3)：后台 subagent 完成回灌 —— 与集群
            // 续行同构：同走 dispatch_continuation → handle_cluster_continuation
            // （快照加载 + 续行 + 持久化 + 自清，全复用零新路径）。
            if msg
                .sender_id
                .starts_with(nemesis_types::constants::SUBAGENT_CONTINUATION_PREFIX)
            {
                let task_id =
                    &msg.sender_id[nemesis_types::constants::SUBAGENT_CONTINUATION_PREFIX.len()..];
                debug!(
                    "[AgentLoop] Background subagent continuation intercepted, task_id={}",
                    task_id
                );
                return GateOutcome::Continuation(task_id.to_string());
            }
            return GateOutcome::Ungated;
        }

        // History request.
        if let Some(request_type) = msg.metadata.get("request_type")
            && request_type == "history"
        {
            return GateOutcome::Ungated;
        }

        // E6 (2026-09-05): /compact /clear — 会话维护命令。gate 同步短路点
        // 拿不到 instance（摘要 LLM 是 async），所以 mint 一个 Maintenance
        // outcome 交给 tail；会话在此获取（维护期间并发用户消息走 busy
        // 闸），尾巴负责释放。先于普通 slash 检查（维护命令有副作用，不是
        // 静态回复）。
        if let Some((kind, receipt)) = parse_maintenance_command(&msg.content) {
            let (_, session_key) = self.route_message(msg);
            if !self.try_acquire_session(&session_key) {
                return GateOutcome::Immediate {
                    agent_id: String::new(),
                    response: "⏳ 会话正在处理消息，本次 /compact /clear 已忽略，请稍后再试"
                        .to_string(),
                };
            }
            return GateOutcome::Maintenance {
                kind,
                session_key,
                receipt,
            };
        }

        // K4 (devtool-upgrade 阶段 7): 用户直发远程编码任务派发——
        // `/build <task> repo:<node>`。先于 F1 精确匹配（带参形态与裸
        // `/build` 天然互斥）；会话在此获取（Maintenance 同款，防并发回
        // 合踩会话）。session_key 用 route_message 现取。
        if let Some((task_text, node_id)) = parse_user_dispatch(&msg.content) {
            let (_, session_key) = self.route_message(msg);
            if !self.try_acquire_session(&session_key) {
                return GateOutcome::Immediate {
                    agent_id: String::new(),
                    response: "⏳ 会话正在处理消息，本次远程派发已忽略，请稍后再试".to_string(),
                };
            }
            return GateOutcome::UserDispatch {
                task_text,
                node_id,
                session_key,
            };
        }

        // K4 (b) (devtool-upgrade 阶段 7): IM 审批卡回执短路——
        // `/approve <id>` / `/deny <id>`。真实裁决由 gateway 的审批回执
        // watcher（bus 订阅方）完成；loop 侧只做静态确认，**吞掉**这条
        // 消息不让 agent 对「同意 xxx」式回执起一轮无意义对话（裁决结果
        // 由 watcher 以显式消息回执到同一对话）。loop 无 pending 注册表，
        // 无效/过期 id 的诚实提示由 watcher 负责（「未找到待审批请求」）。
        if let Some(response) = parse_approval_reply(&msg.content) {
            return GateOutcome::Immediate {
                agent_id: String::new(),
                response,
            };
        }

        // F1 (devtool-upgrade 阶段 4): /plan /build — 模式切换命令。与普通
        // slash 同为 gate 同步短路（改的是 loop 级 RwLock，无 async），但要
        // 先于 handle_command_with_context（那是静态回复表，不带 chat_id，
        // 而模式切换要发布带路由信息的 ModeChanged 事件）。session_key 用
        // route_message 现取（/compact 臂同款）；无订阅者时空转是常态。
        if msg.content.trim() == "/plan" || msg.content.trim() == "/build" {
            let want = if msg.content.trim() == "/plan" {
                crate::types::AgentMode::Plan
            } else {
                crate::types::AgentMode::Build
            };
            let (agent_id, session_key) = self.route_message(msg);
            self.set_mode_with_event(want, &session_key, &msg.chat_id);
            let response = match want {
                crate::types::AgentMode::Plan => "✓ 已切换到 Plan 模式：文件修改类工具已停用（plans/ 目录写放行）。请以文本形式呈现计划；完成规划后用 /build 切回。".to_string(),
                crate::types::AgentMode::Build => "✓ 已切换回 Build 模式：全量工具恢复。".to_string(),
            };
            return GateOutcome::Immediate { agent_id, response };
        }

        // 件4（2026-09-24 三合一收口 §6）：`/discipline on|off [理由]` —
        // 纪律参与切换。与 /plan /build 同为 gate 同步短路（改 loop 级共享
        // 态，无 async）；会话键 route_message 现取（D3：交互会话 = 会话键
        // 态）。off 带理由 → waive 入审计（`.discipline/waive-audit.jsonl`）。
        // 子系统未启用（总开关 false）→ 诚实提示，不改状态。
        let trimmed = msg.content.trim();
        if trimmed == "/discipline" || trimmed.starts_with("/discipline ") {
            let args = trimmed["/discipline".len()..].trim();
            let (sub, rest) = match args.split_once(char::is_whitespace) {
                Some((s, r)) => (s, r.trim()),
                None => (args, ""),
            };
            let (_, session_key) = self.route_message(msg);
            let Some(state) = self.discipline_state() else {
                return GateOutcome::Immediate {
                    agent_id: String::new(),
                    response: "ℹ️ 纪律模式未启用（config: agents.discipline.enabled=false）。\
                               打开后可用 `/discipline on` 进入、`/discipline off [理由]` 退出。"
                        .to_string(),
                };
            };
            let response = match sub {
                "" | "on" => {
                    let already = state.set_interactive(&session_key);
                    if already {
                        "ℹ️ 本会话已处于纪律模式。".to_string()
                    } else {
                        format!(
                            "✓ 纪律模式已开启（本会话）：文件修改前需先写 {}/{}（六字段声明）；\
                             收尾时自动跑证伪命令（预算 2 次）。",
                            crate::discipline::DISCIPLINE_DIR,
                            crate::discipline::DECLARATION_FILE
                        )
                    }
                }
                "off" => {
                    let reason = if rest.is_empty() { None } else { Some(rest) };
                    state.clear_interactive(&session_key, reason);
                    match reason {
                        Some(why) => format!("✓ 纪律模式已退出（本会话）。理由已留痕：{why}"),
                        None => "✓ 纪律模式已退出（本会话）。".to_string(),
                    }
                }
                other => format!("Usage: /discipline [on|off [理由]]（未知子命令：{other}）"),
            };
            return GateOutcome::Immediate {
                agent_id: String::new(),
                response,
            };
        }

        // Slash commands.
        if let Some(response) = self.handle_command_with_context(&msg.content, &msg.channel) {
            return GateOutcome::Immediate {
                agent_id: String::new(),
                response,
            };
        }

        let (agent_id, session_key) = self.route_message(msg);

        // Session busy check — I1 (U7) mode-aware:
        //   Reject: session-level busy disposition — immediate BUSY_MESSAGE
        //   bounce, WITH honest trace (user row + busy bounce row appended
        //   to chat_log; D-2 2026-09-23 — bounced messages used to vanish
        //   without any trace). The pump no longer serializes on this mode.
        //   Queue/Steer: park the message in the session inbox instead of
        //   bouncing. The running turn claims it (steer: next LLM call; queue:
        //   turn end starts a new one with the head). Capacity-bounded with a
        //   clear receipt so the rule is discoverable.
        if !self.try_acquire_session(&session_key) {
            match self.concurrent_mode {
                ConcurrentMode::Reject => {
                    warn!(
                        "[AgentLoop] Session busy, returning busy message: session_key={}, mode={:?}",
                        session_key, self.concurrent_mode
                    );
                    // D-2（2026-09-23 多会话并行清账）：诚实留痕（见
                    // persist_busy_refusal）——被弹回的消息此前零落盘。
                    self.persist_busy_refusal(msg, &session_key, BUSY_MESSAGE);
                    return GateOutcome::Immediate {
                        agent_id,
                        response: BUSY_MESSAGE.to_string(),
                    };
                }
                ConcurrentMode::Queue | ConcurrentMode::Steer => {
                    let queued = crate::inbox::QueuedMessage {
                        msg: nemesis_types::channel::InboundMessage {
                            channel: msg.channel.clone(),
                            sender_id: msg.sender_id.clone(),
                            chat_id: msg.chat_id.clone(),
                            content: msg.content.clone(),
                            media: msg.media.clone(),
                            session_key: msg.session_key.clone(),
                            correlation_id: msg.correlation_id.clone(),
                            metadata: msg.metadata.clone(),
                            voice_playback: msg.voice_playback,
                        },
                        timestamp: chrono::Local::now().to_rfc3339(),
                    };
                    // Second-pass review fix: the `!`-prefix steer channel
                    // exists ONLY in Steer mode. Queue mode is pure
                    // queueing — enqueue_for_mode routes everything to
                    // next_turn when steer is disabled.
                    match self.inbox.enqueue_for_mode(
                        &session_key,
                        queued,
                        self.concurrent_mode == ConcurrentMode::Steer,
                    ) {
                        crate::inbox::EnqueueOutcome::QueuedForNextTurn => {
                            info!(
                                "[AgentLoop] Session busy — message queued for next turn: session_key={}",
                                session_key
                            );
                            return GateOutcome::Immediate {
                                agent_id,
                                response: "⏳ 当前正在处理上一条消息。你的消息已排队，将在本轮结束后继续处理。".to_string(),
                            };
                        }
                        crate::inbox::EnqueueOutcome::QueuedForNextStep => {
                            info!(
                                "[AgentLoop] Session busy — steer message queued for in-turn injection: session_key={}",
                                session_key
                            );
                            return GateOutcome::Immediate {
                                agent_id,
                                response: "⚡ 已接收为紧急插话（消息以 ! 开头），将在 AI 的下一步思考前注入。非紧急消息请去掉 ! 前缀排队等待。".to_string(),
                            };
                        }
                        crate::inbox::EnqueueOutcome::Rejected => {
                            warn!(
                                "[AgentLoop] Session busy and inbox full — message refused: session_key={}",
                                session_key
                            );
                            // D-2 同款诚实留痕：被拒绝的消息也要在历史里可
                            // 追溯（此前只有瞬态回执帧，切会话即「消失」）。
                            self.persist_busy_refusal(
                                msg,
                                &session_key,
                                "⏳ 排队已满，消息未能接收。请等当前任务完成后再发。",
                            );
                            return GateOutcome::Immediate {
                                agent_id,
                                response: "⏳ 排队已满，消息未能接收。请等当前任务完成后再发。"
                                    .to_string(),
                            };
                        }
                    }
                }
            }
        }

        // Create a cancellation token for this session (V5: minted in the
        // gate so the spawned turn tail starts with everything the
        // pre-split monolith had at this point).
        let cancel_token = self.create_cancel_token(&session_key);
        GateOutcome::Admitted(TurnAdmission {
            agent_id,
            session_key,
            cancel_token,
            cp_turn,
        })
    }

    /// Ungated tail: system (non-continuation) + history-request handling
    /// (V5: extracted verbatim from the pre-split monolith's early returns).
    pub(crate) async fn process_ungated(
        &self,
        msg: &nemesis_types::channel::InboundMessage,
    ) -> (String, String, Option<String>) {
        if msg.channel == "system" {
            let (resp, err) = self.process_system_message(msg).await;
            return (String::new(), resp, err);
        }

        // History request.
        if let Some(request_type) = msg.metadata.get("request_type")
            && request_type == "history"
        {
            self.handle_history_request(msg).await;
            return (String::new(), String::new(), None);
        }

        // The gate classified everything else; unreachable in practice.
        (String::new(), String::new(), None)
    }

    /// Admitted turn tail (V5): preprocess → run turn → cleanup/release →
    /// inbox drain. The session is ALREADY acquired and the cancel token
    /// minted by `gate_inbound`; this fn owns release + reinject (the drain
    /// block runs unconditionally after the turn so an admitted session can
    /// never leak busy).
    pub(crate) async fn process_admitted(
        &self,
        msg: &nemesis_types::channel::InboundMessage,
        admission: TurnAdmission,
    ) -> (String, String, Option<String>) {
        let TurnAdmission {
            agent_id,
            session_key,
            cancel_token,
            cp_turn,
        } = admission;

        let voice_playback = msg.voice_playback.unwrap_or(false);

        // B1（2026-09-22 聊天切会话竞态）：user 行落盘从 run_agent_loop_internal
        // 开头提前到本点——预处理（expand_at_files / fetch_url_media /
        // attach_turn_images）之前，「消息被系统接纳」即落盘。turn 进行中
        // （分钟级）用户切走路由再切回，前端 loadHistory 立即能拉到本轮
        // user 行；此前落盘点在预处理链之后（带图/@引用消息可达秒级），
        // 早切回读到空历史、无悬空占位（现象 B）。
        // 诚实取舍：此处 content 是原始文本（@ 未展开、图片未注记），
        // images 置空——带图消息的 chat_log user 行缺 images 字段，历史
        // 视图无图片 chip（instance / session_log 不受影响）。
        // HD「一轮 = jsonl 恰好 +2 行」不变量保持：行数与顺序不变，仅
        // user 行时点提前。cron 元数据在 gate 前已随 metadata 到位，照读。
        let cron_job_id = msg.metadata.get("cron_job_id").map(|s| s.as_str());
        let cron_job_name = msg.metadata.get("cron_job_name").map(|s| s.as_str());
        {
            let log_existed = Self::session_log_exists_before_append(&session_key);
            crate::chat_log::append_chat_log_meta(
                &session_key,
                "user",
                &msg.content,
                &crate::chat_log::ChatLogMeta {
                    model: None,
                    cron_job_id,
                    cron_job_name,
                    images: &[],
                    file_changes: &[],
                    checkpoint_turn: cp_turn,
                },
            );
            if !log_existed {
                self.emit_session_created(&session_key);
            }
        }

        // I2（@文件引用）：内联被引用文件内容。相对路径以 workspace 根为基准
        // 解析（与 read_file 同源，不再是 cwd 漂移基准）；安全闸与图片附加
        // 同一套管线（挂真实 SecurityPlugin 时 Layer 7 block_in_place，
        // multi_thread runtime 为生产前提——同 T5 注释）。
        let at_base = self
            .workspace_root
            .read()
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let processed_content = crate::message_preprocess::expand_at_files(
            &msg.content,
            &at_base,
            &msg.channel,
            #[cfg(feature = "security")]
            self.security.security_plugin.as_deref(),
            #[cfg(not(feature = "security"))]
            None,
        );
        // T5（多模态，goal 2026-09-03）：统一图片附加——文本点名路径（T4 检测）
        // + media 引用（T7/T8/T9 落盘产物）统一过安全闸（8 层管线全跑）产出
        // **路径引用**；失败/拒绝诚实注明合并进轮次文本。注意：挂了真实
        // SecurityPlugin 时管线 Layer 7 会 block_in_place，须 multi_thread
        // runtime（gateway 生产 runtime 即是）。
        // T9：URL media 先经 fetch_url_media 预取（SSRF 闸 + http-pool 下载 →
        // uploads 落盘改写为本地路径引用），再统一走同步附加链；失败注记随
        // 附加注记一并合并进轮次文本。
        let ws_for_uploads = self.workspace_root.read().clone();
        let uploads_base =
            ws_for_uploads.unwrap_or_else(|| nemesis_path::default_path_manager().workspace());
        let uploads_dir = nemesis_path::resolve_uploads_dir_in_workspace(&uploads_base);
        let (media_for_attach, url_notes) = crate::image_attach::fetch_url_media(
            &msg.media,
            &uploads_dir,
            #[cfg(feature = "security")]
            self.security
                .security_plugin
                .as_deref()
                .and_then(|p| p.ssrf_guard()),
            #[cfg(not(feature = "security"))]
            None,
        )
        .await;
        // J6：超限图片降采样开关（默认开）——产物落同一 uploads 目录（TTL 复用）。
        let downscale_dir = self.current_image_downscale().then(|| uploads_dir.clone());
        let attach = crate::image_attach::attach_turn_images(
            &processed_content,
            &media_for_attach,
            self.workspace_root.read().as_deref(),
            downscale_dir.as_deref(),
            &msg.channel,
            #[cfg(feature = "security")]
            self.security.security_plugin.as_deref(),
            #[cfg(not(feature = "security"))]
            None,
        );
        let processed_content = attach.merge_into_text(processed_content);
        let processed_content = crate::image_attach::AttachOutcome {
            attached: Vec::new(),
            notes: url_notes,
        }
        .merge_into_text(processed_content);
        let image_refs = attach.ref_strings();
        // T3 (U12): per-fire tool-round budget set by the gateway cron fire
        // handler from the job's max_rounds payload. Absent/unparsable → None
        // → the turn runs under the global max_turns.
        let cron_max_rounds = msg
            .metadata
            .get("cron_max_rounds")
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|v| *v > 0);
        // I5：本轮客户端上报的打开文件进 per-turn 状态（渲染在 build_messages
        // 的 merged digest；出轮即清——与下方 run 后的 clear 词法配对，cron/
        // heartbeat/continuation 等非 process_admitted 路径天然读不到）。
        *self.pending_open_files.write() =
            nemesis_types::channel::open_files_from_metadata(&msg.metadata);
        // 对话生成：workflow_edit 注入目标同款 per-turn 生命周期（出轮 clear
        // 与上方配对）。
        *self.pending_workflow_edit.write() =
            nemesis_types::channel::workflow_edit_from_metadata(&msg.metadata);
        let result = self
            .run_agent_loop_internal(
                &session_key,
                &processed_content,
                &msg.channel,
                &msg.chat_id,
                voice_playback,
                &cancel_token,
                cron_job_id,
                cron_job_name,
                cron_max_rounds,
                &image_refs,
            )
            .await;

        // Clean up cancellation token and release session.
        self.remove_cancel_token(&session_key);
        self.release_session(&session_key);
        // I5：轮结束清打开文件状态（与上方 set 词法配对，防跨轮陈旧泄漏）。
        self.pending_open_files.write().clear();
        // 对话生成：轮结束清 workflow_edit 目标（同款配对）。
        *self.pending_workflow_edit.write() = None;

        // I1 (U7) post-turn inbox handling:
        //   - Unconsumed next-step (steer) messages ALWAYS transfer back to
        //     the next-turn queue — both for cancelled turns AND for turns
        //     that finished normally with a steer arriving too late for the
        //     escape hatch (second-pass review fix: the cancelled-only
        //     transfer left stale steers in next_step, which a LATER turn
        //     would claim out of context). Transferred messages become the
        //     next turn's input — nothing is lost, nothing arrives stale.
        //   - Completed turn with queued next-turn messages: requeue the head
        //     through the normal inbound path (fresh busy acquire),
        //     serialized behind this turn because the session was already
        //     released.
        self.inbox.transfer_next_step_to_next_turn(&session_key);
        if matches!(
            self.concurrent_mode,
            ConcurrentMode::Queue | ConcurrentMode::Steer
        ) {
            if let Some(head) = self.inbox.claim_next_turn_head(&session_key) {
                info!(
                    "[AgentLoop] processing queued next-turn message: session_key={}, len={}",
                    session_key,
                    head.msg.content.len()
                );
                // ROUND-5 REVIEW FIX: re-inject the queued head into the
                // agent's own inbound mpsc (the same one `run_bus_*`
                // consumes) instead of recursing inline. The normal consumer
                // then applies the SAME post-processing as any other message:
                // [rpc:{cid}] correlation prefix, sent_in_round check+clear,
                // error → "Error processing message" + capture flush, and
                // meta.model. The previous inline recursion published the
                // reply raw — an rpc-channel queued reply lost its prefix
                // (RPCChannel::send drops unprefixed replies) and a FAILED
                // queued turn produced no user-visible output at all.
                // H3 (full review): the queued message wraps the ORIGINAL
                // InboundMessage whole (round-5), so replaying is a clone
                // with only the session_key normalized to this queue's key.
                //
                // NOT bus.publish_inbound (broadcast): that would re-match
                // workflow message-triggers for an already-matched message
                // (double firing). The agent's private mpsc has no other
                // subscriber.
                //
                // try_send (never a blocking await on a full channel from
                // inside the sole consumer task): capacity 1024 vs inbox cap
                // 8/session makes a full channel unreachable in practice; on
                // the theoretical overflow the message is logged and dropped
                // rather than deadlocking the loop.
                let mut inbound = head.msg.clone();
                inbound.session_key = session_key.to_string();
                let reinject = self.reinject_tx.read().clone();
                match reinject {
                    Some(tx) => {
                        if let Err(e) = tx.try_send(inbound) {
                            // No consumer (standalone/tests without run_bus)
                            // or channel full — surface loudly either way.
                            warn!(
                                "[AgentLoop] failed to re-inject queued message (session_key={}, err={}) — message dropped",
                                session_key, e
                            );
                        }
                    }
                    None => {
                        // Standalone mode / tests without the mpsc wired:
                        // process inline as before so the message is not
                        // silently lost, with the error path at least
                        // converted to a user-visible reply.
                        warn!(
                            "[AgentLoop] reinject_tx not set — processing queued message inline (post-processing skipped), session_key={}",
                            session_key
                        );
                        let fut = Box::pin(self.process_inbound_message(&inbound));
                        let (_id2, resp2, err2) = fut.await;
                        let content = match err2 {
                            Some(e) => format!("Error processing message: {}", e),
                            None => resp2,
                        };
                        if !content.is_empty()
                            && let Some(ref tx) = self.outbound_tx
                        {
                            let outbound = nemesis_types::channel::OutboundMessage {
                                channel: head.msg.channel.clone(),
                                chat_id: head.msg.chat_id.clone(),
                                content,
                                message_type: String::new(),
                                meta: nemesis_types::channel::OutboundMeta {
                                    model: None,
                                    session_key: Some(session_key.clone()),
                                    source_node: None,
                                },
                            };
                            let _ = tx.send(outbound).await;
                        }
                    }
                }
            }
            // Drop empty queues (housekeeping; keeps the map bounded).
            if self.inbox.pending(&session_key) == (0, 0) {
                self.inbox.clear(&session_key);
            }
        }

        match result {
            Ok(response) => (agent_id, response, None),
            Err(e) => (agent_id, String::new(), Some(e)),
        }
    }

    // -----------------------------------------------------------------------
    // System message routing
    // -----------------------------------------------------------------------

    /// Get or create the busy state for a session.
    /// Mirrors Go's `getSessionBusyState()`.
    pub fn get_session_busy_state(&self, session_key: &str) -> (bool, usize) {
        let map = self.session_busy.lock();
        match map.get(session_key) {
            Some(state) => (state.busy, state.queue_length),
            None => (false, 0),
        }
    }

    /// Try to acquire a session for processing.
    /// Returns true if acquired, false if busy (and queue is full in queue mode).
    /// Mirrors Go's `tryAcquireSession()`.
    pub fn try_acquire_session(&self, session_key: &str) -> bool {
        let mut map = self.session_busy.lock();
        let state = map.entry(session_key.to_string()).or_default();

        if !state.busy {
            state.busy = true;
            return true;
        }

        // Session is busy — reject HERE. Queueing is NOT done at this layer:
        // the old Queue path incremented queue_length WITHOUT storing the
        // message, and release_session kept busy while queue_length>0 → the
        // session could deadlock (no turn ever acquires to drain the
        // counter). Busy-queueing lives in `crate::inbox` (FIFO per session),
        // driven by `gate_inbound`'s busy branch — reachable since V5's
        // gate-in-pump restructure (the gate runs before the turn task is
        // spawned, so a busy session is detected while its turn is still
        // running). (queue_length stays 0, so release_session naturally sets
        // busy=false.)
        let _ = self.concurrent_mode;
        false
    }

    /// Release a session after processing.
    /// Returns true if there are queued requests remaining.
    /// Mirrors Go's `releaseSession()`.
    pub fn release_session(&self, session_key: &str) -> bool {
        let mut map = self.session_busy.lock();
        if let Some(state) = map.get_mut(session_key) {
            if state.queue_length > 0 {
                state.queue_length -= 1;
                // Keep busy since there are queued requests.
                return true;
            }
            state.busy = false;
        }
        false
    }

    /// Check whether a session is currently busy.
    pub fn is_session_busy(&self, session_key: &str) -> bool {
        let map = self.session_busy.lock();
        map.get(session_key).is_some_and(|s| s.busy)
    }

    /// Get the queue length for a session.
    pub fn session_queue_length(&self, session_key: &str) -> usize {
        let map = self.session_busy.lock();
        map.get(session_key).map_or(0, |s| s.queue_length)
    }

    /// U7 dashboard visibility (G1): one-call snapshot of a session's inbox
    /// state for the `agent.inbox_status` WSAPI command. Read-only — claims
    /// and transfers stay exclusively in the turn lifecycle paths.
    pub fn inbox_status(&self, session_key: &str) -> crate::inbox::InboxStatus {
        let (next_turn, next_step) = self.inbox.pending(session_key);
        crate::inbox::InboxStatus {
            next_turn,
            next_step,
            capacity: self.inbox.capacity(),
            busy: self.is_session_busy(session_key),
            mode: match self.concurrent_mode {
                ConcurrentMode::Reject => "reject",
                ConcurrentMode::Queue => "queue",
                ConcurrentMode::Steer => "steer",
            },
        }
    }

    // -----------------------------------------------------------------------
    // Session cancellation
    // -----------------------------------------------------------------------

    /// Cancel an in-progress session by session_key.
    ///
    /// If the session is currently being processed by the LLM loop, this
    /// triggers the cancellation token, causing the loop to break at the
    /// next check point (after the current LLM call or tool execution).
    ///
    /// Returns true if a cancellation token was found and cancelled.
    pub fn cancel_session(&self, session_key: &str) -> bool {
        if let Some(token) = self.cancel_tokens.get(session_key) {
            token.cancel();
            info!(
                "[AgentLoop] Session cancellation requested: {}",
                session_key
            );
            true
        } else {
            debug!("[AgentLoop] No active session to cancel: {}", session_key);
            false
        }
    }

    /// Cancel all in-progress sessions.
    ///
    /// Returns the number of sessions that were cancelled.
    pub fn cancel_all_sessions(&self) -> usize {
        let mut count = 0;
        for entry in self.cancel_tokens.iter() {
            entry.value().cancel();
            count += 1;
        }
        if count > 0 {
            info!("[AgentLoop] Cancelled {} active session(s)", count);
        }
        count
    }

    /// Create and store a cancellation token for a session.
    /// Returns the token for the caller to pass into the processing pipeline.
    fn create_cancel_token(&self, session_key: &str) -> tokio_util::sync::CancellationToken {
        let token = tokio_util::sync::CancellationToken::new();
        self.cancel_tokens
            .insert(session_key.to_string(), token.clone());
        token
    }

    /// Remove the cancellation token for a session after processing completes.
    fn remove_cancel_token(&self, session_key: &str) {
        self.cancel_tokens.remove(session_key);
    }

    // -----------------------------------------------------------------------
    // Summarization
    // -----------------------------------------------------------------------
}

// ---------------------------------------------------------------------------
// 自由函数归位（P1-c 自 loop.rs 根搬迁；仅增 pub(crate) 可见性标注）
// ---------------------------------------------------------------------------

/// Extract a peer identifier from an inbound message.
///
/// Looks at metadata fields to determine the originating peer.
/// Mirrors Go's `extractPeer`:
/// - If `peer_kind` is set, uses `peer_id` (falls back to sender_id for "direct", chat_id otherwise)
/// - If no metadata, returns sender_id
pub fn extract_peer(msg: &nemesis_types::channel::InboundMessage) -> String {
    if let Some(peer_kind) = msg.metadata.get("peer_kind")
        && !peer_kind.is_empty()
    {
        let peer_id = msg.metadata.get("peer_id").cloned().unwrap_or_else(|| {
            if peer_kind == "direct" {
                msg.sender_id.clone()
            } else {
                msg.chat_id.clone()
            }
        });
        return format!("{}:{}", peer_kind, peer_id);
    }
    msg.sender_id.clone()
}

/// Extract the parent peer identifier from an inbound message.
///
/// Used for routing in nested or forwarded messages.
/// Mirrors Go's `extractParentPeer`.
#[cfg(test)]
pub fn extract_parent_peer(msg: &nemesis_types::channel::InboundMessage) -> Option<String> {
    let parent_kind = msg.metadata.get("parent_peer_kind")?;
    let parent_id = msg.metadata.get("parent_peer_id")?;
    if parent_kind.is_empty() || parent_id.is_empty() {
        return None;
    }
    Some(format!("{}:{}", parent_kind, parent_id))
}

/// Route input for agent resolution.
///
/// This is a legacy compatibility type. For new code, use
/// [`nemesis_routing::RouteInput`] directly with [`RouteResolver`].
#[cfg(test)]
#[derive(Debug, Clone)]
pub struct RouteInput {
    pub channel: String,
    pub account_id: Option<String>,
    pub peer: String,
    pub parent_peer: Option<String>,
    pub guild_id: Option<String>,
    pub team_id: Option<String>,
}

/// Resolved route for a message.
///
/// This is a legacy compatibility type. For new code, use
/// [`nemesis_routing::ResolvedRoute`] directly.
#[cfg(test)]
#[derive(Debug, Clone)]
pub struct RouteOutput {
    pub agent_id: String,
    pub session_key: String,
    pub matched_by: String,
}

/// Resolve the route for a message to determine which agent and session to use.
///
/// Uses the full `RouteResolver` with a default single-agent configuration.
/// The peer field is parsed from the format "kind:id" to extract peer_kind and peer_id.
/// Mirrors Go's `al.registry.ResolveRoute(routing.RouteInput{...})`.
#[cfg(test)]
pub fn resolve_route(input: &RouteInput) -> RouteOutput {
    // Parse peer from "kind:id" format (as produced by extract_peer).
    let (peer_kind, peer_id) = if let Some(colon_pos) = input.peer.find(':') {
        let kind = input.peer[..colon_pos].to_string();
        let id = input.peer[colon_pos + 1..].to_string();
        (Some(kind), Some(id))
    } else {
        // Treat as just an ID with no kind
        (None, Some(input.peer.clone()))
    };

    // Parse parent_peer from "kind:id" format.
    let (parent_peer_kind, parent_peer_id) = input
        .parent_peer
        .as_ref()
        .and_then(|pp| {
            pp.find(':').map(|colon_pos| {
                (
                    Some(pp[..colon_pos].to_string()),
                    Some(pp[colon_pos + 1..].to_string()),
                )
            })
        })
        .unwrap_or((None, None));

    let route_input = RoutingRouteInput {
        channel: input.channel.clone(),
        account_id: input.account_id.clone().unwrap_or_default(),
        peer_kind,
        peer_id,
        parent_peer_kind,
        parent_peer_id,
        guild_id: input.guild_id.clone(),
        team_id: input.team_id.clone(),
        identity_links: std::collections::HashMap::new(),
    };

    // Build a default resolver with a single "main" agent and no bindings.
    let config = RouteConfig {
        bindings: Vec::new(),
        agents: vec![AgentDef {
            id: "main".to_string(),
            is_default: true,
        }],
        dm_scope: "main".to_string(),
    };
    let resolver = RouteResolver::new(config);
    let route = resolver.resolve(&route_input);

    RouteOutput {
        agent_id: route.agent_id,
        session_key: route.session_key,
        matched_by: route.matched_by,
    }
}

//! 总线消费环：is_internal_channel、BUSY_MESSAGE、parse_concurrent_mode、SessionBusyTracker、new_bus/run_bus 系、stop/is_running、spawn_turn_task、finish_message、dispatch_continuation、emit_observer_sync。
//!
//! P1 自 `loop.rs` 物理搬迁（docs/PLAN/2026-09-23_agentloop-god-object-decomposition.md §3.2）；语义零变化。
use super::prelude::*;
use super::*;

/// Check if a channel is internal (not user-facing).
pub fn is_internal_channel(channel: &str) -> bool {
    matches!(channel, "cli" | "system" | "subagent")
}

// ---------------------------------------------------------------------------
// Session busy state management
// ---------------------------------------------------------------------------

/// Busy message returned when session is busy.
pub const BUSY_MESSAGE: &str =
    "\u{23f3} AI is processing a previous request, please try again later";
/// Parse the config string. Unknown values fall back to Reject (fail-safe
/// to legacy behavior) with a warn at the call site.
pub fn parse_concurrent_mode(s: &str) -> ConcurrentMode {
    match s.trim().to_lowercase().as_str() {
        "queue" => ConcurrentMode::Queue,
        "steer" => ConcurrentMode::Steer,
        _ => ConcurrentMode::Reject,
    }
}

/// Tracks busy state for sessions.
pub struct SessionBusyTracker {
    busy: dashmap::DashSet<String>,
    #[allow(dead_code)] // Reserved for future concurrent-mode-aware queue logic
    mode: ConcurrentMode,
    #[allow(dead_code)] // Reserved for future concurrent-mode-aware queue logic
    queue_size: usize,
}

impl SessionBusyTracker {
    /// Create a new tracker with the given mode.
    pub fn new(mode: ConcurrentMode, queue_size: usize) -> Self {
        Self {
            busy: dashmap::DashSet::new(),
            mode,
            queue_size,
        }
    }

    /// Try to acquire a session for processing. Returns false if busy and mode is Reject.
    pub fn try_acquire(&self, session_key: &str) -> bool {
        if self.busy.contains(session_key) {
            return false;
        }
        self.busy.insert(session_key.to_string());
        true
    }

    /// Release a session after processing.
    pub fn release(&self, session_key: &str) {
        self.busy.remove(session_key);
    }

    /// Check whether a session is currently busy.
    pub fn is_busy(&self, session_key: &str) -> bool {
        self.busy.contains(session_key)
    }
}

impl AgentLoop {
    /// Create a new agent loop in bus-integrated mode.
    ///
    /// This mirrors Go's `NewAgentLoop()`. It sets up:
    /// - Agent registry with a default "main" agent
    /// - Session store for persistent history
    /// - Outbound channel for publishing responses
    /// - Session busy tracker
    /// - Route resolver with a default single-agent configuration
    pub fn new_bus(
        provider: Box<dyn LlmProvider>,
        config: AgentConfig,
        outbound_tx: tokio::sync::mpsc::Sender<nemesis_types::channel::OutboundMessage>,
        concurrent_mode: ConcurrentMode,
        queue_size: usize,
        max_continuation_permits: usize,
    ) -> Self {
        let registry = Arc::new(AgentRegistry::with_default(config.clone()));
        let session_store = Arc::new(SessionStore::new_in_memory());

        // Build a default route resolver with a single "main" agent.
        // This can be overridden via set_route_resolver() for multi-agent setups.
        let default_route_config = RouteConfig {
            bindings: Vec::new(),
            agents: vec![AgentDef {
                id: "main".to_string(),
                is_default: true,
            }],
            dm_scope: "main".to_string(),
        };

        let continuation_semaphore = if max_continuation_permits > 0 {
            Some(Arc::new(tokio::sync::Semaphore::new(
                max_continuation_permits,
            )))
        } else {
            None
        };

        let model = config.model.clone();
        info!(
            "[AgentLoop] Created in bus mode, model={}, concurrent_mode={:?}, queue_size={}, max_continuation_permits={}",
            model, concurrent_mode, queue_size, max_continuation_permits
        );

        Self {
            provider: parking_lot::RwLock::new(Arc::from(provider)),
            active_model: parking_lot::RwLock::new(config.model.clone()),
            tools: parking_lot::RwLock::new(HashMap::new()),
            config,
            outbound_tx: Some(outbound_tx),
            registry: Some(registry),
            state_manager: None,
            session_store: Some(session_store),
            chat_seq_lookup: parking_lot::RwLock::new(None),
            running: AtomicBool::new(false),
            session_busy: parking_lot::Mutex::new(HashMap::new()),
            rate_limit_status: parking_lot::Mutex::new(HashMap::new()),
            concurrent_mode,
            reinject_tx: parking_lot::RwLock::new(None),
            queue_size,
            max_continuation_permits,
            continuation_semaphore,
            turn_permits: None,
            max_concurrent_turns: 0,
            summarizing: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            compact_state: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            channel_manager_channels: parking_lot::Mutex::new(Vec::new()),
            sent_in_round: SentInRoundTracker::new(),
            route_resolver: Some(RouteResolver::new(default_route_config)),
            observer_callback: None,
            continuation_manager: None,
            cluster: None,
            observer_manager: None,
            security: SecurityState {
                #[cfg(feature = "security")]
                security_plugin: None,
                #[cfg(not(feature = "security"))]
                security_plugin: None,
                checkpoint_store: parking_lot::RwLock::new(None),
                estop: parking_lot::RwLock::new(None),
                approval_responder: parking_lot::RwLock::new(None),
                question_responder: parking_lot::RwLock::new(None),
                question_asker: parking_lot::RwLock::new(None),
            },
            mcp_manager: None,
            mcp_tool_snapshot: Arc::new(parking_lot::RwLock::new(Vec::new())),
            data_store: None,
            forge: None,
            cancel_tokens: dashmap::DashMap::new(),
            turn_counter: std::sync::atomic::AtomicUsize::new(0),
            turn_file_changes: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            rewind_undo_stacks: parking_lot::Mutex::new(HashMap::new()),
            hooks: HooksState {
                tool_hooks: parking_lot::RwLock::new(crate::hooks::ToolHookManager::new()),
                llm_hooks: parking_lot::RwLock::new(crate::hooks::LlmHookManager::new()),
                lifecycle_hooks: parking_lot::RwLock::new(crate::hooks::LifecycleHookManager::new()),
            },
            memory: MemoryState {
                memory_executor: parking_lot::RwLock::new(None),
                #[cfg(feature = "memory")]
                memory_inject_manager: parking_lot::RwLock::new(None),
                #[cfg(not(feature = "memory"))]
                memory_inject_manager: parking_lot::RwLock::new(None),
                memory_inject_cfg: parking_lot::RwLock::new((false, 3)),
                #[cfg_attr(not(feature = "memory"), allow(dead_code))]
                tool_vec_cache: parking_lot::RwLock::new(std::collections::HashMap::new()),
            },
            tier: parking_lot::RwLock::new(nemesis_types::capability::ModelTier::Big),
            mode: parking_lot::RwLock::new(crate::types::AgentMode::Build),
            agent_event_tx: parking_lot::RwLock::new(None),
            config_path: parking_lot::RwLock::new(None),
            pricing_store: parking_lot::RwLock::new(None),
            lsp_manager: parking_lot::RwLock::new(None),
            commands_hot: parking_lot::RwLock::new(None),
            cc_bridge: parking_lot::RwLock::new(None),
            spill_root: parking_lot::RwLock::new(None),
            skills_loader: parking_lot::RwLock::new(None),
            skills_digest_state: std::sync::Arc::new(crate::skills_digest::DigestState::new()),
            inbox: std::sync::Arc::new(crate::inbox::Inbox::new(queue_size.max(1))),
            turn_task_handles: parking_lot::Mutex::new(Vec::new()),
            workspace_root: parking_lot::RwLock::new(None),
            fs_watcher: parking_lot::RwLock::new(None),
            external_changes: parking_lot::Mutex::new(Vec::new()),
            recent_self_writes: parking_lot::Mutex::new(std::collections::HashMap::new()),
            snapshot_role: parking_lot::RwLock::new("user".to_string()),
            interactive_approval: parking_lot::RwLock::new(false),
            pending_open_files: parking_lot::RwLock::new(Vec::new()),
            pending_workflow_edit: parking_lot::RwLock::new(None),
            #[cfg(feature = "workflow")]
            workflow_engine: parking_lot::RwLock::new(None),
            config_mtime: parking_lot::RwLock::new(None),
            small_model: parking_lot::RwLock::new(None),
        }
    }

    // -----------------------------------------------------------------------
    // Continuation dispatch
    // -----------------------------------------------------------------------

    /// Dispatch a cluster continuation: inline (permits=0) or spawned (permits>0).
    /// Called from both `run_bus_owned` (test) and `run_bus_arc` (production).
    async fn dispatch_continuation(
        &self,
        task_id: String,
        msg: &nemesis_types::channel::InboundMessage,
    ) {
        let task_response = msg.content.clone();
        let task_metadata = msg.metadata.clone();
        let task_failed = task_metadata
            .get("status")
            .map(|s| s == "error")
            .unwrap_or(false);
        // 集群续行归属（2026-09-23）：实际干活的 worker 节点名（gateway
        // Route 2 与 G5 恢复发布的 metadata 同款键）。缺席 = 旧快照/老发布方，
        // 出站不带节点徽章，行为与历史一致。owned String——spawn 分支闭包
        // 要求 'static（与 task_error 同款）。
        let task_source_node = task_metadata.get("source_node").cloned();

        if self.max_continuation_permits == 0 {
            // Inline: process directly in the main loop (no spawn).
            // The main loop is blocked until continuation completes,
            // ensuring serialized execution with no resource contention.
            let task_error = task_metadata.get("error").map(|s| s.as_str());
            if let Some(ref mgr) = self.continuation_manager
                && let Some(ref tx) = self.outbound_tx
            {
                // Clone data from RwLock guards before .await — guards are !Send
                // and cannot be held across yield points in an async fn.
                let provider = self.provider.read().clone();
                let model = self.active_model.read().clone();
                let tools = self.tools.read().clone();

                crate::loop_continuation::handle_cluster_continuation(
                    mgr.as_ref(),
                    &task_id,
                    &task_response,
                    task_failed,
                    task_error,
                    provider.as_ref(),
                    &model,
                    &tools,
                    tx,
                    self.observer_manager.clone(),
                    self.session_store.as_ref().map(|v| v.as_ref()),
                    // F-F：active 模型 vision 解析（config.json 唯一真相源）。
                    self.current_vision().supported,
                    task_source_node.as_deref(),
                )
                .await;
            }
        } else {
            // Spawn with semaphore-controlled concurrency.
            let task_error = task_metadata.get("error").cloned();
            let provider = self.provider.read().clone();
            let model = self.active_model.read().clone();
            let tools = self.tools.read().clone();
            let outbound_tx = self.outbound_tx.clone();
            let continuation_manager = self.continuation_manager.clone();
            let observer_manager = self.observer_manager.clone();
            let session_store = self.session_store.clone();
            let semaphore = self.continuation_semaphore.clone().unwrap();
            // F-F：vision 解析在 spawn 前取值（闭包外，self 不可 move 进去）。
            let vision_supported = self.current_vision().supported;

            tokio::spawn(async move {
                let _permit = semaphore.acquire().await.unwrap();
                if let Some(ref mgr) = continuation_manager
                    && let Some(ref tx) = outbound_tx
                {
                    crate::loop_continuation::handle_cluster_continuation(
                        mgr.as_ref(),
                        &task_id,
                        &task_response,
                        task_failed,
                        task_error.as_deref(),
                        provider.as_ref(),
                        &model,
                        &tools,
                        tx,
                        observer_manager,
                        session_store.as_ref().map(|v| v.as_ref()),
                        vision_supported,
                        task_source_node.as_deref(),
                    )
                    .await;
                }
            });
        }
    }

    // -----------------------------------------------------------------------
    // Registration methods
    // -----------------------------------------------------------------------

    /// Run the main bus consumption loop (takes ownership of the receiver).
    ///
    /// This is the preferred entry point for bus-integrated mode.
    /// Mirrors Go's `AgentLoop.Run(ctx)`. Continuously consumes inbound
    /// messages, processes them, and publishes outbound responses.
    /// Stops when `stop()` is called or the inbound channel closes.
    ///
    /// Test-only variant; production code uses `run_bus_arc`.
    /// Post-turn finish (V5: extracted verbatim from the original run_bus
    /// bodies so the serial pump and spawned turn tasks share one tail):
    /// error funnel + capture flush, sent-in-round check, RPC correlation
    /// prefix, outbound publish.
    ///
    /// `check_sent_in_round`: Reject (serial — no turn can overlap) keeps the
    /// historical check+clear. Queue/Steer Immediate replies (busy receipts,
    /// slash responses) pass false: they can run while the session's turn is
    /// in flight, and touching that turn's sent-in-round flag mid-flight
    /// would corrupt its end-of-turn publish decision.
    pub(crate) async fn finish_message(
        &self,
        msg: &nemesis_types::channel::InboundMessage,
        response: String,
        err: Option<String>,
        check_sent_in_round: bool,
    ) {
        let response = match err {
            Some(e) => {
                // [capture] Agent error funnel: the full error
                // becomes the user-visible response. Flush the
                // session's captured evidence + the complete error
                // text (the user sees a short "Error: ..."; this
                // keeps the full source string for root-causing).
                if let Some(sink) = crate::capture_sink::CaptureSink::global() {
                    sink.flush(&msg.session_key, "agent_error", None, Some(e.as_str()));
                }
                format!("Error processing message: {}", e)
            }
            None => response,
        };

        if response.is_empty() {
            return;
        }

        if check_sent_in_round {
            // Check if a tool (e.g., MessageTool) already sent a response for this
            // session in the current round. Mirrors Go's alreadySent check.
            let already_sent = self.sent_in_round.has_sent_in_round(&msg.session_key);
            // Only clear this session's flag, not all sessions.
            // Go clears per-tool-instance state, so clearing only the current
            // session preserves other sessions' sent-in-round tracking.
            self.sent_in_round.clear(&msg.session_key);

            if already_sent {
                debug!(
                    "[AgentLoop] Skipping outbound publish: message tool already sent response for session {}",
                    msg.session_key
                );
                return;
            }
        }

        if let Some(ref tx) = self.outbound_tx {
            // For RPC channel, add correlation ID prefix if not already present.
            let final_content = if msg.channel == "rpc"
                && !msg.correlation_id.is_empty()
                && !response.starts_with(&format!("[rpc:{}]", msg.correlation_id))
            {
                format!("[rpc:{}] {}", msg.correlation_id, response)
            } else {
                response
            };

            info!(
                "[AgentLoop] Response message     to {}:{}: {}",
                msg.channel,
                msg.chat_id,
                truncate(&final_content, 80)
            );

            let outbound = nemesis_types::channel::OutboundMessage {
                channel: msg.channel.clone(),
                chat_id: msg.chat_id.clone(),
                content: final_content,
                message_type: String::new(),
                meta: nemesis_types::channel::OutboundMeta {
                    model: Some(self.current_display_model()),
                    // L2：会话键随行——web 通道 chat_event_log 按会话（非连接）
                    // 记录，断线重连后 chat.sync 才能寻址。
                    session_key: (!msg.session_key.is_empty()).then(|| msg.session_key.clone()),
                    source_node: None,
                },
            };
            if let Err(e) = tx.send(outbound).await {
                warn!("[AgentLoop] Failed to send outbound message: {}", e);
            }
        }
    }

    /// D-4 (2026-09-23 多会话并行清账)：设置 loop 级并发 turn 上限（安全
    /// 阀）。`0` = 不设限（构造缺省，独立/测试路径不变）；`>0` = 最多 N 个
    /// turn 并发执行，超限的 spawned 任务在信号量上排队（诚实等待，不丢
    /// 失）。gateway 装配期调用一次（主 loop / 项目 loop 同款，来自
    /// `agents.defaults.max_concurrent_turns`）。
    pub fn set_max_concurrent_turns(&mut self, n: usize) {
        self.max_concurrent_turns = n;
        self.turn_permits = (n > 0).then(|| Arc::new(tokio::sync::Semaphore::new(n)));
        info!(
            "[AgentLoop] max_concurrent_turns = {}",
            if n == 0 {
                "unlimited".to_string()
            } else {
                n.to_string()
            }
        );
    }

    /// Spawn a turn task (all modes since the unified pump) and track its
    /// abort handle so `stop()` can cancel in-flight turns. Finished handles
    /// are pruned on each insert, keeping the vec bounded by live turns.
    ///
    /// D-4：并发上限信号量在**任务体内**获取（泵只 spawn 不等待）——超限
    /// 任务排队等许可，gate 的回执与出站路径照常即时；turn abort 时许可随
    /// future 释放。
    pub(crate) fn spawn_turn_task<F>(&self, fut: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let permits = self.turn_permits.clone();
        let handle = tokio::spawn(async move {
            // acquire_owned 把许可与 Arc 绑定，async 块结束时（含 abort 展开丢弃）释放。
            let _permit = match permits {
                Some(p) => Some(p.acquire_owned().await),
                None => None,
            };
            fut.await;
        });
        let mut handles = self.turn_task_handles.lock();
        handles.retain(|h| !h.is_finished());
        handles.push(handle.abort_handle());
    }

    /// Unified bus pump (V5 gate-in-pump; D 2026-09-23 多会话并行清账后
    /// **模式无关**).
    ///
    /// 唯一形态：同步 gate（`gate_inbound`）内联在泵里跑（路由 + 忙判定 +
    /// inbox 停泊 + 回执），每个被放行的 turn 作为 tracked task spawn——
    /// **跨会话并发、同会话保序**是结构不变量（gate 在 spawn 前获取会话；
    /// turn 期间会话保持 busy，同会话后续消息在 gate 处按 concurrent_mode
    /// 处置，绝不可能堵住其他会话）。
    ///
    /// 历史教训（本次清账的根因）：Reject 曾实现为泵级串行——
    /// `process_inbound_message(&msg).await` 内联在接收循环里，上一条 turn
    /// 不跑完下一条消息根本不出队，主 loop 上一个数分钟的 agentGen turn
    /// 堵死所有会话（用户消息堵在不可见队列里零落盘 → 前端切会话即
    /// 「消失」）。Reject 现在只是 gate 忙分支的处置语义（回执拒绝 + 留
    /// 痕），与泵调度彻底解耦。
    ///
    /// Continuations stay inline (serialized with the pump).
    async fn run_bus_impl(
        self: Arc<Self>,
        mut inbound_rx: tokio::sync::mpsc::Receiver<nemesis_types::channel::InboundMessage>,
    ) {
        self.running.store(true, Ordering::Release);
        info!("[AgentLoop] Bus consumption loop started");

        while self.running.load(Ordering::Acquire) {
            match inbound_rx.recv().await {
                Some(msg) => {
                    match self.gate_inbound(&msg) {
                        GateOutcome::Continuation(task_id) => {
                            info!(
                                "[AgentLoop] Handling cluster continuation for task {} (permits={})",
                                task_id, self.max_continuation_permits
                            );
                            self.dispatch_continuation(task_id, &msg).await;
                        }
                        GateOutcome::Immediate {
                            agent_id: _,
                            response,
                        } => {
                            // Busy receipt / busy bounce / queue-full / slash
                            // reply — publish inline. Never touches
                            // sent_in_round (may overlap the session's
                            // running turn; see finish_message).
                            self.finish_message(&msg, response, None, false).await;
                        }
                        GateOutcome::Maintenance {
                            kind,
                            session_key,
                            receipt,
                        } => {
                            // E6: 维护命令 — 会话已在 gate 获取，派独立
                            // task 执行（LLM 摘要可达分钟级，不得堵泵）；
                            // task 尾部释放会话。同串行路径：⏳ 先发、✓
                            // 后发，均不碰 sent_in_round。
                            let this = self.clone();
                            let m = msg.clone();
                            self.spawn_turn_task(async move {
                                this.finish_message(&m, receipt, None, false).await;
                                let response = this.handle_maintenance(kind, &session_key).await;
                                this.release_session(&session_key);
                                this.finish_message(&m, response, None, false).await;
                            });
                        }
                        GateOutcome::UserDispatch {
                            task_text,
                            node_id,
                            session_key,
                        } => {
                            // K4: 用户直发远程编码任务派发 — 会话已在
                            // gate 获取，派独立 task 执行（RPC ACK 等待
                            // 可达分钟级，不得堵泵）；task 尾部释放会话
                            // （process_user_dispatch 内部负责）。
                            let this = self.clone();
                            let m = msg.clone();
                            self.spawn_turn_task(async move {
                                this.process_user_dispatch(&m, &task_text, &node_id, &session_key)
                                    .await;
                            });
                        }
                        GateOutcome::Ungated => {
                            let this = self.clone();
                            let m = msg.clone();
                            self.spawn_turn_task(async move {
                                let (_, response, err) = this.process_ungated(&m).await;
                                this.finish_message(&m, response, err, true).await;
                            });
                        }
                        GateOutcome::Admitted(admission) => {
                            let this = self.clone();
                            let m = msg.clone();
                            self.spawn_turn_task(async move {
                                let (_, response, err) = this.process_admitted(&m, admission).await;
                                this.finish_message(&m, response, err, true).await;
                            });
                        }
                    }
                }
                None => {
                    // Channel closed.
                    break;
                }
            }
        }

        info!("[AgentLoop] Bus consumption loop stopped");
        self.running.store(false, Ordering::Release);
    }

    /// Run the main bus consumption loop (takes ownership of the receiver).
    ///
    /// This is the preferred entry point for bus-integrated mode.
    /// Mirrors Go's `AgentLoop.Run(ctx)`. Continuously consumes inbound
    /// messages, processes them, and publishes outbound responses.
    /// Stops when `stop()` is called or the inbound channel closes.
    ///
    /// Test-only variant; production code uses `run_bus_arc`.
    #[cfg(test)]
    pub async fn run_bus_owned(
        self,
        inbound_rx: tokio::sync::mpsc::Receiver<nemesis_types::channel::InboundMessage>,
    ) {
        std::sync::Arc::new(self).run_bus_impl(inbound_rx).await;
    }

    /// Same as `run_bus_owned` but takes `Arc<Self>` so the AgentLoop can be
    /// shared with other components (e.g. heartbeat handler) while the bus
    /// loop is running.
    pub async fn run_bus_arc(
        self: Arc<Self>,
        inbound_rx: tokio::sync::mpsc::Receiver<nemesis_types::channel::InboundMessage>,
    ) {
        self.run_bus_impl(inbound_rx).await;
    }

    /// Stop the bus consumption loop.
    /// Mirrors Go's `AgentLoop.Stop()`.
    pub fn stop(&self) {
        info!("[AgentLoop] Stop requested");
        self.running.store(false, Ordering::Release);
        // V5 (2026-08-23): kill Queue/Steer mode's spawned turn tasks. The
        // adapter aborts the pump task itself; without this the spawned turns
        // would be orphaned (holding Arc clones) and keep running/publishing
        // after the stop.
        let mut handles = self.turn_task_handles.lock();
        for h in handles.drain(..) {
            h.abort();
        }
    }

    /// Check whether the loop is currently running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    /// Emit an observer event synchronously (for conversation start/end).
    ///
    /// Forwards to both the Phase 5 observer manager and the legacy
    /// `observer_callback`.
    pub(crate) async fn emit_observer_sync(&self, event: crate::loop_executor::ObserverEvent) {
        if let Some(ref mgr) = self.observer_manager {
            let conv_event = event.to_conversation_event();
            mgr.emit_sync(conv_event).await;
        }
        if let Some(ref cb) = self.observer_callback {
            let (event_type, data) = event.to_callback_json();
            cb(event_type, &data);
        }
    }

    // -----------------------------------------------------------------------
    // Cluster continuation handling
    // -----------------------------------------------------------------------

    /// Handle a cluster continuation by loading the snapshot, resuming the LLM
    /// loop, and sending the final response.
    ///
    /// NOTE: The main run_bus_owned loop calls the free function
    /// `crate::loop_continuation::handle_cluster_continuation` directly instead
    /// of this method. Similarly, maybe_summarize calls the standalone
    /// `summarize_history_owned` / `summarize_multipart_owned` / `summarize_batch_owned`
    /// free functions. These self methods are kept as reference implementations
    /// matching the Go AgentLoop method signatures.
    #[allow(dead_code)]
    pub(crate) async fn handle_cluster_continuation(
        &self,
        task_id: &str,
        original_msg: &nemesis_types::channel::InboundMessage,
    ) {
        if let Some(ref mgr) = self.continuation_manager {
            let task_response = &original_msg.content;
            let task_failed = original_msg
                .metadata
                .get("status")
                .map(|s| s == "error")
                .unwrap_or(false);
            let task_error = original_msg.metadata.get("error").map(|s| s.as_str());
            let task_source_node = original_msg.metadata.get("source_node").map(|s| s.as_str());

            // Clone provider and model before .await (RwLock guards are not Send).
            let cont_provider = self.provider.read().clone();
            let cont_model = self.active_model.read().clone();
            if let Some(ref tx) = self.outbound_tx {
                crate::loop_continuation::handle_cluster_continuation(
                    mgr.as_ref(),
                    task_id,
                    task_response,
                    task_failed,
                    task_error,
                    cont_provider.as_ref(),
                    &cont_model,
                    &self.tools,
                    tx,
                    self.observer_manager.clone(),
                    self.session_store.as_ref().map(|v| v.as_ref()),
                    // F-F：active 模型 vision 解析。
                    self.current_vision().supported,
                    task_source_node,
                )
                .await;
            }
        } else {
            warn!(
                "[AgentLoop] No continuation manager configured, cannot handle continuation for task_id={}",
                task_id
            );
        }
    }

    // -----------------------------------------------------------------------
    // Direct processing (bypass bus)
    // -----------------------------------------------------------------------
}

// ---------------------------------------------------------------------------
// 自由函数归位（P1-c 自 loop.rs 根搬迁；仅增 pub(crate) 可见性标注）
// ---------------------------------------------------------------------------

/// Extract the task ID from a cluster continuation sender ID.
///
/// The format is `cluster_continuation:{taskID}`.
#[cfg(test)]
pub fn extract_continuation_task_id(sender_id: &str) -> Option<&str> {
    sender_id.strip_prefix(nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX)
}

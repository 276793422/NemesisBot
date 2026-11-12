//! 总线消费环：is_internal_channel、BUSY_MESSAGE、parse_concurrent_mode、SessionBusyTracker、new_bus/run_bus 系、stop/is_running、spawn_turn_task、finish_message、dispatch_continuation、emit_observer_sync。
//!
//! P1 自 `loop.rs` 物理搬迁（docs/PLAN/2026-09-23_agentloop-god-object-decomposition.md §3.2）；语义零变化。
#[cfg(not(target_os = "linux"))]
use super::prelude::*;
use super::*;

/// Check if a channel is internal (not user-facing).
///
/// 委托仓内单一真相源 `nemesis_types::is_internal_channel`
/// （BUG-20261025-46 收编，本文件原为手抄 `matches!` 副本）。
pub fn is_internal_channel(channel: &str) -> bool {
    nemesis_types::is_internal_channel(channel)
}

// ---------------------------------------------------------------------------
// Session busy state management
// ---------------------------------------------------------------------------

/// Busy message returned when session is busy.
pub const BUSY_MESSAGE: &str =
    "\u{23f3} AI is processing a previous request, please try again later";

/// W6 优雅停机 drain：draining 置位后对外部通道新消息的诚实回执。
pub const DRAINING_RECEIPT: &str =
    "\u{23f4} 正在停机：新消息已停止受理（在途任务正常收尾），请稍后重试";

/// W6：在途 turn gauge 守卫——构造即 +1，Drop 即 -1（panic/abort 展开
/// 路径也减账，gauge 不会单调漂移）。spawn 臂与 inline continuation 臂
/// 共用（[`AgentLoop::spawn_turn_task`] / pump_dispatch Continuation 分支）。
pub(crate) struct InFlightTurnGuard {
    gauge: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl InFlightTurnGuard {
    pub(crate) fn acquire(gauge: &std::sync::Arc<std::sync::atomic::AtomicUsize>) -> Self {
        gauge.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        Self {
            gauge: std::sync::Arc::clone(gauge),
        }
    }
}

impl Drop for InFlightTurnGuard {
    fn drop(&mut self) {
        self.gauge.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}
/// Parse the config string. Unknown values fall back to Reject (fail-safe
/// to legacy behavior) with a warn at the call site.
pub fn parse_concurrent_mode(s: &str) -> ConcurrentMode {
    match s.trim().to_lowercase().as_str() {
        "queue" => ConcurrentMode::Queue,
        "steer" => ConcurrentMode::Steer,
        _ => ConcurrentMode::Reject,
    }
}

/// 回调 metadata → D5 台账终态（status=error → Failed，否则 Done）。
fn registry_status_of(
    msg: &nemesis_types::channel::InboundMessage,
) -> crate::subagent_registry::SubagentStatus {
    if msg.metadata.get("status").is_some_and(|s| s == "error") {
        crate::subagent_registry::SubagentStatus::Failed
    } else {
        crate::subagent_registry::SubagentStatus::Done
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
            session_stats: parking_lot::RwLock::new(None),
            work_recorder: crate::work_timeline::WorkRecorder::default(),
            effort_overrides: parking_lot::RwLock::new(HashMap::new()),
            state_manager: None,
            session_store: Some(session_store),
            chat_seq_lookup: parking_lot::RwLock::new(None),
            running: AtomicBool::new(false),
            stop_token: tokio_util::sync::CancellationToken::new(),
            session_busy: parking_lot::Mutex::new(HashMap::new()),
            rate_limit_status: parking_lot::Mutex::new(HashMap::new()),
            concurrent_mode,
            reinject_tx: parking_lot::RwLock::new(None),
            extra_hidden_tools: parking_lot::RwLock::new(Vec::new()),
            exclusive_hidden_provider: parking_lot::RwLock::new(None),
            checks_capture: parking_lot::RwLock::new(HashMap::new()),
            tool_policy_allow: parking_lot::RwLock::new(Vec::new()),
            compact_policy: parking_lot::RwLock::new(std::sync::Arc::new(
                crate::r#loop::policy::BuiltinCompactPolicy,
            )),
            final_judge_policy: parking_lot::RwLock::new(std::sync::Arc::new(
                crate::r#loop::policy::BuiltinFinalJudgePolicy,
            )),
            validation_policy: parking_lot::RwLock::new(std::sync::Arc::new(
                crate::r#loop::policy::BuiltinValidationPolicy,
            )),
            title_callback: parking_lot::RwLock::new(None),
            tier_override: parking_lot::RwLock::new(None),
            queue_size,
            discipline: parking_lot::RwLock::new(None),
            receipt_key: crate::tool_receipts::ReceiptKey::generate(),
            max_continuation_permits,
            continuation_semaphore,
            turn_permits: None,
            max_concurrent_turns: 0,
            summarizing: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            compact_state: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            compaction_in_flight: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            channel_manager_channels: parking_lot::Mutex::new(Vec::new()),
            sent_in_round: SentInRoundTracker::new(),
            route_resolver: Some(RouteResolver::new(default_route_config)),
            identity_links: HashMap::new(),
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
                credential_broker: parking_lot::RwLock::new(None),
                credential_card_guard: Arc::new(
                    crate::r#loop::credential_injection::CredentialCardGuard::new(),
                ),
                #[cfg(feature = "security")]
                approval_auto: parking_lot::RwLock::new(None),
            },
            mcp_manager: None,
            browser_profile: None,
            mcp_tool_snapshot: Arc::new(parking_lot::RwLock::new(Vec::new())),
            data_store: None,
            forge: None,
            cancel_tokens: dashmap::DashMap::new(),
            mandate_guard: parking_lot::RwLock::new(None),
            turn_counter: std::sync::atomic::AtomicUsize::new(0),
            in_flight_turns: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            draining: std::sync::atomic::AtomicBool::new(false),
            turn_file_changes: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            turn_deliverables: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            rewind_undo_stacks: parking_lot::Mutex::new(HashMap::new()),
            gc_inflight_trees: parking_lot::Mutex::new(std::collections::HashSet::new()),
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
            mode: std::sync::Arc::new(parking_lot::RwLock::new(crate::types::AgentMode::Build)),
            agent_event_tx: parking_lot::RwLock::new(None),
            config_path: parking_lot::RwLock::new(None),
            prompt_system: parking_lot::RwLock::new(crate::prompt::PromptSystem::Classic),
            pricing_store: parking_lot::RwLock::new(None),
            #[cfg(feature = "lsp")]
            lsp_manager: parking_lot::RwLock::new(None),
            #[cfg(feature = "lsp")]
            diagnostics_touched: parking_lot::Mutex::new(
                config_watch::DiagnosticsTouchRegistry::default(),
            ),
            commands_hot: parking_lot::RwLock::new(None),
            cc_bridge: parking_lot::RwLock::new(None),
            spill_root: parking_lot::RwLock::new(None),
            plan_present: parking_lot::RwLock::new(None),
            skills_loader: parking_lot::RwLock::new(None),
            skills_digest_state: std::sync::Arc::new(crate::skills_digest::DigestState::new()),
            inbox: std::sync::Arc::new(crate::inbox::Inbox::new(queue_size.max(1))),
            turn_task_handles: parking_lot::Mutex::new(Vec::new()),
            workspace_root: parking_lot::RwLock::new(None),
            vault_domain_config_path: parking_lot::RwLock::new(None),
            exec_env_resolver: parking_lot::RwLock::new(None),
            instruction_trust: parking_lot::RwLock::new(
                crate::workspace_instructions::InstructionTrust::Trusted,
            ),
            fs_watcher: parking_lot::RwLock::new(None),
            external_changes: parking_lot::Mutex::new(Vec::new()),
            recent_self_writes: parking_lot::Mutex::new(std::collections::HashMap::new()),
            snapshot_role: parking_lot::RwLock::new("user".to_string()),
            #[cfg(all(feature = "codemode", not(target_os = "android")))]
            code_run_tool: parking_lot::RwLock::new(None),
            interactive_approval: parking_lot::RwLock::new(false),
            pending_open_files: parking_lot::RwLock::new(Vec::new()),
            pending_workflow_edit: parking_lot::RwLock::new(None),
            tool_schema_unlocks: parking_lot::RwLock::new(std::collections::HashMap::new()),
            #[cfg(feature = "workflow")]
            workflow_engine: parking_lot::RwLock::new(None),
            config_gen: parking_lot::RwLock::new(None),
            user_profile_cache: parking_lot::RwLock::new(((None, None), String::new())),
            warm_candidates: parking_lot::Mutex::new(HashMap::new()),
            small_model: parking_lot::RwLock::new(None),
            spawn_slot: parking_lot::RwLock::new(None),
            loop_origin: parking_lot::RwLock::new("main".to_string()),
            voice_prompt: parking_lot::RwLock::new(None),
            workspace_boundary: parking_lot::RwLock::new(None),
            plan_products_root: parking_lot::RwLock::new(None),
            token_budget: crate::token_budget::SessionTokenBudgetLedger::default(),
            escalated_floors: parking_lot::Mutex::new(HashMap::new()),
            turn_wal: parking_lot::RwLock::new(None),
            provider_reloader: parking_lot::RwLock::new(None),
        }
    }

    // -----------------------------------------------------------------------
    // Continuation dispatch
    // -----------------------------------------------------------------------

    /// 续行回主路（2026-11-12）的续行轮预算：异步回灌交接主循环后的工具
    /// 轮数上限（对齐旧迷你循环 `max_iterations = 20`，经
    /// [`Self::resume_execution_with_token`] 的 `turn_budget` 注入
    /// run_llm_loop 的 per-turn 覆盖闸；耗尽走 grace round 语义）。
    const CONTINUATION_TURN_BUDGET: u32 = 20;

    /// Dispatch a cluster continuation: inline (permits=0) or spawned (permits>0).
    /// Called from both `run_bus_owned` (test) and `run_bus_arc` (production).
    /// `pub(crate)`：delegate 台账终态回写测试直驱（D5，2026-11-03）。
    ///
    /// 续行回主路（2026-11-12）：执行体从 `handle_cluster_continuation`
    /// 迷你循环（LLM 硬编码 tools=0 + 裸 execute 绕 dispatch 瀑布）换成
    /// [`Self::run_continuation_turn`]（session busy 闸 + run_llm_loop
    /// 全管线）。本函数瘦身为纯编排，旧迷你循环的取参全部退役——
    /// provider/model/tools 由主循环每轮自持解析（回主路的核心收益），
    /// estop 由交接函数早期闸消费，max_tokens/阶梯封顶/vision 投影由
    /// run_llm_loop 的 chat_opts 同源解析。`self: &Arc<Self>`：spawn 臂
    /// 闭包要求 'static。
    pub(crate) async fn dispatch_continuation(
        self: &Arc<Self>,
        task_id: String,
        msg: &nemesis_types::channel::InboundMessage,
    ) {
        if self.max_continuation_permits == 0 {
            // Inline: process directly in the main loop (no spawn).
            // The main loop is blocked until continuation completes,
            // ensuring serialized execution with no resource contention.
            self.run_continuation_turn(&task_id, msg).await;
        } else {
            // Spawn with semaphore-controlled concurrency.
            let semaphore = self.continuation_semaphore.clone().unwrap();
            let this = self.clone();
            let m = msg.clone();
            tokio::spawn(async move {
                let _permit = semaphore.acquire().await.unwrap();
                this.run_continuation_turn(&task_id, &m).await;
            });
        }
    }

    /// 续行回主路（2026-11-12）B1：异步任务结果（集群
    /// `cluster_continuation` / 后台子代理 `subagent_continuation`，gate
    /// 同一出口）交接给**主循环**执行的交接函数。
    ///
    /// 旧路径 [`crate::loop_continuation::handle_cluster_continuation`] 是
    /// 平行迷你循环：LLM 请求硬编码 tools=0、工具执行裸 `tool.execute`
    /// 绕过 dispatch 瀑布（安全 8 层/审批/estop/turn_guard 全部缺席）。
    /// 本函数保留旧路径的恢复/台账语义（快照相位机、D5 台账、
    /// final_persisted 幂等闸、inflight 历史闸、崩溃安全网写回），执行面
    /// 换主循环：
    /// - 活体历史装配：[`crate::loop_continuation::apply_real_result_to_history`]
    ///   把真实结果种入 store 水合出的会话历史（marker 原位替换 / user
    ///   turn 兜底），`set_history` 后走 [`Self::resume_execution_with_token`]
    ///   （工具全量供给、dispatch 瀑布全开）；
    /// - 收尾走 [`Self::finalize_turn_shared`]（chat_log assistant 行带
    ///   节点徽章 + work 时间线 + store 全量持久化 + ConversationEnd）；
    /// - 会话串行：`try_acquire_session` 失败 = 该会话回合在跑——Queue/
    ///   Steer 模式重投递消息入 inbox（回合尾巴 drain 自然排空），Reject
    ///   模式（drain 不认领队列，排队即永久滞留）诚实终局注记。
    ///
    /// 闸序：load（只读学信封）→ estop 早期闸 → 空键 legacy 分支 →
    /// try_acquire ★（先于一切闸状态变更）→ busy 分臂 → 持 busy guard：
    /// D5 台账 → claim 单飞 → final_persisted 幂等闸 → 相位机 confirm →
    /// inflight 标记 → merge 写回（崩溃安全网）→ 活体装配 → resume →
    /// finalize → 摘双 guard → drain → 出站直发 → final_persisted 打标 →
    /// finish_handling。
    async fn run_continuation_turn(
        self: &Arc<Self>,
        task_id: &str,
        msg: &nemesis_types::channel::InboundMessage,
    ) {
        let start_time = std::time::Instant::now();
        // trace_id 与旧迷你循环同形（observer 事件关联锚）。
        let trace_id = format!(
            "continuation-{}-{}",
            task_id,
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(0)
        );

        let Some(mgr) = self.continuation_manager.clone() else {
            // 无续行管理器（极简装配）：D5 台账照常回写终态（旧行为
            // registry.finish 无条件先跑），其余无物可做。
            crate::subagent_registry::global().finish(
                task_id,
                registry_status_of(msg),
                None,
                (!msg.content.is_empty()).then(|| msg.content.clone()),
                msg.metadata.get("error").cloned(),
            );
            return;
        };

        // 回调元数据解析（全部终端臂共用；owned——spawn 场景外的本函数
        // 无闭包，仅避免借用跨 await）。
        let task_failed = msg.metadata.get("status").is_some_and(|s| s == "error");
        let task_error = msg.metadata.get("error").cloned();
        // 集群续行归属（2026-09-23）：实际干活的 worker 节点名（gateway
        // Route 2 与 G5 恢复发布的 metadata 同款键）。缺席 = 旧快照/老发布方，
        // 出站不带节点徽章，行为与历史一致。
        let task_source_node = msg.metadata.get("source_node").cloned();

        // L5（2026-10-12 内部专项 T0）：续行回灌的输出凭据/DLP 复扫。工具
        // 输出回灌有三条路——直接回灌（tool_dispatch 主线，已有复扫）、
        // cluster 续行追加、子代理后台返回（`subagent_continuation:{task_id}`
        // 走同一续行机制）。本函数是后两条的单一汇合点，在此复扫一次即全
        // 覆盖，防只堵正门。worker 返回的内容同样可能回显明文秘密（远端
        // read_file/exec），脱敏式处理与主线同语义。
        #[cfg(feature = "security")]
        let task_response = output_rescan::rescan_tool_output(
            self.security.security_plugin.as_deref(),
            "cluster_continuation",
            msg.content.clone(),
        );
        #[cfg(not(feature = "security"))]
        let task_response = msg.content.clone();

        // D5 台账三元组（一次计算，全部终局臂共用）。旧路径 registry.finish
        // 无条件先跑——estop/排队满等臂中止续行也必须带真实终态回写；
        // 唯一不回写的臂 = Queue/Steer 忙重投递（非终态，结局未定）。
        let reg_status = registry_status_of(msg);
        let reg_summary = (!task_response.is_empty()).then(|| task_response.clone());
        let reg_error = task_error.clone();

        // 1. Load continuation snapshot（只读学信封：session_key/channel/
        //    chat_id 全部来自快照——回调 inbound 是 system 通道幻影帧，不
        //    携带落点）。barrier 等待窗口 = 快照尚未落地的回调竞态（既有
        //    语义原样保留）。未命中 = dashboard 直派 peer_chat（web handler
        //    tasks_submit 不经 cluster_rpc 工具，从不存快照），结果走
        //    TaskManager 路径（gateway Route 3）投递——静默跳过。
        let Some(cont) = mgr.load_continuation(task_id).await else {
            debug!(
                "[Continuation] No continuation for task_id={} (likely dashboard-initiated peer_chat, skipping)",
                task_id
            );
            crate::subagent_registry::global().finish(
                task_id,
                reg_status,
                None,
                reg_summary,
                reg_error,
            );
            return;
        };

        // estop 早期闸（BUG-20261106-041 延续）：急停期间不开新 LLM 轮。
        // 注记落库（store append + chat_log assistant 行带节点徽章）→
        // final_persisted 打标（崩溃窗口重投递幂等）→ 出站 ⛔ 注记（与旧
        // 迷你循环 estop 文案逐字节一致）→ finish_handling（单飞放行 +
        // 快照回收）。任务结果文本不随注记出站（旧语义：急停只回执）。
        if self.security.is_engaged() {
            tracing::warn!(
                "[Continuation] E-stop engaged at callback arrival, aborting continuation handoff for task {task_id}"
            );
            let note = "⛔ 已急停 (E-STOP) — 续行已中止。发送 `nemesisbot estop --release` 恢复。";
            self.finish_continuation_as_note(
                &mgr,
                task_id,
                &cont,
                note,
                task_source_node.as_deref(),
                reg_status,
                reg_summary,
                reg_error,
            )
            .await;
            return;
        }

        // 2. Build tool result content from task response（task_failed →
        //    "Error: ..." 文本，与旧语义同文）。
        let tool_result_content = if task_failed {
            format!(
                "Error: {}",
                task_error
                    .as_deref()
                    .unwrap_or("Task failed with unknown error")
            )
        } else {
            task_response.clone()
        };

        // 空键 legacy 分支：旧磁盘形态快照（session_key 字段诞生前落盘）
        // 无会话身份可闸/可装配——回主路装配没有主体（历史无从水合），
        // 整支**原样委托旧迷你循环**（B2 保留的回滚面，参数未收窄）：
        // 行为与改前逐字节一致（含丢失回执「以 error 结果续行 LLM 让
        // 模型自行向用户说明/重派」契约，adapters 丢失回执注入的回归锁
        // stale_bg_spawn_snapshot_* 钉的就是这段）。本分支之前仅只读
        // load + estop 检查，无闸状态变更，委托无双跑风险。
        if cont.session_key.is_empty() {
            info!(
                "[Continuation] legacy empty-session snapshot → old mini-loop path: task_id={}",
                task_id
            );
            self.handle_cluster_continuation(task_id, msg).await;
            return;
        }

        // 3. 会话串行闸：try_acquire 失败 = 该会话回合在跑。★ 在一切闸
        //    状态变更（claim/confirm/打标）之前——busy 臂全部无副作用返回
        //    （或入队），重投递由回合尾巴 drain 自然排空。
        if !self.try_acquire_session(&cont.session_key) {
            match self.concurrent_mode {
                // Queue/Steer：重投递消息入队（恒 next-turn——续行结果不
                // 做 steer 插话），入队后复查 busy 竞态孤儿（kick）。重投递
                // 重走 gate → Continuation → 本函数，届时会话已空闲。
                ConcurrentMode::Queue | ConcurrentMode::Steer => {
                    let mut redelivery = msg.clone();
                    redelivery.session_key = cont.session_key.clone();
                    let queued = crate::inbox::QueuedMessage {
                        msg: redelivery,
                        timestamp: chrono::Local::now().to_rfc3339(),
                        priority: crate::inbox::priority_of(msg),
                    };
                    match self
                        .inbox
                        .enqueue_for_mode(&cont.session_key, queued, false)
                    {
                        crate::inbox::EnqueueOutcome::QueuedForNextTurn
                        | crate::inbox::EnqueueOutcome::QueuedForNextStep => {
                            self.kick_orphaned_queue_if_idle(&cont.session_key);
                            info!(
                                "[Continuation] session busy — continuation re-delivery queued for next turn: task_id={}, session_key={}",
                                task_id, cont.session_key
                            );
                            return;
                        }
                        // 排队已满/软满：诚实终局——注记（含结果文本，脱敏后）
                        // 落库 + 出站（receipt=false + task_id 记账）+ 快照收口。
                        // 任务结果不静默丢弃（worker 已产出，用户必须可见）。
                        crate::inbox::EnqueueOutcome::Rejected
                        | crate::inbox::EnqueueOutcome::RejectedSoft => {
                            warn!(
                                "[Continuation] session busy and inbox full — delivering result as terminal note: task_id={}",
                                task_id
                            );
                            let note = format!(
                                "⏳ 会话排队已满，后台任务 {task_id} 的结果未能注入对话。任务结果：\n\n{tool_result_content}"
                            );
                            self.finish_continuation_as_note(
                                &mgr,
                                task_id,
                                &cont,
                                &note,
                                task_source_node.as_deref(),
                                reg_status,
                                reg_summary,
                                reg_error,
                            )
                            .await;
                            return;
                        }
                    }
                }
                // Reject：drain 只在 Queue/Steer 认领队列——排队即永久滞留。
                // 诚实终局同上（注记含结果文本）。
                ConcurrentMode::Reject => {
                    warn!(
                        "[Continuation] session busy in reject mode — delivering result as terminal note: task_id={}",
                        task_id
                    );
                    let note = format!(
                        "⏳ 会话正忙（并发模式拒绝新请求），后台任务 {task_id} 的结果未能注入对话。任务结果：\n\n{tool_result_content}"
                    );
                    self.finish_continuation_as_note(
                        &mgr,
                        task_id,
                        &cont,
                        &note,
                        task_source_node.as_deref(),
                        reg_status,
                        reg_summary,
                        reg_error,
                    )
                    .await;
                    return;
                }
            }
        }

        // 持守卫窗口：busy guard 覆盖整个续行回合（panic 展开兜底释放）；
        // drain 前显式 drop（重注入队首要能过 gate 的 busy 判定）。
        let _busy_guard = super::admission::SessionBusyGuard::new(self, &cont.session_key);

        // 子代理可见性（D5，2026-11-03）：delegate 任务终态回写（begin 在
        // `__ASYNC__` marker 落地侧 save_async_continuation）。后台子代理
        // 续行（`subagent_continuation`）的 task_id 不在注册表（行 id 是
        // `subagent:{uuid}`）→ finish 静默 miss，语义正是「不在册不回写」。
        // 用户侧 cancel 的本地视图会被真实结局覆盖——B 端结果才是任务真相，
        // cancel 只代表「不再等待显示」。位置：busy 裁决之后（重投递臂不是
        // 终态，不回写），终端臂各自先回写（estop/空键/排队满）。
        crate::subagent_registry::global().finish(
            task_id,
            registry_status_of(msg),
            None,
            (!task_response.is_empty()).then(|| task_response.clone()),
            task_error.clone(),
        );

        // 0. 单飞闸（发现 F 根修）：认领处理权防重复回调双路处理。必须在
        //    final_persisted/confirm 之前认领——磁盘快照保留到收口，重复
        //    回调会经盘上回退命中。
        if !mgr.claim_handling(task_id).await {
            debug!("[Continuation] task {task_id} 已有在途处理（重复回调诚实跳过）");
            return;
        }

        // 0b. HD：续行重投递幂等闸——上一轮执行已把最终回复落库并在磁盘
        //     快照打标（final_persisted），但进程在「落库 ↔ 快照回收」窗口
        //     内崩溃时 master 重启重查询会重新投递。再跑一遍 = LLM 重跑 +
        //     回复重复 append。见标直接诚实收尾：不再续行，只回收快照。
        if let Some(store) = &mgr.disk_store
            && let Ok(snapshot) = store.load(task_id)
            && snapshot.final_persisted
        {
            warn!(
                task_id = %task_id,
                "[Continuation] 快照已标记 final_persisted（崩溃前已落库），重投递跳过重复续行"
            );
            mgr.finish_handling(task_id).await;
            return;
        }

        // 1b. 相位严格校验——快照恢复状态机 checkpoint→restored→confirmed。
        //     键缺省 = Checkpoint（存活快照直达续行合法）；已 Confirmed 的
        //     重复续行（claim + final_persisted 双闸之后）在此第三道 loud
        //     拒绝，绝不静默续行。
        if let Err(e) = mgr.confirm_continuation(task_id) {
            warn!(task_id = %task_id, "[Continuation] 相位机拒绝续行: {e}");
            mgr.release_handling(task_id).await;
            return;
        }

        // 续行执行期盖会话级在途标记——history gate
        // （rewind/undo/redo/fork）acquire 前查 [`ContinuationManager::has_inflight_work`]
        // 诚实拒绝，防续行最终落库与截断/回退竞争。guard RAII 覆盖全部
        // return 路径（含错误早退），drop 即清。
        let _continuation_inflight = mgr.mark_inflight_session(&cont.session_key);

        // ConversationStart 观察面：真实 session_key（旧迷你循环用幻影键
        // continuation-{task_id}）、sender_id=continuation、content 带任务
        // 号。所有闸放行后才发（早退臂不发——旧迷你循环对 load-miss 等臂
        // 也发 start 是观测噪声）。record_last_channel 跳过——续行不改
        // 「最近通道」语义（回复走快照记录的原通道）。
        self.emit_observer_sync(crate::loop_executor::ObserverEvent::ConversationStart {
            trace_id: trace_id.clone(),
            session_key: cont.session_key.clone(),
            channel: cont.channel.clone(),
            chat_id: cont.chat_id.clone(),
            sender_id: "continuation".to_string(),
            content: format!("cluster_continuation:{task_id}"),
        })
        .await;

        // 4. merge 写回（发现 G 真机根修）：快照表征（LlmMessage）合入真实
        //    结果后**立即写回**（内存+磁盘）——「回调已到、续行未完」窗口内
        //    崩溃，恢复轮询 query_task_result 又落在 B 端存档清理之后时，
        //    盘上快照必须已带结果。写回后快照即恢复权威定义：重启 →
        //    recover_to_manager → merge 因真实结果在案不被 not-found 覆盖
        //    （[`crate::loop_continuation::merge_real_tool_result`]）。元数据
        //    不变，image 字节不落盘（既有剥字节水合语义）。
        let merged = crate::loop_continuation::merge_real_tool_result(
            cont.messages.clone(),
            &cont.tool_call_id,
            tool_result_content.clone(),
        );
        mgr.save_continuation_with_images(
            task_id,
            merged,
            &cont.tool_call_id,
            &cont.channel,
            &cont.chat_id,
            &cont.session_key,
            &cont.peer_id,
            &cont
                .image_refs_by_user_turn
                .iter()
                .flatten()
                .cloned()
                .collect::<Vec<String>>(),
        )
        .await;
        debug!("[Continuation] task {task_id} 合入回调结果后快照已写回（崩溃安全网更新，发现 G）");

        // 5. 活体历史装配（B1#2）：instance 从 store 水合（空则快照回填）
        //    → 真实结果种入（marker 原位替换 / assistant 后补 tool 轮 /
        //    user turn 兜底）→ set_history 回写。
        //    get_or_create 先于结果种入是硬契约（物化先行——防
        //    rebuild_from_chat_log 把 chat_log 行回放进上下文后再加一次，
        //    agent-bench context_integrity 实证）。装载摘要快照（finalize
        //    「cache 相对装载是否变化」判定的基线）紧跟其后。
        //    本 turn 文件变更/交付物缓冲从空开始（兜底上轮异常短路残留，
        //    与 run_agent_loop_internal 同点同规则）。
        self.turn_file_changes.lock().remove(&cont.session_key);
        self.turn_deliverables.lock().remove(&cont.session_key);
        let instance = self.get_or_create_instance(&cont.session_key);
        let summary_at_load = instance.get_summary_cache();
        let mut live_history = instance.get_history();
        // 实质轮判定：system 预置轮不算（instance 构造可能自带 system
        // seed，fresh instance 的 get_history() 并非恒空）。
        let has_substantive = live_history.iter().any(|t| t.role != "system");
        if !has_substantive && !cont.messages.is_empty() {
            // 快照回填（恢复语义保留——旧路径以快照为上下文装配体）：
            // store 无主体历史时以快照消息为底座（结果注入、收尾落库照
            // 常，恢复边沿自愈进正常形态）。生产主形态 store 物化先行不
            // 进此臂。system 消息不入历史（system 每轮由
            // config/build_messages 自持供给）；快照内存侧带水合图片字节
            // 的 images 不转（图片引用走 fallback_images 路径引用）。
            live_history = cont
                .messages
                .iter()
                .filter(|m| m.role != "system")
                .map(|m| crate::types::ConversationTurn {
                    role: m.role.clone(),
                    content: m.content.clone(),
                    tool_calls: m.tool_calls.clone().unwrap_or_default(),
                    tool_call_id: m.tool_call_id.clone(),
                    timestamp: chrono::Local::now().to_rfc3339(),
                    reasoning_content: m.reasoning_content.clone(),
                    tool_name: None,
                    tool_result_projection: None,
                    image_refs: Vec::new(),
                })
                .collect();
        }
        // 兜底臂图片引用：快照最后一条 user 轮的引用（新形态）；旧单层
        // 形态回退 image_refs 全集。
        let fallback_images = cont
            .image_refs_by_user_turn
            .last()
            .cloned()
            .unwrap_or_else(|| cont.image_refs.clone());
        let _applied = crate::loop_continuation::apply_real_result_to_history(
            &mut live_history,
            &cont.tool_call_id,
            task_id,
            &tool_result_content,
            &fallback_images,
        );
        instance.set_history(live_history);

        // 6. 主循环续行：run_llm_loop 全管线（工具全量供给 + dispatch
        //    瀑布：estop/hidden/Plan/安全 8 层/hooks/args_validator/
        //    turn_guard）。context.user = "continuation" 是 log_boundaries
        //    豁免标记（边界 sidecar 随每次异步回灌无界增长的同一失败形态，
        //    与 heartbeat/cron 豁免同族）。per-task cancel token 与主路
        //    gate 同生命周期（mint → run → remove）。
        let mut context = RequestContext::new(
            &cont.channel,
            &cont.chat_id,
            "continuation",
            &cont.session_key,
        );
        context.user = "continuation".to_string();
        let cancel_token = self.create_cancel_token(&cont.session_key);
        let events = self
            .resume_execution_with_token(
                &instance,
                &context,
                &trace_id,
                &cancel_token,
                Some(Self::CONTINUATION_TURN_BUDGET),
            )
            .await;
        self.remove_cancel_token(&cont.session_key);

        // 7. 收尾 = 共享 finalize（chat_log assistant 行带节点徽章 + work
        //    时间线 + store 全量 set_history + ConversationEnd + 标题任务）。
        //    Err（回合错误收尾）时错误文本已作为 assistant 行落盘（finalize
        //    内部），出站携带同一文本——旧迷你循环 LLM 失败出站
        //    `[LLM error: ...]` 的对应物，错误不静默。
        let final_reply = match self
            .finalize_turn_shared(
                &instance,
                &cont.session_key,
                &cont.channel,
                &cont.chat_id,
                &trace_id,
                start_time,
                // cron 字段不适用续行（None；prepare_round 的 cron 预算
                // 文案同样不适用——turn_budget 已单独传 20）。
                None,
                None,
                summary_at_load,
                // 集群续行归属：worker 节点名随 assistant 行落盘。
                task_source_node.as_deref(),
                &events,
            )
            .await
        {
            Ok(text) => text,
            Err(e) => e,
        };

        // 摘双 guard 后再 drain：busy 先于 drain（重注入队首要能过 gate
        // 的 busy 判定，守卫跨 drain 存活会误释放新 turn 的会话）；inflight
        // 同点清（续行已收尾，不再挡 history 闸）。
        drop(_busy_guard);
        drop(_continuation_inflight);
        self.drain_next_turn_queue(&cont.session_key).await;

        // 8. 出站直发（不经 finish_message——其 source_node/task_id 硬编码
        //    None，会丢续行归属与 deliveries 记账）。finalize 已落盘 →
        //    receipt=false + occupies=None（assistant 占行）。
        self.send_continuation_final(
            &cont.channel,
            &cont.chat_id,
            &cont.session_key,
            &final_reply,
            task_source_node.as_deref(),
            task_id,
        )
        .await;

        // 9. HD：落库即打标（磁盘快照回写）——此后到 finish_handling 之间
        //    崩溃的重投递由 0b 幂等闸拦下。
        self.mark_snapshot_final_persisted(&mgr, task_id);

        // 10. 收口（发现 F 根修）：最终回复已持久化/投递，此刻才回收内存
        //     条目 + 磁盘快照。进程崩溃时 recover_to_manager 仍能从盘上
        //     捞回本窗口内的快照。
        mgr.finish_handling(task_id).await;

        info!(
            "[Continuation] continuation turn finalized: task_id={}, content_len={}, target_channel={}",
            task_id,
            final_reply.len(),
            cont.channel
        );
    }

    /// 续行终局注记落库（store append 与 chat_log assistant 行带节点徽章、
    /// 磁盘快照 final_persisted 打标）。空键跳过（旧行为：无会话身份不落
    /// 库）。estop 臂 / 排队满臂 / Reject 忙臂共用。
    async fn persist_continuation_terminal(
        &self,
        mgr: &crate::loop_continuation::ContinuationManager,
        task_id: &str,
        session_key: &str,
        content: &str,
        source_node: Option<&str>,
    ) {
        if session_key.is_empty() {
            return;
        }
        crate::loop_continuation::persist_final_reply(
            self.session_store.as_ref().map(|v| v.as_ref()),
            session_key,
            &self.active_model.read().clone(),
            content,
            source_node,
        );
        self.mark_snapshot_final_persisted(mgr, task_id);
    }

    /// 终局注记臂汇总：D5 台账回写（真实终态三元组）+ 注记落库（store
    /// append + chat_log assistant 行带节点徽章）+ 出站 + 快照收口
    /// （finish_handling）。estop 臂 / 排队满臂 / Reject 忙臂共用——三者
    /// 都是「不再续行」的终局，都必须在返回前完成台账终态与快照生命周期
    /// 收口。
    #[allow(clippy::too_many_arguments)]
    async fn finish_continuation_as_note(
        self: &Arc<Self>,
        mgr: &crate::loop_continuation::ContinuationManager,
        task_id: &str,
        cont: &crate::loop_continuation::ContinuationData,
        note: &str,
        source_node: Option<&str>,
        reg_status: crate::subagent_registry::SubagentStatus,
        reg_summary: Option<String>,
        reg_error: Option<String>,
    ) {
        crate::subagent_registry::global().finish(
            task_id,
            reg_status,
            None,
            reg_summary,
            reg_error,
        );
        self.persist_continuation_terminal(mgr, task_id, &cont.session_key, note, source_node)
            .await;
        self.send_continuation_final(
            &cont.channel,
            &cont.chat_id,
            &cont.session_key,
            note,
            source_node,
            task_id,
        )
        .await;
        mgr.finish_handling(task_id).await;
    }

    /// 磁盘快照 final_persisted 打标（HD：落库即打标——此后到
    /// finish_handling 之间崩溃的重投递由 0b 幂等闸拦下）。打标失败仅
    /// warn（重投递时可能重复 append，毫秒级窗口，后果与旧行为同）。
    fn mark_snapshot_final_persisted(
        &self,
        mgr: &crate::loop_continuation::ContinuationManager,
        task_id: &str,
    ) {
        if let Some(store) = &mgr.disk_store
            && let Ok(mut snapshot) = store.load(task_id)
        {
            snapshot.final_persisted = true;
            if let Err(e) = store.save(&snapshot) {
                warn!(
                    task_id = %task_id,
                    error = %e,
                    "[Continuation] final_persisted 打标落盘失败（重投递时可能重复 append）"
                );
            }
        }
    }

    /// 续行终局回复出站（直发，不经 [`Self::finish_message`]——其
    /// source_node/task_id 硬编码 None，会丢续行归属与 deliveries 记账）。
    /// meta 形态与旧迷你循环 step 6 逐字段一致：model=display、
    /// session_key 随行、receipt=false（回合产物，已落盘）、occupies=None
    /// （assistant 占行）。
    async fn send_continuation_final(
        &self,
        channel: &str,
        chat_id: &str,
        session_key: &str,
        content: &str,
        source_node: Option<&str>,
        task_id: &str,
    ) {
        let Some(ref tx) = self.outbound_tx else {
            return;
        };
        let outbound = nemesis_types::channel::OutboundMessage {
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
            content: content.to_string(),
            message_type: String::new(),
            meta: nemesis_types::channel::OutboundMeta {
                model: Some(self.current_display_model()),
                // 会话键随行——跨进程续行的最终回复同样要能被 chat.sync 补拉。
                session_key: (!session_key.is_empty()).then(|| session_key.to_string()),
                // 集群续行归属：干活的是远端节点，web 通道据此渲染「节点 X」
                // 徽章。None = 不写键，出站帧与历史行为一致。
                source_node: source_node.map(|s| s.to_string()),
                // 项 C：最终回复携带任务 id，通道投递结果据此记账到
                // deliveries（notify_state 三段：pending→accepted→confirmed）。
                task_id: Some(task_id.to_string()),
                sync_via: None,
                note_kind: None,
                receipt: false,
                occupies: None,
            },
        };
        if let Err(e) = tx.send(outbound).await {
            warn!(
                "[Continuation] Failed to send continuation final response: {}",
                e
            );
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
    ///
    /// `occupies`：帧占行真相（OutboundMeta.occupies，行号锚漂移根修）。
    /// 调用点按落盘真相声明：瞬态回执（⏳/⚡/slash/维护两帧/直派收据）
    /// `Some(false)`；落盘留痕回执（💤 暂停排队/拒绝弹回——由 gate 侧
    /// GateOutcome::Immediate 携带）透传；终局回复 None（旧规则：
    /// user/assistant 占行）。
    pub(crate) async fn finish_message(
        &self,
        msg: &nemesis_types::channel::InboundMessage,
        response: String,
        err: Option<String>,
        check_sent_in_round: bool,
        receipt: bool,
        occupies: Option<bool>,
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
                    task_id: None,
                    sync_via: None,
                    note_kind: None,
                    // 回执标记由调用点声明语义（busy 排队/slash 确认=true，
                    // 终局回复=false）——消费方按会话键等终局回复时跳过回执。
                    receipt,
                    // 帧占行真相由调用点声明（行号锚漂移根修）。
                    occupies,
                },
            };
            if let Err(e) = tx.send(outbound).await {
                warn!("[AgentLoop] Failed to send outbound message: {}", e);
            }
        }
    }

    /// 行动代理 P0（2026-10-14）：connector 回执 IM 文本卡推送。写类/资金
    /// 类 op 执行后由 connector 工具经本方法把固定格式凭条卡投递回**发起
    /// 对话**（感知层合同 §3.5/§5：写类必现身，读类只入账本不推——推送
    /// 裁决在工具侧，这里只管投递）。绕过 `finish_message` 的
    /// sent-in-round 去重（凭条卡不是回合回复，不能被 message-tool 已回复
    /// 检查吞掉）；bus 模式无 outbound 泵（standalone/headless）时静默跳过。
    pub async fn push_connector_receipt_card(
        &self,
        channel: &str,
        chat_id: &str,
        session_key: &str,
        card: String,
    ) {
        let Some(ref tx) = self.outbound_tx else {
            return;
        };
        let outbound = nemesis_types::channel::OutboundMessage {
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
            content: card,
            message_type: String::new(),
            meta: nemesis_types::channel::OutboundMeta {
                model: Some(self.current_display_model()),
                session_key: (!session_key.is_empty()).then(|| session_key.to_string()),
                source_node: None,
                task_id: None,
                sync_via: None,
                // 凭条卡 = connector 写类 op 回执，非回合产物（按会话键等
                // 终局回复的消费方必须跳过）。瞬态帧不落盘 → 不占行号锚
                // （行号锚漂移根修）。
                note_kind: None,
                receipt: true,
                occupies: Some(false),
            },
        };
        if let Err(e) = tx.send(outbound).await {
            warn!("[AgentLoop] Failed to send connector receipt card: {}", e);
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
    /// W6：当前在途 turn 数（优雅停机 drain 的 gauge 读数）。
    pub fn in_flight_turns(&self) -> usize {
        self.in_flight_turns
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// W6：进入停机 drain——置位后泵拒绝新 turn（外部通道诚实回执，
    /// 内部通道静默丢），在途 turn 照常收尾。幂等（重复置位无害）。
    pub fn begin_shutdown_drain(&self) {
        self.draining
            .store(true, std::sync::atomic::Ordering::Release);
        info!(
            "[AgentLoop] 停机 drain 开始：入口已关闭，在途 turn = {}",
            self.in_flight_turns()
        );
    }

    /// W6：是否处于 drain 态。
    pub fn is_draining(&self) -> bool {
        self.draining.load(std::sync::atomic::Ordering::Acquire)
    }

    /// W6：有界等在途 turn 归零（100ms 轮询）。返回 true=已归零 /
    /// false=超时（调用方明示在途数后走强制 teardown）。
    pub async fn wait_turns_drained(&self, timeout: std::time::Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if self.in_flight_turns() == 0 {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    /// W6/BUG-20261025-33：多 loop 联合 drain 等待——gateway teardown 的
    /// drain 面不止主 loop（L6++ 项目 loop 与 G1 自建 agent loop 同为常驻
    /// AgentLoop，在途 turn 同样不该被停机 abort），全部 loop 共享同一有界
    /// 窗口轮询，任一在途即未排干。空列表视为已排干。
    pub async fn wait_loops_drained(
        loops: &[std::sync::Arc<AgentLoop>],
        timeout: std::time::Duration,
    ) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if loops.iter().all(|l| l.in_flight_turns() == 0) {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    pub(crate) fn spawn_turn_task<F>(&self, fut: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let permits = self.turn_permits.clone();
        let gauge = Arc::clone(&self.in_flight_turns);
        let handle = tokio::spawn(async move {
            // acquire_owned 把许可与 Arc 绑定，async 块结束时（含 abort 展开丢弃）释放。
            let _permit = match permits {
                Some(p) => Some(p.acquire_owned().await),
                None => None,
            };
            // W6：任务体全程持 in-flight gauge（guard Drop 保证 panic/abort
            // 展开路径也减账）。许可等待段不计——那不是在执行 turn。
            let _gauge = InFlightTurnGuard::acquire(&gauge);
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

        // P32（cache-warmer）：默认关（agents.cache_warmer.enabled=false）→
        // 一次新鲜读后直接返回，无定时器、无任务对象（零副作用）。开 →
        // 周期扫描任务（TTL/经济闸/锁纪律见 cache_warmer.rs 模块注释）。
        self.spawn_cache_warmer_if_enabled();

        // L8（内部专项总计划重评估）turn WAL 启动恢复：扫断头工具调用 →
        // 补 interrupted → agent 键会话合成续轮消息走**正常泵派发**（gate
        // /admission/busy 全语义同源）。在主循环消费前执行——续轮先于
        // 新消息起跑，与真实消息零差别。
        self.recover_interrupted_turns().await;

        while self.running.load(Ordering::Acquire) {
            match inbound_rx.recv().await {
                Some(mut msg) => {
                    self.pump_dispatch(&mut msg).await;
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

    /// 单条入站消息的泵级派发（L8 从 run_bus_impl 抽出，恢复循环与主循环
    /// 共用同一条形状）。rewrite → gate 分类 → 按结果内联或 spawn：
    /// **Continuation 保持与泵串行**（内联 await），其余臂 spawn tracked
    /// task——跨会话并发、同会话保序不变量原样保留。
    async fn pump_dispatch(self: &Arc<Self>, msg: &mut nemesis_types::channel::InboundMessage) {
        // W6 优雅停机 drain：关入口闸——draining 置位后拒绝新 turn（在途
        // 照常收尾；与 estop「冻结 agent 活动」正交）。内部通道静默丢
        // （日志留痕，防系统通道被回执刷屏）；外部通道发诚实回执。零处理
        // 早退（rewrite/gate 都不进）。
        if self.draining.load(Ordering::Acquire) {
            if is_internal_channel(&msg.channel) {
                info!(
                    "[AgentLoop] drain 中丢弃内部消息: channel={} session={}",
                    msg.channel, msg.session_key
                );
            } else {
                // drain 回执 = 瞬态系统回执（不落盘）→ 不占行号锚。
                self.finish_message(
                    msg,
                    DRAINING_RECEIPT.to_string(),
                    None,
                    false,
                    true,
                    Some(false),
                )
                .await;
            }
            return;
        }
        // slash 四段解析链（自定义命令 > 内置模板 > 技能回落）在
        // 生产泵路径的唯一接线点：改写必须在 gate 分类前完成，
        // 消息以最终形态参与 gate 判定（内置短路名 /compact 等
        // 在 rewrite 内部原样跳过；非 slash 消息早退零开销）。
        // 2026-09-28 真模型验证发现：此前的改写只接在 legacy
        // process_inbound_message（tests/inline 回退），泵直调
        // gate_inbound 导致四类 slash 在生产路径原样透传给 LLM。
        self.rewrite_custom_command(msg).await;
        match self.gate_inbound(msg) {
            GateOutcome::Continuation(task_id) => {
                info!(
                    "[AgentLoop] Handling cluster continuation for task {} (permits={})",
                    task_id, self.max_continuation_permits
                );
                // W6：inline continuation 也是在途 turn——gauge 与 spawn 臂同源。
                let _gauge = InFlightTurnGuard::acquire(&self.in_flight_turns);
                self.dispatch_continuation(task_id, msg).await;
            }
            GateOutcome::Immediate {
                agent_id: _,
                response,
                occupies,
            } => {
                // Busy receipt / busy bounce / queue-full / slash
                // reply — publish inline. Never touches
                // sent_in_round (may overlap the session's
                // running turn; see finish_message). 变体级语义 =
                // 静态短路回复（零 LLM 内容）→ 一律打 receipt=true：
                // 按会话键等终局回复的消费方（看板本地派发捕获）必须
                // 跳过——NB-1 实录：queue-ack 被当终局回复 → 57ms
                // 空交付 completed。occupies 由 gate 侧按落盘真相
                // 盖章（瞬态回执 Some(false) / 落盘留痕 Some(true)）。
                self.finish_message(msg, response, None, false, true, occupies)
                    .await;
            }
            GateOutcome::Coalesced { cron_job_id } => {
                // C4（2026-10-24 cron 合闸）：重复 tick 静默丢弃——零出站
                // （回执/落盘都省，防 UI 与历史被 skip 刷屏）。闸门侧已有
                // info 留痕，这里只补泵侧 debug 关联。
                debug!(
                    cron_job_id = %cron_job_id,
                    "[AgentLoop] coalesced cron tick dropped at pump (no receipt, no persistence)"
                );
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
                    // R294（BUG-20261031-235）：panic 兜底守卫——
                    // handle_maintenance（LLM 摘要，分钟级）panic 展开时
                    // 释放会话，不留永久 busy 死会话。正常路径在 drain 前
                    // 显式 drop（= 原 release_session）。
                    let _busy_guard = super::admission::SessionBusyGuard::new(&this, &session_key);
                    // ⏳ 前置回执 = 系统回执（receipt=true）；其后真回复
                    // （可能是 LLM 摘要）= 终局（receipt=false）。两帧都
                    // 是瞬态回执（落盘只有 user 行）→ Some(false) 不占
                    // 行号锚（行号锚漂移根修）。
                    this.finish_message(&m, receipt, None, false, true, Some(false))
                        .await;
                    let response = this
                        .handle_maintenance(kind, &session_key, &m.channel, &m.chat_id)
                        .await;
                    drop(_busy_guard);
                    // 排队消息排空（2026-09-28 真模型验证发现）：
                    // 分钟级维护期间排队的消息此前在维护尾巴
                    // 永久滞留 next_turn——与 admitted 尾部共用
                    // drain_next_turn_queue 对称排空。
                    this.drain_next_turn_queue(&session_key).await;
                    this.finish_message(&m, response, None, false, false, Some(false))
                        .await;
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
                    // 终局回复已由 turn 内部落盘 → None 旧规则（占行）。
                    this.finish_message(&m, response, err, true, false, None)
                        .await;
                });
            }
            GateOutcome::Admitted(admission) => {
                let this = self.clone();
                let m = msg.clone();
                self.spawn_turn_task(async move {
                    let (_, response, err) = this.process_admitted(&m, admission).await;
                    // 终局回复已由 turn 内部落盘 → None 旧规则（占行）。
                    this.finish_message(&m, response, err, true, false, None)
                        .await;
                });
            }
        }
    }

    /// L8 turn WAL 启动恢复：`recover_and_finalize` 产出断头候选 →
    /// 预置键会话合成续轮消息（session_key 原样回填——route_message 对
    /// `is_preset_session_key` 命中键原样采纳，重路由确定性落回原会话）→
    /// 走 [`Self::pump_dispatch`] 正常闸门。非预置键（standalone fallback /
    /// 集群复合键）不自动续轮——重路由不保证落回原会话（fallback 按
    /// sender/chat 重组、集群会话有自己的 G5 恢复链），诚实放弃，
    /// interrupted 留痕照写。
    async fn recover_interrupted_turns(self: &Arc<Self>) {
        let Some(wal) = self.turn_wal() else {
            return;
        };
        let candidates = wal.recover_and_finalize();
        if candidates.is_empty() {
            return;
        }
        info!(
            "[AgentLoop] Turn WAL 恢复：{} 个断头工具调用",
            candidates.len()
        );
        for cand in candidates {
            let rec = &cand.record;
            if !crate::r#loop::is_preset_session_key(&rec.session_key) {
                info!(
                    "[AgentLoop] Turn WAL：会话 {} 非预置键，跳过自动续轮（interrupted 已留痕）",
                    rec.session_key
                );
                continue;
            }
            let content = format!(
                "⟳ 系统续轮：上次进程在工具执行中被中断（tool={}，args={}）。该次调用的结果已丢失；请从中断处继续完成任务（必要时重新执行该工具）。",
                rec.tool, rec.args_preview
            );
            let mut msg = nemesis_types::channel::InboundMessage {
                channel: if rec.channel.is_empty() {
                    "web".to_string()
                } else {
                    rec.channel.clone()
                },
                sender_id: "system".to_string(),
                chat_id: rec.chat_id.clone(),
                content,
                media: Vec::new(),
                session_key: rec.session_key.clone(),
                correlation_id: String::new(),
                metadata: [("turn_wal_resume".to_string(), "1".to_string())]
                    .into_iter()
                    .collect(),
                voice_playback: None,
            };
            info!(
                "[AgentLoop] Turn WAL 续轮：session={} tool={}",
                rec.session_key, rec.tool
            );
            self.pump_dispatch(&mut msg).await;
        }
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
        // 停机令牌（2026-10-24 tokio 停机 panic 根修）：唤醒所有在
        // retry_transient 退避 sleep 里等停机的任务（process_direct 直调
        // 任务不在 turn_task_handles，下面的 abort 覆盖不到——guardian
        // judge / 看板 moderator / headless run 都走那条路）。
        self.stop_token.cancel();
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
    /// TODO: DEL（续行回主路 2026-11-12）：本直通包装随旧迷你循环退役——
    /// 生产主路径（`dispatch_continuation`）已切到 `run_continuation_turn`；
    /// 仍在生产的唯一调用点 = 空键 legacy 分支（旧磁盘形态快照无会话主体，
    /// 整支委托本路径保持改前行为，见 `run_continuation_turn`）。物理删除
    /// 需先裁决空键 legacy 形态的终局去向。保留亦为 s9_tests 等套件直驱
    /// 旧路径行为面。
    /// Similarly, maybe_summarize calls the standalone
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
            // BUG-20261106-041：estop 句柄（Arc clone 后放锁——guard 不跨 .await）。
            let cont_estop = self.security.estop.read().clone();
            // L28：续行 cap 与主循环 chat_opts 同源。
            let max_tokens_cap = self
                .current_max_tokens_override(Some(&cont_model))
                .unwrap_or(nemesis_types::capability::DEFAULT_MAX_TOKENS as u32);
            // L28+：阶梯封顶与主循环同源。
            let max_tokens_ceiling = self.current_max_output_ceiling();
            if let Some(ref tx) = self.outbound_tx {
                crate::loop_continuation::handle_cluster_continuation(
                    mgr.as_ref(),
                    task_id,
                    task_response,
                    task_failed,
                    task_error,
                    cont_provider.as_ref(),
                    &cont_model,
                    max_tokens_cap,
                    max_tokens_ceiling,
                    &self.tools,
                    tx,
                    self.observer_manager.clone(),
                    self.session_store.as_ref().map(|v| v.as_ref()),
                    // F-F：active 模型 vision 解析。
                    self.current_vision().supported,
                    task_source_node,
                    cont_estop.as_deref(),
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

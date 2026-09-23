//! 其余装配 setter 与访问器：observer/cluster/security/session_store/data_store/forge/route_resolver/channel_manager/state_manager/continuation/estop/approval/question/memory 族/workflow_engine/agent_event_tx、wiring_status、get_registry/get_cluster/provider_arc/config_mut。
//!
//! P1 自 `loop.rs` 物理搬迁（docs/PLAN/2026-09-23_agentloop-god-object-decomposition.md §3.2）；语义零变化。
use super::prelude::*;
use super::*;

impl AgentLoop {
    /// 对话生成（2026-09-22）：装配工作流引擎引用。工厂在 loop 构造后从
    /// SharedResources.workflow_engine 调用（feature-gated：no-workflow
    /// 编译不存在此方法）。
    #[cfg(feature = "workflow")]
    pub fn set_workflow_engine(
        &self,
        engine: std::sync::Arc<nemesis_workflow::engine::WorkflowEngine>,
    ) {
        *self.workflow_engine.write() = Some(engine);
    }

    /// 绑定全局急停状态。工厂每次重建 loop 都调一次，所以急停状态在 agent
    /// 重启后自动保持（状态本体在 `SharedResources` 上，不在 loop 上）。
    pub fn set_estop(&self, estop: Arc<crate::estop::EstopState>) {
        *self.estop.write() = Some(estop);
    }

    /// M7：装配审批响应端（gateway 把 WebApprovalManager 挂上来；重复调用
    /// 覆盖前值）。WSAPI approval handler 经 `approval_responder()` 触达。
    pub fn set_approval_responder(
        &self,
        responder: Arc<dyn nemesis_types::agent::ApprovalResponder>,
    ) {
        *self.approval_responder.write() = Some(responder);
    }

    /// M7：取审批响应端（未装配 = `None`——handler 诚实报「未装配」）。
    pub fn approval_responder(&self) -> Option<Arc<dyn nemesis_types::agent::ApprovalResponder>> {
        self.approval_responder.read().clone()
    }

    /// F7：挂结构化提问响应端（gateway 装配 WebQuestionBroker 后调用）。
    pub fn set_question_responder(
        &self,
        responder: Arc<dyn nemesis_types::agent::QuestionResponder>,
    ) {
        *self.question_responder.write() = Some(responder);
    }

    /// F7：取提问响应端（未装配 = `None`——handler 诚实报「未装配」）。
    pub fn question_responder(&self) -> Option<Arc<dyn nemesis_types::agent::QuestionResponder>> {
        self.question_responder.read().clone()
    }

    /// J5：装配 doom-loop 审批提问端（gateway 与 F7 responder 注入同一
    /// WebQuestionBroker Arc——同一 broker 的两个 trait 各挂一槽）。
    /// 未装配 = escalation 审批化不可用（诚实回退现行为）。
    pub fn set_question_asker(&self, asker: Arc<dyn nemesis_types::agent::QuestionAsker>) {
        *self.question_asker.write() = Some(asker);
    }

    /// J5：取审批提问端（未装配 = `None`）。
    pub(crate) fn question_asker(&self) -> Option<Arc<dyn nemesis_types::agent::QuestionAsker>> {
        self.question_asker.read().clone()
    }

    /// K1a (U14): 注册一个用户工具钩子。pre 在固定 security 闸之后、工具
    /// 执行之前运行；post 在执行之后、Forge 记录之前运行（详见
    /// `crate::hooks` 模块文档）。RwLock 注册——运行中随时可挂。
    pub fn add_tool_hook(&self, hook: Arc<dyn crate::hooks::ToolHook>) {
        self.hooks.tool_hooks.write().add(hook);
    }

    /// K1b (U14): 注册一个 LLM 调用级钩子。pre 在 messages 组装后、
    /// LlmRequest observer 事件前运行（可 Append 提醒消息 / 拦下本轮）；
    /// post 在响应错误恢复后、LlmResponse observer 事件前运行（可
    /// Allow/Replace/有限 Retry/Block）。详见 `crate::hooks` 模块文档。
    pub fn add_llm_hook(&self, hook: Arc<dyn crate::hooks::LlmHook>) {
        self.hooks.llm_hooks.write().add(hook);
    }

    /// K2 (U14): 注册一个 prompt/turn 生命周期钩子。on_user_prompt 在
    /// `run_with_trace` 顶部、消息进 history **之前**运行（拦截则模型
    /// 永远看不到该消息）；on_turn_end 在最终答案被接受后、Done 事件前
    /// 运行（可注入 feedback 要求再答一轮，预算封顶 fail-open）。
    pub fn add_lifecycle_hook(&self, hook: Arc<dyn crate::hooks::LifecycleHook>) {
        self.hooks.lifecycle_hooks.write().add(hook);
    }

    /// Wire the re-injection sender for the queue-drain path (round-5 review
    /// fix). The adapter passes a clone of the SAME mpsc sender that feeds
    /// `run_bus_arc`'s receiver, so a drained queued head re-enters the normal
    /// consumer loop and its reply gets full post-processing (rpc prefix,
    /// sent_in_round, error conversion). Call after the channel pair is
    /// created, before `run_bus_*` starts consuming.
    pub fn set_reinject_tx(
        &self,
        tx: tokio::sync::mpsc::Sender<nemesis_types::channel::InboundMessage>,
    ) {
        *self.reinject_tx.write() = Some(tx);
    }

    /// Stash the memory tool executor so the gateway can later attach an approval
    /// gate via `set_memory_approval_gate`. Called by the factory after building
    /// the shared tool config.
    #[cfg(feature = "memory")]
    pub fn set_memory_executor(&self, exec: Arc<nemesis_memory::memory_tools::MemoryToolExecutor>) {
        *self.memory.memory_executor.write() = Some(exec);
    }

    /// P3.1 (sixth batch): wire the auto-inject channel — the memory manager
    /// (read-only retrieval) plus the `auto_inject`/`top_k` flags read from
    /// `config.enhanced_memory.json`. Passing `auto_inject=false` (the
    /// default everywhere) keeps the loop byte-identical to pre-P3.1.
    /// The `manager` param is cfg-gated: absent (and so not suppressable)
    /// under `--no-default-features` — memory injection is a no-op there.
    #[cfg(feature = "memory")]
    pub fn set_memory_inject(
        &self,
        manager: Option<Arc<nemesis_memory::manager::MemoryManager>>,
        auto_inject: bool,
        top_k: usize,
    ) {
        *self.memory.memory_inject_manager.write() = manager;
        *self.memory.memory_inject_cfg.write() = (auto_inject, top_k);
    }

    /// P3.1 stub (memory feature off): accepts only the flags (the manager
    /// param doesn't exist without the memory crate). Injection stays off —
    /// `memory_inject_cfg` records `(false, _)` from the real builder, but
    /// `prefetch_memory_context` returns None regardless (no manager).
    #[cfg(not(feature = "memory"))]
    pub fn set_memory_inject(&self, auto_inject: bool, top_k: usize) {
        *self.memory.memory_inject_cfg.write() = (auto_inject, top_k);
    }

    /// Attach an approval gate to the memory executor (if one was stashed). After
    /// this, agent `memory_store`/`memory_forget` calls require approval.
    #[cfg(feature = "memory")]
    pub fn set_memory_approval_gate(
        &self,
        gate: Arc<dyn nemesis_memory::memory_tools::MemoryApprovalGate>,
    ) {
        if let Some(ref exec) = *self.memory.memory_executor.read() {
            exec.set_approval_gate(gate);
        }
    }

    /// Set the channel manager reference for listing enabled channels.
    /// Mirrors Go's `SetChannelManager()`.
    pub fn set_channel_manager(&self, enabled_channels: Vec<String>) {
        *self.channel_manager_channels.lock() = enabled_channels;
    }

    /// Set the state manager for recording last channel/chat ID.
    /// Mirrors Go's `state.NewManager(workspace)`.
    pub fn set_state_manager(
        &mut self,
        mgr: Arc<nemesis_state::workspace_state::WorkspaceStateManager>,
    ) {
        self.state_manager = Some(mgr);
        debug!("[AgentLoop] State manager configured");
    }

    /// Set the observer callback for event emission.
    /// Mirrors Go's `SetObserverManager()`.
    pub fn set_observer_callback(
        &mut self,
        cb: Arc<dyn Fn(&str, &serde_json::Value) + Send + Sync>,
    ) {
        self.observer_callback = Some(cb);
        debug!("[AgentLoop] Observer callback configured");
    }

    /// Set the route resolver for multi-agent message routing.
    /// Mirrors Go's `AgentLoop.registry` (RouteResolver).
    /// When set, `process_inbound_message` uses the full 7-level priority
    /// cascade to determine agent and session key.
    pub fn set_route_resolver(&mut self, resolver: RouteResolver) {
        self.route_resolver = Some(resolver);
        info!("[AgentLoop] Route resolver configured");
    }

    /// Set the cluster reference.
    ///
    /// Accepts an `Arc<dyn Any + Send + Sync>` to avoid a compile-time dependency
    /// on the `nemesis-cluster` crate. The concrete cluster instance should be
    /// wrapped with `Arc::new(cluster) as Arc<dyn Any + Send + Sync>`.
    /// Mirrors Go's `AgentLoop.cluster` field assignment.
    pub fn set_cluster(&mut self, cluster: Arc<dyn std::any::Any + Send + Sync>) {
        self.cluster = Some(cluster);
    }

    /// Get the cluster reference, if set.
    ///
    /// Returns `Option<&Arc<dyn Any + Send + Sync>>`. The caller is responsible
    /// for downcasting to the concrete cluster type. Mirrors Go's `GetCluster()`.
    pub fn get_cluster(&self) -> Option<&Arc<dyn std::any::Any + Send + Sync>> {
        self.cluster.as_ref()
    }

    /// Set the observer manager for Phase 5 event emission.
    /// Mirrors Go's `SetObserverManager()`.
    pub fn set_observer_manager(&mut self, mgr: Arc<nemesis_observer::Manager>) {
        self.observer_manager = Some(mgr);
    }

    /// Set the security plugin for pre-execution tool safety checks.
    /// Mirrors Go's SecurityPlugin registered via PluginManager.
    #[cfg(feature = "security")]
    pub fn set_security_plugin(&mut self, plugin: Arc<nemesis_security::pipeline::SecurityPlugin>) {
        self.security_plugin = Some(plugin);
    }

    /// Set the session store, replacing the default in-memory store.
    /// Call this to enable disk-persisted conversation history.
    pub fn set_session_store(&mut self, store: Arc<crate::session::SessionStore>) {
        self.session_store = Some(store);
    }

    /// Get the session store, if one is configured.
    ///
    /// Used by callers outside the main agent loop (e.g. cluster_agent) that need
    /// to read/write history via the same SessionStore the loop would use.
    pub fn session_store(&self) -> Option<&Arc<crate::session::SessionStore>> {
        self.session_store.as_ref()
    }

    /// Set the continuation manager for async cluster RPC callbacks.
    ///
    /// When set, `cluster_continuation` messages intercepted by the bus loop
    /// will trigger snapshot loading and LLM resumption.
    pub fn set_continuation_manager(
        &mut self,
        manager: Arc<crate::loop_continuation::ContinuationManager>,
    ) {
        self.continuation_manager = Some(manager);
    }

    /// G4 (devtool-upgrade 阶段 3)：列出重启时遗留在盘上的后台 subagent
    /// pending 快照（`bg_` 前缀，只读不删）。gateway 适配器启动期以这些 id
    /// 注入诚实丢失回执（后台任务是进程内 tokio 任务，gateway 重启即终止、
    /// 无对端可 poll）——回灌走正常续行路径，快照由 resume 自清。
    pub fn list_stale_bg_spawn_task_ids(&self) -> Vec<String> {
        self.continuation_manager
            .as_ref()
            .map(|m| m.list_bg_spawn_pending_sync())
            .unwrap_or_default()
    }

    /// Set the data store for recording LLM usage statistics.
    pub fn set_data_store(&mut self, store: Arc<nemesis_data::DataStore>) {
        self.data_store = Some(store);
    }

    /// 用量记账存储只读访问（全自动流转 P5/E1 二期 token 回传：cluster
    /// agent 执行完 peer_chat 任务后，以 session 聚合差值提取本轮 token
    /// 用量并随回调回传 master 记账）。未装配 = None。
    pub fn data_store(&self) -> Option<Arc<nemesis_data::DataStore>> {
        self.data_store.clone()
    }

    /// Set the Forge instance for experience collection.
    #[cfg(feature = "forge")]
    pub fn set_forge(&mut self, forge: Arc<nemesis_forge::forge::Forge>) {
        self.forge = Some(forge);
    }

    /// Get the observer manager, if set.
    /// Mirrors Go's `GetObserverManager()`.
    pub fn get_observer_manager(&self) -> Option<&Arc<nemesis_observer::Manager>> {
        self.observer_manager.as_ref()
    }

    /// Get the agent registry (bus mode).
    pub fn get_registry(&self) -> Option<&Arc<AgentRegistry>> {
        self.registry.as_ref()
    }

    /// Get a clone of the provider Arc.
    pub fn provider_arc(&self) -> Arc<dyn LlmProvider> {
        self.provider.read().clone()
    }

    /// Get a mutable reference to the agent config.
    pub fn config_mut(&mut self) -> &mut AgentConfig {
        &mut self.config
    }

    // -----------------------------------------------------------------------
    // Bus-integrated main loop
    // -----------------------------------------------------------------------

    /// F1：注入 M1a 事件广播发送端（`SharedResources.agent_event_tx`）。
    /// 模式切换发布 `ModeChanged` 用；`None` = 发布静默跳过。
    pub fn set_agent_event_tx(
        &self,
        tx: Option<tokio::sync::broadcast::Sender<nemesis_types::agent::AgentEvent>>,
    ) {
        *self.agent_event_tx.write() = tx;
    }

    /// ASM-08（2026-09-16 横扫存量加固）：关键接线面快照。
    ///
    /// gateway 级装配点（主/集群/项目 loop）在装配完成时用它断言「该接的
    /// 都接了」，把 T37①/F-U3-2a/ASM-01 一族的「漏接静默」变成装配即炸
    /// （单测自己动手装配所以永远绿，生产装配的接线缺口只能在装配点掀）。
    /// 只报告状态不含策略——「哪些必须非空」的裁决归调用方
    /// （`agent_factory::assert_gateway_critical_wiring`）。`security_plugin`
    /// 条目仅 `security` feature 编入时出现。
    pub fn wiring_status(&self) -> Vec<(&'static str, bool)> {
        // security feature 关闭时无 push，mut 冗余——精确 cfg 门控。
        #[cfg_attr(not(feature = "security"), allow(unused_mut))]
        let mut status = vec![
            ("estop", self.estop.read().is_some()),
            ("workspace_root", self.workspace_root.read().is_some()),
            ("config_path", self.config_path.read().is_some()),
            ("pricing_store", self.pricing_store.read().is_some()),
        ];
        #[cfg(feature = "security")]
        status.push(("security_plugin", self.security_plugin.is_some()));
        status
    }
}

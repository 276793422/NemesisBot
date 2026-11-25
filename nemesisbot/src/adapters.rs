//! Service adapters that bridge async implementations to sync LifecycleService traits.
//!
//! These adapters wrap concrete service instances from individual crates
//! (nemesis-health, nemesis-heartbeat, nemesis-channels) and implement
//! the sync `LifecycleService`-based traits defined in nemesis-services.
//!
//! The async `start()` methods are spawned as background tokio tasks.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(feature = "health")]
use nemesis_services::HealthServer as HealthServerTrait;
#[cfg(feature = "heartbeat")]
use nemesis_services::HeartbeatService as HeartbeatServiceTrait;
use nemesis_services::{
    AgentLoopService as AgentLoopServiceTrait, ChannelManager as ChannelManagerTrait,
    LifecycleService,
};

// ---------------------------------------------------------------------------
// HealthServer adapter
// ---------------------------------------------------------------------------

/// Adapter wrapping `nemesis_health::HealthServer` to implement the
/// `nemesis_services::HealthServer` trait.
#[cfg(feature = "health")]
pub struct HealthServerAdapter {
    inner: Arc<nemesis_health::server::HealthServer>,
    started: AtomicBool,
}

#[cfg(feature = "health")]
impl HealthServerAdapter {
    pub fn new(inner: Arc<nemesis_health::server::HealthServer>) -> Self {
        Self {
            inner,
            started: AtomicBool::new(false),
        }
    }
}

#[cfg(feature = "health")]
impl LifecycleService for HealthServerAdapter {
    fn start(&self) -> Result<(), String> {
        if self.started.swap(true, Ordering::SeqCst) {
            return Ok(()); // Already started
        }
        let inner = self.inner.clone();
        tokio::spawn(async move {
            // start_with_shutdown（非裸 start()）：stop() 靠 inner 的 oneshot
            // 触发 serve 优雅退出并释放监听端口——BotService restart 链
            // （persona 切换 / config 保存重启）依赖 stop→start 可重绑。
            // 裸 start() 不接停机通道，旧 serve 任务永生占端口，后续每次
            // start 都 bind 失败（Address already in use）且 stop 恒无效。
            if let Err(e) = inner.start_with_shutdown().await {
                tracing::error!("[Main] Health server error: {}", e);
            }
        });
        Ok(())
    }

    fn stop(&self) -> Result<(), String> {
        // 优雅停机：oneshot tx 已被 take = 未在跑/已停，天然 no-op（幂等）。
        self.inner.stop();
        self.started.store(false, Ordering::SeqCst);
        Ok(())
    }
}

#[cfg(feature = "health")]
impl HealthServerTrait for HealthServerAdapter {}

// ---------------------------------------------------------------------------
// HeartbeatService adapter
// ---------------------------------------------------------------------------

/// Adapter wrapping `nemesis_heartbeat::HeartbeatService` to implement the
/// `nemesis_services::HeartbeatService` trait.
#[cfg(feature = "heartbeat")]
pub struct HeartbeatServiceAdapter {
    inner: Arc<nemesis_heartbeat::service::HeartbeatService>,
    started: AtomicBool,
}

#[cfg(feature = "heartbeat")]
impl HeartbeatServiceAdapter {
    pub fn new(inner: Arc<nemesis_heartbeat::service::HeartbeatService>) -> Self {
        Self {
            inner,
            started: AtomicBool::new(false),
        }
    }
}

#[cfg(feature = "heartbeat")]
impl LifecycleService for HeartbeatServiceAdapter {
    fn start(&self) -> Result<(), String> {
        if self.started.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let inner = self.inner.clone();
        tokio::spawn(async move {
            if let Err(e) = inner.start().await {
                tracing::error!("[Main] Heartbeat service error: {}", e);
            }
        });
        Ok(())
    }

    fn stop(&self) -> Result<(), String> {
        self.inner.stop();
        self.started.store(false, Ordering::SeqCst);
        Ok(())
    }
}

#[cfg(feature = "heartbeat")]
impl HeartbeatServiceTrait for HeartbeatServiceAdapter {}

// ---------------------------------------------------------------------------
// ChannelManager adapter
// ---------------------------------------------------------------------------

/// Adapter wrapping `nemesis_channels::ChannelManager` to implement the
/// `nemesis_services::ChannelManager` trait.
#[allow(dead_code)]
pub struct ChannelManagerAdapter {
    inner: Arc<nemesis_channels::manager::ChannelManager>,
    enabled_channels: Vec<String>,
    started: AtomicBool,
}

impl ChannelManagerAdapter {
    #[allow(dead_code)]
    pub fn new(
        inner: Arc<nemesis_channels::manager::ChannelManager>,
        enabled_channels: Vec<String>,
    ) -> Self {
        Self {
            inner,
            enabled_channels,
            started: AtomicBool::new(false),
        }
    }
}

impl LifecycleService for ChannelManagerAdapter {
    fn start(&self) -> Result<(), String> {
        if self.started.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let inner = self.inner.clone();
        tokio::spawn(async move {
            if let Err(e) = inner.start_all().await {
                tracing::error!("[Main] Channel manager start error: {}", e);
            }
        });
        Ok(())
    }

    fn stop(&self) -> Result<(), String> {
        let inner = self.inner.clone();
        tokio::spawn(async move {
            if let Err(e) = inner.stop_all().await {
                tracing::error!("[Main] Channel manager stop error: {}", e);
            }
        });
        self.started.store(false, Ordering::SeqCst);
        Ok(())
    }
}

impl ChannelManagerTrait for ChannelManagerAdapter {
    fn enabled_channels(&self) -> Vec<String> {
        self.enabled_channels.clone()
    }
}

// ---------------------------------------------------------------------------
// AgentLoop adapter
// ---------------------------------------------------------------------------

/// All mutable state protected by a single Mutex.
/// This ensures start()/stop() are inherently atomic — no intermediate
/// state is ever visible to concurrent callers.
struct AgentLoopState {
    /// Current AgentLoop instance. `Some` = running, `None` = stopped.
    agent_loop: Option<Arc<nemesis_agent::r#loop::AgentLoop>>,
    /// Handle to the inbound bridge task (bus broadcast → mpsc).
    bridge_handle: Option<tokio::task::JoinHandle<()>>,
    /// 主循环任务的 abort 句柄（BUG-20261126 监督重设计）：JoinHandle 所
    /// 有权移交监督任务 await（死因裁决），state 只留 abort 能力——
    /// `AbortHandle::abort` 与结果读取解耦，stop 不再需要任务句柄本体。
    agent_abort: Option<tokio::task::AbortHandle>,
    /// 本代停机证词（一代一面）：start() 每代新建，stop() 置位。监督
    /// 任务只持自家代际的 clone，老监督永不读新代证词——跨代污染
    /// （旧 expected_stop 共享旗被 start 复位引发的连环误杀）结构性消失。
    /// 仅 `Ok(())` 结局需要它消歧（al.stop() 先置 running=false、任务
    /// 先于 abort 自然收班 = Ok；无证词的 Ok = 无人叫停的自发退出）。
    stop_witness: Option<Arc<AtomicBool>>,
}

/// 运行期 loop 级接线 stash（BUG-20261108-012 横向收尾）：gateway 装配
/// 期打到**初始 loop 实例**上的 responder/broker setter（审批应答端 M7、
/// question 应答端 + doom-loop asker F7/J5）。loop 重建路径（stop→start
/// →factory）不会重放这些 setter——不 stash 的话重启后新 loop 的
/// `approval_responder()`/`question_responder()` 恒 None，WSAPI
/// approval.respond / question.respond 诚实报「未装配」，Dashboard 卡
/// 有卡无答（阻塞端 auditor/broker 走 SharedResources 共享槽不受影响
/// ——正是这种「半边槽化」让缺陷潜伏）。这里 stash 全量、start() 重放。
#[derive(Default)]
struct RuntimeWiring {
    approval_responder: Option<Arc<dyn nemesis_types::agent::ApprovalResponder>>,
    question_responder: Option<Arc<dyn nemesis_types::agent::QuestionResponder>>,
    question_asker: Option<Arc<dyn nemesis_types::agent::QuestionAsker>>,
}

/// Adapter wrapping `nemesis_agent::AgentLoop` to implement the
/// `nemesis_services::AgentLoopService` trait.
///
/// On `start()`: calls `build_agent_loop()` to create a fresh AgentLoop from
/// disk config, subscribes to the message bus inbound broadcast, creates an
/// mpsc bridge, and spawns the agent loop's `run_bus_arc()`.
/// On `stop()`: aborts the inbound bridge, drops the old AgentLoop entirely.
/// The outbound bridge (agent → bus) is persistent and created separately
/// in gateway.rs; it survives stop/start cycles.
///
/// **Thread safety**: All mutable state lives in `Mutex<AgentLoopState>`.
/// `start()` and `stop()` hold this lock for the entire operation, making
/// them inherently serial. No intermediate state is possible.
pub struct AgentLoopServiceAdapter {
    /// Single Mutex protecting all mutable state.
    state: std::sync::Mutex<AgentLoopState>,
    /// Shared resources for factory function.
    shared: Arc<crate::agent_factory::SharedResources>,
    /// Shared reference with AppState — updated on each start/stop.
    agent_loop_ref: Arc<parking_lot::RwLock<Option<Arc<nemesis_agent::r#loop::AgentLoop>>>>,
    bus: Arc<nemesis_bus::MessageBus>,
    /// 运行期接线 stash（见 [`RuntimeWiring`]）：wire_* 写入、start() 重放
    /// 到重建出的新 loop。与 state 锁分离（wire_* 在 run_runtime 装配期
    /// 调用，不与 start/stop 竞争同一把锁）。
    runtime_wiring: std::sync::Mutex<RuntimeWiring>,
    /// 主桥 skip 谓词——命中=true 的消息**不喂主 loop**
    /// （由项目调度器转发进对应项目 loop）。装配期 set 一次（OnceLock），
    /// start() 每次读取 → loop stop/start 循环后谓词仍生效。默认未设置 =
    /// 全部放行（既有行为零影响）。谓词实现 = ProjectLoopManager:
    /// bridge_should_skip，单一裁决点在 route_decision 纯函数。
    skip_predicate: std::sync::OnceLock<
        Arc<dyn Fn(&nemesis_types::channel::InboundMessage) -> bool + Send + Sync>,
    >,
    /// Tokio runtime handle captured at construction time.
    /// Needed because tray callbacks run on the winit thread (no tokio context),
    /// but `start()` needs to spawn async tasks on the tokio runtime.
    rt: tokio::runtime::Handle,
    /// 主循环任务监督（BUG-20261123-003 引入，BUG-20261126 重设计）：
    /// 回合体除工具执行（tool_dispatch 的 catch_unwind）外无 panic 隔离
    /// （admission.rs SessionBusyGuard 注释自证），任务 panic = 主循环
    /// 静默死亡。监督任务 **await 任务句柄**（tokio 直接报死因，事件
    /// 驱动零轮询）：`Err(is_cancelled)` = 我们的 abort（stop）→ 安静
    /// 退役；`Ok(())` + 本代停机证词 = al.stop() 正常收班 → 安静退役；
    /// `Err(is_panic)` 或无证词的 `Ok(())` = 真异常死亡 → 触发
    /// restart_hook（stop+start 重建 fresh loop）+ 退避 1s/10s/60s，
    /// 第 4 次+只留痕不重启（panic 风暴 = 系统性问题，人工介入；计数
    /// 进程生命周期累计，gateway 重启清零）。监督随句柄 resolved 自然
    /// 退休——旧设计的「每代永生监督盯上一代尸体」连环误杀链
    /// （2026-11-25 人格切换三连误杀事故）结构性不可能复发。
    panic_restart_count: std::sync::atomic::AtomicU64,
    /// 重启回调（gateway 装配点注入 `adapter.supervisor_restart` 包装闭包）。
    /// None = 只 error! 留痕不自动重启（诚实降级，监督观测面仍在）。
    restart_hook: std::sync::Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl AgentLoopServiceAdapter {
    /// Create a new adapter with an initial AgentLoop (from first factory call in gateway.rs).
    /// The adapter starts in "stopped" state — `start()` must be called to begin processing.
    pub fn new(
        initial_agent_loop: Arc<nemesis_agent::r#loop::AgentLoop>,
        shared: Arc<crate::agent_factory::SharedResources>,
        bus: Arc<nemesis_bus::MessageBus>,
        agent_loop_ref: Arc<parking_lot::RwLock<Option<Arc<nemesis_agent::r#loop::AgentLoop>>>>,
    ) -> Self {
        Self {
            state: std::sync::Mutex::new(AgentLoopState {
                agent_loop: Some(initial_agent_loop),
                bridge_handle: None,
                agent_abort: None,
                stop_witness: None,
            }),
            shared,
            agent_loop_ref,
            bus,
            runtime_wiring: std::sync::Mutex::new(RuntimeWiring::default()),
            skip_predicate: std::sync::OnceLock::new(),
            rt: tokio::runtime::Handle::current(),
            panic_restart_count: std::sync::atomic::AtomicU64::new(0),
            restart_hook: std::sync::Mutex::new(None),
        }
    }

    /// 装配主循环 panic 重启回调（gateway 装配点调用一次）。闭包内容 =
    /// `Arc<AgentLoopServiceAdapter>` 捕获后调 [`Self::supervisor_restart`]。
    /// 未装配 = 监督只留痕不重启。
    pub fn set_restart_hook(&self, hook: Arc<dyn Fn() + Send + Sync>) {
        *self.restart_hook.lock().unwrap() = Some(hook);
    }

    /// 监督任务的唯一行动入口：退避重启 + 熔断计数。
    /// 退避表 [1s, 10s, 60s]；第 4 次+检测到死亡只留痕不再重启。
    /// pub 仅因 gateway 装配的 restart hook 闭包跨模块调用
    /// （监督任务自身经 hook 间接触达，不直接调本方法）。
    pub fn supervisor_restart(&self) {
        let n = self.panic_restart_count.fetch_add(1, Ordering::SeqCst);
        if n >= 3 {
            tracing::error!(
                "[AgentAdapter] agent loop task died unexpectedly again (panic death #{}) — \
                 auto-restart disabled for this process (threshold 3); manual gateway restart required",
                n + 1
            );
            return;
        }
        tracing::error!(
            "[AgentAdapter] agent loop task died unexpectedly (panic death #{} of 3) — restarting",
            n + 1
        );
        // stop 是重启前置的尽力清理（内部已各自留痕），失败不阻断重启。
        let _ = self.stop();
        if let Err(e) = self.start() {
            // 重启装配失败（如 bus/端口状态异常）：错误大声留痕——监督
            // 语义下这是第 4 次死亡等价的终局面，需人工介入。
            tracing::error!("[AgentAdapter] supervisor restart start() failed: {e}");
        }
    }

    /// 装配期设置主桥 skip 谓词（gateway 组装点调用一次；重复设置
    /// 忽略后者——OnceLock 语义）。None 判定的消息照常进主 loop。
    pub fn set_skip_predicate(
        &self,
        pred: Arc<dyn Fn(&nemesis_types::channel::InboundMessage) -> bool + Send + Sync>,
    ) {
        let _ = self.skip_predicate.set(pred);
    }

    /// Get the current AgentLoop (if running). Used by heartbeat and external callers.
    #[allow(dead_code)] // used by the heartbeat handler (heartbeat feature) + external callers.
    pub fn current(&self) -> Option<Arc<nemesis_agent::r#loop::AgentLoop>> {
        self.state.lock().unwrap().agent_loop.clone()
    }

    /// （WASM 插件框架）：当前 loop 持有器句柄（与 AppState 共享同一 Arc，
    /// start/stop 每次刷新）。热装 hook 捕获它而非裸 Weak——agent 重启后
    /// 仍解析到最新 loop；None = 停机窗口（重启路径 register_plugin_tools
    /// 会从注册表重新拉起，热装事件此时丢弃是诚实降级）。
    #[cfg(feature = "plugins-wasm")]
    pub fn loop_ref_handle(
        &self,
    ) -> Arc<parking_lot::RwLock<Option<Arc<nemesis_agent::r#loop::AgentLoop>>>> {
        self.agent_loop_ref.clone()
    }

    /// 装配审批应答端（run_runtime M7 块调用；替代直打 loop 实例的
    /// `set_approval_responder`）。stash 进 [`RuntimeWiring`] + 立即打到
    /// 当前 loop（若在运行）——stop→start 重建路径由 start() 重放，
    /// 重启后 Dashboard 审批卡照常可答。
    pub fn wire_approval_responder(
        &self,
        responder: Arc<dyn nemesis_types::agent::ApprovalResponder>,
    ) {
        self.runtime_wiring.lock().unwrap().approval_responder = Some(responder.clone());
        if let Some(loop_arc) = self.state.lock().unwrap().agent_loop.clone() {
            loop_arc.set_approval_responder(responder);
            // 与 run_runtime 装配块同款（interactive reachability 进
            // Runtime Policy 快照）；非 security 构建无此调用路径。
            loop_arc.set_interactive_approval(true);
        }
    }

    /// 装配 question 应答端 + doom-loop asker（run_runtime F7/J5 块调用；
    /// 同一 WebQuestionBroker Arc 双槽）。stash + 打当前 loop，重启重放。
    pub fn wire_question_broker(
        &self,
        responder: Arc<dyn nemesis_types::agent::QuestionResponder>,
        asker: Arc<dyn nemesis_types::agent::QuestionAsker>,
    ) {
        {
            let mut w = self.runtime_wiring.lock().unwrap();
            w.question_responder = Some(responder.clone());
            w.question_asker = Some(asker.clone());
        }
        if let Some(loop_arc) = self.state.lock().unwrap().agent_loop.clone() {
            loop_arc.set_question_responder(responder);
            loop_arc.set_question_asker(asker);
        }
    }

    /// 把 stash 的运行期接线重放到重建出的 loop（start() 重建分支调用；
    /// 预建分支同样调用无害——同值覆盖）。审批接线在场才置
    /// interactive_approval（对齐装配块 cfg 语义：非 security 构建恒 false）。
    fn replay_runtime_wiring(&self, loop_arc: &Arc<nemesis_agent::r#loop::AgentLoop>) {
        let w = self.runtime_wiring.lock().unwrap();
        if let Some(ref responder) = w.approval_responder {
            loop_arc.set_approval_responder(responder.clone());
            loop_arc.set_interactive_approval(true);
        }
        if let Some(ref responder) = w.question_responder {
            loop_arc.set_question_responder(responder.clone());
        }
        if let Some(ref asker) = w.question_asker {
            loop_arc.set_question_asker(asker.clone());
        }
    }

    /// 主循环任务监督 spawn（BUG-20261123-003 引入，BUG-20261126 重设计）：
    /// 任务体在 free fn `agent_loop_supervisor_task`（单测直驱）。JoinHandle
    /// 所有权移交监督任务 await；代际停机证词 clone 随行。钩子/计数 spawn
    /// 时快照（钩子在 gateway 装配期先于 start 注入）。
    fn spawn_loop_supervisor(&self, handle: tokio::task::JoinHandle<()>, witness: Arc<AtomicBool>) {
        let hook = self.restart_hook.lock().unwrap().clone();
        let death_seq = self.panic_restart_count.load(Ordering::SeqCst);
        self.rt
            .spawn(agent_loop_supervisor_task(handle, witness, hook, death_seq));
    }
}

impl LifecycleService for AgentLoopServiceAdapter {
    fn start(&self) -> Result<(), String> {
        // BUG-20261126：旧设计此处复位共享 expected_stop 旗——正是连环误杀
        // 的第一推手（新代落旗 = 老监督眼里的「无旗尸体」）。重设计下停机
        // 证词一代一面（下方新建），无共享旗可复位。死亡计数**不清**（进程
        // 生命周期累计，熔断语义见 supervisor_restart；gateway 重启才归零）。
        let mut state = self.state.lock().unwrap();

        if state.agent_loop.is_some() && state.bridge_handle.is_some() {
            tracing::info!("[AgentAdapter] start: already started, skipping");
            return Ok(());
        }

        // If we have a pre-built AgentLoop (from new()) but no tasks yet, use it.
        // Otherwise (after a stop), build a fresh one from disk.
        let agent_loop = if let Some(al) = state.agent_loop.take() {
            tracing::info!("[AgentAdapter] start: using pre-built AgentLoop");
            al
        } else {
            tracing::info!("[AgentAdapter] start: building fresh AgentLoop via factory...");
            match crate::agent_factory::build_agent_loop(&self.shared) {
                Ok(al) => al,
                Err(e) => {
                    // No need to roll back any flag — state is unchanged
                    return Err(format!("Failed to build agent loop: {}", e));
                }
            }
        };

        // Update shared reference for WebServer/AppState.
        *self.agent_loop_ref.write() = Some(agent_loop.clone());

        // BUG-20261108-012 横向收尾：重建分支（factory 新 loop）不经过
        // run_runtime 装配块——审批应答端 / question broker 的 loop 级
        // setter 在此重放，重启后 WSAPI approval.respond / question.respond
        // 照常触达（阻塞端 auditor/broker 走共享槽本就不受影响）。
        self.replay_runtime_wiring(&agent_loop);

        // Create a new mpsc channel for inbound messages
        let (agent_inbound_tx, agent_inbound_rx) =
            tokio::sync::mpsc::channel::<nemesis_types::channel::InboundMessage>(1024);

        // Round-5 review fix: give the agent loop a clone of the SENDER so
        // its queue-drain path can re-inject queued heads into this same
        // mpsc — the reply then flows through run_bus_arc's normal
        // post-processing (rpc prefix, sent_in_round, error conversion)
        // instead of being published raw by an inline recursion.
        agent_loop.set_reinject_tx(agent_inbound_tx.clone());

        // (devtool-upgrade)：重启丢失的后台 subagent 任务 —— 注入
        // 诚实丢失回执。后台任务是进程内 tokio 任务，gateway 重启即终止、
        // 无对端可 poll（cluster first_start 对空 peer_id 快照本就跳过）。
        // 快照**保留**：回灌走正常续行路径（gate → dispatch_continuation →
        // handle_cluster_continuation），以 error 结果续行 LLM 让卡住的会话
        // 解锁（LLM 自行向用户说明/重派），完成后 resume 自清快照。消息在
        // loop 起步前 try_send 进有界通道（空载必成功），保序为首批消息。
        for task_id in agent_loop.list_stale_bg_spawn_task_ids() {
            // 续行回灌（Continuation class，msg_class 单轨 2026-11-21）：
            // 构造器烤死 channel="system" + class；sender 保持
            // `subagent_continuation:{task_id}` 前缀形态（闸顶续行臂按前缀
            // 提取 task_id 路由）。status/error/source 章随行（续行 tool
            // 结果组装消费）。
            let msg = nemesis_types::channel::InboundMessage::continuation(
                format!(
                    "{}{}",
                    nemesis_types::constants::SUBAGENT_CONTINUATION_PREFIX,
                    task_id
                ),
                String::new(),
                "[后台子代理任务丢失] 网关在后台任务运行期间重启，任务结果不可用。",
            )
            .with_metadata("status", "error")
            .with_metadata(
                "error",
                "gateway restarted while the background sub-agent task was running; its result is lost",
            )
            .with_metadata("source", "background_subagent");
            if let Err(e) = agent_inbound_tx.try_send(msg) {
                tracing::warn!(
                    "[AgentAdapter] Failed to inject background-subagent loss note: {}",
                    e
                );
            }
        }

        // Bridge: bus inbound broadcast → agent inbound mpsc
        let bus_inbound = self.bus.subscribe_inbound();
        let bus_for_lag = Arc::clone(&self.bus);
        let skip = self.skip_predicate.get().cloned();
        let rt = self.rt.clone();
        let bridge = rt.spawn(async move {
            let mut rx = bus_inbound;
            let mut total_dropped: u64 = 0;
            loop {
                match rx.recv().await {
                    Ok(msg) => {
                        // 项目会话消息不进主 loop（项目调度器转发）。
                        // system/无归属消息照常放行（谓词只对 ToProject 命中）。
                        if let Some(pred) = skip.as_ref()
                            && pred(&msg)
                        {
                            // debug 留痕（丢失必须留痕纪律）：本臂与项目路由
                            // ToMain 丢弃臂各自独立查归属索引，forget_session
                            // （sessions.clear/delete 联动）恰落在两读之间 =
                            // 本臂让出 + 对臂按 ToMain 丢弃（消息双输窗口）。
                            // 两侧 debug 配对即可从日志定性该窗口。
                            tracing::debug!(
                                channel = %msg.channel,
                                session_key = %msg.session_key,
                                "[Main] agent inbound bridge: project-session message skipped (project router owns routing)"
                            );
                            continue;
                        }
                        // BUG-20261113-138：接收侧（agent loop）已退出，
                        // 这条入站消息无处投递——留痕再断桥。旧形态静默
                        // break，窗口期用户消息零痕迹蒸发，违背「丢失必须
                        // 留痕」（同桥 Lagged 臂有 warn + bus 指标）。
                        // SendError 公开回收消息本体，留痕不引入热路径拷贝。
                        if let Err(tokio::sync::mpsc::error::SendError(dead_msg)) =
                            agent_inbound_tx.send(msg).await
                        {
                            tracing::warn!(
                                channel = %dead_msg.channel,
                                sender_id = %dead_msg.sender_id,
                                "[Main] Agent inbound bridge: agent receiver gone — inbound message dropped, breaking bridge"
                            );
                            break; // Agent receiver dropped
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        total_dropped += n;
                        // 接收侧真实丢失计入 bus 指标（发布侧
                        // no_receiver_dropped 不含 lag 丢帧——两者语义不同）。
                        bus_for_lag.record_inbound_lagged(n);
                        tracing::warn!(
                            "[Main] Agent inbound bridge lagged by {} messages (total dropped: {})",
                            n, total_dropped
                        );
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        if total_dropped > 0 {
                            tracing::warn!(
                                "[Main] Agent inbound bridge closing with {} total dropped messages",
                                total_dropped
                            );
                        }
                        break;
                    }
                }
            }
        });

        // Spawn the agent loop。JoinHandle 所有权整份移交监督任务 await
        // （死因裁决真相源）；state 只留 abort 句柄 + 本代停机证词。
        let agent_loop_clone = agent_loop.clone();
        let agent_task = self.rt.spawn(async move {
            agent_loop_clone.run_bus_arc(agent_inbound_rx).await;
        });
        let agent_abort = agent_task.abort_handle();
        let stop_witness = Arc::new(AtomicBool::new(false));

        // Store everything in state (still holding the lock).
        state.agent_loop = Some(agent_loop);
        state.bridge_handle = Some(bridge);
        state.agent_abort = Some(agent_abort);
        state.stop_witness = Some(Arc::clone(&stop_witness));
        drop(state);

        // 监督任务（锁外 spawn：其重启臂经 hook 走 stop/start 会拿同一把
        // state 锁——await 设计下监督只在任务 resolved 后行动，锁内 spawn
        // 无直接死锁但没必要）。
        self.spawn_loop_supervisor(agent_task, stop_witness);

        tracing::info!("[AgentAdapter] start: agent loop started, listening on bus");
        Ok(())
    }

    fn stop(&self) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();

        if state.agent_loop.is_none() {
            tracing::info!("[AgentAdapter] stop: already stopped, skipping");
            return Ok(());
        }

        // 先立本代停机证词（BUG-20261126）：监督任务 resolve 后读自家代际
        // 证词收班。置位必须在 al.stop() 之前——al.stop() 置 running=false
        // 可使任务先于下方 abort 自然收班（Ok 结局），监督届时证词必须
        // 已在场。幂等 stop 同样置位无害（证词已随 take 归还）。
        if let Some(witness) = state.stop_witness.take() {
            witness.store(true, Ordering::SeqCst);
        }

        tracing::info!("[AgentAdapter] stop: shutting down agent...");

        // Set running=false and clear session busy states.
        if let Some(ref al) = state.agent_loop {
            al.stop();
            al.clear_session_busy();
            // 孤儿卡配对清理：loop 停止 = 在途请求终局——
            // 未决审批卡在 abort handle 前定向关闭（等待方收到拒绝裁决而非
            // 随任务 abort 消失；卡不残留到超时窗口）。trait 默认空实现 =
            // 无审批面形态零开销。
            if let Some(responder) = al.approval_responder() {
                let n = responder.resolve_orphans("agent loop stopped");
                if n > 0 {
                    tracing::warn!("[AgentAdapter] stop: closed {n} pending approval card(s)");
                }
            }
        }

        // Abort the inbound bridge. This drops `agent_inbound_tx` (the mpsc
        // sender), which causes `run_bus_arc`'s `inbound_rx.recv()` to return
        // `None`, breaking the loop promptly — even when idle.
        if let Some(handle) = state.bridge_handle.take() {
            handle.abort();
        }

        // Abort the agent loop task（经代际 abort 句柄；任务已收班时
        // no-op）。Known behavior: if the agent was processing a message,
        // the response for that message is lost. This is expected — the
        // user explicitly stopped the agent, so incomplete replies are
        // acceptable.
        if let Some(abort) = state.agent_abort.take() {
            abort.abort();
        }

        // Drop the old AgentLoop entirely.
        state.agent_loop.take();
        *self.agent_loop_ref.write() = None;

        tracing::info!("[AgentAdapter] stop: agent loop stopped and destroyed");
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.state.lock().unwrap().bridge_handle.is_some()
    }
}

impl AgentLoopServiceTrait for AgentLoopServiceAdapter {
    fn cancel_session(&self, session_key: &str) -> bool {
        let state = self.state.lock().unwrap();
        if let Some(ref al) = state.agent_loop {
            al.cancel_session(session_key)
        } else {
            false
        }
    }

    fn cancel_all_sessions(&self) -> usize {
        let state = self.state.lock().unwrap();
        if let Some(ref al) = state.agent_loop {
            al.cancel_all_sessions()
        } else {
            0
        }
    }
}

/// 主循环监督任务体（BUG-20261123-003 引入，BUG-20261126 重设计；独立
/// free fn 供单测直驱——构造整个 adapter 太重）。**await 任务句柄拿真实
/// 死因**（tokio JoinHandle resolved 即报结局，事件驱动零轮询）：
/// - `Err(e) if e.is_cancelled()`：我们的 abort（stop 流程）→ 安静退役；
/// - `Ok(())` 且本代停机证词置位：al.stop() 先置 running=false、任务先于
///   abort 自然收班的正常形态 → 安静退役；
/// - `Err(is_panic)` 或无证词的 `Ok(())`：真异常死亡 → 按 `death_seq`
///   退避（1s/10s/60s）→ 复核证词（窗口内来了显式 stop 尊重）→ 触发
///   restart 回调（None = 只 error! 留痕）。
///
/// 行动后本任务即退；**句柄所有权在本任务手里，await resolve 即监督自然
/// 退休**——旧设计「每代永生监督盯上一代尸体」（共享 expected_stop 旗被
/// start 复位 + 5s 轮询猜死活）的连环误杀链结构性不可能复发：监督只对
/// 自家代际句柄/证词裁决，stop 不再产生「无旗尸体」。
async fn agent_loop_supervisor_task(
    handle: tokio::task::JoinHandle<()>,
    stop_witness: Arc<AtomicBool>,
    hook: Option<Arc<dyn Fn() + Send + Sync>>,
    death_seq: u64,
) {
    match handle.await {
        // 我们的 abort（stop 流程打断在途轮次）= 正常换班，安静退役。
        Err(e) if e.is_cancelled() => {}
        // 自然收班 + 本代证词在场 = al.stop() 的正常形态，安静退役。
        Ok(()) if stop_witness.load(Ordering::SeqCst) => {}
        // 真 panic（unwind）或无证词的自发退出 = 异常死亡。
        outcome => {
            let reason = match &outcome {
                Ok(()) => "exited without a stop witness (unexpected graceful exit)",
                Err(e) if e.is_panic() => "panicked",
                Err(_) => "failed to join (unknown join error)",
            };
            match hook {
                Some(restart) => {
                    // 退避：panic 风暴不无限快重启（1s/10s/60s，第 4 次+
                    // 由 supervisor_restart 熔断只留痕）。
                    let backoff = match death_seq {
                        0 => std::time::Duration::from_secs(1),
                        1 => std::time::Duration::from_secs(10),
                        _ => std::time::Duration::from_secs(60),
                    };
                    tracing::error!(
                        "[AgentAdapter] agent loop task {reason} — \
                         supervisor restart in {}s (prior deaths: {death_seq})",
                        backoff.as_secs()
                    );
                    tokio::time::sleep(backoff).await;
                    // 退避窗口内来了显式 stop（证词置位）→ 尊重，不重启。
                    if stop_witness.load(Ordering::SeqCst) {
                        return;
                    }
                    restart();
                }
                None => {
                    tracing::error!(
                        "[AgentAdapter] agent loop task {reason}; \
                         no restart hook wired — manual restart required"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------
// WebServerOps adapter
// ---------------------------------------------------------------------------

use nemesis_channels::web::WebServerOps;

/// Adapter that bridges `nemesis_web::SessionManager` to the `WebServerOps` trait
/// used by `WebChannel` for outbound message delivery.
pub struct WebServerOpsAdapter {
    session_manager: Arc<nemesis_web::session::SessionManager>,
    rt: tokio::runtime::Handle,
}

impl WebServerOpsAdapter {
    pub fn new(session_manager: Arc<nemesis_web::session::SessionManager>) -> Self {
        Self {
            session_manager,
            rt: tokio::runtime::Handle::current(),
        }
    }
}

impl WebServerOps for WebServerOpsAdapter {
    fn send_to_session(
        &self,
        session_id: &str,
        role: &str,
        content: &str,
        model: Option<&str>,
        session_key: Option<&str>,
        source_node: Option<&str>,
        note_kind: Option<&str>,
        occupies: Option<bool>,
    ) -> std::result::Result<(), String> {
        let sm = self.session_manager.clone();
        let sid = session_id.to_string();
        let content = content.to_string();
        let model = model.map(|s| s.to_string());
        let session_key = session_key.map(|s| s.to_string());
        let source_node = source_node.map(|s| s.to_string());
        let note_kind = note_kind.map(|s| s.to_string());
        let result = tokio::task::block_in_place(|| {
            self.rt.block_on(nemesis_web::server::send_to_session(
                &sm,
                &sid,
                role,
                &content,
                model.as_deref(),
                session_key.as_deref(),
                source_node.as_deref(),
                note_kind.as_deref(),
                occupies,
            ))
        });
        // （真机日志）：assistant 回复推送失败（会话无活动
        // 连接——典型如崩溃恢复回复回到已断开的驱动端）时打未读标记；
        // 内容已照常落盘（chat_log），标记只承载「有未读」通知事实，
        // 前端拉历史后经 sessions.mark_delivered 清零。
        if let (Err(_), Some(key)) = (&result, session_key.as_deref())
            && role == "assistant"
        {
            nemesis_agent::chat_log::mark_undelivered_reply(key);
        }
        result
    }

    fn send_history_to_session(
        &self,
        session_id: &str,
        content: &str,
    ) -> std::result::Result<(), String> {
        let sm = self.session_manager.clone();
        let sid = session_id.to_string();
        let content = content.to_string();
        tokio::task::block_in_place(|| {
            self.rt
                .block_on(nemesis_web::server::send_history_to_session(
                    &sm, &sid, &content,
                ))
        })
    }

    fn broadcast(&self, content: &str) -> std::result::Result<(), String> {
        let msg = nemesis_web::protocol::ProtocolMessage::new(
            "message",
            "chat",
            "receive",
            Some(serde_json::json!({
                "role": "assistant",
                "content": content,
            })),
        );
        let data = serde_json::to_vec(&msg).map_err(|e| format!("marshal: {}", e))?;
        let sm = self.session_manager.clone();
        let mut failures: Vec<String> = Vec::new();
        for sid in self.active_session_ids() {
            let data_clone = data.clone();
            // BUG-20261113-139：与 server.rs 广播调用点同款慢客户端隔离——
            // 单个挂起会话（hi 满，可等待任意久）不得卡死整段全广播串行
            // 循环，更不得卡死调用宿主（manager 出站泵）。超时值与
            // server.rs 共用 WS_BROADCAST_TIMEOUT 单点。
            // BUG-20261125-047：超时只解决了「永久卡死」，没解决「一坏全
            // 断」——旧形态首个失败会话上的 `?` 直接中断扇出，其后所有
            // 健康会话连尝试都没有。本路径帧不经 chat_event_log 落环
            // （无 seq，chat.sync 无从补拉），中断 = 健康会话永久丢帧。
            // 改为逐会话隔离：失败留痕继续（超时臂 warn——SessionManager
            // 内部只在无队列臂留痕，挂起超时无人看见），收尾聚合报错。
            // Err 契约不变（调用方仍能看到部分失败），全部成功才 Ok。
            let inner = tokio::task::block_in_place(|| {
                self.rt.block_on(tokio::time::timeout(
                    nemesis_web::server::WS_BROADCAST_TIMEOUT,
                    sm.broadcast(&sid, &data_clone),
                ))
            });
            match inner {
                Err(elapsed) => {
                    tracing::warn!(
                        session_id = %sid,
                        "[WebServerOps] broadcast to {sid} timed out ({elapsed}) (slow/dead client isolation) — continuing to remaining sessions"
                    );
                    failures.push(format!("broadcast to {sid}: timed out ({elapsed})"));
                }
                Ok(Err(e)) => failures.push(format!("broadcast to {sid}: {e}")),
                Ok(Ok(())) => {}
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "broadcast partial failure ({} failed): {}",
                failures.len(),
                failures.join("; ")
            ))
        }
    }

    fn active_session_ids(&self) -> Vec<String> {
        self.session_manager
            .all_sessions()
            .into_iter()
            .map(|s| s.id)
            .collect()
    }

    fn start_server(&self) -> std::result::Result<(), String> {
        Ok(())
    }

    fn stop_server(&self) {}
}

// ---------------------------------------------------------------------------
// DeviceService adapter（Layer C）
// ---------------------------------------------------------------------------

/// Adapter wrapping `nemesis_devices::service:DeviceService` to implement the
/// `nemesis_services::DeviceService` trait.
///
/// 长寿命持有链：init_services 构造 + 接线 → `BotService.inject_devices`
/// 持有本 adapter → BotService.start_services/stop 统一管生命周期。此前
/// device_service 是装配块局部变量，块尾 drop 连带杀掉 UsbEventSource
/// （udevadm 子进程），监控装配后瞬间静默死亡。
#[cfg(feature = "devices")]
pub struct DeviceServiceAdapter {
    inner: Arc<nemesis_devices::service::DeviceService>,
    started: AtomicBool,
}

#[cfg(feature = "devices")]
impl DeviceServiceAdapter {
    pub fn new(inner: Arc<nemesis_devices::service::DeviceService>) -> Self {
        Self {
            inner,
            started: AtomicBool::new(false),
        }
    }
}

#[cfg(feature = "devices")]
impl LifecycleService for DeviceServiceAdapter {
    fn start(&self) -> Result<(), String> {
        if self.started.swap(true, Ordering::SeqCst) {
            return Ok(()); // Already started
        }
        let inner = self.inner.clone();
        tokio::spawn(async move {
            if let Err(e) = inner.start().await {
                tracing::error!("[Main] Device service error: {}", e);
            }
        });
        Ok(())
    }

    fn stop(&self) -> Result<(), String> {
        self.inner.stop();
        self.started.store(false, Ordering::SeqCst);
        Ok(())
    }
}

#[cfg(feature = "devices")]
impl nemesis_services::DeviceService for DeviceServiceAdapter {}

/// 设备通知 / 心跳服务的 last channel 供给器：从 AgentLoopServiceAdapter
/// 实时取 agent loop 持有的 WorkspaceStateManager（Layer B）。
///
/// 不缓存：state manager 独立实例内存态只在 new 时读盘一次，agent 每轮
/// turn 写入的是 loop 自己那份实例——必须经 getter 共享同一实例实时读。
/// struct 本体在两消费方 feature 任一开启时编译（devices=设备通知路由，
/// heartbeat=心跳 tick 的渠道解析，BUG-20261121-027）；两个 trait impl
/// 各自挂本方 feature 门，读路径单一真相源（inherent helper）。
#[cfg(any(feature = "devices", feature = "heartbeat"))]
pub struct AgentLastChannelProvider {
    adapter: Arc<AgentLoopServiceAdapter>,
}

#[cfg(any(feature = "devices", feature = "heartbeat"))]
impl AgentLastChannelProvider {
    pub fn new(adapter: Arc<AgentLoopServiceAdapter>) -> Self {
        Self { adapter }
    }

    fn last_channel_from_loop(&self) -> String {
        self.adapter
            .current()
            .and_then(|al| al.state_manager())
            .map(|m| m.get_last_channel())
            .unwrap_or_default()
    }
}

#[cfg(feature = "devices")]
impl nemesis_devices::service::LastChannelProvider for AgentLastChannelProvider {
    fn get_last_channel(&self) -> String {
        self.last_channel_from_loop()
    }
}

/// heartbeat 心跳的 StateManager 同体供给（BUG-20261121-027）：gateway
/// 装配此前零接线——tick 永远拿 None → ("","") → handler fallback
/// cli:direct → 心跳 spawn 的子代理 origin channel/chat_id 一并继承
/// cli:direct → 子任务结果 outbound 到不存在的 "cli" 通道，被
/// ChannelManager::dispatch_outbound 丢弃（dropped_not_found）。HEARTBEAT.md
/// 模板钦定 spawn 是异步回传主路径，此断裂 = 心跳异步子任务结果永远
/// 到不了用户。与 devices 通知的 Layer B 接线同构。
#[cfg(feature = "heartbeat")]
impl nemesis_heartbeat::service::StateManager for AgentLastChannelProvider {
    fn get_last_channel(&self) -> String {
        self.last_channel_from_loop()
    }
}

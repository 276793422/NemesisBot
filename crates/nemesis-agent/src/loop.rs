//! Agent loop: core execution engine that drives LLM + tool interactions.
//!
//! The loop processes messages through these stages:
//!
//! 1. Build context from conversation history
//! 2. Call the LLM provider
//! 3. If the response contains tool calls, execute them and append results
//! 4. Repeat until a plain text response is produced or `max_turns` is reached
//!
//! # Bus-integrated mode
//!
//! The `AgentLoop` can be used in two ways:
//!
//! - **Standalone mode**: Direct calls via `run()`, `process_direct()`, etc.
//! - **Bus-integrated mode**: Continuous consumption from a message bus via
//!   `run_bus_arc()`.
//!
//! In bus-integrated mode, the loop connects to an `mpsc` inbound/outbound
//! channel pair and handles the full Go `AgentLoop` lifecycle including
//! system message routing, history requests, cluster continuation, slash
//! commands, session busy management, summarization, and startup info.
// ===========================================================================
// P1 物理拆分（docs/PLAN/2026-09-23_agentloop-god-object-decomposition.md §3.2）：
// 单文件 loop.rs 拆为 17 个子模块 + 本根文件（struct AgentLoop/散项/测试挂载/re-export）。
// 语义零变化：子模块经 `use super::*` 取根 re-export，根经 glob re-export 保持
// `crate::r#loop::X` 既有路径逐字节不变；字段零改动（struct 留根，impl 分居后代模块）。
// ===========================================================================
//
// 锁纪律（§原则 5 / P2-6）：本模块（含全部子模块）显式固化「guard 不跨
// await」为编译器检查——lock()/read()/write() 的 guard 持有时跨 .await 即告警。
#![warn(clippy::await_holding_lock)]

pub(crate) mod prelude {
    //! 子模块共享导入面（P1 兼容垫片；P1-c 可选溶解为各文件显式导入）。
    #![allow(unused_imports)]
    pub(crate) use std::collections::HashMap;
    pub(crate) use std::sync::Arc;
    pub(crate) use std::sync::atomic::{AtomicBool, Ordering};

    pub(crate) use async_trait::async_trait;
    pub(crate) use serde::{Deserialize, Serialize};
    pub(crate) use tracing::{debug, error, info, warn};

    pub(crate) use crate::context::RequestContext;
    pub(crate) use crate::hooks::HookToolCall;
    pub(crate) use crate::instance::AgentInstance;
    pub(crate) use crate::registry::AgentRegistry;
    pub(crate) use crate::session::{SessionStore, estimate_tokens_for_turns_projected};
    pub(crate) use crate::types::{AgentConfig, AgentEvent, ToolCallInfo, ToolCallResult};
    pub(crate) use nemesis_routing::{
        AgentDef, RouteConfig, RouteInput as RoutingRouteInput, RouteResolver,
    };
}
pub(crate) use prelude::*;

mod admission;
#[allow(unused_imports)]
pub use admission::*;
#[allow(unused_imports)]
pub(crate) use admission::*;
mod bus;
#[allow(unused_imports)]
pub use bus::*;
#[allow(unused_imports)]
pub(crate) use bus::*;
mod commands;
#[allow(unused_imports)]
pub use commands::*;
#[allow(unused_imports)]
pub(crate) use commands::*;
mod compact;
#[allow(unused_imports)]
pub use compact::*;
#[allow(unused_imports)]
pub(crate) use compact::*;
mod config_watch;
#[allow(unused_imports)]
pub use config_watch::*;
#[allow(unused_imports)]
pub(crate) use config_watch::*;
mod fs_watch;
#[allow(unused_imports)]
pub use fs_watch::*;
#[allow(unused_imports)]
pub(crate) use fs_watch::*;
mod llm_types;
#[allow(unused_imports)]
pub use llm_types::*;
#[allow(unused_imports)]
pub(crate) use llm_types::*;
mod maintenance;
#[allow(unused_imports)]
pub use maintenance::*;
#[allow(unused_imports)]
pub(crate) use maintenance::*;
mod messages;
#[allow(unused_imports)]
pub use messages::*;
#[allow(unused_imports)]
pub(crate) use messages::*;
mod observer;
#[allow(unused_imports)]
pub use observer::*;
#[allow(unused_imports)]
pub(crate) use observer::*;
mod rate_limit;
#[allow(unused_imports)]
pub use rate_limit::*;
#[allow(unused_imports)]
pub(crate) use rate_limit::*;
mod recovery;
#[allow(unused_imports)]
pub use recovery::*;
#[allow(unused_imports)]
pub(crate) use recovery::*;
mod rewind;
#[allow(unused_imports)]
pub use rewind::*;
#[allow(unused_imports)]
pub(crate) use rewind::*;
mod run_loop;
#[allow(unused_imports)]
pub use run_loop::*;
#[allow(unused_imports)]
pub(crate) use run_loop::*;
mod tool_batch;
#[allow(unused_imports)]
pub use tool_batch::*;
#[allow(unused_imports)]
pub(crate) use tool_batch::*;
mod tool_defs;
#[allow(unused_imports)]
pub use tool_defs::*;
#[allow(unused_imports)]
pub(crate) use tool_defs::*;
mod tool_dispatch;
#[allow(unused_imports)]
pub use tool_dispatch::*;
#[allow(unused_imports)]
pub(crate) use tool_dispatch::*;
mod tools_trait;
#[allow(unused_imports)]
pub use tools_trait::*;
#[allow(unused_imports)]
pub(crate) use tools_trait::*;
mod turn;
#[allow(unused_imports)]
pub use turn::*;
#[allow(unused_imports)]
pub(crate) use turn::*;
mod wiring;
#[allow(unused_imports)]
pub use wiring::*;
#[allow(unused_imports)]
pub(crate) use wiring::*;

// ---------------------------------------------------------------------------
// Internal channel detection
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// ProcessOptions -- options for how a message is processed
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Per-session busy state with queue length
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// MessageTool sent-in-round tracking (mirrors Go's MessageTool.HasSentInRound)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// AgentLoop -- core execution engine
// ---------------------------------------------------------------------------

/// 钩子三槽收拢（P3-1，§5.1 HooksState）：K1a/K1b/K2 三代钩子管理器原
/// AgentLoop 三字段逐字迁入，锁类型不变；访问经 `self.hooks.<field>`。
/// setter（`add_tool_hook`/`add_llm_hook`/`add_lifecycle_hook`）留在
/// `AgentLoop` 上一行委托，外部 API 零变化。
pub(crate) struct HooksState {
    /// K1a (U14): user tool hooks — pre runs after the fixed security gate,
    /// post runs after execute and before Forge. RwLock so hooks can be
    /// registered from `&self` post-construction (K2 hooks.json wiring).
    /// See `crate::hooks` module doc for the full 布点图.
    pub(crate) tool_hooks: parking_lot::RwLock<crate::hooks::ToolHookManager>,
    /// K1b (U14): LLM-call-level hooks — pre may append reminder messages
    /// (visible in request_log), post may allow/replace/retry/block the
    /// response. See `crate::hooks` module doc（LLM 调用级布点）.
    pub(crate) llm_hooks: parking_lot::RwLock<crate::hooks::LlmHookManager>,
    /// K2 (U14): prompt/turn lifecycle hooks — on_user_prompt runs in
    /// `run_with_trace` BEFORE the message enters history (blocked prompts
    /// are never seen by the model), on_turn_end runs after the final
    /// answer is accepted, before the turn ends. Primary consumer: the
    /// hooks.json dialect bridge (`crate::cc_hooks`).
    pub(crate) lifecycle_hooks: parking_lot::RwLock<crate::hooks::LifecycleHookManager>,
}

pub struct AgentLoop {
    // --- Standalone fields (always present) ---
    /// LLM provider for generating responses.
    /// Wrapped in `RwLock<Arc<...>>` for runtime provider swapping (model switch).
    /// Spawned tasks clone the Arc (cheap), so in-flight requests finish with the
    /// old provider while new requests use the updated one.
    provider: parking_lot::RwLock<Arc<dyn LlmProvider>>,
    /// Active model name, kept in sync with the provider above.
    /// Separated from `config.model` so runtime swaps don't need `&mut self`.
    active_model: parking_lot::RwLock<String>,
    /// Tool registry: name -> tool implementation.
    /// Each tool is wrapped in `Arc` so the map can be cloned and shared
    /// with spawned tasks without requiring `Box` clone support.
    /// Wrapped in `RwLock` for interior mutability — MCP hot-reload needs
    /// to register new tools from `&self` methods (inside the run loop).
    tools: parking_lot::RwLock<HashMap<String, Arc<dyn Tool>>>,
    /// Agent configuration.
    config: AgentConfig,

    // --- Bus-integrated fields (optional) ---
    /// Outbound message sender for bus mode.
    outbound_tx: Option<tokio::sync::mpsc::Sender<nemesis_types::channel::OutboundMessage>>,
    /// Agent registry for multi-agent routing.
    registry: Option<Arc<AgentRegistry>>,
    /// State manager for recording last channel/chat ID (persistent on disk).
    state_manager: Option<Arc<nemesis_state::workspace_state::WorkspaceStateManager>>,
    /// Session store for persistent history.
    session_store: Option<Arc<SessionStore>>,
    /// A1（2026-09-22 聊天切会话竞态）：环尾 seq 查询回调（nemesis-web 的
    /// `chat_event_log::latest_seq` 经 gateway 装配注入——crate 方向
    /// agent←web 不可直调，走依赖注入，同 session_store 模式）。
    /// `handle_history_request` 在读取历史**之后**采样，随响应下发
    /// `last_seq`；`None`（standalone / 未注入）→ 响应不带 last_seq，
    /// 前端走尾部同文兜底（A2）。
    chat_seq_lookup: parking_lot::RwLock<Option<std::sync::Arc<dyn Fn(&str) -> u64 + Send + Sync>>>,
    /// Running flag for the bus consumption loop.
    running: AtomicBool,
    /// Per-session busy state with queue length tracking.
    session_busy: parking_lot::Mutex<HashMap<String, SessionBusyState>>,
    /// BUG 2026-09-21 ①：单会话限流重试实时快照（写侧重试环、读侧
    /// `retry_status`/WSAPI `agent.retry_status`）。turn 收尾即清；abort
    /// 残留由读侧过期兜底（[`RETRY_STATUS_STALE_SECS`]）。
    rate_limit_status: parking_lot::Mutex<HashMap<String, RateLimitStatus>>,
    /// Session busy check: mode-aware I1 (U7).
    /// Concurrent request handling mode.
    concurrent_mode: ConcurrentMode,
    /// Re-injection sender into the agent's own inbound mpsc (round-5 review
    /// fix). The queue-drain path uses it to hand the queued head back to the
    /// normal `run_bus_*` consumer instead of recursing inline — so the reply
    /// gets the SAME post-processing as any other message (rpc correlation
    /// prefix, sent_in_round check+clear, error→"Error processing message"
    /// conversion + capture flush, meta.model). Deliberately NOT the bus
    /// broadcast: re-publishing on the bus would re-match workflow
    /// message-triggers (double firing). `None` in standalone mode (no drain
    /// consumer exists) and until the adapter wires it.
    reinject_tx: parking_lot::RwLock<
        Option<tokio::sync::mpsc::Sender<nemesis_types::channel::InboundMessage>>,
    >,
    /// Configured queue size for queue mode. Stored for config/logging parity
    /// but NOT read: busy-queueing lives in `crate::inbox` (capacity-bounded
    /// FIFO per session), not in the session_busy map (see
    /// `try_start_session`'s comment for why the old counter path was
    /// removed). Remove this field (+ the `new_bus` param + call sites) if
    /// queue mode is permanently retired.
    #[allow(dead_code)]
    queue_size: usize,
    /// Maximum concurrent cluster continuation tasks.
    /// 0 = inline execution in the main loop (no spawn, serialized).
    /// >0 = spawn with semaphore-controlled concurrency.
    max_continuation_permits: usize,
    /// Semaphore for limiting concurrent continuation spawns.
    /// `None` when `max_continuation_permits == 0` (inline mode).
    continuation_semaphore: Option<Arc<tokio::sync::Semaphore>>,
    /// D (2026-09-23 多会话并行清账) D-4：loop 级并发 turn 上限（安全阀）。
    /// `None`（`max_concurrent_turns == 0`，构造缺省）不设限——独立/测试
    /// 路径行为不变；>0 时每个 spawned turn 任务在任务体内先取许可再执行
    /// （泵不被阻塞，超限任务在信号量上 FIFO 排队：诚实等待，不丢失不
    /// 拒绝）。abort 时许可随 future 丢弃释放，无泄漏；许可不嵌套
    /// （turn 内的重注入回泵，不直接 spawn），无死锁面。
    turn_permits: Option<Arc<tokio::sync::Semaphore>>,
    max_concurrent_turns: usize,
    /// Tracks which sessions are currently being summarized.
    /// Wrapped in `Arc` so the flag can be cleared from a spawned task
    /// after summarization completes (mirrors Go's `defer al.summarizing.Delete()`).
    summarizing: Arc<parking_lot::Mutex<HashMap<String, bool>>>,
    /// ⑩ Per-session compaction state for graded tiers (soft/summarize) and
    /// stuck self-check. See `maybe_summarize`.
    compact_state: Arc<parking_lot::Mutex<HashMap<String, CompactState>>>,
    /// Channel manager reference (for channel listing commands).
    channel_manager_channels: parking_lot::Mutex<Vec<String>>,
    /// Tracks whether a message tool already sent a response this round.
    /// Mirrors Go's MessageTool.HasSentInRound() / alreadySent check.
    sent_in_round: SentInRoundTracker,
    /// Route resolver for multi-agent message routing.
    /// Mirrors Go's al.registry (RouteResolver). When set, process_inbound_message
    /// uses the full 7-level priority cascade instead of the default-agent fallback.
    route_resolver: Option<RouteResolver>,
    /// Optional observer event callback (mirrors Go's observerMgr).
    /// Called at conversation_start, conversation_end, llm_request, llm_response, tool_call.
    observer_callback: Option<Arc<dyn Fn(&str, &serde_json::Value) + Send + Sync>>,
    /// Continuation manager for cluster RPC async callbacks.
    continuation_manager: Option<Arc<crate::loop_continuation::ContinuationManager>>,
    /// Cluster reference for cross-node communication.
    /// Stored as `Arc<dyn Any + Send + Sync>` to avoid a circular dependency
    /// on the `nemesis-cluster` crate. The caller can downcast to the concrete
    /// cluster type. Mirrors Go's `AgentLoop.cluster`.
    cluster: Option<Arc<dyn std::any::Any + Send + Sync>>,
    /// Observer manager for Phase 5 event emission.
    /// Mirrors Go's `AgentLoop.observerMgr`.
    observer_manager: Option<Arc<nemesis_observer::Manager>>,
    /// Security plugin for pre-execution tool safety checks.
    /// Mirrors Go's SecurityPlugin registered via PluginManager.
    #[cfg(feature = "security")]
    security_plugin: Option<Arc<nemesis_security::pipeline::SecurityPlugin>>,
    #[cfg(not(feature = "security"))]
    #[allow(dead_code)]
    security_plugin: Option<()>,
    /// MCP Manager for dynamic tool discovery and hot-reload.
    mcp_manager: Option<std::sync::Mutex<nemesis_mcp::manager::McpManager>>,
    /// Snapshot of registered MCP tool names and descriptions.
    /// Shared with McpListTool so it can list MCP tools without accessing the full tool registry.
    mcp_tool_snapshot: Arc<parking_lot::RwLock<Vec<(String, String)>>>,
    /// Optional data store for recording LLM usage statistics.
    data_store: Option<Arc<nemesis_data::DataStore>>,
    /// Forge instance for experience collection during tool execution.
    #[cfg(feature = "forge")]
    forge: Option<Arc<nemesis_forge::forge::Forge>>,
    #[cfg(not(feature = "forge"))]
    #[allow(dead_code)] // placeholder when forge feature is off
    forge: Option<()>,
    /// Per-session cancellation tokens. When a user requests cancellation,
    /// the token for the corresponding session is cancelled, causing the
    /// LLM loop to break at the next check point.
    cancel_tokens: dashmap::DashMap<String, tokio_util::sync::CancellationToken>,
    /// Checkpoint store for the edit safety net. When attached, every writer
    /// tool call snapshots the file's pre-edit content before execution, so a
    /// rewind can restore it. RwLock so it can be attached from `&self` (the
    /// gateway sets it after construction).
    checkpoint_store: parking_lot::RwLock<Option<Arc<crate::checkpoint::CheckpointStore>>>,
    /// Monotonic turn counter for checkpoints (one per inbound message). Global
    /// across sessions in this MVP — adequate for single-session deployments;
    /// multi-session isolation is a documented follow-up.
    turn_counter: std::sync::atomic::AtomicUsize,
    /// D3（devtool-upgrade 阶段 5）：本 turn 声明式文件工具的变更流水，按
    /// session_key 分桶。dispatch 瀑布（`preview_all` 处）顺带收集（独立于
    /// checkpoint 是否挂载）；`run_agent_loop_internal` 在 assistant 最终
    /// 回复落盘时 drain+去重，写进 chat_log jsonl 行的 `file_changes` 字段
    /// （消息↔文件变更映射；M3 会话级 diff 查看器的数据源）。Arc 外壳：
    /// dispatch 瀑布闭包是 `Fn`（'static），须经 Arc 捕获共享。清理时点 =
    /// drain（写后即清）+ turn 开始兜底清（上轮异常短路未走到落盘也不残留
    /// 到本轮）；steer 注入不重入 `run_agent_loop_internal`，正在跑的 turn
    /// 缓冲不受影响。
    turn_file_changes: Arc<parking_lot::Mutex<HashMap<String, Vec<FileChange>>>>,
    /// E3（devtool-upgrade 阶段 5）：消息级回退的 undo 栈，按 session_key
    /// 分栈（会话间互不干扰，栈内严格 LIFO）。`rewind_to_message` 压栈，
    /// `redo_rewind` 弹栈反向。内存态（重启即失——redo 只对本次进程内的
    /// undo 有效，诚实边界）；每栈上限 [`REWIND_UNDO_STACK_CAP`]，超限丢
    /// 最旧。
    rewind_undo_stacks:
        parking_lot::Mutex<HashMap<String, std::collections::VecDeque<RewindUndoEntry>>>,
    /// K1a/K1b/K2 钩子三槽（P3-1 收拢 [`HooksState`]；字段语义见该类型）。
    hooks: HooksState,
    /// Memory tool executor reference, so the gateway can attach an approval
    /// gate post-construction (memory_store/forget require interactive approval).
    #[cfg(feature = "memory")]
    memory_executor:
        parking_lot::RwLock<Option<Arc<nemesis_memory::memory_tools::MemoryToolExecutor>>>,
    #[cfg(not(feature = "memory"))]
    #[allow(dead_code)] // placeholder when memory feature is off
    memory_executor: parking_lot::RwLock<Option<()>>,
    /// P3.1 (sixth batch): memory manager for the AUTO-INJECT channel —
    /// read-only retrieval (top-K vector search over the current user
    /// message) feeding the `# Memory Context` snapshot section. Deliberately
    /// SEPARATE from `memory_executor`: that one gates store/forget behind
    /// interactive approval; auto-inject is pure retrieval and must not trip
    /// the approval gate. `None` (default) disables injection entirely.
    #[cfg(feature = "memory")]
    memory_inject_manager: parking_lot::RwLock<Option<Arc<nemesis_memory::manager::MemoryManager>>>,
    #[cfg(not(feature = "memory"))]
    #[allow(dead_code)] // placeholder when memory feature is off
    memory_inject_manager: parking_lot::RwLock<Option<()>>,
    /// P3.1: auto_inject flag + top_k loaded from config.enhanced_memory.json
    /// by the factory (`set_memory_inject`). Tuple so both values travel
    /// together on the one setter. Default (false, 3) = feature off.
    memory_inject_cfg: parking_lot::RwLock<(bool, usize)>,
    /// Capability tier (small-model-tool-robustness plan, Phase 4a). Resolved at
    /// construction from the active model's `model_tier` config (see
    /// [`nemesis_types::capability`]). Drives tool-set size (Phase 3),
    /// validation-retry budget (Phase 2), and format-repair gating (Phase 5).
    /// `RwLock` so it can be re-resolved if the active model switches at runtime.
    tier: parking_lot::RwLock<nemesis_types::capability::ModelTier>,
    /// F1（devtool-upgrade 阶段 4）：plan/build 工作模式。Build=默认全量；
    /// Plan=只读白名单供给（build_tool_defs 第三过滤层）+ 分发端写类拦截
    /// （`handle_tool_call_at_depth` 入口，防 MCP/未知写工具与陈旧 defs
    /// 漏网）+ plans/ 写放行。运行时态不持久化（重启回 Build）；
    /// `/plan` `/build` 与 WSAPI `chat.set_mode` 双入口改同一份状态。
    mode: parking_lot::RwLock<crate::types::AgentMode>,
    /// F1：M1a 事件广播发送端——模式切换后发布 `ModeChanged`（前端徽标
    /// 实时刷新；chat_id 缺省空 = 只进 SSE EventHub，不路由具体会话）。
    /// `None`（standalone / 未注入）→ 发布静默跳过（观察者通道空转是常态）。
    agent_event_tx: parking_lot::RwLock<
        Option<tokio::sync::broadcast::Sender<nemesis_types::agent::AgentEvent>>,
    >,
    /// Path to config.json — the single source of truth for per-model
    /// `model_tier`. `None` in standalone mode (no config.json to watch). Set
    /// by `agent_factory`; used by `refresh_active_tier` / `check_config_reload`
    /// so dashboard-added models and CLI `model set-tier` are picked up live,
    /// with no stale snapshot.
    config_path: parking_lot::RwLock<Option<std::path::PathBuf>>,
    /// N1（devtool-upgrade 阶段 1）：分层价目表（workspace/data）。三级
    /// context_window 解析链的第 L2 级——config 未显式配置 `context_window`
    /// 时按价目表 `max_input_tokens` 猜。`None`（未注入/打开失败）→ 直接落到
    /// L3 fallback（[`FALLBACK_CONTEXT_WINDOW`]）。
    pricing_store: parking_lot::RwLock<Option<std::sync::Arc<nemesis_data::PricingStore>>>,
    /// C3（devtool-upgrade 阶段 2）：共享 LspManager 单例（与 LspTool 同一
    /// 实例，见 SharedResources.lsp_manager / C5）。编辑后诊断回灌（修复
    /// 闭环）用它同步文档 + 等诊断。`None`（未注入 / standalone）→ 反馈
    /// 静默跳过。Set via `set_lsp_manager` by the agent factory.
    lsp_manager: parking_lot::RwLock<Option<Arc<nemesis_lsp::LspManager>>>,
    /// 自定义 slash 命令表路径（`config.commands.json`；主 agent 专用，集群
    /// agent 不接——命令不该跨节点复制，同 hooks 挂账决策）。
    /// 自定义命令表热重载器（HotReloader 统一收编，2026-08-29：原
    /// path/mtime/cache 三字段手写 mtime 模式收编为一行声明）。
    commands_hot:
        parking_lot::RwLock<Option<nemesis_config::HotReloader<nemesis_config::CommandsConfig>>>,
    /// hooks 方言桥（2026-08-29 T3）：PreCompact/PostCompact 触发用。
    /// SessionEnd 经 SessionEndHookManager（factory 清理点直接调桥）。
    cc_bridge: parking_lot::RwLock<Option<std::sync::Arc<crate::cc_hooks::CcHookBridge>>>,
    /// G4 (U4): root directory for tool-result spill files
    /// (`<home>/logs/spill`). `None` disables spilling (results fall back to
    /// the G3 prune tier). Set via `set_spill_root` by the agent factory.
    spill_root: parking_lot::RwLock<Option<std::path::PathBuf>>,
    /// H3 (P2.2): skills loader for the catalog digest. `None` disables the
    /// digest injection entirely. Set via `set_skills_loader`.
    skills_loader: parking_lot::RwLock<Option<Arc<nemesis_skills::loader::SkillsLoader>>>,
    /// H3 (P2.2): digest emission handle. Round-5: stateless under I2
    /// merged-snapshot semantics (sections re-render from disk every build;
    /// the old per-session hash map gated nothing and was removed — see
    /// skills_digest.rs module doc).
    skills_digest_state: std::sync::Arc<crate::skills_digest::DigestState>,
    /// I1 (U7): per-session message inbox for Queue/Steer modes. Unused in
    /// Reject mode (kept anyway — trivial cost, simplifies mode switching).
    inbox: crate::inbox::SharedInbox,
    /// V5 (2026-08-23): abort handles of the Queue/Steer pump's spawned turn
    /// tasks. The pump itself is aborted by the adapter on stop; without
    /// tracking, those spawned turns would be orphaned and keep publishing
    /// replies after the "stop". `stop()` aborts them (mirrors the serial
    /// pump where aborting the one task killed the in-flight turn). Reject
    /// mode never spawns — stays empty.
    turn_task_handles: parking_lot::Mutex<Vec<tokio::task::AbortHandle>>,
    /// H5 (U18): workspace root for the AGENTS.md/CLAUDE.md instruction
    /// chain. `None` disables the instructions section of the merged
    /// context digest.
    workspace_root: parking_lot::RwLock<Option<std::path::PathBuf>>,
    /// I1 (devtool-upgrade 阶段 3): fs-watcher keep-alive handle. `None` =
    /// not started / disabled / failed (warn-once). Callbacks hold a
    /// Weak<AgentLoop> (handle lives INSIDE the loop — Arc would cycle).
    fs_watcher: parking_lot::RwLock<Option<crate::fs_watcher::WatcherHandle>>,
    /// I1: externally-changed workspace files observed since the last
    /// build_messages drain (capped; one-shot — drained on next build).
    external_changes: parking_lot::Mutex<Vec<String>>,
    /// I1: agent-authored writes (write_file/edit_file) for the self-write
    /// window — watcher events for these paths within
    /// [`crate::fs_watcher::SELF_WRITE_WINDOW`] are dropped (the agent knows
    /// what it just wrote). Normalized workspace-relative lowercase.
    // I1: pub(crate) so sibling test modules (fs_watcher/tests.rs) can age
    // entries past the self-write window without sleeping.
    pub(crate) recent_self_writes:
        parking_lot::Mutex<std::collections::HashMap<String, std::time::Instant>>,
    /// Full-review M4: context-snapshot message role ("user" default;
    /// "system" restores the pre-I2 shape for strict chat templates that
    /// reject adjacent user/user pairs).
    snapshot_role: parking_lot::RwLock<String>,
    /// X2 (U8 refinement): whether interactive approval (desktop popup
    /// adapter wired to the auditor by the gateway) is reachable. Rendered
    /// into the merged context snapshot's `# Runtime Policy` section. The
    /// guardian line reads the security plugin's live judge; the tier line
    /// reads the live capability tier — all three are state (no clocks), so
    /// the section renders deterministically: same state ⇒ same bytes.
    interactive_approval: parking_lot::RwLock<bool>,
    /// I5（devtool-upgrade 阶段 7）：当前轮客户端上报的「当前打开文件」路径
    /// （WSAPI `chat.send` data.open_files → metadata["open_files"] → bus 全链
    /// 路透传，解析走 nemesis-types::channel::open_files_from_metadata 单点）。
    /// Ephemeral per-turn：`process_admitted` 进轮前 set、出轮后 clear（词法
    /// 配对防泄漏——cron/heartbeat/continuation 等非 process_admitted 路径
    /// 天然读不到，无陈旧泄漏）。build_messages 渲染进 merged digest section
    /// （空 = 无 section，字节稳定）；digest 内容随既有 InjectionRecord 台账
    /// 落账 → 字节级回放免费一致。只存路径不读内容（annotation 语义）。
    pending_open_files: parking_lot::RwLock<Vec<String>>,
    /// 对话生成（2026-09-22）：当前轮 workflow_edit 注入目标（WSAPI
    /// `chat.send` data.workflow_edit → metadata["workflow_edit"]，解析走
    /// nemesis-types::channel::workflow_edit_from_metadata 单点）。与
    /// pending_open_files 同款 per-turn ephemeral 生命周期：process_admitted
    /// set、出轮 clear。Some → build_messages 渲染能力表/当前定义 section。
    /// 引擎引用只服务「已注册工作流」的当前定义读取；no-workflow 编译下
    /// 整个字段消失。
    pending_workflow_edit: parking_lot::RwLock<Option<nemesis_types::channel::WorkflowEditTarget>>,
    /// 对话生成：工作流引擎引用（渲染 workflow_edit 命名会话的当前定义块 +
    /// agent_factory 装配；None = 引擎未装配，命名会话诚实注明）。
    #[cfg(feature = "workflow")]
    workflow_engine:
        parking_lot::RwLock<Option<std::sync::Arc<nemesis_workflow::engine::WorkflowEngine>>>,
    /// Y1 (Phase4-a): per-tool description embedding cache (tool name →
    /// (description bytes, vector)) for semantic doc folding. Entries
    /// re-embed only when a tool's description text changes, so after the
    /// first round folding adds no embed calls beyond the query itself.
    /// Read only on the memory-feature path (the embed backend lives in
    /// nemesis-memory); `allow(dead_code)` keeps the no-default-features
    /// build warning-clean.
    #[cfg_attr(not(feature = "memory"), allow(dead_code))]
    tool_vec_cache: parking_lot::RwLock<std::collections::HashMap<String, (String, Vec<f32>)>>,
    /// Last-seen mtime of config.json; `check_config_reload` compares against
    /// this each round to detect on-disk changes without re-reading every turn.
    config_mtime: parking_lot::RwLock<Option<std::time::SystemTime>>,
    /// 全局急停状态（kill switch）。触发后，循环在每轮顶部 break、并在工具
    /// 分发前拒绝调用。`None`（standalone/测试）时永不阻塞 = 零行为变化。
    /// 以 `Option<Arc<...>>` 形态持有，工厂每次重建 loop 时从
    /// `SharedResources.estop` 重新绑定到**同一个** Arc——所以急停状态在
    /// agent 重启后自动保持。
    estop: parking_lot::RwLock<Option<Arc<crate::estop::EstopState>>>,
    /// N2 (devtool-upgrade 阶段 4)：小模型专职杂务通道（`agents.small_model`）。
    /// 手动 compact（E6 `/compact`）的摘要调用优先走它（省 token——摘要不需
    /// 要旗舰档智力）；未配置 = `None`，诚实回退主模型。自动压缩（质量敏感）
    /// 刻意不消费本字段，维持主模型。工厂在 loop 构造后从 config 解析装配；
    /// 运行期改配置需重启 Agent（与 lsp_tool 等启动期装配项同一约定）。
    small_model: parking_lot::RwLock<Option<SmallModelSlot>>,
    /// M7 (devtool-upgrade 阶段 5)：审批响应端（`ApprovalResponder`）。
    /// gateway 装配 WebApprovalManager 后挂在这里，WSAPI `approval.respond` /
    /// `approval.pending` 经 AppState 的 agent_loop 槽触达（不经 security 依赖）。
    /// 未装配 = `None`，approval handler 诚实报「未装配」。
    approval_responder:
        parking_lot::RwLock<Option<Arc<dyn nemesis_types::agent::ApprovalResponder>>>,
    /// F7 (devtool-upgrade 阶段 5)：结构化提问响应端（`QuestionResponder`）。
    /// gateway 装配 WebQuestionBroker 后挂在这里，WSAPI `question.respond` /
    /// `question.pending` 经 AppState 的 agent_loop 槽触达（同审批先例）。
    /// 未装配 = `None`，question handler 诚实报「未装配」。
    question_responder:
        parking_lot::RwLock<Option<Arc<dyn nemesis_types::agent::QuestionResponder>>>,
    /// J5 (devtool-upgrade 阶段 6)：doom-loop 审批卡的提问发起端
    /// （`QuestionAsker`）——gateway 注入与 F7 responder 同源的
    /// WebQuestionBroker Arc 的另一半 trait。escalation 触发且
    /// `agents.doom_loop_approval` 开时经此发卡问用户「继续吗？」。
    /// 未装配 = `None` = 审批通路缺失，escalation 直接走现行为（停轮）。
    question_asker: parking_lot::RwLock<Option<Arc<dyn nemesis_types::agent::QuestionAsker>>>,
}

impl AgentLoop {
    /// Create a new agent loop with the given provider and configuration (standalone mode).
    pub fn new(provider: Box<dyn LlmProvider>, config: AgentConfig) -> Self {
        let model = config.model.clone();
        info!("[AgentLoop] Created in standalone mode, model={}", model);
        Self {
            provider: parking_lot::RwLock::new(Arc::from(provider)),
            active_model: parking_lot::RwLock::new(config.model.clone()),
            tools: parking_lot::RwLock::new(HashMap::new()),
            config,
            outbound_tx: None,
            registry: None,
            state_manager: None,
            session_store: None,
            chat_seq_lookup: parking_lot::RwLock::new(None),
            running: AtomicBool::new(false),
            session_busy: parking_lot::Mutex::new(HashMap::new()),
            rate_limit_status: parking_lot::Mutex::new(HashMap::new()),
            concurrent_mode: ConcurrentMode::Reject,
            reinject_tx: parking_lot::RwLock::new(None),
            queue_size: crate::inbox::DEFAULT_QUEUE_SIZE,
            max_continuation_permits: 0,
            continuation_semaphore: None,
            turn_permits: None,
            max_concurrent_turns: 0,
            summarizing: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            compact_state: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            channel_manager_channels: parking_lot::Mutex::new(Vec::new()),
            sent_in_round: SentInRoundTracker::new(),
            route_resolver: None,
            observer_callback: None,
            continuation_manager: None,
            cluster: None,
            observer_manager: None,
            security_plugin: None,
            mcp_manager: None,
            mcp_tool_snapshot: Arc::new(parking_lot::RwLock::new(Vec::new())),
            data_store: None,
            forge: None,
            cancel_tokens: dashmap::DashMap::new(),
            checkpoint_store: parking_lot::RwLock::new(None),
            turn_counter: std::sync::atomic::AtomicUsize::new(0),
            turn_file_changes: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            rewind_undo_stacks: parking_lot::Mutex::new(HashMap::new()),
            hooks: HooksState {
                tool_hooks: parking_lot::RwLock::new(crate::hooks::ToolHookManager::new()),
                llm_hooks: parking_lot::RwLock::new(crate::hooks::LlmHookManager::new()),
                lifecycle_hooks: parking_lot::RwLock::new(crate::hooks::LifecycleHookManager::new()),
            },
            memory_executor: parking_lot::RwLock::new(None),
            #[cfg(feature = "memory")]
            memory_inject_manager: parking_lot::RwLock::new(None),
            #[cfg(not(feature = "memory"))]
            memory_inject_manager: parking_lot::RwLock::new(None),
            memory_inject_cfg: parking_lot::RwLock::new((false, 3)),
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
            inbox: std::sync::Arc::new(crate::inbox::Inbox::new(crate::inbox::DEFAULT_QUEUE_SIZE)),
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
            tool_vec_cache: parking_lot::RwLock::new(std::collections::HashMap::new()),
            config_mtime: parking_lot::RwLock::new(None),
            estop: parking_lot::RwLock::new(None),
            small_model: parking_lot::RwLock::new(None),
            approval_responder: parking_lot::RwLock::new(None),
            question_responder: parking_lot::RwLock::new(None),
            question_asker: parking_lot::RwLock::new(None),
        }
    }

    /// R1（2026-09-21）：中间轮正文发布（[`nemesis_types::agent::AgentEvent::RoundText`]）。
    /// web pump 的默认路径即覆盖全部投递（注入 session_id + record_tool 入环 +
    /// tool_event 通道 WS push），本方法只负责发；agent_event_tx 未装配
    /// （CLI / B 端 worker）为 no-op。
    fn emit_round_text(&self, session_key: &str, chat_id: &str, content: &str) {
        if let Some(tx) = self.agent_event_tx.read().as_ref() {
            // 无订阅者（CLI / 无人在线）= 观察者通道空转，静默忽略。
            let _ = tx.send(nemesis_types::agent::AgentEvent::RoundText {
                session_key: session_key.to_string(),
                chat_id: chat_id.to_string(),
                content: content.to_string(),
            });
        }
    }

    /// SB（2026-09-17）：会话物化事件——`session_key` 的 jsonl 首行落盘后
    /// 发布 SessionCreated（web pump → SSE `session.created` + WS push），
    /// 前端 force 刷新会话列表（修「首条消息隐式创建的会话不进侧栏，须
    /// 手动刷新」）。调用点必须在 append **前**取 exists、append 后发布；
    /// agent_event_tx 未装配（B 端 worker 等）为 no-op。
    fn emit_session_created(&self, session_key: &str) {
        if let Some(ref tx) = *self.agent_event_tx.read() {
            let session_id = session_key
                .rsplit(':')
                .next()
                .unwrap_or(session_key)
                .to_string();
            let _ = tx.send(nemesis_types::agent::AgentEvent::SessionCreated {
                session_id,
                session_key: session_key.to_string(),
            });
        }
    }

    /// SB：user 行落盘的标准前置——返回落盘前 jsonl 是否已存在（调用方在
    /// append 后据 false 发布 SessionCreated）。
    fn session_log_exists_before_append(session_key: &str) -> bool {
        crate::chat_log::chat_log_exists(session_key)
    }

    /// Process an inbound message through routing and slash command handling (standalone).
    ///
    /// Returns (agent_id, response_content, handled).
    pub fn process_message(
        &self,
        content: &str,
        context: &RequestContext,
    ) -> (String, String, bool) {
        // Check for cluster continuation prefix.
        if context.channel == "system"
            && content.starts_with(nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX)
        {
            debug!(
                "[AgentLoop] Cluster continuation message intercepted: {}",
                content
            );
            return (String::new(), String::new(), true);
        }

        // Check for slash commands.
        if let Some(response) = self.handle_command(content) {
            return (String::new(), response, true);
        }

        (String::new(), String::new(), false)
    }

    /// Returns a reference to the tool registry.
    pub fn tools(&self) -> parking_lot::RwLockReadGuard<'_, HashMap<String, Arc<dyn Tool>>> {
        self.tools.read()
    }

    /// Returns a reference to the agent config.
    pub fn config(&self) -> &AgentConfig {
        &self.config
    }

    /// Mark that a message was sent for the given session in the current round.
    /// Used by tools like MessageTool to prevent double-sending. Mirrors Go's
    /// MessageTool.sentInRound.
    pub fn mark_sent_in_round(&self, session_key: &str) {
        self.sent_in_round.mark_sent(session_key);
    }

    /// Check if a message was already sent in the current round for a session.
    pub fn has_sent_in_round(&self, session_key: &str) -> bool {
        self.sent_in_round.has_sent_in_round(session_key)
    }
}

// ---------------------------------------------------------------------------
// Standalone summarization helpers (usable from spawned tasks)
// ---------------------------------------------------------------------------

/// Adjust a summarize boundary so the verbatim tail `history[new_c..]` doesn't
/// START in the middle of a tool_call/result pair.
///
/// `summarize_prefix_owned` only folds user/assistant `content` into the
/// summary — tool_calls and tool results are not summarized. If the boundary
/// landed between an assistant tool_call (covered by the summary only as text
/// content, which is often empty for a pure tool-call turn) and its tool
/// result, `repair_tool_message_pairs` would drop the orphan result from the
/// tail and the whole interaction would vanish from the LLM's view. Backing
/// `new_c` up past any leading tool messages moves the parent assistant (and
/// sibling results) into the verbatim tail, keeping the pair intact. (The old
/// `truncate_with_tool_pairs` did the equivalent by prepending the parent.)
///
/// The summary still covers `history[..returned_new_c]` and the tail is
/// `history[returned_new_c..]` — the gap-free invariant holds; the tail just
/// grows slightly past `K_TARGET` when a pair straddles the boundary.
fn tool_safe_boundary(history: &[crate::types::ConversationTurn], mut new_c: usize) -> usize {
    while new_c > 0 && new_c < history.len() && history[new_c].role == "tool" {
        new_c -= 1;
    }
    new_c
}

/// Summarize a contiguous prefix of the conversation, merging any existing
/// summary.
///
/// Summarizes **all** of `messages` (no internal "keep last N" step) — the
/// caller has already chosen the verbatim tail boundary (`K_TARGET`), so every
/// message passed in is meant to be folded into the summary. Keeping a "last N"
/// here would leave a gap between the summary and the verbatim tail. Reuses the
/// multipart/batch machinery; merges `existing_summary` (which covers messages
/// before this prefix) into the result.
///
/// G1 (U1) prefix-reuse: the summary request is built as
/// `[system, ...original covered messages..., instruction]` — the same leading
/// messages the main loop sends (byte-equal per message), so the provider's
/// warm KV prefix from the last routed request is REUSED rather than
/// invalidated ("genuine prefix" principle). The old
/// form (single bare user message with `role: content` text concatenation)
/// shared no prefix with real requests and destroyed structure (tool_calls
/// flattened to text).
///
/// Returns `Some(summary)` if a non-empty summary was produced, `None`
/// otherwise. **None 的两种含义都不允许调用方推进 covers_up_to**：
/// (a) 前缀里没有可摘要的 user/assistant 内容；(b) 摘要 LLM 调用失败
/// （2026-08-25 摘要静默失败修复：此前 batch 失败返回空字符串，multipart
/// 拿两段空串去 merge，merge 模型回"你没把摘要贴给我"，这段**失败回复**
/// 被当摘要存进 store、covers 照常推进——被折叠的上下文静默丢失。现在
/// 失败一路传播为 None，调用方保持原 history 不折叠并 warn。）
///
/// T4 (U1) per-model switch: `prefix_reuse == false` falls back to the
/// pre-G1 shape (`summarize_bare_concat_owned`) — per-model config
/// `summarizer_prefix_reuse: false`, for cheap summarizer models that break
/// the assumed warm KV prefix. Default (true) keeps the prefix-reuse shape.
async fn summarize_prefix_owned(
    messages: &[&crate::types::ConversationTurn],
    existing_summary: &str,
    context_window: usize,
    prefix_reuse: bool,
    provider: &dyn LlmProvider,
    model: &str,
    observer_manager: Option<Arc<nemesis_observer::Manager>>,
) -> Option<String> {
    // Oversized message guard.
    let max_msg_tokens = context_window / 2;
    let mut valid_messages: Vec<&crate::types::ConversationTurn> = Vec::new();
    let mut omitted = false;

    for m in messages {
        if m.role != "user" && m.role != "assistant" {
            continue;
        }
        let msg_tokens = crate::session::estimate_tokens(&m.content);
        if msg_tokens > max_msg_tokens {
            omitted = true;
            continue;
        }
        valid_messages.push(m);
    }

    if valid_messages.is_empty() {
        return None;
    }

    let final_summary = if !prefix_reuse {
        // T4 (U1): old shape — single bare user message, no structure.
        summarize_bare_concat_owned(
            &valid_messages,
            existing_summary,
            provider,
            model,
            observer_manager,
        )
        .await
    } else {
        // G1: the system prompt anchoring the prefix. `messages` is
        // history[..new_c]; history[0] is the system turn — include it verbatim
        // (WITHOUT the summary block the main loop appends: that would leak the
        // old summary into the prefix and change it between rounds).
        let system_msg: Option<LlmMessage> = messages
            .first()
            .filter(|m| m.role == "system")
            .map(|m| conversation_turn_to_llm_message(m));

        if valid_messages.len() > 10 {
            summarize_multipart_owned(
                system_msg.as_ref(),
                &valid_messages,
                existing_summary,
                provider,
                model,
                observer_manager,
            )
            .await
        } else {
            summarize_batch_owned(
                system_msg.as_ref(),
                &valid_messages,
                existing_summary,
                provider,
                model,
                observer_manager,
            )
            .await
        }
    };

    let final_summary = match final_summary {
        Some(s) if omitted && !s.is_empty() => Some(format!(
            "{}\n[Note: Some oversized messages were omitted from this summary for efficiency.]",
            s
        )),
        other => other,
    };

    final_summary.filter(|s| !s.is_empty())
}

/// G1 (U1): build an LLM wire message from a ConversationTurn preserving the
/// original structure (role/content/tool_calls/tool_call_id/reasoning) — the
/// same projection `build_messages`'s `turn_to_msg` performs. Shared by the
/// summarizers so the summary request's prefix messages are byte-equal to the
/// main loop's.
///
/// T6（多模态）：摘要请求**不发图片字节**（摘要模型可能是不支持视觉的廉价
/// 档，且图片字节会破坏 G1 前缀复用的字节等价前提）——历史 turn 的
/// `image_refs` 在此不水合，`images` 恒空。失效图片的占位文本也只出现在
/// 主循环请求（`turn_to_request_message`）里，摘要请求保持纯文本。
fn conversation_turn_to_llm_message(turn: &crate::types::ConversationTurn) -> LlmMessage {
    LlmMessage {
        role: turn.role.clone(),
        content: turn.content.clone(),
        tool_calls: if turn.tool_calls.is_empty() {
            None
        } else {
            Some(turn.tool_calls.clone())
        },
        tool_call_id: turn.tool_call_id.clone(),
        reasoning_content: turn.reasoning_content.clone(),
        images: Vec::new(),
    }
}

/// T5/T6（多模态）：主循环请求的 history turn → 请求消息转换（`build_messages`
/// 与 replay 重建共用，水合语义单点）。路径引用**每轮重读**：验真通过 →
/// base64 字节进 `LlmMessage.images`；文件已删/失效 → 占位文本行
/// `[图片已失效: <path>]` 追加进 content（诚实降级，不静默、不炸）。
pub(crate) fn turn_to_request_message(turn: &crate::types::ConversationTurn) -> LlmMessage {
    let mut msg = conversation_turn_to_llm_message(turn);
    if turn.image_refs.is_empty() {
        return msg;
    }
    let (images, placeholders) = crate::image_attach::hydrate_image_refs(&turn.image_refs);
    if !placeholders.is_empty() {
        msg.content.push('\n');
        msg.content.push_str(&placeholders.join("\n"));
    }
    msg.images = images;
    msg
}

/// T10（多模态 D4）：vision=no 模型的 image_refs 确定性投影（纯函数，生产
/// build 与 replay 重建共用）。带引用的 turn 一律清空 `image_refs` 并把占位
/// 文本行追加进 content：
/// - 最新 user 轮（本轮要发的图）→ `[图片未发送: 当前模型 vision=no（不支持
///   图像输入），图片已忽略]`（拒绝注明，文本照常处理，与 T5 失败路径同构）；
/// - 其余（历史）轮 → `[图片已省略: 当前模型仅支持文本]`（模型中途切换不
///   砖会话）。
///
/// 同输入同输出（无时钟/无 IO）：历史前缀字节稳定保 prompt cache；占位写入
/// 的是**投影视图**——历史持久化的 image_refs 不动，切回 vision 模型图片
/// 原样回来。`last_user_idx` 用与 build 注入点判定完全相同的表达式。
pub(crate) fn project_turns_for_no_vision(
    turns: &mut [crate::types::ConversationTurn],
    last_user_idx: Option<usize>,
) {
    for (i, turn) in turns.iter_mut().enumerate() {
        if turn.image_refs.is_empty() {
            continue;
        }
        turn.image_refs.clear();
        let note = if Some(i) == last_user_idx {
            "[图片未发送: 当前模型 vision=no（不支持图像输入），图片已忽略]"
        } else {
            "[图片已省略: 当前模型仅支持文本]"
        };
        if turn.content.trim().is_empty() {
            turn.content.push_str(note);
        } else {
            turn.content.push('\n');
            turn.content.push_str(note);
        }
    }
}

/// T10（多模态 D4 ③）：provider 4xx 兜底——请求带图且本轮调用最终失败时，
/// 把逃生通道（probe 实测 / vision=no）附加到**用户可见**错误文案。措辞条件
/// 化（"若该错误与图像输入有关"），对无关失败（网络超时等）不误导；历史、
/// request_log 与 observer 事件里的原始错误保持干净（不带提示）。
fn append_vision_fallback_hint(err_text: String, request_had_images: bool) -> String {
    if !request_had_images {
        return err_text;
    }
    format!(
        "{}\n[提示: 本次请求携带了图片。若该错误与图像输入有关（如模型不支持视觉），\
         可运行 `model probe <模型名>` 实测视觉能力，或在 config.json 对应模型条目\
         设置 \"vision\": \"no\" 后重试。]",
        err_text
    )
}

/// The trailing instruction for a G1 prefix-reuse summary request. Kept in one
/// place so the batch and multipart paths emit the identical instruction.
const SUMMARIZE_INSTRUCTION: &str =
    "请对以上对话片段做一份简明摘要，保留核心上下文与关键要点，供后续对话作为前情提要使用。";

/// T4 (U1): pre-G1 summary shape, restored as the per-model fallback
/// (`summarizer_prefix_reuse: false`).
///
/// This is the OLD request form the G1 refactor replaced: a single bare user
/// message whose content is the covered messages flattened as
/// `role: content` text lines, plus the instruction (and any existing-summary
/// context). It shares NO prefix with real requests and destroys structure
/// (tool_calls flatten to text) — which is exactly why it is NOT the default.
/// It remains useful for cheap summarizer models whose warm-KV-prefix
/// assumption G1 relies on does not hold (different tokenizer, no prompt
/// caching): a shape-neutral single message is the lowest-common-denominator
/// request those models handle reliably. The G1 prefix-reuse path
/// (summarize_multipart_owned / summarize_batch_owned) stays the default for
/// the main model; this function is invoked ONLY when the per-model switch
/// opts out.
/// Returns `Some(summary)` on a non-empty LLM reply; `None` on LLM failure or
/// empty reply (failure must propagate — never fold history behind a failed
/// summary; see the 2026-08-25 fix note on `summarize_prefix_owned`).
async fn summarize_bare_concat_owned(
    messages: &[&crate::types::ConversationTurn],
    existing_summary: &str,
    provider: &dyn LlmProvider,
    model: &str,
    observer_manager: Option<Arc<nemesis_observer::Manager>>,
) -> Option<String> {
    let mut content = String::new();
    if !existing_summary.is_empty() {
        content.push_str(&format!(
            "Existing context (summary of the earlier conversation, merge with the new summary): {}\n\n",
            existing_summary
        ));
    }
    for m in messages {
        content.push_str(&format!("{}: {}\n", m.role, m.content));
    }
    content.push_str(SUMMARIZE_INSTRUCTION);

    let llm_messages = vec![LlmMessage {
        role: "user".to_string(),
        content,
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
        images: Vec::new(),
    }];

    let response = emit_observer_events_around_llm(
        observer_manager.as_ref(),
        "summarize-bare-concat",
        model,
        provider.chat(model, llm_messages, None, vec![]),
    )
    .await;

    match response {
        Some(Ok(resp)) if !resp.content.is_empty() => Some(resp.content),
        Some(Ok(_)) => None,
        Some(Err(e)) => {
            warn!(
                "[AgentLoop] summarize_bare_concat_owned LLM call failed (summary NOT produced, history stays unfolded): {}",
                e
            );
            None
        }
        None => None,
    }
}

/// Multi-part summarization (standalone, works in spawned task).
///
/// G1 (U1): each part is summarized as `[system?, ...part messages,
/// instruction]` — a part is a contiguous slice of the covered prefix, so its
/// message list is a true ordered prefix subset of the main request's history
/// (prefix-cache friendly in the same way).
///
/// Returns `Some` only when BOTH parts produced real summaries. If either
/// part's LLM call fails → `None` (whole summarization fails; the caller must
/// keep the history unfolded — a half-empty merge prompt makes the merge model
/// answer with a "you didn't paste the summaries" complaint, and storing that
/// complaint as the summary silently amnesiates the covered prefix; this is
/// the exact production failure found in the legacy session 2026-08-25).
async fn summarize_multipart_owned(
    system_msg: Option<&LlmMessage>,
    messages: &[&crate::types::ConversationTurn],
    existing_summary: &str,
    provider: &dyn LlmProvider,
    model: &str,
    observer_manager: Option<Arc<nemesis_observer::Manager>>,
) -> Option<String> {
    let mid = messages.len() / 2;
    let part1 = &messages[..mid];
    let part2 = &messages[mid..];

    let s1 = summarize_batch_owned(
        system_msg,
        part1,
        existing_summary,
        provider,
        model,
        observer_manager.clone(),
    )
    .await;
    let s2 = summarize_batch_owned(
        system_msg,
        part2,
        "",
        provider,
        model,
        observer_manager.clone(),
    )
    .await;

    let (s1, s2) = match (s1, s2) {
        (Some(a), Some(b)) => (a, b),
        (failed, _) => {
            warn!(
                "[AgentLoop] multipart summarization aborted: one part failed to summarize ({}); summary NOT produced, history stays unfolded",
                if failed.is_none() { "part 1" } else { "part 2" }
            );
            return None;
        }
    };

    // Merge via LLM.
    let merge_prompt = format!(
        "Merge these two conversation summaries into one cohesive summary:\n\n1: {}\n\n2: {}",
        s1, s2
    );

    let llm_messages = vec![LlmMessage {
        role: "user".to_string(),
        content: merge_prompt,
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
        images: Vec::new(),
    }];

    let response = emit_observer_events_around_llm(
        observer_manager.as_ref(),
        "summarize-multipart-merge",
        model,
        provider.chat(model, llm_messages, None, vec![]),
    )
    .await;

    match response {
        Some(Ok(resp)) if !resp.content.is_empty() => Some(resp.content),
        Some(Ok(_)) => {
            // Empty merge reply: both parts are validated non-empty —
            // concatenate them instead of losing the coverage.
            Some(format!("{}\n\n{}", s1, s2))
        }
        _ => {
            // Merge call failed: same fallback — both parts hold real
            // summaries, so concatenation preserves the information (the
            // next compaction round will re-fold them into a merged one).
            warn!(
                "[AgentLoop] multipart merge LLM call failed; falling back to concatenating the two (validated) part summaries"
            );
            Some(format!("{}\n\n{}", s1, s2))
        }
    }
}

/// Single-batch summarization (standalone, works in spawned task).
///
/// G1 (U1): the request is `[system?, ...covered messages (original
/// structure), instruction]` — a genuine prefix of the conversation plus the
/// trailing instruction, replacing the old single bare user message with
/// `role: content` text concatenation.
///
/// Returns `Some` on a non-empty LLM reply; `None` on LLM failure or empty
/// reply (failure propagates — see the 2026-08-25 fix note on
/// `summarize_prefix_owned`).
async fn summarize_batch_owned(
    system_msg: Option<&LlmMessage>,
    batch: &[&crate::types::ConversationTurn],
    existing_summary: &str,
    provider: &dyn LlmProvider,
    model: &str,
    observer_manager: Option<Arc<nemesis_observer::Manager>>,
) -> Option<String> {
    let mut messages: Vec<LlmMessage> = Vec::with_capacity(batch.len() + 2);
    if let Some(sys) = system_msg {
        messages.push(sys.clone());
    }
    for m in batch {
        messages.push(conversation_turn_to_llm_message(m));
    }
    // Trailing instruction (merged with any existing-summary context so the
    // fold still carries prior coverage).
    let mut instruction = String::new();
    if !existing_summary.is_empty() {
        instruction.push_str(&format!(
            "Existing context (summary of the earlier conversation, merge with the new summary): {}\n\n",
            existing_summary
        ));
    }
    instruction.push_str(SUMMARIZE_INSTRUCTION);
    messages.push(LlmMessage {
        role: "user".to_string(),
        content: instruction,
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
        images: Vec::new(),
    });

    let response = emit_observer_events_around_llm(
        observer_manager.as_ref(),
        "summarize-batch",
        model,
        provider.chat(model, messages, None, vec![]),
    )
    .await;

    match response {
        Some(Ok(resp)) if !resp.content.is_empty() => Some(resp.content),
        Some(Ok(_)) => None,
        Some(Err(e)) => {
            warn!(
                "[AgentLoop] summarize_batch_owned LLM call failed (summary NOT produced, history stays unfolded): {}",
                e
            );
            None
        }
        None => None,
    }
}

/// Emit observer events (ConversationStart, LlmRequest, LlmResponse, ConversationEnd)
/// around a synchronous LLM call closure. Used by standalone summarization functions.
async fn emit_observer_events_around_llm<Fut>(
    observer_manager: Option<&Arc<nemesis_observer::Manager>>,
    label: &str,
    model: &str,
    llm_call: Fut,
) -> Option<Result<LlmResponse, String>>
where
    Fut: std::future::Future<Output = Result<LlmResponse, String>>,
{
    use crate::loop_executor::ObserverEvent;

    let trace_id = format!(
        "{}-{}",
        label,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );

    // Emit ConversationStart + LlmRequest before the call (await, no block_in_place).
    if let Some(mgr) = observer_manager {
        let start_event = ObserverEvent::ConversationStart {
            trace_id: trace_id.clone(),
            session_key: label.to_string(),
            channel: String::new(),
            chat_id: String::new(),
            sender_id: "summarizer".to_string(),
            content: String::new(),
        };
        mgr.emit(start_event.to_conversation_event()).await;

        let request_event = ObserverEvent::LlmRequest {
            trace_id: trace_id.clone(),
            round: 0,
            model: model.to_string(),
            messages: vec![],
            tools: vec![],
            messages_count: 0,
            tools_count: 0,
            provider_name: String::new(),
            api_key: String::new(),
            api_base: String::new(),
        };
        mgr.emit(request_event.to_conversation_event()).await;
    }

    // Execute the LLM call (async, no block_on).
    let start = std::time::Instant::now();
    let mut response = llm_call.await;
    let duration_ms = start.elapsed().as_millis() as u64;

    let (response_content, raw_req, raw_resp) = match &mut response {
        Ok(r) => {
            let content = r.content.clone();
            let req = r.raw_request_body.take();
            let resp = r.raw_response_body.take();
            (content, req, resp)
        }
        Err(_) => (String::new(), None, None),
    };

    // Emit LlmResponse + ConversationEnd after the call (await, sequential —
    // LlmResponse fully processed before ConversationEnd, as the old emit_sync intended).
    if let Some(mgr) = observer_manager {
        let response_event = ObserverEvent::LlmResponse {
            trace_id: trace_id.clone(),
            round: 0,
            duration_ms,
            has_tool_calls: false,
            content: response_content.clone(),
            tool_calls: vec![],
            tool_calls_count: 0,
            finish_reason: Some("stop".to_string()),
            usage: None,
            raw_request_body: raw_req,
            raw_response_body: raw_resp,
        };
        mgr.emit(response_event.to_conversation_event()).await;

        let end_event = ObserverEvent::ConversationEnd {
            trace_id,
            session_key: label.to_string(),
            total_rounds: 1,
            duration_ms,
            content: response_content,
            channel: String::new(),
            chat_id: String::new(),
        };
        mgr.emit(end_event.to_conversation_event()).await;
    }

    Some(response)
}

// ---------------------------------------------------------------------------
// Cluster integration helpers
// ---------------------------------------------------------------------------

/// Extract the task ID from a cluster continuation sender ID.
///
/// The format is `cluster_continuation:{taskID}`.
#[cfg(test)]
pub fn extract_continuation_task_id(sender_id: &str) -> Option<&str> {
    sender_id.strip_prefix(nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX)
}

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

/// Build an agent-scoped main session key.
///
/// Format: `agent:{agent_id}:main`
pub fn build_agent_main_session_key(agent_id: &str) -> String {
    format!("agent:{}:main", agent_id)
}

// ---------------------------------------------------------------------------
// Message formatting utilities
// ---------------------------------------------------------------------------

/// Format messages for log output, truncating long content.
///
/// Returns a human-readable multi-line representation of the message list
/// suitable for debug logging.
#[cfg(test)]
pub fn format_messages_for_log(messages: &[LlmMessage]) -> String {
    if messages.is_empty() {
        return "[]".to_string();
    }

    let mut result = String::from("[\n");
    for (i, msg) in messages.iter().enumerate() {
        result.push_str(&format!("  [{}] Role: {}\n", i, msg.role));

        if let Some(ref tool_calls) = msg.tool_calls {
            result.push_str("  ToolCalls:\n");
            for tc in tool_calls {
                let args_preview = truncate(&tc.arguments, 200);
                result.push_str(&format!("    - ID: {}, Name: {}\n", tc.id, tc.name));
                result.push_str(&format!("      Arguments: {}\n", args_preview));
            }
        }

        if !msg.content.is_empty() {
            let content_preview = truncate(&msg.content, 200);
            result.push_str(&format!("  Content: {}\n", content_preview));
        }

        if let Some(ref tcid) = msg.tool_call_id {
            result.push_str(&format!("  ToolCallID: {}\n", tcid));
        }

        result.push('\n');
    }
    result.push(']');
    result
}

/// Format tools for log output.
#[cfg(test)]
pub fn format_tools_for_log(tools: &[ToolCallInfo]) -> String {
    if tools.is_empty() {
        return "[]".to_string();
    }
    let mut result = String::from("[\n");
    for tc in tools {
        let args_preview = truncate(&tc.arguments, 200);
        result.push_str(&format!(
            "  - ID: {}, Name: {}, Args: {}\n",
            tc.id, tc.name, args_preview
        ));
    }
    result.push(']');
    result
}

/// Truncate a string to a maximum byte length, appending "..." if truncated.
/// UTF-8 safe: finds the nearest char boundary before slicing.
pub fn truncate(s: &str, max_len: usize) -> String {
    nemesis_types::utils::truncate(s, max_len)
}

/// P3.1 (sixth batch): cheap text similarity (char-bigram Jaccard) for the
/// auto-inject dedup pass. Approximates "same memory, near-identical text"
/// without another embedding call — the dedup bar (0.92) is deliberately
/// high so only true near-duplicates are dropped.
// Only called from the `#[cfg(feature = "memory")]` dedup pass — same gate
// pattern as `prefetch_memory_context` above, so memory-off builds stay
// warning-free.
#[cfg_attr(not(feature = "memory"), allow(dead_code))]
fn textwise_similar(a: &str, b: &str) -> f64 {
    let bigrams = |s: &str| -> std::collections::HashSet<(char, char)> {
        let t: Vec<char> = s.chars().collect();
        t.windows(2).map(|w| (w[0], w[1])).collect()
    };
    let (ba, bb) = (bigrams(a), bigrams(b));
    if ba.is_empty() && bb.is_empty() {
        return 1.0; // both empty (or single-char) → identical
    }
    if ba.is_empty() || bb.is_empty() {
        return 0.0;
    }
    let inter = ba.intersection(&bb).count() as f64;
    let union = ba.union(&bb).count() as f64;
    inter / union
}

/// F8 (devtool-upgrade 阶段 3): wildcard-aware membership test for
/// `agents.hidden_tools`. An entry matches a tool name either exactly or,
/// when it ends with `*`, as a prefix (`mcp_*` hides every MCP tool). A bare
/// `*` hides everything. Matching is case-sensitive (tool names are
/// lowercase by convention). Shared by BOTH gates — the supply side
/// ([`AgentLoop::build_tool_defs`]) and the dispatch side
/// ([`AgentLoop::handle_tool_call_at_depth`]) — so the two can never
/// disagree about what is hidden.
pub(crate) fn tool_name_matches_hidden(entries: &[String], name: &str) -> bool {
    entries.iter().any(|entry| {
        if let Some(prefix) = entry.strip_suffix('*') {
            name.starts_with(prefix)
        } else {
            entry == name
        }
    })
}

/// J3 (devtool-upgrade 阶段 4)：从「配置的 server 名 × 已注册工具键」正推
/// 已注册前缀。与 [`nemesis_mcp::manager::McpManager::find_new_servers`] 的
/// 前缀计算同源（`mcp_<sanitize(server)>_`），再看工具表里有没有以它开头
/// 的键。旧实现从注册键反推切分（数下划线取 [2]）：`underscores[2]` 是
/// 第 3 个下划线，注释声称 `mcp_<srv>_` 实则多切一段（server 或工具名含
/// 下划线时切错 → 热重载误判 server 未注册而重复发现）；恰好 2 个下划线
/// 的键还会越界 panic（guard `len() >= 2` 但索引 [2]）。正推天然无歧义。
fn registered_server_prefixes(configured_servers: &[String], tool_keys: &[String]) -> Vec<String> {
    configured_servers
        .iter()
        .map(|s| format!("mcp_{}_", nemesis_mcp::adapter::sanitize_name(s)))
        .filter(|prefix| tool_keys.iter().any(|k| k.starts_with(prefix.as_str())))
        .collect()
}

#[cfg(test)]
mod inbox_tests;
// 自定义 slash 命令改写（2026-08-29）：rewrite_custom_command 决策表测试。
#[cfg(test)]
mod commands_tests;
// N1 (devtool-upgrade 阶段 1)：三级 context_window 解析链测试。
#[cfg(test)]
mod context_window_tests;
// P0 vault（C1/C2，2026-09-22 计划 §3）：凭据别名最后一刻注入机制。
mod credential_injection;
// P0 vault（D2，2026-09-22 计划 §4）：声明式量化风险限制（滑动窗口 + 超限升级）。
pub mod limits;
// P0 vault（C3）：注入机制测试（纯函数 + loop 级四表面防泄漏）。
#[cfg(test)]
mod credential_injection_tests;
// S9 (quality-hardening goal 冲刺 S9): 独立测试文件挂载（声明式，无内联测试）。
#[cfg(test)]
mod s9_tests;
// G4 (devtool-upgrade 阶段 3)：subagent 后台化 + 完成回灌测试。
#[cfg(test)]
mod g4_background_spawn_tests;
// E6 (devtool-upgrade 阶段 2)：手动会话维护（/compact /clear）测试。
#[cfg(test)]
mod e6_maintenance_tests;
// C3 (devtool-upgrade 阶段 2)：编辑后诊断回灌测试（fake LSP server）。
#[cfg(test)]
mod diagnostics_feedback_tests;
// G0 (devtool-upgrade 阶段 3)：SpawnTool 生产化 + run_detached 测试。
#[cfg(test)]
mod spawn_detached_tests;
// F8 (devtool-upgrade 阶段 3)：hidden_tools 双闸（供给过滤 + dispatch 拦截）测试。
#[cfg(test)]
mod f8_hidden_tools_tests;
// M5 (devtool-upgrade 阶段 3)：会话级 context 占用快照测试。
#[cfg(test)]
mod m5_context_status_tests;
// I3 (devtool-upgrade 阶段 3)：子目录指令懒注入（发现 + 会话去重 + 一次性注入）测试。
#[cfg(test)]
mod i3_lazy_instructions_tests;
// F1 (devtool-upgrade 阶段 4)：plan/build 双模式（供给过滤 + dispatch 闸 +
// plans/ 写放行 + slash 切换 + ModeChanged 事件）测试。
#[cfg(test)]
mod f1_plan_mode_tests;
// J3 (devtool-upgrade 阶段 4)：MCP 客户端补齐 agent 侧测试（前缀前向推导
// 回归锁 + 同名冲突改名）。
#[cfg(test)]
mod mcp_reload_tests;
// N2 (devtool-upgrade 阶段 4)：`agents.small_model` 小模型杂务通道测试
// （摘要路由小模型 / 主模型零调用 / 自动压缩路径不受影响）。
#[cfg(test)]
mod n2_small_model_tests;
// D3 (devtool-upgrade 阶段 5)：消息↔文件变更映射的 agent 侧测试
// （dispatch 瀑布收集独立于 checkpoint 挂载 / drain 即清 + 去重）。
#[cfg(test)]
mod d3_tests;
// E3 (devtool-upgrade 阶段 5)：消息级回退/重做编排测试
// （turn 对齐截断 + 文件恢复 + redo 回填 + 陈旧性守卫）。
#[cfg(test)]
mod e3_tests;
// M3 (devtool-upgrade 阶段 5)：会话级 diff 查看器测试（git/JSON 双形态
// 基线对比 + 未挂载/未知路径诚实报错）。
#[cfg(test)]
mod m3_tests;
// E7 (devtool-upgrade 阶段 5)：会话标题自动生成测试（清洗规则 / 端到端
// spawn 落盘 / 无小模型与手动改名跳过）。
#[cfg(test)]
mod e7_tests;
// I5 (devtool-upgrade 阶段 7)：打开文件上下文测试（digest section 渲染
// 有/无两形态字节稳定 + per-turn set/clear 生命周期 + 解析纯函数联测）。
#[cfg(test)]
mod i5_open_files_tests;
// K4 (devtool-upgrade 阶段 7)：IM 编码入口测试（派发语法解析 + 自足续行
// 快照构造的合法消息序列 + B 端变更摘要渲染封顶/溢出注记）。
#[cfg(test)]
mod k4_user_dispatch_tests;
// 429 限流重试环测试（2026-09-17 BUG 文档裁决④⑧：阶梯 + Retry-After 取
// max + 显式进度 + 终局诚实 + 与 context/transient 环互斥）。
#[cfg(test)]
mod chat_log_timing_tests;
#[cfg(test)]
mod rate_limit_retry_tests;
// R1 (2026-09-21)：中间轮正文事件（RoundText）发布语义测试（带叙述的
// 中间轮逐条发布 + 观察者通道与 chat 事件 Vec 隔离 + 空正文轮不发）。
#[cfg(test)]
mod round_text_tests;
// P2-0（docs/PLAN/2026-09-23_agentloop-god-object-decomposition.md §7 T1/T2）：
// characterization + golden transcript harness（基线在 loop/testdata/golden/）。
#[cfg(test)]
mod characterization_tests;
#[cfg(test)]
mod tests;

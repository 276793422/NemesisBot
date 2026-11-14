//! Workflow engine: registration, DAG validation, and topological execution.
//!
//! Mirrors the Go `engine.go` with full workflow lifecycle management:
//! register/unregister workflows, run executions, cancel/resume, list, and
//! optional JSONL-based persistence.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use chrono::Local;
use dashmap::DashMap;
use nemesis_providers::router::LLMProvider;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::checkpoint::{Checkpoint, CheckpointStore, FileCheckpointStore, SerializableContext};
use crate::context::WorkflowContext;
use crate::events::{WorkflowEvent, WorkflowEventManager};
use crate::nodes::{NodeExecutorRegistry, SubWorkflowNodeExecutor};
use crate::persistence::WorkflowPersistence;
use crate::scheduler::{self, ScheduleOutcome};
use crate::triggers::CronTimezone;
use crate::types::{
    Execution, ExecutionState, NodeDef, NodeResult, TriggerConfig, TriggerSource, Workflow,
};

/// Render a path for logging without panicking on non-UTF8.
fn path_dbg(path: &Path) -> String {
    path.display().to_string()
}

// ---------------------------------------------------------------------------
// Workflow summary types (UI API response shape)
// ---------------------------------------------------------------------------

/// UI-facing summary of a workflow definition. Used by `workflow.list`
/// WSAPI response. Driver status comes from [`crate::driver_status`] —
/// the UI must not hardcode trigger-type→driven mapping.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkflowSummary {
    pub name: String,
    pub description: String,
    pub version: String,
    pub node_count: usize,
    pub trigger_count: usize,
    pub triggers: Vec<TriggerSummary>,
    /// Opaque short ID used by the workflow-chat URL (`/workflow/chat/<chat_index>`).
    /// Computed as the first 16 chars of `sha256(name)` hex (64 bit). Stable
    /// across restarts, not enumerable from outside, and unique enough to
    /// avoid collisions for any realistic workflow count.
    pub chat_index: String,
}

/// Trigger summary within a workflow list response. `driven` and `reason`
/// come from [`crate::driver_status:driver_status_for`] — the UI treats
/// these as authoritative and renders whatever they say.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TriggerSummary {
    pub trigger_type: String,
    pub config: HashMap<String, serde_json::Value>,
    pub driven: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// ISO-8601 next-fire timestamp for cron triggers. `None` for non-cron
    /// triggers or when the cron expression is invalid.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_fire_at: Option<String>,
}

/// Compute the next fire time for a cron trigger. Returns `None` if the
/// trigger isn't a cron trigger, the schedule is missing/invalid, or the
/// schedule has no future fire date.
fn cron_next_fire_at_from_trigger(trigger: &TriggerConfig) -> Option<String> {
    if trigger.trigger_type != "cron" {
        return None;
    }
    // schedule 键解析走单一真相源 helper（schedule 优先、legacy expression
    // 回落，BUG-20261114-042）——与 list_cron_workflows / TriggerManager 同源。
    let schedule = crate::triggers::cron_schedule_from_config(&trigger.config)?;
    let tz_str = trigger.config.get("timezone").and_then(|v| v.as_str());
    let cron = croner::Cron::from_str(schedule).ok()?;

    let next_str = match tz_str {
        Some(s) if CronTimezone::from_config_str(s) == Some(CronTimezone::Utc) => {
            let now = chrono::Utc::now();
            cron.find_next_occurrence(&now, false).ok()?.to_rfc3339()
        }
        _ => {
            let now = Local::now();
            cron.find_next_occurrence(&now, false).ok()?.to_rfc3339()
        }
    };
    Some(next_str)
}

/// Make a workflow name safe to use as a filename. Replaces anything
/// outside `[A-Za-z0-9_-]` with `_`. Empty names get a `wf_` prefix to
/// avoid hidden files.
fn sanitize_workflow_filename(name: &str) -> String {
    if name.is_empty() {
        return "wf_unnamed".to_string();
    }
    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.starts_with('.') {
        format!("wf_{}", sanitized)
    } else {
        sanitized
    }
}

/// 读取 workflow 定义文件并返回其内容里的 workflow 名（
/// 碰撞守卫辅助）。parse 失败返回 None——坏文件没有可辨认的属主。
/// serde_yaml 可直接吃 JSON（YAML 1.2 超集），三种扩展名同一解析。
/// 经 `parser::parse`（BUG-20261110-044 深度闸接线）——炸弹文件返
/// None 跳过而非进程 abort。
fn workflow_file_owner(path: &Path) -> Option<String> {
    let data = std::fs::read(path).ok()?;
    crate::parser::parse(&data).ok().map(|w| w.name)
}

/// [`scheduler::ProgressHook`] implementation that saves a checkpoint after
/// every level (1b step 6). Borrows the engine for the duration of the
/// scheduler call — constructed locally in `run_async` so it never outlives
/// the engine reference.
struct CheckpointHook<'a> {
    engine: &'a WorkflowEngine,
    execution_id: String,
    workflow_name: String,
}

#[async_trait::async_trait]
impl<'a> scheduler::ProgressHook for CheckpointHook<'a> {
    async fn on_level_completed(&self, wf_ctx: &WorkflowContext) {
        // Detect whether a human_review node paused us mid-level. The hook
        // is invoked after *every* level, Waiting or not — capturing the
        // waiting node id here keeps resume straightforward.
        let waiting = wf_ctx
            .get_all_node_results()
            .iter()
            .find(|(_, r)| r.state == ExecutionState::Waiting)
            .map(|(id, _)| id.clone());

        if let Err(e) = self
            .engine
            .save_checkpoint(
                &self.execution_id,
                &self.workflow_name,
                wf_ctx,
                waiting.as_deref(),
                None,
                false,
            )
            .await
        {
            warn!(
                target: "nemesis_workflow::engine",
                execution_id = %self.execution_id,
                error = %e,
                "failed to save checkpoint after level"
            );
        }
    }
}

/// Snapshot a [`WorkflowContext`] into its serialisable form.
///
/// Used by [`WorkflowEngine::save_checkpoint`] (1b step 6) when persisting
/// in-flight state. Variables and input are pulled directly; `node_results`
/// are converted one at a time so we can swap `DateTime<Local>` for UTC and
/// the state enum for its snake_case string.
fn build_serialisable_context(wf_ctx: &WorkflowContext) -> SerializableContext {
    use crate::checkpoint::SerializableNodeResult;
    use crate::types::ExecutionState;

    let mut node_results = HashMap::new();
    for (id, nr) in wf_ctx.get_all_node_results() {
        let state_str = match nr.state {
            ExecutionState::Pending => "pending",
            ExecutionState::Running => "running",
            ExecutionState::Completed => "completed",
            ExecutionState::Failed => "failed",
            ExecutionState::Cancelled => "cancelled",
            ExecutionState::Waiting => "waiting",
        }
        .to_string();
        node_results.insert(
            id,
            SerializableNodeResult {
                node_id: nr.node_id.clone(),
                output: nr.output.clone(),
                error: nr.error.clone(),
                state: state_str,
                started_at: nr.started_at.with_timezone(&chrono::Utc),
                ended_at: nr.ended_at.with_timezone(&chrono::Utc),
                metadata: nr.metadata.clone(),
            },
        );
    }

    SerializableContext {
        variables: wf_ctx.get_all_variables(),
        node_results,
        input: wf_ctx.get_all_input(),
    }
}

/// Rebuild a [`WorkflowContext`] from a saved checkpoint snapshot.
///
/// Inverse of [`build_serialisable_context`]: variables/input come back
/// directly; node results need their state string parsed back to the enum
/// and their UTC timestamps converted back to local (the engine uses local
/// time internally for legacy reasons — see `NodeResult::started_at`).
fn restore_context_from_snapshot(snapshot: &SerializableContext) -> WorkflowContext {
    use crate::checkpoint::parse_state;

    let wf_ctx = WorkflowContext::new(snapshot.input.clone());
    for (k, v) in &snapshot.variables {
        wf_ctx.set_var(k, v.clone());
    }
    for (id, snr) in &snapshot.node_results {
        let nr = NodeResult {
            node_id: snr.node_id.clone(),
            output: snr.output.clone(),
            error: snr.error.clone(),
            state: parse_state(&snr.state),
            started_at: snr.started_at.with_timezone(&chrono::Local),
            ended_at: snr.ended_at.with_timezone(&chrono::Local),
            metadata: snr.metadata.clone(),
        };
        wf_ctx.set_node_result(id, nr);
    }
    wf_ctx
}

/// Error type for workflow engine operations.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("workflow not found: {0}")]
    WorkflowNotFound(String),

    #[error("execution not found: {0}")]
    ExecutionNotFound(String),

    #[error("cycle detected in workflow: {0}")]
    CycleDetected(String),

    #[error("execution already completed: {0}")]
    AlreadyCompleted(String),

    #[error("execution failed: {0}")]
    ExecutionFailed(String),

    #[error("node type unknown: {0}")]
    UnknownNodeType(String),

    #[error("invalid execution state: {0}")]
    InvalidState(String),

    #[error("persistence error: {0}")]
    PersistenceError(String),

    #[error("recursion limit exceeded: {0}")]
    RecursionLimitExceeded(String),
}

/// executions 跟踪表上限（资源生命周期审计）：字段语义是「Active and
/// completed executions」——终态条目原本永不移除，常驻 gateway 里 cron
/// 工作流周期性落 Execution（node_results 携带 LLM 输出，KB 级/条），
/// 随进程寿命无界增长。超上限逐出最老的**终态**条目（Completed/Failed/
/// Cancelled）；磁盘 JSONL 持久化仍是全量历史，内存表只是近期视图。
/// 全部活跃（无终态可逐）时放行超限——活跃数受并发上限约束，不误删
/// in-flight 条目（取消/列表语义不受影响）。
const MAX_TRACKED_EXECUTIONS: usize = 256;

/// The core workflow engine.
///
/// Stores registered workflows, tracks executions, and supports optional
/// persistence to disk. Execution runs nodes in topological order based
/// on their `depends_on` fields and edges.
pub struct WorkflowEngine {
    /// Registered workflow definitions (name -> Workflow).
    workflows: DashMap<String, Workflow>,
    /// Active and completed executions (execution ID -> Execution).
    executions: RwLock<HashMap<String, Execution>>,
    /// Node executor registry for looking up executors by type.
    /// Wrapped in Arc so composite executors (parallel/loop) can hold a
    /// back-reference for dispatching to their own children — without this,
    /// parallel/loop children silently fall back to the inline-node stub
    /// which skips LLM/tool/agent/etc types.
    pub(crate) node_executors: Arc<NodeExecutorRegistry>,
    /// Optional persistence directory. If empty, persistence is disabled.
    persistence_dir: Option<PathBuf>,
    /// Optional workflow definitions directory. When set, workflow
    /// create/update/delete operations also write to this directory so
    /// changes survive gateway restarts. Gateway sets this to
    /// `{home}/workspace/workflow/definitions/`.
    workflow_defs_dir: parking_lot::RwLock<Option<PathBuf>>,
    /// Optional checkpoint store. If `None`, checkpoints are kept in memory
    /// (lost on restart). Gateway wires a [`FileCheckpointStore`] so resume
    /// survives process restarts.
    ///
    /// Wrapped in `RwLock` so callers can swap the store post-construction
    /// through [`Self::set_checkpoint_store`] without needing `&mut self`.
    checkpoint_store: parking_lot::RwLock<Option<Arc<dyn CheckpointStore>>>,
    /// Whether the engine has been shut down.
    closed: RwLock<bool>,
    /// Per-execution cancellation tokens. Presence of an entry implies the
    /// execution is still in-flight (Running); completion removes the entry.
    /// Cancelled tokens cause scheduler + node executors to bail out promptly.
    cancel_tokens: DashMap<String, CancellationToken>,
    /// 墙钟保险丝（A 批批 4）：执行超过该秒数即由看门狗 fire cancel token，
    /// 调度器按取消路径收口（执行落 **Failed + 原因注记**，区别于用户取消
    /// 的 Cancelled）。0 = 关（引擎默认关；
    /// gateway 从 config `workflow.wall_clock_budget_secs` 读，缺省 3600）。
    wall_clock_budget_secs: std::sync::atomic::AtomicU64,
    /// Observer / event-bus for workflow lifecycle events
    /// (Started/Completed/Failed/Cancelled). External systems (web dashboards,
    /// log shippers) register observers via [`event_manager`](Self::event_manager).
    event_manager: WorkflowEventManager,
    /// Per-engine workflow call stack (1c-). Tracks every in-flight
    /// execution so we can enforce MAX_RECURSION_DEPTH for AgentTool
    /// nestings and provide diagnostics (snapshot of active frames).
    call_stack: std::sync::Arc<crate::call_stack::WorkflowCallStack>,
    /// Fan-out event bus for `event`/`message` trigger drivers. Gateway
    /// subscribes a task that turns each [`TriggerEvent`] into a
    /// [`workflows_matching_event`] lookup, then fires the matching workflows.
    /// Business code (workflow lifecycle, forge, etc.) calls
    /// [`publish_event`] when something interesting happens.
    event_dispatcher: crate::event_dispatcher::EventDispatcher,
    /// Per-workflow chat serialization state (1c workflow chat page).
    /// Bundles the per-workflow mutex map + pending-execution guard map so
    /// the WebSocket send handler (acquire + store_guard) and the reply
    /// observer (take_guard + drop) share one instance without extra plumbing
    /// through AppState. See [`crate::workflow_chat_state`] for the design.
    workflow_chat_state: std::sync::Arc<crate::workflow_chat_state::WorkflowChatState>,
    /// Shared slot for LLM usage tracking. The 3 LLM-calling node executors
    /// (`llm`, `question_classifier`, `parameter_extractor`) hold a clone of
    /// this slot and record a `RequestLog` per call when it's populated.
    /// Gateway fills it via [`Self::set_usage_store`] after constructing the
    /// `DataStore`. Empty by default so unit tests work without a database.
    usage_store: crate::nodes::UsageStoreSlot,
    /// 统一执行世界（可选装配）。挂上后：控制面写盘点
    /// （persist_workflow / delete_workflow_file / persist_execution）过
    /// `check_writable` 守卫；script 节点获得 world 工具/Spawn 车道。
    /// `None`（单测/裸装配）= 完全旧行为。经 [`Self::set_execution_world`]
    /// 装配（同时重注册 script 执行器带上 world）。
    execution_world:
        parking_lot::RwLock<Option<Arc<dyn nemesis_sandbox::exec_world::ExecutionWorld>>>,
    /// `new_integrated_with_dirs` 收到的工具 registry。仅为
    /// `set_execution_world` 重注册 script 执行器时保留 tools 车道用；
    /// 其余构造器恒 `None`。
    script_tools: Option<Arc<nemesis_tools::registry::ToolRegistry>>,
    /// 注册序列化锁。环检测（读全表引用图）与
    /// `workflows.insert` 必须原子——两个并发 register（A→B 与 B→A 同刻
    /// 提交）在无锁时可在彼此的图快照里都看不到对方，双双过闸后插入
    /// 成环。注册/更新是低频操作，整段持锁无代价。`unregister` 只删边
    /// 不成环，无需参与。
    register_lock: parking_lot::Mutex<()>,
}

impl WorkflowEngine {
    /// Create a new engine with the default set of built-in node executors
    /// and no persistence.
    ///
    /// Note: the `sub_workflow` node type is registered as a stub that returns
    /// an error. For full sub_workflow support, use [`Self::new_arc`] which
    /// wires the engine reference into the executor.
    pub fn new() -> Self {
        Self {
            workflows: DashMap::new(),
            executions: RwLock::new(HashMap::new()),
            node_executors: Arc::new(NodeExecutorRegistry::new()),
            persistence_dir: None,
            workflow_defs_dir: parking_lot::RwLock::new(None),
            checkpoint_store: parking_lot::RwLock::new(None),
            closed: RwLock::new(false),
            cancel_tokens: DashMap::new(),
            wall_clock_budget_secs: std::sync::atomic::AtomicU64::new(0),
            event_manager: WorkflowEventManager::new(),
            call_stack: std::sync::Arc::new(crate::call_stack::WorkflowCallStack::new()),
            event_dispatcher: crate::event_dispatcher::EventDispatcher::default(),
            workflow_chat_state: std::sync::Arc::new(
                crate::workflow_chat_state::WorkflowChatState::new(),
            ),
            usage_store: crate::nodes::new_usage_store_slot(),
            execution_world: parking_lot::RwLock::new(None),
            script_tools: None,
            register_lock: parking_lot::Mutex::new(()),
        }
    }

    /// Create a new engine wrapped in `Arc` with the `sub_workflow` node
    /// executor wired to this engine, matching Go's `NewEngine` behaviour.
    ///
    /// This is the recommended constructor when workflows may contain
    /// `sub_workflow` nodes. Returns `Arc<Self>` because the sub_workflow
    /// executor holds an `Arc<WorkflowEngine>` reference.
    pub fn new_arc() -> Arc<Self> {
        let engine = Arc::new(Self {
            workflows: DashMap::new(),
            executions: RwLock::new(HashMap::new()),
            node_executors: Arc::new(NodeExecutorRegistry::new()),
            persistence_dir: None,
            workflow_defs_dir: parking_lot::RwLock::new(None),
            checkpoint_store: parking_lot::RwLock::new(None),
            closed: RwLock::new(false),
            cancel_tokens: DashMap::new(),
            wall_clock_budget_secs: std::sync::atomic::AtomicU64::new(0),
            event_manager: WorkflowEventManager::new(),
            call_stack: std::sync::Arc::new(crate::call_stack::WorkflowCallStack::new()),
            event_dispatcher: crate::event_dispatcher::EventDispatcher::default(),
            workflow_chat_state: std::sync::Arc::new(
                crate::workflow_chat_state::WorkflowChatState::new(),
            ),
            usage_store: crate::nodes::new_usage_store_slot(),
            execution_world: parking_lot::RwLock::new(None),
            script_tools: None,
            register_lock: parking_lot::Mutex::new(()),
        });

        // Wire the engine into the sub_workflow executor. `register` works
        // through `&self` (RwLock-backed interior mutability) so no `unsafe`
        // mutation of the Arc is required.
        engine.node_executors.register(
            "sub_workflow",
            Arc::new(SubWorkflowNodeExecutor::new(engine.clone())),
        );

        engine
    }

    /// Create a new engine with persistence enabled.
    ///
    /// Execution state is saved to JSONL files under the given directory.
    pub fn with_persistence(persistence_dir: PathBuf) -> Self {
        Self {
            workflows: DashMap::new(),
            executions: RwLock::new(HashMap::new()),
            node_executors: Arc::new(NodeExecutorRegistry::new()),
            persistence_dir: Some(persistence_dir),
            workflow_defs_dir: parking_lot::RwLock::new(None),
            checkpoint_store: parking_lot::RwLock::new(None),
            closed: RwLock::new(false),
            cancel_tokens: DashMap::new(),
            wall_clock_budget_secs: std::sync::atomic::AtomicU64::new(0),
            event_manager: WorkflowEventManager::new(),
            call_stack: std::sync::Arc::new(crate::call_stack::WorkflowCallStack::new()),
            event_dispatcher: crate::event_dispatcher::EventDispatcher::default(),
            workflow_chat_state: std::sync::Arc::new(
                crate::workflow_chat_state::WorkflowChatState::new(),
            ),
            usage_store: crate::nodes::new_usage_store_slot(),
            execution_world: parking_lot::RwLock::new(None),
            script_tools: None,
            register_lock: parking_lot::Mutex::new(()),
        }
    }

    /// Create a new engine with persistence and full sub_workflow support.
    ///
    /// Like [`Self::new_arc`] but with persistence enabled.
    pub fn with_persistence_arc(persistence_dir: PathBuf) -> Arc<Self> {
        let engine = Arc::new(Self {
            workflows: DashMap::new(),
            executions: RwLock::new(HashMap::new()),
            node_executors: Arc::new(NodeExecutorRegistry::new()),
            persistence_dir: Some(persistence_dir),
            workflow_defs_dir: parking_lot::RwLock::new(None),
            checkpoint_store: parking_lot::RwLock::new(None),
            closed: RwLock::new(false),
            cancel_tokens: DashMap::new(),
            wall_clock_budget_secs: std::sync::atomic::AtomicU64::new(0),
            event_manager: WorkflowEventManager::new(),
            call_stack: std::sync::Arc::new(crate::call_stack::WorkflowCallStack::new()),
            event_dispatcher: crate::event_dispatcher::EventDispatcher::default(),
            workflow_chat_state: std::sync::Arc::new(
                crate::workflow_chat_state::WorkflowChatState::new(),
            ),
            usage_store: crate::nodes::new_usage_store_slot(),
            execution_world: parking_lot::RwLock::new(None),
            script_tools: None,
            register_lock: parking_lot::Mutex::new(()),
        });

        engine.node_executors.register(
            "sub_workflow",
            Arc::new(SubWorkflowNodeExecutor::new(engine.clone())),
        );

        engine
    }

    /// Create a new engine with custom node executors.
    pub fn with_executors(node_executors: Arc<NodeExecutorRegistry>) -> Self {
        Self {
            workflows: DashMap::new(),
            executions: RwLock::new(HashMap::new()),
            node_executors,
            persistence_dir: None,
            workflow_defs_dir: parking_lot::RwLock::new(None),
            checkpoint_store: parking_lot::RwLock::new(None),
            closed: RwLock::new(false),
            cancel_tokens: DashMap::new(),
            wall_clock_budget_secs: std::sync::atomic::AtomicU64::new(0),
            event_manager: WorkflowEventManager::new(),
            call_stack: std::sync::Arc::new(crate::call_stack::WorkflowCallStack::new()),
            event_dispatcher: crate::event_dispatcher::EventDispatcher::default(),
            workflow_chat_state: std::sync::Arc::new(
                crate::workflow_chat_state::WorkflowChatState::new(),
            ),
            usage_store: crate::nodes::new_usage_store_slot(),
            execution_world: parking_lot::RwLock::new(None),
            script_tools: None,
            register_lock: parking_lot::Mutex::new(()),
        }
    }

    /// Create a new engine with custom node executors and persistence.
    pub fn with_executors_and_persistence(
        node_executors: Arc<NodeExecutorRegistry>,
        persistence_dir: PathBuf,
    ) -> Self {
        Self {
            workflows: DashMap::new(),
            executions: RwLock::new(HashMap::new()),
            node_executors,
            persistence_dir: Some(persistence_dir),
            workflow_defs_dir: parking_lot::RwLock::new(None),
            checkpoint_store: parking_lot::RwLock::new(None),
            closed: RwLock::new(false),
            cancel_tokens: DashMap::new(),
            wall_clock_budget_secs: std::sync::atomic::AtomicU64::new(0),
            event_manager: WorkflowEventManager::new(),
            call_stack: std::sync::Arc::new(crate::call_stack::WorkflowCallStack::new()),
            event_dispatcher: crate::event_dispatcher::EventDispatcher::default(),
            workflow_chat_state: std::sync::Arc::new(
                crate::workflow_chat_state::WorkflowChatState::new(),
            ),
            usage_store: crate::nodes::new_usage_store_slot(),
            execution_world: parking_lot::RwLock::new(None),
            script_tools: None,
            register_lock: parking_lot::Mutex::new(()),
        }
    }

    /// Create an integrated engine backed by real LLM and Tool node executors.
    ///
    /// Wires `RealLLMNodeExecutor` and `RealToolNodeExecutor` over the mock
    /// defaults, enabling workflows that contain `llm` and `tool` nodes to
    /// actually invoke the configured provider / tool registry at runtime.
    /// Also wires the `sub_workflow` executor so nested workflows work.
    ///
    /// `persistence_dir` controls where in-flight execution state is
    /// JSONL-persisted AND where checkpoints are stored (under
    /// `{persistence_dir}/checkpoints/`). Pass `None` to disable both.
    /// For separate control over JSONL vs checkpoint locations, use
    /// [`new_integrated_with_dirs`].
    ///
    /// Returns `Arc<Self>` because the `sub_workflow` executor holds a back
    /// reference. This is the recommended constructor for gateway / service
    /// integration (milestone 1a-).
    pub fn new_integrated(
        provider: Arc<dyn LLMProvider>,
        tools: Arc<nemesis_tools::registry::ToolRegistry>,
        persistence_dir: Option<PathBuf>,
    ) -> Arc<Self> {
        Self::new_integrated_with_dirs(provider, tools, persistence_dir.clone(), persistence_dir)
    }

    /// Like [`new_integrated`] but splits JSONL persistence from checkpoint
    /// storage. The gateway uses this so JSONL execution logs go to
    /// `workspace/workflow/executions/` while checkpoints live under
    /// `workspace/workflow/checkpoints/` — both under the same `workflow/`
    /// tree but in their own subdirectories instead of mixing at the root.
    ///
    /// - `executions_dir`: where `{workflow_name}_{execution_id}.jsonl` files
    ///   are written. `None` disables JSONL persistence.
    /// - `checkpoint_root`: parent of the `checkpoints/` subdir. `None`
    ///   disables on-disk checkpoints (in-memory only, lost on restart).
    pub fn new_integrated_with_dirs(
        provider: Arc<dyn LLMProvider>,
        tools: Arc<nemesis_tools::registry::ToolRegistry>,
        executions_dir: Option<PathBuf>,
        checkpoint_root: Option<PathBuf>,
    ) -> Arc<Self> {
        let checkpoint_store: Option<Arc<dyn CheckpointStore>> =
            checkpoint_root
                .as_ref()
                .and_then(|dir| match FileCheckpointStore::new(dir.clone()) {
                    Ok(s) => Some(Arc::new(s) as Arc<dyn CheckpointStore>),
                    Err(e) => {
                        warn!(
                            target: "nemesis_workflow::engine",
                            error = %e,
                            dir = ?dir,
                            "failed to initialise checkpoint store; resume will not survive restart"
                        );
                        None
                    }
                });

        let engine = Arc::new(Self {
            workflows: DashMap::new(),
            executions: RwLock::new(HashMap::new()),
            node_executors: Arc::new(NodeExecutorRegistry::new()),
            persistence_dir: executions_dir,
            workflow_defs_dir: parking_lot::RwLock::new(None),
            checkpoint_store: parking_lot::RwLock::new(checkpoint_store),
            closed: RwLock::new(false),
            cancel_tokens: DashMap::new(),
            wall_clock_budget_secs: std::sync::atomic::AtomicU64::new(0),
            event_manager: WorkflowEventManager::new(),
            call_stack: std::sync::Arc::new(crate::call_stack::WorkflowCallStack::new()),
            event_dispatcher: crate::event_dispatcher::EventDispatcher::default(),
            workflow_chat_state: std::sync::Arc::new(
                crate::workflow_chat_state::WorkflowChatState::new(),
            ),
            usage_store: crate::nodes::new_usage_store_slot(),
            execution_world: parking_lot::RwLock::new(None),
            script_tools: Some(tools.clone()),
            register_lock: parking_lot::Mutex::new(()),
        });

        // Override mock node executors with real ones. Pass the engine's
        // shared usage slot so LLM calls get recorded into the gateway's
        // DataStore once `set_usage_store` is called.
        engine.node_executors.register(
            "llm",
            Arc::new(crate::nodes::RealLLMNodeExecutor::with_usage_store(
                provider.clone(),
                engine.usage_store.clone(),
            )),
        );
        engine.node_executors.register(
            "script",
            Arc::new(crate::nodes::ScriptNodeExecutor::with_tools(tools.clone())),
        );
        engine.node_executors.register(
            "tool",
            Arc::new(crate::nodes::RealToolNodeExecutor::new(tools)),
        );
        engine.node_executors.register(
            "question_classifier",
            Arc::new(
                crate::nodes::QuestionClassifierNodeExecutor::with_usage_store(
                    provider.clone(),
                    engine.usage_store.clone(),
                ),
            ),
        );
        engine.node_executors.register(
            "parameter_extractor",
            Arc::new(
                crate::nodes::ParameterExtractorNodeExecutor::with_usage_store(
                    provider,
                    engine.usage_store.clone(),
                ),
            ),
        );
        engine.node_executors.register(
            "sub_workflow",
            Arc::new(SubWorkflowNodeExecutor::new(engine.clone())),
        );

        // Upgrade parallel/loop stubs to real composite executors that
        // dispatch children through this same registry. Without this, the
        // gateway would run ParallelNodeStub / LoopNodeStub which call
        // execute_inline_node() and silently skip LLM/tool/agent/etc
        // children — they'd report Completed with a "skipped" marker while
        // doing none of the actual work.
        NodeExecutorRegistry::install_composite_executors(&engine.node_executors);

        engine
    }

    /// Register an [`AgentRunner`] for `agent` workflow nodes (milestone 1b-).
    ///
    /// Call this after gateway has constructed the AgentLoop. The runner
    /// is responsible for bridging `agent` workflow nodes to the actual
    /// `AgentLoop::process_direct_with_channel` call, so the workflow crate
    /// doesn't need to depend on `nemesis-agent`.
    pub fn register_agent_runner(&self, runner: Arc<dyn crate::nodes::AgentRunner>) {
        self.node_executors.register(
            "agent",
            Arc::new(crate::nodes::AgentNodeExecutor::new(runner)),
        );
    }

    /// Replace the `tool` node executor (A 批五闸补齐).
    ///
    /// Gateway calls this after the AgentLoop exists to install a
    /// [`RealToolNodeExecutor`] wired with the secure dispatch channel
    /// (`new_with_secure_executor`) — tool nodes then run through the full
    /// dispatch waterfall instead of hitting the bare registry. Registry
    /// `register` is overwrite-by-type, so this swaps the stock executor
    /// installed at construction.
    pub fn set_tool_executor(&self, executor: Arc<dyn crate::nodes::NodeExecutor>) {
        self.node_executors.register("tool", executor);
    }

    /// Register a custom [`NodeExecutor`] for a node type. Generalisation of
    /// [`register_agent_runner`](Self::register_agent_runner) for downstream
    /// crates that need to plug in their own executors (test scaffolding,
    /// domain-specific node types, etc).
    pub fn register_node_executor(
        &self,
        node_type: &str,
        executor: Arc<dyn crate::nodes::NodeExecutor>,
    ) {
        self.node_executors.register(node_type, executor);
    }

    /// Wire an LLM usage `DataStore` so the `llm`, `question_classifier`,
    /// and `parameter_extractor` node executors record a `RequestLog` per
    /// LLM call. No-op if never called (slot stays empty, executors skip
    /// recording). Should be called once by the gateway after the DataStore
    /// is constructed.
    pub fn set_usage_store(&self, store: Arc<nemesis_data::DataStore>) {
        let mut guard = self.usage_store.write();
        *guard = Some(store);
    }

    /// Scan a directory for workflow definition files and register each one.
    ///
    /// Supports `.yaml`, `.yml`, and `.json` extensions. Files that fail to
    /// parse or validate are skipped with a warning log; other files are still
    /// loaded. Returns the number of workflows successfully registered.
    ///
    /// Used by the gateway (milestone 1a-) to populate the engine from
    /// `{home}/workspace/workflow/definitions/` at startup.
    pub fn load_workflows_from_dir(&self, dir: &Path) -> Result<usize, EngineError> {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(err) => {
                if err.kind() == std::io::ErrorKind::NotFound {
                    // Missing dir is fine - no workflows to load.
                    return Ok(0);
                }
                return Err(EngineError::PersistenceError(format!(
                    "read workflow dir {:?}: {}",
                    dir, err
                )));
            }
        };

        let mut count = 0usize;
        // read_dir 迭代序是平台实现细节（NTFS 字典序 vs
        // ext4 哈希序）。register_workflow 对重名是后者静默覆盖——同名定义
        // 文件组（用户拷贝变体忘改 name 的常见错误）的加载赢家随平台漂移，
        // cron/触发器/节点配置在 Windows 与 Linux 部署上静默不同。显式按
        // 路径排序，契约固定为「字典序最后者赢」，与文件系统实现解耦。
        let mut entries: Vec<std::fs::DirEntry> = entries.flatten().collect();
        entries.sort_by_key(|e| e.path());
        for entry in entries {
            let path = entry.path();
            let is_wf_file = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|ext| matches!(ext, "yaml" | "yml" | "json"))
                .unwrap_or(false);
            if !is_wf_file {
                continue;
            }

            match crate::parser::parse_file(&path) {
                Ok(wf) => match self.register_workflow(wf) {
                    Ok(_) => count += 1,
                    Err(err) => {
                        tracing::warn!(
                            path = %path.display(),
                            error = %err,
                            "[Workflow] Skipping file: validation failed"
                        );
                    }
                },
                Err(err) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %err,
                        "[Workflow] Skipping file: parse failed"
                    );
                }
            }
        }

        tracing::info!(
            dir = %path_dbg(dir),
            loaded = count,
            "[Workflow] Loaded workflow definitions"
        );
        Ok(count)
    }

    /// Scan registered workflows for `cron` triggers and return them as a
    /// list of `(workflow_name, cron_expr, timezone, static_input)` tuples.
    ///
    /// Only triggers with `trigger_type == "cron"` are returned. The static
    /// input is the trigger's `config.input` object if present, else an empty
    /// map - the gateway / scheduler is free to enrich it at fire time.
    ///
    /// The timezone field is `"local"` (default) or `"utc"`. Any other value
    /// falls back to local with a warning, so a typo doesn't silently disable
    /// a schedule.
    ///
    /// Used by gateway (milestone 1a-) to register cron schedules at startup.
    pub fn list_cron_workflows(
        &self,
    ) -> Vec<(
        String,
        String,
        CronTimezone,
        HashMap<String, serde_json::Value>,
    )> {
        let mut out = Vec::new();
        for entry in self.workflows.iter() {
            let wf = entry.value();
            for trigger in &wf.triggers {
                if trigger.trigger_type != "cron" {
                    continue;
                }
                let schedule = match crate::triggers::cron_schedule_from_config(&trigger.config) {
                    Some(s) => s.to_string(),
                    None => {
                        tracing::warn!(
                            workflow = %wf.name,
                            "[Workflow] Cron trigger missing 'schedule' field \
                             (legacy alias 'expression' also accepted), skipping"
                        );
                        continue;
                    }
                };
                let timezone = match trigger.config.get("timezone").and_then(|v| v.as_str()) {
                    Some(s) => match CronTimezone::from_config_str(s) {
                        Some(tz) => tz,
                        None => {
                            tracing::warn!(
                                workflow = %wf.name,
                                timezone = %s,
                                "[Workflow] Unknown cron timezone, falling back to local"
                            );
                            CronTimezone::Local
                        }
                    },
                    None => CronTimezone::Local,
                };
                let input = trigger
                    .config
                    .get("input")
                    .and_then(|v| v.as_object())
                    .map(|m| {
                        let mut map: HashMap<String, serde_json::Value> =
                            m.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                        // Backfill the unified `input` string so `{{input}}`
                        // in prompts resolves even when the cron config
                        // doesn't explicitly declare an `input` field.
                        if !map.contains_key("input") {
                            let obj: serde_json::Map<String, serde_json::Value> =
                                map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                            let serialized = serde_json::Value::Object(obj).to_string();
                            map.insert("input".to_string(), serde_json::Value::String(serialized));
                        }
                        map
                    })
                    .unwrap_or_default();
                out.push((wf.name.clone(), schedule, timezone, input));
            }
        }
        out
    }

    /// Spawn a tokio task per cron-triggered workflow that fires the workflow
    /// on schedule. Returns `JoinHandle`s so the caller (gateway) can abort
    /// them on shutdown.
    ///
    /// Uses `croner` for cron parsing. Invalid cron expressions are logged
    /// and skipped.
    ///
    /// Each fire calls `start_async(workflow_name, input, TriggerSource::Cron)`.
    /// Errors during execution are logged but do not stop the schedule.
    ///
    /// Timezone handling: if the trigger config has `"timezone": "utc"`, the
    /// cron expression is evaluated against UTC. Otherwise it's evaluated
    /// against local time (the default), which matches how sysadmins think
    /// about cron.
    pub fn spawn_cron_triggers(self: &Arc<Self>) -> Vec<tokio::task::JoinHandle<()>> {
        let cron_workflows = self.list_cron_workflows();
        let mut handles = Vec::with_capacity(cron_workflows.len());

        for (wf_name, schedule, timezone, input) in cron_workflows {
            let cron = match croner::Cron::from_str(&schedule) {
                Ok(c) => c,
                Err(err) => {
                    tracing::warn!(
                        workflow = %wf_name,
                        schedule = %schedule,
                        error = %err,
                        "[Workflow] Invalid cron expression, skipping"
                    );
                    continue;
                }
            };

            let engine = Arc::clone(self);
            let task_name = wf_name.clone();
            let handle = tokio::spawn(async move {
                loop {
                    // Evaluate "now" in the configured timezone so the cron
                    // expression's wall-clock semantics match the user's
                    // intent. `find_next_occurrence` returns the next fire
                    // time in the same TZ; converting both ends to a
                    // Duration cancels out the offset.
                    let now_utc = chrono::Utc::now();
                    let now_local = chrono::Local::now();
                    let delay = match timezone {
                        CronTimezone::Utc => match cron.find_next_occurrence(&now_utc, false) {
                            Ok(next) => (next - now_utc)
                                .to_std()
                                .unwrap_or_else(|_| std::time::Duration::from_millis(100)),
                            Err(err) => {
                                tracing::warn!(
                                    workflow = %task_name,
                                    error = %err,
                                    "[Workflow] Failed to compute next cron fire, stopping schedule"
                                );
                                return;
                            }
                        },
                        CronTimezone::Local => match cron.find_next_occurrence(&now_local, false) {
                            Ok(next) => (next - now_local)
                                .to_std()
                                .unwrap_or_else(|_| std::time::Duration::from_millis(100)),
                            Err(err) => {
                                tracing::warn!(
                                    workflow = %task_name,
                                    error = %err,
                                    "[Workflow] Failed to compute next cron fire, stopping schedule"
                                );
                                return;
                            }
                        },
                    };
                    tokio::time::sleep(delay).await;

                    let exec_engine = Arc::clone(&engine);
                    let exec_name = task_name.clone();
                    let exec_input = input.clone();
                    tokio::spawn(async move {
                        let trigger = TriggerSource::Cron;
                        match exec_engine
                            .start_async(&exec_name, exec_input, Some(trigger))
                            .await
                        {
                            Ok(id) => {
                                tracing::info!(
                                    workflow = %exec_name,
                                    execution_id = %id,
                                    "[Workflow] Cron-triggered execution started"
                                );
                            }
                            Err(err) => {
                                tracing::warn!(
                                    workflow = %exec_name,
                                    error = %err,
                                    "[Workflow] Cron-triggered start_async failed"
                                );
                            }
                        }
                    });
                }
            });
            handles.push(handle);

            tracing::info!(
                workflow = %wf_name,
                schedule = %schedule,
                timezone = %timezone.label(),
                "[Workflow] Cron schedule registered"
            );
        }

        handles
    }

    // -----------------------------------------------------------------------
    // Workflow Registration
    // -----------------------------------------------------------------------

    /// Borrow this engine's event manager so callers can register/unregister
    /// [`WorkflowObserver`]s. Lifetime ties to `&self`; observers stay
    /// registered for the lifetime of the engine unless explicitly removed.
    pub fn event_manager(&self) -> &WorkflowEventManager {
        &self.event_manager
    }

    /// Borrow the trigger-event dispatcher so callers can subscribe a
    /// consumer (gateway wires a task that calls
    /// [`workflows_matching_event`] → [`start_async`] for each match) and
    /// publish events from business code.
    pub fn event_dispatcher(&self) -> &crate::event_dispatcher::EventDispatcher {
        &self.event_dispatcher
    }

    /// Access the per-workflow chat serialization state (1c-).
    /// The WebSocket send handler and the reply observer share this instance
    /// to serialize concurrent workflow_chat sends to the same workflow.
    pub fn workflow_chat_state(
        &self,
    ) -> &std::sync::Arc<crate::workflow_chat_state::WorkflowChatState> {
        &self.workflow_chat_state
    }

    /// Convenience: publish a [`TriggerEvent`] on this engine's dispatcher.
    pub fn publish_event(&self, event: crate::event_dispatcher::TriggerEvent) {
        self.event_dispatcher.publish(event);
    }

    /// Borrow the workflow call stack. Useful for diagnostics — e.g.
    /// listing currently in-flight executions for a "what's running right
    /// now" WSAPI command.
    pub fn call_stack(&self) -> &std::sync::Arc<crate::call_stack::WorkflowCallStack> {
        &self.call_stack
    }

    /// Borrow the checkpoint store, if configured. Returns `None` when
    /// checkpoints are disabled (in-memory engine, no persistence dir).
    ///
    /// Used by gateway restart-recovery (1b step 7) to enumerate and
    /// resume in-flight executions.
    pub fn checkpoint_store(&self) -> Option<Arc<dyn CheckpointStore>> {
        self.checkpoint_store.read().clone()
    }

    /// Replace the checkpoint store. Mainly useful for tests that want to
    /// inject an [`InMemoryCheckpointStore`]; production code should use
    /// [`Self::new_integrated`] which wires a [`FileCheckpointStore`].
    pub fn set_checkpoint_store(&self, store: Arc<dyn CheckpointStore>) {
        *self.checkpoint_store.write() = Some(store);
    }

    /// Scan the checkpoint store for in-flight executions and restore them
    /// into the engine's in-memory map. Called by gateway on startup (1b-
    /// step 7) so executions paused at `human_review` survive process
    /// restarts.
    ///
    /// Returns the number of executions restored. Each restored execution
    /// is reinserted with its last-known state (`Waiting`, typically). The
    /// caller can then call [`Self::resume_execution`] to continue past the
    /// paused node, or leave it parked until an operator decides.
    ///
    /// Config-drift handling: if the workflow definition's hash no longer
    /// matches the checkpoint's hash, the execution is *not* restored and a
    /// warning is logged. The caller can fish it out of the checkpoint store
    /// manually if needed.
    ///
    /// Mid-flight `Running` checkpoints (no `waiting_node`, incomplete
    /// `completed_nodes`) are also restored but left in `Running` state so
    /// observers can see they were in-flight at crash time. Auto-resuming
    /// them would risk double-execution of side effects.
    pub async fn restore_incomplete_executions(&self) -> Result<usize, EngineError> {
        let store = match self.checkpoint_store.read().clone() {
            Some(s) => s,
            None => return Ok(0),
        };

        let execution_ids = store
            .list_executions()
            .await
            .map_err(|e| EngineError::PersistenceError(format!("list_executions: {e}")))?;

        let mut restored = 0usize;
        for exec_id in execution_ids {
            let cp = match store.latest(&exec_id).await {
                Ok(Some(c)) => c,
                Ok(None) => continue,
                Err(e) => {
                    warn!(
                        target: "nemesis_workflow::engine",
                        execution_id = %exec_id,
                        error = %e,
                        "failed to load checkpoint during restore"
                    );
                    continue;
                }
            };

            // Look up the workflow so we can rebuild a full Execution.
            // We need to know the workflow_name to do the lookup, but the
            // checkpoint doesn't carry it directly — derive from execution_id
            // is fragile. Instead, store it on the checkpoint going forward
            // (1b step 7 enhancement): we look it up by scanning registered
            // workflows whose hash matches.
            let workflow = self
                .workflows
                .iter()
                .find(|entry| entry.value().hash() == cp.workflow_hash)
                .map(|e| e.value().clone());

            let workflow = match workflow {
                Some(w) => w,
                None => {
                    warn!(
                        target: "nemesis_workflow::engine",
                        execution_id = %exec_id,
                        hash = %cp.workflow_hash,
                        "skipping restore: workflow definition not found or hash mismatch (config drift)"
                    );
                    continue;
                }
            };

            // Skip terminal checkpoints — nothing to resume. The `terminal`
            // flag is the authoritative signal (Gap 2 fix); the
            // all-completed fallback covers legacy checkpoints written
            // before the flag existed.
            let all_completed =
                cp.completed_nodes.len() >= workflow.nodes.len() && cp.waiting_node.is_none();
            if cp.terminal || all_completed {
                continue;
            }

            // Rebuild the Execution. We don't have the original input/trigger
            // metadata on the checkpoint (the snapshot carries variables/input
            // separately), so we lift them back out of the context snapshot.
            let now = Local::now();
            let mut execution =
                Execution::new(workflow.name.clone(), cp.context_snapshot.input.clone());
            execution.id = exec_id.clone();
            execution.started_at = cp.saved_at.with_timezone(&chrono::Local);
            execution.workflow_hash = Some(cp.workflow_hash.clone());
            // Restore the original trigger source so post-restore observers
            // can still tell webhook / cli / agent invocations apart.
            // (Gap 1 fix.)
            execution.trigger_source = cp.trigger_source.clone();

            // Restore node_results from the snapshot so resume / inspection
            // can see what each node already produced.
            let wf_ctx = restore_context_from_snapshot(&cp.context_snapshot);
            execution.node_results = wf_ctx.get_all_node_results();
            execution.variables = wf_ctx.get_all_variables();

            // Determine terminal-vs-paused state.
            execution.state = if cp.waiting_node.is_some() {
                ExecutionState::Waiting
            } else {
                // Mid-level crash. Leave as Running so observers notice it;
                // gateway can decide whether to auto-resume.
                ExecutionState::Running
            };
            execution.ended_at = if execution.state == ExecutionState::Waiting {
                None
            } else {
                Some(now)
            };

            {
                let mut execs = self.executions.write().await;
                execs.insert(exec_id.clone(), execution);
            }
            self.evict_oldest_terminal_execution().await;
            restored += 1;
            info!(
                target: "nemesis_workflow::engine",
                execution_id = %exec_id,
                state = ?if cp.waiting_node.is_some() { "Waiting" } else { "Running" },
                "restored execution from checkpoint"
            );
        }

        Ok(restored)
    }

    /// Save a checkpoint for `execution_id` capturing the current state of
    /// `wf_ctx`. Best-effort: returns `Ok()` if no store is configured.
    ///
    /// `completed_nodes` is the set of node IDs that have already finished
    /// (extracted from `wf_ctx` when not supplied explicitly). `waiting_node`
    /// is `Some(id)` when the execution is paused at a `human_review` node.
    ///
    /// `terminal=true` marks this checkpoint as capturing a terminal state
    /// (Completed / Failed / Cancelled); `restore_incomplete_executions` skips
    /// terminal checkpoints so finished workflows stay finished across process
    /// restarts. (Gap 2 fix.)
    ///
    /// `trigger_source` is read from the in-memory execution map so the
    /// checkpoint carries the same origin info (webhook / cli / agent / …) as
    /// the live execution. (Gap 1 fix.)
    pub async fn save_checkpoint(
        &self,
        execution_id: &str,
        workflow_name: &str,
        wf_ctx: &WorkflowContext,
        waiting_node: Option<&str>,
        parent_execution_id: Option<&str>,
        terminal: bool,
    ) -> Result<(), EngineError> {
        let store = match self.checkpoint_store.read().clone() {
            Some(s) => s,
            None => return Ok(()),
        };

        let workflow = self
            .get_workflow(workflow_name)
            .ok_or_else(|| EngineError::WorkflowNotFound(workflow_name.to_string()))?;
        let workflow_hash = workflow.hash();

        // Look up the execution's trigger_source so the checkpoint carries
        // forward the original trigger info. Falls back to None when the
        // execution has already been removed (defensive).
        let trigger_source = {
            let execs = self.executions.read().await;
            execs
                .get(execution_id)
                .and_then(|e| e.trigger_source.clone())
        };

        let snapshot = build_serialisable_context(wf_ctx);
        let completed_nodes: HashSet<String> = wf_ctx
            .get_all_node_results()
            .iter()
            .filter(|(_, r)| r.state == ExecutionState::Completed)
            .map(|(id, _)| id.clone())
            .collect();

        let checkpoint = Checkpoint {
            id: uuid::Uuid::new_v4().to_string(),
            execution_id: execution_id.to_string(),
            saved_at: chrono::Utc::now(),
            completed_nodes,
            waiting_node: waiting_node.map(|s| s.to_string()),
            parent_execution_id: parent_execution_id.map(|s| s.to_string()),
            trigger_source,
            terminal,
            context_snapshot: snapshot,
            workflow_hash,
        };

        store
            .save(checkpoint)
            .await
            .map_err(|e| EngineError::PersistenceError(format!("save checkpoint: {e}")))?;
        Ok(())
    }

    /// Register a workflow definition.
    ///
    /// The workflow is validated before registration. If a workflow with the
    /// same name already exists it is replaced.
    pub fn register_workflow(&self, workflow: Workflow) -> Result<(), EngineError> {
        if let Err(e) = crate::parser::validate(&workflow) {
            return Err(EngineError::ExecutionFailed(format!(
                "validate workflow {:?}: {}",
                workflow.name, e
            )));
        }

        // sub_workflow 跨 workflow 引用环检测（persist_workflow
        // 同闸——成环只可能经这两个入口发生，见方法 doc）。check+insert 在
        // register_lock 内原子完成：并发双 register（A→B 与 B→A 同刻提交）
        // 在无锁时可在彼此的图快照里都看不到对方、双双过闸后插入成环。
        let _register_guard = self.register_lock.lock();
        self.check_subworkflow_cycle(&workflow)?;

        self.workflows.insert(workflow.name.clone(), workflow);
        Ok(())
    }

    /// sub_workflow 跨 workflow 引用环检测（注册期闸）。
    ///
    /// [`crate::nodes:SubWorkflowNodeExecutor`] 经
    /// `engine.run(name, input, None)` 触发子执行——`TriggerSource::None` 的
    /// `depth_from_trigger` = 0，[`MAX_RECURSION_DEPTH`] 闸只数 `AgentTool`
    /// 帧，sub_workflow 链完全绕过。而 `parser::validate` 只查**单 workflow**
    /// DAG（跨 workflow 引用零检查），因此 A→B→A（甚至单 workflow 自引用
    /// A→A）两个定义各自合法、都能注册，运行期 A→B→A→… 无限 spawn 子执行：
    /// 每层新 Execution + checkpoint + JSONL persist + call_stack 帧，直到
    /// 内存/磁盘耗尽。跨 workflow 引用环只可能经 [`Self::register_workflow`]
    /// / [`Self::persist_workflow`]（新增或覆盖引用边）形成——两个入口同闸
    /// 即全覆盖（unregister 只删边不成环；盘上互环定义组加载时后者被拒，
    /// 前者引用的目标不存在 → run 时 `WorkflowNotFound` 诚实失败）。
    ///
    /// 引用收集含 parallel/loop 复合子节点（运行时经 registry 同样分派到
    /// SubWorkflowNodeExecutor）。未注册目标放行（既有语义：run 时诚实
    /// WorkflowNotFound，不成环）。覆盖同名注册时以**新定义**的边参与
    /// 检测（旧边随覆盖失效）。
    fn check_subworkflow_cycle(&self, incoming: &Workflow) -> Result<(), EngineError> {
        let mut graph: HashMap<String, Vec<String>> = HashMap::new();
        for entry in self.workflows.iter() {
            graph.insert(
                entry.key().clone(),
                workflow_subworkflow_targets(entry.value()),
            );
        }
        let incoming_targets = workflow_subworkflow_targets(incoming);
        graph.insert(incoming.name.clone(), incoming_targets);

        // 新环必然包含本次注册引入/覆盖的出边（既有节点间的边不变——若
        // 它们之间已有环，早前任一次注册时就该被拒），所以从 incoming 出发
        // 找回到自己的路径即可。
        if let Some(cycle) = find_path_back_to_start(&incoming.name, &graph) {
            return Err(EngineError::ExecutionFailed(format!(
                "workflow {:?} 的 sub_workflow 引用构成环 [{}]：跨 workflow 循环引用会在运行期无限递归 \
                 spawn 子执行（递归深度闸不覆盖 sub_workflow 链），请打断环",
                incoming.name,
                cycle.join(" → ")
            )));
        }
        Ok(())
    }

    /// Find workflows whose `event` trigger matches the given [`TriggerEvent`].
    ///
    /// Walks every registered workflow's trigger list, filters by
    /// `trigger_type == "event"`, then matches `config.event_type` against the
    /// event's `event_type` (glob allowed, e.g. `"workflow.*"`) and any
    /// remaining config keys against `event.data`. Returns the names of
    /// matching workflows (one entry per workflow even if multiple triggers
    /// match).
    pub fn workflows_matching_event(
        &self,
        event: &crate::event_dispatcher::TriggerEvent,
    ) -> Vec<String> {
        let mgr = crate::triggers::TriggerManager::new();
        // Re-register every workflow's triggers into a transient manager so
        // we can reuse the matching helpers without exposing them publicly.
        for entry in self.workflows.iter() {
            for t in &entry.value().triggers {
                let _ = mgr.register_trigger(
                    entry.key(),
                    crate::triggers::TriggerConfig {
                        trigger_type: t.trigger_type.clone(),
                        config: t.config.clone(),
                    },
                );
            }
        }
        // 事件触发环闸。引擎发布的终态事件 data 携带
        // TRIGGER_CHAIN_KEY（产出它的触发链，见发布点注释）；链上已出现的
        // workflow 不再触发——否则「监听 workflow.completed」的工作流完成即
        // 匹配自己，每次完成 spawn 新执行再完成再发布，无限风暴（A→B→A
        // 互触发环同理）。链随 TriggerSource::Event.data 逐代增长且环名被
        // 本闸拒绝，所以环必然在链第一次含重复名时被打断。手动 fire_event
        // （无链键）不受影响；链键形态异常按无链处理（放行，宁可少拦不误拦）。
        let chain =
            trigger_chain_from_value(event.data.get(crate::event_dispatcher::TRIGGER_CHAIN_KEY));
        let matched = mgr.match_trigger_event(event);
        if chain.is_empty() {
            return matched;
        }
        let suppressed: Vec<&String> = matched.iter().filter(|n| chain.contains(n)).collect();
        if !suppressed.is_empty() {
            warn!(
                target: "nemesis_workflow::engine",
                event_type = %event.event_type,
                suppressed = ?suppressed,
                chain = ?chain,
                "event trigger suppressed: target workflow already on its own trigger chain (loop guard)"
            );
        }
        matched.into_iter().filter(|n| !chain.contains(n)).collect()
    }

    /// Find workflows whose `message` trigger matches the given inbound
    /// message fields.
    pub fn workflows_matching_message(
        &self,
        channel: &str,
        sender_id: &str,
        chat_id: &str,
        content: &str,
    ) -> Vec<String> {
        let mgr = crate::triggers::TriggerManager::new();
        for entry in self.workflows.iter() {
            for t in &entry.value().triggers {
                let _ = mgr.register_trigger(
                    entry.key(),
                    crate::triggers::TriggerConfig {
                        trigger_type: t.trigger_type.clone(),
                        config: t.config.clone(),
                    },
                );
            }
        }
        let msg = crate::triggers::InboundMessageRef {
            channel,
            sender_id,
            chat_id,
            content,
        };
        mgr.match_message(&msg)
    }

    /// Retrieve a workflow by name.
    pub fn get_workflow(&self, name: &str) -> Option<Workflow> {
        self.workflows.get(name).map(|r| r.value().clone())
    }

    /// List all registered workflow names.
    pub fn list_workflows(&self) -> Vec<String> {
        self.workflows.iter().map(|r| r.key().clone()).collect()
    }

    /// Set the directory where workflow definition YAML/JSON files are
    /// persisted. When set, [`Self::persist_workflow`] / [`Self::delete_workflow_file`]
    /// write/delete files under this directory. Gateway wires this to
    /// `{home}/workspace/workflow/definitions/` so UI-driven CRUD survives restarts.
    pub fn set_workflow_defs_dir(&self, dir: PathBuf) {
        *self.workflow_defs_dir.write() = Some(dir);
    }

    /// Query whether workflow definitions are persisted to disk.
    pub fn workflow_defs_dir(&self) -> Option<PathBuf> {
        self.workflow_defs_dir.read().clone()
    }

    /// 装配统一执行世界。
    ///
    /// 挂上后：① 控制面写盘点（`persist_workflow` / `delete_workflow_file` /
    /// `persist_execution`）过 `check_writable` 守卫（越界拒）；② 重注册
    /// `script` 执行器带上 world（integrated 装配保留 tools 车道优先）——
    /// 无 registry 的装配（CLI `workflow run`）经 world 工具车道进
    /// executor 子进程/Sandboxie 盒；③ per-node `sandbox: false` 显式
    /// opt-out 走受守卫 Spawn 车道。
    pub fn set_execution_world(&self, world: Arc<dyn nemesis_sandbox::exec_world::ExecutionWorld>) {
        let exec = match self.script_tools.clone() {
            Some(tools) => {
                crate::nodes::ScriptNodeExecutor::with_tools_and_world(tools, world.clone())
            }
            None => crate::nodes::ScriptNodeExecutor::with_world(world.clone()),
        };
        self.node_executors.register("script", Arc::new(exec));
        *self.execution_world.write() = Some(world.clone());
        info!(
            target: "nemesis_workflow::engine",
            semantics = %world.spawn_semantics(),
            world = world.name(),
            "execution world attached (U10 unified execution)"
        );
    }

    /// Query the attached execution world. `None` = bare engine
    /// (unit tests / no assembly) — all guards inert, script falls to bare
    /// spawn.
    pub fn execution_world(&self) -> Option<Arc<dyn nemesis_sandbox::exec_world::ExecutionWorld>> {
        self.execution_world.read().clone()
    }

    /// Upgrade the parallel/loop stub executors to the real composite
    /// executors (children dispatch through this engine's registry — so
    /// world-wired `script` children route via the registry instead of the
    /// inline bare-spawn stub path). The integrated constructor does this
    /// internally; bare engines (CLI `workflow run`) call this explicitly.
    pub fn install_composite_node_executors(&self) {
        NodeExecutorRegistry::install_composite_executors(&self.node_executors);
    }

    /// 控制面写守卫：world 未装配 = 放行（单测/裸装配旧行为）；
    /// 装配后 path 必须落在 world 的 writable_roots 内。
    fn guard_engine_write(&self, path: &std::path::Path) -> Result<(), EngineError> {
        let world = self.execution_world.read().clone();
        if let Some(world) = world
            && let Err(reason) = world.check_writable(path)
        {
            return Err(EngineError::PersistenceError(format!(
                "[U10 execution-world guard] {reason}"
            )));
        }
        Ok(())
    }

    /// Build a detailed summary of every registered workflow, suitable for
    /// the UI's workflow list view. Includes trigger driver status (the
    /// single source of truth — the UI does not hardcode this) and the
    /// next cron fire time when applicable.
    pub fn list_workflows_detailed(&self) -> Vec<WorkflowSummary> {
        self.workflows
            .iter()
            .map(|entry| {
                let wf = entry.value();
                self.build_workflow_summary(wf)
            })
            .collect()
    }

    pub fn build_workflow_summary(&self, wf: &Workflow) -> WorkflowSummary {
        let triggers_summary: Vec<TriggerSummary> = wf
            .triggers
            .iter()
            .map(|t| {
                let driver = crate::driver_status::driver_status_for(&t.trigger_type);
                let next_fire_at = if t.trigger_type == "cron" {
                    cron_next_fire_at_from_trigger(t)
                } else {
                    None
                };
                TriggerSummary {
                    trigger_type: t.trigger_type.clone(),
                    config: t.config.clone(),
                    driven: driver.driven,
                    reason: driver.reason,
                    next_fire_at,
                }
            })
            .collect();

        WorkflowSummary {
            name: wf.name.clone(),
            description: wf.description.clone(),
            version: wf.version.clone(),
            node_count: wf.nodes.len(),
            trigger_count: wf.triggers.len(),
            triggers: triggers_summary,
            chat_index: Self::chat_index(&wf.name),
        }
    }

    /// Compute the workflow-chat URL index for a workflow name.
    ///
    /// Returns the first 16 hex chars (64 bit) of `sha256(name.to_lowercase())` —
    /// stable, opaque, not enumerable. Used by:
    /// - `WorkflowSummary::chat_index` (UI list response)
    /// - `workflow_by_chat_index` (resolving an incoming chat URL back to a workflow)
    /// - WSAPI `workflow.resolve_chat_target`
    ///
    /// Lowercasing the name first lets users type the URL case-insensitively
    /// without changing the canonical workflow name.
    ///
    /// History: this used to be the first 8 hex chars (32 bit); the 64-bit form
    /// landed with the security hardening (32-bit collisions would
    /// misroute URLs and share chat passwords). 8-hex legacy links keep working
    /// via prefix matching in [`Self::workflow_by_chat_index`].
    pub fn chat_index(workflow_name: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(workflow_name.to_lowercase().as_bytes());
        let hash = hasher.finalize();
        let mut out = String::with_capacity(16);
        for b in hash.iter().take(8) {
            out.push_str(&format!("{:02x}", b));
        }
        out
    }

    /// Resolve a chat URL index back to a workflow name. O(n) scan — fine for
    /// realistic workflow counts. 16-hex (current) indexes match exactly;
    /// legacy 8-hex indexes match by prefix — and an ambiguous prefix
    /// (multiple registered workflows share those 8 hex chars) refuses the
    /// resolution instead of silently picking one: a wrong pick would route
    /// chat messages and passwords to the wrong workflow. Returns None if no
    /// registered workflow's chat_index matches (or the prefix is ambiguous).
    pub fn workflow_by_chat_index(&self, index: &str) -> Option<String> {
        let lower = index.to_lowercase();
        if lower.len() == 16 {
            // Canonical form: exact match only.
            return self
                .workflows
                .iter()
                .find(|e| Self::chat_index(e.key()) == lower)
                .map(|e| e.key().clone());
        }
        if lower.len() != 8 {
            return None;
        }
        let mut matched: Option<String> = None;
        for entry in self.workflows.iter() {
            if Self::chat_index(entry.key()).starts_with(&lower) {
                if matched.is_some() {
                    return None;
                }
                matched = Some(entry.key().clone());
            }
        }
        matched
    }

    /// Register + persist a workflow to disk (UI create/update path).
    /// Writes a YAML file under [`workflow_defs_dir`] and registers in
    /// memory. Returns error if persistence is not configured or if the
    /// workflow fails validation.
    pub fn persist_workflow(&self, workflow: Workflow) -> Result<(), EngineError> {
        // Validate first so we don't write a broken file to disk.
        if let Err(e) = crate::parser::validate(&workflow) {
            return Err(EngineError::ExecutionFailed(format!(
                "validate workflow {:?}: {}",
                workflow.name, e
            )));
        }

        let dir = self.workflow_defs_dir.read().clone().ok_or_else(|| {
            EngineError::PersistenceError(
                "workflow_defs_dir not set; call set_workflow_defs_dir() first".to_string(),
            )
        })?;

        // 环检测 + 落盘 + 内存注册整段在 register_lock 内
        // 原子完成（与 register_workflow 同闸同锁）——check 与 insert 之间隔
        // 着写盘 IO，无锁窗口足以让并发注册互相漏看成环。锁内失败臂（碰撞
        // 守卫/写守卫/fs::write/check）先于 insert 返回，盘面不会留半注册态。
        let _register_guard = self.register_lock.lock();
        self.check_subworkflow_cycle(&workflow)?;

        std::fs::create_dir_all(&dir)
            .map_err(|e| EngineError::PersistenceError(format!("create dir {:?}: {}", dir, e)))?;

        let path = dir.join(format!(
            "{}.yaml",
            sanitize_workflow_filename(&workflow.name)
        ));
        // 碰撞守卫：不同 workflow 名可能 sanitize 出同一
        // 文件名（如 "a/b" 与 "a:b" 同落 a_b.yaml），直接覆盖 = 静默丢掉
        // 别人的定义。只放行两类：同名覆盖（UI update 正常路径）与坏文件
        // （parse 失败，本就加载不出来）。
        if path.exists()
            && let Some(existing) = workflow_file_owner(&path)
            && existing != workflow.name
        {
            return Err(EngineError::PersistenceError(format!(
                "workflow {:?} 的落盘文件名与已有 workflow {:?} 碰撞（{}）——拒绝覆盖，请改名其一",
                workflow.name,
                existing,
                path_dbg(&path)
            )));
        }
        // 控制面写守卫：定义文件必须落在执行世界的 writable_roots 内
        // （文件名已 sanitize，守卫兜配置错位/未来回归）。
        self.guard_engine_write(&path)?;
        let yaml = serde_yaml::to_string(&workflow).map_err(|e| {
            EngineError::PersistenceError(format!("serialize workflow {:?}: {}", workflow.name, e))
        })?;
        std::fs::write(&path, yaml)
            .map_err(|e| EngineError::PersistenceError(format!("write {:?}: {}", path, e)))?;

        self.workflows.insert(workflow.name.clone(), workflow);
        Ok(())
    }

    /// Delete a workflow from disk + memory (UI delete path).
    /// Returns Ok() even if the in-memory entry was already absent (idempotent).
    /// Returns error if persistence is configured and the on-disk file exists
    /// but cannot be removed.
    pub fn delete_workflow_file(&self, name: &str) -> Result<(), EngineError> {
        let dir = self.workflow_defs_dir.read().clone();

        // Remove from memory first — if file deletion fails the caller can
        // retry file removal without re-running the whole flow.
        self.workflows.remove(name);

        if let Some(dir) = dir {
            let candidate = dir.join(format!("{}.yaml", sanitize_workflow_filename(name)));
            if candidate.exists() {
                // 碰撞守卫：文件可能属于 sanitize 同名的
                // 另一 workflow（存量盘面事实），只删内容名字匹配（或无属
                // 主的坏文件）。跳过 yaml 不 early-return——下方.yml/.json
                // 变体仍属本名，照常处理。
                let owner_ok = match workflow_file_owner(&candidate) {
                    Some(owner) if owner != name => {
                        warn!(
                            target: "nemesis_workflow::engine",
                            file = %path_dbg(&candidate),
                            owner = %owner,
                            requested = %name,
                            "skip delete: sanitize filename collision, file belongs to another workflow"
                        );
                        false
                    }
                    _ => true,
                };
                if owner_ok {
                    // 控制面写守卫（删除同守卫：能删就能写）。
                    self.guard_engine_write(&candidate)?;
                    std::fs::remove_file(&candidate).map_err(|e| {
                        EngineError::PersistenceError(format!("remove {:?}: {}", candidate, e))
                    })?;
                }
            }
            // Also try.yml +.json variants — load_workflows_from_dir accepts all three.
            for ext in &["yml", "json"] {
                let p = dir.join(format!("{}.{}", sanitize_workflow_filename(name), ext));
                if !p.exists() {
                    continue;
                }
                // 碰撞守卫（同 yaml 主文件）：属他人则跳过。
                if let Some(owner) = workflow_file_owner(&p)
                    && owner != name
                {
                    continue;
                }
                let _ = std::fs::remove_file(&p);
            }
        }
        Ok(())
    }

    /// Validate a workflow definition without registering it. Returns
    /// all errors as a flat list — empty Vec means valid. The UI uses
    /// this for live validation as the user edits.
    pub fn validate_workflow(wf: &Workflow) -> Vec<String> {
        match crate::parser::validate(wf) {
            Ok(()) => Vec::new(),
            Err(e) => vec![e],
        }
    }

    /// Unregister (remove) a workflow definition from the engine.
    ///
    /// This removes the workflow by name. Any in-progress executions of this
    /// workflow are **not** automatically cancelled.
    pub fn unregister(&self, name: &str) {
        self.workflows.remove(name);
    }

    /// executions 表有界化：超 [`MAX_TRACKED_EXECUTIONS`] 时逐出最老的
    /// **终态**条目（started_at 升序，id 决胜毫秒并列）。无终态可逐
    /// （全部活跃）则停止——宁可短暂超限也不误删 in-flight 条目。
    /// 写锁内循环，逐出批量为常数（上限 256，单次插入至多逐 1 条）。
    async fn evict_oldest_terminal_execution(&self) {
        if self.executions.read().await.len() <= MAX_TRACKED_EXECUTIONS {
            return;
        }
        let mut execs = self.executions.write().await;
        while execs.len() > MAX_TRACKED_EXECUTIONS {
            let victim = execs
                .iter()
                .filter(|(_, e)| {
                    matches!(
                        e.state,
                        ExecutionState::Completed
                            | ExecutionState::Failed
                            | ExecutionState::Cancelled
                    )
                })
                .min_by(|a, b| {
                    a.1.started_at
                        .cmp(&b.1.started_at)
                        .then_with(|| a.0.cmp(b.0))
                })
                .map(|(id, _)| id.clone());
            match victim {
                Some(id) => {
                    execs.remove(&id);
                }
                None => break,
            }
        }
    }

    // -----------------------------------------------------------------------
    // Execution Lifecycle
    // -----------------------------------------------------------------------

    /// Create and persist an execution record for the named workflow without
    /// running any nodes. The returned execution has state `Running` and is
    /// tracked in the engine's in-memory map plus (if enabled) on-disk
    /// persistence.
    ///
    /// Use [`run_async`](Self::run_async) with the returned execution's ID to
    /// actually execute the workflow. Most callers should use the convenience
    /// wrappers [`run`](Self::run), [`run_blocking`](Self::run_blocking), or
    /// [`start_async`](Self::start_async) instead.
    ///
    /// `trigger_source` is recorded on the execution for observability and is
    /// used by 1c to enforce `MAX_RECURSION_DEPTH` for `AgentTool` triggers.
    /// Pass `None` when no specific origin applies (e.g., internal tests).
    pub async fn create_execution(
        &self,
        workflow_name: &str,
        input: HashMap<String, serde_json::Value>,
        trigger_source: Option<TriggerSource>,
    ) -> Result<Execution, EngineError> {
        if *self.closed.read().await {
            return Err(EngineError::InvalidState("engine is closed".to_string()));
        }

        // Verify the workflow is registered before we mint an execution ID,
        // so callers get a clean WorkflowNotFound error up front.
        let _workflow = self
            .get_workflow(workflow_name)
            .ok_or_else(|| EngineError::WorkflowNotFound(workflow_name.to_string()))?;

        let mut execution = Execution::new(workflow_name.to_string(), input);
        execution.state = ExecutionState::Running;
        execution.trigger_source = trigger_source.clone();

        // Store execution in memory
        {
            let mut execs = self.executions.write().await;
            execs.insert(execution.id.clone(), execution.clone());
        }
        self.evict_oldest_terminal_execution().await;

        // Persist initial state
        self.persist_execution(&execution).await;

        // Notify observers (Started). Built unconditionally so the event
        // payload reflects exactly what callers passed in.
        self.event_manager
            .emit(WorkflowEvent::Started {
                execution_id: execution.id.clone(),
                workflow_name: workflow_name.to_string(),
                trigger_source,
                timestamp: Local::now(),
            })
            .await;

        Ok(execution)
    }

    /// Run an existing execution (created via `create_execution` or one of the
    /// convenience wrappers) to completion.
    ///
    /// Performs: workflow lookup, context initialization, scheduler invocation,
    /// state transition, node-result collection, and final persistence. The
    /// cancellation token is removed from the engine's map when this call
    /// returns (successfully or otherwise), so `cancel_execution` will be a
    /// no-op for this execution afterwards.
    pub async fn run_async(&self, execution_id: &str) -> Result<Execution, EngineError> {
        if *self.closed.read().await {
            return Err(EngineError::InvalidState("engine is closed".to_string()));
        }

        // Load the execution snapshot and arm the cancellation token under a
        // single write lock (a). The old snapshot was a plain
        // read with the token inserted later, outside any lock: a
        // cancel_execution landing in between found no token to fire, flipped
        // the recorded state to Cancelled, and this function then ran the
        // whole workflow anyway — every node side effect executed and the
        // user's cancel was overwritten by a terminal state. Holding the
        // write lock makes {state check + token insert} atomic against
        // cancel_execution's {state flip + token fire}, so exactly one
        // ordering wins: cancel first → we see the non-Running state here
        // and bail out without running; insert first → cancel finds and
        // fires the token → the scheduler bails out.
        let (mut execution, cancel_token) = {
            let execs = self.executions.write().await;
            let exec = execs
                .get(execution_id)
                .cloned()
                .ok_or_else(|| EngineError::ExecutionNotFound(execution_id.to_string()))?;
            if exec.state != ExecutionState::Running {
                // Already terminal / cancelled before we got here (e.g. the
                // cancel landed between create_execution and this call):
                // never re-run the workflow nor overwrite the recorded
                // outcome — return the record as-is.
                return Ok(exec);
            }
            if self.cancel_tokens.contains_key(execution_id) {
                return Err(EngineError::InvalidState(format!(
                    "execution {execution_id} is already in flight"
                )));
            }
            let token = CancellationToken::new();
            self.cancel_tokens
                .insert(execution_id.to_string(), token.clone());
            (exec, token)
        };

        let workflow = match self.get_workflow(&execution.workflow_name) {
            Some(w) => w,
            None => {
                // 早退必须回收刚装的 cancel token（resume
                // 路径同款清理——「a failed lookup must not leave it
                // orphaned」）。留着 = 此后任何 run_async 都撞「already in
                // flight」，执行记录永久 Running，workflow 重新注册也救不回。
                self.cancel_tokens.remove(execution_id);
                return Err(EngineError::WorkflowNotFound(
                    execution.workflow_name.clone(),
                ));
            }
        };

        // 1c-: push a call-stack frame for this execution. The depth comes
        // from the trigger source (AgentTool-triggered runs carry a depth;
        // everything else is 0). Push can reject if a caller bypassed the
        // WorkflowTool's pre-check and tried to start at depth > MAX.
        let recursion_depth =
            crate::call_stack::CallFrame::depth_from_trigger(&execution.trigger_source);
        // If we're already inside a workflow run (sub_workflow node, or an
        // agent_node that re-invoked the workflow tool's run action), link
        // this new frame to
        // the current top of the stack as its parent. Top-level invocations
        // (stack empty) have no parent.
        //
        // Known limitation (heuristic, not fixed here): under concurrent
        // unrelated runs the "top of stack" may be a stranger's frame —
        // node executors cannot see their own execution id, so threading
        // the real parent would require execution-context plumbing through
        // the NodeExecutor trait. The heuristic is exact for serial
        // nesting; concurrent top-level runs may record a bogus parent
        // (diagnostic metadata only).
        let parent_execution_id = self
            .call_stack
            .snapshot()
            .last()
            .map(|f| f.execution_id.clone());
        // Own-frame parent, captured once at push time. The shared stack is
        // concurrent — re-reading snapshot().last() later may hit another
        // run's frame, so the checkpoint below must reuse this captured
        // value instead of guessing from the stack again.
        let own_parent_execution_id = parent_execution_id.clone();
        let frame = crate::call_stack::CallFrame {
            execution_id: execution.id.clone(),
            workflow_name: execution.workflow_name.clone(),
            parent_execution_id,
            trigger_source: execution.trigger_source.clone(),
            recursion_depth,
        };
        if let Err(reason) = self.call_stack.push(frame) {
            // 同上，早退前回收 cancel token。
            self.cancel_tokens.remove(execution_id);
            return Err(EngineError::RecursionLimitExceeded(reason));
        }

        // 运行账本开账（确定性 id=workflow_run:{execution_id}；账本
        // 未安装=零变化，best-effort 不阻断执行）。放在递归闸之后——
        // 超限拒绝不产生开账行。Waiting（人工审批挂起）的 run 保持开账，
        // resume 收口；终态在下方 settle。
        let ledger_run_id = nemesis_taskstore::ledger::workflow_run_id(&execution.id);
        nemesis_taskstore::ledger::begin_run(
            &ledger_run_id,
            "workflow_run",
            &execution.workflow_name,
            "workflow",
            serde_json::json!({ "trigger": execution.trigger_source.clone() }),
        );

        // Initialize workflow context from execution input. Identity is
        // filled from the trigger source (安全闸补齐 A4 归因穿透：execution
        // id / workflow name / trigger label / 触发者身份三元组 / 递归深度
        // 随 wf_ctx 下行，tool 节点的 secure executor 消费——审计归因 +
        // 审批卡路由 + 会话级豁免的身份来源；本注释取代原先「node executors
        // cannot see their own execution id」的欠账标注).
        let (trig_session, trig_chat, trig_sender) = execution
            .trigger_source
            .as_ref()
            .map(|t| t.identity_triple())
            .unwrap_or_default();
        let wf_identity = crate::context::WorkflowIdentity {
            execution_id: execution.id.clone(),
            workflow_name: execution.workflow_name.clone(),
            trigger_label: execution
                .trigger_source
                .as_ref()
                .map(|t| t.label())
                .unwrap_or("unknown")
                .to_string(),
            session_key: trig_session,
            chat_id: trig_chat,
            sender_id: trig_sender,
            recursion_depth,
        };
        let mut wf_ctx = WorkflowContext::new_with_identity(wf_identity, execution.input.clone());
        for (k, v) in &workflow.variables {
            // Workflow YAML stores initial variables as strings; lift them to
            // Value::String so the rest of the engine only sees JSON values.
            wf_ctx.set_var(k, serde_json::Value::String(v.clone()));
        }

        // Stamp the execution with the workflow's structural hash so the
        // resume path can detect config drift (1b step 5).
        execution.workflow_hash = Some(workflow.hash());

        // 墙钟保险丝（A 批批 4）：budget>0 时挂看门狗，到点置 timed_out 旗标
        // 并 fire cancel token——调度器按取消路径收口，run_async 下方把该
        // 路径的终态落 **Failed + 原因注记**（PLAN 口径「超时 → cancel +
        // 终态 Failed 带原因注记」），与用户主动取消（Cancelled）区分。
        // 调度返回后立即 abort 看门狗（正常完成的执行不得被迟到触发误杀
        // ——abort 前 token 已从 cancel_tokens 摘除，即便竞态 fire 也无
        // 消费方；迟到触发但调度已返回 Completed 的工作照实记 Completed
        // ——工作真做完了，事后改判 Failed 才是造假）。0 = 关。
        let wall_clock_watchdog = {
            let budget = self
                .wall_clock_budget_secs
                .load(std::sync::atomic::Ordering::Relaxed);
            if budget > 0 {
                let token = cancel_token.clone();
                let execution_id = execution.id.clone();
                let timed_out = Arc::new(std::sync::atomic::AtomicBool::new(false));
                let flag = timed_out.clone();
                Some((
                    tokio::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_secs(budget)).await;
                        flag.store(true, std::sync::atomic::Ordering::Relaxed);
                        warn!(
                            target: "nemesis_workflow::engine",
                            execution_id = %execution_id,
                            budget_secs = budget,
                            "wall clock budget exceeded — cancelling execution"
                        );
                        token.cancel();
                    }),
                    timed_out,
                    budget,
                ))
            } else {
                None
            }
        };

        // Execute the workflow using the scheduler. When a checkpoint store
        // is wired, install a per-level hook so an interrupted execution can
        // resume from the most recently completed level.
        let has_store = self.checkpoint_store.read().is_some();
        let schedule_result = if has_store {
            let hook = CheckpointHook {
                engine: self,
                execution_id: execution.id.clone(),
                workflow_name: execution.workflow_name.clone(),
            };
            scheduler::schedule_with_hook(
                &workflow.nodes,
                &workflow.edges,
                &self.node_executors,
                &mut wf_ctx,
                cancel_token,
                &hook,
            )
            .await
        } else {
            scheduler::schedule(
                &workflow.nodes,
                &workflow.edges,
                &self.node_executors,
                &mut wf_ctx,
                cancel_token,
            )
            .await
        };
        if let Some((handle, _, _)) = &wall_clock_watchdog {
            handle.abort();
        }
        // 看门狗是否已触发（含触发后调度才返回的竞态窗口）：true =
        // Cancelled 收口改判 Failed + 原因注记。
        let (wall_clock_timed_out, wall_clock_budget) = match &wall_clock_watchdog {
            Some((_, flag, budget)) => (flag.load(std::sync::atomic::Ordering::Relaxed), *budget),
            None => (false, 0),
        };

        // Token is no longer needed; remove from map.
        self.cancel_tokens.remove(&execution.id);

        let now = Local::now();

        match schedule_result {
            Ok(ScheduleOutcome::Cancelled) => {
                // 墙钟看门狗触发的取消 → Failed + 原因注记（PLAN 口径
                // 「超时 → cancel + 终态 Failed 带原因注记」），与用户主动
                // 取消（Cancelled）区分；下方账本 settle 同样按 failed 带
                // 原因，审计面可辨。
                if wall_clock_timed_out {
                    execution.state = ExecutionState::Failed;
                    execution.error = Some(format!(
                        "wall clock budget exceeded ({}s) — cancelled by watchdog",
                        wall_clock_budget
                    ));
                    execution.ended_at = Some(now);
                } else {
                    execution.state = ExecutionState::Cancelled;
                    execution.ended_at = Some(now);
                }
            }
            Ok(ScheduleOutcome::Completed) => {
                // Check if any node is in waiting state (human review)
                let all_completed = wf_ctx
                    .get_all_node_results()
                    .values()
                    .all(|r| r.state != ExecutionState::Waiting);

                if all_completed {
                    execution.state = ExecutionState::Completed;
                    execution.ended_at = Some(now);
                } else {
                    // 缺陷 10 修复：停在人工审批的执行**没有结束**，
                    // ended_at 必须保持空——旧实现上方无条件写 Some(now)，Waiting
                    // 分支不回清，观察方（UI/监控/审批队列）会把它当已完结。
                    // 与下方 resume 路径（still_waiting → ended_at = None）同款语义。
                    execution.state = ExecutionState::Waiting;
                    execution.ended_at = None;
                }
            }
            Err(err) => {
                execution.state = ExecutionState::Failed;
                execution.error = Some(err);
                execution.ended_at = Some(now);
            }
        }

        // 运行账本收口（仅终态；Waiting 保持开账等 resume——挂起的
        // 执行本身没有结束，账面如实镜像）。幂等：resume 收口同值 CAS。
        match execution.state {
            ExecutionState::Completed => {
                nemesis_taskstore::ledger::settle_run(&ledger_run_id, "succeeded", None);
            }
            ExecutionState::Failed => {
                let err = execution.error.as_deref();
                nemesis_taskstore::ledger::settle_run(&ledger_run_id, "failed", err);
            }
            ExecutionState::Cancelled => {
                nemesis_taskstore::ledger::settle_run(&ledger_run_id, "cancelled", None);
            }
            _ => {}
        }

        // Copy results from context into execution
        execution.node_results = wf_ctx.get_all_node_results();
        // 变量同样回写——workflow.variables（YAML
        // `variables:` 段）只在上方 seeding 进 wf_ctx，execution.variables
        // 始终空；resume（resume_execution_node）与 cancel（cancel_execution
        // 的终态 checkpoint）都从 execution.variables 重建 wf_ctx，human_review
        // 挂起后进程内 resume 的下游节点读 `{{var}}` 全部落空（模板残留 /
        // 条件边按不成立跳过）。对齐 resume 路径与 checkpoint 恢复路径
        // （restore_incomplete_executions）的既有回写惯例。
        execution.variables = wf_ctx.get_all_variables();

        // Persist final state
        self.persist_execution(&execution).await;

        // Save a checkpoint capturing the post-run state. For Completed /
        // Failed / Cancelled this is a *terminal* checkpoint —
        // `restore_incomplete_executions` skips terminal checkpoints so the
        // finished workflow doesn't get resurrected on the next process
        // restart. For Waiting (paused at a human_review) this captures the
        // waiting node so resume can find it. (Gap 2 fix.)
        //
        // parent_execution_id is read from the live call-stack frame (our own
        // frame is still on the stack at this point — pop happens below).
        let is_terminal = matches!(
            execution.state,
            ExecutionState::Completed | ExecutionState::Failed | ExecutionState::Cancelled
        );
        let waiting: Option<String> = if execution.state == ExecutionState::Waiting {
            smallest_waiting_node_id(&wf_ctx.get_all_node_results())
        } else {
            None
        };
        // Own frame's parent, captured at push time (our frame is still on
        // the stack here, but under concurrency snapshot().last() may be a
        // stranger's frame — never re-derive from the shared stack).
        let parent_for_checkpoint = own_parent_execution_id;
        if let Err(e) = self
            .save_checkpoint(
                &execution.id,
                &execution.workflow_name,
                &wf_ctx,
                waiting.as_deref(),
                parent_for_checkpoint.as_deref(),
                is_terminal,
            )
            .await
        {
            warn!(
                target: "nemesis_workflow::engine",
                execution_id = %execution.id,
                error = %e,
                "failed to save terminal checkpoint"
            );
        }

        // Update in-memory execution
        {
            let mut execs = self.executions.write().await;
            execs.insert(execution.id.clone(), execution.clone());
        }

        // Emit terminal-state event. Only the four terminal states are
        // observable here (Waiting is non-terminal from the engine's POV —
        // observers see Started, then either Completed/Failed/Cancelled).
        let workflow_name_for_event = execution.workflow_name.clone();
        let execution_id_for_event = execution.id.clone();
        // 三个终态事件共享同一份 data 骨架；触发链 =
        // 上代事件携带的链（若本执行由事件触发）+ 本 workflow 名，随
        // TRIGGER_CHAIN_KEY 发布——订阅端（workflows_matching_event）据此
        // 拒绝把链上已出现的 workflow 再次触发，断掉 event trigger
        // 自触发/互触发环。
        let mut trigger_chain_for_event = event_trigger_chain_of(execution.trigger_source.as_ref());
        trigger_chain_for_event.push(workflow_name_for_event.clone());
        let mut shared_event_data = std::collections::HashMap::new();
        shared_event_data.insert(
            "workflow_name".to_string(),
            serde_json::json!(workflow_name_for_event),
        );
        shared_event_data.insert(
            crate::event_dispatcher::TRIGGER_CHAIN_KEY.to_string(),
            serde_json::json!(trigger_chain_for_event),
        );
        match execution.state {
            ExecutionState::Completed => {
                let event_name = workflow_name_for_event.clone();
                let event_id = execution_id_for_event.clone();
                self.event_manager
                    .emit(WorkflowEvent::Completed {
                        execution_id: event_id,
                        workflow_name: event_name,
                        timestamp: Local::now(),
                    })
                    .await;
                let mut data = shared_event_data.clone();
                data.insert("status".to_string(), serde_json::json!("completed"));
                self.publish_event(
                    crate::event_dispatcher::TriggerEvent::new("workflow.completed", data)
                        .with_source_execution_id(execution_id_for_event),
                );
            }
            ExecutionState::Failed => {
                let err = execution.error.clone().unwrap_or_default();
                let event_name = workflow_name_for_event.clone();
                let event_id = execution_id_for_event.clone();
                self.event_manager
                    .emit(WorkflowEvent::Failed {
                        execution_id: event_id,
                        workflow_name: event_name,
                        error: err,
                        timestamp: Local::now(),
                    })
                    .await;
                let mut data = shared_event_data.clone();
                data.insert("status".to_string(), serde_json::json!("failed"));
                self.publish_event(
                    crate::event_dispatcher::TriggerEvent::new("workflow.failed", data)
                        .with_source_execution_id(execution_id_for_event),
                );
            }
            ExecutionState::Cancelled => {
                let event_name = workflow_name_for_event.clone();
                let event_id = execution_id_for_event.clone();
                self.event_manager
                    .emit(WorkflowEvent::Cancelled {
                        execution_id: event_id,
                        workflow_name: event_name,
                        timestamp: Local::now(),
                    })
                    .await;
                let mut data = shared_event_data.clone();
                data.insert("status".to_string(), serde_json::json!("cancelled"));
                self.publish_event(
                    crate::event_dispatcher::TriggerEvent::new("workflow.cancelled", data)
                        .with_source_execution_id(execution_id_for_event),
                );
            }
            _ => {}
        }

        // Tear down our call-stack frame now that the execution has
        // settled. Identity-based removal: concurrent runs interleave on
        // the shared stack, so popping the top would take out another
        // execution's frame and strand ours (phantom frames corrupt
        // snapshot()-based introspection and parent derivation).
        self.call_stack.remove(&execution.id);

        Ok(execution)
    }

    /// Run a registered workflow by name (the core convenience entry point).
    ///
    /// Equivalent to `create_execution(name, input, trigger_source)` followed
    /// by `run_async(execution_id)`. Returns the completed execution.
    ///
    /// This is the Rust equivalent of Go's `Engine.Run`. Prefer
    /// [`run_blocking`](Self::run_blocking) from synchronous contexts or
    /// [`start_async`](Self::start_async) when you need fire-and-forget
    /// semantics with later status polling.
    pub async fn run(
        &self,
        workflow_name: &str,
        input: HashMap<String, serde_json::Value>,
        trigger_source: Option<TriggerSource>,
    ) -> Result<Execution, EngineError> {
        let execution = self
            .create_execution(workflow_name, input, trigger_source)
            .await?;
        self.run_async(&execution.id).await
    }

    /// Synchronous wrapper around [`run`](Self::run).
    ///
    /// Builds a single-threaded tokio runtime and blocks the calling thread
    /// until the workflow completes. **Panics** if called from within an
    /// existing tokio runtime context (use [`run`](Self::run) from inside
    /// async code instead).
    ///
    /// Intended for CLI entry points and other inherently synchronous
    /// callers. The runtime is dropped (and its threads released) when this
    /// function returns.
    pub fn run_blocking(
        &self,
        workflow_name: &str,
        input: HashMap<String, serde_json::Value>,
        trigger_source: Option<TriggerSource>,
    ) -> Result<Execution, EngineError> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| EngineError::InvalidState(format!("runtime creation failed: {}", e)))?;
        rt.block_on(self.run(workflow_name, input, trigger_source))
    }

    /// Fire-and-forget entry point for async callers.
    ///
    /// Creates an execution record, spawns a background tokio task to run it,
    /// and returns the execution ID immediately. The caller can poll
    /// [`get_execution`](Self::get_execution) to observe progress.
    ///
    /// Requires `Arc<WorkflowEngine>` because the spawned task holds an
    /// engine reference. Use [`WorkflowEngine::new_arc`] or
    /// [`WorkflowEngine::with_persistence_arc`] to obtain a suitable handle.
    ///
    /// Returns `Err` synchronously if the workflow is unknown or the engine
    /// is closed; errors raised by the background task itself are logged and
    /// surfaced via the eventual execution state (Failed), not through this
    /// return value.
    pub async fn start_async(
        self: Arc<Self>,
        workflow_name: &str,
        input: HashMap<String, serde_json::Value>,
        trigger_source: Option<TriggerSource>,
    ) -> Result<String, EngineError> {
        let execution = self
            .create_execution(workflow_name, input, trigger_source)
            .await?;
        let execution_id = execution.id.clone();
        let engine = self.clone();
        tokio::spawn(async move {
            if let Err(e) = engine.run_async(&execution_id).await {
                warn!(
                    "[Workflow] Background execution {} failed: {}",
                    execution_id, e
                );
            }
        });
        Ok(execution.id)
    }

    /// Start a new execution for the named workflow.
    ///
    /// This is an alias for [`run`](Self::run) that validates the DAG inline
    /// and executes nodes in dependency order without the full scheduler.
    /// Prefer `run` for full-featured execution; this method provides the
    /// simpler inline execution path used by existing tests.
    ///
    /// **Deprecated**: this code path bypasses the scheduler — no retry,
    /// per-node timeout, conditional edges, cancellation token, or hooks.
    /// Production code should call `run` / `run_async` / `start_async`. This
    /// method is retained only because legacy tests build on it.
    #[deprecated(
        since = "0.1.0",
        note = "use `run` / `run_async` / `start_async` instead — this path skips retry/timeout/conditional-edges/cancellation"
    )]
    pub async fn start_execution(
        &self,
        workflow_name: &str,
        input: HashMap<String, serde_json::Value>,
    ) -> Result<Execution, EngineError> {
        // Check if engine is closed
        if *self.closed.read().await {
            return Err(EngineError::InvalidState("engine is closed".to_string()));
        }

        let workflow = self
            .get_workflow(workflow_name)
            .ok_or_else(|| EngineError::WorkflowNotFound(workflow_name.to_string()))?;

        // Validate DAG: no cycles.
        validate_dag(&workflow.nodes)?;

        let mut execution = Execution::new(workflow_name.to_string(), input);
        execution.state = ExecutionState::Running;

        // Build execution context from input.
        let mut context = execution.input.clone();

        // Build node lookup.
        let node_map: HashMap<&str, &NodeDef> =
            workflow.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

        // Track completed nodes.
        let mut completed: HashSet<String> = HashSet::new();
        let remaining: HashSet<String> = workflow.nodes.iter().map(|n| n.id.clone()).collect();

        // Execute in rounds: each round picks all nodes whose deps are satisfied.
        let mut remaining_vec: Vec<String> = remaining.into_iter().collect();
        let mut iterations = 0;
        let max_iterations = remaining_vec.len() + 1;

        while !remaining_vec.is_empty() && iterations < max_iterations {
            iterations += 1;
            let mut progress = false;

            // Find nodes whose dependencies are all completed.
            let ready: Vec<String> = remaining_vec
                .iter()
                .filter(|id| {
                    let node = node_map[id.as_str()];
                    node.depends_on.iter().all(|dep| completed.contains(dep))
                })
                .cloned()
                .collect();

            for node_id in ready {
                let node = node_map[node_id.as_str()];

                let executor = self
                    .node_executors
                    .get(&node.node_type)
                    .ok_or_else(|| EngineError::UnknownNodeType(node.node_type.clone()))?;

                let wf_ctx = WorkflowContext::new(context.clone());
                let result = match executor.execute(node, &context, &wf_ctx).await {
                    Ok(r) => r,
                    Err(e) => NodeResult {
                        node_id: node_id.clone(),
                        output: serde_json::Value::Null,
                        error: Some(e.clone()),
                        state: ExecutionState::Failed,
                        started_at: Local::now(),
                        ended_at: Local::now(),
                        metadata: HashMap::new(),
                    },
                };

                let node_state = result.state;
                execution
                    .node_results
                    .insert(node_id.clone(), result.clone());

                // Merge output into context. 双写命名空间键与整对象：
                // {{节点id.字段}} / {{节点id}}（能力表承诺的传值语法，
                // 计划类 C 文档-引擎对齐）；平铺键保留，存量工作流不受影响
                // （同名平铺键后到者覆盖，能力表已引导生成器用命名空间形式）。
                if let Some(obj) = result.output.as_object() {
                    for (k, v) in obj {
                        context.insert(k.clone(), v.clone());
                        context.insert(format!("{node_id}.{k}"), v.clone());
                    }
                }
                context.insert(node_id.clone(), result.output.clone());

                remaining_vec.retain(|id| id != &node_id);
                completed.insert(node_id.clone());
                progress = true;

                if node_state == ExecutionState::Failed {
                    execution.state = ExecutionState::Failed;
                    execution.ended_at = Some(Local::now());
                    let updated = execution.clone();
                    {
                        let mut execs = self.executions.write().await;
                        execs.insert(execution.id.clone(), updated.clone());
                    }
                    self.persist_execution(&updated).await;
                    return Ok(updated);
                }

                if node_state == ExecutionState::Waiting {
                    execution.state = ExecutionState::Waiting;
                    execution.ended_at = Some(Local::now());
                    let updated = execution.clone();
                    {
                        let mut execs = self.executions.write().await;
                        execs.insert(execution.id.clone(), updated.clone());
                    }
                    self.persist_execution(&updated).await;
                    return Ok(updated);
                }
            }

            if !progress {
                // Deadlock: remaining nodes have unsatisfied deps (should be caught by
                // cycle detection, but guard anyway).
                execution.state = ExecutionState::Failed;
                execution.ended_at = Some(Local::now());
                let updated = execution.clone();
                {
                    let mut execs = self.executions.write().await;
                    execs.insert(execution.id.clone(), updated.clone());
                }
                self.persist_execution(&updated).await;
                return Err(EngineError::CycleDetected(workflow_name.to_string()));
            }
        }

        execution.state = ExecutionState::Completed;
        execution.ended_at = Some(Local::now());

        let updated = execution.clone();
        {
            let mut execs = self.executions.write().await;
            execs.insert(execution.id.clone(), updated.clone());
        }
        self.persist_execution(&updated).await;
        Ok(updated)
    }

    // -----------------------------------------------------------------------
    // Execution Queries
    // -----------------------------------------------------------------------

    /// Retrieve an execution by its ID.
    ///
    /// If the execution is not found in memory, attempts to load it from
    /// the persistence layer (if persistence is enabled).
    pub async fn get_execution(&self, id: &str) -> Option<Execution> {
        // Try in-memory first
        {
            let execs = self.executions.read().await;
            if let Some(exec) = execs.get(id) {
                return Some(exec.clone());
            }
        }

        // Try loading from persistence
        self.load_execution_from_disk(id).await
    }

    /// Retrieve an execution by its ID, returning an error if not found.
    pub async fn get_execution_or_err(&self, id: &str) -> Result<Execution, EngineError> {
        self.get_execution(id)
            .await
            .ok_or_else(|| EngineError::ExecutionNotFound(id.to_string()))
    }

    /// List all executions, optionally filtered by workflow name.
    ///
    /// When `workflow_name` is empty, returns all executions. Otherwise,
    /// returns only executions for the specified workflow.
    ///
    /// Result is ordered newest-first (`started_at` descending, `id`
    /// descending as tie-break). The backing map is a `HashMap` with no
    /// inherent order — without an explicit sort, consumers that truncate to
    /// a limit (REST / WSAPI `limit` param) would take an arbitrary,
    /// unstable subset.
    pub async fn list_executions(&self, workflow_name: Option<&str>) -> Vec<Execution> {
        let execs = self.executions.read().await;
        let mut listed: Vec<Execution> = execs
            .values()
            .filter(|exec| {
                workflow_name
                    .map(|name| exec.workflow_name == name)
                    .unwrap_or(true)
            })
            .cloned()
            .collect();
        listed.sort_by(|a, b| {
            b.started_at
                .cmp(&a.started_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        listed
    }

    // -----------------------------------------------------------------------
    // Execution Control
    // -----------------------------------------------------------------------

    /// Cancel a running or waiting execution.
    ///
    /// Triggers the execution's cancellation token (causing scheduler and node
    /// executors to bail out), marks the execution as cancelled, and persists
    /// the state change. Only executions in `Running` or `Waiting` state can be
    /// cancelled. Returns the updated execution.
    /// 终态事件单一发射点（BUG-20261114-035）：resume 收口（
    /// [`resume_execution_node`] 尾段 / [`settle_resume_outcome`]）与
    /// [`cancel_execution`] 的 token-缺席臂经此发 [`WorkflowEvent`]，与
    /// run_async 首跑尾段同契约。此前这些收口路径不发任何事件——唯一
    /// 生产观察者 WorkflowChatReplyObserver 只在 Completed/Failed/
    /// Cancelled 事件里释放 workflow_chat per-workflow 互斥守卫，事件
    /// 缺席 = 守卫永久滞留 = 该工作流后续聊天请求静默卡死。
    ///
    /// 刻意只发 observer 事件（event_manager.emit），不发布
    /// publish_event TriggerEvent：resume 后的终态若再进入事件触发链，
    /// 会把「resume 后的完成/取消」当新事件扩散给事件触发工作流——
    /// 产品语义变更，超出本修复范围；run_async 首跑尾段的 TriggerEvent
    /// 发布维持原样。Waiting 非终态：保持沉默（观察者守卫必须留到下
    /// 一次 resume / 取消，与 run_async 首跑 Waiting 不发声一致）。
    async fn emit_terminal_event(&self, execution: &Execution) {
        let event = match execution.state {
            ExecutionState::Completed => WorkflowEvent::Completed {
                execution_id: execution.id.clone(),
                workflow_name: execution.workflow_name.clone(),
                timestamp: Local::now(),
            },
            ExecutionState::Failed => WorkflowEvent::Failed {
                execution_id: execution.id.clone(),
                workflow_name: execution.workflow_name.clone(),
                error: execution.error.clone().unwrap_or_default(),
                timestamp: Local::now(),
            },
            ExecutionState::Cancelled => WorkflowEvent::Cancelled {
                execution_id: execution.id.clone(),
                workflow_name: execution.workflow_name.clone(),
                timestamp: Local::now(),
            },
            _ => return,
        };
        self.event_manager.emit(event).await;
    }

    pub async fn cancel_execution(&self, id: &str) -> Result<Execution, EngineError> {
        let mut execs = self.executions.write().await;
        let execution = execs
            .get_mut(id)
            .ok_or_else(|| EngineError::ExecutionNotFound(id.to_string()))?;

        if execution.state != ExecutionState::Running && execution.state != ExecutionState::Waiting
        {
            return Err(EngineError::InvalidState(format!(
                "execution {} is not running or waiting (state={})",
                id, execution.state
            )));
        }

        execution.state = ExecutionState::Cancelled;
        execution.ended_at = Some(Local::now());

        // Fire the cancellation token while still holding the state lock
        // (a): the run/resume side inserts its token under
        // the same lock, so there is no window where we flip the state but
        // miss a token that appears right after — either the scheduler sees
        // the fired token, or we observed the pre-run state and the run side
        // bails out on its own state check.
        //
        // BUG-20261114-035 终态事件 exactly-once 判别器：token 在场 =
        // run_async/resume 在飞，取消会经调度器被它们的收口尾段 settle
        // 并发终态事件——本方法不得重复发；token 缺席 = Waiting 挂起
        // （run_async/resume 收口前已摘 token）或 run 尚未起飞
        // （create_execution 与 run_async 之间的窗口），无人再收口 →
        // 本方法负责发 Cancelled 事件（见尾部）。
        let had_inflight_token = if let Some(entry) = self.cancel_tokens.get(id) {
            entry.cancel();
            true
        } else {
            false
        };

        let updated = execution.clone();
        drop(execs); // Release lock before persistence I/O

        self.persist_execution(&updated).await;

        // b: flip the latest checkpoint to terminal so a
        // cancelled execution is not resurrected by
        // restore_incomplete_executions on the next process restart (that
        // path only skips `terminal || all_completed` checkpoints). Cancelling
        // a Waiting execution returns without any terminal checkpoint of its
        // own, leaving the most recent checkpoint at the non-terminal Waiting
        // snapshot — startup restore would then bring the cancelled execution
        // back as Waiting / resumable. run_async covers the mid-run cancel
        // itself (terminal checkpoint in its final-state match), so this is
        // the missing Waiting-cancel arm. Best-effort with loud warn on
        // failure; never masks the cancel result.
        let wf_ctx = {
            let ctx = WorkflowContext::new(updated.input.clone());
            for (k, v) in &updated.variables {
                ctx.set_var(k, v.clone());
            }
            for (nid, nr) in &updated.node_results {
                ctx.set_node_result(nid, nr.clone());
            }
            ctx
        };
        if let Err(e) = self
            .save_checkpoint(id, &updated.workflow_name, &wf_ctx, None, None, true)
            .await
        {
            warn!(
                target: "nemesis_workflow::engine",
                execution_id = %id,
                error = %e,
                "failed to save terminal checkpoint on cancel (stale checkpoint may resurrect this execution on restart)"
            );
        }

        // BUG-20261114-035：token 缺席臂在此补发 Cancelled 事件——
        // Waiting 取消此前不发任何事件（run_async 已返回、无人再收口），
        // 唯一生产观察者 WorkflowChatReplyObserver 等不到 Cancelled，
        // workflow_chat 的 per-workflow 互斥守卫永久滞留。token 在场臂
        // 不发（run/resume 收口尾段负责）——exactly-once。
        if !had_inflight_token {
            // BUG-20261114-036：账本收口走同一判别器。Waiting 取消此前
            // 不收账——重启 sweep 只圈 cron_job（ledger.rs
            // sweep_restart_orphans_on），resume 又因 state != Waiting 被拒，
            // workflow_run:{id} 行 + attempt 永久停在 running（cancel_all_
            // running / estop 批量取消放大泄漏面）。token 在场臂由 run/resume
            // 收口尾段 settle（同一 exactly-once 契约，不重复收）；run 尚未
            // 起飞的窗口行还不存在，settle 落空 = 无害 no-op。先收账后发
            // 事件，与 run_async 尾段的账本→事件顺序一致。
            nemesis_taskstore::ledger::settle_run(
                &nemesis_taskstore::ledger::workflow_run_id(id),
                "cancelled",
                None,
            );
            self.emit_terminal_event(&updated).await;
        }

        Ok(updated)
    }

    /// Cancel every Running/Waiting execution in one shot（estop 联动入口，
    /// A 批批 4：gateway estop watcher 消费）。逐个走 [`cancel_execution`]，
    /// 已终态的执行保持不动（per-id 状态检查照常生效）。返回实际取消的
    /// 执行 id 列表。
    pub async fn cancel_all_running(&self) -> Vec<String> {
        let ids: Vec<String> = {
            let execs = self.executions.read().await;
            execs
                .iter()
                .filter(|(_, e)| {
                    matches!(e.state, ExecutionState::Running | ExecutionState::Waiting)
                })
                .map(|(id, _)| id.clone())
                .collect()
        };
        let mut cancelled = Vec::with_capacity(ids.len());
        for id in &ids {
            if self.cancel_execution(id).await.is_ok() {
                cancelled.push(id.clone());
            }
        }
        cancelled
    }

    /// 设置墙钟保险丝秒数（0 = 关）。引擎默认 0；gateway 装配期从 config
    /// `workflow.wall_clock_budget_secs` 读（缺省 3600）。
    pub fn set_wall_clock_budget_secs(&self, secs: u64) {
        self.wall_clock_budget_secs
            .store(secs, std::sync::atomic::Ordering::Relaxed);
    }

    /// Resume a waiting execution (e.g., after human review).
    ///
    /// Updates the waiting node's result with the reviewer's response, then
    /// continues executing the rest of the workflow via `schedule_resume`.
    /// Any nodes that already ran (everything in `node_results` with state
    /// `Completed`) are skipped; only nodes downstream of the waiting node
    /// actually re-execute.
    ///
    /// If a checkpoint store is configured (1b-), the new state is saved
    /// to a fresh checkpoint so a crash mid-resume still recovers.
    ///
    /// On success, returns the updated execution. The execution's terminal
    /// state is `Completed` unless another `human_review` node paused it
    /// again (in which case it's `Waiting`).
    pub async fn resume_execution(
        &self,
        id: &str,
        review_result: HashMap<String, serde_json::Value>,
    ) -> Result<Execution, EngineError> {
        self.resume_execution_node(id, review_result, None).await
    }

    /// 节点定向 resume。
    ///
    /// `target_node = Some(id)` 把评审结果落到指定等待节点；多个 human_review
    /// 同层并行时（同一拓扑层并发执行、双双 Waiting），无定向的 resume 无法
    /// 表达用户评审的是哪一个——旧实现按 HashMap 迭代序（进程内随机）挑一
    /// 个落地，人工决策可能落到用户从未见过的节点上（静默错配）。`None` 时
    /// 仅当恰好一个等待节点才落地，多于一个 loud 拒绝并点名候选集合。
    pub async fn resume_execution_node(
        &self,
        id: &str,
        review_result: HashMap<String, serde_json::Value>,
        target_node: Option<&str>,
    ) -> Result<Execution, EngineError> {
        // Snapshot the execution under the write lock, then release the lock
        // for the duration of the (potentially long) scheduler call.
        let execution_snapshot = {
            let mut execs = self.executions.write().await;
            let execution = execs
                .get_mut(id)
                .ok_or_else(|| EngineError::ExecutionNotFound(id.to_string()))?;

            if execution.state != ExecutionState::Waiting {
                return Err(EngineError::InvalidState(format!(
                    "execution {} is not waiting (state={})",
                    id, execution.state
                )));
            }

            // 候选按 id 排序保证确定性；target 必须命中
            // 等待集合（已解析/未执行的节点 loud 拒绝），无定向且候选多于
            // 一个时 loud 拒绝——绝不随机选边。
            let mut waiting_ids: Vec<String> = execution
                .node_results
                .iter()
                .filter(|(_, r)| r.state == ExecutionState::Waiting)
                .map(|(nid, _)| nid.clone())
                .collect();
            waiting_ids.sort();
            let waiting_id = match target_node {
                Some(target) => {
                    if !waiting_ids.iter().any(|nid| nid == target) {
                        return Err(EngineError::InvalidState(format!(
                            "节点 {:?} 不在本执行等待集合 {:?} 中（未执行或已解析）",
                            target, waiting_ids
                        )));
                    }
                    target.to_string()
                }
                None if waiting_ids.is_empty() => {
                    return Err(EngineError::InvalidState(format!(
                        "execution {} has no node in waiting state",
                        id
                    )));
                }
                None if waiting_ids.len() > 1 => {
                    return Err(EngineError::InvalidState(format!(
                        "execution {} 有 {} 个等待节点 {:?}：无法确定本次评审落点，请在请求中带 node_id 指定目标节点",
                        id,
                        waiting_ids.len(),
                        waiting_ids
                    )));
                }
                None => waiting_ids[0].clone(),
            };
            // NN：结果覆写移出临界区——sub_workflow 等待节点的
            // 评审要路由进子执行（需读原结果的 metadata.execution_id），错误
            // 臂也要能把执行干净恢复回 Waiting（覆写过的结果无从恢复）。
            if let Some(approved) = review_result.get("approved")
                && let Some(b) = approved.as_bool()
            {
                debug!(
                    "[Workflow] Node {} review result: approved={}",
                    waiting_id, b
                );
            }

            // Mark as Running while schedule_resume executes. Arm the fresh
            // cancellation token under the same lock (a): a
            // cancel_execution landing after the lock release must find the
            // token to fire — the old outside-the-lock insert let cancel slip
            // into the gap, flip the state to Cancelled, and the resume then
            // ran the downstream nodes to completion, overwriting the cancel.
            execution.state = ExecutionState::Running;
            execution.ended_at = None;
            let cancel_token = CancellationToken::new();
            self.cancel_tokens
                .insert(id.to_string(), cancel_token.clone());
            let snap = execution.clone();
            (snap, waiting_id, cancel_token)
        };

        let (mut execution, waiting_node_id, cancel_token) = execution_snapshot;

        // Load the workflow definition (needed for schedule_resume). The
        // token was already armed inside the critical section — a failed
        // lookup must not leave it orphaned.
        let workflow = match self.get_workflow(&execution.workflow_name) {
            Some(w) => w,
            None => {
                self.cancel_tokens.remove(id);
                // NN：状态翻 Running 发生在临界区，查找失败
                // 必须翻回去——否则执行卡在 Running 且无 token，进程内
                // 永远无法再 resume（重启才被 checkpoint 治愈）。
                self.restore_waiting_after_failed_resume(id).await;
                return Err(EngineError::WorkflowNotFound(
                    execution.workflow_name.clone(),
                ));
            }
        };

        // Optional config-drift warning.
        if let Some(ref stored_hash) = execution.workflow_hash {
            let current_hash = workflow.hash();
            if stored_hash != &current_hash {
                warn!(
                    target: "nemesis_workflow::engine",
                    execution_id = %id,
                    workflow = %execution.workflow_name,
                    "config drift detected: checkpoint hash {} != current {}",
                    stored_hash, current_hash
                );
            }
        }

        // NN：sub_workflow 等待节点的评审路由。父执行停在
        // sub_workflow 节点 = 子执行停在它自己的 human_review——评审必须
        // 路由进子执行再按子执行终态同步父节点结果；旧实现把评审 JSON
        // 直接覆写进父节点结果并放行父下游（子执行孤儿化、子下游永不
        // 执行、父下游拿到覆写数据——探针实测三连）。运行期 Waiting 传播
        // 语义不变（integration_nesting 钉住的既有设计），只修 resume 侧。
        let waiting_node_type = workflow
            .nodes
            .iter()
            .find(|n| n.id == waiting_node_id)
            .map(|n| n.node_type.as_str())
            .unwrap_or("");
        execution = if waiting_node_type == "sub_workflow" {
            let exec = self
                .resume_subworkflow_node(execution, &waiting_node_id, review_result, &cancel_token)
                .await?;
            // 早期收口臂（Waiting/Failed/Cancelled）已在 settle_resume_outcome
            // 完成账本/checkpoint/persist——直接返回，绝不落回下方尾段：
            // schedule_resume 会把非 Completed 且依赖已满足的 sub_workflow
            // 节点整个重跑（再 spawn 一个新子执行），Failed 被洗成 Waiting、
            // 路由锚被换新。只有 Completed 臂（state 仍为临界区的 Running）
            // 续走尾段 schedule_resume 跑父下游。
            if exec.state != ExecutionState::Running {
                return Ok(exec);
            }
            exec
        } else {
            // 常规节点：评审结果覆写进等待节点结果（既有语义）。
            let result = execution
                .node_results
                .get_mut(&waiting_node_id)
                .expect("waiting_id 来自同一 map，必然存在");
            result.output = serde_json::json!(review_result);
            result.state = ExecutionState::Completed;
            result.ended_at = Local::now();
            execution
        };

        // Build context from the current execution state (carries the
        // just-resolved review_result through to downstream nodes).
        // BUG-20261112-115：identity 必须与 fresh run 同源重建——旧实现
        // `WorkflowContext::new` 把 WorkflowIdentity 落全空默认值，resume 后
        // 下游 tool 节点经 SecureExecutor 透传的 RequestContext 三键
        // （session_key/chat_id/sender_id）全空：审批卡无法回源触发会话、
        // 会话级豁免按空 session_key 永不命中、归因 metadata（wf_instance/
        // wf_trigger/wf_depth）清空。trigger_source 就在同一 execution 上
        // （checkpoint Gap 1 已保证重启恢复带回），recursion_depth 与
        // fresh run 同款确定性推导（depth_from_trigger）。
        let (trig_session, trig_chat, trig_sender) = execution
            .trigger_source
            .as_ref()
            .map(|t| t.identity_triple())
            .unwrap_or_default();
        let wf_identity = crate::context::WorkflowIdentity {
            execution_id: execution.id.clone(),
            workflow_name: execution.workflow_name.clone(),
            trigger_label: execution
                .trigger_source
                .as_ref()
                .map(|t| t.label())
                .unwrap_or("unknown")
                .to_string(),
            session_key: trig_session,
            chat_id: trig_chat,
            sender_id: trig_sender,
            recursion_depth: crate::call_stack::CallFrame::depth_from_trigger(
                &execution.trigger_source,
            ),
        };
        let mut wf_ctx = WorkflowContext::new_with_identity(wf_identity, execution.input.clone());
        for (k, v) in &execution.variables {
            wf_ctx.set_var(k, v.clone());
        }
        for (id, nr) in &execution.node_results {
            wf_ctx.set_node_result(id, nr.clone());
        }

        // Nodes already in `Completed` state must not re-run.
        let completed_nodes: HashSet<String> = execution
            .node_results
            .iter()
            .filter(|(_, r)| r.state == ExecutionState::Completed)
            .map(|(id, _)| id.clone())
            .collect();

        // 墙钟保险丝——resume 段同款（批 4）：恢复后再次挂死同样要被兜住；
        // 按调度段计时（human_review 的合法等待不占预算——罚的不是工作流的
        // 锅）。语义与初始 run 一致：超时取消 → Failed + 原因注记。
        let wall_clock_watchdog = {
            let budget = self
                .wall_clock_budget_secs
                .load(std::sync::atomic::Ordering::Relaxed);
            if budget > 0 {
                let token = cancel_token.clone();
                let execution_id = execution.id.clone();
                let timed_out = Arc::new(std::sync::atomic::AtomicBool::new(false));
                let flag = timed_out.clone();
                Some((
                    tokio::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_secs(budget)).await;
                        flag.store(true, std::sync::atomic::Ordering::Relaxed);
                        warn!(
                            target: "nemesis_workflow::engine",
                            execution_id = %execution_id,
                            budget_secs = budget,
                            "wall clock budget exceeded during resume — cancelling execution"
                        );
                        token.cancel();
                    }),
                    timed_out,
                    budget,
                ))
            } else {
                None
            }
        };

        let schedule_result = scheduler::schedule_resume(
            &workflow.nodes,
            &workflow.edges,
            &self.node_executors,
            &mut wf_ctx,
            &completed_nodes,
            cancel_token.clone(),
        )
        .await;

        self.cancel_tokens.remove(&execution.id);

        if let Some((handle, _, _)) = &wall_clock_watchdog {
            handle.abort();
        }
        let wall_clock_timed_out = match &wall_clock_watchdog {
            Some((_, flag, _)) => flag.load(std::sync::atomic::Ordering::Relaxed),
            None => false,
        };

        let now = Local::now();
        match schedule_result {
            Ok(ScheduleOutcome::Cancelled) => {
                // 看门狗触发的取消 → Failed + 原因注记；用户主动取消
                // （cancel_execution 路径）保持 Cancelled。
                if wall_clock_timed_out {
                    let budget = match &wall_clock_watchdog {
                        Some((_, _, b)) => *b,
                        None => 0,
                    };
                    execution.state = ExecutionState::Failed;
                    execution.error = Some(format!(
                        "wall clock budget exceeded ({}s) — cancelled by watchdog during resume",
                        budget
                    ));
                    execution.ended_at = Some(now);
                } else {
                    execution.state = ExecutionState::Cancelled;
                    execution.ended_at = Some(now);
                }
            }
            Ok(ScheduleOutcome::Completed) => {
                // Detect whether a fresh `human_review` paused us again.
                let still_waiting = wf_ctx
                    .get_all_node_results()
                    .values()
                    .any(|r| r.state == ExecutionState::Waiting);
                execution.state = if still_waiting {
                    ExecutionState::Waiting
                } else {
                    ExecutionState::Completed
                };
                if !still_waiting {
                    execution.ended_at = Some(now);
                } else {
                    execution.ended_at = None;
                }
            }
            Err(err) => {
                execution.state = ExecutionState::Failed;
                execution.error = Some(err);
                execution.ended_at = Some(now);
            }
        }

        // resume 路径账本收口（首跑 run_async 的开账行凭确定性 id
        // 直解；再次 Waiting 保持开账等下一次 resume）。
        let ledger_run_id = nemesis_taskstore::ledger::workflow_run_id(&execution.id);
        match execution.state {
            ExecutionState::Completed => {
                nemesis_taskstore::ledger::settle_run(&ledger_run_id, "succeeded", None);
            }
            ExecutionState::Failed => {
                let err = execution.error.as_deref();
                nemesis_taskstore::ledger::settle_run(&ledger_run_id, "failed", err);
            }
            ExecutionState::Cancelled => {
                nemesis_taskstore::ledger::settle_run(&ledger_run_id, "cancelled", None);
            }
            _ => {}
        }

        // Copy fresh node_results + variables back into the execution.
        execution.node_results = wf_ctx.get_all_node_results();
        execution.variables = wf_ctx.get_all_variables();

        // Save a new checkpoint reflecting post-resume state. Always save —
        // for Waiting/Running this captures the in-flight state so future
        // restarts can resume; for Completed/Failed/Cancelled the terminal
        // flag tells `restore_incomplete_executions` to skip this execution
        // next startup. (Gap 2 fix: previously terminal states were skipped,
        // so the previous in-flight checkpoint would resurrect the execution
        // on the next process restart.)
        let is_terminal = matches!(
            execution.state,
            ExecutionState::Completed | ExecutionState::Failed | ExecutionState::Cancelled
        );
        let waiting: Option<String> = if execution.state == ExecutionState::Waiting {
            smallest_waiting_node_id(&wf_ctx.get_all_node_results())
        } else {
            None
        };
        if let Err(e) = self
            .save_checkpoint(
                &execution.id,
                &execution.workflow_name,
                &wf_ctx,
                waiting.as_deref(),
                None,
                is_terminal,
            )
            .await
        {
            warn!(
                target: "nemesis_workflow::engine",
                execution_id = %id,
                error = %e,
                "failed to save post-resume checkpoint"
            );
        }

        // Persist + update in-memory state.
        self.persist_execution(&execution).await;
        {
            let mut execs = self.executions.write().await;
            execs.insert(execution.id.clone(), execution.clone());
        }

        // BUG-20261114-035：resume 收口与 run_async 首跑同契约发终态
        // 事件。此前缺失 = Waiting 恢复后的完成/失败/取消对观察者不可见
        // （chat 守卫泄漏）；再次 Waiting 本方法不发声（非终态，守卫留
        // 待下一次 resume）。
        self.emit_terminal_event(&execution).await;

        Ok(execution)
    }

    /// NN：sub_workflow 等待节点的评审路由（resume 侧）。
    ///
    /// 父执行的 sub_workflow 节点停在 Waiting = 子执行停在它自己的
    /// human_review。评审路由进子执行（递归 [`resume_execution_node`]），
    /// 再按子执行终态同步父节点结果：
    /// - 子执行 Completed → 父节点结果覆写为子工作流真实输出投影
    ///   （与 [`SubWorkflowNodeExecutor`] 产出同形），返回后公共尾段
    ///   续跑父下游；
    /// - 子执行再次 Waiting（嵌套审批）→ 父节点结果刷新为子工作流当前
    ///   快照，父保持 Waiting 等下一次 resume（非终态 checkpoint、
    ///   账本保持开账）；
    /// - 子执行 Failed/Cancelled → 父诚实失败（错误点名子执行）。
    ///
    /// 用户取消与子执行结果竞态时取消优先（与调度器收臂纪律一致：
    /// 失败尚未记入父执行前，用户取消赢）。
    async fn resume_subworkflow_node(
        &self,
        mut execution: Execution,
        waiting_node_id: &str,
        review_result: HashMap<String, serde_json::Value>,
        cancel_token: &CancellationToken,
    ) -> Result<Execution, EngineError> {
        let child_id = execution
            .node_results
            .get(waiting_node_id)
            .and_then(|r| r.metadata.get("execution_id"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let Some(child_id) = child_id else {
            self.cancel_tokens.remove(&execution.id);
            self.restore_waiting_after_failed_resume(&execution.id)
                .await;
            return Err(EngineError::InvalidState(format!(
                "sub_workflow 节点 {:?} 的等待结果缺少子执行 id（metadata.execution_id）——\
                 无法路由评审；请对实际子执行直接 workflow.resume，或取消本执行重跑",
                waiting_node_id
            )));
        };

        // 子执行形态裁决：Waiting 才需要路由评审；已是终态（用户绕过父
        // 执行直接 resume 过子执行）则按现状同步父节点结果。
        let child = match self.get_execution(&child_id).await {
            Some(c) => c,
            None => {
                self.cancel_tokens.remove(&execution.id);
                self.restore_waiting_after_failed_resume(&execution.id)
                    .await;
                return Err(EngineError::ExecutionNotFound(format!(
                    "子执行 {} 不存在（无法路由 sub_workflow 评审）",
                    child_id
                )));
            }
        };
        let child = if child.state == ExecutionState::Waiting {
            // 递归（resume_execution_node → 本方法 → resume_execution_node）
            // 必须 Box::pin 打断未来尺寸的无限递归；嵌套深度由工作流自身
            // 结构有界（每层一层 human_review）。
            match Box::pin(self.resume_execution_node(&child_id, review_result, None)).await {
                Ok(c) => c,
                Err(e) => {
                    self.cancel_tokens.remove(&execution.id);
                    self.restore_waiting_after_failed_resume(&execution.id)
                        .await;
                    return Err(EngineError::InvalidState(format!(
                        "子执行 {} 评审路由失败：{}（可先直接对子执行带 node_id resume，\
                         再 resume 本执行）",
                        child_id, e
                    )));
                }
            }
        } else {
            child
        };

        // 用户取消与子执行结果竞态：取消优先。
        if cancel_token.is_cancelled() {
            self.cancel_tokens.remove(&execution.id);
            execution.state = ExecutionState::Cancelled;
            execution.ended_at = Some(Local::now());
            return Ok(self.settle_resume_outcome(execution, None).await);
        }

        // 子执行 node_results 投影（与执行器产出同形，父下游
        // `{{call.<child_node>.<field>}}` 拿到真实输出）。
        let mut output_map = serde_json::Map::new();
        for (nid, nr) in &child.node_results {
            output_map.insert(nid.clone(), nr.output.clone());
        }
        let child_outputs = serde_json::Value::Object(output_map);

        match child.state {
            ExecutionState::Waiting => {
                // 嵌套审批：父节点结果刷新为子工作流当前快照；state 保持
                // Waiting、metadata.execution_id 保持原值——下一次 resume
                // 继续路由同一子执行。
                let result = execution
                    .node_results
                    .get_mut(waiting_node_id)
                    .expect("waiting_id 来自同一 map，必然存在");
                result.output = child_outputs;
                self.cancel_tokens.remove(&execution.id);
                execution.state = ExecutionState::Waiting;
                execution.ended_at = None;
                let waiting = smallest_waiting_node_id(&execution.node_results);
                return Ok(self.settle_resume_outcome(execution, waiting).await);
            }
            ExecutionState::Completed => {
                let result = execution
                    .node_results
                    .get_mut(waiting_node_id)
                    .expect("waiting_id 来自同一 map，必然存在");
                result.output = child_outputs;
                result.state = ExecutionState::Completed;
                result.error = None;
                result.ended_at = Local::now();
                // 返回后公共尾段：wf_ctx 重建 + schedule_resume 续跑父下游。
            }
            ExecutionState::Failed | ExecutionState::Cancelled => {
                let msg = format!(
                    "子工作流执行 {} 终态 {}：{}",
                    child_id,
                    child.state,
                    child.error.as_deref().unwrap_or("(无错误信息)")
                );
                let result = execution
                    .node_results
                    .get_mut(waiting_node_id)
                    .expect("waiting_id 来自同一 map，必然存在");
                result.output = child_outputs;
                result.state = ExecutionState::Failed;
                result.error = Some(msg.clone());
                result.ended_at = Local::now();
                self.cancel_tokens.remove(&execution.id);
                execution.state = ExecutionState::Failed;
                execution.error = Some(msg);
                execution.ended_at = Some(Local::now());
                return Ok(self.settle_resume_outcome(execution, None).await);
            }
            ExecutionState::Pending | ExecutionState::Running => {
                // 不可达（resume 只会返回终态或 Waiting），防御臂。
                self.cancel_tokens.remove(&execution.id);
                self.restore_waiting_after_failed_resume(&execution.id)
                    .await;
                return Err(EngineError::InvalidState(format!(
                    "子执行 {} resume 后处于非终态 {:?}——父执行恢复 Waiting",
                    child_id, child.state
                )));
            }
        }

        Ok(execution)
    }

    /// NN：resume 错误臂统一恢复——状态翻 Running 发生在
    /// 临界区，任何错误返回前必须翻回 Waiting（旧实现 workflow 查找
    /// 失败会留下 state=Running + 无 token 的卡死形态，进程内永远无法
    /// 再 resume，重启才被 checkpoint 治愈）。
    ///
    /// BUG-20261114-008：翻回的前提必须是「仍是我们自己翻的 Running」。
    /// 错误窗口内并发 cancel_execution 可合法观察到 Running 并翻成
    /// Cancelled（持久化 + 终态 checkpoint）——最宽窗口 = sub_workflow 子
    /// 执行 resume 全程（分钟级，resume_subworkflow_node 的子评审臂）；
    /// 无条件覆写会把已取消的执行在内存复活成 Waiting：此后 resume 照常
    /// 放行（取消意图被静默抹除）、estop cancel_all_running 的取消同样
    /// 被复活，且内存 Waiting 与盘上 Cancelled 分叉。只在 Running 时翻回，
    /// 其余状态（并发 cancel 已落、未来新状态）一律不动。
    async fn restore_waiting_after_failed_resume(&self, id: &str) {
        let mut execs = self.executions.write().await;
        if let Some(execution) = execs.get_mut(id)
            && execution.state == ExecutionState::Running
        {
            execution.state = ExecutionState::Waiting;
            execution.ended_at = None;
        }
    }

    /// resume 早期收口（不经 schedule_resume 的臂）： 账本 + checkpoint +
    /// persist + 内存回写。`waiting_node` 仅在父执行保持 Waiting 时传
    /// （非终态 checkpoint 的 waiting_node 字段）；终态臂传 None。
    async fn settle_resume_outcome(
        &self,
        execution: Execution,
        waiting_node: Option<String>,
    ) -> Execution {
        // resume 路径账本收口；再次 Waiting 保持开账等下一次 resume。
        let ledger_run_id = nemesis_taskstore::ledger::workflow_run_id(&execution.id);
        match execution.state {
            ExecutionState::Completed => {
                nemesis_taskstore::ledger::settle_run(&ledger_run_id, "succeeded", None);
            }
            ExecutionState::Failed => {
                let err = execution.error.as_deref();
                nemesis_taskstore::ledger::settle_run(&ledger_run_id, "failed", err);
            }
            ExecutionState::Cancelled => {
                nemesis_taskstore::ledger::settle_run(&ledger_run_id, "cancelled", None);
            }
            _ => {}
        }

        let is_terminal = !matches!(execution.state, ExecutionState::Waiting);
        let wf_ctx = {
            let ctx = WorkflowContext::new(execution.input.clone());
            for (k, v) in &execution.variables {
                ctx.set_var(k, v.clone());
            }
            for (nid, nr) in &execution.node_results {
                ctx.set_node_result(nid, nr.clone());
            }
            ctx
        };
        if let Err(e) = self
            .save_checkpoint(
                &execution.id,
                &execution.workflow_name,
                &wf_ctx,
                waiting_node.as_deref(),
                None,
                is_terminal,
            )
            .await
        {
            warn!(
                target: "nemesis_workflow::engine",
                execution_id = %execution.id,
                error = %e,
                "failed to save checkpoint on early resume settle"
            );
        }
        self.persist_execution(&execution).await;
        {
            let mut execs = self.executions.write().await;
            execs.insert(execution.id.clone(), execution.clone());
        }
        // BUG-20261114-035：早期收口臂（sub_workflow 子终态同步 /
        // 取消竞态）同样发终态事件——观察者契约与公共尾段一致；
        // Waiting 臂（嵌套审批）不发声。
        self.emit_terminal_event(&execution).await;
        execution
    }

    // -----------------------------------------------------------------------
    // Lifecycle
    // -----------------------------------------------------------------------

    /// Close the engine and clean up resources.
    ///
    /// Marks the engine as closed. Future calls to `run` or `start_execution`
    /// will return an error. In-progress executions are not automatically
    /// cancelled (they will complete naturally).
    pub async fn close(&self) {
        info!("[Workflow] Closing workflow engine");
        let mut closed = self.closed.write().await;
        *closed = true;

        // Cancel all in-flight executions so scheduler + node executors exit.
        for entry in self.cancel_tokens.iter() {
            entry.cancel();
        }
        self.cancel_tokens.clear();
    }

    /// Check whether the engine is closed.
    pub async fn is_closed(&self) -> bool {
        *self.closed.read().await
    }

    // -----------------------------------------------------------------------
    // Persistence Helpers
    // -----------------------------------------------------------------------

    /// Persist an execution to disk if persistence is enabled.
    ///
    /// Errors are logged but not propagated -- persistence is best-effort.
    async fn persist_execution(&self, execution: &Execution) {
        if let Some(ref dir) = self.persistence_dir {
            // BUG-20261112-103：文件名必须 sanitize。`workflow_name` 来自用户
            // YAML（validate 只查非空、无字符集约束）——原样拼接有两条实害：
            // ① `..`/路径分隔符拼出逃逸路径，且写盘守卫（guard_engine_write）
            // 只在执行体分离开启时装配（gateway/CLI 默认关 = 无守卫），
            // `../../evil` 真落 persistence_dir 之外；② Windows 非法字符
            // （`:` 等）让 OpenOptions 恒失败——该 workflow 的执行历史每次
            // 状态转换都 warn 后丢失。与 definitions 落盘同源
            // （sanitize_workflow_filename 已是 persist_workflow/
            // delete_workflow_file 的既有纪律）。读侧不受影响：
            // load_execution_from_disk / sweep_stale_execution_files 都按
            // 文件内容里的 execution id 匹配，不按文件名。
            let file_path = dir.join(format!(
                "{}_{}.jsonl",
                sanitize_workflow_filename(&execution.workflow_name),
                execution.id
            ));
            // 控制面写守卫：`workflow_name` 来自用户 YAML（此处**未**
            // sanitize——`../evil` 能拼出逃逸路径），守卫是这条链的唯一
            // 拦截层。persist 是 best-effort：拒 = warn + 跳过，不 fail 执行。
            if let Err(e) = self.guard_engine_write(&file_path) {
                warn!(
                    "[Workflow] Refused to persist execution {} outside writable roots: {}",
                    execution.id, e
                );
                return;
            }
            let persistence = WorkflowPersistence::new(&file_path);
            if let Err(e) = persistence.save_execution(execution) {
                warn!(
                    "[Workflow] Failed to persist execution {}: {}",
                    execution.id, e
                );
            } else {
                debug!("[Workflow] Persisted execution {}", execution.id);
            }
        }
    }

    /// Load an execution from disk by ID.
    ///
    /// Searches all JSONL files in the persistence directory for the given ID.
    async fn load_execution_from_disk(&self, id: &str) -> Option<Execution> {
        let dir = self.persistence_dir.as_ref()?;

        // Scan all JSONL files in the directory for the matching execution ID
        let entries = std::fs::read_dir(dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                let persistence = WorkflowPersistence::new(&path);
                if let Ok(execution) = persistence.load_execution(id) {
                    return Some(execution);
                }
            }
        }

        None
    }
}

impl Default for WorkflowEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// 同族统一定义：收集一个 workflow 定义的全部
/// sub_workflow 引用目标（顶层节点 + parallel/loop 复合子节点递归——
/// composite children 运行时经 registry 同样分派到 SubWorkflowNodeExecutor
/// → engine.run，逃过收集就是逃过环闸）。空目标名跳过（执行器同样拒绝空名）。
fn workflow_subworkflow_targets(wf: &Workflow) -> Vec<String> {
    let mut refs = Vec::new();
    for n in &wf.nodes {
        if n.node_type == "sub_workflow" {
            if let Some(target) = n.config.get("workflow").and_then(|v| v.as_str())
                && !target.is_empty()
            {
                refs.push(target.to_string());
            }
        } else if matches!(n.node_type.as_str(), "parallel" | "loop") {
            let cfg: serde_json::Map<String, serde_json::Value> =
                n.config.clone().into_iter().collect();
            collect_composite_subworkflow_targets(&cfg, &mut refs);
        }
    }
    refs
}

/// 递归收集复合节点 config 里 children（`nodes`/`branches` 数组，形态同
/// `get_config_node_list`）的 sub_workflow 引用目标；嵌套 composite 继续下钻。
fn collect_composite_subworkflow_targets(
    config: &serde_json::Map<String, serde_json::Value>,
    refs: &mut Vec<String>,
) {
    for key in &["nodes", "branches"] {
        let Some(arr) = config.get(*key).and_then(|v| v.as_array()) else {
            continue;
        };
        for child in arr {
            let Some(obj) = child.as_object() else {
                continue;
            };
            let child_type = obj
                .get("node_type")
                .or_else(|| obj.get("type"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if child_type == "sub_workflow"
                && let Some(target) = obj
                    .get("config")
                    .and_then(|c| c.get("workflow"))
                    .and_then(|v| v.as_str())
                && !target.is_empty()
            {
                refs.push(target.to_string());
            }
            if matches!(child_type, "parallel" | "loop")
                && let Some(child_config) = obj.get("config").and_then(|c| c.as_object())
            {
                collect_composite_subworkflow_targets(child_config, refs);
            }
        }
    }
}

/// DFS：从 `start` 沿引用图出发，找回到 `start` 的路径（含 start 自环）。
/// 返回 `[start, …, start]` 完整环路径；无环 None。visited 按节点去重
/// （多出边 DAG 有限展开）。
fn find_path_back_to_start(
    start: &str,
    graph: &HashMap<String, Vec<String>>,
) -> Option<Vec<String>> {
    fn dfs(
        node: &str,
        start: &str,
        graph: &HashMap<String, Vec<String>>,
        path: &mut Vec<String>,
        visited: &mut std::collections::HashSet<String>,
    ) -> Option<Vec<String>> {
        let targets = graph.get(node)?;
        for next in targets {
            if next == start {
                let mut cycle = path.clone();
                cycle.push(next.clone());
                return Some(cycle);
            }
            if visited.insert(next.clone()) {
                path.push(next.clone());
                if let Some(c) = dfs(next, start, graph, path, visited) {
                    return Some(c);
                }
                path.pop();
            }
        }
        None
    }
    let mut path = vec![start.to_string()];
    let mut visited = std::collections::HashSet::new();
    visited.insert(start.to_string());
    dfs(start, start, graph, &mut path, &mut visited)
}

/// 从事件 data / `TriggerSource::Event.data` 里读出触发链
/// （[`crate::event_dispatcher:TRIGGER_CHAIN_KEY`] → JSON 字符串数组）。
/// 缺键/形态不对一律按空链处理——链只由引擎发布点写入，读到脏形态放行
/// （宁可少拦不误拦；手动 fire_event 本就没有链键）。
fn trigger_chain_from_value(value: Option<&serde_json::Value>) -> Vec<String> {
    value
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// 本执行的来源触发链：只有 `TriggerSource::Event` 携带上代事件的 data
/// （链就藏在其中，见 [`trigger_chain_from_value`]）；其余触发来源没有
/// 前代链，从空链起算。
fn event_trigger_chain_of(trigger: Option<&TriggerSource>) -> Vec<String> {
    match trigger {
        Some(TriggerSource::Event { data, .. }) => {
            trigger_chain_from_value(data.get(crate::event_dispatcher::TRIGGER_CHAIN_KEY))
        }
        _ => Vec::new(),
    }
}

/// 同族统一定义：多等待节点并存时取 id 最小者作为
/// 「当前等待节点」（checkpoint 的 waiting_node 可观测性字段）——HashMap
/// 迭代序随机会让重启前后漂移，确定性优先。run 与 resume 两处 checkpoint
/// 收口共用。
fn smallest_waiting_node_id(results: &HashMap<String, NodeResult>) -> Option<String> {
    let mut ids: Vec<String> = results
        .iter()
        .filter(|(_, r)| r.state == ExecutionState::Waiting)
        .map(|(id, _)| id.clone())
        .collect();
    ids.sort();
    ids.into_iter().next()
}

/// Validate that the node dependency graph has no cycles using Kahn's algorithm.
fn validate_dag(nodes: &[NodeDef]) -> Result<(), EngineError> {
    if nodes.is_empty() {
        return Ok(());
    }

    let node_ids: HashSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();

    // Build in-degree map.
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();

    for node in nodes {
        in_degree.entry(node.id.as_str()).or_insert(0);
        for dep in &node.depends_on {
            if !node_ids.contains(dep.as_str()) {
                // Referenced dependency does not exist; treat as already satisfied.
                continue;
            }
            *in_degree.entry(node.id.as_str()).or_insert(0) += 1;
            dependents
                .entry(dep.as_str())
                .or_default()
                .push(node.id.as_str());
        }
    }

    let mut queue: VecDeque<&str> = in_degree
        .iter()
        .filter(|(_, deg)| **deg == 0)
        .map(|(&id, _)| id)
        .collect();

    let mut visited = 0;
    while let Some(id) = queue.pop_front() {
        visited += 1;
        if let Some(deps) = dependents.get(id) {
            for &dep_id in deps {
                if let Some(deg) = in_degree.get_mut(dep_id) {
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push_back(dep_id);
                    }
                }
            }
        }
    }

    if visited < nodes.len() {
        // Some nodes were never visited -- cycle exists.
        return Err(EngineError::CycleDetected(
            "circular dependency".to_string(),
        ));
    }

    Ok(())
}

#[cfg(test)]
#[allow(deprecated)]
mod tests;

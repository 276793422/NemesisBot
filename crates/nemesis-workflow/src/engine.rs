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

use crate::checkpoint::{
    Checkpoint, CheckpointStore, FileCheckpointStore, SerializableContext,
};
use crate::context::WorkflowContext;
use crate::events::{WorkflowEvent, WorkflowEventManager};
use crate::nodes::{NodeExecutorRegistry, SubWorkflowNodeExecutor};
use crate::persistence::WorkflowPersistence;
use crate::scheduler::{self, ScheduleOutcome};
use crate::triggers::CronTimezone;
use crate::types::{Execution, ExecutionState, NodeDef, NodeResult, TriggerSource, Workflow};

/// Render a path for logging without panicking on non-UTF8.
fn path_dbg(path: &Path) -> String {
    path.display().to_string()
}

/// [`scheduler::ProgressHook`] implementation that saves a checkpoint after
/// every level (1b-A1 step 6). Borrows the engine for the duration of the
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
/// Used by [`WorkflowEngine::save_checkpoint`] (1b-A1 step 6) when persisting
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
    node_executors: NodeExecutorRegistry,
    /// Optional persistence directory. If empty, persistence is disabled.
    persistence_dir: Option<PathBuf>,
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
    /// Observer / event-bus for workflow lifecycle events
    /// (Started/Completed/Failed/Cancelled). External systems (web dashboards,
    /// log shippers) register observers via [`event_manager`](Self::event_manager).
    event_manager: WorkflowEventManager,
    /// Per-engine workflow call stack (1c-F2). Tracks every in-flight
    /// execution so we can enforce MAX_RECURSION_DEPTH for AgentTool
    /// nestings and provide diagnostics (snapshot of active frames).
    call_stack: std::sync::Arc<crate::call_stack::WorkflowCallStack>,
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
            node_executors: NodeExecutorRegistry::new(),
            persistence_dir: None,
            checkpoint_store: parking_lot::RwLock::new(None),
            closed: RwLock::new(false),
            cancel_tokens: DashMap::new(),
            event_manager: WorkflowEventManager::new(),
            call_stack: std::sync::Arc::new(crate::call_stack::WorkflowCallStack::new()),
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
            node_executors: NodeExecutorRegistry::new(),
            persistence_dir: None,
            checkpoint_store: parking_lot::RwLock::new(None),
            closed: RwLock::new(false),
            cancel_tokens: DashMap::new(),
            event_manager: WorkflowEventManager::new(),
            call_stack: std::sync::Arc::new(crate::call_stack::WorkflowCallStack::new()),
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
            node_executors: NodeExecutorRegistry::new(),
            persistence_dir: Some(persistence_dir),
            checkpoint_store: parking_lot::RwLock::new(None),
            closed: RwLock::new(false),
            cancel_tokens: DashMap::new(),
            event_manager: WorkflowEventManager::new(),
            call_stack: std::sync::Arc::new(crate::call_stack::WorkflowCallStack::new()),
        }
    }

    /// Create a new engine with persistence and full sub_workflow support.
    ///
    /// Like [`Self::new_arc`] but with persistence enabled.
    pub fn with_persistence_arc(persistence_dir: PathBuf) -> Arc<Self> {
        let engine = Arc::new(Self {
            workflows: DashMap::new(),
            executions: RwLock::new(HashMap::new()),
            node_executors: NodeExecutorRegistry::new(),
            persistence_dir: Some(persistence_dir),
            checkpoint_store: parking_lot::RwLock::new(None),
            closed: RwLock::new(false),
            cancel_tokens: DashMap::new(),
            event_manager: WorkflowEventManager::new(),
            call_stack: std::sync::Arc::new(crate::call_stack::WorkflowCallStack::new()),
        });

        engine.node_executors.register(
            "sub_workflow",
            Arc::new(SubWorkflowNodeExecutor::new(engine.clone())),
        );

        engine
    }

    /// Create a new engine with custom node executors.
    pub fn with_executors(node_executors: NodeExecutorRegistry) -> Self {
        Self {
            workflows: DashMap::new(),
            executions: RwLock::new(HashMap::new()),
            node_executors,
            persistence_dir: None,
            checkpoint_store: parking_lot::RwLock::new(None),
            closed: RwLock::new(false),
            cancel_tokens: DashMap::new(),
            event_manager: WorkflowEventManager::new(),
            call_stack: std::sync::Arc::new(crate::call_stack::WorkflowCallStack::new()),
        }
    }

    /// Create a new engine with custom node executors and persistence.
    pub fn with_executors_and_persistence(
        node_executors: NodeExecutorRegistry,
        persistence_dir: PathBuf,
    ) -> Self {
        Self {
            workflows: DashMap::new(),
            executions: RwLock::new(HashMap::new()),
            node_executors,
            persistence_dir: Some(persistence_dir),
            checkpoint_store: parking_lot::RwLock::new(None),
            closed: RwLock::new(false),
            cancel_tokens: DashMap::new(),
            event_manager: WorkflowEventManager::new(),
            call_stack: std::sync::Arc::new(crate::call_stack::WorkflowCallStack::new()),
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
    /// JSONL-persisted; pass `None` to disable persistence.
    ///
    /// Returns `Arc<Self>` because the `sub_workflow` executor holds a back
    /// reference. This is the recommended constructor for gateway / service
    /// integration (milestone 1a-E1).
    pub fn new_integrated(
        provider: Arc<dyn LLMProvider>,
        tools: Arc<nemesis_tools::registry::ToolRegistry>,
        persistence_dir: Option<PathBuf>,
    ) -> Arc<Self> {
        // If persistence is enabled, also enable on-disk checkpoints so resume
        // survives restarts. Checkpoints live under
        // `{persistence_dir}/checkpoints/`.
        let checkpoint_store: Option<Arc<dyn CheckpointStore>> = persistence_dir
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
            node_executors: NodeExecutorRegistry::new(),
            persistence_dir,
            checkpoint_store: parking_lot::RwLock::new(checkpoint_store),
            closed: RwLock::new(false),
            cancel_tokens: DashMap::new(),
            event_manager: WorkflowEventManager::new(),
            call_stack: std::sync::Arc::new(crate::call_stack::WorkflowCallStack::new()),
        });

        // Override mock node executors with real ones.
        engine.node_executors.register(
            "llm",
            Arc::new(crate::nodes::RealLLMNodeExecutor::new(provider.clone())),
        );
        engine.node_executors.register(
            "tool",
            Arc::new(crate::nodes::RealToolNodeExecutor::new(tools)),
        );
        engine.node_executors.register(
            "question_classifier",
            Arc::new(crate::nodes::QuestionClassifierNodeExecutor::new(provider.clone())),
        );
        engine.node_executors.register(
            "parameter_extractor",
            Arc::new(crate::nodes::ParameterExtractorNodeExecutor::new(provider)),
        );
        engine.node_executors.register(
            "sub_workflow",
            Arc::new(SubWorkflowNodeExecutor::new(engine.clone())),
        );

        engine
    }

    /// Register an [`AgentRunner`] for `agent` workflow nodes (milestone 1b-D2).
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

    /// Scan a directory for workflow definition files and register each one.
    ///
    /// Supports `.yaml`, `.yml`, and `.json` extensions. Files that fail to
    /// parse or validate are skipped with a warning log; other files are still
    /// loaded. Returns the number of workflows successfully registered.
    ///
    /// Used by the gateway (milestone 1a-E1) to populate the engine from
    /// `{home}/workspace/workflows/` at startup.
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
        for entry in entries.flatten() {
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
    /// Used by gateway (milestone 1a-E2) to register cron schedules at startup.
    pub fn list_cron_workflows(
        &self,
    ) -> Vec<(String, String, CronTimezone, HashMap<String, serde_json::Value>)> {
        let mut out = Vec::new();
        for entry in self.workflows.iter() {
            let wf = entry.value();
            for trigger in &wf.triggers {
                if trigger.trigger_type != "cron" {
                    continue;
                }
                let schedule = match trigger.config.get("schedule").and_then(|v| v.as_str()) {
                    Some(s) => s.to_string(),
                    None => {
                        tracing::warn!(
                            workflow = %wf.name,
                            "[Workflow] Cron trigger missing 'schedule' field, skipping"
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
                        m.iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect::<HashMap<_, _>>()
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
                        CronTimezone::Utc => {
                            match cron.find_next_occurrence(&now_utc, false) {
                                Ok(next) => (next - now_utc).to_std().unwrap_or_else(|_| {
                                    std::time::Duration::from_millis(100)
                                }),
                                Err(err) => {
                                    tracing::warn!(
                                        workflow = %task_name,
                                        error = %err,
                                        "[Workflow] Failed to compute next cron fire, stopping schedule"
                                    );
                                    return;
                                }
                            }
                        }
                        CronTimezone::Local => {
                            match cron.find_next_occurrence(&now_local, false) {
                                Ok(next) => (next - now_local).to_std().unwrap_or_else(|_| {
                                    std::time::Duration::from_millis(100)
                                }),
                                Err(err) => {
                                    tracing::warn!(
                                        workflow = %task_name,
                                        error = %err,
                                        "[Workflow] Failed to compute next cron fire, stopping schedule"
                                    );
                                    return;
                                }
                            }
                        }
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

    /// Borrow the workflow call stack. Useful for diagnostics — e.g.
    /// listing currently in-flight executions for a "what's running right
    /// now" WSAPI command.
    pub fn call_stack(&self) -> &std::sync::Arc<crate::call_stack::WorkflowCallStack> {
        &self.call_stack
    }

    /// Borrow the checkpoint store, if configured. Returns `None` when
    /// checkpoints are disabled (in-memory engine, no persistence dir).
    ///
    /// Used by gateway restart-recovery (1b-A1 step 7) to enumerate and
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
    /// into the engine's in-memory map. Called by gateway on startup (1b-A1
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
            // (1b-A1 step 7 enhancement): we look it up by scanning registered
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

            // Skip terminal checkpoints — nothing to resume.
            let all_completed = cp.completed_nodes.len() >= workflow.nodes.len()
                && cp.waiting_node.is_none();
            if all_completed {
                continue;
            }

            // Rebuild the Execution. We don't have the original input/trigger
            // metadata on the checkpoint (the snapshot carries variables/input
            // separately), so we lift them back out of the context snapshot.
            let now = Local::now();
            let mut execution = Execution::new(workflow.name.clone(), cp.context_snapshot.input.clone());
            execution.id = exec_id.clone();
            execution.started_at = cp.saved_at.with_timezone(&chrono::Local);
            execution.workflow_hash = Some(cp.workflow_hash.clone());

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
    /// `wf_ctx`. Best-effort: returns `Ok(())` if no store is configured.
    ///
    /// `completed_nodes` is the set of node IDs that have already finished
    /// (extracted from `wf_ctx` when not supplied explicitly). `waiting_node`
    /// is `Some(id)` when the execution is paused at a `human_review` node.
    pub async fn save_checkpoint(
        &self,
        execution_id: &str,
        workflow_name: &str,
        wf_ctx: &WorkflowContext,
        waiting_node: Option<&str>,
        parent_execution_id: Option<&str>,
    ) -> Result<(), EngineError> {
        let store = match self.checkpoint_store.read().clone() {
            Some(s) => s,
            None => return Ok(()),
        };

        let workflow = self
            .get_workflow(workflow_name)
            .ok_or_else(|| EngineError::WorkflowNotFound(workflow_name.to_string()))?;
        let workflow_hash = workflow.hash();

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
        self.workflows.insert(workflow.name.clone(), workflow);
        Ok(())
    }

    /// Retrieve a workflow by name.
    pub fn get_workflow(&self, name: &str) -> Option<Workflow> {
        self.workflows.get(name).map(|r| r.value().clone())
    }

    /// List all registered workflow names.
    pub fn list_workflows(&self) -> Vec<String> {
        self.workflows.iter().map(|r| r.key().clone()).collect()
    }

    /// Unregister (remove) a workflow definition from the engine.
    ///
    /// This removes the workflow by name. Any in-progress executions of this
    /// workflow are **not** automatically cancelled.
    pub fn unregister(&self, name: &str) {
        self.workflows.remove(name);
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

        // Load the execution snapshot we will mutate.
        let mut execution = {
            let execs = self.executions.read().await;
            execs.get(execution_id)
                .cloned()
                .ok_or_else(|| EngineError::ExecutionNotFound(execution_id.to_string()))?
        };

        let workflow = self
            .get_workflow(&execution.workflow_name)
            .ok_or_else(|| EngineError::WorkflowNotFound(execution.workflow_name.clone()))?;

        // 1c-F2: push a call-stack frame for this execution. The depth comes
        // from the trigger source (AgentTool-triggered runs carry a depth;
        // everything else is 0). Push can reject if a caller bypassed the
        // WorkflowRunTool's pre-check and tried to start at depth > MAX.
        let recursion_depth = crate::call_stack::CallFrame::depth_from_trigger(
            &execution.trigger_source,
        );
        // If we're already inside a workflow run (sub_workflow node, or an
        // agent_node that re-invoked workflow_run), link this new frame to
        // the current top of the stack as its parent. Top-level invocations
        // (stack empty) have no parent.
        let parent_execution_id = self
            .call_stack
            .snapshot()
            .last()
            .map(|f| f.execution_id.clone());
        let frame = crate::call_stack::CallFrame {
            execution_id: execution.id.clone(),
            workflow_name: execution.workflow_name.clone(),
            parent_execution_id,
            trigger_source: execution.trigger_source.clone(),
            recursion_depth,
        };
        if let Err(reason) = self.call_stack.push(frame) {
            return Err(EngineError::RecursionLimitExceeded(reason));
        }

        // Initialize workflow context from execution input
        let mut wf_ctx = WorkflowContext::new(execution.input.clone());
        for (k, v) in &workflow.variables {
            // Workflow YAML stores initial variables as strings; lift them to
            // Value::String so the rest of the engine only sees JSON values.
            wf_ctx.set_var(k, serde_json::Value::String(v.clone()));
        }

        // Stamp the execution with the workflow's structural hash so the
        // resume path can detect config drift (1b-A1 step 5).
        execution.workflow_hash = Some(workflow.hash());

        // Create cancellation token for this execution. Stored in cancel_tokens
        // so cancel_execution(id) can trigger it; removed on completion.
        let cancel_token = CancellationToken::new();
        self.cancel_tokens
            .insert(execution.id.clone(), cancel_token.clone());

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

        // Token is no longer needed; remove from map.
        self.cancel_tokens.remove(&execution.id);

        let now = Local::now();
        execution.ended_at = Some(now);

        match schedule_result {
            Ok(ScheduleOutcome::Cancelled) => {
                execution.state = ExecutionState::Cancelled;
            }
            Ok(ScheduleOutcome::Completed) => {
                // Check if any node is in waiting state (human review)
                let all_completed = wf_ctx
                    .get_all_node_results()
                    .values()
                    .all(|r| r.state != ExecutionState::Waiting);

                if all_completed {
                    execution.state = ExecutionState::Completed;
                } else {
                    execution.state = ExecutionState::Waiting;
                }
            }
            Err(err) => {
                execution.state = ExecutionState::Failed;
                execution.error = Some(err);
            }
        }

        // Copy results from context into execution
        execution.node_results = wf_ctx.get_all_node_results();

        // Persist final state
        self.persist_execution(&execution).await;

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
        match execution.state {
            ExecutionState::Completed => {
                self.event_manager
                    .emit(WorkflowEvent::Completed {
                        execution_id: execution_id_for_event,
                        workflow_name: workflow_name_for_event,
                        timestamp: Local::now(),
                    })
                    .await;
            }
            ExecutionState::Failed => {
                let err = execution.error.clone().unwrap_or_default();
                self.event_manager
                    .emit(WorkflowEvent::Failed {
                        execution_id: execution_id_for_event,
                        workflow_name: workflow_name_for_event,
                        error: err,
                        timestamp: Local::now(),
                    })
                    .await;
            }
            ExecutionState::Cancelled => {
                self.event_manager
                    .emit(WorkflowEvent::Cancelled {
                        execution_id: execution_id_for_event,
                        workflow_name: workflow_name_for_event,
                        timestamp: Local::now(),
                    })
                    .await;
            }
            _ => {}
        }

        // Pop our call-stack frame now that the execution has settled.
        self.call_stack.pop();

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
        let node_map: HashMap<&str, &NodeDef> = workflow
            .nodes
            .iter()
            .map(|n| (n.id.as_str(), n))
            .collect();

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
                execution.node_results.insert(node_id.clone(), result.clone());

                // Merge output into context.
                if let Some(obj) = result.output.as_object() {
                    for (k, v) in obj {
                        context.insert(k.clone(), v.clone());
                    }
                }

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
    pub async fn list_executions(&self, workflow_name: Option<&str>) -> Vec<Execution> {
        let execs = self.executions.read().await;
        execs
            .values()
            .filter(|exec| {
                workflow_name
                    .map(|name| exec.workflow_name == name)
                    .unwrap_or(true)
            })
            .cloned()
            .collect()
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
    pub async fn cancel_execution(&self, id: &str) -> Result<Execution, EngineError> {
        // Trigger the cancellation token first so in-flight tasks exit promptly.
        if let Some(entry) = self.cancel_tokens.get(id) {
            entry.cancel();
            drop(entry);
        } else {
            // No active token: execution may already be finished. Validate state below.
        }

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

        let updated = execution.clone();
        drop(execs); // Release lock before persistence I/O

        self.persist_execution(&updated).await;

        Ok(updated)
    }

    /// Resume a waiting execution (e.g., after human review).
    ///
    /// Updates the waiting node's result with the reviewer's response, then
    /// continues executing the rest of the workflow via `schedule_resume`.
    /// Any nodes that already ran (everything in `node_results` with state
    /// `Completed`) are skipped; only nodes downstream of the waiting node
    /// actually re-execute.
    ///
    /// If a checkpoint store is configured (1b-A1), the new state is saved
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

            // Find the waiting node and update its result with the review.
            let mut found_waiting: Option<String> = None;
            for (node_id, result) in execution.node_results.iter_mut() {
                if result.state == ExecutionState::Waiting {
                    result.output = serde_json::json!(review_result);
                    result.state = ExecutionState::Completed;
                    result.ended_at = Local::now();
                    if let Some(approved) = review_result.get("approved") {
                        if let Some(b) = approved.as_bool() {
                            debug!(
                                "[Workflow] Node {} review result: approved={}",
                                node_id, b
                            );
                        }
                    }
                    found_waiting = Some(node_id.clone());
                    break;
                }
            }

            let waiting_id = found_waiting.ok_or_else(|| {
                EngineError::InvalidState(format!(
                    "execution {} has no node in waiting state",
                    id
                ))
            })?;

            // Mark as Running while schedule_resume executes.
            execution.state = ExecutionState::Running;
            execution.ended_at = None;
            let snap = execution.clone();
            (snap, waiting_id)
        };

        let (mut execution, _waiting_node_id) = execution_snapshot;

        // Load the workflow definition (needed for schedule_resume).
        let workflow = self
            .get_workflow(&execution.workflow_name)
            .ok_or_else(|| EngineError::WorkflowNotFound(execution.workflow_name.clone()))?;

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

        // Build context from the current execution state (carries the
        // just-resolved review_result through to downstream nodes).
        let mut wf_ctx = WorkflowContext::new(execution.input.clone());
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

        // Install / refresh cancellation token.
        let cancel_token = CancellationToken::new();
        self.cancel_tokens
            .insert(execution.id.clone(), cancel_token.clone());

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

        let now = Local::now();
        match schedule_result {
            Ok(ScheduleOutcome::Cancelled) => {
                execution.state = ExecutionState::Cancelled;
                execution.ended_at = Some(now);
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

        // Copy fresh node_results + variables back into the execution.
        execution.node_results = wf_ctx.get_all_node_results();
        execution.variables = wf_ctx.get_all_variables();

        // Save a new checkpoint reflecting post-resume state. Skip when the
        // execution has already terminated (no value in checkpointing a dead
        // execution; the latest checkpoint stays as the last in-flight one).
        if execution.state == ExecutionState::Waiting || execution.state == ExecutionState::Running
        {
            let results = wf_ctx.get_all_node_results();
            let waiting: Option<String> = if execution.state == ExecutionState::Waiting {
                results
                    .iter()
                    .find(|(_, r)| r.state == ExecutionState::Waiting)
                    .map(|(id, _)| id.clone())
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
        }

        // Persist + update in-memory state.
        self.persist_execution(&execution).await;
        {
            let mut execs = self.executions.write().await;
            execs.insert(execution.id.clone(), execution.clone());
        }

        Ok(execution)
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
            let file_path = dir.join(format!("{}_{}.jsonl", execution.workflow_name, execution.id));
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
        return Err(EngineError::CycleDetected("circular dependency".to_string()));
    }

    Ok(())
}

#[cfg(test)]
mod tests;

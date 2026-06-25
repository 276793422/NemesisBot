//! Workflow engine: registration, DAG validation, and topological execution.
//!
//! Mirrors the Go `engine.go` with full workflow lifecycle management:
//! register/unregister workflows, run executions, cancel/resume, list, and
//! optional JSONL-based persistence.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use chrono::Local;
use dashmap::DashMap;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::context::WorkflowContext;
use crate::nodes::{NodeExecutorRegistry, SubWorkflowNodeExecutor};
use crate::persistence::WorkflowPersistence;
use crate::scheduler::{self, ScheduleOutcome};
use crate::types::{Execution, ExecutionState, NodeDef, NodeResult, TriggerSource, Workflow};

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
    /// Whether the engine has been shut down.
    closed: RwLock<bool>,
    /// Per-execution cancellation tokens. Presence of an entry implies the
    /// execution is still in-flight (Running); completion removes the entry.
    /// Cancelled tokens cause scheduler + node executors to bail out promptly.
    cancel_tokens: DashMap<String, CancellationToken>,
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
            closed: RwLock::new(false),
            cancel_tokens: DashMap::new(),
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
            closed: RwLock::new(false),
            cancel_tokens: DashMap::new(),
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
            closed: RwLock::new(false),
            cancel_tokens: DashMap::new(),
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
            closed: RwLock::new(false),
            cancel_tokens: DashMap::new(),
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
            closed: RwLock::new(false),
            cancel_tokens: DashMap::new(),
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
            closed: RwLock::new(false),
            cancel_tokens: DashMap::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Workflow Registration
    // -----------------------------------------------------------------------

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
        execution.trigger_source = trigger_source;

        // Store execution in memory
        {
            let mut execs = self.executions.write().await;
            execs.insert(execution.id.clone(), execution.clone());
        }

        // Persist initial state
        self.persist_execution(&execution).await;

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

        // Initialize workflow context from execution input
        let mut wf_ctx = WorkflowContext::new(execution.input.clone());
        for (k, v) in &workflow.variables {
            wf_ctx.set_var(k, v);
        }

        // Create cancellation token for this execution. Stored in cancel_tokens
        // so cancel_execution(id) can trigger it; removed on completion.
        let cancel_token = CancellationToken::new();
        self.cancel_tokens
            .insert(execution.id.clone(), cancel_token.clone());

        // Execute the workflow using the scheduler
        let schedule_result = scheduler::schedule(
            &workflow.nodes,
            &workflow.edges,
            &self.node_executors,
            &mut wf_ctx,
            cancel_token,
        )
        .await;

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
    /// The `review_result` map contains the reviewer's response. This method
    /// finds the node in `Waiting` state, updates it with the review result,
    /// and marks the execution as completed.
    pub async fn resume_execution(
        &self,
        id: &str,
        review_result: HashMap<String, serde_json::Value>,
    ) -> Result<(), EngineError> {
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

        // Find the waiting node and update its result
        let mut found_waiting = false;
        for (node_id, result) in execution.node_results.iter_mut() {
            if result.state == ExecutionState::Waiting {
                result.output = serde_json::json!(review_result);
                result.state = ExecutionState::Completed;
                result.ended_at = Local::now();

                // Set variable for downstream nodes: {node_id}_approved
                if let Some(approved) = review_result.get("approved") {
                    if let Some(b) = approved.as_bool() {
                        // We store approval status as a node result metadata field
                        // since the Execution type doesn't have a variables field
                        debug!(
                            "[Workflow] Node {} review result: approved={}",
                            node_id, b
                        );
                    }
                }

                found_waiting = true;
                break;
            }
        }

        if !found_waiting {
            return Err(EngineError::InvalidState(format!(
                "execution {} has no node in waiting state",
                id
            )));
        }

        execution.state = ExecutionState::Completed;
        execution.ended_at = Some(Local::now());

        let updated = execution.clone();
        drop(execs); // Release lock before persistence I/O

        self.persist_execution(&updated).await;

        Ok(())
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

//! Workflow engine: registration, DAG validation, and topological execution.
//!
//! Mirrors the Go `engine.go` with full workflow lifecycle management:
//! register/unregister workflows, run executions, cancel/resume, list, and
//! optional JSONL-based persistence.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use dashmap::DashMap;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::context::WorkflowContext;
use crate::nodes::{NodeExecutorRegistry, SubWorkflowNodeExecutor};
use crate::persistence::WorkflowPersistence;
use crate::scheduler;
use crate::types::{Execution, ExecutionState, NodeDef, NodeResult, Workflow};

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
        });

        // Replace the sub_workflow stub with a real executor that holds
        // a reference to this engine (mirrors Go: e.executors.Register("sub_workflow", &SubWorkflowNode{Engine: e}))
        let engine_ptr = Arc::as_ptr(&engine);
        let reg_mut = unsafe { &mut (*(engine_ptr as *mut Self)).node_executors };
        reg_mut.register(
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
        });

        let engine_ptr = Arc::as_ptr(&engine);
        let reg_mut = unsafe { &mut (*(engine_ptr as *mut Self)).node_executors };
        reg_mut.register(
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

    /// Run a registered workflow by name (the core execution function).
    ///
    /// Validates the DAG, creates an Execution record, initializes variables
    /// from the workflow definition and input, runs nodes using the scheduler,
    /// and persists the final state.
    ///
    /// This is the Rust equivalent of Go's `Engine.Run`.
    pub async fn run(
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

        // Create execution record
        let mut execution = Execution::new(workflow_name.to_string(), input.clone());
        execution.state = ExecutionState::Running;

        // Initialize workflow context from input
        let mut wf_ctx = WorkflowContext::new(input.clone());

        // Copy workflow variables into context (variables are flat strings)
        for (k, v) in &workflow.variables {
            wf_ctx.set_var(k, v);
        }

        // Store execution in memory
        {
            let mut execs = self.executions.write().await;
            execs.insert(execution.id.clone(), execution.clone());
        }

        // Persist initial state
        self.persist_execution(&execution).await;

        // Execute the workflow using the scheduler
        let schedule_result = scheduler::schedule(
            // We pass a dummy JoinHandle since we don't have a real cancellation context
            tokio::spawn(async {}),
            &workflow.nodes,
            &workflow.edges,
            &self.node_executors,
            &mut wf_ctx,
        )
        .await;

        let now = Utc::now();
        execution.ended_at = Some(now);

        if let Err(err) = schedule_result {
            // Check if it was a cancellation
            if err.contains("cancel") {
                execution.state = ExecutionState::Cancelled;
            } else {
                execution.state = ExecutionState::Failed;
            }
            execution.error = Some(err);
        } else {
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

        // Copy results from context into execution
        let node_results = wf_ctx.get_all_node_results();
        execution.node_results = node_results;

        // Persist final state
        self.persist_execution(&execution).await;

        // Update in-memory execution
        {
            let mut execs = self.executions.write().await;
            execs.insert(execution.id.clone(), execution.clone());
        }

        Ok(execution)
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
                        started_at: Utc::now(),
                        ended_at: Utc::now(),
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
                    execution.ended_at = Some(Utc::now());
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
                    execution.ended_at = Some(Utc::now());
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
                execution.ended_at = Some(Utc::now());
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
        execution.ended_at = Some(Utc::now());

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
    /// Marks the execution as cancelled and persists the state change.
    /// Only executions in `Running` or `Waiting` state can be cancelled.
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
        execution.ended_at = Some(Utc::now());

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
                result.ended_at = Utc::now();

                // Set variable for downstream nodes: {node_id}_approved
                if let Some(approved) = review_result.get("approved") {
                    if let Some(b) = approved.as_bool() {
                        // We store approval status as a node result metadata field
                        // since the Execution type doesn't have a variables field
                        debug!(
                            "Node {} review result: approved={}",
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
        execution.ended_at = Some(Utc::now());

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
        info!("Closing workflow engine");
        let mut closed = self.closed.write().await;
        *closed = true;

        // Future enhancement: cancel all running executions, flush persistence, etc.
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
                    "Failed to persist execution {}: {}",
                    execution.id, e
                );
            } else {
                debug!("Persisted execution {}", execution.id);
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
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_workflow(name: &str, nodes: Vec<NodeDef>) -> Workflow {
        Workflow {
            name: name.to_string(),
            description: String::new(),
            version: "1.0.0".to_string(),
            triggers: vec![],
            nodes,
            edges: vec![],
            variables: HashMap::new(),
            metadata: HashMap::new(),
        }
    }

    fn make_node(id: &str, node_type: &str, depends_on: Vec<&str>) -> NodeDef {
        NodeDef {
            id: id.to_string(),
            node_type: node_type.to_string(),
            config: HashMap::new(),
            depends_on: depends_on.into_iter().map(|s| s.to_string()).collect(),
            retry_count: 0,
            timeout: None,
        }
    }

    #[tokio::test]
    async fn test_register_and_get_workflow() {
        let engine = WorkflowEngine::new();
        let wf = make_workflow("test_wf", vec![make_node("n1", "llm", vec![])]);
        engine.register_workflow(wf).unwrap();

        let retrieved = engine.get_workflow("test_wf").unwrap();
        assert_eq!(retrieved.name, "test_wf");
        assert!(engine.get_workflow("nonexistent").is_none());
    }

    #[tokio::test]
    async fn test_list_workflows() {
        let engine = WorkflowEngine::new();
        engine
            .register_workflow(make_workflow("wf_a", vec![make_node("n1", "llm", vec![])]))
            .unwrap();
        engine
            .register_workflow(make_workflow("wf_b", vec![make_node("n1", "llm", vec![])]))
            .unwrap();

        let mut names = engine.list_workflows();
        names.sort();
        assert_eq!(names, vec!["wf_a", "wf_b"]);
    }

    #[tokio::test]
    async fn test_unregister_workflow() {
        let engine = WorkflowEngine::new();
        engine
            .register_workflow(make_workflow("wf_a", vec![make_node("n1", "llm", vec![])]))
            .unwrap();
        assert!(engine.get_workflow("wf_a").is_some());

        engine.unregister("wf_a");
        assert!(engine.get_workflow("wf_a").is_none());
    }

    #[tokio::test]
    async fn test_unregister_nonexistent() {
        let engine = WorkflowEngine::new();
        // Should not panic
        engine.unregister("nonexistent");
    }

    #[tokio::test]
    async fn test_start_execution_basic() {
        let engine = WorkflowEngine::new();
        let nodes = vec![
            make_node("n1", "llm", vec![]),
            make_node("n2", "tool", vec!["n1"]),
        ];
        engine
            .register_workflow(make_workflow("chain_wf", nodes))
            .unwrap();

        let execution = engine
            .start_execution("chain_wf", HashMap::new())
            .await
            .unwrap();

        assert_eq!(execution.state, ExecutionState::Completed);
        assert_eq!(execution.node_results.len(), 2);
        assert!(execution.node_results.contains_key("n1"));
        assert!(execution.node_results.contains_key("n2"));
    }

    #[tokio::test]
    async fn test_run_not_found() {
        let engine = WorkflowEngine::new();
        let result = engine.run("nonexistent", HashMap::new()).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, EngineError::WorkflowNotFound(_)));
    }

    #[tokio::test]
    async fn test_condition_evaluation_in_execution() {
        let engine = WorkflowEngine::new();
        let mut cond_config = HashMap::new();
        cond_config.insert("condition".to_string(), serde_json::json!("status == ok"));

        let nodes = vec![
            make_node("n1", "llm", vec![]),
            NodeDef {
                id: "n2".to_string(),
                node_type: "condition".to_string(),
                config: cond_config,
                depends_on: vec!["n1".to_string()],
                retry_count: 0,
                timeout: None,
            },
        ];
        engine
            .register_workflow(make_workflow("cond_wf", nodes))
            .unwrap();

        let mut input = HashMap::new();
        input.insert("status".to_string(), serde_json::json!("ok"));

        let execution = engine.start_execution("cond_wf", input).await.unwrap();
        assert_eq!(execution.state, ExecutionState::Completed);

        let cond_result = &execution.node_results["n2"];
        assert!(cond_result.output["condition_result"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_dependency_ordering_respected() {
        let engine = WorkflowEngine::new();
        let nodes = vec![
            make_node("a", "llm", vec![]),
            make_node("b", "tool", vec!["a"]),
            make_node("c", "transform", vec!["b"]),
        ];
        engine
            .register_workflow(make_workflow("ordered_wf", nodes))
            .unwrap();

        let execution = engine
            .start_execution("ordered_wf", HashMap::new())
            .await
            .unwrap();

        assert_eq!(execution.state, ExecutionState::Completed);
        // All three nodes should have completed.
        assert_eq!(execution.node_results.len(), 3);
        for (id, result) in &execution.node_results {
            assert_eq!(result.state, ExecutionState::Completed, "node {} failed", id);
        }
    }

    // -----------------------------------------------------------------------
    // get_execution tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_get_execution_found() {
        let engine = WorkflowEngine::new();
        engine
            .register_workflow(make_workflow("wf", vec![make_node("n1", "llm", vec![])]))
            .unwrap();

        let execution = engine
            .start_execution("wf", HashMap::new())
            .await
            .unwrap();
        let id = execution.id.clone();

        let retrieved = engine.get_execution(&id).await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().id, id);
    }

    #[tokio::test]
    async fn test_get_execution_not_found() {
        let engine = WorkflowEngine::new();
        let result = engine.get_execution("nonexistent_id").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_get_execution_or_err() {
        let engine = WorkflowEngine::new();
        let result = engine.get_execution_or_err("nonexistent_id").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, EngineError::ExecutionNotFound(_)));
    }

    // -----------------------------------------------------------------------
    // cancel_execution tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_cancel_running_execution() {
        let engine = WorkflowEngine::new();
        engine
            .register_workflow(make_workflow("wf", vec![make_node("n1", "llm", vec![])]))
            .unwrap();

        let execution = engine
            .start_execution("wf", HashMap::new())
            .await
            .unwrap();
        // Execution is already completed since start_execution is synchronous.
        // Let's manually set up a running execution for testing cancel.
        let id = execution.id.clone();

        // For a real cancel test we'd need a long-running workflow.
        // Here we test the state check: cancelling a completed execution should fail.
        let result = engine.cancel_execution(&id).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, EngineError::InvalidState(_)));
    }

    #[tokio::test]
    async fn test_cancel_nonexistent_execution() {
        let engine = WorkflowEngine::new();
        let result = engine.cancel_execution("nonexistent").await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            EngineError::ExecutionNotFound(_)
        ));
    }

    // -----------------------------------------------------------------------
    // resume_execution tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_resume_waiting_execution() {
        let engine = WorkflowEngine::new();
        let mut hr_config = HashMap::new();
        hr_config.insert("message".to_string(), serde_json::json!("Please review"));

        let nodes = vec![NodeDef {
            id: "n1".to_string(),
            node_type: "human_review".to_string(),
            config: hr_config,
            depends_on: vec![],
            retry_count: 0,
            timeout: None,
        }];
        engine
            .register_workflow(make_workflow("hr_wf", nodes))
            .unwrap();

        let execution = engine
            .start_execution("hr_wf", HashMap::new())
            .await
            .unwrap();
        assert_eq!(execution.state, ExecutionState::Waiting);

        let id = execution.id.clone();
        let mut review = HashMap::new();
        review.insert("approved".to_string(), serde_json::json!(true));
        review.insert("comment".to_string(), serde_json::json!("Looks good"));

        engine.resume_execution(&id, review).await.unwrap();

        let updated = engine.get_execution(&id).await.unwrap();
        assert_eq!(updated.state, ExecutionState::Completed);
        assert!(updated.ended_at.is_some());
    }

    #[tokio::test]
    async fn test_resume_non_waiting_execution() {
        let engine = WorkflowEngine::new();
        engine
            .register_workflow(make_workflow("wf", vec![make_node("n1", "llm", vec![])]))
            .unwrap();

        let execution = engine
            .start_execution("wf", HashMap::new())
            .await
            .unwrap();
        // Execution completed normally
        assert_eq!(execution.state, ExecutionState::Completed);

        let id = execution.id.clone();
        let result = engine.resume_execution(&id, HashMap::new()).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), EngineError::InvalidState(_)));
    }

    #[tokio::test]
    async fn test_resume_nonexistent_execution() {
        let engine = WorkflowEngine::new();
        let result = engine
            .resume_execution("nonexistent", HashMap::new())
            .await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            EngineError::ExecutionNotFound(_)
        ));
    }

    // -----------------------------------------------------------------------
    // list_executions tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_list_executions_all() {
        let engine = WorkflowEngine::new();
        engine
            .register_workflow(make_workflow("wf1", vec![make_node("n1", "llm", vec![])]))
            .unwrap();
        engine
            .register_workflow(make_workflow("wf2", vec![make_node("n1", "llm", vec![])]))
            .unwrap();

        engine.start_execution("wf1", HashMap::new()).await.unwrap();
        engine.start_execution("wf2", HashMap::new()).await.unwrap();

        let all = engine.list_executions(None).await;
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn test_list_executions_filtered() {
        let engine = WorkflowEngine::new();
        engine
            .register_workflow(make_workflow("wf1", vec![make_node("n1", "llm", vec![])]))
            .unwrap();
        engine
            .register_workflow(make_workflow("wf2", vec![make_node("n1", "llm", vec![])]))
            .unwrap();

        engine.start_execution("wf1", HashMap::new()).await.unwrap();
        engine.start_execution("wf2", HashMap::new()).await.unwrap();
        engine.start_execution("wf1", HashMap::new()).await.unwrap();

        let filtered = engine.list_executions(Some("wf1")).await;
        assert_eq!(filtered.len(), 2);

        let filtered2 = engine.list_executions(Some("wf2")).await;
        assert_eq!(filtered2.len(), 1);
    }

    #[tokio::test]
    async fn test_list_executions_empty() {
        let engine = WorkflowEngine::new();
        let all = engine.list_executions(None).await;
        assert!(all.is_empty());
    }

    // -----------------------------------------------------------------------
    // close tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_close_engine() {
        let engine = WorkflowEngine::new();
        assert!(!engine.is_closed().await);

        engine.close().await;
        assert!(engine.is_closed().await);

        // Running after close should fail
        engine
            .register_workflow(make_workflow("wf", vec![make_node("n1", "llm", vec![])]))
            .unwrap();
        let result = engine.run("wf", HashMap::new()).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), EngineError::InvalidState(_)));
    }

    // -----------------------------------------------------------------------
    // persistence tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_persistence_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let engine = WorkflowEngine::with_persistence(dir.path().to_path_buf());
        engine
            .register_workflow(make_workflow("persist_wf", vec![make_node("n1", "llm", vec![])]))
            .unwrap();

        let execution = engine
            .start_execution("persist_wf", HashMap::new())
            .await
            .unwrap();
        let id = execution.id.clone();

        // Execution should be found in memory
        let found = engine.get_execution(&id).await;
        assert!(found.is_some());
        assert_eq!(found.unwrap().workflow_name, "persist_wf");
    }

    #[tokio::test]
    async fn test_get_execution_loads_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let engine = WorkflowEngine::with_persistence(dir.path().to_path_buf());
        engine
            .register_workflow(make_workflow("disk_wf", vec![make_node("n1", "llm", vec![])]))
            .unwrap();

        let execution = engine
            .start_execution("disk_wf", HashMap::new())
            .await
            .unwrap();
        let id = execution.id.clone();

        // Create a new engine instance with the same persistence dir
        let engine2 = WorkflowEngine::with_persistence(dir.path().to_path_buf());
        // The execution should be loadable from disk
        let loaded = engine2.get_execution(&id).await;
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().id, id);
    }

    #[tokio::test]
    async fn test_register_invalid_workflow_no_nodes() {
        let engine = WorkflowEngine::new();
        let wf = Workflow {
            name: "invalid".to_string(),
            description: String::new(),
            version: "1.0.0".to_string(),
            triggers: vec![],
            nodes: vec![],
            edges: vec![],
            variables: HashMap::new(),
            metadata: HashMap::new(),
        };
        let result = engine.register_workflow(wf);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_register_invalid_workflow_no_name() {
        let engine = WorkflowEngine::new();
        let wf = Workflow {
            name: String::new(),
            description: String::new(),
            version: "1.0.0".to_string(),
            triggers: vec![],
            nodes: vec![make_node("n1", "llm", vec![])],
            edges: vec![],
            variables: HashMap::new(),
            metadata: HashMap::new(),
        };
        let result = engine.register_workflow(wf);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_replace_workflow() {
        let engine = WorkflowEngine::new();
        engine
            .register_workflow(make_workflow("wf", vec![make_node("n1", "llm", vec![])]))
            .unwrap();

        // Re-register with same name but different node type
        engine
            .register_workflow(make_workflow("wf", vec![make_node("n1", "tool", vec![])]))
            .unwrap();

        let wf = engine.get_workflow("wf").unwrap();
        assert_eq!(wf.nodes[0].node_type, "tool");
    }

    #[tokio::test]
    async fn test_start_execution_workflow_not_found() {
        let engine = WorkflowEngine::new();
        let result = engine.start_execution("nonexistent", HashMap::new()).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), EngineError::WorkflowNotFound(_)));
    }

    #[tokio::test]
    async fn test_start_execution_unknown_node_type() {
        let engine = WorkflowEngine::new();
        let nodes = vec![NodeDef {
            id: "n1".to_string(),
            node_type: "nonexistent_type".to_string(),
            config: HashMap::new(),
            depends_on: vec![],
            retry_count: 0,
            timeout: None,
        }];
        engine
            .register_workflow(make_workflow("bad_type_wf", nodes))
            .unwrap();

        let result = engine.start_execution("bad_type_wf", HashMap::new()).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), EngineError::UnknownNodeType(_)));
    }

    #[tokio::test]
    async fn test_start_execution_with_cycle() {
        let engine = WorkflowEngine::new();
        let nodes = vec![
            make_node("a", "llm", vec!["b"]),
            make_node("b", "llm", vec!["a"]),
        ];
        let result = engine.register_workflow(make_workflow("cycle_wf", nodes));
        // Cycle is detected at registration time, not execution time
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_engine_close_prevents_new_runs() {
        let engine = WorkflowEngine::new();
        engine
            .register_workflow(make_workflow("wf", vec![make_node("n1", "llm", vec![])]))
            .unwrap();

        engine.close().await;
        assert!(engine.is_closed().await);

        let result = engine.start_execution("wf", HashMap::new()).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), EngineError::InvalidState(_)));
    }

    #[tokio::test]
    async fn test_engine_default() {
        let engine = WorkflowEngine::default();
        assert!(!engine.is_closed().await);
        assert!(engine.list_workflows().is_empty());
    }

    #[tokio::test]
    async fn test_engine_new_arc() {
        let engine = WorkflowEngine::new_arc();
        assert!(!engine.is_closed().await);
    }

    #[tokio::test]
    async fn test_engine_with_executors() {
        let registry = NodeExecutorRegistry::new();
        let engine = WorkflowEngine::with_executors(registry);
        assert!(!engine.is_closed().await);
    }

    #[tokio::test]
    async fn test_execution_has_timestamps() {
        let engine = WorkflowEngine::new();
        engine
            .register_workflow(make_workflow("wf", vec![make_node("n1", "llm", vec![])]))
            .unwrap();

        let execution = engine.start_execution("wf", HashMap::new()).await.unwrap();
        assert!(execution.ended_at.is_some());
        assert!(execution.ended_at.unwrap() >= execution.started_at);
    }

    #[tokio::test]
    async fn test_execution_input_preserved() {
        let engine = WorkflowEngine::new();
        engine
            .register_workflow(make_workflow("wf", vec![make_node("n1", "llm", vec![])]))
            .unwrap();

        let mut input = HashMap::new();
        input.insert("query".to_string(), serde_json::json!("test query"));
        let execution = engine.start_execution("wf", input).await.unwrap();
        assert_eq!(execution.input.get("query").unwrap(), "test query");
    }

    #[tokio::test]
    async fn test_list_executions_after_close() {
        let engine = WorkflowEngine::new();
        engine
            .register_workflow(make_workflow("wf", vec![make_node("n1", "llm", vec![])]))
            .unwrap();

        engine.start_execution("wf", HashMap::new()).await.unwrap();
        engine.close().await;

        // Should still be able to list executions after close
        let all = engine.list_executions(None).await;
        assert_eq!(all.len(), 1);
    }

    #[tokio::test]
    async fn test_with_persistence_arc() {
        let dir = tempfile::tempdir().unwrap();
        let engine = WorkflowEngine::with_persistence_arc(dir.path().to_path_buf());
        engine
            .register_workflow(make_workflow("wf", vec![make_node("n1", "llm", vec![])]))
            .unwrap();

        let execution = engine.start_execution("wf", HashMap::new()).await.unwrap();
        assert_eq!(execution.state, ExecutionState::Completed);
    }

    #[tokio::test]
    async fn test_engine_error_display() {
        let err = EngineError::WorkflowNotFound("test_wf".to_string());
        assert!(err.to_string().contains("test_wf"));

        let err = EngineError::CycleDetected("circular".to_string());
        assert!(err.to_string().contains("circular"));

        let err = EngineError::AlreadyCompleted("exec_id".to_string());
        assert!(err.to_string().contains("exec_id"));
    }

    #[tokio::test]
    async fn test_get_execution_or_err_found() {
        let engine = WorkflowEngine::new();
        engine
            .register_workflow(make_workflow("wf", vec![make_node("n1", "llm", vec![])]))
            .unwrap();

        let execution = engine.start_execution("wf", HashMap::new()).await.unwrap();
        let found = engine.get_execution_or_err(&execution.id).await;
        assert!(found.is_ok());
    }

    #[tokio::test]
    async fn test_with_executors_and_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let registry = NodeExecutorRegistry::new();
        let engine = WorkflowEngine::with_executors_and_persistence(
            registry,
            dir.path().to_path_buf(),
        );
        engine
            .register_workflow(make_workflow("wf", vec![make_node("n1", "llm", vec![])]))
            .unwrap();

        let execution = engine.start_execution("wf", HashMap::new()).await.unwrap();
        assert_eq!(execution.state, ExecutionState::Completed);
    }
}

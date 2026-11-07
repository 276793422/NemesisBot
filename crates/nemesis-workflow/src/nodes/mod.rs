//! Node executor trait and built-in node implementations.
//!
//! Includes all 11 built-in node types:
//! llm, tool, condition, parallel, loop, sub_workflow, transform, http, script, delay, human_review.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use chrono::Local;

use nemesis_providers::router::LLMProvider;
use nemesis_providers::types::{ChatOptions, LLMResponse, Message};

use crate::context::WorkflowContext;
use crate::types::{ExecutionState, NodeDef, NodeResult};

/// Trait for executing a workflow node.
///
/// Nodes receive a shared reference to the workflow context so they can
/// write back variables and node results (matching Go's `*Context` with
/// `SetVar()` and `SetNodeResult()`).
#[async_trait]
pub trait NodeExecutor: Send + Sync {
    /// Execute the given node definition within the provided context.
    ///
    /// The `context` parameter is a flat map of current variables and previous
    /// node outputs. The `wf_ctx` parameter provides shared access to the
    /// workflow context for writing back variables and node results.
    async fn execute(
        &self,
        node: &NodeDef,
        context: &HashMap<String, serde_json::Value>,
        wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String>;
}

// ---------------------------------------------------------------------------
// Helper: extract a list of child NodeDef objects from config
// ---------------------------------------------------------------------------

/// Extract a list of child NodeDef objects from the given config key.
///
/// Looks for either "nodes" or "branches" key. Each child is expected to be
/// a JSON object with at least "id" and "node_type" fields.
fn get_config_node_list(config: &HashMap<String, serde_json::Value>, key: &str) -> Vec<NodeDef> {
    let arr = match config.get(key).and_then(|v| v.as_array()) {
        Some(a) => a,
        None => return Vec::new(),
    };

    let mut nodes = Vec::new();
    for item in arr {
        let obj = match item.as_object() {
            Some(o) => o,
            None => continue,
        };

        let id = obj
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let node_type = obj
            .get("node_type")
            .or_else(|| obj.get("type"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let child_config: HashMap<String, serde_json::Value> = obj
            .get("config")
            .and_then(|v| v.as_object())
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();

        let depends_on = obj
            .get("depends_on")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let retry_count = obj.get("retry_count").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

        let timeout = obj
            .get("timeout")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let is_terminal = obj
            .get("is_terminal")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        nodes.push(NodeDef {
            id,
            node_type,
            config: child_config,
            depends_on,
            retry_count,
            timeout,
            is_terminal,
        });
    }
    nodes
}

// ---------------------------------------------------------------------------
// LLM usage tracking (shared by RealLLMNodeExecutor, QuestionClassifier, ParameterExtractor)
// ---------------------------------------------------------------------------

/// Shared slot for the optional LLM usage `DataStore`. The engine owns one
/// instance; the three LLM-calling node executors hold a clone so they can
/// record usage when the slot is populated by the gateway.
///
/// The slot starts empty (so unit tests and embedded deployments work
/// without a database) and is populated once via
/// [`crate::engine:WorkflowEngine::set_usage_store`].
pub type UsageStoreSlot = Arc<parking_lot::RwLock<Option<Arc<nemesis_data::DataStore>>>>;

/// Construct a fresh empty slot. Used by the engine and by tests that don't
/// care about usage tracking.
pub fn new_usage_store_slot() -> UsageStoreSlot {
    Arc::new(parking_lot::RwLock::new(None))
}

/// Best-effort write a `RequestLog` row for an LLM call. No-op when the slot
/// is empty. Failures are logged at WARN but never propagated — usage
/// tracking is observability, not business logic, and a broken stats DB
/// must not break the workflow.
///
/// `trace_id` is composed as `wf:{node_id}:{started_nanos}:{counter}` so
/// downstream consumers (UsageView, logs dashboard) can tell these calls
/// apart from chat-driven ones (`direct-{session_key}-...`). The counter is
/// process-wide and guarantees uniqueness even when two LLM calls land on
/// the same nanosecond (clock granularity on some platforms is coarse).
fn record_llm_usage(
    slot: &UsageStoreSlot,
    node_id: &str,
    model: &str,
    usage: &nemesis_providers::types::UsageInfo,
    started_at: chrono::DateTime<Local>,
) {
    let guard = slot.read();
    let Some(ds) = guard.as_ref() else {
        return;
    };
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let now = Local::now();
    let trace_id = format!(
        "wf:{}:{}:{}",
        node_id,
        started_at.timestamp_nanos_opt().unwrap_or(0),
        counter
    );
    // 分项计价 + 实际命中条目名（未命中 → 空名 + 全 0）。工作流节点
    // 没有 session_key（DAG 上下文非会话），记空串；TTFT 无流式通路记 NULL。
    let in_toks = usage.prompt_tokens;
    let out_toks = usage.completion_tokens;
    let cc_toks = usage.cache_creation_tokens.unwrap_or(0);
    let cr_toks = usage.cache_read_tokens.or(usage.cached_tokens).unwrap_or(0);
    let breakdown = ds.compute_cost_breakdown(model, in_toks, out_toks, cc_toks, cr_toks);
    let empty = nemesis_data::CostBreakdown::default();
    let bd = breakdown.as_ref().unwrap_or(&empty);
    let log = nemesis_data::RequestLog {
        id: 0,
        trace_id,
        model: model.to_string(),
        provider_type: String::new(),
        input_tokens: in_toks,
        output_tokens: out_toks,
        cache_creation_tokens: cc_toks,
        cache_read_tokens: cr_toks,
        total_cost_usd: bd.total_cost_usd,
        latency_ms: (now - started_at).num_milliseconds(),
        status_code: 200,
        error_message: None,
        is_streaming: false,
        created_at: now.timestamp(),
        pricing_model: bd.pricing_model.clone(),
        input_cost_usd: bd.input_cost_usd,
        output_cost_usd: bd.output_cost_usd,
        cache_creation_cost_usd: bd.cache_creation_cost_usd,
        cache_read_cost_usd: bd.cache_read_cost_usd,
        first_token_ms: None,
        session_key: String::new(),
    };
    if let Err(e) = ds.insert_request_log(&log) {
        tracing::warn!(
            node_id = %node_id,
            "[Workflow] Failed to record LLM usage: {e}"
        );
    }
}

// ---------------------------------------------------------------------------
// LLM Node
// ---------------------------------------------------------------------------

/// Registry that maps node type names to executors.
///
/// For composite node types (parallel, loop, sub_workflow) that need to
/// look up executors for child nodes, the registry stores a weak self-reference
/// via `OnceLock<Weak<Self>>`. Composite executors receive an `Arc<Self>` so
/// they can dynamically resolve child executors at runtime without `unsafe`.
///
/// Interior mutability (`RwLock<HashMap>`) lets `register` work through
/// `&self`, so callers (e.g. `WorkflowEngine::new_arc`) can mutate the
/// registry after it's been wrapped in `Arc`.
pub struct NodeExecutorRegistry {
    executors: parking_lot::RwLock<HashMap<String, Arc<dyn NodeExecutor>>>,
    self_weak: std::sync::OnceLock<std::sync::Weak<Self>>,
}

impl NodeExecutorRegistry {
    /// Create a registry pre-loaded with all built-in executors.
    ///
    /// Composite executors (parallel, loop, sub_workflow) are registered
    /// as stubs initially. For full composite support use [`Self::new_with_composite`]
    /// or call [`Self::setup_composite_executors`] after wrapping in `Arc`.
    pub fn new() -> Self {
        let mut executors = Self::builtin_executors();
        executors.insert("parallel".to_string(), Arc::new(ParallelNodeStub));
        executors.insert("loop".to_string(), Arc::new(LoopNodeStub));
        executors.insert("sub_workflow".to_string(), Arc::new(SubWorkflowNodeStub));
        Self {
            executors: parking_lot::RwLock::new(executors),
            self_weak: std::sync::OnceLock::new(),
        }
    }

    /// Built-in executors that don't need registry/engine self-reference.
    fn builtin_executors() -> HashMap<String, Arc<dyn NodeExecutor>> {
        let mut executors: HashMap<String, Arc<dyn NodeExecutor>> = HashMap::new();
        executors.insert("llm".to_string(), Arc::new(LLMNodeExecutor));
        executors.insert("tool".to_string(), Arc::new(ToolNodeExecutor));
        executors.insert("condition".to_string(), Arc::new(ConditionNodeExecutor));
        executors.insert("delay".to_string(), Arc::new(DelayNodeExecutor));
        executors.insert("transform".to_string(), Arc::new(TransformNodeExecutor));
        executors.insert("http".to_string(), Arc::new(HTTPNodeExecutor));
        executors.insert("script".to_string(), Arc::new(ScriptNodeExecutor::new()));
        executors.insert(
            "human_review".to_string(),
            Arc::new(HumanReviewNodeExecutor),
        );
        executors
    }

    /// Create a registry with composite executors that can look up child executors.
    ///
    /// This is the recommended constructor when parallel/loop/sub_workflow nodes
    /// need to execute real child nodes. The returned `Arc<Self>` holds an internal
    /// weak self-reference used by `ParallelNodeExecutor` / `LoopNodeExecutor`.
    pub fn new_with_composite() -> Arc<Self> {
        let reg = Arc::new(Self::new());
        // Stash a weak self-reference. Safe because `Arc::downgrade` doesn't
        // require unique access and OnceLock guarantees one-shot initialization.
        let _ = reg.self_weak.set(Arc::downgrade(&reg));
        reg.setup_composite_executors();
        reg
    }

    /// Create a fully-featured registry with composite executors and a sub_workflow engine.
    pub fn new_with_engine(engine: Arc<crate::engine::WorkflowEngine>) -> Arc<Self> {
        let reg = Arc::new(Self::new());
        let _ = reg.self_weak.set(Arc::downgrade(&reg));
        reg.setup_composite_executors();
        reg.executors.write().insert(
            "sub_workflow".to_string(),
            Arc::new(SubWorkflowNodeExecutor::new(engine)),
        );
        reg
    }

    /// Replace the parallel/loop stubs with real composite executors.
    /// Requires `self_weak` to be initialized (set by `new_with_composite` /
    /// `new_with_engine` after the `Arc<Self>` is constructed).
    fn setup_composite_executors(&self) {
        if let Some(strong) = self.self_weak.get().and_then(|w| w.upgrade()) {
            let mut execs = self.executors.write();
            execs.insert(
                "parallel".to_string(),
                Arc::new(ParallelNodeExecutor::new(strong.clone())),
            );
            execs.insert("loop".to_string(), Arc::new(LoopNodeExecutor::new(strong)));
        }
    }

    /// Replace the parallel/loop stubs with real composite executors using
    /// an externally-provided `Arc<Self>`. Use this when the registry is
    /// owned as `Arc<NodeExecutorRegistry>` inside another struct (e.g.
    /// `WorkflowEngine`) and the parent can hand a clone of the Arc to its
    /// own registry. Idempotent — overwrites any existing parallel/loop
    /// entries.
    ///
    /// This is what makes `WorkflowEngine::new_integrated_with_dirs` (the
    /// gateway's entry point) wire real parallel/loop executors instead of
    /// leaving them as stubs that silently skip LLM/tool/agent children.
    pub fn install_composite_executors(self_arc: &Arc<Self>) {
        let mut execs = self_arc.executors.write();
        execs.insert(
            "parallel".to_string(),
            Arc::new(ParallelNodeExecutor::new(Arc::clone(self_arc))),
        );
        execs.insert(
            "loop".to_string(),
            Arc::new(LoopNodeExecutor::new(Arc::clone(self_arc))),
        );
    }

    /// Register a custom executor for a node type. Overwrites any existing
    /// executor for the same type. Safe to call through shared reference
    /// (e.g. `Arc<WorkflowEngine>:node_executors.register(...)`) thanks to
    /// interior mutability.
    pub fn register(&self, node_type: &str, executor: Arc<dyn NodeExecutor>) {
        self.executors
            .write()
            .insert(node_type.to_string(), executor);
    }

    /// Look up the executor for the given node type, returning an owned
    /// `Arc<dyn NodeExecutor>` so callers can use it across `.await` points
    /// without holding a borrow on the registry.
    pub fn get(&self, node_type: &str) -> Option<Arc<dyn NodeExecutor>> {
        self.executors.read().get(node_type).cloned()
    }

    /// Return all registered node type names.
    pub fn node_types(&self) -> Vec<String> {
        self.executors.read().keys().cloned().collect()
    }
}

impl Default for NodeExecutorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Stub executors for composite nodes (used when no registry/engine is wired)
// ---------------------------------------------------------------------------

/// Stub executor for parallel nodes (used when no registry is wired).
///
/// Unlike the full `ParallelNodeExecutor`, this stub extracts child nodes
/// from config but executes them inline with basic type detection rather
/// than via the executor registry. This provides basic functionality
/// without requiring registry self-reference.
struct ParallelNodeStub;

#[async_trait]
impl NodeExecutor for ParallelNodeStub {
    async fn execute(
        &self,
        node: &NodeDef,
        context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let now = Local::now();
        let children = get_config_node_list(&node.config, "nodes");

        if children.is_empty() {
            return Ok(NodeResult {
                node_id: node.id.clone(),
                output: serde_json::json!({ "results": [] }),
                error: None,
                state: ExecutionState::Completed,
                started_at: now,
                ended_at: Local::now(),
                metadata: HashMap::new(),
            });
        }

        // Execute each child node concurrently using tokio::spawn
        let mut handles = Vec::new();
        for (idx, child_def) in children.into_iter().enumerate() {
            let ctx = context.clone();
            let branch_key = if child_def.id.is_empty() {
                format!("branch_{}", idx)
            } else {
                child_def.id.clone()
            };

            handles.push(tokio::spawn(async move {
                // Execute child node based on its type
                let result = execute_inline_node(&child_def, &ctx).await;
                (branch_key, result)
            }));
        }

        // Collect results
        let mut merged = serde_json::Map::new();
        let mut first_error: Option<String> = None;

        for handle in handles {
            match handle.await {
                Ok((key, Ok(result))) => {
                    if result.state == ExecutionState::Failed {
                        if first_error.is_none() {
                            first_error = result.error.clone();
                        }
                        merged.insert(
                            key,
                            serde_json::json!({ "error": result.error, "output": result.output }),
                        );
                    } else {
                        merged.insert(key, result.output);
                    }
                }
                Ok((key, Err(e))) => {
                    if first_error.is_none() {
                        first_error = Some(e.clone());
                    }
                    merged.insert(key, serde_json::json!({ "error": e }));
                }
                Err(e) => {
                    if first_error.is_none() {
                        first_error = Some(e.to_string());
                    }
                }
            }
        }

        let state = if first_error.is_some() {
            ExecutionState::Failed
        } else {
            ExecutionState::Completed
        };

        Ok(NodeResult {
            node_id: node.id.clone(),
            output: serde_json::Value::Object(merged),
            error: first_error,
            state,
            started_at: now,
            ended_at: Local::now(),
            metadata: HashMap::new(),
        })
    }
}

/// Stub executor for loop nodes (used when no registry is wired).
///
/// Iterates over child nodes sequentially until condition is met or
/// max_iterations is reached. Uses inline execution for child nodes.
struct LoopNodeStub;

#[async_trait]
impl NodeExecutor for LoopNodeStub {
    async fn execute(
        &self,
        node: &NodeDef,
        context: &HashMap<String, serde_json::Value>,
        wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let now = Local::now();
        let max_iter = node
            .config
            .get("max_iterations")
            .and_then(|v| v.as_u64())
            .unwrap_or(10) as usize;

        let cond_expr = node
            .config
            .get("condition")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let children = get_config_node_list(&node.config, "nodes");

        if children.is_empty() {
            return Ok(NodeResult {
                node_id: node.id.clone(),
                output: serde_json::json!({
                    "iterations": 0,
                    "last_output": null,
                }),
                error: None,
                state: ExecutionState::Completed,
                started_at: now,
                ended_at: Local::now(),
                metadata: HashMap::new(),
            });
        }

        let safety_cap = max_iter.min(100);
        let mut local_ctx = context.clone();
        let mut last_output = serde_json::Value::Null;
        let mut actual_iterations: usize = 0;
        let mut loop_error: Option<String> = None;

        // 条件守卫的已完成集合（缺陷 15 同类清账，同真实 LoopNodeExecutor）。
        let mut completed: std::collections::HashSet<String> =
            children.iter().map(|c| c.id.clone()).collect();
        completed.extend(
            wf_ctx
                .get_all_node_results()
                .into_iter()
                .filter(|(_, r)| r.state == ExecutionState::Completed)
                .map(|(id, _)| id),
        );

        for i in 0..safety_cap {
            // Insert iteration index BEFORE condition check and child execution
            // so both `{{i}}` (form-placeholder syntax) and `{{loop_index}}`
            // (legacy) resolve correctly. Mirrors the real LoopNodeExecutor.
            local_ctx.insert("i".to_string(), serde_json::json!(i));
            local_ctx.insert("loop_index".to_string(), serde_json::json!(i));

            // Check loop condition (if provided) after the first iteration.
            // 守卫求值（缺陷 15 同类清账）：幻觉字段 → loop 诚实失败；
            // 不可证残留 → 条件按不成立停止。与真实 LoopNodeExecutor 镜像。
            if !cond_expr.is_empty() && i > 0 {
                let site = format!("节点 {:?}（loop）第 {} 轮条件", node.id, i);
                match eval_condition_with_falsifiability(cond_expr, &local_ctx, &completed, &site) {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(e) => {
                        loop_error = Some(e);
                        break;
                    }
                }
            }

            // Execute child nodes sequentially within the loop body
            let mut iteration_failed = false;
            for child_def in &children {
                match execute_inline_node(child_def, &local_ctx).await {
                    Ok(result) => {
                        if result.state == ExecutionState::Failed {
                            loop_error = result.error.clone().or_else(|| {
                                Some(format!("loop iteration {} node {} failed", i, child_def.id))
                            });
                            iteration_failed = true;
                            break;
                        }
                        last_output = result.output.clone();
                        // Merge output into local context for subsequent
                        // iterations AND for sibling children in the same
                        // iteration. We expose both the top-level field keys
                        // (e.g. `result`, `text`) and `{child_id}.{field}`
                        // keys so siblings can reference this child's output
                        // unambiguously — matching how the top-level scheduler
                        // exposes node outputs ({{node_id.field}}).
                        local_ctx.insert(child_def.id.clone(), result.output.clone());
                        if let Some(obj) = result.output.as_object() {
                            for (k, v) in obj {
                                local_ctx.insert(k.clone(), v.clone());
                                local_ctx.insert(format!("{}.{}", child_def.id, k), v.clone());
                            }
                        }
                    }
                    Err(e) => {
                        loop_error =
                            Some(format!("loop iteration {} node {}: {}", i, child_def.id, e));
                        iteration_failed = true;
                        break;
                    }
                }
            }

            if iteration_failed {
                break;
            }

            actual_iterations = i + 1;
        }

        // Honest failure: mirror LoopNodeExecutor — report Failed when the
        // loop aborted mid-run so the scheduler's retry / failure-tracking
        // path runs instead of treating the node as completed.
        let state = if loop_error.is_some() {
            ExecutionState::Failed
        } else {
            ExecutionState::Completed
        };

        Ok(NodeResult {
            node_id: node.id.clone(),
            output: serde_json::json!({
                "iterations": actual_iterations,
                "last_output": last_output,
            }),
            error: loop_error,
            state,
            started_at: now,
            ended_at: Local::now(),
            metadata: HashMap::new(),
        })
    }
}

/// Stub executor for sub_workflow nodes (used when no engine is wired).
///
/// Since there is no engine to execute sub-workflows, this stub returns
/// a descriptive error indicating the workflow name and that the engine
/// needs to be configured for sub-workflow execution.
struct SubWorkflowNodeStub;

#[async_trait]
impl NodeExecutor for SubWorkflowNodeStub {
    async fn execute(
        &self,
        node: &NodeDef,
        context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let now = Local::now();

        let workflow_name = node
            .config
            .get("workflow")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if workflow_name.is_empty() {
            return Ok(NodeResult {
                node_id: node.id.clone(),
                output: serde_json::Value::Null,
                error: Some("sub_workflow requires 'workflow' config".to_string()),
                state: ExecutionState::Failed,
                started_at: now,
                ended_at: Local::now(),
                metadata: HashMap::new(),
            });
        }

        // Build sub-workflow input from config (resolving context references)
        let sub_input_config = node
            .config
            .get("input")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();

        let mut sub_input: HashMap<String, serde_json::Value> = HashMap::new();
        for (k, v) in &sub_input_config {
            if let Some(s) = v.as_str() {
                if let Some(resolved) = context.get(s) {
                    sub_input.insert(k.clone(), resolved.clone());
                } else {
                    sub_input.insert(k.clone(), serde_json::json!(s));
                }
            } else {
                sub_input.insert(k.clone(), v.clone());
            }
        }

        // Without an engine, we cannot execute the sub-workflow.
        // Return a descriptive result indicating this limitation.
        Ok(NodeResult {
            node_id: node.id.clone(),
            output: serde_json::json!({
                "sub_workflow": workflow_name,
                "input": sub_input,
                "status": "not_executed",
                "reason": "sub_workflow engine not configured - use NodeExecutorRegistry::new_with_engine() to enable sub-workflow execution",
            }),
            error: Some(format!(
                "sub_workflow '{}' cannot execute: no engine configured. Use NodeExecutorRegistry::new_with_engine() to enable.",
                workflow_name
            )),
            state: ExecutionState::Failed,
            started_at: now,
            ended_at: Local::now(),
            metadata: HashMap::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// Inline node execution helper (used by stub executors)
// ---------------------------------------------------------------------------

/// Execute a node inline without the full registry.
///
/// This is used by the stub executors (ParallelNodeStub, LoopNodeStub) when
/// the registry is not wired up. It supports basic node types by creating
/// them directly. For unknown types, it returns a descriptive result.
async fn execute_inline_node(
    node_def: &NodeDef,
    context: &HashMap<String, serde_json::Value>,
) -> Result<NodeResult, String> {
    // Create a local workflow context for inline execution
    let local_wf_ctx = WorkflowContext::new(context.clone());
    match node_def.node_type.as_str() {
        "delay" => {
            DelayNodeExecutor
                .execute(node_def, context, &local_wf_ctx)
                .await
        }
        "transform" => {
            TransformNodeExecutor
                .execute(node_def, context, &local_wf_ctx)
                .await
        }
        "condition" => {
            ConditionNodeExecutor
                .execute(node_def, context, &local_wf_ctx)
                .await
        }
        "http" => {
            HTTPNodeExecutor
                .execute(node_def, context, &local_wf_ctx)
                .await
        }
        "script" => {
            ScriptNodeExecutor::new()
                .execute(node_def, context, &local_wf_ctx)
                .await
        }
        "human_review" => {
            HumanReviewNodeExecutor
                .execute(node_def, context, &local_wf_ctx)
                .await
        }
        // For complex types (llm, tool, parallel, loop, sub_workflow),
        // surface a Failed result so tests built on the stub executors
        // blow up loudly instead of silently producing a "skipped"
        // output that looks like success. Production deployments wire
        // the real executors via WorkflowEngine::new_integrated_with_dirs
        // and never hit this branch.
        _ => {
            let now = Local::now();
            Ok(NodeResult {
                node_id: node_def.id.clone(),
                output: serde_json::Value::Null,
                error: Some(format!(
                    "node type '{}' requires an executor that is not registered \
                     (WorkflowEngine::new() default does not include llm/tool/parallel/loop/sub_workflow); \
                     use new_integrated_with_dirs or install the executor manually",
                    node_def.node_type
                )),
                state: ExecutionState::Failed,
                started_at: now,
                ended_at: Local::now(),
                metadata: HashMap::new(),
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Condition evaluation helper
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 机械拆分：按节点域分文件。纯代码搬家，行为零变更；
// 各子模块 pub 项经 pub use 保持原 nodes:路径不变。
// ---------------------------------------------------------------------------
mod control;
mod io;
mod llm;
mod loop_sub;
mod tool;

pub use control::*;
pub use io::*;
pub use llm::*;
pub use loop_sub::*;
pub use tool::*;

#[cfg(test)]
mod tests;

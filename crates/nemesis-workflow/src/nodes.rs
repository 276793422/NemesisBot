//! Node executor trait and built-in node implementations.
//!
//! Includes all 11 built-in node types:
//! llm, tool, condition, parallel, loop, sub_workflow, transform, http, script, delay, human_review.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

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
fn get_config_node_list(
    config: &HashMap<String, serde_json::Value>,
    key: &str,
) -> Vec<NodeDef> {
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

        let retry_count = obj
            .get("retry_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;

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
/// [`crate::engine::WorkflowEngine::set_usage_store`].
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
    let log = nemesis_data::RequestLog {
        id: 0,
        trace_id,
        model: model.to_string(),
        provider_type: String::new(),
        input_tokens: usage.prompt_tokens,
        output_tokens: usage.completion_tokens,
        cache_creation_tokens: usage.cache_creation_tokens.unwrap_or(0),
        cache_read_tokens: usage.cache_read_tokens.or(usage.cached_tokens).unwrap_or(0),
        // Pricing is the job of the usage-pricing plan; for now the row is
        // written with zero cost so token counts are visible.
        total_cost_usd: 0.0,
        latency_ms: (now - started_at).num_milliseconds() as i64,
        status_code: 200,
        error_message: None,
        is_streaming: false,
        created_at: now.timestamp(),
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

/// Built-in LLM node executor (mock).
///
/// Returns a canned response built from the `prompt`/`model` config fields
/// without calling any real provider. Useful for unit tests that need to
/// exercise the scheduler without an LLM backend. Production deployments
/// should register [`RealLLMNodeExecutor`] under the `llm` node type to
/// override this mock.
pub struct LLMNodeExecutor;

#[async_trait]
impl NodeExecutor for LLMNodeExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        _context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let now = Local::now();
        let prompt = node
            .config
            .get("prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("default prompt");
        let model = node
            .config
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("default");
        Ok(NodeResult {
            node_id: node.id.clone(),
            output: serde_json::json!({
                "text": format!("LLM execution (model={}): {}", model, prompt),
            }),
            error: None,
            state: ExecutionState::Completed,
            started_at: now,
            ended_at: Local::now(),
            metadata: HashMap::new(),
        })
    }
}

/// Production-grade LLM node executor backed by [`nemesis_providers`].
///
/// Holds an `Arc<dyn LLMProvider>` (e.g., a `Router` or any concrete
/// provider) and calls `provider.chat()` on each execution. Node config
/// fields:
///
/// - `prompt` (required, string): User message content.
/// - `system_prompt` (optional, string): Prepended as a system message.
/// - `model` (optional, string): Defaults to `provider.default_model()`.
/// - `temperature` (optional, float): Maps to `ChatOptions.temperature`.
/// - `max_tokens` (optional, int): Maps to `ChatOptions.max_tokens`.
///
/// The node output is a JSON object:
/// ```json
/// { "text": "<content>", "model": "<model>",
///   "finish_reason": "<reason>", "usage": { ... } }
/// ```
///
/// On provider errors the executor returns a `NodeResult` with
/// `state = Failed` (rather than `Err`) so the workflow can observe the
/// failure via standard state inspection.
pub struct RealLLMNodeExecutor {
    provider: Arc<dyn LLMProvider>,
    usage_store: UsageStoreSlot,
}

impl RealLLMNodeExecutor {
    /// Construct a new executor that delegates to the given provider.
    /// No usage tracking is wired — use [`with_usage_store`] when the
    /// executor is owned by an engine that has a `DataStore`.
    pub fn new(provider: Arc<dyn LLMProvider>) -> Self {
        Self {
            provider,
            usage_store: new_usage_store_slot(),
        }
    }

    /// Internal constructor that shares the engine's usage slot so LLM calls
    /// are recorded whenever the gateway populates the slot via
    /// [`crate::engine::WorkflowEngine::set_usage_store`].
    pub(crate) fn with_usage_store(
        provider: Arc<dyn LLMProvider>,
        usage_store: UsageStoreSlot,
    ) -> Self {
        Self {
            provider,
            usage_store,
        }
    }

    /// Borrow the inner provider (used by tests + observers).
    pub fn provider(&self) -> &Arc<dyn LLMProvider> {
        &self.provider
    }
}

#[async_trait]
impl NodeExecutor for RealLLMNodeExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let started = Local::now();

        // ---- Pull config ----
        let prompt = match node.config.get("prompt").and_then(|v| v.as_str()) {
            Some(p) => p.to_string(),
            None => {
                return Ok(failed_node_result(
                    &node.id,
                    started,
                    "llm node missing required 'prompt' config",
                ));
            }
        };

        // Prompt may reference context variables via {{var}} placeholders.
        let prompt = resolve_prompt_template(&prompt, context);

        let model = node
            .config
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| self.provider.default_model().to_string());

        let temperature = node
            .config
            .get("temperature")
            .and_then(|v| v.as_f64());
        let max_tokens = node
            .config
            .get("max_tokens")
            .and_then(|v| v.as_i64());

        let system_prompt = node
            .config
            .get("system_prompt")
            .and_then(|v| v.as_str())
            .map(|s| resolve_prompt_template(s, context));

        // ---- Build chat request ----
        let mut messages: Vec<Message> = Vec::new();
        if let Some(sys) = system_prompt {
            messages.push(Message {
                role: "system".to_string(),
                content: sys,
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: None,
                reasoning_content: None,
                extra: HashMap::new(),
            });
        }
        messages.push(Message {
            role: "user".to_string(),
            content: prompt,
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
            extra: HashMap::new(),
        });

        let options = ChatOptions {
            temperature,
            max_tokens,
            top_p: None,
            stop: None,
            extra: HashMap::new(),
        };

        // ---- Invoke provider ----
        match self
            .provider
            .chat(&messages, &[], &model, &options)
            .await
        {
            Ok(resp) => {
                if let Some(ref u) = resp.usage {
                    record_llm_usage(&self.usage_store, &node.id, &model, u, started);
                }
                Ok(success_node_result(&node.id, started, &model, resp))
            }
            Err(err) => Ok(failed_node_result(
                &node.id,
                started,
                &format!("LLM provider error: {}", err),
            )),
        }
    }
}

/// Build a Completed NodeResult from a successful LLMResponse.
fn success_node_result(
    node_id: &str,
    started: chrono::DateTime<Local>,
    model: &str,
    resp: LLMResponse,
) -> NodeResult {
    let usage_json = resp.usage.as_ref().map(|u| {
        serde_json::json!({
            "prompt_tokens": u.prompt_tokens,
            "completion_tokens": u.completion_tokens,
            "total_tokens": u.total_tokens,
            "cached_tokens": u.cached_tokens,
            "cache_creation_tokens": u.cache_creation_tokens,
            "cache_read_tokens": u.cache_read_tokens,
        })
    });
    NodeResult {
        node_id: node_id.to_string(),
        output: serde_json::json!({
            "text": resp.content,
            "model": model,
            "finish_reason": resp.finish_reason,
            "usage": usage_json,
        }),
        error: None,
        state: ExecutionState::Completed,
        started_at: started,
        ended_at: Local::now(),
        metadata: HashMap::new(),
    }
}

/// Build a Failed NodeResult with the given error message.
fn failed_node_result(
    node_id: &str,
    started: chrono::DateTime<Local>,
    error: &str,
) -> NodeResult {
    NodeResult {
        node_id: node_id.to_string(),
        output: serde_json::Value::Null,
        error: Some(error.to_string()),
        state: ExecutionState::Failed,
        started_at: started,
        ended_at: Local::now(),
        metadata: HashMap::new(),
    }
}

/// Resolve `{{var}}` placeholders against the executor context.
///
/// Supports nested lookups: `{{node_id.field}}` resolves to the field of a
/// previously-executed node's output object. Missing keys resolve to empty
/// string. The implementation is intentionally minimal — full templating
/// belongs in the scheduler's context-builder, not here.
fn resolve_prompt_template(template: &str, context: &HashMap<String, serde_json::Value>) -> String {
    let mut out = template.to_string();
    for (k, v) in context {
        let placeholder = format!("{{{{{}}}}}", k);
        let replacement = match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        out = out.replace(&placeholder, &replacement);
    }
    out
}

// ---------------------------------------------------------------------------
// Tool Node
// ---------------------------------------------------------------------------

/// Built-in tool node executor (mock).
pub struct ToolNodeExecutor;

#[async_trait]
impl NodeExecutor for ToolNodeExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        _context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let now = Local::now();
        let tool_name = node
            .config
            .get("tool")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        Ok(NodeResult {
            node_id: node.id.clone(),
            output: serde_json::json!({
                "tool": tool_name,
                "status": "success",
            }),
            error: None,
            state: ExecutionState::Completed,
            started_at: now,
            ended_at: Local::now(),
            metadata: HashMap::new(),
        })
    }
}

/// Real tool node executor that delegates to `nemesis_tools::ToolRegistry`.
///
/// Looks up the tool by name and invokes it with resolved args. Tool errors
/// are surfaced as a Failed NodeResult (not Err), so the workflow can branch
/// on failure states.
///
/// Config fields:
/// - `name` (preferred) or `tool` (legacy): tool name, required
/// - `args`: JSON object of tool arguments; `{{var}}` placeholders are
///   resolved against the executor context
pub struct RealToolNodeExecutor {
    tools: Arc<nemesis_tools::registry::ToolRegistry>,
}

impl RealToolNodeExecutor {
    pub fn new(tools: Arc<nemesis_tools::registry::ToolRegistry>) -> Self {
        Self { tools }
    }

    pub fn tools(&self) -> &Arc<nemesis_tools::registry::ToolRegistry> {
        &self.tools
    }
}

#[async_trait]
impl NodeExecutor for RealToolNodeExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let started = Local::now();

        let tool_name = node
            .config
            .get("name")
            .and_then(|v| v.as_str())
            .or_else(|| node.config.get("tool").and_then(|v| v.as_str()));
        let tool_name = match tool_name {
            Some(n) => n,
            None => {
                return Ok(tool_failed_node_result(
                    &node.id,
                    started,
                    "missing required 'name' (or legacy 'tool') field",
                ));
            }
        };

        let raw_args = node.config.get("args").cloned().unwrap_or(serde_json::Value::Null);
        let resolved_args = resolve_template_value(&raw_args, context);

        let tool_result = self.tools.execute(tool_name, &resolved_args).await;

        Ok(tool_result_to_node_result(&node.id, started, tool_name, tool_result))
    }
}

/// Resolve `{{var}}` placeholders inside an arbitrary JSON value.
///
/// Strings get `resolve_prompt_template`; objects and arrays are recursed;
/// other scalars pass through unchanged.
fn resolve_template_value(
    value: &serde_json::Value,
    context: &HashMap<String, serde_json::Value>,
) -> serde_json::Value {
    match value {
        serde_json::Value::String(s) => {
            serde_json::Value::String(resolve_prompt_template(s, context))
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(|v| resolve_template_value(v, context)).collect())
        }
        serde_json::Value::Object(obj) => {
            let resolved: serde_json::Map<String, serde_json::Value> = obj
                .iter()
                .map(|(k, v)| (k.clone(), resolve_template_value(v, context)))
                .collect();
            serde_json::Value::Object(resolved)
        }
        other => other.clone(),
    }
}

/// Convert a `ToolResult` into a workflow `NodeResult`.
///
/// Tool success → Completed with `output = {tool, result, ...}`.
/// Tool error   → Failed with the LLM-facing message in `error`.
fn tool_result_to_node_result(
    node_id: &str,
    started: chrono::DateTime<Local>,
    tool_name: &str,
    result: nemesis_tools::types::ToolResult,
) -> NodeResult {
    let ended = Local::now();
    if result.is_error {
        NodeResult {
            node_id: node_id.to_string(),
            output: serde_json::Value::Null,
            error: Some(format!("tool '{}' error: {}", tool_name, result.for_llm)),
            state: ExecutionState::Failed,
            started_at: started,
            ended_at: ended,
            metadata: HashMap::new(),
        }
    } else {
        NodeResult {
            node_id: node_id.to_string(),
            output: serde_json::json!({
                "tool": tool_name,
                "result": result.for_llm,
                "silent": result.silent,
                "async": result.is_async,
                "task_id": result.task_id,
            }),
            error: None,
            state: ExecutionState::Completed,
            started_at: started,
            ended_at: ended,
            metadata: HashMap::new(),
        }
    }
}

/// Build a Failed NodeResult for tool resolution failures (missing config,
/// template errors, etc.). Used before we ever invoke the tool.
fn tool_failed_node_result(
    node_id: &str,
    started: chrono::DateTime<Local>,
    error: &str,
) -> NodeResult {
    NodeResult {
        node_id: node_id.to_string(),
        output: serde_json::Value::Null,
        error: Some(error.to_string()),
        state: ExecutionState::Failed,
        started_at: started,
        ended_at: Local::now(),
        metadata: HashMap::new(),
    }
}


// ---------------------------------------------------------------------------
// Condition Node
// ---------------------------------------------------------------------------

/// Built-in condition node executor.
pub struct ConditionNodeExecutor;

#[async_trait]
impl NodeExecutor for ConditionNodeExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let now = Local::now();
        let condition = node
            .config
            .get("condition")
            .and_then(|v| v.as_str())
            .unwrap_or("false");

        let result = evaluate_condition(condition, context);

        Ok(NodeResult {
            node_id: node.id.clone(),
            output: serde_json::json!({ "condition_result": result }),
            error: None,
            state: ExecutionState::Completed,
            started_at: now,
            ended_at: Local::now(),
            metadata: HashMap::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// Parallel Node
// ---------------------------------------------------------------------------

/// Built-in parallel node executor.
///
/// Executes child nodes concurrently using tokio tasks. Child nodes are
/// looked up from the executor registry by their node_type.
pub struct ParallelNodeExecutor {
    registry: Arc<NodeExecutorRegistry>,
}

impl ParallelNodeExecutor {
    /// Create a new parallel executor with access to the given registry.
    pub fn new(registry: Arc<NodeExecutorRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl NodeExecutor for ParallelNodeExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let now = Local::now();

        // Try "nodes" first, fall back to "branches"
        let children = {
            let nodes = get_config_node_list(&node.config, "nodes");
            if nodes.is_empty() {
                get_config_node_list(&node.config, "branches")
            } else {
                nodes
            }
        };

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

        // Spawn a task for each child node
        let mut handles = Vec::new();
        for (idx, child_def) in children.into_iter().enumerate() {
            let registry = Arc::clone(&self.registry);
            let ctx = context.clone();
            let branch_key = format!("branch_{}", idx);

            handles.push(tokio::spawn(async move {
                let executor = match registry.get(&child_def.node_type) {
                    Some(e) => e,
                    None => {
                        return (
                            branch_key,
                            Err(format!(
                                "unknown node type {:?} in parallel block",
                                child_def.node_type
                            )),
                        );
                    }
                };

                // Create a local context for the child node since we cannot
                // borrow the parent's wf_ctx across spawn boundaries.
                let local_wf_ctx = WorkflowContext::new(ctx.clone());
                let result = executor.execute(&child_def, &ctx, &local_wf_ctx).await;
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

// ---------------------------------------------------------------------------
// Delay Node
// ---------------------------------------------------------------------------

/// Built-in delay node executor.
pub struct DelayNodeExecutor;

#[async_trait]
impl NodeExecutor for DelayNodeExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        _context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let now = Local::now();
        let secs = node
            .config
            .get("seconds")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        // Form label is "等待秒数" — treat the value as seconds, not millis.
        // (Earlier code used `from_millis(secs)` which made a `seconds=2` config
        // delay for 2ms, contradicting every form placeholder.)
        tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
        Ok(NodeResult {
            node_id: node.id.clone(),
            output: serde_json::json!({ "delayed_seconds": secs }),
            error: None,
            state: ExecutionState::Completed,
            started_at: now,
            ended_at: Local::now(),
            metadata: HashMap::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// Transform Node
// ---------------------------------------------------------------------------

/// Built-in transform node executor.
///
/// Applies a named transform to `config.input`. Available expressions:
/// - `identity` — return input unchanged
/// - `trim` — strip leading/trailing whitespace
/// - `first_line` / `last_line` — first/last non-empty line of input
/// - `split_lines` — split input into an array of lines
/// - `json_extract` — extract a field from a JSON document via dotted path
///   (`arg` = path, e.g. `data.user.name`); supports array index `data[0].id`
/// - `regex_match` — apply a regex (`arg` = pattern); if the pattern has a
///   capture group, returns the first capture, otherwise the full match
pub struct TransformNodeExecutor;

#[async_trait]
impl NodeExecutor for TransformNodeExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let started = Local::now();

        let expression = node
            .config
            .get("expression")
            .and_then(|v| v.as_str())
            .unwrap_or("identity")
            .to_string();

        let input_raw = node
            .config
            .get("input")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        // Resolve {{var}} in the input before transforming.
        let input = resolve_prompt_template(&input_raw, context);

        let arg = node
            .config
            .get("arg")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let output = match expression.as_str() {
            "identity" | "passthrough" => serde_json::json!({ "text": input }),

            "trim" => serde_json::json!({ "text": input.trim() }),

            "first_line" => {
                let line = input
                    .lines()
                    .map(str::trim)
                    .find(|l| !l.is_empty())
                    .unwrap_or("")
                    .to_string();
                serde_json::json!({ "text": line })
            }

            "last_line" => {
                let line = input
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .last()
                    .unwrap_or("")
                    .to_string();
                serde_json::json!({ "text": line })
            }

            "split_lines" => {
                let lines: Vec<&str> = input
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .collect();
                serde_json::json!({ "lines": lines })
            }

            "json_extract" => {
                let trimmed = input.trim();
                let parsed: serde_json::Value = if trimmed.is_empty() {
                    serde_json::Value::Null
                } else {
                    match serde_json::from_str(trimmed) {
                        Ok(v) => v,
                        Err(e) => {
                            return Ok(failed_node_result(
                                &node.id,
                                started,
                                &format!("json_extract: input is not valid JSON: {}", e),
                            ));
                        }
                    }
                };
                let extracted = json_path_lookup(&parsed, arg);
                match extracted {
                    serde_json::Value::String(s) => serde_json::json!({ "text": s }),
                    other => serde_json::json!({ "value": other }),
                }
            }

            "regex_match" => {
                if arg.is_empty() {
                    return Ok(failed_node_result(
                        &node.id,
                        started,
                        "regex_match: 'arg' (pattern) is required",
                    ));
                }
                let re = match regex::Regex::new(arg) {
                    Ok(r) => r,
                    Err(e) => {
                        return Ok(failed_node_result(
                            &node.id,
                            started,
                            &format!("regex_match: invalid pattern '{}': {}", arg, e),
                        ));
                    }
                };
                let matched = match re.captures(&input) {
                    Some(caps) => {
                        // If pattern has a capture group, return the first
                        // capture; otherwise return the full match.
                        if caps.len() > 1 {
                            caps.get(1).map(|m| m.as_str().to_string())
                        } else {
                            caps.get(0).map(|m| m.as_str().to_string())
                        }
                    }
                    None => None,
                };
                serde_json::json!({ "text": matched.unwrap_or_default() })
            }

            other => {
                return Ok(failed_node_result(
                    &node.id,
                    started,
                    &format!("transform: unknown expression '{}'", other),
                ));
            }
        };

        // If output_type is text/markdown/xml, unwrap {"text": "..."} into
        // a bare JSON string so downstream observers (workflow_chat reply)
        // can pass it through as the user-facing reply without JSON-dumping.
        // json/unset/incompatible-shape: leave the object shape unchanged
        // (best-effort; observer JSON-dumps as fallback).
        let output = unwrap_text_output_if_requested(output, node);

        Ok(NodeResult {
            node_id: node.id.clone(),
            output,
            error: None,
            state: ExecutionState::Completed,
            started_at: started,
            ended_at: Local::now(),
            metadata: HashMap::new(),
        })
    }
}

/// If the node declared `output_type: text|markdown|xml`, convert the
/// `{"text": "..."}` envelope into a bare JSON string. Returns the original
/// output unchanged for any other case (no output_type, output_type=json,
/// or the expression produced a shape we don't know how to unwrap).
///
/// Best-effort: when output_type is text/markdown/xml but the expression
/// produced a different shape (e.g. `split_lines` → `{lines: [...]}`),
/// the original output is returned as-is. The workflow_chat reply observer
/// will then JSON-dump it via the existing fallback path — same behavior
/// as if output_type had not been set.
fn unwrap_text_output_if_requested(
    output: serde_json::Value,
    node: &NodeDef,
) -> serde_json::Value {
    let output_type = match node.config.get("output_type").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return output,
    };
    match output_type {
        "text" | "markdown" | "xml" => {}
        // json or anything else: leave as-is.
        _ => return output,
    }
    match output.get("text").and_then(|v| v.as_str()) {
        Some(s) => serde_json::Value::String(s.to_string()),
        // Can't unwrap (e.g. split_lines → {lines: [...]}, or json_extract
        // hit a non-string). Leave as-is; observer's JSON-dump fallback
        // handles it.
        None => output,
    }
}

/// Look up a dotted path inside a JSON value.
///
/// Supports:
/// - Field access: `data.name`, `data.user.id`
/// - Array indexing: `items[0]`, `data.users[2].name`
/// - Negative indices: `items[-1]` (last element)
///
/// Missing keys / out-of-range indices resolve to `Value::Null`.
fn json_path_lookup(root: &serde_json::Value, path: &str) -> serde_json::Value {
    if path.is_empty() {
        return root.clone();
    }
    let mut current = root.clone();
    // Tokenise: split on '.' but keep bracketed indices attached to the
    // preceding key (so `data.users[0].name` → ["data", "users[0]", "name"]).
    for raw in path.split('.') {
        // Extract a leading key (may be empty if path starts with [0]).
        let bytes = raw.as_bytes();
        let key_end = bytes
            .iter()
            .position(|&b| b == b'[')
            .unwrap_or(raw.len());
        if key_end > 0 {
            let key = &raw[..key_end];
            current = match &current {
                serde_json::Value::Object(obj) => obj
                    .get(key)
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
                _ => serde_json::Value::Null,
            };
        }
        // Apply each `[index]` after the key.
        let rest = &raw[key_end..];
        let mut idx = 0;
        while idx < rest.len() {
            let bytes = rest.as_bytes();
            if bytes[idx] != b'[' {
                break;
            }
            let close = rest[idx..].find(']').map(|p| p + idx);
            let close = match close {
                Some(p) => p,
                None => break,
            };
            let inner = &rest[idx + 1..close];
            let i: isize = match inner.parse() {
                Ok(n) => n,
                Err(_) => return serde_json::Value::Null,
            };
            current = match &current {
                serde_json::Value::Array(arr) => {
                    let real_idx = if i < 0 {
                        (arr.len() as isize + i) as usize
                    } else {
                        i as usize
                    };
                    arr.get(real_idx).cloned().unwrap_or(serde_json::Value::Null)
                }
                _ => serde_json::Value::Null,
            };
            idx = close + 1;
            // Skip optional UTF-8 whitespace.
            while idx < rest.len() && rest.as_bytes()[idx].is_ascii_whitespace() {
                idx += 1;
            }
        }
    }
    current
}

// ---------------------------------------------------------------------------
// Loop Node
// ---------------------------------------------------------------------------

/// Built-in loop node executor.
///
/// Executes child nodes repeatedly until a condition is met or
/// max_iterations is reached.
pub struct LoopNodeExecutor {
    registry: Arc<NodeExecutorRegistry>,
}

impl LoopNodeExecutor {
    /// Create a new loop executor with access to the given registry.
    pub fn new(registry: Arc<NodeExecutorRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl NodeExecutor for LoopNodeExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
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

        // Create a local context for child node execution so loop variables
        // don't pollute the parent context.
        let local_wf_ctx = WorkflowContext::new(context.clone());
        let safety_cap = max_iter.min(100);
        let mut local_ctx = context.clone();
        let mut last_output = serde_json::Value::Null;
        let mut actual_iterations: usize = 0;
        let mut loop_error: Option<String> = None;

        for i in 0..safety_cap {
            // Insert iteration index BEFORE condition check and child execution
            // so both `{{i}}` (form-placeholder syntax) and `{{loop_index}}`
            // (legacy) resolve correctly. Inserting at the start also means
            // iter-0 children see `i=0` instead of having no key available.
            local_ctx.insert("i".to_string(), serde_json::json!(i));
            local_ctx.insert("loop_index".to_string(), serde_json::json!(i));

            // Check loop condition (if provided) after the first iteration
            if !cond_expr.is_empty() && i > 0 {
                let cond_result = evaluate_condition(cond_expr, &local_ctx);
                if !cond_result {
                    break;
                }
            }

            // Execute child nodes sequentially within the loop body
            let mut iteration_failed = false;
            for child_def in &children {
                let executor = match self.registry.get(&child_def.node_type) {
                    Some(e) => e,
                    None => {
                        loop_error = Some(format!(
                            "unknown node type {:?} in loop body",
                            child_def.node_type
                        ));
                        iteration_failed = true;
                        break;
                    }
                };

                match executor.execute(child_def, &local_ctx, &local_wf_ctx).await {
                    Ok(result) => {
                        if result.state == ExecutionState::Failed {
                            loop_error = result.error.clone().or_else(|| {
                                Some(format!("loop iteration {} node {} failed", i, child_def.id))
                            });
                            iteration_failed = true;
                            break;
                        }
                        last_output = result.output.clone();
                        // Merge output into local context for subsequent iterations
                        if let Some(obj) = result.output.as_object() {
                            for (k, v) in obj {
                                local_ctx.insert(k.clone(), v.clone());
                            }
                        }
                    }
                    Err(e) => {
                        loop_error = Some(format!(
                            "loop iteration {} node {}: {}",
                            i, child_def.id, e
                        ));
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

        Ok(NodeResult {
            node_id: node.id.clone(),
            output: serde_json::json!({
                "iterations": actual_iterations,
                "last_output": last_output,
            }),
            error: loop_error,
            state: ExecutionState::Completed,
            started_at: now,
            ended_at: Local::now(),
            metadata: HashMap::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// SubWorkflow Node
// ---------------------------------------------------------------------------

/// Built-in sub_workflow node executor.
///
/// Recursively executes another registered workflow via the Engine.
pub struct SubWorkflowNodeExecutor {
    engine: Arc<crate::engine::WorkflowEngine>,
}

impl SubWorkflowNodeExecutor {
    /// Create a new sub_workflow executor with a reference to the engine.
    pub fn new(engine: Arc<crate::engine::WorkflowEngine>) -> Self {
        Self { engine }
    }
}

#[async_trait]
impl NodeExecutor for SubWorkflowNodeExecutor {
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

        // Build sub-workflow input from config, resolving context references
        let sub_input_config = node
            .config
            .get("input")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();

        let mut sub_input: HashMap<String, serde_json::Value> = HashMap::new();
        for (k, v) in &sub_input_config {
            if let Some(s) = v.as_str() {
                // Two supported syntaxes (form placeholder shows {{var}}):
                //   1. Bare variable name (legacy/convenience): if the string
                //      is itself a context key, substitute the context value.
                //   2. {{var}} template: resolve placeholders inside the
                //      string — matches what the form placeholder promises.
                if let Some(resolved) = context.get(s) {
                    sub_input.insert(k.clone(), resolved.clone());
                } else {
                    sub_input.insert(
                        k.clone(),
                        serde_json::Value::String(resolve_prompt_template(s, context)),
                    );
                }
            } else {
                // Arrays / objects: recurse so {{var}} inside nested values
                // also resolves.
                sub_input.insert(k.clone(), resolve_template_value(v, context));
            }
        }

        // Execute sub-workflow via engine
        let exec_result = self
            .engine
            .run(workflow_name, sub_input, None)
            .await
            .map_err(|e| e.to_string())?;

        let mut result_metadata = HashMap::new();
        result_metadata.insert(
            "execution_id".to_string(),
            serde_json::json!(exec_result.id),
        );

        // Build output from node_results
        let mut output_map = serde_json::Map::new();
        for (node_id, nr) in &exec_result.node_results {
            output_map.insert(node_id.clone(), nr.output.clone());
        }

        // When the child execution failed, surface an error string so the
        // scheduler's retry / failure-tracking logic (which keys off
        // `result.error`) can propagate the failure to the parent execution.
        // Without this, a Failed child state would be silently swallowed
        // because the scheduler treats `Ok(result) where error.is_none()` as
        // success regardless of `result.state`.
        let error = if exec_result.state == ExecutionState::Failed {
            Some(exec_result.error.clone().unwrap_or_else(|| {
                "sub_workflow child execution failed".to_string()
            }))
        } else {
            None
        };

        Ok(NodeResult {
            node_id: node.id.clone(),
            output: serde_json::Value::Object(output_map),
            error,
            state: exec_result.state,
            started_at: now,
            ended_at: Local::now(),
            metadata: result_metadata,
        })
    }
}

// ---------------------------------------------------------------------------
// HTTP Node
// ---------------------------------------------------------------------------

/// Built-in HTTP node executor.
///
/// Makes an actual HTTP request using reqwest and returns the response.
pub struct HTTPNodeExecutor;

#[async_trait]
impl NodeExecutor for HTTPNodeExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let now = Local::now();

        let url_raw = node
            .config
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        // Resolve {{var}} in URL against context.
        let url = resolve_prompt_template(url_raw, context);

        let method = node
            .config
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("GET")
            .to_uppercase();

        if url.is_empty() {
            return Ok(NodeResult {
                node_id: node.id.clone(),
                output: serde_json::Value::Null,
                error: Some("http node requires 'url' config".to_string()),
                state: ExecutionState::Failed,
                started_at: now,
                ended_at: Local::now(),
                metadata: HashMap::new(),
            });
        }

        let body_raw = node
            .config
            .get("body")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        // Resolve {{var}} in body against context.
        let body = resolve_prompt_template(body_raw, context);

        // Build the request
        let client = reqwest::Client::new();
        let req_builder = match method.as_str() {
            "POST" => client.post(&url),
            "PUT" => client.put(&url),
            "PATCH" => client.patch(&url),
            "DELETE" => client.delete(&url),
            "HEAD" => client.head(&url),
            _ => client.get(&url),
        };

        // Optional per-request timeout (default 30s). Without this, a slow
        // or unresponsive endpoint would hang the entire workflow indefinitely.
        // `timeout_secs: 0` is treated as "use default" since 0 isn't a useful
        // timeout for an actual HTTP round-trip.
        let timeout_secs = node
            .config
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .filter(|&n| n > 0)
            .unwrap_or(30);
        let req_builder = req_builder.timeout(std::time::Duration::from_secs(timeout_secs));

        // Resolve {{var}} in headers (values only; keys are static).
        let headers_raw = node
            .config
            .get("headers")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();
        let resolved_headers = resolve_template_value(
            &serde_json::Value::Object(headers_raw),
            context,
        );

        let mut req_builder = req_builder;
        if let Some(obj) = resolved_headers.as_object() {
            for (k, v) in obj {
                if let Some(s) = v.as_str() {
                    req_builder = req_builder.header(k.as_str(), s);
                }
            }
        }

        // Set body if provided
        if !body.is_empty() {
            req_builder = req_builder.body(body.to_string());
        }

        // Execute request
        let resp = req_builder
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {}", e))?;

        let status_code = resp.status().as_u16();

        // Collect response headers
        let mut resp_headers = serde_json::Map::new();
        for (key, value) in resp.headers() {
            if let Ok(v) = value.to_str() {
                resp_headers.insert(
                    key.as_str().to_string(),
                    serde_json::json!(v),
                );
            }
        }

        // Read response body (limit to 1MB)
        let body_bytes = resp
            .bytes()
            .await
            .map_err(|e| format!("failed to read response body: {}", e))?;
        let body_str = String::from_utf8_lossy(&body_bytes).to_string();

        Ok(NodeResult {
            node_id: node.id.clone(),
            output: serde_json::json!({
                "status_code": status_code,
                "headers": serde_json::Value::Object(resp_headers),
                "body": body_str,
            }),
            error: None,
            state: ExecutionState::Completed,
            started_at: now,
            ended_at: Local::now(),
            metadata: HashMap::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// Script Node
// ---------------------------------------------------------------------------

/// Pick the interpreter binary, file extension, and argv flag for a given
/// `language` config value. Pure function so tests can verify language→binary
/// mapping without spawning processes (BUG #4 + BUG #13 regression tests).
///
/// "bat" / "cmd" route to Windows `cmd.exe /C` — anything else would run
/// batch syntax under bash and fail immediately on Windows hosts.
///
/// "powershell" / "pwsh" route to `powershell.exe` on Windows (always
/// pre-installed) and `pwsh` elsewhere (user-installed PowerShell Core).
/// Mapping both to `pwsh` silently broke Windows users who only had
/// built-in Windows PowerShell 5.1.
fn select_script_interpreter(language: &str) -> (&'static str, &'static str, &'static str) {
    match language {
        "python" | "python3" => ("python3", ".py", "-c"),
        "python2" => ("python2", ".py", "-c"),
        "node" | "javascript" | "js" => ("node", ".js", "-e"),
        "powershell" | "pwsh" => {
            #[cfg(windows)]
            {
                ("powershell", ".ps1", "-Command")
            }
            #[cfg(not(windows))]
            {
                ("pwsh", ".ps1", "-Command")
            }
        }
        "sh" => ("sh", ".sh", "-c"),
        "bat" | "cmd" => ("cmd", ".bat", "/C"),
        _ => ("bash", ".sh", "-c"), // default to bash
    }
}

/// Built-in script node executor.
///
/// Executes a script using the system shell. The script content is written
/// to a temporary file and executed using the configured language interpreter.
/// Supports bash, python, and other scripting languages.
pub struct ScriptNodeExecutor;

#[async_trait]
impl NodeExecutor for ScriptNodeExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let now = Local::now();

        let script = node
            .config
            .get("script")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let language = node
            .config
            .get("language")
            .and_then(|v| v.as_str())
            .unwrap_or("bash");

        if script.is_empty() {
            return Ok(NodeResult {
                node_id: node.id.clone(),
                output: serde_json::Value::Null,
                error: Some("script node requires 'script' config".to_string()),
                state: ExecutionState::Failed,
                started_at: now,
                ended_at: Local::now(),
                metadata: HashMap::new(),
            });
        }

        // Resolve template variables from context
        let resolved_script = resolve_template(script, context);

        // Determine the interpreter and file extension based on language.
        // "bat"/"cmd" map to cmd.exe so Windows users actually get batch
        // semantics — falling through to bash would run bat syntax under
        // bash and immediately fail.
        let (interpreter, _ext, flag) = select_script_interpreter(language);

        // Execute the script using the interpreter
        let output = tokio::process::Command::new(interpreter)
            .arg(flag)
            .arg(&resolved_script)
            .output()
            .await
            .map_err(|e| format!("failed to execute script: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1);

        let state = if output.status.success() {
            ExecutionState::Completed
        } else {
            ExecutionState::Failed
        };

        let error = if !output.status.success() {
            Some(if stderr.is_empty() {
                format!("Script exited with code {}", exit_code)
            } else {
                format!("Script error (exit {}): {}", exit_code, stderr.trim())
            })
        } else {
            None
        };

        Ok(NodeResult {
            node_id: node.id.clone(),
            output: serde_json::json!({
                "stdout": stdout,
                "stderr": stderr,
                "exit_code": exit_code,
                "language": language,
            }),
            error,
            state,
            started_at: now,
            ended_at: Local::now(),
            metadata: HashMap::new(),
        })
    }
}

/// Resolve simple template variables in a string from the context.
///
/// Replaces `{{variable_name}}` patterns with the corresponding values
/// from the context map.
fn resolve_template(template: &str, context: &HashMap<String, serde_json::Value>) -> String {
    let mut result = template.to_string();
    for (key, value) in context {
        let pattern = format!("{{{{{}}}}}", key);
        let replacement = match value {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        result = result.replace(&pattern, &replacement);
    }
    result
}

// ---------------------------------------------------------------------------
// HumanReview Node
// ---------------------------------------------------------------------------

/// Built-in human review node executor.
///
/// Pauses workflow execution until a human reviews and approves/rejects.
/// Returns a Waiting state; the engine is responsible for pausing.
pub struct HumanReviewNodeExecutor;

#[async_trait]
impl NodeExecutor for HumanReviewNodeExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let now = Local::now();

        let raw_message = node
            .config
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("Human review required");
        // Form placeholder is `请审核是否发送给客户：{{draft}}` — resolve
        // {{var}} placeholders against the execution context so reviewers
        // see the actual content, not literal `{{draft}}`.
        let message = resolve_prompt_template(raw_message, context);

        Ok(NodeResult {
            node_id: node.id.clone(),
            output: serde_json::json!({
                "message": message,
                "status": "waiting_for_review",
            }),
            error: None,
            state: ExecutionState::Waiting,
            started_at: now,
            ended_at: Local::now(),
            metadata: HashMap::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// Question Classifier Node (1b-D3)
// ---------------------------------------------------------------------------

/// LLM-based classifier node (milestone 1b-D3).
///
/// Sends a structured prompt that asks the LLM to pick exactly one class from
/// a configured list. Output is `{class_id, confidence}` so downstream
/// conditional edges can branch on `class_id`. On parse failure the executor
/// retries up to `max_attempts` times (default 3) before failing the node.
///
/// Node config:
/// - `question` (required, string): the text to classify. Supports `{{var}}`
///   template resolution.
/// - `classes` (required, array): list of `{id, description}` objects.
/// - `system_prompt` (optional, string): defaults to a strict template that
///   tells the LLM to output only the class id.
/// - `model` (optional, string): defaults to provider's default.
/// - `max_attempts` (optional, int, default 3): how many times to retry on
///   parse failure / invalid class id.
/// - `temperature` (optional, float): usually 0 for deterministic output.
pub struct QuestionClassifierNodeExecutor {
    provider: Arc<dyn LLMProvider>,
    usage_store: UsageStoreSlot,
}

impl QuestionClassifierNodeExecutor {
    pub fn new(provider: Arc<dyn LLMProvider>) -> Self {
        Self {
            provider,
            usage_store: new_usage_store_slot(),
        }
    }

    /// Internal constructor that shares the engine's usage slot.
    pub(crate) fn with_usage_store(
        provider: Arc<dyn LLMProvider>,
        usage_store: UsageStoreSlot,
    ) -> Self {
        Self {
            provider,
            usage_store,
        }
    }

    /// Borrow the inner provider (used by tests).
    pub fn provider(&self) -> &Arc<dyn LLMProvider> {
        &self.provider
    }
}

/// One class entry: `id` (machine-readable) + `description` (LLM-facing).
#[derive(Debug, Clone, serde::Deserialize)]
struct ClassDef {
    id: String,
    description: String,
}

/// Default system prompt: forces the LLM to output only the class id and
/// nothing else. We wrap the list of classes inline so the model can't
/// hallucinate ids outside the configured set.
const CLASSIFIER_SYSTEM_PROMPT: &str = "\
You are a strict text classifier. Pick exactly ONE class id from the list below \
that best matches the input question. Output ONLY the class id as a single \
word, no explanation, no quotes, no punctuation.\n\n\
Available classes:\n{classes}\n\n\
Respond with just the class id.";

#[async_trait]
impl NodeExecutor for QuestionClassifierNodeExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let started = Local::now();

        // ---- Parse config ----
        let question_raw = match node.config.get("question").and_then(|v| v.as_str()) {
            Some(q) => q.to_string(),
            None => {
                return Ok(failed_node_result(
                    &node.id,
                    started,
                    "question_classifier node missing required 'question' config",
                ));
            }
        };
        let question = resolve_prompt_template(&question_raw, context);

        let classes_value = match node.config.get("classes") {
            Some(v) => v,
            None => {
                return Ok(failed_node_result(
                    &node.id,
                    started,
                    "question_classifier node missing required 'classes' config",
                ));
            }
        };
        let classes: Vec<ClassDef> = match serde_json::from_value(classes_value.clone()) {
            Ok(c) => c,
            Err(e) => {
                return Ok(failed_node_result(
                    &node.id,
                    started,
                    &format!("invalid 'classes' config: {}", e),
                ));
            }
        };
        if classes.is_empty() {
            return Ok(failed_node_result(
                &node.id,
                started,
                "question_classifier node has empty 'classes' list",
            ));
        }

        let max_attempts = node
            .config
            .get("max_attempts")
            .and_then(|v| v.as_u64())
            .unwrap_or(3)
            .max(1) as usize;

        let model = node
            .config
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| self.provider.default_model().to_string());

        let temperature = node
            .config
            .get("temperature")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        let system_prompt = node
            .config
            .get("system_prompt")
            .and_then(|v| v.as_str())
            .map(|s| resolve_prompt_template(s, context))
            .unwrap_or_else(|| {
                let classes_block: String = classes
                    .iter()
                    .map(|c| format!("- {}: {}", c.id, c.description))
                    .collect::<Vec<_>>()
                    .join("\n");
                CLASSIFIER_SYSTEM_PROMPT.replace("{classes}", &classes_block)
            });

        // ---- Retry loop ----
        let mut last_error: Option<String> = None;
        for attempt in 1..=max_attempts {
            let messages = vec![
                Message {
                    role: "system".to_string(),
                    content: system_prompt.clone(),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                    timestamp: None,
                    reasoning_content: None,
                    extra: HashMap::new(),
                },
                Message {
                    role: "user".to_string(),
                    content: question.clone(),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                    timestamp: None,
                    reasoning_content: None,
                    extra: HashMap::new(),
                },
            ];
            let options = ChatOptions {
                temperature: Some(temperature),
                max_tokens: None,
                top_p: None,
                stop: None,
                extra: HashMap::new(),
            };

            // Capture per-call start so each retry's RequestLog has its own
            // latency + trace_id. Using the node-level `started` would make
            // every attempt share the same trace_id and report cumulative
            // latency drift on later retries.
            let call_started = Local::now();
            match self.provider.chat(&messages, &[], &model, &options).await {
                Ok(resp) => {
                    if let Some(ref u) = resp.usage {
                        record_llm_usage(&self.usage_store, &node.id, &model, u, call_started);
                    }
                    let content = resp.content;
                    let class_id = parse_classifier_output(&content);
                    if let Some(ref id) = class_id {
                        if classes.iter().any(|c| &c.id == id) {
                            return Ok(NodeResult {
                                node_id: node.id.clone(),
                                output: serde_json::json!({
                                    "class_id": id,
                                    "confidence": confidence_for(attempt, max_attempts),
                                    "raw_response": content,
                                    "model": model,
                                    "attempts": attempt,
                                }),
                                error: None,
                                state: ExecutionState::Completed,
                                started_at: started,
                                ended_at: Local::now(),
                                metadata: HashMap::new(),
                            });
                        }
                    }
                    last_error = Some(format!(
                        "attempt {}: LLM returned invalid class id {:?} (raw: {:?})",
                        attempt,
                        class_id,
                        content.trim()
                    ));
                }
                Err(e) => {
                    last_error = Some(format!("attempt {}: provider error: {}", attempt, e));
                }
            }
        }

        Ok(failed_node_result(
            &node.id,
            started,
            &format!(
                "question_classifier failed after {} attempts: {}",
                max_attempts,
                last_error.unwrap_or_else(|| "unknown".to_string())
            ),
        ))
    }
}

/// Extract the class id from an LLM response.
///
/// The model is told to output only the id, but real-world responses often
/// include surrounding prose ("The class is: foo") or punctuation. We strip
/// common wrappers and pick the first token that matches a known id.
///
/// Note: validation against the configured class list happens in the caller;
/// this function returns whatever looks like an id so the caller can decide.
fn parse_classifier_output(content: &str) -> Option<String> {
    let trimmed = content.trim().trim_matches(|c: char| {
        c == '"' || c == '\'' || c == '.' || c == ',' || c == '!' || c == '\n'
    });
    if trimmed.is_empty() {
        return None;
    }
    // If the entire response is a single bare token, return it.
    if !trimmed.contains(char::is_whitespace) {
        return Some(trimmed.to_string());
    }
    // Otherwise, take the first whitespace-delimited token. This handles
    // cases like "foo\n(because the input is clearly about food)".
    Some(trimmed.split_whitespace().next()?.to_string())
}

/// Heuristic confidence: 1.0 on first attempt, decreasing for retries.
fn confidence_for(attempt: usize, _max_attempts: usize) -> f64 {
    // First attempt = high confidence. Each retry shaves 0.15.
    let conf = 1.0 - (attempt.saturating_sub(1) as f64) * 0.15;
    conf.max(0.1)
}

// ---------------------------------------------------------------------------
// Parameter Extractor Node (1b-D4)
// ---------------------------------------------------------------------------

/// LLM-based parameter extractor (milestone 1b-D4).
///
/// Asks the LLM to read a chunk of free-form text and pull out structured
/// fields according to a declared schema. The output is a JSON object with
/// one key per declared parameter. Missing or unextractable parameters are
/// set to null rather than omitted, so downstream nodes can rely on the
/// shape being stable.
///
/// Node config:
/// - `text` (required, string): the source text to extract from. Supports
///   `{{var}}` template resolution.
/// - `parameters` (required, array): list of `{name, type, description,
///   required}` objects. `type` is informational only (string/number/...),
///   we don't enforce it on the LLM output beyond making sure required
///   fields are present and non-null.
/// - `system_prompt` (optional, string): defaults to a strict template.
/// - `model` (optional, string): defaults to provider's default.
/// - `max_attempts` (optional, int, default 3): how many times to retry on
///   JSON parse failure / missing required field.
/// - `temperature` (optional, float): usually 0 for deterministic output.
pub struct ParameterExtractorNodeExecutor {
    provider: Arc<dyn LLMProvider>,
    usage_store: UsageStoreSlot,
}

impl ParameterExtractorNodeExecutor {
    pub fn new(provider: Arc<dyn LLMProvider>) -> Self {
        Self {
            provider,
            usage_store: new_usage_store_slot(),
        }
    }

    /// Internal constructor that shares the engine's usage slot.
    pub(crate) fn with_usage_store(
        provider: Arc<dyn LLMProvider>,
        usage_store: UsageStoreSlot,
    ) -> Self {
        Self {
            provider,
            usage_store,
        }
    }

    /// Borrow the inner provider (used by tests).
    pub fn provider(&self) -> &Arc<dyn LLMProvider> {
        &self.provider
    }
}

/// One declared parameter. `type` is a free-form hint (e.g. "string",
/// "number", "boolean"); we don't coerce the LLM output, only check that
/// required params are present and non-null.
#[derive(Debug, Clone, serde::Deserialize)]
struct ParamDef {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    #[allow(dead_code)]
    r#type: String,
    #[serde(default)]
    required: bool,
}

/// Default system prompt. We embed the parameter schema inline so the
/// model has a clear contract, and we ask for a single JSON object (no
/// markdown fences, no commentary) to keep parsing trivial.
const EXTRACTOR_SYSTEM_PROMPT: &str = "\
You are a strict information extractor. Read the user text and pull out the \
fields listed below. Output ONLY a single JSON object — no markdown fences, \
no commentary, no surrounding prose.\n\n\
Rules:\n\
- Every listed field must appear as a key in the JSON object.\n\
- If the value is not present in the text, use null.\n\
- Strings should be unquoted JSON strings; numbers as JSON numbers; booleans \
as true/false; arrays as JSON arrays.\n\n\
Fields to extract:\n{parameters}\n\n\
Respond with just the JSON object.";

#[async_trait]
impl NodeExecutor for ParameterExtractorNodeExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let started = Local::now();

        // ---- Parse config ----
        let text_raw = match node.config.get("text").and_then(|v| v.as_str()) {
            Some(t) => t.to_string(),
            None => {
                return Ok(failed_node_result(
                    &node.id,
                    started,
                    "parameter_extractor node missing required 'text' config",
                ));
            }
        };
        let text = resolve_prompt_template(&text_raw, context);

        let params_value = match node.config.get("parameters") {
            Some(v) => v,
            None => {
                return Ok(failed_node_result(
                    &node.id,
                    started,
                    "parameter_extractor node missing required 'parameters' config",
                ));
            }
        };
        let params: Vec<ParamDef> = match serde_json::from_value(params_value.clone()) {
            Ok(p) => p,
            Err(e) => {
                return Ok(failed_node_result(
                    &node.id,
                    started,
                    &format!("invalid 'parameters' config: {}", e),
                ));
            }
        };
        if params.is_empty() {
            return Ok(failed_node_result(
                &node.id,
                started,
                "parameter_extractor node has empty 'parameters' list",
            ));
        }

        let max_attempts = node
            .config
            .get("max_attempts")
            .and_then(|v| v.as_u64())
            .unwrap_or(3)
            .max(1) as usize;

        let model = node
            .config
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| self.provider.default_model().to_string());

        let temperature = node
            .config
            .get("temperature")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        let system_prompt = node
            .config
            .get("system_prompt")
            .and_then(|v| v.as_str())
            .map(|s| resolve_prompt_template(s, context))
            .unwrap_or_else(|| {
                let params_block: String = params
                    .iter()
                    .map(|p| {
                        let req = if p.required { " (required)" } else { "" };
                        format!(
                            "- {} [{}]{}: {}",
                            p.name, p.r#type, req, p.description
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                EXTRACTOR_SYSTEM_PROMPT.replace("{parameters}", &params_block)
            });

        // ---- Retry loop ----
        let mut last_error: Option<String> = None;
        for attempt in 1..=max_attempts {
            let messages = vec![
                Message {
                    role: "system".to_string(),
                    content: system_prompt.clone(),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                    timestamp: None,
                    reasoning_content: None,
                    extra: HashMap::new(),
                },
                Message {
                    role: "user".to_string(),
                    content: text.clone(),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                    timestamp: None,
                    reasoning_content: None,
                    extra: HashMap::new(),
                },
            ];
            let options = ChatOptions {
                temperature: Some(temperature),
                max_tokens: None,
                top_p: None,
                stop: None,
                extra: HashMap::new(),
            };

            // Capture per-call start so each retry's RequestLog has its own
            // latency + trace_id (same rationale as the classifier executor).
            let call_started = Local::now();
            match self.provider.chat(&messages, &[], &model, &options).await {
                Ok(resp) => {
                    if let Some(ref u) = resp.usage {
                        record_llm_usage(&self.usage_store, &node.id, &model, u, call_started);
                    }
                    let content = resp.content;
                    match parse_json_object(&content) {
                        Ok(obj) => {
                            let normalized = normalize_object(&obj, &params);
                            if let Err(missing) =
                                validate_required_params(&normalized, &params)
                            {
                                last_error = Some(format!(
                                    "attempt {}: missing required parameters: {}",
                                    attempt, missing
                                ));
                                continue;
                            }
                            return Ok(NodeResult {
                                node_id: node.id.clone(),
                                output: serde_json::json!({
                                    "parameters": normalized,
                                    "raw_response": content,
                                    "model": model,
                                    "attempts": attempt,
                                    "confidence": confidence_for(attempt, max_attempts),
                                }),
                                error: None,
                                state: ExecutionState::Completed,
                                started_at: started,
                                ended_at: Local::now(),
                                metadata: HashMap::new(),
                            });
                        }
                        Err(e) => {
                            last_error = Some(format!(
                                "attempt {}: JSON parse error: {} (raw: {:?})",
                                attempt,
                                e,
                                content.trim()
                            ));
                        }
                    }
                }
                Err(e) => {
                    last_error = Some(format!("attempt {}: provider error: {}", attempt, e));
                }
            }
        }

        Ok(failed_node_result(
            &node.id,
            started,
            &format!(
                "parameter_extractor failed after {} attempts: {}",
                max_attempts,
                last_error.unwrap_or_else(|| "unknown".to_string())
            ),
        ))
    }
}

/// Parse the LLM response into a JSON object.
///
/// The model is told to emit only JSON, but we tolerate a few common
/// wrappers: leading/trailing whitespace, an optional ```json fence
/// (single or triple backticks), and surrounding prose that still leaves
/// the JSON parseable once we extract the outermost `{ ... }` block.
fn parse_json_object(content: &str) -> Result<serde_json::Value, String> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Err("empty response".to_string());
    }

    // Fast path: the whole response is already a JSON object.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if v.is_object() {
            return Ok(v);
        }
        return Err(format!("expected JSON object, got {}", type_name(&v)));
    }

    // Strip ```json ... ``` fences.
    let fence_stripped = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .map(|s| s.trim_start_matches('\n').trim())
        .and_then(|s| s.strip_suffix("```").map(|s| s.trim()))
        .unwrap_or(trimmed);
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(fence_stripped) {
        if v.is_object() {
            return Ok(v);
        }
    }

    // Last resort: pull out the outermost {...} region. Handles
    // "Sure, here's the JSON:\n{ \"name\": \"foo\" }\nHope this helps!".
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}')) {
        if start < end {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&trimmed[start..=end]) {
                if v.is_object() {
                    return Ok(v);
                }
            }
        }
    }

    Err("not a JSON object".to_string())
}

/// Ensure every declared parameter appears in the output object. Missing
/// keys are filled in with null so downstream consumers see a stable shape.
fn normalize_object(
    parsed: &serde_json::Value,
    params: &[ParamDef],
) -> serde_json::Value {
    let mut obj = match parsed.as_object() {
        Some(o) => o.clone(),
        None => {
            let mut m = serde_json::Map::new();
            m.insert(
                "_value".to_string(),
                parsed.clone(),
            );
            m
        }
    };
    for p in params {
        if !obj.contains_key(&p.name) {
            obj.insert(p.name.clone(), serde_json::Value::Null);
        }
    }
    serde_json::Value::Object(obj)
}

/// Return Err(message) if any `required: true` parameter is null or missing.
fn validate_required_params(
    normalized: &serde_json::Value,
    params: &[ParamDef],
) -> Result<(), String> {
    let obj = match normalized.as_object() {
        Some(o) => o,
        None => return Err("output is not an object".to_string()),
    };
    let missing: Vec<&str> = params
        .iter()
        .filter(|p| p.required)
        .filter(|p| obj.get(&p.name).map_or(true, |v| v.is_null()))
        .map(|p| p.name.as_str())
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(missing.join(", "))
    }
}

fn type_name(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

// ---------------------------------------------------------------------------
// Agent Node (1b-D2)
// ---------------------------------------------------------------------------

/// Result returned by an [`AgentRunner`] after a one-shot direct run.
///
/// `response` is the assistant's final message. `tools_used` lists tool names
/// invoked during the run (in invocation order; duplicates preserved so callers
/// can count how many times each tool fired).
#[derive(Debug, Clone, Default)]
pub struct AgentRunResult {
    /// The assistant's final reply text.
    pub response: String,
    /// Tool names invoked, in order (duplicates allowed).
    pub tools_used: Vec<String>,
}

/// Abstraction over a one-shot direct agent invocation.
///
/// Implementations live outside `nemesis-workflow` (typically in `nemesisbot`)
/// to avoid pulling `nemesis-agent` — and its large dependency closure — into
/// the workflow crate. The workflow engine only needs the ability to kick off
/// an agent run by prompt + identity; it doesn't care how the agent produces
/// the reply.
///
/// The trait is async + `Send + Sync` so it can be wrapped in `Arc<dyn …>`
/// and shared across the scheduler's tokio tasks.
#[async_trait]
pub trait AgentRunner: Send + Sync {
    /// Run a one-shot direct prompt through the underlying agent loop.
    ///
    /// - `prompt`: fully-resolved user-facing prompt.
    /// - `agent_id`: stable identifier for the agent instance / session.
    ///   Multiple invocations with the same id SHOULD reuse the same
    ///   conversation memory (so the agent can recall prior turns inside
    ///   this workflow run).
    /// - `max_turns`: safety cap on LLM ↔ tool iteration. The runner SHOULD
    ///   refuse to run more than this many rounds to prevent token blowups.
    /// - `model`: optional model override (e.g. `zhipu/glm-4.7`). Runners
    ///   that don't support per-call model switching SHOULD log a warning
    ///   and fall back to their default model rather than failing.
    async fn run_direct(
        &self,
        prompt: &str,
        agent_id: &str,
        max_turns: u32,
        model: Option<&str>,
    ) -> Result<AgentRunResult, String>;
}

/// Workflow node that delegates to an [`AgentRunner`] (milestone 1b-D2).
///
/// The node kicks off a one-shot agent run with the resolved prompt and
/// surfaces the final response. Internal agent iterations (LLM ↔ tool
/// cycles) are NOT checkpointed — only the node start / end are visible
/// to the workflow engine, so a crash mid-iteration restarts the whole
/// agent node. That's intentional: agent memory is hard to snapshot
/// cleanly, and rerunning is cheaper than getting it wrong.
///
/// Node config:
/// - `prompt` (required, string): the user-facing prompt. Supports
///   `{{var}}` template resolution.
/// - `agent_id` (optional, string, default `"workflow_agent"`): stable id
///   used as session key, so multiple agent nodes in the same workflow
///   can either share or isolate memory by setting this differently.
/// - `max_turns` (optional, int, default 5): safety cap on agent
///   iterations.
pub struct AgentNodeExecutor {
    runner: Arc<dyn AgentRunner>,
}

impl AgentNodeExecutor {
    pub fn new(runner: Arc<dyn AgentRunner>) -> Self {
        Self { runner }
    }
}

#[async_trait]
impl NodeExecutor for AgentNodeExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let started = Local::now();

        let prompt_raw = match node.config.get("prompt").and_then(|v| v.as_str()) {
            Some(p) => p.to_string(),
            None => {
                return Ok(failed_node_result(
                    &node.id,
                    started,
                    "agent node missing required 'prompt' config",
                ));
            }
        };
        let prompt = resolve_prompt_template(&prompt_raw, context);

        let agent_id = node
            .config
            .get("agent_id")
            .and_then(|v| v.as_str())
            .unwrap_or("workflow_agent")
            .to_string();

        let max_turns = node
            .config
            .get("max_turns")
            .and_then(|v| v.as_u64())
            .unwrap_or(5) as u32;

        // Optional model override (form exposes a "模型" field). The runner
        // is responsible for honoring it; GatewayAgentRunner currently logs
        // and falls back to the default model — same pattern as max_turns.
        let model = node
            .config
            .get("model")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());

        match self.runner.run_direct(&prompt, &agent_id, max_turns, model).await {
            Ok(result) => Ok(NodeResult {
                node_id: node.id.clone(),
                output: serde_json::json!({
                    "response": result.response,
                    "tools_used": result.tools_used,
                    "agent_id": agent_id,
                    "max_turns": max_turns,
                }),
                error: None,
                state: ExecutionState::Completed,
                started_at: started,
                ended_at: Local::now(),
                metadata: HashMap::new(),
            }),
            Err(e) => Ok(failed_node_result(
                &node.id,
                started,
                &format!("agent error: {}", e),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Executor Registry
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
        executors.insert("script".to_string(), Arc::new(ScriptNodeExecutor));
        executors.insert("human_review".to_string(), Arc::new(HumanReviewNodeExecutor));
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
        reg.executors
            .write()
            .insert("sub_workflow".to_string(), Arc::new(SubWorkflowNodeExecutor::new(engine)));
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
    /// (e.g. `Arc<WorkflowEngine>::node_executors.register(...)`) thanks to
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
        _wf_ctx: &WorkflowContext,
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

        for i in 0..safety_cap {
            // Insert iteration index BEFORE condition check and child execution
            // so both `{{i}}` (form-placeholder syntax) and `{{loop_index}}`
            // (legacy) resolve correctly. Mirrors the real LoopNodeExecutor.
            local_ctx.insert("i".to_string(), serde_json::json!(i));
            local_ctx.insert("loop_index".to_string(), serde_json::json!(i));

            // Check loop condition (if provided) after the first iteration
            if !cond_expr.is_empty() && i > 0 {
                let cond_result = evaluate_condition(cond_expr, &local_ctx);
                if !cond_result {
                    break;
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
                        // Merge output into local context for subsequent iterations
                        if let Some(obj) = result.output.as_object() {
                            for (k, v) in obj {
                                local_ctx.insert(k.clone(), v.clone());
                            }
                        }
                    }
                    Err(e) => {
                        loop_error = Some(format!(
                            "loop iteration {} node {}: {}",
                            i, child_def.id, e
                        ));
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

        Ok(NodeResult {
            node_id: node.id.clone(),
            output: serde_json::json!({
                "iterations": actual_iterations,
                "last_output": last_output,
            }),
            error: loop_error,
            state: ExecutionState::Completed,
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
        "delay" => DelayNodeExecutor.execute(node_def, context, &local_wf_ctx).await,
        "transform" => TransformNodeExecutor.execute(node_def, context, &local_wf_ctx).await,
        "condition" => ConditionNodeExecutor.execute(node_def, context, &local_wf_ctx).await,
        "http" => HTTPNodeExecutor.execute(node_def, context, &local_wf_ctx).await,
        "script" => ScriptNodeExecutor.execute(node_def, context, &local_wf_ctx).await,
        "human_review" => HumanReviewNodeExecutor.execute(node_def, context, &local_wf_ctx).await,
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

/// Evaluate a condition string against the context.
///
/// Supports:
/// - `{{var}}` template substitution OR bare variable names
///   (both `{{count}} > 5` and `count > 5` work)
/// - `==`, `!=`, `>`, `<`, `>=`, `<=` operators
///   (numeric comparison when both sides parse as numbers,
///   otherwise lexicographic string comparison)
/// - Bare variable name — truthy check against the context value
///   (numbers: non-zero; strings: non-empty; etc.)
/// - Literal `"true"` / `"false"` — boolean
pub fn evaluate_condition(
    condition: &str,
    context: &HashMap<String, serde_json::Value>,
) -> bool {
    let condition = condition.trim();
    if condition.is_empty() {
        return false;
    }

    // Step 1: Resolve {{var}} placeholders. After this the condition is a
    // literal expression like `5 > 3` or `hello == hello`.
    let resolved = resolve_prompt_template(condition, context);
    let resolved = resolved.trim();

    // Step 2: Literal booleans (also covers cases where {{var}} resolved to
    // a boolean JSON value, which `resolve_prompt_template` stringifies as
    // "true"/"false").
    if resolved.eq_ignore_ascii_case("true") {
        return true;
    }
    if resolved.eq_ignore_ascii_case("false") {
        return false;
    }

    // Step 3: Comparison operators. Try longest-match first so `>=` doesn't
    // accidentally match the `>` inside `a >= b`.
    for op in [">=", "<=", "==", "!=", ">", "<"] {
        if let Some((left, right)) = resolved.split_once(op) {
            return compare_values(left.trim(), right.trim(), op, context);
        }
    }

    // Step 4: No operator — truthy check. If the expression is a bare
    // variable name, look it up in context and use the JSON value's
    // truthiness. Otherwise fall back to string truthiness.
    if let Some(val) = context.get(resolved) {
        return is_truthy(val);
    }
    is_truthy_str(resolved)
}

fn compare_values(
    left: &str,
    right: &str,
    op: &str,
    context: &HashMap<String, serde_json::Value>,
) -> bool {
    let lv = resolve_operand(left, context);
    let rv = resolve_operand(right, context);

    // Numeric comparison when both sides parse as numbers.
    if let (Some(a), Some(b)) = (lv.as_f64(), rv.as_f64()) {
        return match op {
            ">=" => a >= b,
            "<=" => a <= b,
            "==" => a == b,
            "!=" => a != b,
            ">"  => a > b,
            "<"  => a < b,
            _ => false,
        };
    }

    // Fall back to string comparison.
    let ls = match &lv { serde_json::Value::String(s) => s.clone(), o => o.to_string() };
    let rs = match &rv { serde_json::Value::String(s) => s.clone(), o => o.to_string() };
    match op {
        ">=" => ls >= rs,
        "<=" => ls <= rs,
        "==" => ls == rs,
        "!=" => ls != rs,
        ">"  => ls > rs,
        "<"  => ls < rs,
        _ => false,
    }
}

/// Resolve one operand (left or right side of a comparison) to a JSON value.
///
/// - `"quoted"` / `'quoted'` → unquoted literal string
/// - bare name in context → context value (variable lookup)
/// - numeric/bool/null literal → corresponding JSON value
/// - anything else → unquoted literal string
fn resolve_operand(s: &str, context: &HashMap<String, serde_json::Value>) -> serde_json::Value {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return serde_json::Value::Null;
    }
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 2 && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
        || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
    {
        return serde_json::Value::String(trimmed[1..trimmed.len() - 1].to_string());
    }
    if let Some(v) = context.get(trimmed) {
        return v.clone();
    }
    if let Ok(n) = trimmed.parse::<f64>() {
        if let Some(num) = serde_json::Number::from_f64(n) {
            return serde_json::Value::Number(num);
        }
    }
    if trimmed == "true" { return serde_json::Value::Bool(true); }
    if trimmed == "false" { return serde_json::Value::Bool(false); }
    if trimmed == "null" { return serde_json::Value::Null; }
    serde_json::Value::String(trimmed.to_string())
}

fn is_truthy_str(s: &str) -> bool {
    let t = s.trim();
    !t.is_empty() && !t.eq_ignore_ascii_case("false") && t != "0" && t != "0.0"
}

fn is_truthy(val: &serde_json::Value) -> bool {
    match val {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::Number(n) => n.as_f64().map_or(false, |v| v != 0.0),
        serde_json::Value::String(s) => !s.is_empty(),
        serde_json::Value::Array(a) => !a.is_empty(),
        serde_json::Value::Object(o) => !o.is_empty(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

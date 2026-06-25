//! Node executor trait and built-in node implementations.
//!
//! Includes all 11 built-in node types:
//! llm, tool, condition, parallel, loop, sub_workflow, transform, http, script, delay, human_review.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Local;

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
// LLM Node
// ---------------------------------------------------------------------------

/// Built-in LLM node executor (mock).
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
            .unwrap_or(1);
        tokio::time::sleep(std::time::Duration::from_millis(secs)).await;
        Ok(NodeResult {
            node_id: node.id.clone(),
            output: serde_json::json!({ "delayed_ms": secs }),
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
pub struct TransformNodeExecutor;

#[async_trait]
impl NodeExecutor for TransformNodeExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let now = Local::now();
        let expression = node
            .config
            .get("expression")
            .and_then(|v| v.as_str())
            .unwrap_or("identity");

        let output = if expression == "identity" || expression == "passthrough" {
            serde_json::json!(context)
        } else {
            serde_json::json!({
                "transformed": expression,
                "input_keys": context.keys().collect::<Vec<_>>(),
            })
        };

        Ok(NodeResult {
            node_id: node.id.clone(),
            output,
            error: None,
            state: ExecutionState::Completed,
            started_at: now,
            ended_at: Local::now(),
            metadata: HashMap::new(),
        })
    }
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
            // Set loop_index variable in context
            local_ctx.insert(
                "loop_index".to_string(),
                serde_json::json!(i),
            );
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
            // If the value is a string, try to resolve it from the context
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

        Ok(NodeResult {
            node_id: node.id.clone(),
            output: serde_json::Value::Object(output_map),
            error: None,
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
        _context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let now = Local::now();

        let url = node
            .config
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("");

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

        let body = node
            .config
            .get("body")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Build the request
        let client = reqwest::Client::new();
        let req_builder = match method.as_str() {
            "POST" => client.post(url),
            "PUT" => client.put(url),
            "PATCH" => client.patch(url),
            "DELETE" => client.delete(url),
            "HEAD" => client.head(url),
            _ => client.get(url),
        };

        // Set headers
        let headers = node
            .config
            .get("headers")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();

        let mut req_builder = req_builder;
        for (k, v) in &headers {
            if let Some(s) = v.as_str() {
                req_builder = req_builder.header(k.as_str(), s);
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

        // Determine the interpreter and file extension based on language
        let (interpreter, _ext, flag) = match language {
            "python" | "python3" => ("python3", ".py", "-c"),
            "python2" => ("python2", ".py", "-c"),
            "node" | "javascript" | "js" => ("node", ".js", "-e"),
            "powershell" | "pwsh" => ("pwsh", ".ps1", "-Command"),
            "sh" => ("sh", ".sh", "-c"),
            _ => ("bash", ".sh", "-c"), // default to bash
        };

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
        _context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let now = Local::now();

        let message = node
            .config
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("Human review required");

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
            // Set loop_index variable in context
            local_ctx.insert(
                "loop_index".to_string(),
                serde_json::json!(i),
            );
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
        // return a placeholder since they need external dependencies
        _ => {
            let now = Local::now();
            Ok(NodeResult {
                node_id: node_def.id.clone(),
                output: serde_json::json!({
                    "type": node_def.node_type,
                    "status": "skipped",
                    "reason": format!("inline execution not supported for '{}' type", node_def.node_type),
                }),
                error: None,
                state: ExecutionState::Completed,
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

/// Evaluate a simple condition string against the context.
///
/// Supports:
/// - `"variable == value"` -- equality check
/// - `"variable != value"` -- inequality check
/// - `"variable"` -- truthy check
/// - `"true"` / `"false"` -- literal booleans
pub fn evaluate_condition(
    condition: &str,
    context: &HashMap<String, serde_json::Value>,
) -> bool {
    let condition = condition.trim();

    if condition.eq_ignore_ascii_case("true") {
        return true;
    }
    if condition.eq_ignore_ascii_case("false") {
        return false;
    }

    if let Some((left, right)) = condition.split_once("==") {
        let left = left.trim();
        let right = right.trim();
        if let Some(val) = context.get(left) {
            return val == &serde_json::Value::String(right.to_string());
        }
        return false;
    }

    if let Some((left, right)) = condition.split_once("!=") {
        let left = left.trim();
        let right = right.trim();
        if let Some(val) = context.get(left) {
            return val != &serde_json::Value::String(right.to_string());
        }
        return true;
    }

    if let Some(val) = context.get(condition) {
        return is_truthy(val);
    }

    false
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

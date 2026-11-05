// ---------------------------------------------------------------------------
// 机械拆分：本文件内容由 nodes.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

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
        let mode = node
            .config
            .get("mode")
            .and_then(|v| v.as_str())
            .unwrap_or("counter")
            .to_string();
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
                    "mode": mode,
                }),
                error: None,
                state: ExecutionState::Completed,
                started_at: now,
                ended_at: Local::now(),
                metadata: HashMap::new(),
            });
        }

        let safety_cap = max_iter.min(100);

        // Resolve the iteration plan. In `foreach` mode the loop iterates over
        // a resolved list (binding the current element to `{{item}}` /
        // `{{item_index}}`); in `counter` mode it runs a fixed number of times.
        let items: Vec<serde_json::Value> = if mode == "foreach" {
            let raw_items = node
                .config
                .get("items")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            match resolve_items_list(&raw_items, context) {
                Ok(list) => list,
                Err(e) => {
                    return Ok(NodeResult {
                        node_id: node.id.clone(),
                        output: serde_json::json!({ "iterations": 0, "mode": mode }),
                        error: Some(e),
                        state: ExecutionState::Failed,
                        started_at: now,
                        ended_at: Local::now(),
                        metadata: HashMap::new(),
                    });
                }
            }
        } else {
            Vec::new()
        };

        let items_total = items.len();
        let iter_count = if mode == "foreach" {
            items_total.min(safety_cap)
        } else {
            safety_cap
        };

        // Optional custom name for the per-iteration element variable
        // (defaults to `item`; the standard `item`/`item_index` are always set).
        let item_var = node
            .config
            .get("item_var")
            .and_then(|v| v.as_str())
            .unwrap_or("item")
            .to_string();

        // Create a local context for child node execution so loop variables
        // don't pollute the parent context.
        let local_wf_ctx = WorkflowContext::new(context.clone());
        let mut local_ctx = context.clone();
        let mut last_output = serde_json::Value::Null;
        let mut actual_iterations: usize = 0;
        let mut loop_error: Option<String> = None;

        for i in 0..iter_count {
            // Insert iteration index BEFORE condition check and child execution
            // so both `{{i}}` (form-placeholder syntax) and `{{loop_index}}`
            // (legacy) resolve correctly. Inserting at the start also means
            // iter-0 children see `i=0` instead of having no key available.
            local_ctx.insert("i".to_string(), serde_json::json!(i));
            local_ctx.insert("loop_index".to_string(), serde_json::json!(i));

            // In foreach mode, bind the current element.
            if mode == "foreach" {
                let item = items.get(i).cloned().unwrap_or(serde_json::Value::Null);
                local_ctx.insert("item_index".to_string(), serde_json::json!(i));
                local_ctx.insert("item".to_string(), item.clone());
                if item_var != "item" {
                    local_ctx.insert(item_var.clone(), item);
                }
            }

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
                        if matches!(
                            result.state,
                            ExecutionState::Waiting | ExecutionState::Cancelled
                        ) {
                            // 运行期兜底：同 parallel——
                            // 暂停类子状态诚实失败，绝不静默当成功继续迭代。
                            loop_error = Some(format!(
                                "loop iteration {} node {} 返回 {} 状态：暂停类节点不支持在 composite 内（审批落地前 workflow 不得继续）",
                                i, child_def.id, result.state
                            ));
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

        let mut output = serde_json::json!({
            "iterations": actual_iterations,
            "last_output": last_output,
            "mode": mode,
        });
        if mode == "foreach" {
            output["items_count"] = serde_json::json!(items_total);
            output["items_iterated"] = serde_json::json!(actual_iterations);
        }

        // Honest failure: when the loop aborted mid-run, report Failed so the
        // scheduler's retry / failure-tracking path runs and downstream nodes
        // can branch on the failure. Reporting Completed with `error: Some`
        // would silently swallow the failure (the scheduler keys off `state`).
        let state = if loop_error.is_some() {
            ExecutionState::Failed
        } else {
            ExecutionState::Completed
        };

        Ok(NodeResult {
            node_id: node.id.clone(),
            output,
            error: loop_error,
            state,
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
                    let resolved = match resolve_runtime_string(s, context, "sub_workflow.input") {
                        Ok(v) => v,
                        Err(e) => return Ok(failed_node_result(&node.id, now, &e)),
                    };
                    sub_input.insert(k.clone(), serde_json::Value::String(resolved));
                }
            } else {
                // Arrays / objects: recurse so {{var}} inside nested values
                // also resolves.
                let resolved = match resolve_template_value(v, context, "sub_workflow.input") {
                    Ok(v) => v,
                    Err(e) => return Ok(failed_node_result(&node.id, now, &e)),
                };
                sub_input.insert(k.clone(), resolved);
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

        // When the child execution failed, surface an error string alongside
        // the Failed state so parent-side retry / failure-tracking has a
        // concrete cause message. Failure propagation itself keys off
        // `state: exec_result.state` (the scheduler branches on
        // `state != Failed`), not off `error`; without the state passthrough
        // a Failed child would be treated as success regardless of `error`.
        let error = if exec_result.state == ExecutionState::Failed {
            Some(
                exec_result
                    .error
                    .clone()
                    .unwrap_or_else(|| "sub_workflow child execution failed".to_string()),
            )
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

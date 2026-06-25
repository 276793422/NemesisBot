//! Workflow scheduler - Topological sort and level-based DAG execution.
//!
//! Mirrors the Go `scheduler.go` with topological sort that produces execution
//! levels for parallel node execution, and a `Schedule` function that runs
//! nodes level-by-level with retry support.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use crate::context::WorkflowContext;
use crate::nodes::NodeExecutorRegistry;
use crate::types::{Edge, ExecutionState, NodeDef};

/// Performs a topological sort on the workflow graph.
///
/// Returns execution levels where each level contains node IDs that can be
/// executed in parallel. Returns an error if a cycle is detected.
pub fn topological_sort(nodes: &[NodeDef], edges: &[Edge]) -> Result<Vec<Vec<String>>, String> {
    let mut in_degree: HashMap<String, usize> = HashMap::new();
    let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
    let mut node_ids: HashSet<String> = HashSet::new();

    for n in nodes {
        node_ids.insert(n.id.clone());
        in_degree.insert(n.id.clone(), 0);
    }

    for e in edges {
        adjacency
            .entry(e.from_node.clone())
            .or_default()
            .push(e.to_node.clone());
        *in_degree.entry(e.to_node.clone()).or_insert(0) += 1;
    }

    // Account for DependsOn
    for n in nodes {
        for dep in &n.depends_on {
            adjacency
                .entry(dep.clone())
                .or_default()
                .push(n.id.clone());
            *in_degree.entry(n.id.clone()).or_insert(0) += 1;
        }
    }

    // Kahn's algorithm with level tracking
    let mut levels: Vec<Vec<String>> = Vec::new();
    let mut queue: Vec<String> = in_degree
        .iter()
        .filter(|(_, deg)| **deg == 0)
        .map(|(id, _)| id.clone())
        .collect();

    let mut visited = 0;

    while !queue.is_empty() {
        levels.push(queue.clone());
        let mut next_queue = Vec::new();

        for id in &queue {
            visited += 1;
            if let Some(neighbors) = adjacency.get(id) {
                for neighbor in neighbors {
                    if let Some(deg) = in_degree.get_mut(neighbor) {
                        *deg -= 1;
                        if *deg == 0 {
                            next_queue.push(neighbor.clone());
                        }
                    }
                }
            }
        }

        queue = next_queue;
    }

    if visited != node_ids.len() {
        return Err("cycle detected in workflow graph".to_string());
    }

    Ok(levels)
}

/// Build a flat context map from the workflow context.
///
/// Combines workflow variables and previous node result outputs into a single
/// HashMap suitable for passing to node executors.  For node outputs, each
/// field is stored as `node_id.field` so downstream nodes can reference them.
fn build_executor_context(wf_ctx: &WorkflowContext) -> HashMap<String, serde_json::Value> {
    let mut ctx: HashMap<String, serde_json::Value> = HashMap::new();

    // Workflow variables
    for (k, v) in wf_ctx.get_all_variables() {
        ctx.insert(k, serde_json::json!(v));
    }

    // Previous node results: store each output field as node_id.field
    for (node_id, result) in wf_ctx.get_all_node_results() {
        if let Some(obj) = result.output.as_object() {
            for (field, val) in obj {
                ctx.insert(format!("{}.{}", node_id, field), val.clone());
            }
        }
        // Also store the full output under the node_id
        ctx.insert(node_id, result.output.clone());
    }

    ctx
}

/// Scheduler outcome indicating how a schedule/schedule_resume call ended.
///
/// `Ok` variants represent expected paths (completed normally, cancelled by user).
/// `Err` carries an internal failure (node panic, I/O error, etc.).
///
/// Note: `Waiting` variant will be added in 1b-A1-step4 when human_review /
/// Checkpointer integration lands.
#[derive(Debug, Clone, PartialEq)]
pub enum ScheduleOutcome {
    /// All runnable nodes completed without cancellation.
    Completed,
    /// Cancellation token was triggered mid-execution.
    Cancelled,
}

/// Execute workflow nodes respecting dependencies and parallelism.
///
/// Nodes at the same topological level are executed concurrently.
/// Supports retry, per-node timeout, and conditional edge evaluation.
/// After each node executes, its output fields are propagated into the
/// workflow context as `node_id.field = value` entries.
///
/// Cancellation: pass a `CancellationToken` that, when triggered, causes the
/// scheduler to stop spawning new levels and abort waiting on in-flight nodes.
/// Already-running node executors receive `cancel.cancelled()` via select! and
/// are expected to bail out promptly.
#[allow(clippy::too_many_arguments)]
pub async fn schedule(
    nodes: &[NodeDef],
    edges: &[Edge],
    executors: &NodeExecutorRegistry,
    wf_ctx: &mut WorkflowContext,
    cancel: CancellationToken,
) -> Result<ScheduleOutcome, String> {
    // Build node lookup
    let node_map: HashMap<String, &NodeDef> = nodes.iter().map(|n| (n.id.clone(), n)).collect();

    // Build conditional edge map
    let mut cond_edges: HashMap<String, Vec<&Edge>> = HashMap::new();
    for e in edges {
        if e.condition.is_some() {
            cond_edges.entry(e.to_node.clone()).or_default().push(e);
        }
    }

    // Compute execution levels
    let levels = topological_sort(nodes, edges)?;

    // Execute level by level
    for level in levels {
        // Bail out early if cancelled before spawning the next level.
        if cancel.is_cancelled() {
            return Ok(ScheduleOutcome::Cancelled);
        }

        // Filter nodes that should run based on conditional edges
        let runnable: Vec<String> = level
            .into_iter()
            .filter(|id| should_run_node(id, &cond_edges, wf_ctx))
            .collect();

        if runnable.is_empty() {
            continue;
        }

        // Build context from current workflow state (before spawning tasks)
        let exec_ctx = build_executor_context(wf_ctx);

        // Execute all nodes in this level concurrently
        let mut handles = Vec::new();

        for node_id in runnable {
            let node = match node_map.get(&node_id) {
                Some(n) => (*n).clone(),
                None => return Err(format!("node {:?} not found in definition", node_id)),
            };

            let executor = match executors.get(&node.node_type) {
                Some(e) => Arc::clone(e),
                None => {
                    return Err(format!(
                        "no executor for node type {:?} (node {})",
                        node.node_type, node_id
                    ))
                }
            };

            let node_id = node.id.clone();
            let max_retries = node.retry_count;
            let node_timeout = node.timeout_duration();
            let ctx = exec_ctx.clone();
            let wf_ctx_clone = wf_ctx.clone();
            let task_cancel = cancel.clone();

            let handle = tokio::spawn(async move {
                // Execute with retry; bail out if cancelled between attempts.
                let mut last_result = None;
                let mut last_error: Option<String> = None;

                for attempt in 0..=max_retries {
                    if task_cancel.is_cancelled() {
                        break;
                    }

                    let result = if let Some(dur) = node_timeout {
                        tokio::select! {
                            biased;
                            _ = task_cancel.cancelled() => break,
                            r = timeout(dur, executor.execute(&node, &ctx, &wf_ctx_clone)) => {
                                match r {
                                    Ok(inner) => inner,
                                    Err(_) => Err(format!("node {:?} timed out after {:?}", node.id, dur)),
                                }
                            }
                        }
                    } else {
                        tokio::select! {
                            biased;
                            _ = task_cancel.cancelled() => break,
                            r = executor.execute(&node, &ctx, &wf_ctx_clone) => r,
                        }
                    };

                    match result {
                        Ok(r) if r.state != ExecutionState::Failed => {
                            last_result = Some(r);
                            last_error = None;
                            break;
                        }
                        Ok(r) => {
                            let err = r.error.clone();
                            last_result = Some(r);
                            last_error = err;
                        }
                        Err(e) => {
                            last_error = Some(e);
                        }
                    }

                    // Backoff before retry, but bail if cancelled during sleep.
                    if attempt < max_retries {
                        let backoff = Duration::from_millis((attempt as u64 + 1) * 500);
                        tokio::select! {
                            biased;
                            _ = task_cancel.cancelled() => break,
                            _ = tokio::time::sleep(backoff) => {}
                        }
                    }
                }

                (node_id, last_result, last_error)
            });

            handles.push(handle);
        }

        // Collect results; abort waiting if cancelled.
        for handle in handles {
            let collected = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Ok(ScheduleOutcome::Cancelled),
                r = handle => r,
            };

            match collected {
                Ok((node_id, Some(result), None)) => {
                    if let Some(obj) = result.output.as_object() {
                        for (field, val) in obj {
                            wf_ctx.set_var(
                                &format!("{}.{}", node_id, field),
                                &val.to_string(),
                            );
                        }
                    }
                    wf_ctx.set_node_result(&node_id, result);
                }
                Ok((node_id, _, Some(err))) => {
                    return Err(format!("node {:?} execution failed: {}", node_id, err));
                }
                Ok((node_id, None, None)) => {
                    // Could be a cancellation during retry/backoff: check flag.
                    if cancel.is_cancelled() {
                        return Ok(ScheduleOutcome::Cancelled);
                    }
                    return Err(format!("node {:?} produced no result", node_id));
                }
                Err(e) => {
                    if cancel.is_cancelled() {
                        return Ok(ScheduleOutcome::Cancelled);
                    }
                    return Err(format!("node task panicked: {}", e));
                }
            }
        }
    }

    Ok(ScheduleOutcome::Completed)
}

/// Check if a node should be executed based on conditional edges.
///
/// Evaluates conditions using expression-style matching (e.g., `status == "ok"`,
/// `count != 0`) via the same `evaluate_condition` function used by the
/// ConditionNodeExecutor. Falls back to simple boolean matching for literal
/// conditions.
fn should_run_node(
    node_id: &str,
    cond_edges: &HashMap<String, Vec<&Edge>>,
    wf_ctx: &WorkflowContext,
) -> bool {
    if let Some(edges) = cond_edges.get(node_id) {
        for edge in edges {
            if let Some(ref cond) = edge.condition {
                let resolved = wf_ctx.resolve(cond);

                // First, try simple boolean check for resolved value
                let lower = resolved.to_lowercase();
                match lower.as_str() {
                    "true" | "1" | "yes" => continue,
                    "false" | "0" | "no" => return false,
                    _ => {}
                }

                // If the resolved value is unchanged (no template variables
                // were present), evaluate the condition as an expression
                // against the workflow context.
                if resolved == cond.as_str() {
                    let ctx = build_executor_context(wf_ctx);
                    if !crate::nodes::evaluate_condition(cond, &ctx) {
                        return false;
                    }
                } else {
                    // Template was resolved but didn't match a known boolean;
                    // treat the resolved value itself as truthy/falsy.
                    if resolved.is_empty() {
                        return false;
                    }
                }
            }
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

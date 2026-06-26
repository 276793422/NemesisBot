//! Workflow scheduler - Topological sort and level-based DAG execution.
//!
//! Mirrors the Go `scheduler.go` with topological sort that produces execution
//! levels for parallel node execution, and a `Schedule` function that runs
//! nodes level-by-level with retry support.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use async_trait::async_trait;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use crate::context::WorkflowContext;
use crate::nodes::NodeExecutorRegistry;
use crate::types::{Edge, ExecutionState, NodeDef};

/// Optional progress hook invoked by the scheduler after each level finishes.
///
/// Used by the engine (1b-A1 step 6) to persist a checkpoint of the current
/// `wf_ctx` between levels so an interrupted execution can resume without
/// re-running completed nodes.
///
/// The hook receives the workflow context as it stands *after* the level's
/// results have been merged in. Implementations must be cheap — they run on
/// the scheduler's task, blocking further levels until they return.
#[async_trait]
pub trait ProgressHook: Send + Sync {
    async fn on_level_completed(&self, wf_ctx: &WorkflowContext);
}/// Performs a topological sort on the workflow graph.
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
/// Combines workflow input, variables, and previous node result outputs into
/// a single HashMap suitable for passing to node executors. For node outputs,
/// each field is stored as `node_id.field` so downstream nodes can reference
/// them.
///
/// **Merge order matters**: input < variables < node_results. If a key is
/// claimed by multiple stores, the later store wins — so a workflow author
/// can't accidentally shadow `node_id.field` references by naming a variable
/// `some_node.x`, and a `set_var` call can't be silently overwritten by a
/// stale input field of the same name. Trigger-time inputs (workflow_chat's
/// `input`/`content`/`chat_id`/...) are the lowest-priority baseline.
fn build_executor_context(wf_ctx: &WorkflowContext) -> HashMap<String, serde_json::Value> {
    let mut ctx: HashMap<String, serde_json::Value> = HashMap::new();

    // Workflow input (trigger-time fields). Lowest precedence — variables
    // and node results can override.
    for (k, v) in wf_ctx.get_all_input() {
        ctx.insert(k, v);
    }

    // Workflow variables (already JSON-typed since 1b-B3).
    for (k, v) in wf_ctx.get_all_variables() {
        ctx.insert(k, v);
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
/// `Waiting` is detected *after* the scheduler returns — by inspecting node
/// results — so the scheduler itself never produces a `Waiting` outcome. This
/// matches the engine's existing post-schedule inspection logic and keeps the
/// outcome enum focused on how the scheduler exited.
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
///
/// Fresh-run entry point: see [`schedule_resume`] for the checkpoint-driven
/// resume path that skips already-completed nodes.
#[allow(clippy::too_many_arguments)]
pub async fn schedule(
    nodes: &[NodeDef],
    edges: &[Edge],
    executors: &NodeExecutorRegistry,
    wf_ctx: &mut WorkflowContext,
    cancel: CancellationToken,
) -> Result<ScheduleOutcome, String> {
    let empty = HashSet::new();
    schedule_inner(nodes, edges, executors, wf_ctx, &empty, cancel, None).await
}

/// Like [`schedule`] but with a per-level progress hook (1b-A1 step 6).
#[allow(clippy::too_many_arguments)]
pub async fn schedule_with_hook(
    nodes: &[NodeDef],
    edges: &[Edge],
    executors: &NodeExecutorRegistry,
    wf_ctx: &mut WorkflowContext,
    cancel: CancellationToken,
    hook: &dyn ProgressHook,
) -> Result<ScheduleOutcome, String> {
    let empty = HashSet::new();
    schedule_inner(nodes, edges, executors, wf_ctx, &empty, cancel, Some(hook)).await
}

/// Resume execution from a checkpoint.
///
/// `completed_nodes` lists nodes that already ran successfully before the
/// crash / pause and must not be re-executed. Their previous outputs are
/// expected to already be present in `wf_ctx` (the caller restores them from
/// the checkpoint's `context_snapshot` before invoking this function).
///
/// The scheduler runs the same DAG, just with the skip filter applied. This
/// is what lets us resume deterministically after a crash: the topology is
/// unchanged, only the set of "still needs to run" nodes differs.
///
/// Conditional edges, retry, timeout, and cancellation behave identically to
/// [`schedule`]. A node that lands in `ExecutionState::Waiting` is saved to
/// the workflow context like any other result — the caller detects Waiting
/// state after this function returns.
#[allow(clippy::too_many_arguments)]
pub async fn schedule_resume(
    nodes: &[NodeDef],
    edges: &[Edge],
    executors: &NodeExecutorRegistry,
    wf_ctx: &mut WorkflowContext,
    completed_nodes: &HashSet<String>,
    cancel: CancellationToken,
) -> Result<ScheduleOutcome, String> {
    schedule_inner(nodes, edges, executors, wf_ctx, completed_nodes, cancel, None).await
}

/// Like [`schedule_resume`] but with a per-level progress hook.
#[allow(clippy::too_many_arguments)]
pub async fn schedule_resume_with_hook(
    nodes: &[NodeDef],
    edges: &[Edge],
    executors: &NodeExecutorRegistry,
    wf_ctx: &mut WorkflowContext,
    completed_nodes: &HashSet<String>,
    cancel: CancellationToken,
    hook: &dyn ProgressHook,
) -> Result<ScheduleOutcome, String> {
    schedule_inner(nodes, edges, executors, wf_ctx, completed_nodes, cancel, Some(hook)).await
}

/// Shared body for [`schedule`] and [`schedule_resume`].
///
/// `skip` lists node IDs that must not be executed (already-completed nodes
/// when resuming from a checkpoint; empty for fresh runs).
/// `hook` is invoked after each level finishes (1b-A1 step 6) so callers can
/// persist progress; pass `None` to skip.
#[allow(clippy::too_many_arguments)]
async fn schedule_inner(
    nodes: &[NodeDef],
    edges: &[Edge],
    executors: &NodeExecutorRegistry,
    wf_ctx: &mut WorkflowContext,
    skip: &HashSet<String>,
    cancel: CancellationToken,
    hook: Option<&dyn ProgressHook>,
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

        // Filter nodes that should run based on conditional edges and the
        // resume skip set. Skipped nodes (already-completed when resuming
        // from a checkpoint) are dropped here — their outputs were already
        // restored into wf_ctx by the caller.
        let runnable: Vec<String> = level
            .into_iter()
            .filter(|id| !skip.contains(id))
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
                Some(e) => e,
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
                            // Store as JSON value (1b-B3) — preserves structure
                            // for downstream nodes instead of stringifying.
                            wf_ctx.set_var(
                                &format!("{}.{}", node_id, field),
                                val.clone(),
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

        // After all nodes in this level have settled (some may be in Waiting
        // state), give the hook a chance to checkpoint progress before the
        // next level starts. Skipping on cancel keeps shutdown snappy.
        if let Some(h) = hook {
            h.on_level_completed(wf_ctx).await;
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

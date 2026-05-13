//! Workflow scheduler - Topological sort and level-based DAG execution.
//!
//! Mirrors the Go `scheduler.go` with topological sort that produces execution
//! levels for parallel node execution, and a `Schedule` function that runs
//! nodes level-by-level with retry support.

use std::collections::{HashMap, HashSet};

use crate::types::{Edge, ExecutionState, NodeDef};
use crate::context::WorkflowContext;
use crate::nodes::NodeExecutorRegistry;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

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

/// Execute workflow nodes respecting dependencies and parallelism.
///
/// Nodes at the same topological level are executed concurrently.
/// Supports retry, per-node timeout, and conditional edge evaluation.
/// After each node executes, its output fields are propagated into the
/// workflow context as `node_id.field = value` entries.
#[allow(clippy::too_many_arguments)]
pub async fn schedule(
    _ctx: tokio::task::JoinHandle<()>,
    nodes: &[NodeDef],
    edges: &[Edge],
    executors: &NodeExecutorRegistry,
    wf_ctx: &mut WorkflowContext,
) -> Result<(), String> {
    // Build node lookup
    let node_map: HashMap<String, &NodeDef> = nodes.iter().map(|n| (n.id.clone(), n)).collect();

    // Build conditional edge map
    let mut cond_edges: HashMap<String, Vec<&Edge>> = HashMap::new();
    for e in edges {
        if e.condition.is_some() {
            cond_edges
                .entry(e.to_node.clone())
                .or_default()
                .push(e);
        }
    }

    // Compute execution levels
    let levels = topological_sort(nodes, edges)?;

    // Execute level by level
    for level in levels {
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
                None => return Err(format!("no executor for node type {:?} (node {})", node.node_type, node_id)),
            };

            let node_id = node.id.clone();
            let max_retries = node.retry_count;
            let node_timeout = node.timeout_duration();
            let ctx = exec_ctx.clone();
            let wf_ctx_clone = wf_ctx.clone();

            let handle = tokio::spawn(async move {
                // Execute with retry
                let mut last_result = None;
                let mut last_error = None;

                for attempt in 0..=max_retries {
                    let result = if let Some(dur) = node_timeout {
                        match timeout(dur, executor.execute(&node, &ctx, &wf_ctx_clone)).await {
                            Ok(r) => r,
                            Err(_) => Err(format!("node {:?} timed out after {:?}", node.id, dur)),
                        }
                    } else {
                        executor.execute(&node, &ctx, &wf_ctx_clone).await
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

                    // Backoff before retry
                    if attempt < max_retries {
                        let backoff = Duration::from_millis((attempt as u64 + 1) * 500);
                        tokio::time::sleep(backoff).await;
                    }
                }

                (node_id, last_result, last_error)
            });

            handles.push(handle);
        }

        // Collect results and propagate variables into workflow context
        for handle in handles {
            match handle.await {
                Ok((node_id, Some(result), None)) => {
                    // Set output variables into context: node_id.field = value
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
                    return Err(format!("node {:?} produced no result", node_id));
                }
                Err(e) => {
                    return Err(format!("node task panicked: {}", e));
                }
            }
        }
    }

    Ok(())
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
mod tests {
    use super::*;
    use crate::types::NodeDef;

    fn make_node(id: &str, depends_on: Vec<&str>) -> NodeDef {
        NodeDef {
            id: id.to_string(),
            node_type: "llm".to_string(),
            config: HashMap::new(),
            depends_on: depends_on.into_iter().map(|s| s.to_string()).collect(),
            retry_count: 0,
            timeout: None,
        }
    }

    #[test]
    fn test_topological_sort_linear() {
        let nodes = vec![
            make_node("a", vec![]),
            make_node("b", vec!["a"]),
            make_node("c", vec!["b"]),
        ];
        let edges = vec![];

        let levels = topological_sort(&nodes, &edges).unwrap();
        assert_eq!(levels.len(), 3);
        assert!(levels[0].contains(&"a".to_string()));
        assert!(levels[1].contains(&"b".to_string()));
        assert!(levels[2].contains(&"c".to_string()));
    }

    #[test]
    fn test_topological_sort_parallel() {
        let nodes = vec![
            make_node("a", vec![]),
            make_node("b", vec![]),
            make_node("c", vec!["a", "b"]),
        ];
        let edges = vec![];

        let levels = topological_sort(&nodes, &edges).unwrap();
        assert_eq!(levels.len(), 2);
        // First level should contain both a and b
        assert_eq!(levels[0].len(), 2);
        assert!(levels[1].contains(&"c".to_string()));
    }

    #[test]
    fn test_topological_sort_cycle_detected() {
        let nodes = vec![
            make_node("a", vec!["b"]),
            make_node("b", vec!["a"]),
        ];
        let edges = vec![];

        let result = topological_sort(&nodes, &edges);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cycle"));
    }

    #[test]
    fn test_topological_sort_with_edges() {
        let nodes = vec![
            make_node("a", vec![]),
            make_node("b", vec![]),
        ];
        let edges = vec![Edge {
            from_node: "a".to_string(),
            to_node: "b".to_string(),
            condition: None,
        }];

        let levels = topological_sort(&nodes, &edges).unwrap();
        assert_eq!(levels.len(), 2);
        assert!(levels[0].contains(&"a".to_string()));
        assert!(levels[1].contains(&"b".to_string()));
    }

    #[test]
    fn test_should_run_node_no_conditions() {
        let wf_ctx = WorkflowContext::new(HashMap::new());
        let cond_edges = HashMap::new();
        assert!(should_run_node("any", &cond_edges, &wf_ctx));
    }

    #[test]
    fn test_should_run_node_expression_equality() {
        // condition: "status == ok" should evaluate against context
        let mut input = HashMap::new();
        input.insert("status".to_string(), serde_json::json!("ok"));
        let wf_ctx = WorkflowContext::new(input);
        wf_ctx.set_var("status", "ok");

        let edge = Edge {
            from_node: "source".to_string(),
            to_node: "target".to_string(),
            condition: Some("status == ok".to_string()),
        };
        let cond_edges: HashMap<String, Vec<&Edge>> = {
            let mut m = HashMap::new();
            m.insert("target".to_string(), vec![&edge]);
            m
        };
        assert!(should_run_node("target", &cond_edges, &wf_ctx));
    }

    #[test]
    fn test_should_run_node_expression_inequality() {
        // condition: "status == ok" with status "error" should not run
        let mut input = HashMap::new();
        input.insert("status".to_string(), serde_json::json!("error"));
        let wf_ctx = WorkflowContext::new(input);
        wf_ctx.set_var("status", "error");

        let edge = Edge {
            from_node: "source".to_string(),
            to_node: "target".to_string(),
            condition: Some("status == ok".to_string()),
        };
        let cond_edges: HashMap<String, Vec<&Edge>> = {
            let mut m = HashMap::new();
            m.insert("target".to_string(), vec![&edge]);
            m
        };
        assert!(!should_run_node("target", &cond_edges, &wf_ctx));
    }

    #[test]
    fn test_should_run_node_boolean_true() {
        let wf_ctx = WorkflowContext::new(HashMap::new());
        wf_ctx.set_var("should_run", "true");

        let edge = Edge {
            from_node: "source".to_string(),
            to_node: "target".to_string(),
            condition: Some("{{should_run}}".to_string()),
        };
        let cond_edges: HashMap<String, Vec<&Edge>> = {
            let mut m = HashMap::new();
            m.insert("target".to_string(), vec![&edge]);
            m
        };
        assert!(should_run_node("target", &cond_edges, &wf_ctx));
    }

    // ============================================================
    // Additional scheduler tests: complex expressions, templates,
    // topological sort edge cases
    // ============================================================

    #[test]
    fn test_topological_sort_empty() {
        let nodes: Vec<NodeDef> = vec![];
        let edges = vec![];
        let levels = topological_sort(&nodes, &edges).unwrap();
        assert!(levels.is_empty());
    }

    #[test]
    fn test_topological_sort_single_node() {
        let nodes = vec![make_node("a", vec![])];
        let edges = vec![];
        let levels = topological_sort(&nodes, &edges).unwrap();
        assert_eq!(levels.len(), 1);
        assert!(levels[0].contains(&"a".to_string()));
    }

    #[test]
    fn test_topological_sort_diamond() {
        // Diamond: a -> b, a -> c, b -> d, c -> d
        let nodes = vec![
            make_node("a", vec![]),
            make_node("b", vec!["a"]),
            make_node("c", vec!["a"]),
            make_node("d", vec!["b", "c"]),
        ];
        let edges = vec![];
        let levels = topological_sort(&nodes, &edges).unwrap();
        assert_eq!(levels.len(), 3); // [a], [b, c], [d]
        assert!(levels[0].contains(&"a".to_string()));
        assert_eq!(levels[1].len(), 2);
        assert!(levels[2].contains(&"d".to_string()));
    }

    #[test]
    fn test_topological_sort_self_cycle() {
        let nodes = vec![make_node("a", vec!["a"])];
        let edges = vec![];
        let result = topological_sort(&nodes, &edges);
        assert!(result.is_err());
    }

    #[test]
    fn test_topological_sort_three_node_cycle() {
        let nodes = vec![
            make_node("a", vec!["c"]),
            make_node("b", vec!["a"]),
            make_node("c", vec!["b"]),
        ];
        let edges = vec![];
        let result = topological_sort(&nodes, &edges);
        assert!(result.is_err());
    }

    #[test]
    fn test_topological_sort_with_multiple_edges() {
        let nodes = vec![
            make_node("a", vec![]),
            make_node("b", vec![]),
            make_node("c", vec![]),
            make_node("d", vec![]),
        ];
        let edges = vec![
            Edge { from_node: "a".to_string(), to_node: "c".to_string(), condition: None },
            Edge { from_node: "b".to_string(), to_node: "c".to_string(), condition: None },
            Edge { from_node: "c".to_string(), to_node: "d".to_string(), condition: None },
        ];
        let levels = topological_sort(&nodes, &edges).unwrap();
        // d should be in a later level than c
        let d_level = levels.iter().position(|l| l.contains(&"d".to_string())).unwrap();
        let c_level = levels.iter().position(|l| l.contains(&"c".to_string())).unwrap();
        assert!(d_level > c_level);
    }

    #[test]
    fn test_should_run_node_expression_not_equals() {
        let mut input = HashMap::new();
        input.insert("status".to_string(), serde_json::json!("error"));
        let wf_ctx = WorkflowContext::new(input);
        wf_ctx.set_var("status", "error");

        let edge = Edge {
            from_node: "source".to_string(),
            to_node: "target".to_string(),
            condition: Some("status != ok".to_string()),
        };
        let cond_edges: HashMap<String, Vec<&Edge>> = {
            let mut m = HashMap::new();
            m.insert("target".to_string(), vec![&edge]);
            m
        };
        // "status != ok" with status="error" should run
        assert!(should_run_node("target", &cond_edges, &wf_ctx));
    }

    #[test]
    fn test_should_run_node_expression_not_equals_same_value() {
        let mut input = HashMap::new();
        input.insert("status".to_string(), serde_json::json!("ok"));
        let wf_ctx = WorkflowContext::new(input);
        wf_ctx.set_var("status", "ok");

        let edge = Edge {
            from_node: "source".to_string(),
            to_node: "target".to_string(),
            condition: Some("status != ok".to_string()),
        };
        let cond_edges: HashMap<String, Vec<&Edge>> = {
            let mut m = HashMap::new();
            m.insert("target".to_string(), vec![&edge]);
            m
        };
        // "status != ok" with status="ok" should NOT run
        assert!(!should_run_node("target", &cond_edges, &wf_ctx));
    }

    #[test]
    fn test_should_run_node_boolean_false() {
        let wf_ctx = WorkflowContext::new(HashMap::new());
        wf_ctx.set_var("should_run", "false");

        let edge = Edge {
            from_node: "source".to_string(),
            to_node: "target".to_string(),
            condition: Some("{{should_run}}".to_string()),
        };
        let cond_edges: HashMap<String, Vec<&Edge>> = {
            let mut m = HashMap::new();
            m.insert("target".to_string(), vec![&edge]);
            m
        };
        assert!(!should_run_node("target", &cond_edges, &wf_ctx));
    }

    #[test]
    fn test_should_run_node_no_matching_edge() {
        let wf_ctx = WorkflowContext::new(HashMap::new());
        let cond_edges: HashMap<String, Vec<&Edge>> = HashMap::new();
        // No edges for this node - should default to true
        assert!(should_run_node("unknown_node", &cond_edges, &wf_ctx));
    }

    #[test]
    fn test_should_run_node_edge_without_condition() {
        let wf_ctx = WorkflowContext::new(HashMap::new());
        let edge = Edge {
            from_node: "source".to_string(),
            to_node: "target".to_string(),
            condition: None,
        };
        let cond_edges: HashMap<String, Vec<&Edge>> = {
            let mut m = HashMap::new();
            m.insert("target".to_string(), vec![&edge]);
            m
        };
        // Edge without condition should not block
        assert!(should_run_node("target", &cond_edges, &wf_ctx));
    }

    #[test]
    fn test_should_run_node_boolean_zero() {
        let wf_ctx = WorkflowContext::new(HashMap::new());
        wf_ctx.set_var("flag", "0");

        let edge = Edge {
            from_node: "source".to_string(),
            to_node: "target".to_string(),
            condition: Some("{{flag}}".to_string()),
        };
        let cond_edges: HashMap<String, Vec<&Edge>> = {
            let mut m = HashMap::new();
            m.insert("target".to_string(), vec![&edge]);
            m
        };
        assert!(!should_run_node("target", &cond_edges, &wf_ctx));
    }

    #[test]
    fn test_should_run_node_boolean_yes() {
        let wf_ctx = WorkflowContext::new(HashMap::new());
        wf_ctx.set_var("flag", "yes");

        let edge = Edge {
            from_node: "source".to_string(),
            to_node: "target".to_string(),
            condition: Some("{{flag}}".to_string()),
        };
        let cond_edges: HashMap<String, Vec<&Edge>> = {
            let mut m = HashMap::new();
            m.insert("target".to_string(), vec![&edge]);
            m
        };
        assert!(should_run_node("target", &cond_edges, &wf_ctx));
    }

    #[test]
    fn test_should_run_node_boolean_no() {
        let wf_ctx = WorkflowContext::new(HashMap::new());
        wf_ctx.set_var("flag", "no");

        let edge = Edge {
            from_node: "source".to_string(),
            to_node: "target".to_string(),
            condition: Some("{{flag}}".to_string()),
        };
        let cond_edges: HashMap<String, Vec<&Edge>> = {
            let mut m = HashMap::new();
            m.insert("target".to_string(), vec![&edge]);
            m
        };
        assert!(!should_run_node("target", &cond_edges, &wf_ctx));
    }

    #[test]
    fn test_should_run_node_resolved_empty() {
        let wf_ctx = WorkflowContext::new(HashMap::new());
        wf_ctx.set_var("flag", "");

        let edge = Edge {
            from_node: "source".to_string(),
            to_node: "target".to_string(),
            condition: Some("{{flag}}".to_string()),
        };
        let cond_edges: HashMap<String, Vec<&Edge>> = {
            let mut m = HashMap::new();
            m.insert("target".to_string(), vec![&edge]);
            m
        };
        assert!(!should_run_node("target", &cond_edges, &wf_ctx));
    }

    #[test]
    fn test_should_run_node_resolved_nonempty_truthy() {
        let wf_ctx = WorkflowContext::new(HashMap::new());
        wf_ctx.set_var("flag", "some_value");

        let edge = Edge {
            from_node: "source".to_string(),
            to_node: "target".to_string(),
            condition: Some("{{flag}}".to_string()),
        };
        let cond_edges: HashMap<String, Vec<&Edge>> = {
            let mut m = HashMap::new();
            m.insert("target".to_string(), vec![&edge]);
            m
        };
        assert!(should_run_node("target", &cond_edges, &wf_ctx));
    }

    #[test]
    fn test_should_run_node_multiple_conditions_first_false() {
        let wf_ctx = WorkflowContext::new(HashMap::new());
        wf_ctx.set_var("flag", "false");

        let edge1 = Edge {
            from_node: "source".to_string(),
            to_node: "target".to_string(),
            condition: Some("{{flag}}".to_string()),
        };
        let edge2 = Edge {
            from_node: "other".to_string(),
            to_node: "target".to_string(),
            condition: Some("true".to_string()),
        };
        let cond_edges: HashMap<String, Vec<&Edge>> = {
            let mut m = HashMap::new();
            m.insert("target".to_string(), vec![&edge1, &edge2]);
            m
        };
        assert!(!should_run_node("target", &cond_edges, &wf_ctx));
    }

    #[test]
    fn test_build_executor_context_variables() {
        let input = HashMap::new();
        let wf_ctx = WorkflowContext::new(input);
        wf_ctx.set_var("key", "value");
        wf_ctx.set_var("extra", "data");

        let ctx = build_executor_context(&wf_ctx);
        assert_eq!(ctx.get("key").unwrap(), &serde_json::json!("value"));
        assert_eq!(ctx.get("extra").unwrap(), &serde_json::json!("data"));
    }

    #[test]
    fn test_build_executor_context_empty() {
        let input = HashMap::new();
        let wf_ctx = WorkflowContext::new(input);

        let ctx = build_executor_context(&wf_ctx);
        assert!(ctx.is_empty());
    }

    #[test]
    fn test_build_executor_context_with_node_results() {
        use crate::types::NodeResult;
        use chrono::Utc;

        let input = HashMap::new();
        let wf_ctx = WorkflowContext::new(input);
        let result = NodeResult {
            node_id: "node_a".to_string(),
            output: serde_json::json!({"field1": "val1", "field2": 42}),
            error: None,
            state: ExecutionState::Completed,
            started_at: Utc::now(),
            ended_at: Utc::now(),
            metadata: HashMap::new(),
        };
        wf_ctx.set_node_result("node_a", result);

        let ctx = build_executor_context(&wf_ctx);
        assert_eq!(ctx.get("node_a.field1").unwrap(), &serde_json::json!("val1"));
        assert_eq!(ctx.get("node_a.field2").unwrap(), &serde_json::json!(42));
        assert!(ctx.contains_key("node_a"));
    }

    #[test]
    fn test_topological_sort_wide_parallel() {
        let nodes: Vec<NodeDef> = (0..10)
            .map(|i| make_node(&format!("n{}", i), vec![]))
            .collect();
        let edges = vec![];
        let levels = topological_sort(&nodes, &edges).unwrap();
        assert_eq!(levels.len(), 1);
        assert_eq!(levels[0].len(), 10);
    }

    #[test]
    fn test_topological_sort_chain_of_5() {
        let nodes = vec![
            make_node("a", vec![]),
            make_node("b", vec!["a"]),
            make_node("c", vec!["b"]),
            make_node("d", vec!["c"]),
            make_node("e", vec!["d"]),
        ];
        let edges = vec![];
        let levels = topological_sort(&nodes, &edges).unwrap();
        assert_eq!(levels.len(), 5);
    }
}

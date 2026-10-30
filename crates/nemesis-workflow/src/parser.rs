//! YAML/JSON workflow parser and validator.
//!
//! Mirrors the Go `parser.go` with YAML/JSON parsing via serde,
//! and comprehensive validation (unique IDs, edge references, DAG check).

use std::collections::HashSet;
use std::path::Path;

use crate::scheduler::topological_sort;
use crate::types::Workflow;

/// Parse a YAML or JSON byte slice into a Workflow definition.
pub fn parse(data: &[u8]) -> Result<Workflow, String> {
    // Try YAML first (YAML is a superset of JSON)
    serde_yaml::from_slice(data).map_err(|e| format!("parse error: {}", e))
}

/// Parse a JSON byte slice into a Workflow definition.
pub fn parse_json(data: &[u8]) -> Result<Workflow, String> {
    serde_json::from_slice(data).map_err(|e| format!("JSON parse error: {}", e))
}

/// Read and parse a file (YAML or JSON) into a Workflow definition.
pub fn parse_file(path: &Path) -> Result<Workflow, String> {
    let data = std::fs::read(path).map_err(|e| format!("read file {:?}: {}", path, e))?;
    parse(&data)
}

/// Validate a Workflow definition for correctness.
///
/// Checks:
/// - Workflow has a name
/// - At least one node exists
/// - All node IDs are unique
/// - All edge references point to valid nodes
/// - The graph is a valid DAG (no cycles)
/// - Trigger types are recognized
pub fn validate(wf: &Workflow) -> Result<(), String> {
    if wf.name.is_empty() {
        return Err("workflow must have a name".to_string());
    }

    if wf.nodes.is_empty() {
        return Err(format!(
            "workflow {:?} must have at least one node",
            wf.name
        ));
    }

    // Check unique node IDs
    let mut node_ids = HashSet::new();
    for n in &wf.nodes {
        if n.id.is_empty() {
            return Err(format!("node missing id in workflow {:?}", wf.name));
        }
        if node_ids.contains(&n.id) {
            return Err(format!(
                "duplicate node id {:?} in workflow {:?}",
                n.id, wf.name
            ));
        }
        node_ids.insert(n.id.clone());
    }

    // Validate edges reference existing nodes
    for (i, e) in wf.edges.iter().enumerate() {
        if !node_ids.contains(&e.from_node) {
            return Err(format!(
                "edge {} references unknown 'from' node {:?}",
                i, e.from_node
            ));
        }
        if !node_ids.contains(&e.to_node) {
            return Err(format!(
                "edge {} references unknown 'to' node {:?}",
                i, e.to_node
            ));
        }
    }

    // Validate DependsOn references
    for n in &wf.nodes {
        for dep in &n.depends_on {
            if !node_ids.contains(dep) {
                return Err(format!("node {:?} depends_on unknown node {:?}", n.id, dep));
            }
        }
    }

    // Reject pausing nodes (human_review) inside composite blocks
    // (parallel/loop). Composite executors key success off
    // `state != Failed`, so a child stuck in Waiting would be swallowed as
    // a completed branch — the workflow would finish without the human
    // approval that must gate downstream nodes (the 缺陷 9 invariant:
    // downstream never runs before review resolves). Composite children
    // also have no per-child checkpointing, so propagating Waiting would
    // re-execute every child (side effects doubled) on resume — the honest
    // form is rejection here, before anything runs.
    for n in &wf.nodes {
        if matches!(n.node_type.as_str(), "parallel" | "loop") {
            let config_value = serde_json::to_value(&n.config).unwrap_or(serde_json::Value::Null);
            reject_pausing_composite_children(&wf.name, &n.id, &config_value)?;
        }
    }

    // Check for cycles using topological sort
    if let Err(e) = topological_sort(&wf.nodes, &wf.edges) {
        return Err(format!("workflow {:?}: {}", wf.name, e));
    }

    // Validate trigger configs
    for (i, t) in wf.triggers.iter().enumerate() {
        match t.trigger_type.as_str() {
            "cron" | "webhook" | "event" | "message" => {}
            other => {
                return Err(format!("trigger {} has unknown type {:?}", i, other));
            }
        }
    }

    Ok(())
}

/// Walk a composite node's config for `human_review` children (recursive, so
/// nested parallel/loop blocks are covered) and reject the first one found.
/// Child shape mirrors `get_config_node_list`: `nodes` / `branches` arrays of
/// node objects with `node_type` (or legacy `type`).
fn reject_pausing_composite_children(
    workflow_name: &str,
    composite_id: &str,
    config: &serde_json::Value,
) -> Result<(), String> {
    for key in &["nodes", "branches"] {
        let Some(arr) = config.get(*key).and_then(|v| v.as_array()) else {
            continue;
        };
        for child in arr {
            let Some(obj) = child.as_object() else {
                continue;
            };
            let child_type = obj
                .get("node_type")
                .or_else(|| obj.get("type"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let child_id = obj.get("id").and_then(|v| v.as_str()).unwrap_or("(无 id)");
            if child_type == "human_review" {
                return Err(format!(
                    "workflow {:?}: composite 节点 {:?} 的 {} 子节点 {:?} 是 human_review——\
                     审批暂停类节点不支持放在 parallel/loop 内（子节点 Waiting 会被当成功吞掉，\
                     审批永远不发生）。请把它移到顶层作为独立节点",
                    workflow_name, composite_id, key, child_id
                ));
            }
            if matches!(child_type, "parallel" | "loop") {
                let child_config = obj
                    .get("config")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                reject_pausing_composite_children(workflow_name, composite_id, &child_config)?;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

//! YAML/JSON workflow parser and validator.
//!
//! Mirrors the Go `parser.go` with YAML/JSON parsing via serde,
//! and comprehensive validation (unique IDs, edge references, DAG check).

use std::collections::HashSet;
use std::path::Path;

use crate::types::Workflow;
use crate::scheduler::topological_sort;

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
        return Err(format!("workflow {:?} must have at least one node", wf.name));
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
                return Err(format!(
                    "node {:?} depends_on unknown node {:?}",
                    n.id, dep
                ));
            }
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

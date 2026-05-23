//! Workflow execution context - Variable storage, node results, and template resolution.
//!
//! Mirrors the Go `context.go` with variable storage, node result tracking,
//! and template resolution ({{variable}}, {{node_id.field}}, {{input.key}}).

use std::collections::HashMap;
use std::sync::RwLock;

use regex::Regex;

use crate::types::NodeResult;

/// Holds the execution state for a single workflow run.
///
/// Provides variable storage, node results, and template resolution.
/// Thread-safe via internal RwLock.
#[derive(Debug)]
pub struct WorkflowContext {
    /// Workflow variables.
    variables: RwLock<HashMap<String, String>>,
    /// Node execution results.
    node_results: RwLock<HashMap<String, NodeResult>>,
    /// Workflow input data.
    input: HashMap<String, serde_json::Value>,
}

// Need Clone for tests
impl Clone for WorkflowContext {
    fn clone(&self) -> Self {
        let vars = self.variables.read().unwrap().clone();
        let results = self.node_results.read().unwrap().clone();
        Self {
            variables: RwLock::new(vars),
            node_results: RwLock::new(results),
            input: self.input.clone(),
        }
    }
}

impl WorkflowContext {
    /// Create a new workflow execution context.
    pub fn new(input: HashMap<String, serde_json::Value>) -> Self {
        Self {
            variables: RwLock::new(HashMap::new()),
            node_results: RwLock::new(HashMap::new()),
            input,
        }
    }

    /// Set a workflow variable.
    pub fn set_var(&self, key: &str, value: &str) {
        self.variables
            .write()
            .unwrap()
            .insert(key.to_string(), value.to_string());
    }

    /// Get a workflow variable. Returns empty string if not found.
    pub fn get_var(&self, key: &str) -> String {
        self.variables
            .read()
            .unwrap()
            .get(key)
            .cloned()
            .unwrap_or_default()
    }

    /// Store the result of a node execution.
    pub fn set_node_result(&self, node_id: &str, result: NodeResult) {
        self.node_results
            .write()
            .unwrap()
            .insert(node_id.to_string(), result);
    }

    /// Retrieve the result of a previously executed node.
    pub fn get_node_result(&self, node_id: &str) -> Option<NodeResult> {
        self.node_results
            .read()
            .unwrap()
            .get(node_id)
            .cloned()
    }

    /// Return a copy of all workflow variables.
    pub fn get_all_variables(&self) -> HashMap<String, String> {
        self.variables.read().unwrap().clone()
    }

    /// Return a copy of all node results.
    pub fn get_all_node_results(&self) -> HashMap<String, NodeResult> {
        self.node_results.read().unwrap().clone()
    }

    /// Resolve template references in a string.
    ///
    /// Supported patterns:
    /// - `{{variable}}` - resolve from workflow variables
    /// - `{{node_id}}` - resolve full output of a node
    /// - `{{node_id.field}}` - resolve a specific field from a node's output
    /// - `{{input.key}}` - resolve from workflow input
    pub fn resolve(&self, template: &str) -> String {
        let re = Regex::new(r"\{\{([^}]+)\}\}").unwrap();

        re.replace_all(template, |caps: &regex::Captures| {
            let key = caps[1].trim();

            // Try input.key pattern
            if let Some(field) = key.strip_prefix("input.") {
                if let Some(val) = self.input.get(field) {
                    return value_to_string(val);
                }
                return caps[0].to_string(); // unresolved
            }

            // Try node_id.field pattern
            if let Some(dot_pos) = key.find('.') {
                let node_id = &key[..dot_pos];
                let field = &key[dot_pos + 1..];
                if let Some(result) = self.get_node_result(node_id) {
                    if let Some(obj) = result.output.as_object() {
                        if let Some(val) = obj.get(field) {
                            return value_to_string(val);
                        }
                    }
                }
                return caps[0].to_string(); // unresolved
            }

            // Try plain variable
            let var_val = self.get_var(key);
            if !var_val.is_empty() {
                return var_val;
            }

            // Try node output (full)
            if let Some(result) = self.get_node_result(key) {
                return value_to_string(&result.output);
            }

            caps[0].to_string() // unresolved
        })
        .to_string()
    }

    /// Create a shallow clone of this context.
    pub fn clone_context(&self) -> Self {
        self.clone()
    }
}

/// Convert a JSON value to a display string.
fn value_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

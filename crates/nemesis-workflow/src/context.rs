//! Workflow execution context - Variable storage, node results, and template resolution.
//!
//! Mirrors the Go `context.go` with variable storage, node results, and template
//! resolution ({{variable}}, {{node_id.field}}, {{input.key}}).
//!
//! Variables are stored as `serde_json::Value` (since milestone 1b-) so
//! workflows can carry arbitrary JSON structures between nodes. String
//! variables stay fully compatible with the old `HashMap<String, String>`
//! persistence format - serde transparently converts `Value::String` to/from
//! plain JSON strings, so existing JSONL snapshots load without migration.

use std::collections::HashMap;
use std::sync::RwLock;

use regex::Regex;

use crate::types::NodeResult;

/// Execution identity carried by one workflow run — who triggered it and
/// from where (安全闸补齐 A4 归因穿透).
///
/// Filled by the engine at `run_async` from the execution's trigger source
/// and consumed by node executors: the tool node's secure executor turns it
/// into audit attribution (`wf_instance` / `wf_node_id` / `wf_name` /
/// `wf_trigger` / `wf_depth` metadata) and into the per-call request context
/// that routes approval cards back to the triggering conversation and makes
/// session-scoped approval exemptions work. All-empty identity = legacy/test
/// paths constructed via [`WorkflowContext::new`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkflowIdentity {
    /// The execution this run belongs to (audit attribution).
    pub execution_id: String,
    /// Workflow name being executed.
    pub workflow_name: String,
    /// Human-readable trigger label (`"agent-tool"` / `"cron"` / `"chat"` /
    /// ...; see [`crate::types::TriggerSource::label`]).
    pub trigger_label: String,
    /// Triggering session key — drives the session-scoped approval
    /// exemptions (session allow-all).
    pub session_key: String,
    /// Triggering chat id — approval cards route back to this conversation.
    pub chat_id: String,
    /// Triggering sender id.
    pub sender_id: String,
    /// Agent-tool recursion depth at trigger time (0 for other triggers).
    pub recursion_depth: u32,
}

/// Holds the execution state for a single workflow run.
///
/// Provides variable storage, node results, and template resolution.
/// Thread-safe via interior `RwLock`.
#[derive(Debug)]
pub struct WorkflowContext {
    /// Workflow variables. Stored as JSON values since 1b so nodes can
    /// carry arrays, objects, numbers, etc. between stages.
    variables: RwLock<HashMap<String, serde_json::Value>>,
    /// Node execution results.
    node_results: RwLock<HashMap<String, NodeResult>>,
    /// Workflow input data.
    input: HashMap<String, serde_json::Value>,
    /// Per-run execution identity (who/where triggered; see
    /// [`WorkflowIdentity`]). Default = empty (legacy/test paths).
    identity: WorkflowIdentity,
}

impl Clone for WorkflowContext {
    fn clone(&self) -> Self {
        let vars = self.variables.read().unwrap().clone();
        let results = self.node_results.read().unwrap().clone();
        Self {
            variables: RwLock::new(vars),
            node_results: RwLock::new(results),
            input: self.input.clone(),
            identity: self.identity.clone(),
        }
    }
}

impl WorkflowContext {
    /// Create a new workflow execution context.
    ///
    /// Identity is left empty — this is the legacy/test constructor. The
    /// engine fills identity via [`Self::new_with_identity`].
    pub fn new(input: HashMap<String, serde_json::Value>) -> Self {
        Self {
            variables: RwLock::new(HashMap::new()),
            node_results: RwLock::new(HashMap::new()),
            input,
            identity: WorkflowIdentity::default(),
        }
    }

    /// Create a new workflow execution context with a filled execution
    /// identity (安全闸补齐 A4：归因/审批路由/会话豁免的身份随行).
    pub fn new_with_identity(
        identity: WorkflowIdentity,
        input: HashMap<String, serde_json::Value>,
    ) -> Self {
        Self {
            variables: RwLock::new(HashMap::new()),
            node_results: RwLock::new(HashMap::new()),
            input,
            identity,
        }
    }

    /// Per-run execution identity (who/where triggered this run).
    pub fn identity(&self) -> &WorkflowIdentity {
        &self.identity
    }

    /// Set a workflow variable to any JSON-serialisable value.
    ///
    /// `impl Into<serde_json::Value>` accepts:
    /// - `&str` / `String` → `Value::String`
    /// - `serde_json::json!(...)` literal
    /// - any `T: Serialize` via `serde_json::to_value(...)` (caller-side)
    pub fn set_var<V: Into<serde_json::Value>>(&self, key: &str, value: V) {
        self.variables
            .write()
            .unwrap()
            .insert(key.to_string(), value.into());
    }

    /// Get a workflow variable as a JSON value, if present.
    ///
    /// Returns `None` for missing keys. Use [`get_var_str`](Self::get_var_str)
    /// for the common string-scalar case.
    pub fn get_var(&self, key: &str) -> Option<serde_json::Value> {
        self.variables.read().unwrap().get(key).cloned()
    }

    /// Get a workflow variable as a string.
    ///
    /// Returns `Some(s)` if the key exists and holds a string value; `None`
    /// otherwise. For arbitrary JSON, use [`get_var`](Self::get_var) and
    /// render with `value_to_string`.
    pub fn get_var_str(&self, key: &str) -> Option<String> {
        match self.get_var(key) {
            Some(serde_json::Value::String(s)) => Some(s),
            Some(other) => Some(value_to_string(&other)),
            None => None,
        }
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
        self.node_results.read().unwrap().get(node_id).cloned()
    }

    /// Return a copy of all workflow variables as JSON values.
    pub fn get_all_variables(&self) -> HashMap<String, serde_json::Value> {
        self.variables.read().unwrap().clone()
    }

    /// Return a copy of all node results.
    pub fn get_all_node_results(&self) -> HashMap<String, NodeResult> {
        self.node_results.read().unwrap().clone()
    }

    /// Return a copy of the workflow input map.
    ///
    /// Used by the Checkpointer (1b-) when snapshotting context: input is
    /// private to the context but the snapshot needs a serialisable copy.
    pub fn get_all_input(&self) -> HashMap<String, serde_json::Value> {
        self.input.clone()
    }

    /// Resolve template references in a string.
    ///
    /// Supported patterns:
    /// - `{{variable}}` - resolve from workflow variables
    /// - `{{node_id}}` - resolve full output of a node
    /// - `{{node_id.field}}` - resolve a specific field from a node's output
    /// - `{{input.key}}` - resolve from workflow input
    ///
    /// For JSON-typed values (objects/arrays), the template renders the JSON
    /// encoding via `serde_json::to_string` (no whitespace). For string values
    /// the raw string is inlined (no quotes).
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
                if let Some(result) = self.get_node_result(node_id)
                    && let Some(obj) = result.output.as_object()
                    && let Some(val) = obj.get(field)
                {
                    return value_to_string(val);
                }
                return caps[0].to_string(); // unresolved
            }

            // Try plain variable (JSON-typed since 1b-).
            if let Some(val) = self.get_var(key) {
                return value_to_string(&val);
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

/// Convert a JSON value to a display string for template rendering.
///
/// - Strings → raw value (no quotes), matching Go's behaviour and old tests.
/// - `null` → empty string.
/// - Objects / arrays / numbers / bools → compact JSON encoding.
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

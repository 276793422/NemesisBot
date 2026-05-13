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
mod tests {
    use super::*;
    use crate::types::ExecutionState;
    use chrono::Utc;

    #[test]
    fn test_set_and_get_var() {
        let ctx = WorkflowContext::new(HashMap::new());
        ctx.set_var("name", "test");
        assert_eq!(ctx.get_var("name"), "test");
        assert_eq!(ctx.get_var("missing"), "");
    }

    #[test]
    fn test_set_and_get_node_result() {
        let ctx = WorkflowContext::new(HashMap::new());
        let now = Utc::now();
        let result = NodeResult {
            node_id: "n1".to_string(),
            output: serde_json::json!({"status": "ok", "count": 42}),
            error: None,
            state: ExecutionState::Completed,
            started_at: now,
            ended_at: now,
            metadata: HashMap::new(),
        };
        ctx.set_node_result("n1", result);

        let retrieved = ctx.get_node_result("n1").unwrap();
        assert_eq!(retrieved.state, ExecutionState::Completed);
        assert!(ctx.get_node_result("missing").is_none());
    }

    #[test]
    fn test_get_all_variables() {
        let ctx = WorkflowContext::new(HashMap::new());
        ctx.set_var("a", "1");
        ctx.set_var("b", "2");
        let vars = ctx.get_all_variables();
        assert_eq!(vars.len(), 2);
        assert_eq!(vars["a"], "1");
        assert_eq!(vars["b"], "2");
    }

    #[test]
    fn test_resolve_variable() {
        let ctx = WorkflowContext::new(HashMap::new());
        ctx.set_var("name", "world");
        assert_eq!(ctx.resolve("hello {{name}}"), "hello world");
    }

    #[test]
    fn test_resolve_input() {
        let mut input = HashMap::new();
        input.insert("key".to_string(), serde_json::json!("value123"));
        let ctx = WorkflowContext::new(input);
        assert_eq!(ctx.resolve("{{input.key}}"), "value123");
    }

    #[test]
    fn test_resolve_node_field() {
        let ctx = WorkflowContext::new(HashMap::new());
        let now = Utc::now();
        let result = NodeResult {
            node_id: "step1".to_string(),
            output: serde_json::json!({"url": "http://example.com", "status": 200}),
            error: None,
            state: ExecutionState::Completed,
            started_at: now,
            ended_at: now,
            metadata: HashMap::new(),
        };
        ctx.set_node_result("step1", result);

        assert_eq!(
            ctx.resolve("Result: {{step1.url}}"),
            "Result: http://example.com"
        );
        assert_eq!(ctx.resolve("{{step1.status}}"), "200");
    }

    #[test]
    fn test_resolve_unresolved() {
        let ctx = WorkflowContext::new(HashMap::new());
        assert_eq!(ctx.resolve("{{unknown}}"), "{{unknown}}");
    }

    #[test]
    fn test_resolve_node_full_output() {
        let ctx = WorkflowContext::new(HashMap::new());
        let now = Utc::now();
        let result = NodeResult {
            node_id: "n1".to_string(),
            output: serde_json::json!("direct_output"),
            error: None,
            state: ExecutionState::Completed,
            started_at: now,
            ended_at: now,
            metadata: HashMap::new(),
        };
        ctx.set_node_result("n1", result);
        assert_eq!(ctx.resolve("{{n1}}"), "direct_output");
    }

    #[test]
    fn test_clone_context() {
        let ctx = WorkflowContext::new(HashMap::new());
        ctx.set_var("x", "10");
        let clone = ctx.clone_context();
        assert_eq!(clone.get_var("x"), "10");
    }

    #[test]
    fn test_set_var_overwrite() {
        let ctx = WorkflowContext::new(HashMap::new());
        ctx.set_var("key", "old");
        assert_eq!(ctx.get_var("key"), "old");
        ctx.set_var("key", "new");
        assert_eq!(ctx.get_var("key"), "new");
    }

    #[test]
    fn test_get_all_node_results_empty() {
        let ctx = WorkflowContext::new(HashMap::new());
        let results = ctx.get_all_node_results();
        assert!(results.is_empty());
    }

    #[test]
    fn test_get_all_variables_empty() {
        let ctx = WorkflowContext::new(HashMap::new());
        let vars = ctx.get_all_variables();
        assert!(vars.is_empty());
    }

    #[test]
    fn test_multiple_node_results() {
        let ctx = WorkflowContext::new(HashMap::new());
        let now = chrono::Utc::now();

        for i in 0..5 {
            let result = NodeResult {
                node_id: format!("n{}", i),
                output: serde_json::json!({"index": i}),
                error: None,
                state: ExecutionState::Completed,
                started_at: now,
                ended_at: now,
                metadata: HashMap::new(),
            };
            ctx.set_node_result(&format!("n{}", i), result);
        }

        let results = ctx.get_all_node_results();
        assert_eq!(results.len(), 5);
        for i in 0..5 {
            assert!(results.contains_key(&format!("n{}", i)));
        }
    }

    #[test]
    fn test_resolve_multiple_variables() {
        let ctx = WorkflowContext::new(HashMap::new());
        ctx.set_var("greeting", "Hello");
        ctx.set_var("name", "World");
        assert_eq!(ctx.resolve("{{greeting}} {{name}}!"), "Hello World!");
    }

    #[test]
    fn test_resolve_adjacent_variables() {
        let ctx = WorkflowContext::new(HashMap::new());
        ctx.set_var("a", "foo");
        ctx.set_var("b", "bar");
        assert_eq!(ctx.resolve("{{a}}{{b}}"), "foobar");
    }

    #[test]
    fn test_resolve_input_missing_field() {
        let input = HashMap::new();
        let ctx = WorkflowContext::new(input);
        assert_eq!(ctx.resolve("{{input.missing}}"), "{{input.missing}}");
    }

    #[test]
    fn test_resolve_node_field_missing() {
        let ctx = WorkflowContext::new(HashMap::new());
        let now = chrono::Utc::now();
        let result = NodeResult {
            node_id: "n1".to_string(),
            output: serde_json::json!({"existing": "value"}),
            error: None,
            state: ExecutionState::Completed,
            started_at: now,
            ended_at: now,
            metadata: HashMap::new(),
        };
        ctx.set_node_result("n1", result);
        assert_eq!(ctx.resolve("{{n1.missing}}"), "{{n1.missing}}");
    }

    #[test]
    fn test_resolve_plain_text_no_templates() {
        let ctx = WorkflowContext::new(HashMap::new());
        assert_eq!(ctx.resolve("no templates here"), "no templates here");
    }

    #[test]
    fn test_resolve_empty_string() {
        let ctx = WorkflowContext::new(HashMap::new());
        assert_eq!(ctx.resolve(""), "");
    }

    #[test]
    fn test_resolve_mixed_resolved_unresolved() {
        let ctx = WorkflowContext::new(HashMap::new());
        ctx.set_var("known", "value");
        let result = ctx.resolve("{{known}} and {{unknown}}");
        assert_eq!(result, "value and {{unknown}}");
    }

    #[test]
    fn test_overwrite_node_result() {
        let ctx = WorkflowContext::new(HashMap::new());
        let now = chrono::Utc::now();

        let result1 = NodeResult {
            node_id: "n1".to_string(),
            output: serde_json::json!({"v": 1}),
            error: None,
            state: ExecutionState::Running,
            started_at: now,
            ended_at: now,
            metadata: HashMap::new(),
        };
        ctx.set_node_result("n1", result1);

        let result2 = NodeResult {
            node_id: "n1".to_string(),
            output: serde_json::json!({"v": 2}),
            error: None,
            state: ExecutionState::Completed,
            started_at: now,
            ended_at: now,
            metadata: HashMap::new(),
        };
        ctx.set_node_result("n1", result2);

        let retrieved = ctx.get_node_result("n1").unwrap();
        assert_eq!(retrieved.state, ExecutionState::Completed);
        assert_eq!(retrieved.output["v"], 2);
    }

    #[test]
    fn test_input_preserved_after_clone() {
        let mut input = HashMap::new();
        input.insert("key".to_string(), serde_json::json!("input_value"));
        let ctx = WorkflowContext::new(input);
        let clone = ctx.clone_context();
        assert_eq!(clone.resolve("{{input.key}}"), "input_value");
    }

    #[test]
    fn test_resolve_node_non_object_output() {
        let ctx = WorkflowContext::new(HashMap::new());
        let now = chrono::Utc::now();
        let result = NodeResult {
            node_id: "n1".to_string(),
            output: serde_json::json!("string_output"),
            error: None,
            state: ExecutionState::Completed,
            started_at: now,
            ended_at: now,
            metadata: HashMap::new(),
        };
        ctx.set_node_result("n1", result);
        // Full output resolution
        assert_eq!(ctx.resolve("{{n1}}"), "string_output");
        // Field resolution should fail for non-object
        assert_eq!(ctx.resolve("{{n1.field}}"), "{{n1.field}}");
    }

    #[test]
    fn test_context_debug_format() {
        let ctx = WorkflowContext::new(HashMap::new());
        ctx.set_var("test", "value");
        let debug_str = format!("{:?}", ctx);
        assert!(!debug_str.is_empty());
    }
}

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

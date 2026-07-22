use super::*;
use crate::types::ExecutionState;
use chrono::Local;
use serde_json::json;

#[test]
fn test_set_and_get_var() {
    let ctx = WorkflowContext::new(HashMap::new());
    ctx.set_var("name", "test");
    assert_eq!(ctx.get_var_str("name"), Some("test".to_string()));
    assert_eq!(ctx.get_var_str("missing"), None);
}

#[test]
fn test_set_and_get_node_result() {
    let ctx = WorkflowContext::new(HashMap::new());
    let now = Local::now();
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
    assert_eq!(vars["a"], json!("1"));
    assert_eq!(vars["b"], json!("2"));
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
    let now = Local::now();
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
    let now = Local::now();
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
    assert_eq!(clone.get_var_str("x"), Some("10".to_string()));
}

#[test]
fn test_set_var_overwrite() {
    let ctx = WorkflowContext::new(HashMap::new());
    ctx.set_var("key", "old");
    assert_eq!(ctx.get_var_str("key"), Some("old".to_string()));
    ctx.set_var("key", "new");
    assert_eq!(ctx.get_var_str("key"), Some("new".to_string()));
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
    let now = chrono::Local::now();

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
    let now = Local::now();
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
    let now = Local::now();

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
    let now = Local::now();
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

// ===========================================================================
// 1b-B3: variables JSON 化 — new tests
// ===========================================================================

#[test]
fn test_set_get_complex_value_array() {
    let ctx = WorkflowContext::new(HashMap::new());
    let arr = json!([1, 2, 3, "four"]);
    ctx.set_var("items", arr.clone());
    assert_eq!(ctx.get_var("items"), Some(arr));
}

#[test]
fn test_set_get_complex_value_object() {
    let ctx = WorkflowContext::new(HashMap::new());
    let obj = json!({
        "users": [{"id": 1, "name": "alice"}, {"id": 2, "name": "bob"}],
        "count": 2,
        "meta": {"page": 1, "total": 100}
    });
    ctx.set_var("data", obj.clone());
    assert_eq!(ctx.get_var("data"), Some(obj));
}

#[test]
fn test_set_get_complex_value_number_bool_null() {
    let ctx = WorkflowContext::new(HashMap::new());
    ctx.set_var("count", json!(42));
    ctx.set_var("pi", json!(3.14));
    ctx.set_var("flag", json!(true));
    ctx.set_var("empty", json!(null));

    assert_eq!(ctx.get_var("count"), Some(json!(42)));
    assert_eq!(ctx.get_var("pi"), Some(json!(3.14)));
    assert_eq!(ctx.get_var("flag"), Some(json!(true)));
    assert_eq!(ctx.get_var("empty"), Some(json!(null)));
}

#[test]
fn test_template_renders_object_as_json() {
    let ctx = WorkflowContext::new(HashMap::new());
    ctx.set_var("data", json!({"name": "alice", "age": 30}));
    // Object should render as compact JSON (no whitespace).
    // Note: serde_json::Map stores keys sorted (BTreeMap), so the rendered
    // output is canonical, not insertion-order.
    let resolved = ctx.resolve("payload={{data}}");
    assert_eq!(resolved, r#"payload={"age":30,"name":"alice"}"#);
}

#[test]
fn test_template_renders_array_as_json() {
    let ctx = WorkflowContext::new(HashMap::new());
    ctx.set_var("items", json!([1, 2, 3]));
    assert_eq!(ctx.resolve("{{items}}"), "[1,2,3]");
}

#[test]
fn test_template_renders_number_as_json() {
    let ctx = WorkflowContext::new(HashMap::new());
    ctx.set_var("count", json!(42));
    assert_eq!(ctx.resolve("count={{count}}"), "count=42");

    ctx.set_var("pi", json!(3.14));
    assert_eq!(ctx.resolve("pi={{pi}}"), "pi=3.14");

    ctx.set_var("flag", json!(true));
    assert_eq!(ctx.resolve("flag={{flag}}"), "flag=true");

    ctx.set_var("empty", json!(null));
    assert_eq!(ctx.resolve("x={{empty}}"), "x=");
}

#[test]
fn test_template_renders_string_without_quotes() {
    // Strings inline raw (no surrounding quotes) — backward compat with
    // the old `HashMap<String, String>` behaviour.
    let ctx = WorkflowContext::new(HashMap::new());
    ctx.set_var("name", "alice");
    assert_eq!(ctx.resolve("hi {{name}}"), "hi alice");
}

#[test]
fn test_old_string_format_compat() {
    // set_var with &str produces Value::String — transparent upgrade.
    let ctx = WorkflowContext::new(HashMap::new());
    ctx.set_var("k", "v");
    match ctx.get_var("k") {
        Some(serde_json::Value::String(s)) => assert_eq!(s, "v"),
        other => panic!("expected Value::String, got {:?}", other),
    }
}

#[test]
fn test_load_old_jsonl_file_string_vars() {
    // Simulates loading an old JSONL snapshot where variables was a
    // `HashMap<String, String>`. serde_json should deserialise each string
    // value as `Value::String`, so no explicit migration is needed.
    //
    // The Execution.variables type is `HashMap<String, serde_json::Value>`
    // since 1b-B3; an old JSONL line looks like:
    //   {"variables": {"name": "alice", "count": "42"}, ...}
    // and the deserialised Value type for "alice" and "42" is String.
    let old_json = r#"{
        "name": "old_exec",
        "workflow_name": "wf",
        "variables": {
            "name": "alice",
            "count": "42",
            "flag": "true"
        }
    }"#;

    #[derive(serde::Deserialize)]
    struct OldSnapshot {
        #[allow(dead_code)]
        name: String,
        #[allow(dead_code)]
        workflow_name: String,
        variables: HashMap<String, serde_json::Value>,
    }

    let snap: OldSnapshot = serde_json::from_str(old_json).unwrap();
    assert_eq!(snap.variables["name"], json!("alice"));
    assert_eq!(snap.variables["count"], json!("42"));
    assert_eq!(snap.variables["flag"], json!("true"));
}

#[test]
fn test_get_var_str_falls_back_to_json_render() {
    // Non-string scalars: get_var_str renders via value_to_string (so callers
    // that just want "a string-ish representation" keep working without
    // pattern matching on Value).
    let ctx = WorkflowContext::new(HashMap::new());
    ctx.set_var("n", json!(42));
    assert_eq!(ctx.get_var_str("n"), Some("42".to_string()));
    ctx.set_var("b", json!(true));
    assert_eq!(ctx.get_var_str("b"), Some("true".to_string()));
    ctx.set_var("o", json!({"x":1}));
    assert_eq!(ctx.get_var_str("o"), Some(r#"{"x":1}"#.to_string()));
}

#[test]
fn test_get_var_str_returns_none_for_missing() {
    let ctx = WorkflowContext::new(HashMap::new());
    assert_eq!(ctx.get_var_str("missing"), None);
}

#[test]
fn test_set_var_accepts_string_literal() {
    // &str implements Into<Value> via the blanket impl, so the old
    // ctx.set_var("k", "v") call site keeps working.
    let ctx = WorkflowContext::new(HashMap::new());
    ctx.set_var("k", "v");
    assert_eq!(ctx.get_var_str("k"), Some("v".to_string()));
}

#[test]
fn test_set_var_accepts_owned_string() {
    let ctx = WorkflowContext::new(HashMap::new());
    let s = String::from("owned");
    ctx.set_var("k", s);
    assert_eq!(ctx.get_var_str("k"), Some("owned".to_string()));
}

#[test]
fn test_set_var_accepts_json_macro() {
    let ctx = WorkflowContext::new(HashMap::new());
    ctx.set_var("data", json!({"a": 1, "b": [2, 3]}));
    match ctx.get_var("data") {
        Some(v) => assert_eq!(v["a"], 1),
        None => panic!("expected value"),
    }
}

#[test]
fn test_get_var_preserves_type() {
    let ctx = WorkflowContext::new(HashMap::new());
    ctx.set_var("n", json!(42));
    // Number type is preserved, not stringified on read.
    match ctx.get_var("n") {
        Some(serde_json::Value::Number(n)) => {
            assert_eq!(n.as_i64(), Some(42));
        }
        other => panic!("expected Number, got {:?}", other),
    }
}

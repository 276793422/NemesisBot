use super::*;
use chrono::Local;
use nemesis_workflow::types::{Edge, Execution, ExecutionState, NodeDef, NodeResult, Workflow};
use std::collections::HashMap;

fn make_node(id: &str, is_terminal: bool) -> NodeDef {
    NodeDef {
        id: id.to_string(),
        node_type: "transform".to_string(),
        config: HashMap::new(),
        depends_on: Vec::new(),
        retry_count: 0,
        timeout: None,
        is_terminal,
    }
}

fn make_result(id: &str, output: serde_json::Value) -> NodeResult {
    let now = Local::now();
    NodeResult {
        node_id: id.to_string(),
        output,
        error: None,
        state: ExecutionState::Completed,
        started_at: now,
        ended_at: now,
        metadata: HashMap::new(),
    }
}

fn make_execution(node_results: Vec<(&str, serde_json::Value)>) -> Execution {
    let mut nr_map = HashMap::new();
    for (id, v) in node_results {
        nr_map.insert(id.to_string(), make_result(id, v));
    }
    let mut exec = Execution::new("test_workflow".to_string(), HashMap::new());
    exec.node_results = nr_map;
    exec
}

/// Single terminal node with bare string output → returned as-is.
/// This is the canonical output_type=text/markdown/xml path.
#[test]
fn test_bare_string_terminal_output_returned_directly() {
    let workflow = Workflow {
        name: "test_workflow".to_string(),
        description: String::new(),
        version: "1.0.0".to_string(),
        triggers: Vec::new(),
        nodes: vec![make_node("format_reply", true)],
        edges: Vec::new(),
        variables: HashMap::new(),
        metadata: HashMap::new(),
    };
    let exec = make_execution(vec![(
        "format_reply",
        serde_json::Value::String("hello world".to_string()),
    )]);
    let reply = build_completed_reply_with_workflow(&workflow, &exec);
    assert_eq!(reply, "hello world");
}

/// Bare string output on a leaf node (no explicit is_terminal) — same
/// behavior via the fallback path.
#[test]
fn test_bare_string_leaf_output_returned_directly() {
    let workflow = Workflow {
        name: "test_workflow".to_string(),
        description: String::new(),
        version: "1.0.0".to_string(),
        triggers: Vec::new(),
        nodes: vec![make_node("upstream", false), make_node("leaf", false)],
        edges: vec![Edge {
            from_node: "upstream".to_string(),
            to_node: "leaf".to_string(),
            condition: None,
        }],
        variables: HashMap::new(),
        metadata: HashMap::new(),
    };
    let exec = make_execution(vec![
        ("upstream", serde_json::json!({"text": "ignored"})),
        (
            "leaf",
            serde_json::Value::String("**译文**：hello".to_string()),
        ),
    ]);
    let reply = build_completed_reply_with_workflow(&workflow, &exec);
    assert_eq!(reply, "**译文**：hello");
}

/// Legacy agent-node path: terminal output is an object with a
/// `response` field. Should still work via the merged-output branch.
#[test]
fn test_agent_response_field_still_works() {
    let workflow = Workflow {
        name: "test_workflow".to_string(),
        description: String::new(),
        version: "1.0.0".to_string(),
        triggers: Vec::new(),
        nodes: vec![make_node("agent_1", true)],
        edges: Vec::new(),
        variables: HashMap::new(),
        metadata: HashMap::new(),
    };
    let exec = make_execution(vec![(
        "agent_1",
        serde_json::json!({"response": "I am the agent reply"}),
    )]);
    let reply = build_completed_reply_with_workflow(&workflow, &exec);
    assert_eq!(reply, "I am the agent reply");
}

/// Empty bare string falls through the bare-string priority path (the
/// `!s.is_empty()` guard treats empty as "no real reply") and ends up
/// JSON-dumped via the merged-output fallback. This is intentional — an
/// empty transform output shouldn't be served to webchat as a blank
/// reply just because it has the right shape.
#[test]
fn test_empty_bare_string_falls_through() {
    let workflow = Workflow {
        name: "test_workflow".to_string(),
        description: String::new(),
        version: "1.0.0".to_string(),
        triggers: Vec::new(),
        nodes: vec![make_node("format_reply", true)],
        edges: Vec::new(),
        variables: HashMap::new(),
        metadata: HashMap::new(),
    };
    let exec = make_execution(vec![(
        "format_reply",
        serde_json::Value::String(String::new()),
    )]);
    let reply = build_completed_reply_with_workflow(&workflow, &exec);
    // compute_output wraps the empty string as {format_reply: ""} since
    // it's a non-object, non-null value — JSON-dump shows that shape.
    assert!(reply.contains("\"format_reply\""));
    assert!(reply.contains("\"\""));
}

/// JSON envelope output (no output_type configured) → JSON-dumped.
/// This is the legacy behavior, preserved for backward compat.
#[test]
fn test_json_envelope_output_dumped_as_json() {
    let workflow = Workflow {
        name: "test_workflow".to_string(),
        description: String::new(),
        version: "1.0.0".to_string(),
        triggers: Vec::new(),
        nodes: vec![make_node("code_1", true)],
        edges: Vec::new(),
        variables: HashMap::new(),
        metadata: HashMap::new(),
    };
    let exec = make_execution(vec![(
        "code_1",
        serde_json::json!({"lines": ["a", "b", "c"]}),
    )]);
    let reply = build_completed_reply_with_workflow(&workflow, &exec);
    assert!(reply.contains("\"lines\""));
    assert!(reply.contains("\"a\""));
}

/// Multiple terminal nodes, first one with bare string wins. Order
/// matches the workflow.nodes declaration order.
#[test]
fn test_first_terminal_with_bare_string_wins() {
    let workflow = Workflow {
        name: "test_workflow".to_string(),
        description: String::new(),
        version: "1.0.0".to_string(),
        triggers: Vec::new(),
        nodes: vec![make_node("first", true), make_node("second", true)],
        edges: Vec::new(),
        variables: HashMap::new(),
        metadata: HashMap::new(),
    };
    let exec = make_execution(vec![
        (
            "first",
            serde_json::Value::String("first reply".to_string()),
        ),
        ("second", serde_json::json!({"response": "second reply"})),
    ]);
    let reply = build_completed_reply_with_workflow(&workflow, &exec);
    assert_eq!(reply, "first reply");
}

/// No node results at all → "[工作流完成，无输出]".
#[test]
fn test_no_node_results_returns_no_output_message() {
    let workflow = Workflow {
        name: "test_workflow".to_string(),
        description: String::new(),
        version: "1.0.0".to_string(),
        triggers: Vec::new(),
        nodes: vec![make_node("only", true)],
        edges: Vec::new(),
        variables: HashMap::new(),
        metadata: HashMap::new(),
    };
    let exec = make_execution(Vec::new());
    let reply = build_completed_reply_with_workflow(&workflow, &exec);
    assert_eq!(reply, "[工作流完成，无输出]");
}

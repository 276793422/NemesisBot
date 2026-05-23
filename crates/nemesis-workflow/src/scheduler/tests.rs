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

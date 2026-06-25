use super::*;
use crate::engine::WorkflowEngine;
use crate::types::Workflow;

fn make_node(id: &str, node_type: &str, config: HashMap<String, serde_json::Value>) -> NodeDef {
    NodeDef {
        id: id.to_string(),
        node_type: node_type.to_string(),
        config,
        depends_on: vec![],
        retry_count: 0,
        timeout: None,
    is_terminal: false,
    }
}

/// Helper to create an empty WorkflowContext for tests.
fn empty_wf_ctx() -> WorkflowContext {
    WorkflowContext::new(HashMap::new())
}

#[tokio::test]
async fn test_llm_node_executor() {
    let exec = LLMNodeExecutor;
    let mut config = HashMap::new();
    config.insert("prompt".to_string(), serde_json::json!("Hello"));
    let node = make_node("n1", "llm", config);
    let ctx = empty_wf_ctx();
    let result = exec.execute(&node, &HashMap::new(), &ctx).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    assert!(result.error.is_none());
    let text = result.output["text"].as_str().unwrap();
    assert!(text.contains("Hello"));
}

#[tokio::test]
async fn test_condition_node_executor() {
    let exec = ConditionNodeExecutor;
    let mut config = HashMap::new();
    config.insert("condition".to_string(), serde_json::json!("status == ok"));
    let node = make_node("n1", "condition", config);

    let mut ctx = HashMap::new();
    ctx.insert("status".to_string(), serde_json::json!("ok"));

    let result = exec.execute(&node, &ctx, &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    assert!(result.output["condition_result"].as_bool().unwrap());
}

#[tokio::test]
async fn test_delay_node_executor() {
    let exec = DelayNodeExecutor;
    let mut config = HashMap::new();
    config.insert("seconds".to_string(), serde_json::json!(10));
    let node = make_node("n1", "delay", config);
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
}

#[tokio::test]
async fn test_parallel_node_executor_with_registry() {
    let registry = NodeExecutorRegistry::new_with_composite();
    let exec = registry.get("parallel").unwrap();

    let mut config = HashMap::new();
    config.insert(
        "nodes".to_string(),
        serde_json::json!([
            { "id": "a", "node_type": "llm", "config": { "prompt": "hello" } },
            { "id": "b", "node_type": "tool", "config": { "tool": "grep" } },
        ]),
    );
    let node = make_node("n1", "parallel", config);
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    let obj = result.output.as_object().unwrap();
    // Should have branch_0 and branch_1
    assert!(obj.contains_key("branch_0"));
    assert!(obj.contains_key("branch_1"));
}

#[tokio::test]
async fn test_parallel_node_stub() {
    let exec = ParallelNodeStub;
    let mut config = HashMap::new();
    config.insert(
        "nodes".to_string(),
        serde_json::json!([
            { "id": "a", "node_type": "delay", "config": { "seconds": 0 } },
            { "id": "b", "node_type": "delay", "config": { "seconds": 0 } },
            { "id": "c", "node_type": "delay", "config": { "seconds": 0 } },
        ]),
    );
    let node = make_node("n1", "parallel", config);
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    let obj = result.output.as_object().unwrap();
    // Should have results for each child node
    assert!(obj.contains_key("a"));
    assert!(obj.contains_key("b"));
    assert!(obj.contains_key("c"));
}

#[tokio::test]
async fn test_loop_node_executor_with_registry() {
    let registry = NodeExecutorRegistry::new_with_composite();
    let exec = registry.get("loop").unwrap();

    let mut config = HashMap::new();
    config.insert("max_iterations".to_string(), serde_json::json!(3));
    config.insert(
        "nodes".to_string(),
        serde_json::json!([
            { "id": "inner", "node_type": "llm", "config": { "prompt": "loop" } }
        ]),
    );
    let node = make_node("n1", "loop", config);
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    assert_eq!(result.output["iterations"].as_u64().unwrap(), 3);
}

#[tokio::test]
async fn test_loop_node_stub() {
    let exec = LoopNodeStub;
    let mut config = HashMap::new();
    config.insert("max_iterations".to_string(), serde_json::json!(5));
    config.insert(
        "nodes".to_string(),
        serde_json::json!([
            { "id": "inner", "node_type": "delay", "config": { "seconds": 0 } }
        ]),
    );
    let node = make_node("n1", "loop", config);
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    assert_eq!(result.output["iterations"].as_u64().unwrap(), 5);
}

#[tokio::test]
async fn test_sub_workflow_node_stub() {
    let exec = SubWorkflowNodeStub;
    let mut config = HashMap::new();
    config.insert("workflow".to_string(), serde_json::json!("child_wf"));
    let node = make_node("n1", "sub_workflow", config);
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    // Without an engine, the stub returns Failed with a descriptive error
    assert_eq!(result.state, ExecutionState::Failed);
    assert!(result.error.unwrap().contains("engine configured"));
    assert_eq!(
        result.output["sub_workflow"].as_str().unwrap(),
        "child_wf"
    );
}

#[tokio::test]
async fn test_sub_workflow_missing_config() {
    let exec = SubWorkflowNodeStub;
    let node = make_node("n1", "sub_workflow", HashMap::new());
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Failed);
    assert!(result.error.unwrap().contains("workflow"));
}

#[tokio::test]
async fn test_http_node_executor() {
    let exec = HTTPNodeExecutor;
    let mut config = HashMap::new();
    config.insert("url".to_string(), serde_json::json!("http://example.com/api"));
    config.insert("method".to_string(), serde_json::json!("POST"));
    let node = make_node("n1", "http", config);
    // This will attempt a real HTTP request - in tests it may fail if no server.
    // We test the error path (connection refused) gracefully.
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await;
    // With a real URL it may succeed or fail depending on network, but it should not panic.
    match result {
        Ok(r) => {
            // If it succeeds, check structure
            assert!(r.output.get("status_code").is_some() || r.error.is_some());
        }
        Err(_) => {
            // Network error is fine for a test
        }
    }
}

#[tokio::test]
async fn test_http_node_missing_url() {
    let exec = HTTPNodeExecutor;
    let node = make_node("n1", "http", HashMap::new());
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Failed);
}

#[tokio::test]
async fn test_script_node_executor() {
    let exec = ScriptNodeExecutor;
    let mut config = HashMap::new();
    config.insert("script".to_string(), serde_json::json!("echo hello"));
    config.insert("language".to_string(), serde_json::json!("bash"));
    let node = make_node("n1", "script", config);
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    // New implementation returns stdout, stderr, exit_code
    assert_eq!(result.output["exit_code"].as_i64().unwrap(), 0);
    assert!(result.output["stdout"].as_str().unwrap().contains("hello"));
}

#[tokio::test]
async fn test_human_review_node_executor() {
    let exec = HumanReviewNodeExecutor;
    let mut config = HashMap::new();
    config.insert("message".to_string(), serde_json::json!("Please review"));
    let node = make_node("n1", "human_review", config);
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Waiting);
    assert_eq!(
        result.output["status"].as_str().unwrap(),
        "waiting_for_review"
    );
}

#[test]
fn test_registry_has_all_node_types() {
    let registry = NodeExecutorRegistry::new();
    let types = registry.node_types();
    assert!(types.contains(&"llm".to_string()));
    assert!(types.contains(&"tool".to_string()));
    assert!(types.contains(&"condition".to_string()));
    assert!(types.contains(&"parallel".to_string()));
    assert!(types.contains(&"loop".to_string()));
    assert!(types.contains(&"sub_workflow".to_string()));
    assert!(types.contains(&"transform".to_string()));
    assert!(types.contains(&"http".to_string()));
    assert!(types.contains(&"script".to_string()));
    assert!(types.contains(&"delay".to_string()));
    assert!(types.contains(&"human_review".to_string()));
    assert_eq!(types.len(), 11);
}

#[test]
fn test_registry_custom_executor() {
    let registry = NodeExecutorRegistry::new();
    registry.register("custom", Arc::new(LLMNodeExecutor));
    assert!(registry.get("custom").is_some());
}

#[test]
fn test_get_config_node_list() {
    let mut config = HashMap::new();
    config.insert(
        "nodes".to_string(),
        serde_json::json!([
            { "id": "n1", "node_type": "llm", "config": { "prompt": "hello" } },
            { "id": "n2", "node_type": "tool", "config": { "tool": "grep" }, "depends_on": ["n1"] }
        ]),
    );

    let nodes = get_config_node_list(&config, "nodes");
    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0].id, "n1");
    assert_eq!(nodes[0].node_type, "llm");
    assert_eq!(nodes[1].id, "n2");
    assert_eq!(nodes[1].node_type, "tool");
    assert_eq!(nodes[1].depends_on, vec!["n1".to_string()]);
}

#[test]
fn test_get_config_node_list_empty() {
    let config = HashMap::new();
    let nodes = get_config_node_list(&config, "nodes");
    assert!(nodes.is_empty());
}

#[test]
fn test_get_config_node_list_fallback_branches() {
    let mut config = HashMap::new();
    config.insert(
        "branches".to_string(),
        serde_json::json!([{ "id": "b1", "node_type": "llm" }]),
    );

    let nodes = get_config_node_list(&config, "branches");
    assert_eq!(nodes.len(), 1);
}

// ============================================================
// Additional nodes tests: registry, transform, template, edge cases
// ============================================================

#[test]
fn test_node_executor_registry_default() {
    let registry = NodeExecutorRegistry::default();
    let types = registry.node_types();
    assert_eq!(types.len(), 11);
}

#[test]
fn test_node_executor_registry_get_nonexistent() {
    let registry = NodeExecutorRegistry::new();
    assert!(registry.get("nonexistent").is_none());
}

#[test]
fn test_node_executor_registry_register_overwrite() {
    let registry = NodeExecutorRegistry::new();
    // Overwrite the existing "llm" executor
    registry.register("llm", Arc::new(DelayNodeExecutor));
    // Should return the new executor (no panic)
    assert!(registry.get("llm").is_some());
}

#[test]
fn test_node_executor_registry_new_with_composite() {
    let registry = NodeExecutorRegistry::new_with_composite();
    assert!(registry.get("parallel").is_some());
    assert!(registry.get("loop").is_some());
    assert!(registry.get("llm").is_some());
}

#[test]
fn test_registry_concurrent_access() {
    // Verify that the RwLock-backed registry is safe under concurrent
    // read/write access from multiple threads. Regression guard for the
    // unsafe removal refactor (1a-A3).
    use std::sync::Arc;
    use std::thread;

    let registry = Arc::new(NodeExecutorRegistry::new());
    let mut handles = Vec::new();

    // Writer thread: continuously register custom executors
    let writer_reg = Arc::clone(&registry);
    let writer = thread::spawn(move || {
        for i in 0..50 {
            writer_reg.register(
                &format!("custom_{}", i),
                Arc::new(crate::nodes::DelayNodeExecutor),
            );
        }
    });
    handles.push(writer);

    // Reader threads: continuously look up types
    for _ in 0..4 {
        let reader_reg = Arc::clone(&registry);
        let reader = thread::spawn(move || {
            for i in 0..50 {
                let _ = reader_reg.get(&format!("custom_{}", i));
                let _ = reader_reg.get("llm");
                let _ = reader_reg.node_types();
            }
        });
        handles.push(reader);
    }

    for h in handles {
        h.join().unwrap();
    }

    // After concurrent writes complete, all custom types should be present.
    for i in 0..50 {
        assert!(registry.get(&format!("custom_{}", i)).is_some(), "custom_{} missing", i);
    }
}

#[tokio::test]
async fn test_transform_node_executor_jsonpath() {
    let exec = TransformNodeExecutor;
    let mut config = HashMap::new();
    config.insert("expression".to_string(), serde_json::json!("$.name"));
    let node = make_node("n1", "transform", config);

    let mut ctx = HashMap::new();
    ctx.insert("data".to_string(), serde_json::json!({"name": "test-value"}));

    let result = exec.execute(&node, &ctx, &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
}

#[tokio::test]
async fn test_transform_node_executor_template() {
    let exec = TransformNodeExecutor;
    let mut config = HashMap::new();
    config.insert("template".to_string(), serde_json::json!("Hello {{name}}"));
    let node = make_node("n1", "transform", config);

    let mut ctx = HashMap::new();
    ctx.insert("name".to_string(), serde_json::json!("World"));

    let result = exec.execute(&node, &ctx, &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
}

#[tokio::test]
async fn test_transform_node_default_identity() {
    let exec = TransformNodeExecutor;
    let node = make_node("n1", "transform", HashMap::new());
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    // Default is identity - should pass through context
    assert_eq!(result.state, ExecutionState::Completed);
}

#[tokio::test]
async fn test_condition_node_false() {
    let exec = ConditionNodeExecutor;
    let mut config = HashMap::new();
    config.insert("condition".to_string(), serde_json::json!("status == ok"));
    let node = make_node("n1", "condition", config);

    let mut ctx = HashMap::new();
    ctx.insert("status".to_string(), serde_json::json!("error"));

    let result = exec.execute(&node, &ctx, &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    assert!(!result.output["condition_result"].as_bool().unwrap());
}

#[tokio::test]
async fn test_delay_node_default_seconds() {
    let exec = DelayNodeExecutor;
    let node = make_node("n1", "delay", HashMap::new());
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
}

#[tokio::test]
async fn test_human_review_default_message() {
    let exec = HumanReviewNodeExecutor;
    let node = make_node("n1", "human_review", HashMap::new());
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Waiting);
    assert_eq!(
        result.output["message"].as_str().unwrap(),
        "Human review required"
    );
}

#[tokio::test]
async fn test_script_node_missing_script() {
    let exec = ScriptNodeExecutor;
    let node = make_node("n1", "script", HashMap::new());
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Failed);
}

#[test]
fn test_resolve_template_simple() {
    let mut ctx = HashMap::new();
    ctx.insert("name".to_string(), serde_json::json!("World"));
    ctx.insert("count".to_string(), serde_json::json!(42));
    let result = resolve_template("Hello {{name}}, count={{count}}", &ctx);
    assert_eq!(result, "Hello World, count=42");
}

#[test]
fn test_resolve_template_no_vars() {
    let ctx = HashMap::new();
    let result = resolve_template("No variables here", &ctx);
    assert_eq!(result, "No variables here");
}

#[test]
fn test_resolve_template_missing_var() {
    let ctx = HashMap::new();
    let result = resolve_template("Hello {{name}}", &ctx);
    // Unresolved variables remain as-is
    assert_eq!(result, "Hello {{name}}");
}

#[test]
fn test_get_config_node_list_missing_id_and_type() {
    let mut config = HashMap::new();
    config.insert(
        "nodes".to_string(),
        serde_json::json!([
            { "config": {} }
        ]),
    );
    let nodes = get_config_node_list(&config, "nodes");
    assert_eq!(nodes.len(), 1);
    assert!(nodes[0].id.is_empty());
    assert!(nodes[0].node_type.is_empty());
}

#[tokio::test]
async fn test_parallel_stub_empty_children() {
    let exec = ParallelNodeStub;
    let config = HashMap::new();
    let node = make_node("n1", "parallel", config);
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    assert!(result.output["results"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_loop_stub_default_iterations() {
    let exec = LoopNodeStub;
    let mut config = HashMap::new();
    config.insert(
        "nodes".to_string(),
        serde_json::json!([{ "id": "inner", "node_type": "delay", "config": { "seconds": 0 } }]),
    );
    let node = make_node("n1", "loop", config);
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    // Default max_iterations should be 1
    assert!(result.output["iterations"].as_u64().unwrap() >= 1);
}

// ============================================================
// Additional coverage tests for nodes.rs
// ============================================================

#[tokio::test]
async fn test_tool_node_executor_default() {
    let exec = ToolNodeExecutor;
    let node = make_node("n1", "tool", HashMap::new());
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    assert_eq!(result.output["tool"].as_str().unwrap(), "unknown");
    assert_eq!(result.output["status"].as_str().unwrap(), "success");
}

#[tokio::test]
async fn test_tool_node_executor_with_name() {
    let exec = ToolNodeExecutor;
    let mut config = HashMap::new();
    config.insert("tool".to_string(), serde_json::json!("grep"));
    let node = make_node("n1", "tool", config);
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.output["tool"].as_str().unwrap(), "grep");
}

#[tokio::test]
async fn test_llm_node_executor_default_prompt() {
    let exec = LLMNodeExecutor;
    let node = make_node("n1", "llm", HashMap::new());
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    assert!(result.output["text"].as_str().unwrap().contains("default prompt"));
    assert!(result.output["text"].as_str().unwrap().contains("model=default"));
}

#[tokio::test]
async fn test_llm_node_executor_with_model() {
    let exec = LLMNodeExecutor;
    let mut config = HashMap::new();
    config.insert("prompt".to_string(), serde_json::json!("Summarize this"));
    config.insert("model".to_string(), serde_json::json!("gpt-4"));
    let node = make_node("n1", "llm", config);
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    let text = result.output["text"].as_str().unwrap();
    assert!(text.contains("gpt-4"));
    assert!(text.contains("Summarize this"));
}

#[tokio::test]
async fn test_condition_node_default_false() {
    let exec = ConditionNodeExecutor;
    let node = make_node("n1", "condition", HashMap::new());
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    assert!(!result.output["condition_result"].as_bool().unwrap());
}

#[tokio::test]
async fn test_condition_node_true_literal() {
    let exec = ConditionNodeExecutor;
    let mut config = HashMap::new();
    config.insert("condition".to_string(), serde_json::json!("true"));
    let node = make_node("n1", "condition", config);
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert!(result.output["condition_result"].as_bool().unwrap());
}

#[tokio::test]
async fn test_condition_node_inequality() {
    let exec = ConditionNodeExecutor;
    let mut config = HashMap::new();
    config.insert("condition".to_string(), serde_json::json!("status != ok"));
    let node = make_node("n1", "condition", config);

    let mut ctx = HashMap::new();
    ctx.insert("status".to_string(), serde_json::json!("error"));

    let result = exec.execute(&node, &ctx, &empty_wf_ctx()).await.unwrap();
    assert!(result.output["condition_result"].as_bool().unwrap());
}

#[tokio::test]
async fn test_condition_node_truthy_variable() {
    let exec = ConditionNodeExecutor;
    let mut config = HashMap::new();
    config.insert("condition".to_string(), serde_json::json!("flag"));
    let node = make_node("n1", "condition", config);

    let mut ctx = HashMap::new();
    ctx.insert("flag".to_string(), serde_json::json!("yes"));

    let result = exec.execute(&node, &ctx, &empty_wf_ctx()).await.unwrap();
    assert!(result.output["condition_result"].as_bool().unwrap());
}

#[tokio::test]
async fn test_transform_node_identity() {
    let exec = TransformNodeExecutor;
    let mut config = HashMap::new();
    config.insert("expression".to_string(), serde_json::json!("identity"));
    let node = make_node("n1", "transform", config);

    let mut ctx = HashMap::new();
    ctx.insert("key".to_string(), serde_json::json!("value"));

    let result = exec.execute(&node, &ctx, &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    let output_obj = result.output.as_object().unwrap();
    assert_eq!(output_obj.get("key").unwrap(), "value");
}

#[tokio::test]
async fn test_transform_node_passthrough() {
    let exec = TransformNodeExecutor;
    let mut config = HashMap::new();
    config.insert("expression".to_string(), serde_json::json!("passthrough"));
    let node = make_node("n1", "transform", config);
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
}

#[tokio::test]
async fn test_transform_node_custom_expression() {
    let exec = TransformNodeExecutor;
    let mut config = HashMap::new();
    config.insert("expression".to_string(), serde_json::json!("uppercase(data)"));
    let node = make_node("n1", "transform", config);

    let mut ctx = HashMap::new();
    ctx.insert("data".to_string(), serde_json::json!("hello"));

    let result = exec.execute(&node, &ctx, &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    assert_eq!(result.output["transformed"].as_str().unwrap(), "uppercase(data)");
    let keys = result.output["input_keys"].as_array().unwrap();
    assert!(keys.iter().any(|k| k.as_str() == Some("data")));
}

#[tokio::test]
async fn test_delay_node_with_seconds() {
    let exec = DelayNodeExecutor;
    let mut config = HashMap::new();
    config.insert("seconds".to_string(), serde_json::json!(0));
    let node = make_node("n1", "delay", config);
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    assert_eq!(result.output["delayed_ms"].as_u64().unwrap(), 0);
}

#[tokio::test]
async fn test_parallel_node_with_branches_key() {
    let registry = NodeExecutorRegistry::new_with_composite();
    let exec = registry.get("parallel").unwrap();

    // Use "branches" key instead of "nodes"
    let mut config = HashMap::new();
    config.insert(
        "branches".to_string(),
        serde_json::json!([
            { "id": "b1", "node_type": "delay", "config": { "seconds": 0 } },
        ]),
    );
    let node = make_node("n1", "parallel", config);
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    assert!(result.output.as_object().unwrap().contains_key("branch_0"));
}

#[tokio::test]
async fn test_parallel_node_empty_children() {
    let registry = NodeExecutorRegistry::new_with_composite();
    let exec = registry.get("parallel").unwrap();

    let node = make_node("n1", "parallel", HashMap::new());
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    assert!(result.output["results"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_parallel_node_with_unknown_child_type() {
    let registry = NodeExecutorRegistry::new_with_composite();
    let exec = registry.get("parallel").unwrap();

    let mut config = HashMap::new();
    config.insert(
        "nodes".to_string(),
        serde_json::json!([
            { "id": "bad", "node_type": "nonexistent_type", "config": {} },
        ]),
    );
    let node = make_node("n1", "parallel", config);
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    // Should fail because child type is unknown
    assert_eq!(result.state, ExecutionState::Failed);
    assert!(result.error.is_some());
}

#[tokio::test]
async fn test_loop_node_empty_children() {
    let registry = NodeExecutorRegistry::new_with_composite();
    let exec = registry.get("loop").unwrap();

    let node = make_node("n1", "loop", HashMap::new());
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    assert_eq!(result.output["iterations"].as_u64().unwrap(), 0);
    assert!(result.output["last_output"].is_null());
}

#[tokio::test]
async fn test_loop_node_with_condition_stops_early() {
    let registry = NodeExecutorRegistry::new_with_composite();
    let exec = registry.get("loop").unwrap();

    let mut config = HashMap::new();
    config.insert("max_iterations".to_string(), serde_json::json!(10));
    config.insert("condition".to_string(), serde_json::json!("false"));
    config.insert(
        "nodes".to_string(),
        serde_json::json!([
            { "id": "inner", "node_type": "delay", "config": { "seconds": 0 } }
        ]),
    );
    let node = make_node("n1", "loop", config);
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    // condition is "false", so after first iteration it should stop
    assert_eq!(result.output["iterations"].as_u64().unwrap(), 1);
}

#[tokio::test]
async fn test_loop_stub_empty_children() {
    let exec = LoopNodeStub;
    let node = make_node("n1", "loop", HashMap::new());
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    assert_eq!(result.output["iterations"].as_u64().unwrap(), 0);
}

#[tokio::test]
async fn test_loop_stub_with_condition_stops() {
    let exec = LoopNodeStub;
    let mut config = HashMap::new();
    config.insert("max_iterations".to_string(), serde_json::json!(10));
    config.insert("condition".to_string(), serde_json::json!("false"));
    config.insert(
        "nodes".to_string(),
        serde_json::json!([
            { "id": "inner", "node_type": "delay", "config": { "seconds": 0 } }
        ]),
    );
    let node = make_node("n1", "loop", config);
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    assert_eq!(result.output["iterations"].as_u64().unwrap(), 1);
}

#[tokio::test]
async fn test_loop_node_with_unknown_child_type() {
    let registry = NodeExecutorRegistry::new_with_composite();
    let exec = registry.get("loop").unwrap();

    let mut config = HashMap::new();
    config.insert("max_iterations".to_string(), serde_json::json!(2));
    config.insert(
        "nodes".to_string(),
        serde_json::json!([
            { "id": "bad", "node_type": "nonexistent_type", "config": {} }
        ]),
    );
    let node = make_node("n1", "loop", config);
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert!(result.error.is_some());
    assert!(result.error.unwrap().contains("unknown node type"));
}

#[tokio::test]
async fn test_sub_workflow_node_with_engine() {
    let engine = WorkflowEngine::new_arc();
    let exec = SubWorkflowNodeExecutor::new(engine);

    let mut config = HashMap::new();
    config.insert("workflow".to_string(), serde_json::json!("child_wf"));
    let node = make_node("n1", "sub_workflow", config);
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await;
    // child_wf is not registered, so this should fail
    assert!(result.is_err());
}

#[tokio::test]
async fn test_sub_workflow_node_with_engine_success() {
    let engine = WorkflowEngine::new_arc();
    // Register a child workflow
    engine.register_workflow(Workflow {
        name: "child_wf".to_string(),
        description: String::new(),
        version: "1.0.0".to_string(),
        triggers: vec![],
        nodes: vec![NodeDef {
            id: "cn1".to_string(),
            node_type: "llm".to_string(),
            config: HashMap::new(),
            depends_on: vec![],
            retry_count: 0,
            timeout: None,
        is_terminal: false,
        }],
        edges: vec![],
        variables: HashMap::new(),
        metadata: HashMap::new(),
    }).unwrap();

    let exec = SubWorkflowNodeExecutor::new(engine);
    let mut config = HashMap::new();
    config.insert("workflow".to_string(), serde_json::json!("child_wf"));
    let node = make_node("n1", "sub_workflow", config);
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    assert!(result.metadata.contains_key("execution_id"));
}

#[tokio::test]
async fn test_sub_workflow_node_missing_workflow_config() {
    let engine = WorkflowEngine::new_arc();
    let exec = SubWorkflowNodeExecutor::new(engine);
    let node = make_node("n1", "sub_workflow", HashMap::new());
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Failed);
    assert!(result.error.unwrap().contains("workflow"));
}

#[tokio::test]
async fn test_sub_workflow_stub_with_input_config() {
    let exec = SubWorkflowNodeStub;
    let mut config = HashMap::new();
    config.insert("workflow".to_string(), serde_json::json!("child_wf"));
    config.insert("input".to_string(), serde_json::json!({
        "query": "search_term",
        "limit": 10,
    }));
    let node = make_node("n1", "sub_workflow", config);

    let mut ctx = HashMap::new();
    ctx.insert("search_term".to_string(), serde_json::json!("resolved_value"));

    let result = exec.execute(&node, &ctx, &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Failed);
    // Check that input was resolved from context
    let input_obj = result.output["input"].as_object().unwrap();
    assert_eq!(input_obj.get("query").unwrap(), "resolved_value");
    assert_eq!(input_obj.get("limit").unwrap(), 10);
}

#[tokio::test]
async fn test_http_node_post_method() {
    let exec = HTTPNodeExecutor;
    let mut config = HashMap::new();
    config.insert("url".to_string(), serde_json::json!("http://127.0.0.1:1/nonexistent"));
    config.insert("method".to_string(), serde_json::json!("POST"));
    config.insert("body".to_string(), serde_json::json!("test_body"));
    let node = make_node("n1", "http", config);
    // Should fail to connect, not panic
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await;
    // Connection will fail - that's expected
    match result {
        Err(e) => assert!(e.contains("HTTP request failed")),
        Ok(r) => {
            // Might succeed on some systems, verify structure
            assert!(r.output.get("status_code").is_some());
        }
    }
}

#[tokio::test]
async fn test_http_node_with_headers() {
    let exec = HTTPNodeExecutor;
    let mut config = HashMap::new();
    config.insert("url".to_string(), serde_json::json!("http://127.0.0.1:1/test"));
    config.insert("method".to_string(), serde_json::json!("GET"));
    config.insert("headers".to_string(), serde_json::json!({
        "Content-Type": "application/json",
        "X-Custom": "value",
    }));
    let node = make_node("n1", "http", config);
    // Should attempt the request with headers
    let _ = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await;
}

#[tokio::test]
async fn test_http_node_various_methods() {
    let exec = HTTPNodeExecutor;

    for method in &["PUT", "PATCH", "DELETE", "HEAD"] {
        let mut config = HashMap::new();
        config.insert("url".to_string(), serde_json::json!("http://127.0.0.1:1/test"));
        config.insert("method".to_string(), serde_json::json!(*method));
        let node = make_node("n1", "http", config);
        // Should not panic for any method
        let _ = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await;
    }
}

#[tokio::test]
async fn test_script_node_with_context_variables() {
    let exec = ScriptNodeExecutor;
    let mut config = HashMap::new();
    config.insert("script".to_string(), serde_json::json!("echo {{name}}"));
    config.insert("language".to_string(), serde_json::json!("bash"));
    let node = make_node("n1", "script", config);

    let mut ctx = HashMap::new();
    ctx.insert("name".to_string(), serde_json::json!("World"));

    let result = exec.execute(&node, &ctx, &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    assert!(result.output["stdout"].as_str().unwrap().contains("World"));
}

#[tokio::test]
async fn test_script_node_failing_script() {
    let exec = ScriptNodeExecutor;
    let mut config = HashMap::new();
    config.insert("script".to_string(), serde_json::json!("exit 1"));
    config.insert("language".to_string(), serde_json::json!("bash"));
    let node = make_node("n1", "script", config);
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Failed);
    assert_eq!(result.output["exit_code"].as_i64().unwrap(), 1);
}

#[tokio::test]
async fn test_script_node_sh_language() {
    let exec = ScriptNodeExecutor;
    let mut config = HashMap::new();
    config.insert("script".to_string(), serde_json::json!("echo sh_test"));
    config.insert("language".to_string(), serde_json::json!("sh"));
    let node = make_node("n1", "script", config);
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    assert!(result.output["stdout"].as_str().unwrap().contains("sh_test"));
}

#[tokio::test]
async fn test_human_review_with_message() {
    let exec = HumanReviewNodeExecutor;
    let mut config = HashMap::new();
    config.insert("message".to_string(), serde_json::json!("Please approve deployment"));
    let node = make_node("n1", "human_review", config);
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Waiting);
    assert_eq!(
        result.output["message"].as_str().unwrap(),
        "Please approve deployment"
    );
    assert_eq!(
        result.output["status"].as_str().unwrap(),
        "waiting_for_review"
    );
}

#[tokio::test]
async fn test_inline_node_execution_for_unknown_type() {
    let node = NodeDef {
        id: "test".to_string(),
        node_type: "custom_unknown".to_string(),
        config: HashMap::new(),
        depends_on: vec![],
        retry_count: 0,
        timeout: None,
    is_terminal: false,
    };
    let result = execute_inline_node(&node, &HashMap::new()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    assert_eq!(result.output["status"].as_str().unwrap(), "skipped");
    assert!(result.output["reason"].as_str().unwrap().contains("inline execution not supported"));
}

#[tokio::test]
async fn test_inline_node_execution_transform() {
    let node = NodeDef {
        id: "test".to_string(),
        node_type: "transform".to_string(),
        config: HashMap::new(),
        depends_on: vec![],
        retry_count: 0,
        timeout: None,
    is_terminal: false,
    };
    let result = execute_inline_node(&node, &HashMap::new()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
}

#[tokio::test]
async fn test_inline_node_execution_condition() {
    let mut config = HashMap::new();
    config.insert("condition".to_string(), serde_json::json!("true"));
    let node = NodeDef {
        id: "test".to_string(),
        node_type: "condition".to_string(),
        config,
        depends_on: vec![],
        retry_count: 0,
        timeout: None,
    is_terminal: false,
    };
    let result = execute_inline_node(&node, &HashMap::new()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
}

#[test]
fn test_evaluate_condition_truthy_values() {
    let mut ctx = HashMap::new();
    ctx.insert("bool_true".to_string(), serde_json::json!(true));
    ctx.insert("number_nonzero".to_string(), serde_json::json!(42));
    ctx.insert("string_nonempty".to_string(), serde_json::json!("hello"));
    ctx.insert("array_nonempty".to_string(), serde_json::json!([1, 2, 3]));
    ctx.insert("object_nonempty".to_string(), serde_json::json!({"key": "val"}));
    ctx.insert("null_val".to_string(), serde_json::Value::Null);
    ctx.insert("bool_false".to_string(), serde_json::json!(false));
    ctx.insert("number_zero".to_string(), serde_json::json!(0));
    ctx.insert("string_empty".to_string(), serde_json::json!(""));
    ctx.insert("array_empty".to_string(), serde_json::json!([]));
    ctx.insert("object_empty".to_string(), serde_json::json!({}));

    assert!(evaluate_condition("bool_true", &ctx));
    assert!(evaluate_condition("number_nonzero", &ctx));
    assert!(evaluate_condition("string_nonempty", &ctx));
    assert!(evaluate_condition("array_nonempty", &ctx));
    assert!(evaluate_condition("object_nonempty", &ctx));
    assert!(!evaluate_condition("null_val", &ctx));
    assert!(!evaluate_condition("bool_false", &ctx));
    assert!(!evaluate_condition("number_zero", &ctx));
    assert!(!evaluate_condition("string_empty", &ctx));
    assert!(!evaluate_condition("array_empty", &ctx));
    assert!(!evaluate_condition("object_empty", &ctx));
}

#[test]
fn test_evaluate_condition_equality_different_value() {
    let mut ctx = HashMap::new();
    ctx.insert("count".to_string(), serde_json::json!(5));
    let result = evaluate_condition("count == 5", &ctx);
    // Note: ctx value is Number(5) but comparison creates String("5")
    // so they won't be equal - this tests the == path returning false
    assert!(!result);
}

#[test]
fn test_evaluate_condition_inequality_missing_key() {
    let ctx = HashMap::new();
    // When left side is not in context, != returns true
    let result = evaluate_condition("missing != something", &ctx);
    assert!(result);
}

#[test]
fn test_evaluate_condition_literal_true() {
    let ctx = HashMap::new();
    assert!(evaluate_condition("true", &ctx));
    assert!(evaluate_condition("TRUE", &ctx));
    assert!(evaluate_condition("True", &ctx));
}

#[test]
fn test_evaluate_condition_literal_false() {
    let ctx = HashMap::new();
    assert!(!evaluate_condition("false", &ctx));
    assert!(!evaluate_condition("FALSE", &ctx));
    assert!(!evaluate_condition("False", &ctx));
}

#[test]
fn test_get_config_node_list_with_type_fallback() {
    let mut config = HashMap::new();
    config.insert(
        "nodes".to_string(),
        serde_json::json!([
            { "id": "n1", "type": "llm", "config": {} }
        ]),
    );
    let nodes = get_config_node_list(&config, "nodes");
    assert_eq!(nodes.len(), 1);
    // Should fall back to "type" if "node_type" is not present
    assert_eq!(nodes[0].node_type, "llm");
}

#[test]
fn test_get_config_node_list_with_retry_and_timeout() {
    let mut config = HashMap::new();
    config.insert(
        "nodes".to_string(),
        serde_json::json!([
            { "id": "n1", "node_type": "llm", "config": {}, "retry_count": 3, "timeout": "30s" }
        ]),
    );
    let nodes = get_config_node_list(&config, "nodes");
    assert_eq!(nodes[0].retry_count, 3);
    assert_eq!(nodes[0].timeout, Some("30s".to_string()));
}

#[test]
fn test_get_config_node_list_non_array() {
    let mut config = HashMap::new();
    config.insert("nodes".to_string(), serde_json::json!("not an array"));
    let nodes = get_config_node_list(&config, "nodes");
    assert!(nodes.is_empty());
}

#[test]
fn test_get_config_node_list_non_object_items() {
    let mut config = HashMap::new();
    config.insert(
        "nodes".to_string(),
        serde_json::json!(["string_item", 123, true]),
    );
    let nodes = get_config_node_list(&config, "nodes");
    // Non-object items should be skipped
    assert!(nodes.is_empty());
}

#[tokio::test]
async fn test_parallel_stub_with_named_children() {
    let exec = ParallelNodeStub;
    let mut config = HashMap::new();
    config.insert(
        "nodes".to_string(),
        serde_json::json!([
            { "id": "alpha", "node_type": "delay", "config": { "seconds": 0 } },
            { "id": "", "node_type": "delay", "config": { "seconds": 0 } },
        ]),
    );
    let node = make_node("n1", "parallel", config);
    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    let obj = result.output.as_object().unwrap();
    assert!(obj.contains_key("alpha"));
    // Empty-id child should use branch_0 style key
    assert!(obj.contains_key("branch_1"));
}

#[tokio::test]
async fn test_new_with_engine_registry() {
    let engine = WorkflowEngine::new_arc();
    let registry = NodeExecutorRegistry::new_with_engine(engine);
    assert!(registry.get("sub_workflow").is_some());
    assert!(registry.get("parallel").is_some());
    assert!(registry.get("loop").is_some());
    assert!(registry.get("llm").is_some());
}

// ---------------------------------------------------------------------------
// RealLLMNodeExecutor (1a-D1)
// ---------------------------------------------------------------------------

use async_trait::async_trait;
use nemesis_providers::failover::FailoverError;
use nemesis_providers::router::LLMProvider;
use nemesis_providers::types::{ChatOptions, LLMResponse, Message, ToolDefinition};

/// Provider stub that returns a fixed response and records the request.
struct StubProvider {
    name: String,
    default_model: String,
    response: String,
    fail_with: Option<String>,
    last_model: std::sync::Mutex<Option<String>>,
    last_options: std::sync::Mutex<Option<ChatOptions>>,
    last_messages: std::sync::Mutex<Vec<Message>>,
}

impl StubProvider {
    fn success(name: &str, model: &str, response: &str) -> Self {
        Self {
            name: name.to_string(),
            default_model: model.to_string(),
            response: response.to_string(),
            fail_with: None,
            last_model: std::sync::Mutex::new(None),
            last_options: std::sync::Mutex::new(None),
            last_messages: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn failing(name: &str, model: &str, err: &str) -> Self {
        Self {
            name: name.to_string(),
            default_model: model.to_string(),
            response: String::new(),
            fail_with: Some(err.to_string()),
            last_model: std::sync::Mutex::new(None),
            last_options: std::sync::Mutex::new(None),
            last_messages: std::sync::Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl LLMProvider for StubProvider {
    async fn chat(
        &self,
        messages: &[Message],
        _tools: &[ToolDefinition],
        model: &str,
        options: &ChatOptions,
    ) -> Result<LLMResponse, FailoverError> {
        *self.last_model.lock().unwrap() = Some(model.to_string());
        *self.last_options.lock().unwrap() = Some(options.clone());
        *self.last_messages.lock().unwrap() = messages.to_vec();

        if let Some(err) = &self.fail_with {
            return Err(FailoverError::Unknown {
                provider: self.name.clone(),
                message: err.clone(),
            });
        }

        Ok(LLMResponse {
            content: self.response.clone(),
            tool_calls: Vec::new(),
            finish_reason: "stop".to_string(),
            usage: Some(nemesis_providers::types::UsageInfo {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
                ..Default::default()
            }),
            reasoning_content: None,
            extra: HashMap::new(),
            raw_request_body: None,
            raw_response_body: None,
        })
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    fn name(&self) -> &str {
        &self.name
    }
}

fn llm_config_prompt_only(prompt: &str) -> HashMap<String, serde_json::Value> {
    let mut c = HashMap::new();
    c.insert("prompt".to_string(), serde_json::json!(prompt));
    c
}

#[tokio::test]
async fn test_real_llm_executor_success_path() {
    let provider = Arc::new(StubProvider::success("stub", "stub-model", "hello world"));
    let exec = RealLLMNodeExecutor::new(Arc::clone(&provider) as Arc<dyn LLMProvider>);
    let node = make_node("n1", "llm", llm_config_prompt_only("Hi"));
    let result = exec
        .execute(&node, &HashMap::new(), &empty_wf_ctx())
        .await
        .unwrap();

    assert_eq!(result.state, ExecutionState::Completed);
    assert!(result.error.is_none());

    let out = result.output.as_object().unwrap();
    assert_eq!(out.get("text").unwrap(), "hello world");
    assert_eq!(out.get("model").unwrap(), "stub-model");
    assert_eq!(out.get("finish_reason").unwrap(), "stop");
    let usage = out.get("usage").unwrap().as_object().unwrap();
    assert_eq!(usage.get("total_tokens").unwrap(), 15);
}

#[tokio::test]
async fn test_real_llm_executor_passes_temperature_and_max_tokens() {
    let provider = Arc::new(StubProvider::success("stub", "stub-model", "ok"));
    let exec = RealLLMNodeExecutor::new(Arc::clone(&provider) as Arc<dyn LLMProvider>);

    let mut config = llm_config_prompt_only("Hi");
    config.insert("temperature".to_string(), serde_json::json!(0.7));
    config.insert("max_tokens".to_string(), serde_json::json!(256u64));
    let node = make_node("n1", "llm", config);

    let _ = exec
        .execute(&node, &HashMap::new(), &empty_wf_ctx())
        .await
        .unwrap();

    let captured = provider
        .last_options
        .lock()
        .unwrap()
        .clone()
        .expect("provider should have captured ChatOptions");
    assert_eq!(captured.temperature, Some(0.7));
    assert_eq!(captured.max_tokens, Some(256));
}

#[tokio::test]
async fn test_real_llm_executor_honors_explicit_model() {
    let provider = Arc::new(StubProvider::success("stub", "default-model", "ok"));
    let exec = RealLLMNodeExecutor::new(Arc::clone(&provider) as Arc<dyn LLMProvider>);

    let mut config = llm_config_prompt_only("Hi");
    config.insert("model".to_string(), serde_json::json!("custom-7b"));
    let node = make_node("n1", "llm", config);

    let _ = exec
        .execute(&node, &HashMap::new(), &empty_wf_ctx())
        .await
        .unwrap();

    let captured = provider.last_model.lock().unwrap().clone();
    assert_eq!(captured.as_deref(), Some("custom-7b"));
}

#[tokio::test]
async fn test_real_llm_executor_falls_back_to_default_model() {
    let provider = Arc::new(StubProvider::success("stub", "stub-default", "ok"));
    let exec = RealLLMNodeExecutor::new(Arc::clone(&provider) as Arc<dyn LLMProvider>);

    let node = make_node("n1", "llm", llm_config_prompt_only("Hi"));
    let _ = exec
        .execute(&node, &HashMap::new(), &empty_wf_ctx())
        .await
        .unwrap();

    let captured = provider
        .last_model
        .lock()
        .unwrap()
        .clone()
        .unwrap();
    assert_eq!(captured, "stub-default");
}

#[tokio::test]
async fn test_real_llm_executor_provider_error_returns_failed_node_result() {
    // Provider errors are surfaced as a Failed NodeResult (not Err), so the
    // workflow can capture and branch on the failure state.
    let provider = Arc::new(StubProvider::failing("stub", "stub-model", "timeout"));
    let exec = RealLLMNodeExecutor::new(Arc::clone(&provider) as Arc<dyn LLMProvider>);
    let node = make_node("n1", "llm", llm_config_prompt_only("Hi"));
    let result = exec
        .execute(&node, &HashMap::new(), &empty_wf_ctx())
        .await
        .unwrap();

    assert_eq!(result.state, ExecutionState::Failed);
    let err = result.error.expect("Failed state should carry an error");
    assert!(err.contains("LLM provider error"));
    assert!(err.contains("timeout"));
}

#[tokio::test]
async fn test_real_llm_executor_missing_prompt_fails() {
    let provider = Arc::new(StubProvider::success("stub", "stub-model", "ok"));
    let exec = RealLLMNodeExecutor::new(provider as Arc<dyn LLMProvider>);
    let node = make_node("n1", "llm", HashMap::new());
    let result = exec
        .execute(&node, &HashMap::new(), &empty_wf_ctx())
        .await
        .unwrap();

    assert_eq!(result.state, ExecutionState::Failed);
    let err = result.error.unwrap();
    assert!(err.contains("missing required 'prompt'"));
}

#[tokio::test]
async fn test_real_llm_executor_resolves_prompt_template() {
    let provider = Arc::new(StubProvider::success("stub", "stub-model", "ok"));
    let exec = RealLLMNodeExecutor::new(Arc::clone(&provider) as Arc<dyn LLMProvider>);

    let node = make_node("n1", "llm", llm_config_prompt_only("Hello {{name}}!"));
    let mut ctx = HashMap::new();
    ctx.insert("name".to_string(), serde_json::json!("Alice"));

    let _ = exec.execute(&node, &ctx, &empty_wf_ctx()).await.unwrap();

    let captured = provider.last_messages.lock().unwrap().clone();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].content, "Hello Alice!");
}

#[tokio::test]
async fn test_real_llm_executor_prepends_system_prompt() {
    let provider = Arc::new(StubProvider::success("stub", "stub-model", "ok"));
    let exec = RealLLMNodeExecutor::new(Arc::clone(&provider) as Arc<dyn LLMProvider>);

    let mut config = llm_config_prompt_only("Hi");
    config.insert(
        "system_prompt".to_string(),
        serde_json::json!("You are a robot."),
    );
    let node = make_node("n1", "llm", config);

    let _ = exec
        .execute(&node, &HashMap::new(), &empty_wf_ctx())
        .await
        .unwrap();

    let captured = provider.last_messages.lock().unwrap().clone();
    assert_eq!(captured.len(), 2);
    assert_eq!(captured[0].role, "system");
    assert_eq!(captured[0].content, "You are a robot.");
    assert_eq!(captured[1].role, "user");
}

#[tokio::test]
async fn test_real_llm_executor_can_be_registered_as_llm_node() {
    // Verifies the real executor can override the default mock via
    // NodeExecutorRegistry::register("llm", ...).
    let provider = Arc::new(StubProvider::success("stub", "stub-model", "real-response"));
    let registry = NodeExecutorRegistry::new();
    registry.register(
        "llm",
        Arc::new(RealLLMNodeExecutor::new(provider as Arc<dyn LLMProvider>)),
    );

    let executor = registry.get("llm").expect("llm executor should be registered");
    let node = make_node("n1", "llm", llm_config_prompt_only("Hi"));
    let result = executor
        .execute(&node, &HashMap::new(), &empty_wf_ctx())
        .await
        .unwrap();
    let out = result.output.as_object().unwrap();
    assert_eq!(out.get("text").unwrap(), "real-response");
}

// ===========================================================================
// RealToolNodeExecutor tests
// ===========================================================================

use nemesis_tools::registry::{Tool, ToolRegistry};
use nemesis_tools::types::ToolResult;

/// Minimal in-memory Tool implementation for tests.
///
/// Captures the last args it was called with and returns either a
/// pre-canned success result or an error result.
struct StubTool {
    name: String,
    canned_output: String,
    fail_with: Option<String>,
    last_args: std::sync::Mutex<Option<serde_json::Value>>,
}

impl StubTool {
    fn success(name: &str, output: &str) -> Self {
        Self {
            name: name.to_string(),
            canned_output: output.to_string(),
            fail_with: None,
            last_args: std::sync::Mutex::new(None),
        }
    }

    fn failing(name: &str, err: &str) -> Self {
        Self {
            name: name.to_string(),
            canned_output: String::new(),
            fail_with: Some(err.to_string()),
            last_args: std::sync::Mutex::new(None),
        }
    }

    fn captured_args(&self) -> Option<serde_json::Value> {
        self.last_args.lock().unwrap().clone()
    }
}

#[async_trait]
impl Tool for StubTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        "stub tool for workflow tests"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object"})
    }

    async fn execute(&self, args: &serde_json::Value) -> ToolResult {
        *self.last_args.lock().unwrap() = Some(args.clone());
        if let Some(err) = &self.fail_with {
            return ToolResult::error(err);
        }
        ToolResult::success(&self.canned_output)
    }
}

fn tool_config_name_only(name: &str) -> HashMap<String, serde_json::Value> {
    let mut c = HashMap::new();
    c.insert("name".to_string(), serde_json::json!(name));
    c
}

fn tool_config_with_args(name: &str, args: serde_json::Value) -> HashMap<String, serde_json::Value> {
    let mut c = HashMap::new();
    c.insert("name".to_string(), serde_json::json!(name));
    c.insert("args".to_string(), args);
    c
}

#[tokio::test]
async fn test_real_tool_executor_calls_registered_tool() {
    let registry = Arc::new(ToolRegistry::new());
    let stub = Arc::new(StubTool::success("weather", "sunny, 22C"));
    registry.register(stub as Arc<dyn Tool>);

    let exec = RealToolNodeExecutor::new(Arc::clone(&registry));
    let node = make_node("n1", "tool", tool_config_name_only("weather"));
    let result = exec
        .execute(&node, &HashMap::new(), &empty_wf_ctx())
        .await
        .unwrap();

    assert_eq!(result.state, ExecutionState::Completed);
    assert!(result.error.is_none());
    let out = result.output.as_object().unwrap();
    assert_eq!(out.get("tool").unwrap(), "weather");
    assert_eq!(out.get("result").unwrap(), "sunny, 22C");
}

#[tokio::test]
async fn test_real_tool_executor_passes_args_through() {
    let registry = Arc::new(ToolRegistry::new());
    let stub = Arc::new(StubTool::success("echo", ""));
    registry.register(stub.clone() as Arc<dyn Tool>);

    let exec = RealToolNodeExecutor::new(Arc::clone(&registry));
    let args = serde_json::json!({"city": "Tokyo", "units": "metric"});
    let node = make_node("n1", "tool", tool_config_with_args("echo", args));
    let _ = exec
        .execute(&node, &HashMap::new(), &empty_wf_ctx())
        .await
        .unwrap();

    let captured = stub.captured_args().expect("tool should have been invoked");
    assert_eq!(captured["city"], "Tokyo");
    assert_eq!(captured["units"], "metric");
}

#[tokio::test]
async fn test_real_tool_executor_resolves_template_in_args() {
    let registry = Arc::new(ToolRegistry::new());
    let stub = Arc::new(StubTool::success("echo", ""));
    registry.register(stub.clone() as Arc<dyn Tool>);

    let exec = RealToolNodeExecutor::new(Arc::clone(&registry));

    let mut ctx = HashMap::new();
    ctx.insert("city".to_string(), serde_json::json!("Paris"));

    // String field with {{city}} placeholder + nested object.
    let args = serde_json::json!({
        "query": "weather in {{city}}",
        "nested": {"location": "{{city}}", "count": 3},
        "plain": 42,
    });
    let node = make_node("n1", "tool", tool_config_with_args("echo", args));
    let _ = exec.execute(&node, &ctx, &empty_wf_ctx()).await.unwrap();

    let captured = stub.captured_args().expect("tool should have been invoked");
    assert_eq!(captured["query"], "weather in Paris");
    assert_eq!(captured["nested"]["location"], "Paris");
    assert_eq!(captured["nested"]["count"], 3);
    assert_eq!(captured["plain"], 42);
}

#[tokio::test]
async fn test_real_tool_executor_surfaces_tool_error_as_failed_state() {
    let registry = Arc::new(ToolRegistry::new());
    let stub = Arc::new(StubTool::failing("db", "connection refused"));
    registry.register(stub as Arc<dyn Tool>);

    let exec = RealToolNodeExecutor::new(Arc::clone(&registry));
    let node = make_node("n1", "tool", tool_config_name_only("db"));
    let result = exec
        .execute(&node, &HashMap::new(), &empty_wf_ctx())
        .await
        .unwrap();

    // Tool errors become a Failed NodeResult (not Err) so the workflow can
    // branch on failure.
    assert_eq!(result.state, ExecutionState::Failed);
    let err = result.error.expect("Failed state should carry an error");
    assert!(err.contains("tool 'db' error"));
    assert!(err.contains("connection refused"));
}

#[tokio::test]
async fn test_real_tool_executor_missing_name_fails() {
    let registry = Arc::new(ToolRegistry::new());
    let exec = RealToolNodeExecutor::new(Arc::clone(&registry));
    let node = make_node("n1", "tool", HashMap::new());
    let result = exec
        .execute(&node, &HashMap::new(), &empty_wf_ctx())
        .await
        .unwrap();

    assert_eq!(result.state, ExecutionState::Failed);
    let err = result.error.unwrap();
    assert!(err.contains("missing required 'name'"));
}

#[tokio::test]
async fn test_real_tool_executor_unknown_tool_returns_failed() {
    let registry = Arc::new(ToolRegistry::new());
    // Registry is empty - tool lookup will fail inside ToolRegistry::execute.
    let exec = RealToolNodeExecutor::new(Arc::clone(&registry));
    let node = make_node("n1", "tool", tool_config_name_only("does_not_exist"));
    let result = exec
        .execute(&node, &HashMap::new(), &empty_wf_ctx())
        .await
        .unwrap();

    assert_eq!(result.state, ExecutionState::Failed);
    let err = result.error.unwrap();
    // ToolRegistry::error result: "tool \"does_not_exist\" not found"
    assert!(err.contains("does_not_exist"));
}

#[tokio::test]
async fn test_real_tool_executor_accepts_legacy_tool_key() {
    // Existing workflows use config["tool"] instead of config["name"].
    // Both should work.
    let registry = Arc::new(ToolRegistry::new());
    let stub = Arc::new(StubTool::success("legacy_tool", "ok"));
    registry.register(stub as Arc<dyn Tool>);

    let exec = RealToolNodeExecutor::new(Arc::clone(&registry));
    let mut config = HashMap::new();
    config.insert("tool".to_string(), serde_json::json!("legacy_tool"));
    let node = make_node("n1", "tool", config);
    let result = exec
        .execute(&node, &HashMap::new(), &empty_wf_ctx())
        .await
        .unwrap();

    assert_eq!(result.state, ExecutionState::Completed);
    let out = result.output.as_object().unwrap();
    assert_eq!(out.get("tool").unwrap(), "legacy_tool");
}

#[tokio::test]
async fn test_real_tool_executor_can_be_registered_as_tool_node() {
    // Verifies the real executor can override the default mock via
    // NodeExecutorRegistry::register("tool", ...).
    let registry = Arc::new(ToolRegistry::new());
    let stub = Arc::new(StubTool::success("ping", "pong"));
    registry.register(stub as Arc<dyn Tool>);

    let node_registry = NodeExecutorRegistry::new();
    node_registry.register("tool", Arc::new(RealToolNodeExecutor::new(Arc::clone(&registry))));

    let executor = node_registry
        .get("tool")
        .expect("tool executor should be registered");
    let node = make_node("n1", "tool", tool_config_name_only("ping"));
    let result = executor
        .execute(&node, &HashMap::new(), &empty_wf_ctx())
        .await
        .unwrap();
    let out = result.output.as_object().unwrap();
    assert_eq!(out.get("result").unwrap(), "pong");
}

#[tokio::test]
async fn test_real_tool_executor_defaults_args_to_null() {
    // If config has no "args" key, the tool still gets called (with null).
    let registry = Arc::new(ToolRegistry::new());
    let stub = Arc::new(StubTool::success("noop", "done"));
    registry.register(stub.clone() as Arc<dyn Tool>);

    let exec = RealToolNodeExecutor::new(Arc::clone(&registry));
    let node = make_node("n1", "tool", tool_config_name_only("noop"));
    let _ = exec
        .execute(&node, &HashMap::new(), &empty_wf_ctx())
        .await
        .unwrap();

    let captured = stub.captured_args().expect("tool should have been invoked");
    assert!(captured.is_null());
}

// ---------------------------------------------------------------------------
// QuestionClassifierNodeExecutor tests (1b-D3)
// ---------------------------------------------------------------------------

fn classifier_config(question: &str, classes: &[(&str, &str)]) -> HashMap<String, serde_json::Value> {
    let classes_json: Vec<serde_json::Value> = classes
        .iter()
        .map(|(id, desc)| {
            serde_json::json!({"id": id, "description": desc})
        })
        .collect();
    HashMap::from([
        ("question".to_string(), serde_json::json!(question)),
        ("classes".to_string(), serde_json::Value::Array(classes_json)),
    ])
}

#[tokio::test]
async fn test_question_classifier_success_first_attempt() {
    let provider = Arc::new(StubProvider::success("stub", "stub-model", "billing"));
    let exec = QuestionClassifierNodeExecutor::new(Arc::clone(&provider) as Arc<dyn LLMProvider>);
    let node = make_node(
        "classify",
        "question_classifier",
        classifier_config("I want a refund", &[
            ("billing", "questions about invoices, refunds, payments"),
            ("support", "technical issues"),
            ("sales", "purchase inquiries"),
        ]),
    );

    let result = exec
        .execute(&node, &HashMap::new(), &empty_wf_ctx())
        .await
        .unwrap();

    assert_eq!(result.state, ExecutionState::Completed);
    let out = result.output.as_object().unwrap();
    assert_eq!(out.get("class_id").unwrap(), "billing");
    assert_eq!(out.get("attempts").unwrap(), 1);
    // First-attempt confidence is 1.0.
    let confidence = out.get("confidence").unwrap().as_f64().unwrap();
    assert!(confidence > 0.99, "expected confidence 1.0, got {}", confidence);
}

#[tokio::test]
async fn test_question_classifier_strips_wrapper_punctuation() {
    // LLM often wraps the id in quotes / periods. The parser should clean it.
    let provider = Arc::new(StubProvider::success("stub", "stub-model", "\"sales.\""));
    let exec = QuestionClassifierNodeExecutor::new(Arc::clone(&provider) as Arc<dyn LLMProvider>);
    let node = make_node(
        "classify",
        "question_classifier",
        classifier_config("How much is the pro plan?", &[
            ("billing", "..."),
            ("sales", "..."),
        ]),
    );

    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    assert_eq!(result.output["class_id"], "sales");
}

#[tokio::test]
async fn test_question_classifier_takes_first_token_when_prose_follows() {
    // Defensive parse: "billing\n(because invoice)" → "billing"
    let provider = Arc::new(StubProvider::success("stub", "stub-model",
        "billing\n(because the user mentioned invoices)"));
    let exec = QuestionClassifierNodeExecutor::new(Arc::clone(&provider) as Arc<dyn LLMProvider>);
    let node = make_node(
        "classify",
        "question_classifier",
        classifier_config("refund please", &[
            ("billing", "..."),
            ("support", "..."),
        ]),
    );

    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    assert_eq!(result.output["class_id"], "billing");
}

#[tokio::test]
async fn test_question_classifier_fails_on_unknown_class() {
    // Stub returns "hr" but only ["billing","support"] are valid.
    let provider = Arc::new(StubProvider::success("stub", "stub-model", "hr"));
    let exec = QuestionClassifierNodeExecutor::new(Arc::clone(&provider) as Arc<dyn LLMProvider>);
    let mut config = classifier_config("help", &[
        ("billing", "..."),
        ("support", "..."),
    ]);
    config.insert("max_attempts".to_string(), serde_json::json!(2));
    let node = make_node("classify", "question_classifier", config);

    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Failed);
    assert!(result.error.as_ref().unwrap().contains("failed after 2 attempts"));
}

#[tokio::test]
async fn test_question_classifier_missing_question_fails() {
    let provider = Arc::new(StubProvider::success("stub", "stub-model", "billing"));
    let exec = QuestionClassifierNodeExecutor::new(Arc::clone(&provider) as Arc<dyn LLMProvider>);
    let mut config = HashMap::new();
    config.insert(
        "classes".to_string(),
        serde_json::json!([{"id": "billing", "description": "..."}]),
    );
    let node = make_node("classify", "question_classifier", config);

    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Failed);
    assert!(result.error.as_ref().unwrap().contains("missing required 'question'"));
}

#[tokio::test]
async fn test_question_classifier_missing_classes_fails() {
    let provider = Arc::new(StubProvider::success("stub", "stub-model", "billing"));
    let exec = QuestionClassifierNodeExecutor::new(Arc::clone(&provider) as Arc<dyn LLMProvider>);
    let mut config = HashMap::new();
    config.insert("question".to_string(), serde_json::json!("hello"));
    let node = make_node("classify", "question_classifier", config);

    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Failed);
    assert!(result.error.as_ref().unwrap().contains("missing required 'classes'"));
}

#[tokio::test]
async fn test_question_classifier_empty_classes_fails() {
    let provider = Arc::new(StubProvider::success("stub", "stub-model", "billing"));
    let exec = QuestionClassifierNodeExecutor::new(Arc::clone(&provider) as Arc<dyn LLMProvider>);
    let node = make_node(
        "classify",
        "question_classifier",
        classifier_config("hello", &[]),
    );

    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Failed);
    assert!(result.error.as_ref().unwrap().contains("empty 'classes'"));
}

#[tokio::test]
async fn test_question_classifier_resolves_template_in_question() {
    let provider = Arc::new(StubProvider::success("stub", "stub-model", "support"));
    let exec = QuestionClassifierNodeExecutor::new(Arc::clone(&provider) as Arc<dyn LLMProvider>);
    let node = make_node(
        "classify",
        "question_classifier",
        classifier_config("{{user_msg}}", &[("support", "...")]),
    );
    let ctx = HashMap::from([(
        "user_msg".to_string(),
        serde_json::json!("the app crashed"),
    )]);

    let result = exec.execute(&node, &ctx, &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    assert_eq!(result.output["class_id"], "support");
}

#[tokio::test]
async fn test_question_classifier_provider_error_retries_then_fails() {
    let provider = Arc::new(StubProvider::failing("stub", "stub-model", "rate limited"));
    let exec = QuestionClassifierNodeExecutor::new(Arc::clone(&provider) as Arc<dyn LLMProvider>);
    let mut config = classifier_config("hello", &[("billing", "...")]);
    config.insert("max_attempts".to_string(), serde_json::json!(2));
    let node = make_node("classify", "question_classifier", config);

    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Failed);
    let err = result.error.as_ref().unwrap();
    assert!(err.contains("failed after 2 attempts"));
    assert!(err.contains("rate limited"));
}

#[tokio::test]
async fn test_question_classifier_uses_default_temperature_zero() {
    let provider = Arc::new(StubProvider::success("stub", "stub-model", "billing"));
    let exec = QuestionClassifierNodeExecutor::new(Arc::clone(&provider) as Arc<dyn LLMProvider>);
    let node = make_node(
        "classify",
        "question_classifier",
        classifier_config("hi", &[("billing", "...")]),
    );

    let _ = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    let last_options = provider.last_options.lock().unwrap().clone().unwrap();
    assert_eq!(last_options.temperature, Some(0.0));
}

#[test]
fn parse_classifier_output_handles_bare_id() {
    assert_eq!(parse_classifier_output("billing"), Some("billing".to_string()));
}

#[test]
fn parse_classifier_output_trims_punctuation() {
    assert_eq!(parse_classifier_output("  \"billing.\"  "), Some("billing".to_string()));
    assert_eq!(parse_classifier_output("'sales',"), Some("sales".to_string()));
}

#[test]
fn parse_classifier_output_takes_first_token_from_prose() {
    assert_eq!(
        parse_classifier_output("billing because invoice"),
        Some("billing".to_string())
    );
}

#[test]
fn parse_classifier_output_returns_none_for_empty() {
    assert_eq!(parse_classifier_output("   "), None);
    assert_eq!(parse_classifier_output(""), None);
}

// ---------------------------------------------------------------------------
// ParameterExtractorNodeExecutor tests (1b-D4)
// ---------------------------------------------------------------------------

/// Build a parameter_extractor config from a text + a list of
/// `(name, type, description, required)` tuples.
fn extractor_config(
    text: &str,
    params: &[(&str, &str, &str, bool)],
) -> HashMap<String, serde_json::Value> {
    let params_json: Vec<serde_json::Value> = params
        .iter()
        .map(|(name, ty, desc, req)| {
            serde_json::json!({
                "name": name,
                "type": ty,
                "description": desc,
                "required": req,
            })
        })
        .collect();
    HashMap::from([
        ("text".to_string(), serde_json::json!(text)),
        ("parameters".to_string(), serde_json::Value::Array(params_json)),
    ])
}

#[tokio::test]
async fn test_extractor_returns_valid_json() {
    let provider = Arc::new(StubProvider::success(
        "stub",
        "stub-model",
        r#"{"name":"Alice","age":30}"#,
    ));
    let exec = ParameterExtractorNodeExecutor::new(Arc::clone(&provider) as Arc<dyn LLMProvider>);
    let node = make_node(
        "extract",
        "parameter_extractor",
        extractor_config(
            "Hi, I'm Alice and I'm 30 years old.",
            &[
                ("name", "string", "user's name", true),
                ("age", "number", "user's age", false),
            ],
        ),
    );

    let result = exec
        .execute(&node, &HashMap::new(), &empty_wf_ctx())
        .await
        .unwrap();

    assert_eq!(result.state, ExecutionState::Completed);
    let out = result.output.as_object().unwrap();
    let params = out.get("parameters").unwrap().as_object().unwrap();
    assert_eq!(params.get("name").unwrap(), "Alice");
    assert_eq!(params.get("age").unwrap(), 30);
    assert_eq!(out.get("attempts").unwrap(), 1);
}

#[tokio::test]
async fn test_extractor_handles_partial_data() {
    // Only name is extractable; the optional `email` field should be filled
    // with null (not omitted) so downstream nodes see a stable shape.
    let provider = Arc::new(StubProvider::success(
        "stub",
        "stub-model",
        r#"{"name":"Bob"}"#,
    ));
    let exec = ParameterExtractorNodeExecutor::new(Arc::clone(&provider) as Arc<dyn LLMProvider>);
    let node = make_node(
        "extract",
        "parameter_extractor",
        extractor_config(
            "I'm Bob.",
            &[
                ("name", "string", "name", true),
                ("email", "string", "email", false),
            ],
        ),
    );

    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    let params = result.output["parameters"].as_object().unwrap();
    assert_eq!(params.get("name").unwrap(), "Bob");
    assert!(params.get("email").unwrap().is_null());
}

#[tokio::test]
async fn test_extractor_fills_missing_optional_with_null() {
    // LLM returns only the required field; optional ones are added by normalizer.
    let provider = Arc::new(StubProvider::success(
        "stub",
        "stub-model",
        r#"{"city":"NYC"}"#,
    ));
    let exec = ParameterExtractorNodeExecutor::new(Arc::clone(&provider) as Arc<dyn LLMProvider>);
    let node = make_node(
        "extract",
        "parameter_extractor",
        extractor_config(
            "lives in NYC",
            &[
                ("city", "string", "city", true),
                ("country", "string", "country", false),
                ("zip", "string", "zip code", false),
            ],
        ),
    );

    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    let params = result.output["parameters"].as_object().unwrap();
    assert_eq!(params.len(), 3, "all declared fields should appear");
    assert_eq!(params.get("city").unwrap(), "NYC");
    assert!(params.get("country").unwrap().is_null());
    assert!(params.get("zip").unwrap().is_null());
}

#[tokio::test]
async fn test_extractor_strips_markdown_fence() {
    let provider = Arc::new(StubProvider::success(
        "stub",
        "stub-model",
        "```json\n{\"name\":\"Carol\"}\n```",
    ));
    let exec = ParameterExtractorNodeExecutor::new(Arc::clone(&provider) as Arc<dyn LLMProvider>);
    let node = make_node(
        "extract",
        "parameter_extractor",
        extractor_config("Carol", &[("name", "string", "name", true)]),
    );

    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    assert_eq!(result.output["parameters"]["name"], "Carol");
}

#[tokio::test]
async fn test_extractor_extracts_json_from_prose() {
    // LLM wraps the JSON with chatty text — parser should still find the {...} region.
    let provider = Arc::new(StubProvider::success(
        "stub",
        "stub-model",
        "Sure, here's the JSON:\n{\"name\":\"Dave\"}\nLet me know if you need anything else!",
    ));
    let exec = ParameterExtractorNodeExecutor::new(Arc::clone(&provider) as Arc<dyn LLMProvider>);
    let node = make_node(
        "extract",
        "parameter_extractor",
        extractor_config("Dave", &[("name", "string", "name", true)]),
    );

    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    assert_eq!(result.output["parameters"]["name"], "Dave");
}

#[tokio::test]
async fn test_extractor_retry_on_invalid_json() {
    use std::sync::atomic::AtomicUsize;

    // First call returns junk; second call returns valid JSON.
    struct TwoPhase {
        attempts: AtomicUsize,
        first: String,
        second: String,
    }
    #[async_trait]
    impl LLMProvider for TwoPhase {
        fn name(&self) -> &str { "two-phase" }
        fn default_model(&self) -> &str { "stub-model" }
        async fn chat(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _model: &str,
            _options: &ChatOptions,
        ) -> Result<LLMResponse, FailoverError> {
            let n = self.attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let content = if n == 0 { self.first.clone() } else { self.second.clone() };
            Ok(LLMResponse {
                content,
                tool_calls: Vec::new(),
                finish_reason: "stop".to_string(),
                usage: None,
                reasoning_content: None,
                extra: HashMap::new(),
                raw_request_body: None,
                raw_response_body: None,
            })
        }
    }

    let provider = Arc::new(TwoPhase {
        attempts: AtomicUsize::new(0),
        first: "this is not json at all".to_string(),
        second: r#"{"name":"Eve"}"#.to_string(),
    });
    let exec = ParameterExtractorNodeExecutor::new(Arc::clone(&provider) as Arc<dyn LLMProvider>);
    let node = make_node(
        "extract",
        "parameter_extractor",
        extractor_config("Eve", &[("name", "string", "name", true)]),
    );

    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    assert_eq!(result.output["parameters"]["name"], "Eve");
    assert_eq!(result.output["attempts"], 2);
}

#[tokio::test]
async fn test_extractor_fails_when_required_missing_after_retries() {
    // LLM never emits the required field — should exhaust retries and fail.
    let provider = Arc::new(StubProvider::success(
        "stub",
        "stub-model",
        r#"{"email":"a@b.com"}"#,
    ));
    let exec = ParameterExtractorNodeExecutor::new(Arc::clone(&provider) as Arc<dyn LLMProvider>);
    let mut cfg = extractor_config(
        "contact info",
        &[
            ("name", "string", "name", true),
            ("email", "string", "email", false),
        ],
    );
    cfg.insert("max_attempts".to_string(), serde_json::json!(2));
    let node = make_node("extract", "parameter_extractor", cfg);

    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Failed);
    let err = result.error.as_ref().unwrap();
    assert!(err.contains("failed after 2 attempts"), "got: {}", err);
    assert!(err.contains("missing required"), "got: {}", err);
    assert!(err.contains("name"), "got: {}", err);
}

#[tokio::test]
async fn test_extractor_missing_text_fails() {
    let provider = Arc::new(StubProvider::success("stub", "stub-model", "{}"));
    let exec = ParameterExtractorNodeExecutor::new(Arc::clone(&provider) as Arc<dyn LLMProvider>);
    let mut cfg = HashMap::new();
    cfg.insert(
        "parameters".to_string(),
        serde_json::json!([{"name": "x", "type": "string", "description": "", "required": false}]),
    );
    let node = make_node("extract", "parameter_extractor", cfg);

    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Failed);
    assert!(result.error.as_ref().unwrap().contains("missing required 'text'"));
}

#[tokio::test]
async fn test_extractor_missing_parameters_fails() {
    let provider = Arc::new(StubProvider::success("stub", "stub-model", "{}"));
    let exec = ParameterExtractorNodeExecutor::new(Arc::clone(&provider) as Arc<dyn LLMProvider>);
    let mut cfg = HashMap::new();
    cfg.insert("text".to_string(), serde_json::json!("hello"));
    let node = make_node("extract", "parameter_extractor", cfg);

    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Failed);
    assert!(result.error.as_ref().unwrap().contains("missing required 'parameters'"));
}

#[tokio::test]
async fn test_extractor_empty_parameters_fails() {
    let provider = Arc::new(StubProvider::success("stub", "stub-model", "{}"));
    let exec = ParameterExtractorNodeExecutor::new(Arc::clone(&provider) as Arc<dyn LLMProvider>);
    let node = make_node(
        "extract",
        "parameter_extractor",
        extractor_config("hello", &[]),
    );

    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Failed);
    assert!(result.error.as_ref().unwrap().contains("empty 'parameters'"));
}

#[tokio::test]
async fn test_extractor_resolves_template_in_text() {
    let provider = Arc::new(StubProvider::success(
        "stub",
        "stub-model",
        r#"{"name":"Frank"}"#,
    ));
    let exec = ParameterExtractorNodeExecutor::new(Arc::clone(&provider) as Arc<dyn LLMProvider>);
    let node = make_node(
        "extract",
        "parameter_extractor",
        extractor_config("{{user_input}}", &[("name", "string", "name", true)]),
    );
    let ctx = HashMap::from([(
        "user_input".to_string(),
        serde_json::json!("My name is Frank"),
    )]);

    let result = exec.execute(&node, &ctx, &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    assert_eq!(result.output["parameters"]["name"], "Frank");

    // The provider should have seen the resolved text, not the raw template.
    let last = provider.last_messages.lock().unwrap().clone();
    assert_eq!(last[1].content, "My name is Frank");
}

#[tokio::test]
async fn test_extractor_default_temperature_zero() {
    let provider = Arc::new(StubProvider::success("stub", "stub-model", "{}"));
    let exec = ParameterExtractorNodeExecutor::new(Arc::clone(&provider) as Arc<dyn LLMProvider>);
    let node = make_node(
        "extract",
        "parameter_extractor",
        extractor_config("hi", &[("x", "string", "", false)]),
    );

    let _ = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    let last_options = provider.last_options.lock().unwrap().clone().unwrap();
    assert_eq!(last_options.temperature, Some(0.0));
}

#[tokio::test]
async fn test_extractor_provider_error_retries_then_fails() {
    let provider = Arc::new(StubProvider::failing("stub", "stub-model", "rate limited"));
    let exec = ParameterExtractorNodeExecutor::new(Arc::clone(&provider) as Arc<dyn LLMProvider>);
    let mut cfg = extractor_config("hi", &[("x", "string", "", true)]);
    cfg.insert("max_attempts".to_string(), serde_json::json!(2));
    let node = make_node("extract", "parameter_extractor", cfg);

    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Failed);
    let err = result.error.as_ref().unwrap();
    assert!(err.contains("failed after 2 attempts"));
    assert!(err.contains("rate limited"));
}

#[test]
fn parse_json_object_handles_bare_object() {
    let v = parse_json_object(r#"{"a":1}"#).unwrap();
    assert_eq!(v["a"], 1);
}

#[test]
fn parse_json_object_strips_fences() {
    let v = parse_json_object("```json\n{\"a\":1}\n```").unwrap();
    assert_eq!(v["a"], 1);
    let v = parse_json_object("```\n{\"a\":1}\n```").unwrap();
    assert_eq!(v["a"], 1);
}

#[test]
fn parse_json_object_extracts_from_prose() {
    let v = parse_json_object("here you go:\n{\"a\":1}\nenjoy!").unwrap();
    assert_eq!(v["a"], 1);
}

#[test]
fn parse_json_object_rejects_non_object() {
    // Array parses as JSON but is rejected because we need an object.
    assert!(parse_json_object("[1,2,3]").is_err());
    assert!(parse_json_object("\"string\"").is_err());
    assert!(parse_json_object("not json at all").is_err());
    assert!(parse_json_object("").is_err());
}

// ---------------------------------------------------------------------------
// AgentNodeExecutor tests (1b-D2)
// ---------------------------------------------------------------------------

/// Records each `run_direct` invocation so tests can assert against them.
#[derive(Clone)]
struct CapturedCall {
    prompt: String,
    agent_id: String,
    max_turns: u32,
}

/// Test runner that returns a queued response and remembers the last call.
struct StubAgentRunner {
    response: String,
    fail_with: Option<String>,
    tools_used: Vec<String>,
    last_call: std::sync::Mutex<Option<CapturedCall>>,
    call_count: std::sync::atomic::AtomicUsize,
}

impl StubAgentRunner {
    fn success(response: &str, tools: &[&str]) -> Self {
        Self {
            response: response.to_string(),
            fail_with: None,
            tools_used: tools.iter().map(|s| s.to_string()).collect(),
            last_call: std::sync::Mutex::new(None),
            call_count: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn failing(err: &str) -> Self {
        Self {
            response: String::new(),
            fail_with: Some(err.to_string()),
            tools_used: Vec::new(),
            last_call: std::sync::Mutex::new(None),
            call_count: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.call_count.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait]
impl AgentRunner for StubAgentRunner {
    async fn run_direct(
        &self,
        prompt: &str,
        agent_id: &str,
        max_turns: u32,
    ) -> Result<AgentRunResult, String> {
        self.call_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        *self.last_call.lock().unwrap() = Some(CapturedCall {
            prompt: prompt.to_string(),
            agent_id: agent_id.to_string(),
            max_turns,
        });
        if let Some(e) = &self.fail_with {
            return Err(e.clone());
        }
        Ok(AgentRunResult {
            response: self.response.clone(),
            tools_used: self.tools_used.clone(),
        })
    }
}

fn agent_config(prompt: &str) -> HashMap<String, serde_json::Value> {
    HashMap::from([("prompt".to_string(), serde_json::json!(prompt))])
}

#[tokio::test]
async fn test_agent_node_executes() {
    let runner = Arc::new(StubAgentRunner::success("all done", &["weather", "memory"]));
    let exec = AgentNodeExecutor::new(Arc::clone(&runner) as Arc<dyn AgentRunner>);
    let node = make_node("agent", "agent", agent_config("What's the weather?"));

    let result = exec
        .execute(&node, &HashMap::new(), &empty_wf_ctx())
        .await
        .unwrap();

    assert_eq!(result.state, ExecutionState::Completed);
    let out = result.output.as_object().unwrap();
    assert_eq!(out.get("response").unwrap(), "all done");
    let tools = out.get("tools_used").unwrap().as_array().unwrap();
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0], "weather");
    assert_eq!(tools[1], "memory");

    // Default agent_id and max_turns.
    assert_eq!(out.get("agent_id").unwrap(), "workflow_agent");
    assert_eq!(out.get("max_turns").unwrap(), 5);

    // The runner saw the resolved prompt.
    let captured = runner.last_call.lock().unwrap().clone().unwrap();
    assert_eq!(captured.prompt, "What's the weather?");
    assert_eq!(captured.agent_id, "workflow_agent");
    assert_eq!(captured.max_turns, 5);
}

#[tokio::test]
async fn test_agent_node_respects_max_turns() {
    let runner = Arc::new(StubAgentRunner::success("ok", &[]));
    let exec = AgentNodeExecutor::new(Arc::clone(&runner) as Arc<dyn AgentRunner>);
    let mut cfg = agent_config("hello");
    cfg.insert("max_turns".to_string(), serde_json::json!(12));
    cfg.insert("agent_id".to_string(), serde_json::json!("custom_agent"));
    let node = make_node("agent", "agent", cfg);

    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);

    let captured = runner.last_call.lock().unwrap().clone().unwrap();
    assert_eq!(captured.max_turns, 12);
    assert_eq!(captured.agent_id, "custom_agent");
}

#[tokio::test]
async fn test_agent_node_error_propagation() {
    let runner = Arc::new(StubAgentRunner::failing("model timeout"));
    let exec = AgentNodeExecutor::new(Arc::clone(&runner) as Arc<dyn AgentRunner>);
    let node = make_node("agent", "agent", agent_config("hello"));

    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Failed);
    let err = result.error.as_ref().unwrap();
    assert!(err.contains("agent error"), "got: {}", err);
    assert!(err.contains("model timeout"), "got: {}", err);
}

#[tokio::test]
async fn test_agent_node_missing_prompt_fails() {
    let runner = Arc::new(StubAgentRunner::success("never called", &[]));
    let exec = AgentNodeExecutor::new(Arc::clone(&runner) as Arc<dyn AgentRunner>);
    let node = make_node("agent", "agent", HashMap::new());

    let result = exec.execute(&node, &HashMap::new(), &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Failed);
    assert!(result.error.as_ref().unwrap().contains("missing required 'prompt'"));
    // The runner should NOT have been called.
    assert_eq!(runner.calls(), 0);
}

#[tokio::test]
async fn test_agent_node_resolves_template_in_prompt() {
    let runner = Arc::new(StubAgentRunner::success("ok", &[]));
    let exec = AgentNodeExecutor::new(Arc::clone(&runner) as Arc<dyn AgentRunner>);
    let node = make_node(
        "agent",
        "agent",
        agent_config("Tell {{name}} about {{topic}}"),
    );
    let ctx = HashMap::from([
        ("name".to_string(), serde_json::json!("Alice")),
        ("topic".to_string(), serde_json::json!("quantum physics")),
    ]);

    let result = exec.execute(&node, &ctx, &empty_wf_ctx()).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);

    let captured = runner.last_call.lock().unwrap().clone().unwrap();
    assert_eq!(captured.prompt, "Tell Alice about quantum physics");
}

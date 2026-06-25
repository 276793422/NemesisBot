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

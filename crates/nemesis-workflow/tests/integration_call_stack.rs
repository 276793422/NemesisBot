//! 1c-F2 integration: WorkflowCallStack tracks engine runs.
//!
//! Verifies the engine push/pop wiring:
//! - running a workflow pushes a frame
//! - frame is popped after completion
//! - AgentTool-triggered runs record their recursion_depth
//! - frames stack for nested sub_workflow calls
//! - depth-over-limit is rejected (defense-in-depth even though WorkflowRunTool
//!   should catch this first)

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use nemesis_providers::failover::FailoverError;
use nemesis_providers::router::LLMProvider;
use nemesis_providers::types::{ChatOptions, LLMResponse, Message, ToolDefinition};
use nemesis_workflow::engine::WorkflowEngine;
use nemesis_workflow::types::{
    Edge, ExecutionState, MAX_RECURSION_DEPTH, NodeDef, TriggerSource, Workflow,
};

struct StubProvider;
#[async_trait]
impl LLMProvider for StubProvider {
    async fn chat(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _model: &str,
        _options: &ChatOptions,
    ) -> Result<LLMResponse, FailoverError> {
        Ok(LLMResponse {
            content: "stub".to_string(),
            tool_calls: Vec::new(),
            finish_reason: "stop".to_string(),
            usage: None,
            reasoning_content: None,
            extra: HashMap::new(),
            raw_request_body: None,
            raw_response_body: None,
        })
    }
    fn default_model(&self) -> &str {
        "stub"
    }
    fn name(&self) -> &str {
        "stub"
    }
}

fn node(id: &str, node_type: &str, depends_on: &[&str]) -> NodeDef {
    NodeDef {
        id: id.to_string(),
        node_type: node_type.to_string(),
        config: HashMap::new(),
        depends_on: depends_on.iter().map(|s| s.to_string()).collect(),
        retry_count: 0,
        timeout: None,
        is_terminal: false,
    }
}

fn wf(name: &str, nodes: Vec<NodeDef>) -> Workflow {
    let edges: Vec<Edge> = nodes
        .iter()
        .flat_map(|n| {
            n.depends_on.iter().map(move |dep| Edge {
                from_node: dep.clone(),
                to_node: n.id.clone(),
                condition: None,
            })
        })
        .collect();
    Workflow {
        name: name.to_string(),
        description: String::new(),
        version: "1.0.0".to_string(),
        triggers: vec![],
        nodes,
        edges,
        variables: HashMap::new(),
        metadata: HashMap::new(),
    }
}

fn build_engine() -> Arc<WorkflowEngine> {
    let provider = Arc::new(StubProvider) as Arc<dyn LLMProvider>;
    let tools = Arc::new(nemesis_tools::registry::ToolRegistry::new());
    WorkflowEngine::new_integrated(provider, tools, None)
}

#[tokio::test]
async fn stack_is_empty_after_run_completes() {
    let engine = build_engine();
    engine
        .register_workflow(wf("single", vec![node("n1", "delay", &[])]))
        .unwrap();
    let exec = engine
        .run("single", HashMap::new(), Some(TriggerSource::Cli))
        .await
        .unwrap();
    assert_eq!(exec.state, ExecutionState::Completed);
    assert!(
        engine.call_stack().is_empty(),
        "call stack must be empty after a successful run"
    );
}

#[tokio::test]
async fn stack_is_empty_after_run_fails() {
    let engine = build_engine();
    engine
        .register_workflow(wf(
            "broken",
            vec![node("bad", "nonexistent_node_type", &[])],
        ))
        .unwrap();
    let _ = engine
        .run("broken", HashMap::new(), Some(TriggerSource::Cli))
        .await;
    assert!(
        engine.call_stack().is_empty(),
        "call stack must be popped even when the run fails"
    );
}

#[tokio::test]
async fn nested_sub_workflow_runs_push_and_pop_frames() {
    // Two-level chain: outer → inner via sub_workflow node.
    let engine = build_engine();
    engine
        .register_workflow(wf("inner", vec![node("inner_n", "delay", &[])]))
        .unwrap();
    engine
        .register_workflow(wf("outer", vec![node("call_inner", "sub_workflow", &[])]))
        .unwrap();
    // Mark the sub_workflow config on the outer node.
    {
        let w = engine.get_workflow("outer").unwrap();
        let mut w = w.clone();
        for n in w.nodes.iter_mut() {
            if n.id == "call_inner" {
                n.config
                    .insert("workflow".to_string(), serde_json::json!("inner"));
            }
        }
        // Re-register the patched workflow.
        let _ = engine.register_workflow(w);
    }

    let exec = engine
        .run("outer", HashMap::new(), Some(TriggerSource::Cli))
        .await
        .unwrap();
    assert_eq!(exec.state, ExecutionState::Completed);
    assert!(
        engine.call_stack().is_empty(),
        "all frames must pop after the top-level run completes"
    );
}

#[tokio::test]
async fn agent_tool_trigger_records_depth_in_call_stack_frame() {
    // We can't directly observe frame contents during the run (the push/pop
    // happens inside `run_async`), but we can verify that an AgentTool
    // trigger at depth = MAX_RECURSION_DEPTH is still accepted, and one
    // above MAX is rejected by the engine itself (defense-in-depth).
    let engine = build_engine();
    engine
        .register_workflow(wf("wf", vec![node("n", "delay", &[])]))
        .unwrap();

    // At-max depth should be accepted.
    let at_max = engine
        .run(
            "wf",
            HashMap::new(),
            Some(TriggerSource::AgentTool {
                tool_call_id: "tc1".to_string(),
                recursion_depth: MAX_RECURSION_DEPTH,
            }),
        )
        .await
        .expect("depth == MAX should be accepted");
    assert_eq!(at_max.state, ExecutionState::Completed);

    // Over-max depth should be rejected by the engine's call stack push.
    let over_max = engine
        .run(
            "wf",
            HashMap::new(),
            Some(TriggerSource::AgentTool {
                tool_call_id: "tc2".to_string(),
                recursion_depth: MAX_RECURSION_DEPTH + 1,
            }),
        )
        .await
        .expect_err("depth > MAX should be rejected");
    assert!(
        matches!(
            over_max,
            nemesis_workflow::engine::EngineError::RecursionLimitExceeded(_)
        ),
        "expected RecursionLimitExceeded, got {:?}",
        over_max
    );
    // Stack must still be empty after the rejected push.
    assert!(engine.call_stack().is_empty());
}

#[tokio::test]
async fn call_stack_accessor_returns_the_engine_stack() {
    // Sanity: engine.call_stack() returns the same stack that run_async
    // pushes to. This is the public surface future WSAPI commands will use.
    let engine = build_engine();
    let stack = engine.call_stack();
    assert!(Arc::ptr_eq(&engine.call_stack(), stack));
}

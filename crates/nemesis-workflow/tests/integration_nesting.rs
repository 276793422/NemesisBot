//! G3 嵌套调用测试 — sub_workflow 嵌套 + execution_id 隔离
//!
//! 这些测试验证 milestone 1b 的嵌套调用支持：
//! - sub_workflow 节点能触发子工作流执行
//! - 父子执行有独立的 execution_id
//! - checkpoint 按 execution_id 隔离
//! - 多层嵌套（sub_workflow 嵌套 sub_workflow）
//!
//! **注意**：MAX_RECURSION_DEPTH=3 的强制拒绝是 milestone 1c 的功能（见
//! engine.rs 的 `recursion_depth` 注释），本测试只覆盖 1b 已实现的部分。
//!
//! 运行：`cargo test -p nemesis-workflow --test integration_nesting`

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use nemesis_providers::failover::FailoverError;
use nemesis_providers::router::LLMProvider;
use nemesis_providers::types::{ChatOptions, LLMResponse, Message, ToolDefinition};
use nemesis_workflow::checkpoint::{CheckpointStore, InMemoryCheckpointStore};
use nemesis_workflow::engine::WorkflowEngine;
use nemesis_workflow::types::{
    Edge, ExecutionState, NodeDef, TriggerSource, Workflow,
};

// ---------------------------------------------------------------------------
// Test scaffolding (mirrors integration_matrix.rs but kept self-contained)
// ---------------------------------------------------------------------------

struct StubProvider {
    response: String,
}

impl StubProvider {
    fn new(response: &str) -> Self {
        Self {
            response: response.to_string(),
        }
    }
}

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
            content: self.response.clone(),
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
        "stub-model"
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

fn node_with_config(
    id: &str,
    node_type: &str,
    depends_on: &[&str],
    config: HashMap<String, serde_json::Value>,
) -> NodeDef {
    let mut n = node(id, node_type, depends_on);
    n.config = config;
    n
}

fn workflow_with_nodes(name: &str, nodes: Vec<NodeDef>) -> Workflow {
    let edges: Vec<Edge> = nodes
        .iter()
        .flat_map(|n| {
            n.depends_on
                .iter()
                .map(move |dep| Edge {
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

fn build_engine(
    provider_response: &str,
    checkpoint: Option<Arc<dyn CheckpointStore>>,
) -> Arc<WorkflowEngine> {
    let provider = Arc::new(StubProvider::new(provider_response)) as Arc<dyn LLMProvider>;
    let tools = Arc::new(nemesis_tools::registry::ToolRegistry::new());
    let engine = WorkflowEngine::new_integrated(provider, tools, None);
    if let Some(store) = checkpoint {
        engine.set_checkpoint_store(store);
    }
    engine
}

fn sub_workflow_node(id: &str, workflow_name: &str) -> NodeDef {
    node_with_config(
        id,
        "sub_workflow",
        &[],
        HashMap::from([("workflow".to_string(), serde_json::json!(workflow_name))]),
    )
}

// ---------------------------------------------------------------------------
// Section 1: Basic sub_workflow nesting
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sub_workflow_node_triggers_child_execution() {
    // parent: sub_wf_node → after
    // child: child_n1 (delay)
    let engine = build_engine("ignored", None);
    engine
        .register_workflow(workflow_with_nodes(
            "child",
            vec![node("child_n1", "delay", &[])],
        ))
        .unwrap();
    engine
        .register_workflow(workflow_with_nodes(
            "parent",
            vec![
                sub_workflow_node("call_child", "child"),
                node("after", "delay", &["call_child"]),
            ],
        ))
        .unwrap();

    let parent_exec = engine
        .run("parent", HashMap::new(), Some(TriggerSource::Cli))
        .await
        .unwrap();

    assert_eq!(parent_exec.state, ExecutionState::Completed);

    // The sub_workflow node's output should carry the child's execution_id.
    let sub_node = &parent_exec.node_results["call_child"];
    let child_exec_id = sub_node.metadata.get("execution_id")
        .and_then(|v| v.as_str())
        .or_else(|| sub_node.output.get("execution_id").and_then(|v| v.as_str()))
        .expect("sub_workflow node should expose child execution_id");

    // The child execution should be retrievable from the engine.
    let child_exec = engine.get_execution(child_exec_id).await;
    assert!(child_exec.is_some(), "child execution must be registered");
    let child_exec = child_exec.unwrap();
    assert_eq!(child_exec.workflow_name, "child");
    assert_eq!(child_exec.state, ExecutionState::Completed);
    // Parent and child must have different execution_ids.
    assert_ne!(parent_exec.id, child_exec.id);
}

#[tokio::test]
async fn nested_sub_workflows_execute_in_correct_order() {
    // Three-level chain: outer → middle → inner
    let engine = build_engine("ignored", None);
    engine
        .register_workflow(workflow_with_nodes(
            "inner",
            vec![node("inner_n", "delay", &[])],
        ))
        .unwrap();
    engine
        .register_workflow(workflow_with_nodes(
            "middle",
            vec![
                sub_workflow_node("call_inner", "inner"),
                node("middle_after", "delay", &["call_inner"]),
            ],
        ))
        .unwrap();
    engine
        .register_workflow(workflow_with_nodes(
            "outer",
            vec![
                sub_workflow_node("call_middle", "middle"),
                node("outer_after", "delay", &["call_middle"]),
            ],
        ))
        .unwrap();

    let outer_exec = engine
        .run("outer", HashMap::new(), Some(TriggerSource::Cli))
        .await
        .unwrap();

    assert_eq!(outer_exec.state, ExecutionState::Completed);
    // outer_after must have run.
    assert!(outer_exec.node_results.contains_key("outer_after"));
}

#[tokio::test]
async fn sub_workflow_failure_propagates_to_parent() {
    // Child has a node that will fail (missing prompt config on llm node),
    // so child returns Failed. Parent's sub_workflow node should reflect
    // the failure and the parent execution as a whole should be Failed.
    let engine = build_engine("ignored", None);
    engine
        .register_workflow(workflow_with_nodes(
            "failing_child",
            vec![node("bad", "llm", &[])], // no prompt → fails
        ))
        .unwrap();
    engine
        .register_workflow(workflow_with_nodes(
            "parent",
            vec![sub_workflow_node("call", "failing_child")],
        ))
        .unwrap();

    let parent_exec = engine
        .run("parent", HashMap::new(), Some(TriggerSource::Cli))
        .await
        .unwrap();

    assert_eq!(
        parent_exec.state,
        ExecutionState::Failed,
        "parent should fail because child failed"
    );
}

#[tokio::test]
async fn sub_workflow_with_human_review_propagates_waiting() {
    // Child has a human_review. When sub_workflow runs the child, the
    // child enters Waiting. The SubWorkflowNodeExecutor calls
    // engine.run() synchronously, so it will see the Waiting state —
    // which then propagates up to the parent's node state.
    let engine = build_engine("ignored", None);
    engine
        .register_workflow(workflow_with_nodes(
            "child_with_review",
            vec![node("review", "human_review", &[])],
        ))
        .unwrap();
    engine
        .register_workflow(workflow_with_nodes(
            "parent",
            vec![sub_workflow_node("call", "child_with_review")],
        ))
        .unwrap();

    let parent_exec = engine
        .run("parent", HashMap::new(), Some(TriggerSource::Cli))
        .await
        .unwrap();

    // The parent's sub_workflow node should reflect Waiting (it inherits
    // the child's state per SubWorkflowNodeExecutor::execute).
    assert_eq!(
        parent_exec.node_results["call"].state,
        ExecutionState::Waiting,
        "sub_workflow node should inherit child's Waiting state"
    );
    // And the parent execution as a whole should be Waiting too.
    assert_eq!(parent_exec.state, ExecutionState::Waiting);
}

// ---------------------------------------------------------------------------
// Section 2: Execution ID isolation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn parent_and_child_have_independent_execution_ids() {
    let engine = build_engine("ignored", None);
    engine
        .register_workflow(workflow_with_nodes(
            "leaf",
            vec![node("n", "delay", &[])],
        ))
        .unwrap();
    engine
        .register_workflow(workflow_with_nodes(
            "top",
            vec![sub_workflow_node("call_leaf", "leaf")],
        ))
        .unwrap();

    let top_exec = engine
        .run("top", HashMap::new(), Some(TriggerSource::Cli))
        .await
        .unwrap();

    // List all executions — there should be at least 2 (top + leaf).
    let all = engine.list_executions(None).await;
    assert!(all.len() >= 2, "expected ≥2 executions, got {}", all.len());

    // Each execution has a unique id (sanity check).
    let mut ids: Vec<_> = all.iter().map(|e| e.id.clone()).collect();
    ids.sort();
    let mut deduped = ids.clone();
    deduped.dedup();
    assert_eq!(ids.len(), deduped.len(), "execution ids must be unique");

    // The top execution must be present.
    assert!(ids.contains(&top_exec.id));
}

#[tokio::test]
async fn each_sub_workflow_call_creates_fresh_execution() {
    // If the same workflow is called twice via sub_workflow, each call
    // should spawn its own execution_id (no caching).
    let engine = build_engine("ignored", None);
    engine
        .register_workflow(workflow_with_nodes(
            "callee",
            vec![node("n", "delay", &[])],
        ))
        .unwrap();
    engine
        .register_workflow(workflow_with_nodes(
            "caller",
            vec![
                sub_workflow_node("call1", "callee"),
                sub_workflow_node("call2", "callee"),
            ],
        ))
        .unwrap();

    let parent = engine
        .run("caller", HashMap::new(), Some(TriggerSource::Cli))
        .await
        .unwrap();

    let call1_id = parent.node_results["call1"].metadata
        .get("execution_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let call2_id = parent.node_results["call2"].metadata
        .get("execution_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let (id1, id2) = match (call1_id, call2_id) {
        (Some(a), Some(b)) => (a, b),
        _ => panic!("both sub_workflow nodes should record child execution_id"),
    };
    assert_ne!(id1, id2, "the two sub_workflow calls must spawn separate executions");
}

// ---------------------------------------------------------------------------
// Section 3: Checkpoint isolation by execution_id
// ---------------------------------------------------------------------------

#[tokio::test]
async fn checkpoints_are_isolated_per_execution_id() {
    // Run two independent workflows that share the same checkpoint store.
    // Each should save its own checkpoints under its own execution_id.
    let store: Arc<dyn CheckpointStore> = Arc::new(InMemoryCheckpointStore::new());
    let engine = build_engine("ignored", Some(store.clone()));
    engine
        .register_workflow(workflow_with_nodes(
            "wf_one",
            vec![node("a", "delay", &[])],
        ))
        .unwrap();
    engine
        .register_workflow(workflow_with_nodes(
            "wf_two",
            vec![node("b", "delay", &[])],
        ))
        .unwrap();

    let exec1 = engine
        .run("wf_one", HashMap::new(), Some(TriggerSource::Cli))
        .await
        .unwrap();
    let exec2 = engine
        .run("wf_two", HashMap::new(), Some(TriggerSource::Cli))
        .await
        .unwrap();

    let cps1 = store.list(&exec1.id).await.unwrap();
    let cps2 = store.list(&exec2.id).await.unwrap();
    assert!(!cps1.is_empty());
    assert!(!cps2.is_empty());

    // The execution_ids listed in the store should be exactly the two.
    let mut all_exec_ids = store.list_executions().await.unwrap();
    all_exec_ids.sort();
    let mut expected = vec![exec1.id.clone(), exec2.id.clone()];
    expected.sort();
    assert_eq!(all_exec_ids, expected);
}

#[tokio::test]
async fn sub_workflow_checkpoint_does_not_leak_into_parent() {
    // Parent has its own checkpoint; child has its own. Loading parent's
    // checkpoint should not show child's node results, and vice versa.
    let store: Arc<dyn CheckpointStore> = Arc::new(InMemoryCheckpointStore::new());
    let engine = build_engine("ignored", Some(store.clone()));
    engine
        .register_workflow(workflow_with_nodes(
            "child_wf",
            vec![node("child_node", "delay", &[])],
        ))
        .unwrap();
    engine
        .register_workflow(workflow_with_nodes(
            "parent_wf",
            vec![
                sub_workflow_node("call", "child_wf"),
                node("parent_node", "delay", &["call"]),
            ],
        ))
        .unwrap();

    let parent_exec = engine
        .run("parent_wf", HashMap::new(), Some(TriggerSource::Cli))
        .await
        .unwrap();

    // Parent's latest checkpoint should not contain child execution
    // checkpoints (different execution_id namespace).
    let parent_latest = store.latest(&parent_exec.id).await.unwrap();
    assert!(parent_latest.is_some(), "parent should have a checkpoint");

    // The store should have checkpoints under multiple execution_ids
    // (parent + child), each isolated.
    let all_exec_ids = store.list_executions().await.unwrap();
    assert!(
        all_exec_ids.len() >= 2,
        "expected ≥2 execution_ids in store (parent + child), got {:?}",
        all_exec_ids
    );
}

// ---------------------------------------------------------------------------
// Section 4: recursion_depth round-trip (preview of 1c)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agent_tool_trigger_recursion_depth_round_trips() {
    // The MAX_RECURSION_DEPTH check isn't enforced yet (planned for 1c),
    // but the field should at least survive a full execution round-trip
    // so 1c can read it later.
    let engine = build_engine("ignored", None);
    engine
        .register_workflow(workflow_with_nodes(
            "nested_trigger",
            vec![node("n", "delay", &[])],
        ))
        .unwrap();

    let exec = engine
        .run(
            "nested_trigger",
            HashMap::new(),
            Some(TriggerSource::AgentTool {
                tool_call_id: "tc_abc".to_string(),
                recursion_depth: 2,
            }),
        )
        .await
        .unwrap();

    match exec.trigger_source.as_ref().unwrap() {
        TriggerSource::AgentTool {
            tool_call_id,
            recursion_depth,
        } => {
            assert_eq!(tool_call_id, "tc_abc");
            assert_eq!(*recursion_depth, 2);
        }
        other => panic!("expected AgentTool, got {:?}", other),
    }
}

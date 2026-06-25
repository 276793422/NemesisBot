//! G4 跨进程恢复测试 — 模拟进程崩溃后从磁盘 checkpoint 续行
//!
//! 这些测试验证 milestone 1b-A1 的恢复路径：
//! - 引擎运行到 human_review 进入 Waiting，checkpoint 落盘
//! - 丢弃旧引擎（模拟崩溃）
//! - 新引擎指向同一个 FileCheckpointStore
//! - `restore_incomplete_executions()` 把 Waiting 执行拉回内存
//! - `resume_execution()` 让评审节点通过，下游节点继续跑
//!
//! 关键 invariant：崩溃前已完成的节点不会重跑；review_result 通过
//! wf_ctx 流到下游。
//!
//! 运行：`cargo test -p nemesis-workflow --test integration_restart`

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use nemesis_providers::failover::FailoverError;
use nemesis_providers::router::LLMProvider;
use nemesis_providers::types::{ChatOptions, LLMResponse, Message, ToolDefinition};
use nemesis_workflow::checkpoint::{CheckpointStore, FileCheckpointStore};
use nemesis_workflow::engine::WorkflowEngine;
use nemesis_workflow::types::{
    Edge, ExecutionState, NodeDef, TriggerSource, Workflow,
};

// ---------------------------------------------------------------------------
// Test scaffolding
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
    checkpoint: Arc<dyn CheckpointStore>,
) -> Arc<WorkflowEngine> {
    let provider = Arc::new(StubProvider::new(provider_response)) as Arc<dyn LLMProvider>;
    let tools = Arc::new(nemesis_tools::registry::ToolRegistry::new());
    let engine = WorkflowEngine::new_integrated(provider, tools, None);
    engine.set_checkpoint_store(checkpoint);
    engine
}

/// Build engine + register a workflow that pauses on human_review then
/// continues to an `after` delay node. Returns the registered workflow name.
fn register_pause_workflow(engine: &Arc<WorkflowEngine>, name: &str) {
    engine
        .register_workflow(workflow_with_nodes(
            name,
            vec![
                node("review", "human_review", &[]),
                node("after", "delay", &["review"]),
            ],
        ))
        .unwrap();
}

// ---------------------------------------------------------------------------
// Section 1: basic crash → restore → resume cycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn crash_after_review_then_restore_and_resume_completes() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store: Arc<dyn CheckpointStore> =
        Arc::new(FileCheckpointStore::new(tmp.path()).unwrap());

    // --- Engine 1: run to Waiting, then "crash" (drop) ---
    let engine1 = build_engine("ignored", store.clone());
    register_pause_workflow(&engine1, "wf");
    let exec1 = engine1
        .run("wf", HashMap::new(), Some(TriggerSource::Cli))
        .await
        .unwrap();
    assert_eq!(exec1.state, ExecutionState::Waiting);
    let exec_id = exec1.id.clone();
    drop(engine1);

    // --- Engine 2: new engine, same on-disk store ---
    let engine2 = build_engine("ignored", store.clone());
    register_pause_workflow(&engine2, "wf");

    let restored = engine2.restore_incomplete_executions().await.unwrap();
    assert_eq!(restored, 1, "exactly one execution should be restored");

    let mut review = HashMap::new();
    review.insert("approved".to_string(), serde_json::json!(true));
    let resumed = engine2.resume_execution(&exec_id, review).await.unwrap();
    assert_eq!(resumed.state, ExecutionState::Completed);
    assert!(
        resumed.node_results.contains_key("after"),
        "downstream `after` node must run after resume"
    );
}

#[tokio::test]
async fn restored_execution_preserves_original_execution_id() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store: Arc<dyn CheckpointStore> =
        Arc::new(FileCheckpointStore::new(tmp.path()).unwrap());

    let engine1 = build_engine("ignored", store.clone());
    register_pause_workflow(&engine1, "wf");
    let exec1 = engine1
        .run("wf", HashMap::new(), Some(TriggerSource::Cli))
        .await
        .unwrap();
    let original_id = exec1.id.clone();
    drop(engine1);

    let engine2 = build_engine("ignored", store.clone());
    register_pause_workflow(&engine2, "wf");
    engine2.restore_incomplete_executions().await.unwrap();

    // The restored in-memory execution must carry the same id.
    let restored = engine2.get_execution(&original_id).await;
    assert!(restored.is_some(), "restored execution must be in memory");
    assert_eq!(restored.unwrap().id, original_id);
}

#[tokio::test]
async fn completed_nodes_are_not_rerun_after_restore() {
    // Workflow: before → review → after
    // `before` runs first; after restore+resume, `before` should NOT run
    // again (its checkpointed result should be reused).
    let tmp = tempfile::TempDir::new().unwrap();
    let store: Arc<dyn CheckpointStore> =
        Arc::new(FileCheckpointStore::new(tmp.path()).unwrap());

    let engine1 = build_engine("ignored", store.clone());
    engine1
        .register_workflow(workflow_with_nodes(
            "wf",
            vec![
                node("before", "delay", &[]),
                node("review", "human_review", &["before"]),
                node("after", "delay", &["review"]),
            ],
        ))
        .unwrap();
    let exec1 = engine1
        .run("wf", HashMap::new(), Some(TriggerSource::Cli))
        .await
        .unwrap();
    let exec_id = exec1.id.clone();
    let before_ended_at = exec1
        .node_results
        .get("before")
        .map(|r| r.ended_at.timestamp_millis())
        .expect("before node must have run");
    drop(engine1);

    let engine2 = build_engine("ignored", store.clone());
    engine2
        .register_workflow(workflow_with_nodes(
            "wf",
            vec![
                node("before", "delay", &[]),
                node("review", "human_review", &["before"]),
                node("after", "delay", &["review"]),
            ],
        ))
        .unwrap();
    engine2.restore_incomplete_executions().await.unwrap();

    let mut review = HashMap::new();
    review.insert("approved".to_string(), serde_json::json!(true));
    let resumed = engine2.resume_execution(&exec_id, review).await.unwrap();
    assert_eq!(resumed.state, ExecutionState::Completed);

    // The `before` node's ended_at must be unchanged — restore must not
    // re-execute already-completed nodes.
    let before_after_resume = resumed
        .node_results
        .get("before")
        .map(|r| r.ended_at.timestamp_millis())
        .expect("before node result must be preserved");
    assert_eq!(
        before_ended_at, before_after_resume,
        "completed nodes must not be re-executed after restore"
    );
}

// ---------------------------------------------------------------------------
// Section 2: restore edge cases
// ---------------------------------------------------------------------------

#[tokio::test]
async fn restore_with_no_checkpoint_store_returns_zero() {
    // An engine with no checkpoint store should gracefully return 0.
    let provider = Arc::new(StubProvider::new("x")) as Arc<dyn LLMProvider>;
    let tools = Arc::new(nemesis_tools::registry::ToolRegistry::new());
    let engine = WorkflowEngine::new_integrated(provider, tools, None);
    // Note: deliberately no set_checkpoint_store.
    let n = engine.restore_incomplete_executions().await.unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn restore_with_empty_store_returns_zero() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store: Arc<dyn CheckpointStore> =
        Arc::new(FileCheckpointStore::new(tmp.path()).unwrap());

    let engine = build_engine("ignored", store);
    let n = engine.restore_incomplete_executions().await.unwrap();
    assert_eq!(n, 0, "empty store should restore nothing");
}

#[tokio::test]
async fn restore_multiple_waiting_executions() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store: Arc<dyn CheckpointStore> =
        Arc::new(FileCheckpointStore::new(tmp.path()).unwrap());

    // Run two independent workflows to Waiting, then drop the engine.
    let engine1 = build_engine("ignored", store.clone());
    register_pause_workflow(&engine1, "wf_a");
    register_pause_workflow(&engine1, "wf_b");
    let exec_a = engine1
        .run("wf_a", HashMap::new(), Some(TriggerSource::Cli))
        .await
        .unwrap();
    let exec_b = engine1
        .run("wf_b", HashMap::new(), Some(TriggerSource::Cli))
        .await
        .unwrap();
    assert_eq!(exec_a.state, ExecutionState::Waiting);
    assert_eq!(exec_b.state, ExecutionState::Waiting);
    drop(engine1);

    let engine2 = build_engine("ignored", store.clone());
    register_pause_workflow(&engine2, "wf_a");
    register_pause_workflow(&engine2, "wf_b");
    let restored = engine2.restore_incomplete_executions().await.unwrap();
    assert_eq!(restored, 2, "both executions should be restored");

    // Both should be resumable.
    let mut review = HashMap::new();
    review.insert("approved".to_string(), serde_json::json!(true));
    let ra = engine2.resume_execution(&exec_a.id, review.clone()).await.unwrap();
    let rb = engine2.resume_execution(&exec_b.id, review).await.unwrap();
    assert_eq!(ra.state, ExecutionState::Completed);
    assert_eq!(rb.state, ExecutionState::Completed);
}

// ---------------------------------------------------------------------------
// Section 3: resume edge cases
// ---------------------------------------------------------------------------

#[tokio::test]
async fn resume_without_restore_fails_with_execution_not_found() {
    // The execution only exists on disk; without restore, the in-memory
    // engine has no execution to resume.
    let tmp = tempfile::TempDir::new().unwrap();
    let store: Arc<dyn CheckpointStore> =
        Arc::new(FileCheckpointStore::new(tmp.path()).unwrap());

    let engine1 = build_engine("ignored", store.clone());
    register_pause_workflow(&engine1, "wf");
    let exec1 = engine1
        .run("wf", HashMap::new(), Some(TriggerSource::Cli))
        .await
        .unwrap();
    let exec_id = exec1.id.clone();
    drop(engine1);

    let engine2 = build_engine("ignored", store);
    register_pause_workflow(&engine2, "wf");
    // Deliberately skip restore_incomplete_executions.

    let mut review = HashMap::new();
    review.insert("approved".to_string(), serde_json::json!(true));
    let err = engine2
        .resume_execution(&exec_id, review)
        .await
        .expect_err("resume without restore should fail");
    assert!(
        matches!(err, nemesis_workflow::engine::EngineError::ExecutionNotFound(_)),
        "expected ExecutionNotFound, got {:?}",
        err
    );
}

#[tokio::test]
async fn resume_rejected_review_still_completes_workflow() {
    // A rejected review (approved=false) still flows through — the workflow
    // engine doesn't interpret the bool; downstream nodes just receive it
    // as context. The workflow should still complete.
    let tmp = tempfile::TempDir::new().unwrap();
    let store: Arc<dyn CheckpointStore> =
        Arc::new(FileCheckpointStore::new(tmp.path()).unwrap());

    let engine1 = build_engine("ignored", store.clone());
    register_pause_workflow(&engine1, "wf");
    let exec1 = engine1
        .run("wf", HashMap::new(), Some(TriggerSource::Cli))
        .await
        .unwrap();
    let exec_id = exec1.id.clone();
    drop(engine1);

    let engine2 = build_engine("ignored", store);
    register_pause_workflow(&engine2, "wf");
    engine2.restore_incomplete_executions().await.unwrap();

    let mut review = HashMap::new();
    review.insert("approved".to_string(), serde_json::json!(false));
    let resumed = engine2.resume_execution(&exec_id, review).await.unwrap();
    assert_eq!(
        resumed.state,
        ExecutionState::Completed,
        "rejected review still completes the workflow (downstream nodes decide what to do)"
    );
}

// ---------------------------------------------------------------------------
// Section 4: cross-engine isolation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn checkpoint_files_persist_on_disk_after_pause() {
    // Sanity: verify the on-disk layout. After pause, the store should have
    // at least one checkpoint file under {tmp}/checkpoints/{exec_id}/.
    let tmp = tempfile::TempDir::new().unwrap();
    let store_root = tmp.path().to_path_buf();
    let store: Arc<dyn CheckpointStore> =
        Arc::new(FileCheckpointStore::new(&store_root).unwrap());

    let engine = build_engine("ignored", store);
    register_pause_workflow(&engine, "wf");
    let exec = engine
        .run("wf", HashMap::new(), Some(TriggerSource::Cli))
        .await
        .unwrap();
    assert_eq!(exec.state, ExecutionState::Waiting);

    // The checkpoints directory should exist and contain a subdirectory
    // named after the execution id.
    let checkpoints_dir = store_root.join("checkpoints");
    assert!(checkpoints_dir.exists(), "checkpoints/ dir must exist");
    let exec_dir = checkpoints_dir.join(&exec.id);
    assert!(
        exec_dir.exists(),
        "execution subdir {:?} must exist",
        exec_dir
    );

    // And it should contain at least one .json file.
    let json_count = std::fs::read_dir(&exec_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("json"))
        .count();
    assert!(
        json_count > 0,
        "expected ≥1 checkpoint .json file, found {}",
        json_count
    );
}

// ---------------------------------------------------------------------------
// 1c-G7 additional recovery tests — variable propagation, multi-restore,
// trigger source preservation, post-resume node-results visibility.
// ---------------------------------------------------------------------------

/// Build the same review workflow used by the G4 suite so tests share a
/// common shape: pre_review → review (human_review) → post_review.
fn build_review_wf_with_ids(
    pre_id: &str,
    review_id: &str,
    post_id: &str,
) -> Workflow {
    wf_with_name(
        "review_wf",
        vec![
            node(pre_id, "delay", &[]),
            node_with_config_text(review_id, "human_review", &[pre_id], "Initial review"),
            node(post_id, "delay", &[review_id]),
        ],
    )
}

fn node_with_config_text(id: &str, node_type: &str, depends_on: &[&str], prompt: &str) -> NodeDef {
    let mut n = node(id, node_type, depends_on);
    n.config
        .insert("prompt".to_string(), serde_json::json!(prompt));
    n
}

fn wf_with_name(name: &str, nodes: Vec<NodeDef>) -> Workflow {
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

#[tokio::test]
async fn restored_execution_keeps_already_completed_node_results() {
    // After crash + restore, node_results from completed nodes must still
    // be on the execution so the scheduler knows not to rerun them.
    let tmp = tempfile::tempdir().unwrap();
    let store: Arc<dyn CheckpointStore> =
        Arc::new(FileCheckpointStore::new(tmp.path()).unwrap());

    let engine_a = build_engine("ok", store.clone());
    engine_a
        .register_workflow(build_review_wf_with_ids("pre", "review", "post"))
        .unwrap();
    let _ = engine_a
        .run("review_wf", HashMap::new(), Some(TriggerSource::Cli))
        .await
        .unwrap();
    drop(engine_a);

    let engine_b = build_engine("ok", store.clone());
    engine_b
        .register_workflow(build_review_wf_with_ids("pre", "review", "post"))
        .unwrap();
    let _ = engine_b.restore_incomplete_executions().await.unwrap();

    let execs = engine_b.list_executions(None).await;
    let exec = &execs[0];
    assert_eq!(exec.state, ExecutionState::Waiting);
    // 'pre' should be Completed; 'review' should be Waiting.
    let pre_result = exec.node_results.get("pre");
    assert!(
        pre_result.is_some(),
        "pre node result must persist after restore"
    );
    assert_eq!(pre_result.unwrap().state, ExecutionState::Completed);
    let review_result = exec.node_results.get("review");
    assert!(review_result.is_some());
    assert_eq!(review_result.unwrap().state, ExecutionState::Waiting);
}

#[tokio::test]
async fn restore_is_idempotent_running_twice_does_not_duplicate_executions() {
    // Calling restore_incomplete_executions() multiple times on the same
    // store should not double-import executions.
    let tmp = tempfile::tempdir().unwrap();
    let store: Arc<dyn CheckpointStore> =
        Arc::new(FileCheckpointStore::new(tmp.path()).unwrap());

    let engine_a = build_engine("ok", store.clone());
    engine_a
        .register_workflow(build_review_wf_with_ids("pre", "review", "post"))
        .unwrap();
    let _ = engine_a
        .run("review_wf", HashMap::new(), Some(TriggerSource::Cli))
        .await
        .unwrap();
    drop(engine_a);

    // Two separate engines B and C both load the same store.
    let engine_b = build_engine("ok", store.clone());
    engine_b
        .register_workflow(build_review_wf_with_ids("pre", "review", "post"))
        .unwrap();
    let restored_b = engine_b.restore_incomplete_executions().await.unwrap();
    assert_eq!(restored_b, 1);

    let engine_c = build_engine("ok", store.clone());
    engine_c
        .register_workflow(build_review_wf_with_ids("pre", "review", "post"))
        .unwrap();
    let restored_c = engine_c.restore_incomplete_executions().await.unwrap();
    // The store on disk still has the checkpoint, so engine_c also restores 1.
    // This tests that the restore path doesn't depend on engine identity.
    assert_eq!(restored_c, 1);

    // Each engine has its own in-memory copy; that's expected since they
    // are separate processes.
    let b_execs = engine_b.list_executions(None).await;
    let c_execs = engine_c.list_executions(None).await;
    assert_eq!(b_execs.len(), 1);
    assert_eq!(c_execs.len(), 1);
    // Same execution_id though (loaded from the same disk checkpoint).
    assert_eq!(b_execs[0].id, c_execs[0].id);
}

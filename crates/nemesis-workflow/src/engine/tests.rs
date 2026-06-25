use super::*;
use crate::checkpoint::{CheckpointStore, InMemoryCheckpointStore};
use std::collections::HashMap;
use std::sync::Arc;

fn make_workflow(name: &str, nodes: Vec<NodeDef>) -> Workflow {
    Workflow {
        name: name.to_string(),
        description: String::new(),
        version: "1.0.0".to_string(),
        triggers: vec![],
        nodes,
        edges: vec![],
        variables: HashMap::new(),
        metadata: HashMap::new(),
    }
}

fn make_node(id: &str, node_type: &str, depends_on: Vec<&str>) -> NodeDef {
    NodeDef {
        id: id.to_string(),
        node_type: node_type.to_string(),
        config: HashMap::new(),
        depends_on: depends_on.into_iter().map(|s| s.to_string()).collect(),
        retry_count: 0,
        timeout: None,
    is_terminal: false,
    }
}

#[tokio::test]
async fn test_register_and_get_workflow() {
    let engine = WorkflowEngine::new();
    let wf = make_workflow("test_wf", vec![make_node("n1", "llm", vec![])]);
    engine.register_workflow(wf).unwrap();

    let retrieved = engine.get_workflow("test_wf").unwrap();
    assert_eq!(retrieved.name, "test_wf");
    assert!(engine.get_workflow("nonexistent").is_none());
}

#[tokio::test]
async fn test_list_workflows() {
    let engine = WorkflowEngine::new();
    engine
        .register_workflow(make_workflow("wf_a", vec![make_node("n1", "llm", vec![])]))
        .unwrap();
    engine
        .register_workflow(make_workflow("wf_b", vec![make_node("n1", "llm", vec![])]))
        .unwrap();

    let mut names = engine.list_workflows();
    names.sort();
    assert_eq!(names, vec!["wf_a", "wf_b"]);
}

#[tokio::test]
async fn test_unregister_workflow() {
    let engine = WorkflowEngine::new();
    engine
        .register_workflow(make_workflow("wf_a", vec![make_node("n1", "llm", vec![])]))
        .unwrap();
    assert!(engine.get_workflow("wf_a").is_some());

    engine.unregister("wf_a");
    assert!(engine.get_workflow("wf_a").is_none());
}

#[tokio::test]
async fn test_unregister_nonexistent() {
    let engine = WorkflowEngine::new();
    // Should not panic
    engine.unregister("nonexistent");
}

#[tokio::test]
async fn test_start_execution_basic() {
    let engine = WorkflowEngine::new();
    let nodes = vec![
        make_node("n1", "llm", vec![]),
        make_node("n2", "tool", vec!["n1"]),
    ];
    engine
        .register_workflow(make_workflow("chain_wf", nodes))
        .unwrap();

    let execution = engine
        .start_execution("chain_wf", HashMap::new())
        .await
        .unwrap();

    assert_eq!(execution.state, ExecutionState::Completed);
    assert_eq!(execution.node_results.len(), 2);
    assert!(execution.node_results.contains_key("n1"));
    assert!(execution.node_results.contains_key("n2"));
}

#[tokio::test]
async fn test_run_not_found() {
    let engine = WorkflowEngine::new();
    let result = engine.run("nonexistent", HashMap::new(), None).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, EngineError::WorkflowNotFound(_)));
}

#[tokio::test]
async fn test_condition_evaluation_in_execution() {
    let engine = WorkflowEngine::new();
    let mut cond_config = HashMap::new();
    cond_config.insert("condition".to_string(), serde_json::json!("status == ok"));

    let nodes = vec![
        make_node("n1", "llm", vec![]),
        NodeDef {
            id: "n2".to_string(),
            node_type: "condition".to_string(),
            config: cond_config,
            depends_on: vec!["n1".to_string()],
            retry_count: 0,
            timeout: None,
        is_terminal: false,
        },
    ];
    engine
        .register_workflow(make_workflow("cond_wf", nodes))
        .unwrap();

    let mut input = HashMap::new();
    input.insert("status".to_string(), serde_json::json!("ok"));

    let execution = engine.start_execution("cond_wf", input).await.unwrap();
    assert_eq!(execution.state, ExecutionState::Completed);

    let cond_result = &execution.node_results["n2"];
    assert!(cond_result.output["condition_result"].as_bool().unwrap());
}

#[tokio::test]
async fn test_dependency_ordering_respected() {
    let engine = WorkflowEngine::new();
    let nodes = vec![
        make_node("a", "llm", vec![]),
        make_node("b", "tool", vec!["a"]),
        make_node("c", "transform", vec!["b"]),
    ];
    engine
        .register_workflow(make_workflow("ordered_wf", nodes))
        .unwrap();

    let execution = engine
        .start_execution("ordered_wf", HashMap::new())
        .await
        .unwrap();

    assert_eq!(execution.state, ExecutionState::Completed);
    // All three nodes should have completed.
    assert_eq!(execution.node_results.len(), 3);
    for (id, result) in &execution.node_results {
        assert_eq!(result.state, ExecutionState::Completed, "node {} failed", id);
    }
}

// -----------------------------------------------------------------------
// get_execution tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_get_execution_found() {
    let engine = WorkflowEngine::new();
    engine
        .register_workflow(make_workflow("wf", vec![make_node("n1", "llm", vec![])]))
        .unwrap();

    let execution = engine
        .start_execution("wf", HashMap::new())
        .await
        .unwrap();
    let id = execution.id.clone();

    let retrieved = engine.get_execution(&id).await;
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().id, id);
}

#[tokio::test]
async fn test_get_execution_not_found() {
    let engine = WorkflowEngine::new();
    let result = engine.get_execution("nonexistent_id").await;
    assert!(result.is_none());
}

#[tokio::test]
async fn test_get_execution_or_err() {
    let engine = WorkflowEngine::new();
    let result = engine.get_execution_or_err("nonexistent_id").await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, EngineError::ExecutionNotFound(_)));
}

// -----------------------------------------------------------------------
// cancel_execution tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_cancel_running_execution() {
    let engine = WorkflowEngine::new();
    engine
        .register_workflow(make_workflow("wf", vec![make_node("n1", "llm", vec![])]))
        .unwrap();

    let execution = engine
        .start_execution("wf", HashMap::new())
        .await
        .unwrap();
    // Execution is already completed since start_execution is synchronous.
    // Let's manually set up a running execution for testing cancel.
    let id = execution.id.clone();

    // For a real cancel test we'd need a long-running workflow.
    // Here we test the state check: cancelling a completed execution should fail.
    let result = engine.cancel_execution(&id).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, EngineError::InvalidState(_)));
}

#[tokio::test]
async fn test_cancel_nonexistent_execution() {
    let engine = WorkflowEngine::new();
    let result = engine.cancel_execution("nonexistent").await;
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        EngineError::ExecutionNotFound(_)
    ));
}

// -----------------------------------------------------------------------
// resume_execution tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_resume_waiting_execution() {
    let engine = WorkflowEngine::new();
    let mut hr_config = HashMap::new();
    hr_config.insert("message".to_string(), serde_json::json!("Please review"));

    let nodes = vec![NodeDef {
        id: "n1".to_string(),
        node_type: "human_review".to_string(),
        config: hr_config,
        depends_on: vec![],
        retry_count: 0,
        timeout: None,
    is_terminal: false,
    }];
    engine
        .register_workflow(make_workflow("hr_wf", nodes))
        .unwrap();

    let execution = engine
        .start_execution("hr_wf", HashMap::new())
        .await
        .unwrap();
    assert_eq!(execution.state, ExecutionState::Waiting);

    let id = execution.id.clone();
    let mut review = HashMap::new();
    review.insert("approved".to_string(), serde_json::json!(true));
    review.insert("comment".to_string(), serde_json::json!("Looks good"));

    engine.resume_execution(&id, review).await.unwrap();

    let updated = engine.get_execution(&id).await.unwrap();
    assert_eq!(updated.state, ExecutionState::Completed);
    assert!(updated.ended_at.is_some());
}

#[tokio::test]
async fn test_resume_non_waiting_execution() {
    let engine = WorkflowEngine::new();
    engine
        .register_workflow(make_workflow("wf", vec![make_node("n1", "llm", vec![])]))
        .unwrap();

    let execution = engine
        .start_execution("wf", HashMap::new())
        .await
        .unwrap();
    // Execution completed normally
    assert_eq!(execution.state, ExecutionState::Completed);

    let id = execution.id.clone();
    let result = engine.resume_execution(&id, HashMap::new()).await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), EngineError::InvalidState(_)));
}

#[tokio::test]
async fn test_resume_nonexistent_execution() {
    let engine = WorkflowEngine::new();
    let result = engine
        .resume_execution("nonexistent", HashMap::new())
        .await;
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        EngineError::ExecutionNotFound(_)
    ));
}

// -----------------------------------------------------------------------
// resume_execution runs downstream nodes (1b-A1 step 5 regression)
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_resume_runs_downstream_nodes() {
    // n1: human_review (Waiting), n2: llm (downstream). After resume, n2 must
    // have run — not just be marked Completed by the resume path itself.
    let engine = WorkflowEngine::new_arc();
    let nodes = vec![
        NodeDef {
            id: "review".to_string(),
            node_type: "human_review".to_string(),
            config: HashMap::from([(
                "message".to_string(),
                serde_json::json!("Please review"),
            )]),
            depends_on: vec![],
            retry_count: 0,
            timeout: None,
            is_terminal: false,
        },
        NodeDef {
            id: "after".to_string(),
            node_type: "llm".to_string(),
            config: HashMap::from([(
                "prompt".to_string(),
                serde_json::json!("post-review"),
            )]),
            depends_on: vec!["review".to_string()],
            retry_count: 0,
            timeout: None,
            is_terminal: false,
        },
    ];
    engine.register_workflow(make_workflow("resume_chain", nodes)).unwrap();

    let execution = engine
        .start_execution("resume_chain", HashMap::new())
        .await
        .unwrap();
    assert_eq!(execution.state, ExecutionState::Waiting);
    // Before resume: only `review` ran (and is Waiting). `after` must not have
    // produced output yet because the scheduler bailed out at the Waiting node.
    let id = execution.id.clone();
    let mut review = HashMap::new();
    review.insert("approved".to_string(), serde_json::json!(true));

    let resumed = engine.resume_execution(&id, review).await.unwrap();
    assert_eq!(resumed.state, ExecutionState::Completed);

    // `after` must have run during resume and its output must be present.
    let after = resumed.node_results.get("after").expect("downstream `after` should have run");
    assert_eq!(after.state, ExecutionState::Completed);
    assert!(
        after.output.get("text").is_some(),
        "downstream node output should be populated by mock LLM executor"
    );

    // And the previously-waiting `review` is now Completed.
    let review_state = resumed
        .node_results
        .get("review")
        .map(|r| r.state)
        .expect("review node result should exist");
    assert_eq!(review_state, ExecutionState::Completed);
}

// -----------------------------------------------------------------------
// list_executions tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_list_executions_all() {
    let engine = WorkflowEngine::new();
    engine
        .register_workflow(make_workflow("wf1", vec![make_node("n1", "llm", vec![])]))
        .unwrap();
    engine
        .register_workflow(make_workflow("wf2", vec![make_node("n1", "llm", vec![])]))
        .unwrap();

    engine.start_execution("wf1", HashMap::new()).await.unwrap();
    engine.start_execution("wf2", HashMap::new()).await.unwrap();

    let all = engine.list_executions(None).await;
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn test_list_executions_filtered() {
    let engine = WorkflowEngine::new();
    engine
        .register_workflow(make_workflow("wf1", vec![make_node("n1", "llm", vec![])]))
        .unwrap();
    engine
        .register_workflow(make_workflow("wf2", vec![make_node("n1", "llm", vec![])]))
        .unwrap();

    engine.start_execution("wf1", HashMap::new()).await.unwrap();
    engine.start_execution("wf2", HashMap::new()).await.unwrap();
    engine.start_execution("wf1", HashMap::new()).await.unwrap();

    let filtered = engine.list_executions(Some("wf1")).await;
    assert_eq!(filtered.len(), 2);

    let filtered2 = engine.list_executions(Some("wf2")).await;
    assert_eq!(filtered2.len(), 1);
}

#[tokio::test]
async fn test_list_executions_empty() {
    let engine = WorkflowEngine::new();
    let all = engine.list_executions(None).await;
    assert!(all.is_empty());
}

// -----------------------------------------------------------------------
// close tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_close_engine() {
    let engine = WorkflowEngine::new();
    assert!(!engine.is_closed().await);

    engine.close().await;
    assert!(engine.is_closed().await);

    // Running after close should fail
    engine
        .register_workflow(make_workflow("wf", vec![make_node("n1", "llm", vec![])]))
        .unwrap();
    let result = engine.run("wf", HashMap::new(), None).await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), EngineError::InvalidState(_)));
}

// -----------------------------------------------------------------------
// persistence tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_persistence_save_and_load() {
    let dir = tempfile::tempdir().unwrap();
    let engine = WorkflowEngine::with_persistence(dir.path().to_path_buf());
    engine
        .register_workflow(make_workflow("persist_wf", vec![make_node("n1", "llm", vec![])]))
        .unwrap();

    let execution = engine
        .start_execution("persist_wf", HashMap::new())
        .await
        .unwrap();
    let id = execution.id.clone();

    // Execution should be found in memory
    let found = engine.get_execution(&id).await;
    assert!(found.is_some());
    assert_eq!(found.unwrap().workflow_name, "persist_wf");
}

#[tokio::test]
async fn test_get_execution_loads_from_disk() {
    let dir = tempfile::tempdir().unwrap();
    let engine = WorkflowEngine::with_persistence(dir.path().to_path_buf());
    engine
        .register_workflow(make_workflow("disk_wf", vec![make_node("n1", "llm", vec![])]))
        .unwrap();

    let execution = engine
        .start_execution("disk_wf", HashMap::new())
        .await
        .unwrap();
    let id = execution.id.clone();

    // Create a new engine instance with the same persistence dir
    let engine2 = WorkflowEngine::with_persistence(dir.path().to_path_buf());
    // The execution should be loadable from disk
    let loaded = engine2.get_execution(&id).await;
    assert!(loaded.is_some());
    assert_eq!(loaded.unwrap().id, id);
}

#[tokio::test]
async fn test_register_invalid_workflow_no_nodes() {
    let engine = WorkflowEngine::new();
    let wf = Workflow {
        name: "invalid".to_string(),
        description: String::new(),
        version: "1.0.0".to_string(),
        triggers: vec![],
        nodes: vec![],
        edges: vec![],
        variables: HashMap::new(),
        metadata: HashMap::new(),
    };
    let result = engine.register_workflow(wf);
    assert!(result.is_err());
}

#[tokio::test]
async fn test_register_invalid_workflow_no_name() {
    let engine = WorkflowEngine::new();
    let wf = Workflow {
        name: String::new(),
        description: String::new(),
        version: "1.0.0".to_string(),
        triggers: vec![],
        nodes: vec![make_node("n1", "llm", vec![])],
        edges: vec![],
        variables: HashMap::new(),
        metadata: HashMap::new(),
    };
    let result = engine.register_workflow(wf);
    assert!(result.is_err());
}

#[tokio::test]
async fn test_replace_workflow() {
    let engine = WorkflowEngine::new();
    engine
        .register_workflow(make_workflow("wf", vec![make_node("n1", "llm", vec![])]))
        .unwrap();

    // Re-register with same name but different node type
    engine
        .register_workflow(make_workflow("wf", vec![make_node("n1", "tool", vec![])]))
        .unwrap();

    let wf = engine.get_workflow("wf").unwrap();
    assert_eq!(wf.nodes[0].node_type, "tool");
}

#[tokio::test]
async fn test_start_execution_workflow_not_found() {
    let engine = WorkflowEngine::new();
    let result = engine.start_execution("nonexistent", HashMap::new()).await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), EngineError::WorkflowNotFound(_)));
}

#[tokio::test]
async fn test_start_execution_unknown_node_type() {
    let engine = WorkflowEngine::new();
    let nodes = vec![NodeDef {
        id: "n1".to_string(),
        node_type: "nonexistent_type".to_string(),
        config: HashMap::new(),
        depends_on: vec![],
        retry_count: 0,
        timeout: None,
    is_terminal: false,
    }];
    engine
        .register_workflow(make_workflow("bad_type_wf", nodes))
        .unwrap();

    let result = engine.start_execution("bad_type_wf", HashMap::new()).await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), EngineError::UnknownNodeType(_)));
}

#[tokio::test]
async fn test_start_execution_with_cycle() {
    let engine = WorkflowEngine::new();
    let nodes = vec![
        make_node("a", "llm", vec!["b"]),
        make_node("b", "llm", vec!["a"]),
    ];
    let result = engine.register_workflow(make_workflow("cycle_wf", nodes));
    // Cycle is detected at registration time, not execution time
    assert!(result.is_err());
}

#[tokio::test]
async fn test_engine_close_prevents_new_runs() {
    let engine = WorkflowEngine::new();
    engine
        .register_workflow(make_workflow("wf", vec![make_node("n1", "llm", vec![])]))
        .unwrap();

    engine.close().await;
    assert!(engine.is_closed().await);

    let result = engine.start_execution("wf", HashMap::new()).await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), EngineError::InvalidState(_)));
}

#[tokio::test]
async fn test_engine_default() {
    let engine = WorkflowEngine::default();
    assert!(!engine.is_closed().await);
    assert!(engine.list_workflows().is_empty());
}

#[tokio::test]
async fn test_engine_new_arc() {
    let engine = WorkflowEngine::new_arc();
    assert!(!engine.is_closed().await);
}

#[tokio::test]
async fn test_engine_with_executors() {
    let registry = NodeExecutorRegistry::new();
    let engine = WorkflowEngine::with_executors(registry);
    assert!(!engine.is_closed().await);
}

#[tokio::test]
async fn test_execution_has_timestamps() {
    let engine = WorkflowEngine::new();
    engine
        .register_workflow(make_workflow("wf", vec![make_node("n1", "llm", vec![])]))
        .unwrap();

    let execution = engine.start_execution("wf", HashMap::new()).await.unwrap();
    assert!(execution.ended_at.is_some());
    assert!(execution.ended_at.unwrap() >= execution.started_at);
}

#[tokio::test]
async fn test_execution_input_preserved() {
    let engine = WorkflowEngine::new();
    engine
        .register_workflow(make_workflow("wf", vec![make_node("n1", "llm", vec![])]))
        .unwrap();

    let mut input = HashMap::new();
    input.insert("query".to_string(), serde_json::json!("test query"));
    let execution = engine.start_execution("wf", input).await.unwrap();
    assert_eq!(execution.input.get("query").unwrap(), "test query");
}

#[tokio::test]
async fn test_list_executions_after_close() {
    let engine = WorkflowEngine::new();
    engine
        .register_workflow(make_workflow("wf", vec![make_node("n1", "llm", vec![])]))
        .unwrap();

    engine.start_execution("wf", HashMap::new()).await.unwrap();
    engine.close().await;

    // Should still be able to list executions after close
    let all = engine.list_executions(None).await;
    assert_eq!(all.len(), 1);
}

#[tokio::test]
async fn test_with_persistence_arc() {
    let dir = tempfile::tempdir().unwrap();
    let engine = WorkflowEngine::with_persistence_arc(dir.path().to_path_buf());
    engine
        .register_workflow(make_workflow("wf", vec![make_node("n1", "llm", vec![])]))
        .unwrap();

    let execution = engine.start_execution("wf", HashMap::new()).await.unwrap();
    assert_eq!(execution.state, ExecutionState::Completed);
}

#[tokio::test]
async fn test_engine_error_display() {
    let err = EngineError::WorkflowNotFound("test_wf".to_string());
    assert!(err.to_string().contains("test_wf"));

    let err = EngineError::CycleDetected("circular".to_string());
    assert!(err.to_string().contains("circular"));

    let err = EngineError::AlreadyCompleted("exec_id".to_string());
    assert!(err.to_string().contains("exec_id"));
}

#[tokio::test]
async fn test_get_execution_or_err_found() {
    let engine = WorkflowEngine::new();
    engine
        .register_workflow(make_workflow("wf", vec![make_node("n1", "llm", vec![])]))
        .unwrap();

    let execution = engine.start_execution("wf", HashMap::new()).await.unwrap();
    let found = engine.get_execution_or_err(&execution.id).await;
    assert!(found.is_ok());
}

#[tokio::test]
async fn test_with_executors_and_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let registry = NodeExecutorRegistry::new();
    let engine = WorkflowEngine::with_executors_and_persistence(
        registry,
        dir.path().to_path_buf(),
    );
    engine
        .register_workflow(make_workflow("wf", vec![make_node("n1", "llm", vec![])]))
        .unwrap();

    let execution = engine.start_execution("wf", HashMap::new()).await.unwrap();
    assert_eq!(execution.state, ExecutionState::Completed);
}

// ---------------------------------------------------------------------------
// Cancellation integration tests (1a-A2)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_cancel_returns_cancelled_state() {
    use std::sync::Arc;
    use std::time::Duration;

    let engine = Arc::new(WorkflowEngine::new());
    let mut node = make_node("n1", "delay", vec![]);
    // DelayNodeExecutor treats `seconds` as milliseconds (legacy naming).
    node.config
        .insert("seconds".to_string(), serde_json::json!(10_000u64));
    engine
        .register_workflow(make_workflow("long_wf", vec![node]))
        .unwrap();

    let engine_for_run = engine.clone();
    let run_handle = tokio::spawn(async move {
        let mut input = HashMap::new();
        engine_for_run
            .run("long_wf", input.drain().collect(), None)
            .await
            .unwrap()
    });

    // Wait for the execution to start.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let executions = engine.list_executions(None).await;
    assert_eq!(executions.len(), 1, "expected one in-flight execution");
    let id = executions[0].id.clone();
    assert_eq!(executions[0].state, ExecutionState::Running);

    let cancelled = engine.cancel_execution(&id).await.unwrap();
    assert_eq!(cancelled.state, ExecutionState::Cancelled);

    // run() future should resolve quickly after cancel.
    let join_result = tokio::time::timeout(Duration::from_secs(3), run_handle)
        .await
        .expect("run did not resolve within 3s of cancel");
    let execution = join_result.unwrap();
    assert_eq!(
        execution.state,
        ExecutionState::Cancelled,
        "run() should return Cancelled state after cancellation"
    );

    // Token should be cleaned up.
    assert!(
        engine.cancel_tokens.get(&id).is_none(),
        "cancel token should be removed after run() completes"
    );
}

#[tokio::test]
async fn test_close_cancels_all_in_flight() {
    use std::sync::Arc;
    use std::time::Duration;

    let engine = Arc::new(WorkflowEngine::new());
    let mut node = make_node("n1", "delay", vec![]);
    node.config
        .insert("seconds".to_string(), serde_json::json!(10_000u64));
    engine
        .register_workflow(make_workflow("long_wf", vec![node]))
        .unwrap();

    let engine_for_run = engine.clone();
    let run_handle = tokio::spawn(async move {
        let mut input = HashMap::new();
        engine_for_run
            .run("long_wf", input.drain().collect(), None)
            .await
            .unwrap()
    });

    tokio::time::sleep(Duration::from_millis(300)).await;
    let id = engine.list_executions(None).await[0].id.clone();

    engine.clone().close().await;

    let join_result = tokio::time::timeout(Duration::from_secs(3), run_handle)
        .await
        .expect("run did not resolve within 3s of close");
    let outcome = join_result.unwrap();
    assert_eq!(outcome.state, ExecutionState::Cancelled);
    assert!(engine.cancel_tokens.get(&id).is_none());
}

// ---------------------------------------------------------------------------
// Dual-mode entry points: run_blocking + start_async (1a-C1)
// ---------------------------------------------------------------------------

#[test]
fn test_run_blocking_completes() {
    // run_blocking creates a current-thread runtime and blocks until done.
    // Verifies the synchronous entry point can execute a simple workflow
    // without an externally provided tokio runtime.
    let engine = WorkflowEngine::new();
    let nodes = vec![
        make_node("n1", "llm", vec![]),
        make_node("n2", "tool", vec!["n1"]),
    ];
    engine
        .register_workflow(make_workflow("blocking_wf", nodes))
        .unwrap();

    let execution = engine.run_blocking("blocking_wf", HashMap::new(), None).unwrap();

    assert_eq!(execution.state, ExecutionState::Completed);
    assert_eq!(execution.node_results.len(), 2);
    assert!(execution.node_results.contains_key("n1"));
    assert!(execution.node_results.contains_key("n2"));
    assert!(execution.ended_at.is_some());
}

#[test]
fn test_run_blocking_unknown_workflow() {
    // Synchronous entry point surfaces WorkflowNotFound synchronously
    // rather than panicking or hanging.
    let engine = WorkflowEngine::new();
    let result = engine.run_blocking("does_not_exist", HashMap::new(), None);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, EngineError::WorkflowNotFound(_)));
}

#[tokio::test]
async fn test_start_async_returns_id_quickly() {
    // start_async spawns a background task and returns an execution ID
    // without waiting for the workflow to complete. We verify the ID is
    // well-formed and that an execution record exists for it.
    use std::sync::Arc;
    use std::time::Duration;

    let engine = Arc::new(WorkflowEngine::new_arc());
    let nodes = vec![
        make_node("n1", "llm", vec![]),
        make_node("n2", "tool", vec!["n1"]),
    ];
    engine
        .register_workflow(make_workflow("async_wf", nodes))
        .unwrap();

    let start = std::time::Instant::now();
    let execution_id = WorkflowEngine::start_async(
        Arc::clone(&engine),
        "async_wf",
        HashMap::new(),
        None,
    )
    .await
    .expect("start_async should return execution id");
    let elapsed = start.elapsed();

    // ID format check (UUID v4: 8-4-4-4-12)
    let parts: Vec<&str> = execution_id.split('-').collect();
    assert_eq!(parts.len(), 5);
    assert_eq!(parts[0].len(), 8);

    // Should return well before nodes complete under any reasonable load.
    // The mock llm+tool executors are sub-millisecond, but allow generous
    // headroom for slow CI machines.
    assert!(
        elapsed < Duration::from_millis(500),
        "start_async took too long: {:?}",
        elapsed
    );

    // Execution record must exist immediately after start_async returns.
    let execution = engine
        .get_execution(&execution_id)
        .await
        .expect("execution should exist after start_async");
    assert_eq!(execution.id, execution_id);
    assert_eq!(execution.workflow_name, "async_wf");
}

#[tokio::test]
async fn test_start_async_unknown_workflow() {
    // start_async surfaces WorkflowNotFound synchronously (without spawning).
    use std::sync::Arc;

    let engine = Arc::new(WorkflowEngine::new_arc());
    let result = WorkflowEngine::start_async(Arc::clone(&engine), "nope", HashMap::new(), None).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, EngineError::WorkflowNotFound(_)));
    // No execution record should have been minted.
    assert!(engine.list_executions(None).await.is_empty());
}

#[tokio::test]
async fn test_start_async_eventually_completes() {
    // Polls get_execution until the background task reaches a terminal state,
    // verifying that start_async actually drives the workflow to completion.
    use std::sync::Arc;
    use std::time::Duration;

    let engine = Arc::new(WorkflowEngine::new_arc());
    let nodes = vec![
        make_node("n1", "llm", vec![]),
        make_node("n2", "tool", vec!["n1"]),
    ];
    engine
        .register_workflow(make_workflow("poll_wf", nodes))
        .unwrap();

    let execution_id = WorkflowEngine::start_async(
        Arc::clone(&engine),
        "poll_wf",
        HashMap::new(),
        None,
    )
    .await
    .unwrap();

    // Poll up to 2 seconds for completion.
    let mut final_state: Option<ExecutionState> = None;
    for _ in 0..200 {
        if let Some(execution) = engine.get_execution(&execution_id).await {
            match execution.state {
                ExecutionState::Completed
                | ExecutionState::Failed
                | ExecutionState::Cancelled => {
                    final_state = Some(execution.state);
                    break;
                }
                _ => {}
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert_eq!(
        final_state,
        Some(ExecutionState::Completed),
        "background execution did not reach Completed within 2s"
    );
}

#[tokio::test]
async fn test_create_execution_then_run_async_separately() {
    // Verifies the two-step internal API: create_execution mints the record,
    // run_async drives it. Useful for callers that need the ID before the
    // workflow starts (e.g., to register a progress channel).
    let engine = WorkflowEngine::new();
    engine
        .register_workflow(make_workflow("two_step_wf", vec![make_node("n1", "llm", vec![])]))
        .unwrap();

    let execution = engine
        .create_execution("two_step_wf", HashMap::new(), None)
        .await
        .unwrap();
    assert_eq!(execution.state, ExecutionState::Running);
    assert!(execution.ended_at.is_none());

    // Execution is queryable before run_async is called.
    let stored = engine.get_execution(&execution.id).await.unwrap();
    assert_eq!(stored.state, ExecutionState::Running);

    let completed = engine.run_async(&execution.id).await.unwrap();
    assert_eq!(completed.state, ExecutionState::Completed);
    assert!(completed.ended_at.is_some());
    assert_eq!(completed.node_results.len(), 1);
}

// ---------------------------------------------------------------------------
// TriggerSource integration (1a-C2)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_trigger_source_cli_recorded_on_execution() {
    // TriggerSource::Cli is stamped onto the execution by run().
    let engine = WorkflowEngine::new();
    engine
        .register_workflow(make_workflow("cli_wf", vec![make_node("n1", "llm", vec![])]))
        .unwrap();

    let execution = engine
        .run("cli_wf", HashMap::new(), Some(TriggerSource::Cli))
        .await
        .unwrap();
    assert_eq!(execution.trigger_source, Some(TriggerSource::Cli));
}

#[tokio::test]
async fn test_trigger_source_cron_recorded_on_execution() {
    let engine = WorkflowEngine::new();
    engine
        .register_workflow(make_workflow("cron_wf", vec![make_node("n1", "llm", vec![])]))
        .unwrap();

    let execution = engine
        .run("cron_wf", HashMap::new(), Some(TriggerSource::Cron))
        .await
        .unwrap();
    assert_eq!(execution.trigger_source, Some(TriggerSource::Cron));
}

#[tokio::test]
async fn test_trigger_source_webhook_recorded_with_payload() {
    // Webhook variant carries its payload through the trigger_source field.
    let engine = WorkflowEngine::new();
    engine
        .register_workflow(make_workflow("webhook_wf", vec![make_node("n1", "llm", vec![])]))
        .unwrap();

    let payload = serde_json::json!({"event": "push", "ref": "main"});
    let trigger = TriggerSource::Webhook {
        payload: payload.clone(),
    };
    let execution = engine
        .run("webhook_wf", HashMap::new(), Some(trigger))
        .await
        .unwrap();

    match execution.trigger_source {
        Some(TriggerSource::Webhook { payload: p }) => assert_eq!(p, payload),
        other => panic!("expected Webhook variant, got {:?}", other),
    }
}

#[tokio::test]
async fn test_trigger_source_agent_tool_carries_recursion_depth() {
    // AgentTool trigger carries tool_call_id + recursion_depth, both preserved
    // through the engine. This is the field 1c reads to enforce
    // MAX_RECURSION_DEPTH.
    let engine = WorkflowEngine::new();
    engine
        .register_workflow(make_workflow("agent_wf", vec![make_node("n1", "llm", vec![])]))
        .unwrap();

    let trigger = TriggerSource::AgentTool {
        tool_call_id: "tc_abc".to_string(),
        recursion_depth: 2,
    };
    let execution = engine
        .run("agent_wf", HashMap::new(), Some(trigger))
        .await
        .unwrap();

    match execution.trigger_source {
        Some(TriggerSource::AgentTool {
            tool_call_id,
            recursion_depth,
        }) => {
            assert_eq!(tool_call_id, "tc_abc");
            assert_eq!(recursion_depth, 2);
        }
        other => panic!("expected AgentTool variant, got {:?}", other),
    }
}

#[tokio::test]
async fn test_trigger_source_none_default() {
    // Passing None leaves trigger_source unset (legacy behavior).
    let engine = WorkflowEngine::new();
    engine
        .register_workflow(make_workflow("plain_wf", vec![make_node("n1", "llm", vec![])]))
        .unwrap();

    let execution = engine
        .run("plain_wf", HashMap::new(), None)
        .await
        .unwrap();
    assert!(execution.trigger_source.is_none());
}

#[tokio::test]
async fn test_trigger_source_preserved_through_start_async() {
    // start_async path also stamps the trigger_source on the initial
    // execution record (visible immediately) and the final state (after
    // background task completes).
    use std::sync::Arc;
    use std::time::Duration;

    let engine = Arc::new(WorkflowEngine::new_arc());
    engine
        .register_workflow(make_workflow("async_trig_wf", vec![make_node("n1", "llm", vec![])]))
        .unwrap();

    let execution_id = WorkflowEngine::start_async(
        Arc::clone(&engine),
        "async_trig_wf",
        HashMap::new(),
        Some(TriggerSource::Cli),
    )
    .await
    .unwrap();

    // Should be visible on the initial record immediately.
    let early = engine.get_execution(&execution_id).await.unwrap();
    assert_eq!(early.trigger_source, Some(TriggerSource::Cli));

    // And on the completed record after the background task finishes.
    let mut final_exec = None;
    for _ in 0..200 {
        if let Some(e) = engine.get_execution(&execution_id).await {
            if matches!(
                e.state,
                ExecutionState::Completed | ExecutionState::Failed | ExecutionState::Cancelled
            ) {
                final_exec = Some(e);
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let final_exec = final_exec.expect("execution should complete within 2s");
    assert_eq!(final_exec.trigger_source, Some(TriggerSource::Cli));
}

#[test]
fn test_trigger_source_via_run_blocking() {
    // run_blocking also propagates trigger_source correctly.
    let engine = WorkflowEngine::new();
    engine
        .register_workflow(make_workflow("blocking_trig_wf", vec![make_node("n1", "llm", vec![])]))
        .unwrap();

    let execution = engine
        .run_blocking(
            "blocking_trig_wf",
            HashMap::new(),
            Some(TriggerSource::Cron),
        )
        .unwrap();
    assert_eq!(execution.trigger_source, Some(TriggerSource::Cron));
}

// ---------------------------------------------------------------------------
// WorkflowEvent observer integration (1a-C3)
// ---------------------------------------------------------------------------

use crate::events::{WorkflowEvent, WorkflowObserver};
use async_trait::async_trait;
use std::sync::Mutex;
use std::time::Duration;

/// Test observer that captures every event into a Vec.
struct RecordingObserver {
    name: String,
    events: Mutex<Vec<WorkflowEvent>>,
}

impl RecordingObserver {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            events: Mutex::new(Vec::new()),
        }
    }

    fn snapshot(&self) -> Vec<WorkflowEvent> {
        self.events.lock().unwrap().clone()
    }

    fn event_kinds(&self) -> Vec<&'static str> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .map(|e| match e {
                WorkflowEvent::Started { .. } => "started",
                WorkflowEvent::NodeStarted { .. } => "node_started",
                WorkflowEvent::NodeCompleted { .. } => "node_completed",
                WorkflowEvent::NodeFailed { .. } => "node_failed",
                WorkflowEvent::Completed { .. } => "completed",
                WorkflowEvent::Failed { .. } => "failed",
                WorkflowEvent::Cancelled { .. } => "cancelled",
            })
            .collect()
    }
}

#[async_trait]
impl WorkflowObserver for RecordingObserver {
    fn name(&self) -> &str {
        &self.name
    }

    async fn on_event(&self, event: WorkflowEvent) {
        let mut events = self.events.lock().unwrap();
        events.push(event);
    }
}

#[tokio::test]
async fn test_events_for_completed_workflow() {
    // A successful workflow should emit Started + Completed.
    let engine = WorkflowEngine::new();
    let recorder = Arc::new(RecordingObserver::new("recorder"));
    engine
        .event_manager()
        .register(Arc::clone(&recorder) as Arc<dyn WorkflowObserver>)
        .await;

    engine
        .register_workflow(make_workflow("ev_wf", vec![make_node("n1", "llm", vec![])]))
        .unwrap();

    let _ = engine.run("ev_wf", HashMap::new(), None).await.unwrap();

    // Emit runs in a spawned task; give it a moment to land.
    tokio::time::sleep(Duration::from_millis(30)).await;

    let kinds = recorder.event_kinds();
    assert_eq!(kinds, vec!["started", "completed"]);
}

#[tokio::test]
async fn test_events_for_cancelled_workflow() {
    // A cancelled workflow should emit Started + Cancelled.
    use std::sync::Arc;
    use std::time::Duration;

    let engine = Arc::new(WorkflowEngine::new());
    let recorder = Arc::new(RecordingObserver::new("recorder"));
    engine
        .event_manager()
        .register(Arc::clone(&recorder) as Arc<dyn WorkflowObserver>)
        .await;

    // Slow node so we can cancel mid-execution.
    let mut node = make_node("n1", "delay", vec![]);
    node.config
        .insert("seconds".to_string(), serde_json::json!(10_000u64));
    engine
        .register_workflow(make_workflow("ev_cancel_wf", vec![node]))
        .unwrap();

    let engine_for_run = engine.clone();
    let run_handle = tokio::spawn(async move {
        engine_for_run
            .run("ev_cancel_wf", HashMap::new(), None)
            .await
            .unwrap()
    });

    tokio::time::sleep(Duration::from_millis(200)).await;
    let id = engine.list_executions(None).await[0].id.clone();
    engine.cancel_execution(&id).await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(3), run_handle)
        .await
        .expect("run resolves within 3s of cancel")
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    let kinds = recorder.event_kinds();
    assert_eq!(kinds, vec!["started", "cancelled"]);
}

#[tokio::test]
async fn test_started_event_carries_trigger_source() {
    // The Started event payload must echo back the trigger_source passed
    // to run().
    let engine = WorkflowEngine::new();
    let recorder = Arc::new(RecordingObserver::new("recorder"));
    engine
        .event_manager()
        .register(Arc::clone(&recorder) as Arc<dyn WorkflowObserver>)
        .await;

    engine
        .register_workflow(make_workflow("ev_trig_wf", vec![make_node("n1", "llm", vec![])]))
        .unwrap();

    let _ = engine
        .run(
            "ev_trig_wf",
            HashMap::new(),
            Some(TriggerSource::Cli),
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;

    let snapshot = recorder.snapshot();
    let started = snapshot
        .iter()
        .find(|e| matches!(e, WorkflowEvent::Started { .. }))
        .expect("Started event should have been emitted");
    match started {
        WorkflowEvent::Started {
            trigger_source, ..
        } => assert_eq!(*trigger_source, Some(TriggerSource::Cli)),
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn test_no_events_without_observers() {
    // When no observers are registered, emit is effectively a no-op and
    // must not error or interfere with execution.
    let engine = WorkflowEngine::new();
    engine
        .register_workflow(make_workflow("ev_no_obs", vec![make_node("n1", "llm", vec![])]))
        .unwrap();
    assert!(!engine.event_manager().has_observers().await);

    let execution = engine.run("ev_no_obs", HashMap::new(), None).await.unwrap();
    assert_eq!(execution.state, ExecutionState::Completed);
}

// ===========================================================================
// load_workflows_from_dir + new_integrated
// ===========================================================================

use std::path::PathBuf;

/// Write a workflow YAML/JSON file under a temp dir and return its path.
fn write_wf_file(dir: &Path, name: &str, kind: &str) -> PathBuf {
    let ext = if kind == "yaml" { "yaml" } else { "json" };
    let path = dir.join(format!("{}.{}", name, ext));
    let body = if kind == "yaml" {
        format!(
            r#"name: {name}
version: "1.0.0"
nodes:
  - id: n1
    node_type: delay
    config:
      seconds: 0
"#
        )
    } else {
        format!(
            r#"{{"name":"{name}","version":"1.0.0","nodes":[{{"id":"n1","node_type":"delay","config":{{"seconds":0}}}}]}}"#
        )
    };
    std::fs::write(&path, body).unwrap();
    path
}

#[tokio::test]
async fn test_load_workflows_from_dir_loads_yaml_and_json() {
    let tmp = tempfile::tempdir().unwrap();
    write_wf_file(tmp.path(), "wf_a", "yaml");
    write_wf_file(tmp.path(), "wf_b", "json");
    // Non-workflow file should be skipped.
    std::fs::write(tmp.path().join("README.md"), "# not a workflow").unwrap();

    let engine = WorkflowEngine::new();
    let count = engine.load_workflows_from_dir(tmp.path()).unwrap();
    assert_eq!(count, 2);
    let names = engine.list_workflows();
    assert!(names.contains(&"wf_a".to_string()));
    assert!(names.contains(&"wf_b".to_string()));
}

#[tokio::test]
async fn test_load_workflows_from_dir_missing_dir_is_ok() {
    // Missing directory returns Ok(0) so gateway startup doesn't fail when
    // users haven't created workflows/ yet.
    let engine = WorkflowEngine::new();
    let bogus = PathBuf::from("/this/path/does/not/exist/zzz");
    let count = engine.load_workflows_from_dir(&bogus).unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn test_load_workflows_from_dir_skips_invalid_files() {
    let tmp = tempfile::tempdir().unwrap();
    // Valid workflow.
    write_wf_file(tmp.path(), "good", "yaml");
    // Invalid YAML - parse should fail and we skip.
    std::fs::write(tmp.path().join("bad.yaml"), "name: bad\n  bad-indent: [").unwrap();

    let engine = WorkflowEngine::new();
    let count = engine.load_workflows_from_dir(tmp.path()).unwrap();
    assert_eq!(count, 1);
    assert!(engine.list_workflows().contains(&"good".to_string()));
}

#[tokio::test]
async fn test_new_integrated_wires_real_llm_and_tool_executors() {
    // The integrated constructor should register real llm/tool executors
    // over the mock defaults. We verify by looking them up and confirming
    // they exist (the real executors' execute path is covered in nodes/tests).
    use async_trait::async_trait;
    use nemesis_providers::failover::FailoverError;
    use nemesis_providers::router::LLMProvider;
    use nemesis_providers::types::{ChatOptions, LLMResponse, Message, ToolDefinition};

    struct NullProvider;
    #[async_trait]
    impl LLMProvider for NullProvider {
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
            "null"
        }
    }

    let tools = Arc::new(nemesis_tools::registry::ToolRegistry::new());
    let engine = WorkflowEngine::new_integrated(Arc::new(NullProvider) as Arc<dyn LLMProvider>, tools, None);

    // Real executors must be registered for "llm" and "tool".
    assert!(engine.node_executors.get("llm").is_some());
    assert!(engine.node_executors.get("tool").is_some());
    assert!(engine.node_executors.get("sub_workflow").is_some());
}

#[tokio::test]
async fn test_list_cron_workflows_returns_cron_triggers() {
    use crate::types::TriggerConfig;
    use serde_json::json;

    let mut wf = make_workflow("cron_wf", vec![make_node("n1", "delay", vec![])]);
    wf.triggers = vec![TriggerConfig {
        trigger_type: "cron".to_string(),
        config: HashMap::from([
            ("schedule".to_string(), json!("*/5 * * * *")),
            (
                "input".to_string(),
                json!({"topic": "news", "limit": 10}),
            ),
        ]),
    }];

    let engine = WorkflowEngine::new();
    engine.register_workflow(wf).unwrap();

    let crons = engine.list_cron_workflows();
    assert_eq!(crons.len(), 1);
    let (name, schedule, timezone, input) = &crons[0];
    assert_eq!(name, "cron_wf");
    assert_eq!(schedule, "*/5 * * * *");
    assert_eq!(timezone, &crate::triggers::CronTimezone::Local);
    assert_eq!(input.get("topic").unwrap(), &json!("news"));
    assert_eq!(input.get("limit").unwrap(), &json!(10));
}

#[tokio::test]
async fn test_list_cron_workflows_skips_non_cron_and_missing_schedule() {
    use crate::types::TriggerConfig;
    use serde_json::json;

    // Webhook trigger - should be skipped.
    let mut wf_a = make_workflow("hook_wf", vec![make_node("n1", "delay", vec![])]);
    wf_a.triggers = vec![TriggerConfig {
        trigger_type: "webhook".to_string(),
        config: HashMap::new(),
    }];

    // Cron trigger missing schedule - should be skipped with warning.
    let mut wf_b = make_workflow("bad_cron", vec![make_node("n1", "delay", vec![])]);
    wf_b.triggers = vec![TriggerConfig {
        trigger_type: "cron".to_string(),
        config: HashMap::from([("input".to_string(), json!({}))]),
    }];

    // Cron trigger with schedule - should be returned.
    let mut wf_c = make_workflow("good_cron", vec![make_node("n1", "delay", vec![])]);
    wf_c.triggers = vec![TriggerConfig {
        trigger_type: "cron".to_string(),
        config: HashMap::from([("schedule".to_string(), json!("0 0 * * *"))]),
    }];

    let engine = WorkflowEngine::new();
    engine.register_workflow(wf_a).unwrap();
    engine.register_workflow(wf_b).unwrap();
    engine.register_workflow(wf_c).unwrap();

    let crons = engine.list_cron_workflows();
    assert_eq!(crons.len(), 1);
    assert_eq!(crons[0].0, "good_cron");
    assert_eq!(crons[0].1, "0 0 * * *");
}

#[tokio::test]
async fn test_list_cron_workflows_empty_when_no_triggers() {
    let engine = WorkflowEngine::new();
    engine
        .register_workflow(make_workflow("no_trigger", vec![make_node("n1", "delay", vec![])]))
        .unwrap();
    let crons = engine.list_cron_workflows();
    assert!(crons.is_empty());
}

#[tokio::test]
async fn test_spawn_cron_triggers_handles_invalid_expression() {
    // Invalid cron should be logged and skipped, not panic. The returned
    // JoinHandle list excludes the failed entry.
    use crate::types::TriggerConfig;
    use serde_json::json;

    let mut wf_bad = make_workflow("bad_expr", vec![make_node("n1", "delay", vec![])]);
    wf_bad.triggers = vec![TriggerConfig {
        trigger_type: "cron".to_string(),
        config: HashMap::from([("schedule".to_string(), json!("not-a-cron"))]),
    }];

    let engine = WorkflowEngine::new_arc();
    engine.register_workflow(wf_bad).unwrap();

    let handles = engine.spawn_cron_triggers();
    assert!(handles.is_empty(), "invalid cron should be skipped");
}

#[tokio::test]
async fn test_list_cron_workflows_defaults_to_local_timezone() {
    use crate::types::TriggerConfig;
    use serde_json::json;

    let mut wf = make_workflow("tz_default", vec![make_node("n1", "delay", vec![])]);
    wf.triggers = vec![TriggerConfig {
        trigger_type: "cron".to_string(),
        config: HashMap::from([("schedule".to_string(), json!("0 9 * * *"))]),
    }];

    let engine = WorkflowEngine::new();
    engine.register_workflow(wf).unwrap();

    let crons = engine.list_cron_workflows();
    assert_eq!(crons.len(), 1);
    assert_eq!(crons[0].2, CronTimezone::Local, "default should be local");
}

#[tokio::test]
async fn test_list_cron_workflows_respects_utc_timezone() {
    use crate::types::TriggerConfig;
    use serde_json::json;

    let mut wf = make_workflow("tz_utc", vec![make_node("n1", "delay", vec![])]);
    wf.triggers = vec![TriggerConfig {
        trigger_type: "cron".to_string(),
        config: HashMap::from([
            ("schedule".to_string(), json!("0 9 * * *")),
            ("timezone".to_string(), json!("utc")),
        ]),
    }];

    let engine = WorkflowEngine::new();
    engine.register_workflow(wf).unwrap();

    let crons = engine.list_cron_workflows();
    assert_eq!(crons.len(), 1);
    assert_eq!(crons[0].2, CronTimezone::Utc);
}

#[tokio::test]
async fn test_list_cron_workflows_unknown_timezone_falls_back_to_local() {
    use crate::types::TriggerConfig;
    use serde_json::json;

    let mut wf = make_workflow("tz_unknown", vec![make_node("n1", "delay", vec![])]);
    wf.triggers = vec![TriggerConfig {
        trigger_type: "cron".to_string(),
        config: HashMap::from([
            ("schedule".to_string(), json!("0 9 * * *")),
            ("timezone".to_string(), json!("Mars/Olympus")),
        ]),
    }];

    let engine = WorkflowEngine::new();
    engine.register_workflow(wf).unwrap();

    let crons = engine.list_cron_workflows();
    assert_eq!(crons.len(), 1);
    assert_eq!(crons[0].2, CronTimezone::Local);
}

#[test]
fn cron_timezone_parses_known_strings() {
    assert_eq!(CronTimezone::from_config_str("local"), Some(CronTimezone::Local));
    assert_eq!(CronTimezone::from_config_str("LOCAL"), Some(CronTimezone::Local));
    assert_eq!(CronTimezone::from_config_str("utc"), Some(CronTimezone::Utc));
    assert_eq!(CronTimezone::from_config_str("UTC"), Some(CronTimezone::Utc));
    assert_eq!(CronTimezone::from_config_str("  utc  "), Some(CronTimezone::Utc));
    assert_eq!(CronTimezone::from_config_str("Mars"), None);
    assert_eq!(CronTimezone::from_config_str(""), None);
}

// ---------------------------------------------------------------------------
// Auto-checkpoint tests (1b-A1 step 6)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn auto_checkpoint_saves_after_each_level() {
    // Two-level DAG: a (root) → b (downstream). When a checkpoint store is
    // wired, the engine must save at least one checkpoint as the workflow
    // progresses — even if it ultimately completes.
    let store: Arc<dyn CheckpointStore> = Arc::new(InMemoryCheckpointStore::new());

    let engine = WorkflowEngine::new_arc();
    engine.set_checkpoint_store(store.clone());
    engine
        .register_workflow(make_workflow(
            "two_level",
            vec![
                make_node("a", "llm", vec![]),
                make_node("b", "llm", vec!["a"]),
            ],
        ))
        .unwrap();

    let exec = engine.run("two_level", HashMap::new(), None).await.unwrap();
    assert_eq!(exec.state, ExecutionState::Completed);

    // At least one checkpoint should exist for this execution.
    let list = store.list(&exec.id).await.unwrap();
    assert!(
        !list.is_empty(),
        "expected at least one checkpoint after run; got 0"
    );

    // The latest checkpoint should reflect the full run (both nodes completed).
    let latest = store.latest(&exec.id).await.unwrap().unwrap();
    assert!(
        latest.completed_nodes.contains("a"),
        "checkpoint should include completed node a"
    );
    assert!(
        latest.completed_nodes.contains("b"),
        "checkpoint should include completed node b"
    );
    assert_eq!(latest.workflow_hash, exec.workflow_hash.unwrap());
}

#[tokio::test]
async fn auto_checkpoint_captures_waiting_node() {
    // A human_review node pauses execution. The checkpoint should record
    // `waiting_node: Some("review")` so the resume path knows where to pick up.
    let store: Arc<dyn CheckpointStore> = Arc::new(InMemoryCheckpointStore::new());

    let engine = WorkflowEngine::new_arc();
    engine.set_checkpoint_store(store.clone());

    let nodes = vec![NodeDef {
        id: "review".to_string(),
        node_type: "human_review".to_string(),
        config: HashMap::new(),
        depends_on: vec![],
        retry_count: 0,
        timeout: None,
        is_terminal: false,
    }];
    engine.register_workflow(make_workflow("hr_wf", nodes)).unwrap();

    let exec = engine.run("hr_wf", HashMap::new(), None).await.unwrap();
    assert_eq!(exec.state, ExecutionState::Waiting);

    let latest = store.latest(&exec.id).await.unwrap().unwrap();
    assert_eq!(
        latest.waiting_node.as_deref(),
        Some("review"),
        "checkpoint should record the waiting node id"
    );
}

#[tokio::test]
async fn restore_incomplete_executions_revives_waiting_workflow() {
    // Simulate a crash by:
    //   1. Engine A runs a human_review workflow → checkpoint saved.
    //   2. Drop engine A.
    //   3. Engine B boots with the *same* checkpoint store.
    //   4. restore_incomplete_executions() should bring the Waiting
    //      execution back so resume_execution() can be called.
    let store: Arc<dyn CheckpointStore> = Arc::new(InMemoryCheckpointStore::new());

    let nodes = vec![NodeDef {
        id: "review".to_string(),
        node_type: "human_review".to_string(),
        config: HashMap::new(),
        depends_on: vec![],
        retry_count: 0,
        timeout: None,
        is_terminal: false,
    }];
    let wf = make_workflow("hr_wf", nodes);

    // First lifecycle: run the workflow, persist a Waiting checkpoint.
    let engine_a = WorkflowEngine::new_arc();
    engine_a.set_checkpoint_store(store.clone());
    engine_a.register_workflow(wf.clone()).unwrap();
    let exec_a = engine_a.run("hr_wf", HashMap::new(), None).await.unwrap();
    assert_eq!(exec_a.state, ExecutionState::Waiting);
    let exec_id = exec_a.id.clone();
    drop(engine_a);

    // Second lifecycle: same store, fresh engine. Restore.
    let engine_b = WorkflowEngine::new_arc();
    engine_b.set_checkpoint_store(store.clone());
    engine_b.register_workflow(wf).unwrap();
    let restored = engine_b.restore_incomplete_executions().await.unwrap();
    assert_eq!(restored, 1, "expected one execution to be restored");

    let revived = engine_b.get_execution(&exec_id).await.unwrap();
    assert_eq!(revived.state, ExecutionState::Waiting);

    // Resume should now work and drive the workflow to completion.
    let resumed = engine_b
        .resume_execution(&exec_id, HashMap::from([("approved".to_string(), serde_json::json!(true))]))
        .await
        .unwrap();
    assert_eq!(resumed.state, ExecutionState::Completed);
}

#[tokio::test]
async fn restore_skips_executions_with_config_drift() {
    // If the workflow definition changed between crash and restart, the
    // hash check should refuse to restore the checkpoint.
    let store: Arc<dyn CheckpointStore> = Arc::new(InMemoryCheckpointStore::new());

    let nodes_v1 = vec![NodeDef {
        id: "review".to_string(),
        node_type: "human_review".to_string(),
        config: HashMap::new(),
        depends_on: vec![],
        retry_count: 0,
        timeout: None,
        is_terminal: false,
    }];
    let wf_v1 = make_workflow("hr_wf", nodes_v1);

    let engine_a = WorkflowEngine::new_arc();
    engine_a.set_checkpoint_store(store.clone());
    engine_a.register_workflow(wf_v1).unwrap();
    let _ = engine_a.run("hr_wf", HashMap::new(), None).await.unwrap();
    drop(engine_a);

    // New workflow definition with an extra node — different hash.
    let nodes_v2 = vec![
        NodeDef {
            id: "review".to_string(),
            node_type: "human_review".to_string(),
            config: HashMap::new(),
            depends_on: vec![],
            retry_count: 0,
            timeout: None,
            is_terminal: false,
        },
        make_node("after", "llm", vec!["review"]),
    ];
    let wf_v2 = make_workflow("hr_wf", nodes_v2);

    let engine_b = WorkflowEngine::new_arc();
    engine_b.set_checkpoint_store(store.clone());
    engine_b.register_workflow(wf_v2).unwrap();
    let restored = engine_b.restore_incomplete_executions().await.unwrap();
    assert_eq!(restored, 0, "config drift should prevent restore");
}

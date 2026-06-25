use super::*;
use std::collections::HashMap;

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

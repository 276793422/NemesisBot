use super::*;

#[test]
fn test_subagent_manager_spawn() {
    let manager = SubagentManager::new();
    let id = manager.spawn(
        "Test task".to_string(),
        "test".to_string(),
        String::new(),
        "web".to_string(),
        "chat-1".to_string(),
        None,
    );
    assert!(id.starts_with("subagent-"));

    let task = manager.get_task(&id).unwrap();
    assert_eq!(task.status, "running");
    assert_eq!(task.task, "Test task");
}

#[test]
fn test_subagent_manager_list_tasks() {
    let manager = SubagentManager::new();
    manager.spawn(
        "Task 1".to_string(),
        "a".to_string(),
        String::new(),
        "web".to_string(),
        "c".to_string(),
        None,
    );
    manager.spawn(
        "Task 2".to_string(),
        "b".to_string(),
        String::new(),
        "web".to_string(),
        "c".to_string(),
        None,
    );

    let tasks = manager.list_tasks();
    assert_eq!(tasks.len(), 2);
}

#[test]
fn test_subagent_manager_update() {
    let manager = SubagentManager::new();
    let id = manager.spawn(
        "Task".to_string(),
        "test".to_string(),
        String::new(),
        "web".to_string(),
        "c".to_string(),
        None,
    );

    manager.update_task(&id, "completed", "Done!");
    let task = manager.get_task(&id).unwrap();
    assert_eq!(task.status, "completed");
    assert_eq!(task.result, "Done!");
}

#[test]
fn test_subagent_manager_remove() {
    let manager = SubagentManager::new();
    let id = manager.spawn(
        "Task".to_string(),
        "test".to_string(),
        String::new(),
        "web".to_string(),
        "c".to_string(),
        None,
    );

    assert!(manager.remove_task(&id));
    assert!(manager.get_task(&id).is_none());
}

#[tokio::test]
async fn test_subagent_tool_execute() {
    let manager = Arc::new(SubagentManager::new());
    let tool = SubagentTool::new(manager);

    let result = tool
        .execute(&serde_json::json!({"task": "Say hello", "label": "greeting"}))
        .await;
    assert!(!result.is_error);
    assert!(result.for_llm.contains("Subagent task completed"));
    assert!(result.for_llm.contains("greeting"));
}

#[tokio::test]
async fn test_subagent_tool_missing_task() {
    let manager = Arc::new(SubagentManager::new());
    let tool = SubagentTool::new(manager);

    let result = tool.execute(&serde_json::json!({})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("task"));
}

#[test]
fn test_subagent_tool_metadata() {
    let manager = Arc::new(SubagentManager::new());
    let tool = SubagentTool::new(manager);
    assert_eq!(tool.name(), "subagent");
    assert!(!tool.description().is_empty());
}

#[test]
fn test_subagent_tool_contextual() {
    let manager = Arc::new(SubagentManager::new());
    let mut tool = SubagentTool::new(manager);
    assert_eq!(tool.origin_channel, "cli");
    let ctx = crate::registry::ToolExecutionContext {
        channel: "rpc".to_string(),
        chat_id: "chat-123".to_string(),
        ..Default::default()
    };
    ContextualTool::set_context(&mut tool, &ctx);
    assert_eq!(tool.origin_channel, "rpc");
    assert_eq!(tool.origin_chat_id, "chat-123");
}

// ---- New tests ----

#[test]
fn test_subagent_manager_get_nonexistent() {
    let manager = SubagentManager::new();
    assert!(manager.get_task("nonexistent").is_none());
}

#[test]
fn test_subagent_manager_remove_nonexistent() {
    let manager = SubagentManager::new();
    assert!(!manager.remove_task("nonexistent"));
}

#[test]
fn test_subagent_manager_spawn_multiple() {
    let manager = SubagentManager::new();
    let mut ids = Vec::new();
    for i in 0..10 {
        let id = manager.spawn(
            format!("Task {}", i),
            "test".to_string(),
            String::new(),
            "web".to_string(),
            format!("chat-{}", i),
            None,
        );
        ids.push(id);
    }
    assert_eq!(manager.list_tasks().len(), 10);
    // All IDs should be unique
    let unique: std::collections::HashSet<_> = ids.into_iter().collect();
    assert_eq!(unique.len(), 10);
}

#[test]
fn test_subagent_manager_update_status_variants() {
    let manager = SubagentManager::new();
    let id = manager.spawn(
        "Task".to_string(),
        "test".to_string(),
        String::new(),
        "web".to_string(),
        "c".to_string(),
        None,
    );

    for status in &["running", "completed", "failed", "cancelled"] {
        manager.update_task(&id, status, &format!("result for {}", status));
        let task = manager.get_task(&id).unwrap();
        assert_eq!(task.status, *status);
    }
}

#[tokio::test]
async fn test_subagent_tool_with_label() {
    let manager = Arc::new(SubagentManager::new());
    let tool = SubagentTool::new(manager);

    let result = tool
        .execute(&serde_json::json!({"task": "Do something", "label": "my-label"}))
        .await;
    assert!(!result.is_error);
}

#[test]
fn test_subagent_task_fields() {
    let manager = SubagentManager::new();
    let id = manager.spawn(
        "Test task description".to_string(),
        "test-label".to_string(),
        "agent-1".to_string(),
        "web".to_string(),
        "chat-abc".to_string(),
        None,
    );
    let task = manager.get_task(&id).unwrap();
    assert_eq!(task.task, "Test task description");
    assert_eq!(task.label, "test-label");
    assert_eq!(task.agent_id, "agent-1");
    assert_eq!(task.origin_channel, "web");
    assert_eq!(task.origin_chat_id, "chat-abc");
}

// ============================================================
// Tests for real LLM execution via run_tool_loop
// ============================================================

#[test]
fn test_subagent_manager_has_llm_callback() {
    let manager = SubagentManager::new();
    assert!(!manager.has_llm_callback(), "Should start without callback");

    manager.set_llm_callback(Arc::new(|_msgs| crate::toolloop::LLMResponse {
        content: "test".to_string(),
        tool_calls: vec![],
    }));
    assert!(
        manager.has_llm_callback(),
        "Should have callback after setting"
    );
}

#[tokio::test]
async fn test_subagent_tool_with_llm_callback() {
    let manager = Arc::new(SubagentManager::new());

    // Set up a mock LLM callback that returns a fixed response
    manager.set_llm_callback(Arc::new(|_msgs| crate::toolloop::LLMResponse {
        content: "Analysis complete: data processed".to_string(),
        tool_calls: vec![],
    }));

    let tool = SubagentTool::new(manager);
    let result = tool
        .execute(&serde_json::json!({
            "task": "analyze data",
            "label": "data-analysis"
        }))
        .await;

    assert!(!result.is_error, "Should not error: {}", result.for_llm);
    assert!(
        result.for_llm.contains("data-analysis"),
        "Should contain label"
    );
    assert!(
        result.for_llm.contains("Iterations: 1"),
        "Should report 1 iteration, got: {}",
        result.for_llm
    );
    assert!(
        result.for_llm.contains("Analysis complete"),
        "Should contain LLM response, got: {}",
        result.for_llm
    );
}

#[tokio::test]
async fn test_subagent_tool_llm_fallback_without_callback() {
    let manager = Arc::new(SubagentManager::new());
    // Deliberately NOT setting llm_callback

    let tool = SubagentTool::new(manager);
    let result = tool
        .execute(&serde_json::json!({"task": "no callback test", "label": "no-llm"}))
        .await;

    assert!(!result.is_error);
    assert!(result.for_llm.contains("Subagent task completed"));
    assert!(
        result.for_llm.contains("Iterations: 0"),
        "Without LLM callback, iterations should be 0"
    );
}

#[tokio::test]
async fn test_subagent_tool_llm_long_content_truncation() {
    let manager = Arc::new(SubagentManager::new());

    // LLM returns very long content
    let long_content = "y".repeat(1000);
    manager.set_llm_callback(Arc::new(move |_msgs| crate::toolloop::LLMResponse {
        content: long_content.clone(),
        tool_calls: vec![],
    }));

    let tool = SubagentTool::new(manager);
    let result = tool
        .execute(&serde_json::json!({"task": "long output"}))
        .await;

    assert!(!result.is_error);
    let user_content = result.for_user.as_ref().unwrap();
    assert!(
        user_content.len() <= 503,
        "User content should be truncated, got {} chars",
        user_content.len()
    );
}

// ============================================================
// Additional coverage tests for 95%+ target
// ============================================================

#[test]
fn test_subagent_manager_default() {
    let manager = SubagentManager::default();
    assert!(manager.list_tasks().is_empty());
}

#[tokio::test]
async fn test_subagent_tool_empty_task() {
    let manager = Arc::new(SubagentManager::new());
    let tool = SubagentTool::new(manager);
    let result = tool.execute(&serde_json::json!({"task": ""})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("task"));
}

#[tokio::test]
async fn test_subagent_tool_no_label() {
    let manager = Arc::new(SubagentManager::new());
    let tool = SubagentTool::new(manager);
    let result = tool
        .execute(&serde_json::json!({"task": "do something"}))
        .await;
    assert!(!result.is_error);
    assert!(result.for_llm.contains("(unnamed)"));
}

#[test]
fn test_subagent_task_serialization() {
    let task = SubagentTask {
        id: "subagent-1".to_string(),
        task: "test task".to_string(),
        label: "test".to_string(),
        agent_id: "agent-1".to_string(),
        origin_channel: "web".to_string(),
        origin_chat_id: "chat-1".to_string(),
        status: "running".to_string(),
        result: String::new(),
        created: 1234567890,
    };
    let json = serde_json::to_string(&task).unwrap();
    let parsed: SubagentTask = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.id, task.id);
    assert_eq!(parsed.task, task.task);
    assert_eq!(parsed.status, task.status);
    assert_eq!(parsed.created, task.created);
}

#[test]
fn test_subagent_manager_set_max_iterations() {
    let mut manager = SubagentManager::new();
    manager.set_max_iterations(5);
    // Just verify no panic
}

#[test]
fn test_subagent_manager_update_nonexistent() {
    let manager = SubagentManager::new();
    // Should not panic
    manager.update_task("nonexistent", "completed", "result");
}

#[tokio::test]
async fn test_subagent_tool_parameters() {
    let manager = Arc::new(SubagentManager::new());
    let tool = SubagentTool::new(manager);
    let params = tool.parameters();
    assert!(params["properties"]["task"].is_object());
    assert!(params["properties"]["label"].is_object());
    assert!(
        params["required"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("task"))
    );
}

#[tokio::test]
async fn test_subagent_tool_with_task_null() {
    let manager = Arc::new(SubagentManager::new());
    let tool = SubagentTool::new(manager);
    let result = tool.execute(&serde_json::json!({"task": null})).await;
    assert!(result.is_error);
}

// ============================================================
// Additional coverage tests for 95%+ target
// ============================================================

#[tokio::test]
async fn test_subagent_tool_non_object_args() {
    let manager = Arc::new(SubagentManager::new());
    let tool = SubagentTool::new(manager);
    let result = tool.execute(&serde_json::json!("not an object")).await;
    assert!(result.is_error);
}

#[test]
fn test_subagent_manager_spawn_and_get() {
    let manager = SubagentManager::new();
    let id = manager.spawn(
        "Test task".to_string(),
        "label".to_string(),
        "agent-1".to_string(),
        "web".to_string(),
        "chat-1".to_string(),
        None,
    );
    let task = manager.get_task(&id).unwrap();
    assert_eq!(task.id, id);
    assert_eq!(task.task, "Test task");
    assert_eq!(task.status, "running");
    assert_eq!(task.result, "");
    assert!(task.created > 0);
}

#[test]
fn test_subagent_manager_list_tasks_after_remove() {
    let manager = SubagentManager::new();
    let id1 = manager.spawn(
        "T1".to_string(),
        String::new(),
        String::new(),
        "web".to_string(),
        "c1".to_string(),
        None,
    );
    let id2 = manager.spawn(
        "T2".to_string(),
        String::new(),
        String::new(),
        "web".to_string(),
        "c2".to_string(),
        None,
    );
    assert_eq!(manager.list_tasks().len(), 2);

    manager.remove_task(&id1);
    assert_eq!(manager.list_tasks().len(), 1);
    assert!(manager.get_task(&id1).is_none());
    assert!(manager.get_task(&id2).is_some());
}

#[test]
fn test_subagent_manager_update_sets_result() {
    let manager = SubagentManager::new();
    let id = manager.spawn(
        "Task".to_string(),
        String::new(),
        String::new(),
        "web".to_string(),
        "c".to_string(),
        None,
    );

    manager.update_task(&id, "completed", "task done successfully");
    let task = manager.get_task(&id).unwrap();
    assert_eq!(task.status, "completed");
    assert_eq!(task.result, "task done successfully");
}

#[tokio::test]
async fn test_subagent_tool_with_llm_callback_tool_calls() {
    let manager = Arc::new(SubagentManager::new());

    // LLM that makes one tool call, then completes
    let call_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let count_clone = call_count.clone();
    manager.set_llm_callback(Arc::new(move |_msgs| {
        let n = count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n == 0 {
            crate::toolloop::LLMResponse {
                content: String::new(),
                tool_calls: vec![crate::toolloop::LLMToolCall {
                    id: "call-1".to_string(),
                    name: "read_file".to_string(),
                    arguments: serde_json::json!({"path": "/tmp/test"}),
                }],
            }
        } else {
            crate::toolloop::LLMResponse {
                content: "Tool executed, task complete".to_string(),
                tool_calls: vec![],
            }
        }
    }));

    let tool = SubagentTool::new(manager);
    let result = tool
        .execute(&serde_json::json!({
            "task": "read a file",
            "label": "file-reader"
        }))
        .await;

    assert!(!result.is_error, "Should not error: {}", result.for_llm);
    assert!(result.for_llm.contains("file-reader"));
}

#[test]
fn test_subagent_task_all_fields() {
    let task = SubagentTask {
        id: "subagent-42".to_string(),
        task: "complex analysis".to_string(),
        label: "analysis".to_string(),
        agent_id: "agent-007".to_string(),
        origin_channel: "rpc".to_string(),
        origin_chat_id: "chat-xyz".to_string(),
        status: "pending".to_string(),
        result: "not started".to_string(),
        created: 1700000000000u64,
    };

    let json = serde_json::to_string_pretty(&task).unwrap();
    let parsed: SubagentTask = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.id, "subagent-42");
    assert_eq!(parsed.agent_id, "agent-007");
    assert_eq!(parsed.created, 1700000000000u64);
}

#[tokio::test]
async fn test_subagent_tool_execute_non_string_task() {
    let manager = Arc::new(SubagentManager::new());
    let tool = SubagentTool::new(manager);
    let result = tool.execute(&serde_json::json!({"task": 123})).await;
    assert!(result.is_error);
}

#[tokio::test]
async fn test_subagent_tool_execute_with_non_object_args() {
    let manager = Arc::new(SubagentManager::new());
    let tool = SubagentTool::new(manager);
    let result = tool.execute(&serde_json::json!(42)).await;
    assert!(result.is_error);
}

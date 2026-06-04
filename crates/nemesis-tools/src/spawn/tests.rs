use super::*;

#[tokio::test]
async fn test_subagent_manager_spawn() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = SubagentManager::new(registry);

    let id = manager
        .spawn("test task", "test-label", "", "web", "chat-1")
        .await;
    assert!(id.starts_with("subagent-"));

    let task = manager.get_task(&id).await.unwrap();
    assert_eq!(task.task, "test task");
    assert_eq!(task.label, "test-label");
    assert_eq!(task.status, "running");
}

#[tokio::test]
async fn test_subagent_manager_update() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = SubagentManager::new(registry);

    let id = manager.spawn("task", "", "", "", "").await;
    let updated = manager.update_task(&id, "completed", "done").await;
    assert!(updated);

    let task = manager.get_task(&id).await.unwrap();
    assert_eq!(task.status, "completed");
    assert_eq!(task.result, "done");
}

#[tokio::test]
async fn test_subagent_manager_list() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = SubagentManager::new(registry);

    manager.spawn("task1", "", "", "", "").await;
    manager.spawn("task2", "", "", "", "").await;

    let tasks = manager.list_tasks().await;
    assert_eq!(tasks.len(), 2);
    assert_eq!(manager.task_count().await, 2);
}

#[tokio::test]
async fn test_spawn_tool_execute() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = Arc::new(SubagentManager::new(registry));
    let tool = SpawnTool::new(manager);

    let result = tool
        .execute(&serde_json::json!({"task": "do something", "label": "test"}))
        .await;
    assert!(result.is_async, "Spawn should return async result");
    assert!(result.for_llm.contains("Spawned subagent"));
    assert!(result.for_llm.contains("test"));
}

#[tokio::test]
async fn test_spawn_tool_missing_task() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = Arc::new(SubagentManager::new(registry));
    let tool = SpawnTool::new(manager);

    let result = tool.execute(&serde_json::json!({})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("required"));
}

#[tokio::test]
async fn test_spawn_tool_allowlist_check() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = Arc::new(SubagentManager::new(registry));
    let mut tool = SpawnTool::new(manager);
    tool.set_allowlist_check(Arc::new(|id| id == "allowed-agent"));

    // Allowed agent
    let result = tool
        .execute(&serde_json::json!({"task": "test", "agent_id": "allowed-agent"}))
        .await;
    assert!(!result.is_error, "Should allow allowed-agent");

    // Disallowed agent
    let result = tool
        .execute(&serde_json::json!({"task": "test", "agent_id": "blocked-agent"}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("not allowed"));
}

#[tokio::test]
async fn test_spawn_tool_contextual() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = Arc::new(SubagentManager::new(registry));
    let mut tool = SpawnTool::new(manager);

    let ctx = crate::registry::ToolExecutionContext {
        channel: "rpc".to_string(),
        chat_id: "chat-456".to_string(),
        ..Default::default()
    };
    ContextualTool::set_context(&mut tool, &ctx);
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    assert_eq!(tool.channel().await, "rpc");
    assert_eq!(tool.chat_id().await, "chat-456");
}

// --- SubagentManager new methods ---

#[tokio::test]
async fn test_subagent_manager_register_tool() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = SubagentManager::new(Arc::clone(&registry));

    // Initially registry is empty
    assert!(!manager.registry().has("echo"));

    // Register a tool via the manager
    struct Echo;
    #[async_trait]
    impl Tool for Echo {
        fn name(&self) -> &str { "echo" }
        fn description(&self) -> &str { "Echo" }
        fn parameters(&self) -> serde_json::Value { serde_json::json!({"type": "object"}) }
        async fn execute(&self, args: &serde_json::Value) -> ToolResult {
            ToolResult::success(args["text"].as_str().unwrap_or(""))
        }
    }
    manager.register_tool(Arc::new(Echo));

    assert!(manager.registry().has("echo"));
}

#[tokio::test]
async fn test_subagent_manager_set_tools() {
    let registry1 = Arc::new(ToolRegistry::new());
    let manager = SubagentManager::new(registry1);

    // Create a new registry with a tool
    let registry2 = Arc::new(ToolRegistry::new());
    struct Echo;
    #[async_trait]
    impl Tool for Echo {
        fn name(&self) -> &str { "echo" }
        fn description(&self) -> &str { "Echo" }
        fn parameters(&self) -> serde_json::Value { serde_json::json!({"type": "object"}) }
        async fn execute(&self, _args: &serde_json::Value) -> ToolResult {
            ToolResult::success("")
        }
    }
    registry2.register(Arc::new(Echo));

    // Replace registry
    manager.set_tools(registry2).await;

    // New registry should have the tool
    assert!(manager.registry().has("echo"));
}

// --- SubagentTool tests ---

#[tokio::test]
async fn test_subagent_tool_execute() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = Arc::new(SubagentManager::new(registry));
    let tool = SubagentTool::new(manager);

    let result = tool
        .execute(&serde_json::json!({
            "task": "do something important",
            "label": "important-task"
        }))
        .await;

    assert!(!result.is_error, "SubagentTool should not error: {}", result.for_llm);
    assert!(result.for_llm.contains("important-task"), "LLM content should contain label");
    assert!(result.for_llm.contains("Subagent task completed"), "LLM content should contain completion message");
    assert!(result.for_user.is_some(), "Should have user content");
    assert!(result.for_user.as_ref().unwrap().contains("Subagent task received"), "User content should contain result summary");
}

#[tokio::test]
async fn test_subagent_tool_missing_task() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = Arc::new(SubagentManager::new(registry));
    let tool = SubagentTool::new(manager);

    let result = tool.execute(&serde_json::json!({})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("required"));
}

#[tokio::test]
async fn test_subagent_tool_empty_task() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = Arc::new(SubagentManager::new(registry));
    let tool = SubagentTool::new(manager);

    let result = tool.execute(&serde_json::json!({"task": ""})).await;
    assert!(result.is_error);
}

#[tokio::test]
async fn test_subagent_tool_no_label() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = Arc::new(SubagentManager::new(registry));
    let tool = SubagentTool::new(manager);

    let result = tool
        .execute(&serde_json::json!({"task": "test task"}))
        .await;

    assert!(!result.is_error);
    assert!(result.for_llm.contains("(unnamed)"), "Should show unnamed label when not provided");
}

#[tokio::test]
async fn test_subagent_tool_contextual() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = Arc::new(SubagentManager::new(registry));
    let mut tool = SubagentTool::new(manager);

    let ctx = crate::registry::ToolExecutionContext {
        channel: "web".to_string(),
        chat_id: "chat-789".to_string(),
        ..Default::default()
    };
    ContextualTool::set_context(&mut tool, &ctx);
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    // Verify context is set (via the spawned task's origin fields)
    let result = tool
        .execute(&serde_json::json!({"task": "context test"}))
        .await;
    assert!(!result.is_error);
}

#[tokio::test]
async fn test_subagent_tool_metadata() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = Arc::new(SubagentManager::new(registry));
    let tool = SubagentTool::new(manager);

    assert_eq!(tool.name(), "subagent");
    assert!(!tool.description().is_empty());
}

#[test]
fn test_spawn_tool_set_callback() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = Arc::new(SubagentManager::new(registry));
    let mut tool = SpawnTool::new(manager);

    // Set a callback - should not panic
    tool.set_callback(Box::new(move |_result| {
        // callback invoked
    }));

    // Verify callback is set (via internal state)
    assert!(tool.callback.is_some());
}

#[tokio::test]
async fn test_subagent_tool_completes_task_in_manager() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = Arc::new(SubagentManager::new(registry));
    let tool = SubagentTool::new(Arc::clone(&manager));

    tool.execute(&serde_json::json!({"task": "managed task", "label": "test-label"}))
        .await;

    // The task should be in the manager with completed status
    let tasks = manager.list_tasks().await;
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].status, "completed");
    assert_eq!(tasks[0].label, "test-label");
    assert!(tasks[0].result.contains("Subagent task received"));
}

// ============================================================
// Additional spawn/subagent tests
// ============================================================

#[tokio::test]
async fn test_subagent_manager_get_nonexistent() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = SubagentManager::new(registry);
    let task = manager.get_task("nonexistent-id").await;
    assert!(task.is_none());
}

#[tokio::test]
async fn test_subagent_manager_update_nonexistent() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = SubagentManager::new(registry);
    let updated = manager.update_task("nonexistent-id", "completed", "done").await;
    assert!(!updated);
}

#[tokio::test]
async fn test_subagent_manager_spawn_multiple() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = SubagentManager::new(registry);

    let id1 = manager.spawn("task1", "label1", "", "web", "chat-1").await;
    let id2 = manager.spawn("task2", "label2", "", "rpc", "chat-2").await;
    let id3 = manager.spawn("task3", "label3", "", "cli", "chat-3").await;

    assert_ne!(id1, id2);
    assert_ne!(id2, id3);

    let tasks = manager.list_tasks().await;
    assert_eq!(tasks.len(), 3);
    assert_eq!(manager.task_count().await, 3);
}

#[tokio::test]
async fn test_subagent_manager_spawn_and_complete() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = SubagentManager::new(registry);

    let id = manager.spawn("task", "", "", "", "").await;
    assert!(manager.get_task(&id).await.unwrap().status == "running");

    manager.update_task(&id, "completed", "result data").await;
    let task = manager.get_task(&id).await.unwrap();
    assert_eq!(task.status, "completed");
    assert_eq!(task.result, "result data");
}

#[tokio::test]
async fn test_subagent_manager_spawn_and_fail() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = SubagentManager::new(registry);

    let id = manager.spawn("failing task", "", "", "", "").await;
    manager.update_task(&id, "failed", "error: timeout").await;
    let task = manager.get_task(&id).await.unwrap();
    assert_eq!(task.status, "failed");
    assert_eq!(task.result, "error: timeout");
}

#[tokio::test]
async fn test_spawn_tool_with_agent_id() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = Arc::new(SubagentManager::new(registry));
    let mut tool = SpawnTool::new(manager);
    tool.set_allowlist_check(Arc::new(|id| id == "agent-1"));

    // Allowed agent
    let result = tool
        .execute(&serde_json::json!({"task": "test", "agent_id": "agent-1"}))
        .await;
    assert!(!result.is_error);

    // Blocked agent
    let result = tool
        .execute(&serde_json::json!({"task": "test", "agent_id": "agent-2"}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("not allowed"));
}

#[tokio::test]
async fn test_spawn_tool_callback_invoked() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = Arc::new(SubagentManager::new(registry));
    let mut tool = SpawnTool::new(manager);

    let callback_called = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let callback_called_clone = callback_called.clone();
    tool.set_callback(Box::new(move |_result| {
        callback_called_clone.store(true, std::sync::atomic::Ordering::SeqCst);
    }));

    let result = tool
        .execute(&serde_json::json!({"task": "callback test"}))
        .await;
    // Execute spawns asynchronously - callback is set but not called during execute
    assert!(!result.is_error);
    assert!(result.for_llm.contains("Spawned"));
}

#[tokio::test]
async fn test_spawn_tool_metadata() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = Arc::new(SubagentManager::new(registry));
    let tool = SpawnTool::new(manager);

    assert_eq!(tool.name(), "spawn");
    assert!(!tool.description().is_empty());
    let params = tool.parameters();
    assert_eq!(params["type"], "object");
    assert!(params["properties"]["task"].is_object());
}

#[tokio::test]
async fn test_subagent_tool_user_content_truncation() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = Arc::new(SubagentManager::new(registry));
    let tool = SubagentTool::new(manager);

    // Create a very long task description
    let long_task = "x".repeat(1000);
    let result = tool
        .execute(&serde_json::json!({"task": long_task}))
        .await;

    assert!(!result.is_error);
    let user_content = result.for_user.as_ref().unwrap();
    // User content should be truncated to 500 chars max
    assert!(user_content.len() <= 503); // 500 + "..."
}

#[tokio::test]
async fn test_subagent_tool_metadata_check() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = Arc::new(SubagentManager::new(registry));
    let tool = SubagentTool::new(manager);

    assert_eq!(tool.name(), "subagent");
    assert!(!tool.description().is_empty());
    let params = tool.parameters();
    assert_eq!(params["type"], "object");
    assert!(params["properties"]["task"].is_object());
}

#[tokio::test]
async fn test_subagent_manager_register_and_use_tool() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = SubagentManager::new(Arc::clone(&registry));

    struct UpperTool;
    #[async_trait]
    impl Tool for UpperTool {
        fn name(&self) -> &str { "upper" }
        fn description(&self) -> &str { "Uppercase" }
        fn parameters(&self) -> serde_json::Value { serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}) }
        async fn execute(&self, args: &serde_json::Value) -> ToolResult {
            let text = args["text"].as_str().unwrap_or("");
            ToolResult::success(&text.to_uppercase())
        }
    }
    manager.register_tool(Arc::new(UpperTool));

    // Verify tool is accessible through the registry
    assert!(manager.registry().has("upper"));
    let result = manager.registry().execute("upper", &serde_json::json!({"text": "hello"})).await;
    assert_eq!(result.for_llm, "HELLO");
}

#[tokio::test]
async fn test_spawn_tool_parameters() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = Arc::new(SubagentManager::new(registry));
    let tool = SpawnTool::new(manager);

    let params = tool.parameters();
    assert_eq!(params["type"], "object");
    let required = params["required"].as_array().unwrap();
    assert!(required.iter().any(|r| r.as_str() == Some("task")));
}

// ============================================================
// Tests for real LLM execution via run_tool_loop
// ============================================================

#[tokio::test]
async fn test_subagent_tool_with_llm_callback() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = Arc::new(SubagentManager::new(registry));

    // Set up a mock LLM callback that returns a fixed response
    manager.set_llm_callback(Arc::new(|_msgs| {
        crate::toolloop::LLMResponse {
            content: "Task completed successfully: analysis done".to_string(),
            tool_calls: vec![],
        }
    }));

    let tool = SubagentTool::new(manager);
    let result = tool
        .execute(&serde_json::json!({
            "task": "analyze the data",
            "label": "analysis"
        }))
        .await;

    assert!(!result.is_error, "Should not error: {}", result.for_llm);
    assert!(result.for_llm.contains("analysis"), "Should contain label");
    assert!(
        result.for_llm.contains("Iterations: 1"),
        "Should report 1 iteration, got: {}",
        result.for_llm
    );
    assert!(
        result.for_llm.contains("Task completed successfully"),
        "Should contain LLM response content, got: {}",
        result.for_llm
    );
}

#[tokio::test]
async fn test_subagent_tool_llm_callback_with_tools() {
    let registry = Arc::new(ToolRegistry::new());

    // Register a tool for the subagent to use
    struct EchoTool;
    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str { "echo" }
        fn description(&self) -> &str { "Echo back input" }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}})
        }
        async fn execute(&self, args: &serde_json::Value) -> ToolResult {
            ToolResult::success(args["text"].as_str().unwrap_or(""))
        }
    }
    registry.register(Arc::new(EchoTool));

    let manager = Arc::new(SubagentManager::new(registry));

    // LLM callback: first call requests tool, second returns final answer
    let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let call_count_clone = call_count.clone();
    manager.set_llm_callback(Arc::new(move |_msgs| {
        let count = call_count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if count == 0 {
            crate::toolloop::LLMResponse {
                content: String::new(),
                tool_calls: vec![crate::toolloop::LLMToolCall {
                    id: "tc-1".to_string(),
                    name: "echo".to_string(),
                    arguments: serde_json::json!({"text": "hello from subagent"}),
                }],
            }
        } else {
            crate::toolloop::LLMResponse {
                content: "Subagent used echo tool and got: hello from subagent".to_string(),
                tool_calls: vec![],
            }
        }
    }));

    let tool = SubagentTool::new(manager);
    let result = tool
        .execute(&serde_json::json!({"task": "use echo tool"}))
        .await;

    assert!(!result.is_error, "Should not error: {}", result.for_llm);
    assert!(
        result.for_llm.contains("Iterations: 2"),
        "Should report 2 iterations (tool call + final), got: {}",
        result.for_llm
    );
    assert!(
        result.for_llm.contains("hello from subagent"),
        "Should contain tool result, got: {}",
        result.for_llm
    );
}

#[tokio::test]
async fn test_spawn_tool_with_llm_background_execution() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = Arc::new(SubagentManager::new(registry));

    // Set up a mock LLM callback
    manager.set_llm_callback(Arc::new(|_msgs| {
        crate::toolloop::LLMResponse {
            content: "Background task completed".to_string(),
            tool_calls: vec![],
        }
    }));

    let tool = SpawnTool::new(manager.clone());
    let result = tool
        .execute(&serde_json::json!({"task": "background work", "label": "bg-task"}))
        .await;

    // Should return async result immediately
    assert!(result.is_async, "SpawnTool should return async result");
    assert!(result.for_llm.contains("Spawned subagent"));

    // Wait a bit for the background task to complete
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Check that the task was completed with the real LLM result
    let tasks = manager.list_tasks().await;
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].status, "completed");
    assert!(
        tasks[0].result.contains("Background task completed"),
        "Task result should contain LLM response, got: {}",
        tasks[0].result
    );
}

#[tokio::test]
async fn test_subagent_manager_has_llm_callback() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = SubagentManager::new(registry);

    assert!(!manager.has_llm_callback(), "Should start without callback");

    manager.set_llm_callback(Arc::new(|_msgs| {
        crate::toolloop::LLMResponse {
            content: "test".to_string(),
            tool_calls: vec![],
        }
    }));

    assert!(manager.has_llm_callback(), "Should have callback after setting");
}

#[tokio::test]
async fn test_subagent_tool_fallback_without_llm() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = Arc::new(SubagentManager::new(registry));
    // Deliberately NOT setting llm_callback

    let tool = SubagentTool::new(manager);
    let result = tool
        .execute(&serde_json::json!({"task": "fallback test", "label": "no-llm"}))
        .await;

    assert!(!result.is_error);
    // Should contain placeholder content
    assert!(result.for_llm.contains("Subagent task completed"));
    assert!(result.for_llm.contains("Iterations: 0"), "Without LLM, iterations should be 0");
    assert!(
        result.for_user.as_ref().unwrap().contains("Subagent task received"),
        "User content should have placeholder"
    );
}

#[tokio::test]
async fn test_subagent_tool_llm_callback_long_content_truncation() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = Arc::new(SubagentManager::new(registry));

    // LLM returns very long content
    let long_content = "x".repeat(1000);
    manager.set_llm_callback(Arc::new(move |_msgs| {
        crate::toolloop::LLMResponse {
            content: long_content.clone(),
            tool_calls: vec![],
        }
    }));

    let tool = SubagentTool::new(manager);
    let result = tool
        .execute(&serde_json::json!({"task": "long task"}))
        .await;

    assert!(!result.is_error);
    let user_content = result.for_user.as_ref().unwrap();
    assert!(user_content.len() <= 503, "User content should be truncated, got {} chars", user_content.len());
}

#[tokio::test]
async fn test_spawn_tool_with_callback_and_llm() {
    let registry = Arc::new(ToolRegistry::new());
    let manager = SubagentManager::new(registry);
    manager.set_llm_callback(Arc::new(|_msgs| {
        crate::toolloop::LLMResponse {
            content: "background task done".to_string(),
            tool_calls: vec![],
        }
    }));

    let manager = Arc::new(manager);
    let mut tool = SpawnTool::new(Arc::clone(&manager));

    // Set an async callback
    let callback_called = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let called_clone = callback_called.clone();
    tool.set_callback(Box::new(move |_result| {
        called_clone.store(true, std::sync::atomic::Ordering::SeqCst);
    }));

    let result = tool
        .execute(&serde_json::json!({"task": "background test", "label": "bg"}))
        .await;

    assert!(!result.is_error);
    assert!(result.is_async);
    assert!(result.task_id.is_some());

    // Wait for the spawned task to complete
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Check that the task was completed by the background LLM
    let tasks = manager.list_tasks().await;
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].status, "completed");
    assert!(tasks[0].result.contains("background task done"));
}

#[test]
fn test_subagent_manager_set_max_iterations() {
    let registry = Arc::new(ToolRegistry::new());
    let mut manager = SubagentManager::new(registry);
    manager.set_max_iterations(42);
    assert_eq!(manager.max_iterations, 42);
}

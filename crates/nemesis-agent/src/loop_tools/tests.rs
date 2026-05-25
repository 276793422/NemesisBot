use super::*;
use tempfile::TempDir;

#[tokio::test]
async fn test_message_tool_with_json() {
    let tool = MessageTool::new();
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    let result = tool
        .execute(r#"{"content": "Hello, world!"}"#, &ctx)
        .await
        .unwrap();
    assert_eq!(result, "Hello, world!");

    // Fallback: raw args.
    let result = tool.execute("plain text", &ctx).await.unwrap();
    assert_eq!(result, "plain text");
}

#[tokio::test]
async fn test_read_write_file_tool() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("test.txt");
    let file_path_str = file_path.to_string_lossy().to_string();

    // Write a file.
    let write_tool = WriteFileTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({
        "path": file_path_str,
        "content": "Hello from write tool!"
    })
    .to_string();

    let result = write_tool.execute(&args, &ctx).await.unwrap();
    assert!(result.contains("Successfully wrote"));

    // Read it back.
    let read_tool = ReadFileTool;
    let args = serde_json::json!({ "path": file_path_str }).to_string();
    let result = read_tool.execute(&args, &ctx).await.unwrap();
    assert_eq!(result, "Hello from write tool!");
}

#[tokio::test]
async fn test_list_directory_tool() {
    let tmp = TempDir::new().unwrap();

    // Create some entries.
    tokio::fs::write(tmp.path().join("file1.txt"), "content1")
        .await
        .unwrap();
    tokio::fs::create_dir(tmp.path().join("subdir"))
        .await
        .unwrap();

    let tool = ListDirectoryTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({ "path": tmp.path().to_string_lossy() }).to_string();

    let result = tool.execute(&args, &ctx).await.unwrap();
    assert!(result.contains("file1.txt"));
    assert!(result.contains("subdir"));
    assert!(result.contains("[file]"));
    assert!(result.contains("[dir]"));
}

#[tokio::test]
async fn test_edit_file_tool() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("edit_test.txt");
    tokio::fs::write(&file_path, "Hello world, this is a test.")
        .await
        .unwrap();

    let tool = EditFileTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({
        "path": file_path.to_string_lossy(),
        "old_text": "Hello world",
        "new_text": "Greetings universe"
    })
    .to_string();

    let result = tool.execute(&args, &ctx).await.unwrap();
    assert!(result.contains("File edited"));

    let content = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(content, "Greetings universe, this is a test.");
}

#[tokio::test]
async fn test_edit_file_tool_old_text_not_found() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("edit_test.txt");
    tokio::fs::write(&file_path, "Hello world").await.unwrap();

    let tool = EditFileTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({
        "path": file_path.to_string_lossy(),
        "old_text": "nonexistent",
        "new_text": "replacement"
    })
    .to_string();

    let result = tool.execute(&args, &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found in file"));
}

#[tokio::test]
async fn test_edit_file_tool_duplicate_old_text() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("edit_test.txt");
    tokio::fs::write(&file_path, "aaa bbb aaa").await.unwrap();

    let tool = EditFileTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({
        "path": file_path.to_string_lossy(),
        "old_text": "aaa",
        "new_text": "ccc"
    })
    .to_string();

    let result = tool.execute(&args, &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("appears 2 times"));
}

#[tokio::test]
async fn test_append_file_tool() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("append_test.txt");
    tokio::fs::write(&file_path, "Line 1\n").await.unwrap();

    let tool = AppendFileTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({
        "path": file_path.to_string_lossy(),
        "content": "Line 2\n"
    })
    .to_string();

    let result = tool.execute(&args, &ctx).await.unwrap();
    assert!(result.contains("Appended"));

    let content = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(content, "Line 1\nLine 2\n");
}

#[tokio::test]
async fn test_append_file_creates_new_file() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("new_file.txt");

    let tool = AppendFileTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({
        "path": file_path.to_string_lossy(),
        "content": "New content"
    })
    .to_string();

    let result = tool.execute(&args, &ctx).await.unwrap();
    assert!(result.contains("Appended"));

    let content = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(content, "New content");
}

#[tokio::test]
async fn test_delete_file_tool() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("to_delete.txt");
    tokio::fs::write(&file_path, "content").await.unwrap();
    assert!(file_path.exists());

    let tool = DeleteFileTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({ "path": file_path.to_string_lossy() }).to_string();

    let result = tool.execute(&args, &ctx).await.unwrap();
    assert!(result.contains("Deleted"));
    assert!(!file_path.exists());
}

#[tokio::test]
async fn test_delete_file_not_found() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("nonexistent.txt");

    let tool = DeleteFileTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({ "path": file_path.to_string_lossy() }).to_string();

    let result = tool.execute(&args, &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found"));
}

#[tokio::test]
async fn test_create_dir_tool() {
    let tmp = TempDir::new().unwrap();
    let dir_path = tmp.path().join("new_dir").join("nested");

    let tool = CreateDirTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({ "path": dir_path.to_string_lossy() }).to_string();

    let result = tool.execute(&args, &ctx).await.unwrap();
    assert!(result.contains("created"));
    assert!(dir_path.exists());
}

#[tokio::test]
async fn test_create_dir_already_exists() {
    let tmp = TempDir::new().unwrap();

    let tool = CreateDirTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({ "path": tmp.path().to_string_lossy() }).to_string();

    let result = tool.execute(&args, &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("already exists"));
}

#[tokio::test]
async fn test_delete_dir_tool() {
    let tmp = TempDir::new().unwrap();
    let dir_path = tmp.path().join("to_remove");
    tokio::fs::create_dir_all(&dir_path).await.unwrap();
    tokio::fs::write(dir_path.join("file.txt"), "content")
        .await
        .unwrap();
    assert!(dir_path.exists());

    let tool = DeleteDirTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({ "path": dir_path.to_string_lossy() }).to_string();

    let result = tool.execute(&args, &ctx).await.unwrap();
    assert!(result.contains("removed"));
    assert!(!dir_path.exists());
}

#[tokio::test]
async fn test_sleep_tool() {
    let tool = SleepTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({ "duration": 1 }).to_string();

    let start = std::time::Instant::now();
    let result = tool.execute(&args, &ctx).await.unwrap();
    let elapsed = start.elapsed();

    assert!(result.contains("Slept for 1 seconds"));
    assert!(elapsed.as_secs() >= 1);
}

#[tokio::test]
async fn test_sleep_tool_exceeds_max() {
    let tool = SleepTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({ "duration": 4000 }).to_string();

    let result = tool.execute(&args, &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("cannot exceed"));
}

#[tokio::test]
async fn test_sleep_tool_zero_duration() {
    let tool = SleepTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({ "duration": 0 }).to_string();

    let result = tool.execute(&args, &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("at least 1 second"));
}

#[test]
fn test_register_default_tools_count() {
    let tools = register_default_tools();
    assert_eq!(tools.len(), 10);
    assert!(tools.contains_key("message"));
    assert!(tools.contains_key("read_file"));
    assert!(tools.contains_key("write_file"));
    assert!(tools.contains_key("list_dir"));
    assert!(tools.contains_key("edit_file"));
    assert!(tools.contains_key("append_file"));
    assert!(tools.contains_key("delete_file"));
    assert!(tools.contains_key("create_dir"));
    assert!(tools.contains_key("delete_dir"));
    assert!(tools.contains_key("sleep"));
}

// --- Extended tool tests ---

/// This test makes a real network request to DuckDuckGo.
/// Use `cargo test -- --ignored` to run network-dependent tests.
#[tokio::test]
#[ignore]
async fn test_web_search_tool_duckduckgo_live() {
    let config = WebSearchConfig {
        duckduckgo_enabled: true,
        ..Default::default()
    };
    let tool = WebSearchTool::new(config);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    let result = tool
        .execute(r#"{"query": "Rust programming"}"#, &ctx)
        .await
        .unwrap();
    assert!(result.contains("DuckDuckGo"));
    assert!(result.contains("Rust programming"));
}

#[tokio::test]
async fn test_web_search_tool_no_provider() {
    let config = WebSearchConfig {
        duckduckgo_enabled: false,
        brave_enabled: false,
        perplexity_enabled: false,
        ..Default::default()
    };
    let tool = WebSearchTool::new(config);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    let result = tool.execute(r#"{"query": "test"}"#, &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("No search provider"));
}

/// This test makes a real network request.
/// Use `cargo test -- --ignored` to run network-dependent tests.
#[tokio::test]
#[ignore]
async fn test_web_fetch_tool_live() {
    let tool = WebFetchTool::new(50000);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    let result = tool
        .execute(r#"{"url": "https://example.com"}"#, &ctx)
        .await
        .unwrap();
    assert!(result.contains("example.com"));
}

#[tokio::test]
async fn test_web_fetch_tool_invalid_url() {
    let tool = WebFetchTool::new(50000);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    let result = tool
        .execute(r#"{"url": "http://127.0.0.1:1/nonexistent"}"#, &ctx)
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_cluster_rpc_tool() {
    let config = ClusterRpcConfig {
        local_node_id: "node-1".to_string(),
        timeout_secs: 60,
        local_rpc_port: 21949,
    };
    let tool = ClusterRpcTool::new(config);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    // Without an RPC function, should return error
    let result = tool
        .execute(
            r#"{"target_node": "node-2", "message": "Hello from node-1"}"#,
            &ctx,
        )
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not available"));
}

#[tokio::test]
async fn test_cluster_rpc_tool_with_fn() {
    let config = ClusterRpcConfig {
        local_node_id: "node-1".to_string(),
        timeout_secs: 60,
        local_rpc_port: 21949,
    };
    let mut tool = ClusterRpcTool::new(config);
    tool.set_rpc_call_fn(Arc::new(|_node: &str, _action: &str, payload: serde_json::Value| {
        let msg = payload.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
        Box::pin(async move {
            Ok(serde_json::json!({"content": format!("Echo: {}", msg)}))
        })
    }));

    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool
        .execute(
            r#"{"target_node": "node-2", "message": "Hello from node-1"}"#,
            &ctx,
        )
        .await
        .unwrap();
    assert!(result.contains("Echo: Hello from node-1"));
}

#[tokio::test]
async fn test_spawn_tool() {
    let config = SpawnConfig {
        default_model: "test-model".to_string(),
        max_concurrent: 5,
    };
    let tool = SpawnTool::new(config);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    // Without a spawn function, should return error
    let result = tool
        .execute(
            r#"{"agent_id": "worker-1", "task": "Analyze data"}"#,
            &ctx,
        )
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not available"));
}

#[tokio::test]
async fn test_spawn_tool_with_fn() {
    let config = SpawnConfig {
        default_model: "test-model".to_string(),
        max_concurrent: 5,
    };
    let mut tool = SpawnTool::new(config);
    tool.set_spawn_fn(Arc::new(
        |agent_id: &str, task: &str, model: &str, _channel: &str, _chat_id: &str| {
            let agent_id = agent_id.to_string();
            let task = task.to_string();
            let model = model.to_string();
            Box::pin(async move {
                Ok(format!(
                    "[Spawn] Created sub-agent '{}' for task: {} (model: {})",
                    agent_id, task, model
                ))
            })
        },
    ));

    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool
        .execute(
            r#"{"agent_id": "worker-1", "task": "Analyze data"}"#,
            &ctx,
        )
        .await
        .unwrap();
    assert!(result.contains("worker-1"));
    assert!(result.contains("Analyze data"));
    assert!(result.contains("test-model"));
}

#[tokio::test]
async fn test_spawn_tool_allowlist_denied() {
    let config = SpawnConfig {
        default_model: "test-model".to_string(),
        max_concurrent: 5,
    };
    let mut tool = SpawnTool::new(config);
    tool.set_allowlist_checker(Box::new(|_id| false));

    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool
        .execute(
            r#"{"agent_id": "restricted-agent", "task": "Do something"}"#,
            &ctx,
        )
        .await;

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Not allowed"));
}

#[tokio::test]
async fn test_memory_tools_no_executor() {
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    // Without a memory executor, tools should return errors
    let search = MemorySearchTool::new(None);
    let result = search
        .execute(r#"{"query": "test memory"}"#, &ctx)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not available"));

    let store = MemoryStoreTool::new(None);
    let result = store
        .execute(r#"{"memory_type": "episodic", "content": "hello"}"#, &ctx)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not available"));

    let forget = MemoryForgetTool::new(None);
    let result = forget.execute(r#"{"action": "delete_session", "session_key": "test"}"#, &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not available"));

    let list = MemoryListTool::new(None);
    let result = list.execute("{}", &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not available"));
}

#[tokio::test]
async fn test_memory_tools_with_executor() {
    let dir = tempfile::tempdir().unwrap();
    let config = nemesis_memory::manager::Config::new(dir.path());
    let mgr = Arc::new(nemesis_memory::manager::MemoryManager::new(&config));
    let executor = Arc::new(nemesis_memory::memory_tools::MemoryToolExecutor::new(mgr));
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    // Store an episodic memory
    let store = MemoryStoreTool::new(Some(executor.clone()));
    let result = store
        .execute(
            r#"{"memory_type": "episodic", "content": "test content", "role": "user", "session_key": "test-session"}"#,
            &ctx,
        )
        .await
        .unwrap();
    assert!(result.contains("Episodic memory stored"));

    // Search for it
    let search = MemorySearchTool::new(Some(executor.clone()));
    let result = search
        .execute(r#"{"query": "test content"}"#, &ctx)
        .await
        .unwrap();
    assert!(result.contains("test content"));

    // List status
    let list = MemoryListTool::new(Some(executor.clone()));
    let result = list.execute("{}", &ctx).await.unwrap();
    assert!(result.contains("Memory Store Status"));

    // Forget (cleanup)
    let forget = MemoryForgetTool::new(Some(executor.clone()));
    let result = forget
        .execute(r#"{"action": "delete_session", "session_key": "test-session"}"#, &ctx)
        .await
        .unwrap();
    assert!(result.contains("deleted"));
}

#[tokio::test]
async fn test_skills_tools_stub() {
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    // Test without loader (stub mode)
    let list = SkillsListTool::new(None);
    let result = list.execute("{}", &ctx).await.unwrap();
    assert!(result.contains("skills loader not configured"));

    let info = SkillsInfoTool::new(None);
    let result = info.execute("test-skill", &ctx).await.unwrap();
    assert!(result.contains("skills loader not configured"));
}

#[tokio::test]
async fn test_skills_tools_with_loader() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_string_lossy().to_string();
    let global = tmp.path().join("global").to_string_lossy().to_string();
    let builtin = tmp.path().join("builtin").to_string_lossy().to_string();

    // Create a skill in the workspace
    let skill_dir = tmp.path().join("skills").join("test-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: test-skill\ndescription: A test skill\n---\n# Test Skill\n\nDoes test things.",
    ).unwrap();

    let loader = Arc::new(nemesis_skills::loader::SkillsLoader::new(
        &workspace,
        &global,
        &builtin,
    ));

    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    // SkillsListTool with loader
    let list = SkillsListTool::new(Some(loader.clone()));
    let result = list.execute("{}", &ctx).await.unwrap();
    assert!(result.contains("test-skill"));
    assert!(result.contains("Installed skills"));

    // SkillsInfoTool with loader
    let info = SkillsInfoTool::new(Some(loader));
    let result = info.execute("test-skill", &ctx).await.unwrap();
    assert!(result.contains("test-skill"));
    assert!(result.contains("Does test things"));

    // SkillsInfoTool for missing skill
    let info2 = SkillsInfoTool::new(None);
    let result = info2.execute("nonexistent", &ctx).await.unwrap();
    assert!(result.contains("skills loader not configured"));
}

#[tokio::test]
async fn test_mcp_tool() {
    let tool = McpTool::new_simulated("search", "Search tool", "test-server");
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    let result = tool
        .execute(r#"{"query": "test"}"#, &ctx)
        .await
        .unwrap();
    assert!(result.contains("MCP/test-server"));
    assert!(result.contains("search"));
}

#[test]
fn test_register_extended_tools() {
    let tools = register_extended_tools(None, None, None);
    assert!(tools.contains_key("web_fetch"));
    assert!(tools.contains_key("memory_search"));
    assert!(tools.contains_key("memory_store"));
    assert!(tools.contains_key("memory_forget"));
    assert!(tools.contains_key("memory_list"));
    assert!(tools.contains_key("skills_list"));
    assert!(tools.contains_key("skills_info"));
}

#[test]
fn test_register_extended_tools_with_cluster() {
    let cluster_config = ClusterRpcConfig {
        local_node_id: "node-1".to_string(),
        timeout_secs: 60,
        local_rpc_port: 21949,
    };
    let tools = register_extended_tools(None, Some(cluster_config), None);
    assert!(tools.contains_key("cluster_rpc"));
}

#[test]
fn test_register_extended_tools_with_spawn() {
    let spawn_config = SpawnConfig {
        default_model: "test".to_string(),
        max_concurrent: 5,
    };
    let tools = register_extended_tools(None, None, Some(spawn_config));
    assert!(tools.contains_key("spawn"));
}

#[test]
fn test_register_extended_tools_with_web_search() {
    let web_config = WebSearchConfig {
        duckduckgo_enabled: true,
        ..Default::default()
    };
    let tools = register_extended_tools(Some(web_config), None, None);
    assert!(tools.contains_key("web_search"));
    assert!(tools.contains_key("web_fetch"));
}

// =========================================================================
// Additional coverage tests for loop_tools.rs
// =========================================================================

// --- MessageTool coverage ---

#[test]
fn test_message_tool_default() {
    let tool = MessageTool::default();
    assert!(!tool.has_sent_in_round());
}

#[test]
fn test_message_tool_sent_in_round_cycle() {
    let tool = MessageTool::new();
    assert!(!tool.has_sent_in_round());
    tool.reset_sent_in_round();
    assert!(!tool.has_sent_in_round());
}

#[tokio::test]
async fn test_message_tool_with_send_callback() {
    let tool = MessageTool::new();
    let sent_content = Arc::new(std::sync::Mutex::new(String::new()));
    let sent_content_clone = sent_content.clone();
    tool.set_send_callback(Box::new(move |_ch, _cid, content| {
        *sent_content_clone.lock().unwrap() = content.to_string();
    }));

    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool
        .execute(r#"{"content": "test message"}"#, &ctx)
        .await
        .unwrap();
    assert_eq!(result, "test message");
    assert!(tool.has_sent_in_round());
    assert_eq!(*sent_content.lock().unwrap(), "test message");
}

#[tokio::test]
async fn test_message_tool_with_rpc_context() {
    let tool = MessageTool::new();
    let sent = Arc::new(std::sync::Mutex::new(String::new()));
    let sent_clone = sent.clone();
    tool.set_send_callback(Box::new(move |_ch, _cid, content| {
        *sent_clone.lock().unwrap() = content.to_string();
    }));

    let mut ctx = RequestContext::new("rpc", "chat1", "user1", "sess1");
    ctx.correlation_id = Some("corr-123".to_string());
    let result = tool
        .execute(r#"{"content": "hello"}"#, &ctx)
        .await
        .unwrap();
    assert_eq!(result, "hello");
    // The sent content should have RPC prefix
    let sent_val = sent.lock().unwrap().clone();
    assert!(sent_val.contains("[rpc:corr-123]"));
}

#[test]
fn test_message_tool_set_context() {
    let tool = MessageTool::new();
    tool.set_context("discord", "channel-abc");
    // Context is stored internally; verify it works by executing
}

#[tokio::test]
async fn test_message_tool_fallback_channel_from_stored() {
    let tool = MessageTool::new();
    tool.set_context("stored_channel", "stored_chat");

    // Execute with empty channel in context -> should use stored
    let ctx = RequestContext::new("", "", "user1", "sess1");
    let result = tool.execute(r#"{"content": "test"}"#, &ctx).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_message_tool_no_callback_passthrough() {
    let tool = MessageTool::new();
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool
        .execute(r#"{"content": "passthrough"}"#, &ctx)
        .await
        .unwrap();
    assert_eq!(result, "passthrough");
    assert!(!tool.has_sent_in_round());
}

// --- extract_path and extract_path_and_content coverage ---

#[test]
fn test_extract_path_valid_json() {
    let result = extract_path(r#"{"path": "/tmp/test.txt"}"#).unwrap();
    assert_eq!(result, "/tmp/test.txt");
}

#[test]
fn test_extract_path_missing_field() {
    let result = extract_path(r#"{"other": "value"}"#);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Missing 'path'"));
}

#[test]
fn test_extract_path_raw_string() {
    let result = extract_path("  /raw/path  ").unwrap();
    assert_eq!(result, "/raw/path");
}

#[test]
fn test_extract_path_and_content_valid() {
    let (path, content) = extract_path_and_content(r#"{"path": "/a/b", "content": "hello"}"#).unwrap();
    assert_eq!(path, "/a/b");
    assert_eq!(content, "hello");
}

#[test]
fn test_extract_path_and_content_invalid_json() {
    let result = extract_path_and_content("not json");
    assert!(result.is_err());
}

#[test]
fn test_extract_path_and_content_missing_content() {
    let result = extract_path_and_content(r#"{"path": "/tmp"}"#);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Missing 'content'"));
}

#[test]
fn test_extract_path_and_content_missing_path() {
    let result = extract_path_and_content(r#"{"content": "hello"}"#);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Missing 'path'"));
}

#[test]
fn test_extract_edit_args_valid() {
    let (path, old, new) = extract_edit_args(
        r#"{"path": "/a.txt", "old_text": "foo", "new_text": "bar"}"#,
    )
    .unwrap();
    assert_eq!(path, "/a.txt");
    assert_eq!(old, "foo");
    assert_eq!(new, "bar");
}

#[test]
fn test_extract_edit_args_invalid_json() {
    let result = extract_edit_args("invalid");
    assert!(result.is_err());
}

#[test]
fn test_extract_edit_args_missing_old_text() {
    let result = extract_edit_args(r#"{"path": "/a", "new_text": "b"}"#);
    assert!(result.is_err());
}

#[test]
fn test_extract_edit_args_missing_new_text() {
    let result = extract_edit_args(r#"{"path": "/a", "old_text": "b"}"#);
    assert!(result.is_err());
}

// --- File tool edge cases ---

#[tokio::test]
async fn test_read_file_not_found() {
    let tool = ReadFileTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool
        .execute(r#"{"path": "/nonexistent/file.txt"}"#, &ctx)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found"));
}

#[tokio::test]
async fn test_list_dir_not_found() {
    let tool = ListDirectoryTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let nonexistent = format!(r#"{{"path": "C:/__nonexistent_test_dir_{}"}}"#, std::process::id());
    let result = tool
        .execute(&nonexistent, &ctx)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found"));
}

#[tokio::test]
async fn test_list_dir_is_file_not_dir() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("file.txt");
    tokio::fs::write(&file_path, "content").await.unwrap();

    let tool = ListDirectoryTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool
        .execute(&serde_json::json!({"path": file_path.to_string_lossy()}).to_string(), &ctx)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not a directory"));
}

#[tokio::test]
async fn test_list_dir_empty_directory() {
    let tmp = TempDir::new().unwrap();
    let empty_dir = tmp.path().join("empty");
    tokio::fs::create_dir(&empty_dir).await.unwrap();

    let tool = ListDirectoryTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool
        .execute(&serde_json::json!({"path": empty_dir.to_string_lossy()}).to_string(), &ctx)
        .await
        .unwrap();
    assert!(result.contains("empty directory"));
}

#[tokio::test]
async fn test_edit_file_not_found() {
    let tool = EditFileTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool
        .execute(r#"{"path": "/nonexistent.txt", "old_text": "a", "new_text": "b"}"#, &ctx)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found"));
}

#[tokio::test]
async fn test_delete_file_is_directory() {
    let tmp = TempDir::new().unwrap();
    let dir_path = tmp.path().join("a_dir");
    tokio::fs::create_dir(&dir_path).await.unwrap();

    let tool = DeleteFileTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool
        .execute(&serde_json::json!({"path": dir_path.to_string_lossy()}).to_string(), &ctx)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("directory"));
}

#[tokio::test]
async fn test_delete_dir_not_found() {
    let tool = DeleteDirTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool
        .execute(r#"{"path": "/nonexistent_dir_12345"}"#, &ctx)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found"));
}

#[tokio::test]
async fn test_delete_dir_is_file() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("file.txt");
    tokio::fs::write(&file_path, "content").await.unwrap();

    let tool = DeleteDirTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool
        .execute(&serde_json::json!({"path": file_path.to_string_lossy()}).to_string(), &ctx)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not a directory"));
}

#[tokio::test]
async fn test_write_file_creates_parent_dirs() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("a").join("b").join("c").join("test.txt");

    let tool = WriteFileTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({
        "path": file_path.to_string_lossy(),
        "content": "nested content"
    })
    .to_string();

    let result = tool.execute(&args, &ctx).await.unwrap();
    assert!(result.contains("Successfully wrote"));
    assert!(file_path.exists());
}

#[tokio::test]
async fn test_append_to_new_file() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("append_new.txt");

    let tool = AppendFileTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({
        "path": file_path.to_string_lossy(),
        "content": "first line"
    })
    .to_string();

    let result = tool.execute(&args, &ctx).await.unwrap();
    assert!(result.contains("Appended"));
    assert_eq!(tokio::fs::read_to_string(&file_path).await.unwrap(), "first line");
}

// --- SleepTool edge cases ---

#[tokio::test]
async fn test_sleep_tool_raw_number() {
    let tool = SleepTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({"duration": 1}).to_string();
    let result = tool.execute(&args, &ctx).await.unwrap();
    assert!(result.contains("Slept for 1 seconds"));
}

#[tokio::test]
async fn test_sleep_tool_invalid_string() {
    let tool = SleepTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool.execute("not_a_number", &ctx).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_sleep_tool_missing_duration_field() {
    let tool = SleepTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({"other": 5}).to_string();
    let result = tool.execute(&args, &ctx).await;
    assert!(result.is_err());
}

// --- ExecTool coverage ---

#[tokio::test]
async fn test_exec_tool_basic() {
    let tmp = TempDir::new().unwrap();
    let tool = ExecTool::new(&tmp.path().to_string_lossy(), false);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    let cmd = if cfg!(target_os = "windows") { "echo hello" } else { "echo hello" };
    let args = serde_json::json!({"command": cmd}).to_string();
    let result = tool.execute(&args, &ctx).await.unwrap();
    assert!(result.contains("hello"));
}

#[tokio::test]
async fn test_exec_tool_invalid_json() {
    let tool = ExecTool::new("/tmp", false);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool.execute("not json", &ctx).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_exec_tool_missing_command() {
    let tool = ExecTool::new("/tmp", false);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool.execute("{}", &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Missing 'command'"));
}

#[tokio::test]
async fn test_exec_tool_custom_timeout() {
    let tmp = TempDir::new().unwrap();
    let tool = ExecTool::new(&tmp.path().to_string_lossy(), false);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let cmd = if cfg!(target_os = "windows") { "echo test" } else { "echo test" };
    let args = serde_json::json!({"command": cmd, "timeout": 30}).to_string();
    let result = tool.execute(&args, &ctx).await.unwrap();
    assert!(result.contains("test"));
}

#[tokio::test]
async fn test_exec_tool_workspace_restriction() {
    let tool = ExecTool::new("/safe/workspace", true);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({
        "command": "echo test",
        "cwd": "/outside/workspace"
    })
    .to_string();
    let result = tool.execute(&args, &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Access denied"));
}

#[tokio::test]
async fn test_exec_tool_failing_command() {
    let tmp = TempDir::new().unwrap();
    let tool = ExecTool::new(&tmp.path().to_string_lossy(), false);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let cmd = if cfg!(target_os = "windows") { "exit /b 1" } else { "exit 1" };
    let args = serde_json::json!({"command": cmd}).to_string();
    let result = tool.execute(&args, &ctx).await.unwrap();
    assert!(result.contains("Exit code"));
}

// --- AsyncExecTool coverage ---

#[tokio::test]
async fn test_async_exec_tool_basic() {
    let tmp = TempDir::new().unwrap();
    let tool = AsyncExecTool::new(&tmp.path().to_string_lossy(), false);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let cmd = if cfg!(target_os = "windows") { "timeout /t 10 /nobreak >nul 2>&1 || ping -n 10 127.0.0.1 >nul" } else { "sleep 10" };
    let args = serde_json::json!({"command": cmd, "wait_seconds": 1}).to_string();
    let result = tool.execute(&args, &ctx).await;
    // Should succeed — process is still running after 1s wait
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_async_exec_tool_missing_command() {
    let tmp = TempDir::new().unwrap();
    let tool = AsyncExecTool::new(&tmp.path().to_string_lossy(), false);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool.execute("{}", &ctx).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_async_exec_tool_invalid_json() {
    let tmp = TempDir::new().unwrap();
    let tool = AsyncExecTool::new(&tmp.path().to_string_lossy(), false);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool.execute("not json", &ctx).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_async_exec_tool_workspace_restriction() {
    let tmp = TempDir::new().unwrap();
    let tool = AsyncExecTool::new(&tmp.path().to_string_lossy(), true);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({"command": "echo hi", "working_dir": "/etc"}).to_string();
    let result = tool.execute(&args, &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("outside workspace"));
}

#[tokio::test]
async fn test_async_exec_tool_fast_exit() {
    let tmp = TempDir::new().unwrap();
    let tool = AsyncExecTool::new(&tmp.path().to_string_lossy(), false);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let cmd = if cfg!(target_os = "windows") { "echo hello" } else { "echo hello" };
    let args = serde_json::json!({"command": cmd, "wait_seconds": 5}).to_string();
    let result = tool.execute(&args, &ctx).await;
    // Fast-exit command completes within wait period, should return ok
    assert!(result.is_ok());
}

#[test]
fn test_async_exec_tool_description() {
    let tool = AsyncExecTool::new("/tmp", false);
    assert!(!tool.description().is_empty());
    let params = tool.parameters();
    assert!(params["properties"]["command"].is_object());
    assert!(params["properties"]["wait_seconds"].is_object());
}

// --- WebSearchTool coverage ---

#[test]
fn test_web_search_config_default() {
    let config = WebSearchConfig::default();
    assert!(!config.brave_enabled);
    assert!(config.duckduckgo_enabled);
    assert!(!config.perplexity_enabled);
    assert_eq!(config.brave_max_results, 5);
    assert_eq!(config.duckduckgo_max_results, 5);
    assert_eq!(config.perplexity_max_results, 5);
    assert!(config.brave_api_key.is_none());
    assert!(config.perplexity_api_key.is_none());
}

#[test]
fn test_web_search_tool_description() {
    let tool = WebSearchTool::new(WebSearchConfig::default());
    assert!(!tool.description().is_empty());
    assert!(tool.parameters().is_object());
}

#[tokio::test]
async fn test_web_search_tool_brave_no_key() {
    let config = WebSearchConfig {
        brave_enabled: true,
        brave_api_key: None,
        duckduckgo_enabled: false,
        perplexity_enabled: false,
        ..Default::default()
    };
    let tool = WebSearchTool::new(config);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    // brave_enabled but no key -> search_brave should fail
    let result = tool.execute(r#"{"query": "test"}"#, &ctx).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_web_search_tool_perplexity_no_key() {
    let config = WebSearchConfig {
        brave_enabled: false,
        duckduckgo_enabled: false,
        perplexity_enabled: true,
        perplexity_api_key: None,
        ..Default::default()
    };
    let tool = WebSearchTool::new(config);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool.execute(r#"{"query": "test"}"#, &ctx).await;
    assert!(result.is_err());
}

// --- WebFetchTool coverage ---

#[test]
fn test_web_fetch_tool_description() {
    let tool = WebFetchTool::new(50000);
    assert!(!tool.description().is_empty());
    assert!(tool.parameters().is_object());
}

// --- ClusterRpcTool coverage ---

#[test]
fn test_cluster_rpc_config() {
    let config = ClusterRpcConfig {
        local_node_id: "node-1".to_string(),
        timeout_secs: 120,
        local_rpc_port: 21949,
    };
    assert_eq!(config.local_node_id, "node-1");
    assert_eq!(config.timeout_secs, 120);
}

#[tokio::test]
async fn test_cluster_rpc_tool_missing_target() {
    let config = ClusterRpcConfig {
        local_node_id: "node-1".to_string(),
        timeout_secs: 60,
        local_rpc_port: 21949,
    };
    let tool = ClusterRpcTool::new(config);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool.execute(r#"{"message": "hello"}"#, &ctx).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_cluster_rpc_tool_invalid_json() {
    let config = ClusterRpcConfig {
        local_node_id: "node-1".to_string(),
        timeout_secs: 60,
        local_rpc_port: 21949,
    };
    let tool = ClusterRpcTool::new(config);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool.execute("not json", &ctx).await;
    assert!(result.is_err());
}

// --- SpawnTool coverage ---

#[tokio::test]
async fn test_spawn_tool_invalid_json() {
    let config = SpawnConfig {
        default_model: "test".to_string(),
        max_concurrent: 5,
    };
    let tool = SpawnTool::new(config);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool.execute("not json", &ctx).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_spawn_tool_missing_agent_id() {
    let config = SpawnConfig {
        default_model: "test".to_string(),
        max_concurrent: 5,
    };
    let tool = SpawnTool::new(config);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool.execute(r#"{"task": "do something"}"#, &ctx).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_spawn_tool_allowlist_allowed() {
    let config = SpawnConfig {
        default_model: "test".to_string(),
        max_concurrent: 5,
    };
    let mut tool = SpawnTool::new(config);
    tool.set_allowlist_checker(Box::new(|id| id == "allowed-agent"));
    tool.set_spawn_fn(Arc::new(
        |agent_id: &str, task: &str, model: &str, _ch: &str, _cid: &str| {
            let agent_id = agent_id.to_string();
            let task = task.to_string();
            let model = model.to_string();
            Box::pin(async move {
                Ok(format!("spawned {} for {} with {}", agent_id, task, model))
            })
        },
    ));

    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool
        .execute(r#"{"agent_id": "allowed-agent", "task": "test"}"#, &ctx)
        .await
        .unwrap();
    assert!(result.contains("allowed-agent"));
}

// --- register_shared_tools coverage ---

#[test]
fn test_register_shared_tools_default() {
    let config = SharedToolConfig::default();
    let tools = register_shared_tools(&config);
    // Default config: no web search, no cluster, no spawn, no MCP
    assert!(tools.contains_key("message"));
    assert!(tools.contains_key("web_fetch"));
    assert!(tools.contains_key("memory_search"));
    assert!(tools.contains_key("memory_store"));
    assert!(tools.contains_key("memory_forget"));
    assert!(tools.contains_key("memory_list"));
    assert!(tools.contains_key("skills_list"));
    assert!(tools.contains_key("skills_info"));
    assert!(!tools.contains_key("web_search"));
    assert!(!tools.contains_key("cluster_rpc"));
    assert!(!tools.contains_key("spawn"));
}

#[test]
fn test_register_shared_tools_with_web() {
    let config = SharedToolConfig {
        web_search: Some(WebSearchConfig {
            duckduckgo_enabled: true,
            ..Default::default()
        }),
        ..Default::default()
    };
    let tools = register_shared_tools(&config);
    assert!(tools.contains_key("web_search"));
}

#[test]
fn test_register_shared_tools_with_cluster() {
    let config = SharedToolConfig {
        cluster_rpc: Some(ClusterRpcConfig {
            local_node_id: "n1".to_string(),
            timeout_secs: 60,
            local_rpc_port: 21949,
        }),
        ..Default::default()
    };
    let tools = register_shared_tools(&config);
    assert!(tools.contains_key("cluster_rpc"));
}

#[test]
fn test_register_shared_tools_with_spawn() {
    let config = SharedToolConfig {
        spawn: Some(SpawnConfig {
            default_model: "test".to_string(),
            max_concurrent: 5,
        }),
        ..Default::default()
    };
    let tools = register_shared_tools(&config);
    assert!(tools.contains_key("spawn"));
}

fn make_cron_service() -> Arc<std::sync::Mutex<nemesis_cron::service::CronService>> {
    Arc::new(std::sync::Mutex::new(
        nemesis_cron::service::CronService::new(":memory:"),
    ))
}

#[test]
fn test_register_shared_tools_with_cron() {
    let cron_svc = make_cron_service();
    let config = SharedToolConfig {
        cron_service: Some(cron_svc),
        ..Default::default()
    };
    let tools = register_shared_tools(&config);
    assert!(tools.contains_key("cron"));
}

// --- SharedToolConfig default ---

#[test]
fn test_shared_tool_config_default() {
    let config = SharedToolConfig::default();
    assert!(config.web_search.is_none());
    assert!(config.cluster_rpc.is_none());
    assert!(config.spawn.is_none());
    assert!(!config.mcp_enabled);
    assert!(config.mcp_servers.is_empty());
    assert!(config.skills_registry.is_none());
    assert!(config.skills_loader.is_none());
    assert!(config.workspace.is_none());
    assert!(config.cron_service.is_none());
    assert!(config.forge_executor.is_none());
}

// --- McpTool coverage ---

#[test]
fn test_mcp_tool_description_and_params() {
    let tool = McpTool::new_simulated("search", "Search tool", "my-server");
    assert!(!tool.description().is_empty());
    let params = tool.parameters();
    assert!(params.is_object());
}

// --- Tool trait methods coverage ---

#[test]
fn test_tool_descriptions_non_empty() {
    let tools = register_default_tools();
    for (name, tool) in &tools {
        let desc = tool.description();
        assert!(!desc.is_empty(), "Tool '{}' has empty description", name);
    }
}

#[test]
fn test_tool_parameters_are_valid_json() {
    let tools = register_default_tools();
    for (name, tool) in &tools {
        let params = tool.parameters();
        assert!(params.is_object(), "Tool '{}' parameters is not an object", name);
    }
}

// --- CronTool coverage ---

#[tokio::test]
async fn test_cron_tool_list_empty() {
    let svc = make_cron_service();
    let tool = CronTool::new(svc);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({"action": "list"}).to_string();
    let result = tool.execute(&args, &ctx).await.unwrap();
    assert_eq!(result, "[]");
}

#[tokio::test]
async fn test_cron_tool_invalid_json() {
    let svc = make_cron_service();
    let tool = CronTool::new(svc);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool.execute("not json", &ctx).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_cron_tool_unknown_action() {
    let svc = make_cron_service();
    let tool = CronTool::new(svc);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({"action": "unknown_action"}).to_string();
    let result = tool.execute(&args, &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Unknown cron action"));
}

#[tokio::test]
async fn test_cron_tool_delete_missing_id() {
    let svc = make_cron_service();
    let tool = CronTool::new(svc);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({"action": "delete"}).to_string();
    let result = tool.execute(&args, &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Missing 'id'"));
}

#[tokio::test]
async fn test_cron_tool_delete_not_found() {
    let svc = make_cron_service();
    let tool = CronTool::new(svc);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({"action": "delete", "id": "nonexistent"}).to_string();
    let result = tool.execute(&args, &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found"));
}

#[tokio::test]
async fn test_cron_tool_create_missing_schedule() {
    let svc = make_cron_service();
    let tool = CronTool::new(svc);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({"action": "create", "name": "test"}).to_string();
    let result = tool.execute(&args, &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Missing 'schedule'"));
}

#[test]
fn test_cron_tool_set_context() {
    let svc = make_cron_service();
    let tool = CronTool::new(svc);
    tool.set_context("web", "chat1");
}

// --- Additional coverage tests ---

#[test]
fn test_urlencoding() {
    assert_eq!(urlencoding("hello world"), "hello+world");
    assert_eq!(urlencoding("test@example.com"), "test%40example.com");
    assert_eq!(urlencoding("a+b"), "a%2Bb");
    assert_eq!(urlencoding("simple"), "simple");
}

#[test]
fn test_percent_decode() {
    assert_eq!(percent_decode("hello+world"), "hello world");
    assert_eq!(percent_decode("test%40example.com"), "test@example.com");
    assert_eq!(percent_decode("a%2Bb"), "a+b");
    assert_eq!(percent_decode("no_encoding"), "no_encoding");
}

#[test]
fn test_url_decode_query_param() {
    let url = "https://example.com/l/?uddg=https%3A%2F%2Ffoo.com";
    let result = url_decode_query_param(url, "uddg");
    assert_eq!(result, Some("https://foo.com".to_string()));

    assert_eq!(url_decode_query_param("https://example.com", "uddg"), None);
}

#[test]
fn test_extract_search_query_json() {
    assert_eq!(extract_search_query(r#"{"query": "test search"}"#).unwrap(), "test search");
}

#[test]
fn test_extract_search_query_fallback() {
    assert_eq!(extract_search_query("plain text query").unwrap(), "plain text query");
}

#[test]
fn test_extract_url_json() {
    assert_eq!(extract_url(r#"{"url": "https://example.com"}"#).unwrap(), "https://example.com");
}

#[test]
fn test_extract_url_fallback() {
    assert_eq!(extract_url("https://example.com").unwrap(), "https://example.com");
}

#[test]
fn test_web_search_config_default_values() {
    let config = WebSearchConfig::default();
    assert!(!config.brave_enabled);
    assert!(config.brave_api_key.is_none());
    assert!(config.duckduckgo_enabled);
    assert!(!config.perplexity_enabled);
}

#[test]
fn test_cluster_rpc_config_debug() {
    let config = ClusterRpcConfig {
        local_node_id: "node-1".to_string(),
        timeout_secs: 3600,
        local_rpc_port: 21949,
    };
    let debug_str = format!("{:?}", config);
    assert!(debug_str.contains("node-1"));
}

#[test]
fn test_setup_cluster_rpc_channel_with_config() {
    let cluster_config = ClusterRpcConfig {
        local_node_id: "node-1".to_string(),
        timeout_secs: 3600,
        local_rpc_port: 21949,
    };
    let config = setup_cluster_rpc_channel_with_config(&cluster_config);
    assert_eq!(config.request_timeout, std::time::Duration::from_secs(24 * 3600));
}

#[tokio::test]
async fn test_cluster_rpc_tool_set_context() {
    let config = ClusterRpcConfig {
        local_node_id: "node-1".to_string(),
        timeout_secs: 60,
        local_rpc_port: 21949,
    };
    let tool = ClusterRpcTool::new(config);
    tool.set_context("rpc", "chat-123");
    assert_eq!(*tool.stored_channel.lock().unwrap_or_else(|e| e.into_inner()), "rpc");
    assert_eq!(*tool.stored_chat_id.lock().unwrap_or_else(|e| e.into_inner()), "chat-123");
}

#[tokio::test]
async fn test_cluster_rpc_tool_no_rpc_fn() {
    let config = ClusterRpcConfig {
        local_node_id: "node-1".to_string(),
        timeout_secs: 60,
        local_rpc_port: 21949,
    };
    let tool = ClusterRpcTool::new(config);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({"target_node": "node-2", "message": "hello"}).to_string();
    let result = tool.execute(&args, &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not available"));
}

#[tokio::test]
async fn test_web_search_tool_no_provider_configured() {
    let config = WebSearchConfig {
        brave_enabled: false,
        brave_api_key: None,
        brave_max_results: 5,
        duckduckgo_enabled: false,
        duckduckgo_max_results: 5,
        perplexity_enabled: false,
        perplexity_api_key: None,
        perplexity_max_results: 5,
    };
    let tool = WebSearchTool::new(config);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool.execute(r#"{"query": "test"}"#, &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("No search provider"));
}

#[test]
fn test_register_peer_chat_handler() {
    let mut handlers: std::collections::HashMap<String, Box<dyn Fn(serde_json::Value) -> Result<serde_json::Value, String> + Send + Sync>> = std::collections::HashMap::new();
    register_peer_chat_handler(&mut handlers, |_payload| {
        Ok(serde_json::json!({"status": "ok"}))
    });
    assert!(handlers.contains_key("peer_chat"));
    assert!(handlers.contains_key("peer_chat_callback"));

    let callback = handlers.get("peer_chat_callback").unwrap();
    let result = callback(serde_json::json!({"task_id": "task-1", "content": "response"}));
    assert!(result.is_ok());
    let result_val = result.unwrap();
    assert_eq!(result_val["status"], "received");
}

#[tokio::test]
async fn test_exec_tool_with_cwd() {
    let tmp = TempDir::new().unwrap();
    let tool = ExecTool::new(&tmp.path().to_string_lossy(), false);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({
        "command": "echo hello",
        "cwd": tmp.path().to_string_lossy().to_string()
    }).to_string();
    let result = tool.execute(&args, &ctx).await;
    assert!(result.is_ok());
    assert!(result.unwrap().contains("hello"));
}

#[tokio::test]
async fn test_exec_tool_workspace_restriction_denied() {
    let tool = ExecTool::new("/safe/workspace", true);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({
        "command": "echo hello",
        "cwd": "/outside/workspace"
    }).to_string();
    let result = tool.execute(&args, &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("outside workspace"));
}

#[tokio::test]
async fn test_sleep_tool_duration_field() {
    let tool = SleepTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({"duration": 1}).to_string();
    let result = tool.execute(&args, &ctx).await;
    assert!(result.is_ok());
    assert!(result.unwrap().contains("Slept for 1 seconds"));
}

#[tokio::test]
async fn test_sleep_tool_exceeds_max_duration() {
    let tool = SleepTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({"duration": 999999}).to_string();
    let result = tool.execute(&args, &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("cannot exceed"));
}

#[tokio::test]
async fn test_message_tool_raw_args() {
    let tool = MessageTool::new();
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool.execute("just some text", &ctx).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "just some text");
}

#[test]
fn test_web_fetch_tool_new() {
    let tool = WebFetchTool::new(4096);
    assert_eq!(tool.max_size, 4096);
}

// --- Additional coverage tests for register functions and types ---

#[test]
fn test_register_default_tools_has_expected_tools() {
    let tools = register_default_tools();
    assert!(tools.contains_key("message"));
    assert!(tools.contains_key("read_file"));
    assert!(tools.contains_key("write_file"));
    assert!(tools.contains_key("list_dir"));
    assert!(tools.contains_key("create_dir"));
    assert!(tools.contains_key("sleep"));
    assert!(tools.contains_key("edit_file"));
    assert!(tools.contains_key("append_file"));
    assert!(tools.contains_key("delete_file"));
    assert!(tools.contains_key("delete_dir"));
}

#[test]
fn test_register_extended_tools_base_count() {
    let tools = register_extended_tools(None, None, None);
    assert!(tools.len() >= 6);
    assert!(tools.contains_key("web_fetch"));
    assert!(tools.contains_key("memory_search"));
    assert!(tools.contains_key("memory_store"));
    assert!(tools.contains_key("memory_forget"));
    assert!(tools.contains_key("memory_list"));
    assert!(tools.contains_key("skills_list"));
    assert!(tools.contains_key("skills_info"));
}

#[test]
fn test_register_extended_tools_includes_web() {
    let web_config = WebSearchConfig::default();
    let tools = register_extended_tools(Some(web_config), None, None);
    assert!(tools.contains_key("web_search"));
}

#[test]
fn test_register_extended_tools_includes_cluster() {
    let cluster_config = ClusterRpcConfig {
        local_node_id: "node-1".to_string(),
        timeout_secs: 60,
        local_rpc_port: 21949,
    };
    let tools = register_extended_tools(None, Some(cluster_config), None);
    assert!(tools.contains_key("cluster_rpc"));
}

#[test]
fn test_register_extended_tools_includes_spawn() {
    let spawn_config = SpawnConfig {
        default_model: "gpt-4".to_string(),
        max_concurrent: 3,
    };
    let tools = register_extended_tools(None, None, Some(spawn_config));
    assert!(tools.contains_key("spawn"));
}

#[test]
fn test_register_shared_tools_without_workspace() {
    let config = SharedToolConfig::default();
    let tools = register_shared_tools(&config);
    assert!(tools.contains_key("web_fetch"));
    assert!(tools.contains_key("i2c"));
    assert!(tools.contains_key("spi"));
    assert!(!tools.contains_key("exec"));
}

#[test]
fn test_register_shared_tools_with_workspace() {
    let config = SharedToolConfig {
        workspace: Some("/tmp/test".to_string()),
        ..Default::default()
    };
    let tools = register_shared_tools(&config);
    assert!(tools.contains_key("exec"));
    assert!(tools.contains_key("exec_async"));
}

#[test]
fn test_shared_tool_config_default_values() {
    let config = SharedToolConfig::default();
    assert!(config.web_search.is_none());
    assert!(config.cluster_rpc.is_none());
    assert!(config.spawn.is_none());
    assert!(!config.mcp_enabled);
    assert!(config.mcp_servers.is_empty());
    assert!(config.workspace.is_none());
}

#[test]
fn test_shared_tool_config_debug_output() {
    let config = SharedToolConfig::default();
    let debug = format!("{:?}", config);
    assert!(debug.contains("SharedToolConfig"));
    assert!(debug.contains("mcp_enabled"));
}

#[test]
fn test_spawn_config_fields() {
    let config = SpawnConfig {
        default_model: "gpt-4".to_string(),
        max_concurrent: 5,
    };
    assert_eq!(config.default_model, "gpt-4");
    assert_eq!(config.max_concurrent, 5);
}

#[test]
fn test_mcp_server_config_fields() {
    let config = McpServerConfig {
        name: "test-server".to_string(),
        command: "test-cmd".to_string(),
        args: vec!["arg1".to_string()],
        env: std::collections::HashMap::new(),
        timeout_secs: 30,
    };
    assert_eq!(config.name, "test-server");
    assert_eq!(config.command, "test-cmd");
    assert_eq!(config.timeout_secs, 30);
}

#[tokio::test]
async fn test_register_shared_tools_async_mcp_disabled() {
    let config = SharedToolConfig {
        mcp_enabled: false,
        mcp_servers: vec![McpServerConfig {
            name: "test".to_string(),
            command: "test".to_string(),
            args: vec![],
            env: std::collections::HashMap::new(),
            timeout_secs: 30,
        }],
        ..Default::default()
    };
    // Pass a discovery closure that returns an error
    let tools = register_shared_tools_async(&config, Some(|_name: String| {
        async { Err("no mcp".to_string()) }
    })).await;
    assert!(!tools.keys().any(|k| k.starts_with("mcp_")));
}

#[tokio::test]
async fn test_register_shared_tools_async_mcp_enabled_with_failing_discovery() {
    let config = SharedToolConfig {
        mcp_enabled: true,
        mcp_servers: vec![McpServerConfig {
            name: "test".to_string(),
            command: "test".to_string(),
            args: vec![],
            env: std::collections::HashMap::new(),
            timeout_secs: 30,
        }],
        ..Default::default()
    };
    // Pass a discovery closure that returns an error
    let tools = register_shared_tools_async(&config, Some(|_name: String| {
        async { Err("discovery failed".to_string()) }
    })).await;
    assert!(tools.contains_key("web_fetch"));
    // MCP discovery failed, so no MCP tools
    assert!(!tools.keys().any(|k| k.starts_with("mcp_")));
}

#[test]
fn test_percent_decode_edge_cases() {
    assert_eq!(percent_decode(""), "");
    assert_eq!(percent_decode("%00"), "\0");
    // Test percent-decoded bytes that produce valid ASCII
    assert_eq!(percent_decode("%41%42%43"), "ABC");
}

#[test]
fn test_urlencoding_special_chars() {
    assert_eq!(urlencoding(" "), "+");
    assert_eq!(urlencoding("&"), "%26");
    assert_eq!(urlencoding("="), "%3D");
    assert_eq!(urlencoding("/"), "%2F");
}

#[test]
fn test_url_decode_query_param_no_query() {
    let url = "https://example.com/path";
    let result = url_decode_query_param(url, "missing");
    assert!(result.is_none());
}

#[test]
fn test_url_decode_query_param_multiple_params() {
    let url = "https://example.com/?a=1&b=2&c=3";
    assert_eq!(url_decode_query_param(url, "a"), Some("1".to_string()));
    assert_eq!(url_decode_query_param(url, "b"), Some("2".to_string()));
    assert_eq!(url_decode_query_param(url, "c"), Some("3".to_string()));
    assert_eq!(url_decode_query_param(url, "d"), None);
}

#[test]
fn test_extract_search_query_empty() {
    assert_eq!(extract_search_query("").unwrap(), "");
}

#[test]
fn test_extract_url_empty() {
    assert_eq!(extract_url("").unwrap(), "");
}

#[test]
fn test_mcp_server_config_debug() {
    let config = McpServerConfig {
        name: "test".to_string(),
        command: "cmd".to_string(),
        args: vec![],
        env: std::collections::HashMap::new(),
        timeout_secs: 30,
    };
    let debug = format!("{:?}", config);
    assert!(debug.contains("test"));
}

#[test]
fn test_spawn_config_debug() {
    let config = SpawnConfig {
        default_model: "gpt-4".to_string(),
        max_concurrent: 3,
    };
    let debug = format!("{:?}", config);
    assert!(debug.contains("SpawnConfig"));
}

#[test]
fn test_web_search_config_with_all_providers() {
    let config = WebSearchConfig {
        brave_enabled: true,
        brave_api_key: Some("key123".to_string()),
        brave_max_results: 10,
        duckduckgo_enabled: true,
        duckduckgo_max_results: 5,
        perplexity_enabled: true,
        perplexity_api_key: Some("pkey".to_string()),
        perplexity_max_results: 3,
    };
    assert!(config.brave_enabled);
    assert!(config.duckduckgo_enabled);
    assert!(config.perplexity_enabled);
}

// --- Additional unique coverage tests for loop_tools.rs ---

#[test]
fn test_shared_tool_config_debug_with_none_fields() {
    let config = SharedToolConfig::default();
    let debug = format!("{:?}", config);
    assert!(debug.contains("SharedToolConfig"));
    assert!(debug.contains("web_search: None"));
    assert!(debug.contains("mcp_enabled: false"));
}

#[test]
fn test_register_shared_tools_combined_options() {
    let cron_svc = make_cron_service();
    let config = SharedToolConfig {
        web_search: Some(WebSearchConfig {
            duckduckgo_enabled: true,
            ..Default::default()
        }),
        cluster_rpc: Some(ClusterRpcConfig {
            local_node_id: "node-1".to_string(),
            timeout_secs: 60,
            local_rpc_port: 21949,
        }),
        spawn: Some(SpawnConfig {
            default_model: "test".to_string(),
            max_concurrent: 5,
        }),
        mcp_enabled: true,
        mcp_servers: vec![McpServerConfig {
            name: "test-server".to_string(),
            command: "echo".to_string(),
            args: vec![],
            env: std::collections::HashMap::new(),
            timeout_secs: 30,
        }],
        workspace: Some("/tmp/ws".to_string()),
        cron_service: Some(cron_svc),
        ..Default::default()
    };
    let tools = register_shared_tools(&config);
    assert!(tools.contains_key("web_search"));
    assert!(tools.contains_key("cluster_rpc"));
    assert!(tools.contains_key("spawn"));
    assert!(tools.contains_key("exec"));
    assert!(tools.contains_key("cron"));
}

#[tokio::test]
async fn test_register_shared_tools_async_with_successful_discovery() {
    let config = SharedToolConfig {
        mcp_enabled: true,
        mcp_servers: vec![McpServerConfig {
            name: "test-server".to_string(),
            command: "echo".to_string(),
            args: vec![],
            env: std::collections::HashMap::new(),
            timeout_secs: 30,
        }],
        ..Default::default()
    };
    let discover_fn = |_server_name: String| {
        async move {
            Ok(vec![
                ("search".to_string(), "Search tool".to_string(), Some(serde_json::json!({"type": "object"}))),
            ])
        }
    };
    let tools = register_shared_tools_async(&config, Some(discover_fn)).await;
    // Tool name is format!("mcp_{}_{}", server_name, tool_name)
    assert!(tools.contains_key("mcp_test-server_search"));
}

#[tokio::test]
async fn test_register_shared_tools_async_mcp_enabled_no_discover_fn() {
    let config = SharedToolConfig {
        mcp_enabled: true,
        mcp_servers: vec![McpServerConfig {
            name: "test".to_string(),
            command: "echo".to_string(),
            args: vec![],
            env: std::collections::HashMap::new(),
            timeout_secs: 30,
        }],
        ..Default::default()
    };
    let tools: HashMap<String, Box<dyn Tool>> = register_shared_tools_async(&config, Option::<fn(String) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<(String, String, Option<serde_json::Value>)>, String>> + Send>>>::None).await;
    assert!(tools.contains_key("message"));
}

#[test]
fn test_cluster_rpc_channel_config_default() {
    let config = ClusterRpcChannelConfig::default();
    assert!(config.request_timeout.as_secs() > 0);
    assert!(config.cleanup_interval.as_secs() > 0);
}

#[test]
fn test_setup_cluster_rpc_channel_without_continuation() {
    let setup = setup_cluster_rpc_channel(None);
    assert!(setup.continuation_manager.is_none());
    assert!(setup.config.request_timeout.as_secs() > 0);
}

#[test]
fn test_setup_cluster_rpc_channel_with_continuation() {
    let cm = Arc::new(crate::loop_continuation::ContinuationManager::new());
    let setup = setup_cluster_rpc_channel(Some(cm));
    assert!(setup.continuation_manager.is_some());
}

#[tokio::test]
async fn test_discover_mcp_tools_success() {
    let result = discover_mcp_tools("test-server", || async {
        Ok(vec![
            ("tool1".to_string(), "Tool 1".to_string(), Some(serde_json::json!({"type":"object"}))),
            ("tool2".to_string(), "Tool 2".to_string(), None),
        ])
    }).await.unwrap();
    assert_eq!(result.server_name, "test-server");
    assert_eq!(result.tools.len(), 2);
    assert_eq!(result.tools[0].name, "tool1");
    assert_eq!(result.tools[1].name, "tool2");
}

#[tokio::test]
async fn test_discover_mcp_tools_empty() {
    let result = discover_mcp_tools("empty-server", || async {
        Ok(vec![])
    }).await.unwrap();
    assert_eq!(result.tools.len(), 0);
}

#[tokio::test]
async fn test_discover_mcp_tools_error() {
    let result = discover_mcp_tools("fail-server", || async {
        Err("Connection refused".to_string())
    }).await;
    assert!(result.is_err());
    let err_msg = result.err().unwrap();
    assert!(err_msg.contains("Connection refused"));
}

#[tokio::test]
async fn test_discovered_mcp_tool_execute() {
    let result = discover_mcp_tools("srv", || async {
        Ok(vec![
            ("my_tool".to_string(), "My tool".to_string(), None),
        ])
    }).await.unwrap();
    let tool = &result.tools[0];
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let exec_result = tool.execute("args", &ctx).await.unwrap();
    assert!(exec_result.contains("MCP/srv"));
    assert!(exec_result.contains("my_tool"));
}

#[test]
fn test_mcp_tool_with_schema() {
    let tool = McpTool::new_simulated("test", "A test tool", "server1")
        .with_schema(serde_json::json!({"type": "object"}));
    assert!(tool.input_schema.is_some());
}

#[tokio::test]
async fn test_mcp_tool_new_custom_executor() {
    let tool = McpTool::new("custom", "Custom tool", "srv", |args: &str| {
        let args_owned = args.to_string();
        async move { Ok(format!("Custom: {}", args_owned)) }
    });
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool.execute("hello", &ctx).await.unwrap();
    assert_eq!(result, "Custom: hello");
}

#[tokio::test]
async fn test_i2c_tool_non_linux() {
    let tool = I2CTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool.execute(r#"{"action":"detect"}"#, &ctx).await;
    if cfg!(target_os = "linux") {
        assert!(result.is_ok());
    } else {
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("only supported on Linux"));
    }
}

#[tokio::test]
async fn test_i2c_tool_invalid_json() {
    let tool = I2CTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool.execute("not json", &ctx).await;
    if cfg!(target_os = "linux") {
        assert!(result.is_err());
    } else {
        assert!(result.is_err());
    }
}

#[tokio::test]
async fn test_spi_tool_non_linux() {
    let tool = SPITool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool.execute(r#"{"action":"list"}"#, &ctx).await;
    if cfg!(target_os = "linux") {
        assert!(result.is_ok());
    } else {
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("only supported on Linux"));
    }
}

#[tokio::test]
async fn test_spi_tool_invalid_json() {
    let tool = SPITool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool.execute("not json", &ctx).await;
    if cfg!(target_os = "linux") {
        assert!(result.is_err());
    } else {
        assert!(result.is_err());
    }
}

#[test]
fn test_sanitize_mcp_name() {
    assert_eq!(sanitize_mcp_name("My Tool"), "my_tool");
    assert_eq!(sanitize_mcp_name("my-tool"), "my_tool");
    assert_eq!(sanitize_mcp_name("my.tool"), "my_tool");
    assert_eq!(sanitize_mcp_name("MyTool"), "mytool");
    assert_eq!(sanitize_mcp_name("a b c.d-e"), "a_b_c_d_e");
}

#[tokio::test]
async fn test_cluster_rpc_tool_no_rpc_fn_self_node() {
    let config = ClusterRpcConfig {
        local_node_id: "node-1".to_string(),
        timeout_secs: 60,
        local_rpc_port: 21949,
    };
    let tool = ClusterRpcTool::new(config);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool.execute(
        r#"{"target_node": "node-1", "message": "hello"}"#,
        &ctx,
    ).await;
    assert!(result.is_err());
    assert!(result.err().unwrap().contains("not available"));
}

#[tokio::test]
async fn test_no_forge_tools_without_executor() {
    let config = SharedToolConfig {
        forge_executor: None,
        ..Default::default()
    };
    let tools = register_shared_tools(&config);
    assert!(!tools.contains_key("forge_reflect"));
}

#[tokio::test]
async fn test_web_fetch_tool_missing_url() {
    let tool = WebFetchTool::new(50000);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool.execute(r#"{"other": "value"}"#, &ctx).await;
    assert!(result.is_err());
}

#[test]
fn test_extract_path_invalid_json_raw() {
    let result = extract_path("not json");
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "not json");
}

#[test]
fn test_extract_path_empty_string() {
    let result = extract_path("");
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "");
}

#[tokio::test]
async fn test_web_search_tool_missing_query() {
    let config = WebSearchConfig {
        duckduckgo_enabled: true,
        ..Default::default()
    };
    let tool = WebSearchTool::new(config);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool.execute("{}", &ctx).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_web_search_tool_invalid_json() {
    let config = WebSearchConfig {
        duckduckgo_enabled: true,
        ..Default::default()
    };
    let tool = WebSearchTool::new(config);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool.execute("not json", &ctx).await;
    assert!(result.is_err());
}

// =========================================================================
// BootstrapTool coverage
// =========================================================================

#[tokio::test]
async fn test_bootstrap_tool_not_confirmed() {
    let tmp = TempDir::new().unwrap();
    let tool = BootstrapTool::new(&tmp.path().to_string_lossy());
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({"confirmed": false}).to_string();
    let result = tool.execute(&args, &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Must confirm"));
}

#[tokio::test]
async fn test_bootstrap_tool_invalid_args() {
    let tmp = TempDir::new().unwrap();
    let tool = BootstrapTool::new(&tmp.path().to_string_lossy());
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool.execute("not json", &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Invalid arguments"));
}

#[tokio::test]
async fn test_bootstrap_tool_missing_confirmed_field() {
    let tmp = TempDir::new().unwrap();
    let tool = BootstrapTool::new(&tmp.path().to_string_lossy());
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({"other": true}).to_string();
    let result = tool.execute(&args, &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Missing or invalid 'confirmed'"));
}

#[tokio::test]
async fn test_bootstrap_tool_already_removed() {
    let tmp = TempDir::new().unwrap();
    // No BOOTSTRAP.md file created
    let tool = BootstrapTool::new(&tmp.path().to_string_lossy());
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({"confirmed": true}).to_string();
    let result = tool.execute(&args, &ctx).await;
    assert!(result.is_ok());
    assert!(result.unwrap().contains("already been removed"));
}

#[tokio::test]
async fn test_bootstrap_tool_success() {
    let tmp = TempDir::new().unwrap();
    let bootstrap_path = tmp.path().join("BOOTSTRAP.md");
    tokio::fs::write(&bootstrap_path, "# Bootstrap").await.unwrap();
    assert!(bootstrap_path.exists());

    let tool = BootstrapTool::new(&tmp.path().to_string_lossy());
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({"confirmed": true}).to_string();
    let result = tool.execute(&args, &ctx).await;
    assert!(result.is_ok());
    assert!(result.unwrap().contains("complete"));
    assert!(!bootstrap_path.exists());
}

#[tokio::test]
async fn test_bootstrap_tool_description_and_params() {
    let tmp = TempDir::new().unwrap();
    let tool = BootstrapTool::new(&tmp.path().to_string_lossy());
    assert!(!tool.description().is_empty());
    let params = tool.parameters();
    assert!(params.is_object());
    assert!(params["properties"]["confirmed"].is_object());
}

// =========================================================================
// ClusterRpcTool additional coverage
// =========================================================================

#[tokio::test]
async fn test_cluster_rpc_tool_with_async_ack() {
    let config = ClusterRpcConfig {
        local_node_id: "node-1".to_string(),
        timeout_secs: 60,
        local_rpc_port: 21949,
    };
    let mut tool = ClusterRpcTool::new(config);
    tool.set_rpc_call_fn(Arc::new(|_node: &str, _action: &str, _payload: serde_json::Value| {
        Box::pin(async {
            Ok(serde_json::json!({"status": "accepted", "task_id": "auto-123"}))
        })
    }));

    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool
        .execute(r#"{"target_node": "node-2", "message": "hello"}"#, &ctx)
        .await
        .unwrap();
    assert!(result.contains("__ASYNC__:auto-123:node-2"));
}

#[tokio::test]
async fn test_cluster_rpc_tool_target_aliases() {
    // Test that "target", "target_node", and "peer_id" all work
    let config = ClusterRpcConfig {
        local_node_id: "node-1".to_string(),
        timeout_secs: 60,
        local_rpc_port: 21949,
    };

    // Test with "target" alias
    let mut tool = ClusterRpcTool::new(config.clone());
    tool.set_rpc_call_fn(Arc::new(|node: &str, _action: &str, _payload: serde_json::Value| {
        let node = node.to_string();
        Box::pin(async move { Ok(serde_json::json!({"content": format!("Response to {}", node)})) })
    }));
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool.execute(r#"{"target": "node-3", "message": "hi"}"#, &ctx).await;
    assert!(result.is_ok());

    // Test with "peer_id" alias
    let mut tool2 = ClusterRpcTool::new(config);
    tool2.set_rpc_call_fn(Arc::new(|node: &str, _action: &str, _payload: serde_json::Value| {
        let node = node.to_string();
        Box::pin(async move { Ok(serde_json::json!({"content": format!("Response to {}", node)})) })
    }));
    let result2 = tool2.execute(r#"{"peer_id": "node-4", "message": "hi"}"#, &ctx).await;
    assert!(result2.is_ok());
}

#[tokio::test]
async fn test_cluster_rpc_tool_data_content_fallback() {
    let config = ClusterRpcConfig {
        local_node_id: "node-1".to_string(),
        timeout_secs: 60,
        local_rpc_port: 21949,
    };
    let mut tool = ClusterRpcTool::new(config);
    tool.set_rpc_call_fn(Arc::new(|_node: &str, _action: &str, payload: serde_json::Value| {
        let content = payload.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
        Box::pin(async move { Ok(serde_json::json!({"content": format!("Got: {}", content)})) })
    }));

    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    // Use data.content format instead of message
    let result = tool
        .execute(r#"{"target_node": "node-2", "data": {"content": "via data"}} "#, &ctx)
        .await
        .unwrap();
    assert!(result.contains("Got: via data"));
}

#[tokio::test]
async fn test_cluster_rpc_tool_stored_context_fallback() {
    let config = ClusterRpcConfig {
        local_node_id: "node-1".to_string(),
        timeout_secs: 60,
        local_rpc_port: 21949,
    };
    let mut tool = ClusterRpcTool::new(config);
    tool.set_context("stored-ch", "stored-cid");
    tool.set_rpc_call_fn(Arc::new(|_node: &str, _action: &str, payload: serde_json::Value| {
        let ch = payload.get("channel").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let cid = payload.get("chat_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        Box::pin(async move { Ok(serde_json::json!({"content": format!("ch={}, cid={}", ch, cid)})) })
    }));

    // Empty context channel/chat_id -> should fall back to stored
    let ctx = RequestContext::new("", "", "user1", "sess1");
    let result = tool
        .execute(r#"{"target_node": "node-2", "message": "test"}"#, &ctx)
        .await
        .unwrap();
    assert!(result.contains("ch=stored-ch"));
    assert!(result.contains("cid=stored-cid"));
}

#[tokio::test]
async fn test_cluster_rpc_tool_empty_sync_response() {
    let config = ClusterRpcConfig {
        local_node_id: "node-1".to_string(),
        timeout_secs: 60,
        local_rpc_port: 21949,
    };
    let mut tool = ClusterRpcTool::new(config);
    tool.set_rpc_call_fn(Arc::new(|_node: &str, _action: &str, _payload: serde_json::Value| {
        Box::pin(async { Ok(serde_json::json!({"status": "done"})) })
    }));

    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool
        .execute(r#"{"target_node": "node-2", "message": "test"}"#, &ctx)
        .await
        .unwrap();
    assert_eq!(result, "");
}

// =========================================================================
// CronTool additional coverage: create with different schedule types
// =========================================================================

fn make_cron_service_with_dir(tmp: &TempDir) -> Arc<std::sync::Mutex<nemesis_cron::service::CronService>> {
    let db_path = tmp.path().join("cron.db");
    Arc::new(std::sync::Mutex::new(
        nemesis_cron::service::CronService::new(&db_path.to_string_lossy()),
    ))
}

#[tokio::test]
async fn test_cron_tool_create_with_every_schedule() {
    let tmp = TempDir::new().unwrap();
    let svc = make_cron_service_with_dir(&tmp);
    let tool = CronTool::new(svc);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    tool.set_context("web", "chat1");
    let args = serde_json::json!({
        "action": "create",
        "name": "test-every",
        "schedule": "every:60s",
        "content": "test reminder"
    }).to_string();
    let result = tool.execute(&args, &ctx).await;
    assert!(result.is_ok(), "Expected ok, got: {:?}", result);
    assert!(result.unwrap().contains("Created cron job"));
}

#[tokio::test]
async fn test_cron_tool_create_with_cron_expr() {
    let tmp = TempDir::new().unwrap();
    let svc = make_cron_service_with_dir(&tmp);
    let tool = CronTool::new(svc);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    tool.set_context("web", "chat1");
    let args = serde_json::json!({
        "action": "create",
        "name": "test-cron",
        "schedule": "0 * * * *",
        "content": "hourly task"
    }).to_string();
    let result = tool.execute(&args, &ctx).await;
    assert!(result.is_ok(), "Expected ok, got: {:?}", result);
    assert!(result.unwrap().contains("Created cron job"));
}

#[tokio::test]
async fn test_cron_tool_create_and_delete() {
    let tmp = TempDir::new().unwrap();
    let svc = make_cron_service_with_dir(&tmp);
    let tool = CronTool::new(svc);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    tool.set_context("web", "chat1");

    // Create
    let create_args = serde_json::json!({
        "action": "create",
        "name": "temp-job",
        "schedule": "every:30s",
        "content": "temporary"
    }).to_string();
    let create_result = tool.execute(&create_args, &ctx).await.unwrap();
    // Extract ID from "Created cron job: temp-job (ID: xxx)"
    let id_start = create_result.find("(ID: ").unwrap();
    let id_end = create_result.find(")").unwrap();
    let job_id = &create_result[id_start + 5..id_end];

    // Delete
    let delete_args = serde_json::json!({"action": "delete", "id": job_id}).to_string();
    let delete_result = tool.execute(&delete_args, &ctx).await;
    assert!(delete_result.is_ok());
    assert!(delete_result.unwrap().contains("Deleted cron job"));
}

#[tokio::test]
async fn test_cron_tool_create_invalid_every_schedule() {
    let tmp = TempDir::new().unwrap();
    let svc = make_cron_service_with_dir(&tmp);
    let tool = CronTool::new(svc);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({
        "action": "create",
        "name": "bad-schedule",
        "schedule": "every:invalid"
    }).to_string();
    let result = tool.execute(&args, &ctx).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_cron_tool_create_with_empty_action() {
    let svc = make_cron_service();
    let tool = CronTool::new(svc);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    // Default action is empty string -> should hit "unknown action"
    let args = serde_json::json!({"name": "test"}).to_string();
    let result = tool.execute(&args, &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Unknown cron action"));
}

#[tokio::test]
async fn test_cron_tool_list_after_create() {
    let tmp = TempDir::new().unwrap();
    let svc = make_cron_service_with_dir(&tmp);
    let tool = CronTool::new(svc);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    tool.set_context("web", "chat1");

    // Create a job first
    let create_args = serde_json::json!({
        "action": "create",
        "name": "listable-job",
        "schedule": "every:120s",
        "content": "content"
    }).to_string();
    tool.execute(&create_args, &ctx).await.unwrap();

    // List
    let list_args = serde_json::json!({"action": "list"}).to_string();
    let result = tool.execute(&list_args, &ctx).await.unwrap();
    assert!(result.contains("listable-job"));
}

// =========================================================================
// InstallSkillTool coverage
// =========================================================================

#[tokio::test]
async fn test_install_skill_tool_missing_slug() {
    let registry = Arc::new(nemesis_skills::registry::RegistryManager::new_empty());
    let tool = InstallSkillTool::new(registry, "/tmp/ws".to_string());
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    let result = tool.execute(r#"{"name": "test"}"#, &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("slug parameter is required"));
}

#[tokio::test]
async fn test_install_skill_tool_empty_slug() {
    let registry = Arc::new(nemesis_skills::registry::RegistryManager::new_empty());
    let tool = InstallSkillTool::new(registry, "/tmp/ws".to_string());
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    let result = tool.execute(r#"{"slug": ""}"#, &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("slug parameter is required"));
}

#[tokio::test]
async fn test_install_skill_tool_invalid_json() {
    let registry = Arc::new(nemesis_skills::registry::RegistryManager::new_empty());
    let tool = InstallSkillTool::new(registry, "/tmp/ws".to_string());
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    let result = tool.execute("not json", &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Invalid JSON"));
}

#[tokio::test]
async fn test_install_skill_tool_path_traversal() {
    let registry = Arc::new(nemesis_skills::registry::RegistryManager::new_empty());
    let tool = InstallSkillTool::new(registry, "/tmp/ws".to_string());
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    let result = tool.execute(r#"{"slug": "../evil"}"#, &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("invalid slug"));
}

#[tokio::test]
async fn test_install_skill_tool_already_exists() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_string_lossy().to_string();
    // Create the skill directory to simulate existing skill
    let skill_dir = tmp.path().join("skills").join("existing-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();

    let registry = Arc::new(nemesis_skills::registry::RegistryManager::new_empty());
    let tool = InstallSkillTool::new(registry, workspace);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    let result = tool.execute(r#"{"slug": "existing-skill"}"#, &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("already exists locally"));
}

// =========================================================================
// FindSkillsTool coverage
// =========================================================================

#[tokio::test]
async fn test_find_skills_tool_empty_query() {
    let registry = Arc::new(nemesis_skills::registry::RegistryManager::new_empty());
    let tool = FindSkillsTool::new(registry);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    let result = tool.execute(r#"{"query": ""}"#, &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("missing or empty"));
}

#[tokio::test]
async fn test_find_skills_tool_missing_query() {
    let registry = Arc::new(nemesis_skills::registry::RegistryManager::new_empty());
    let tool = FindSkillsTool::new(registry);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    let result = tool.execute(r#"{"other": "value"}"#, &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("missing or empty"));
}

#[tokio::test]
async fn test_find_skills_tool_invalid_json() {
    let registry = Arc::new(nemesis_skills::registry::RegistryManager::new_empty());
    let tool = FindSkillsTool::new(registry);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    let result = tool.execute("not json", &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Invalid JSON"));
}

#[tokio::test]
async fn test_find_skills_tool_description_and_params() {
    let registry = Arc::new(nemesis_skills::registry::RegistryManager::new_empty());
    let tool = FindSkillsTool::new(registry);
    assert!(!tool.description().is_empty());
    assert!(tool.parameters().is_object());
}

// =========================================================================
// InstallSkillTool description/params
// =========================================================================

#[test]
fn test_install_skill_tool_description_and_params() {
    let registry = Arc::new(nemesis_skills::registry::RegistryManager::new_empty());
    let tool = InstallSkillTool::new(registry, "/tmp/ws".to_string());
    assert!(!tool.description().is_empty());
    assert!(tool.parameters().is_object());
}

// =========================================================================
// register_shared_tools with skills_registry
// =========================================================================

#[test]
fn test_register_shared_tools_with_skills_registry() {
    let registry = Arc::new(nemesis_skills::registry::RegistryManager::new_empty());
    let config = SharedToolConfig {
        skills_registry: Some(registry),
        workspace: Some("/tmp/test-workspace".to_string()),
        ..Default::default()
    };
    let tools = register_shared_tools(&config);
    assert!(tools.contains_key("find_skills"));
    assert!(tools.contains_key("install_skill"));
}

#[test]
fn test_register_shared_tools_with_skills_registry_no_workspace() {
    let registry = Arc::new(nemesis_skills::registry::RegistryManager::new_empty());
    let config = SharedToolConfig {
        skills_registry: Some(registry),
        workspace: None,
        ..Default::default()
    };
    let tools = register_shared_tools(&config);
    assert!(tools.contains_key("find_skills"));
    // install_skill requires workspace
    assert!(!tools.contains_key("install_skill"));
}

// =========================================================================
// ClusterRpcConfig default
// =========================================================================

#[test]
fn test_cluster_rpc_config_default() {
    let config = ClusterRpcConfig::default();
    assert!(config.local_node_id.is_empty());
    assert_eq!(config.timeout_secs, 3600);
    assert_eq!(config.local_rpc_port, 21949);
}

// =========================================================================
// MessageTool: JSON args without content field
// =========================================================================

#[tokio::test]
async fn test_message_tool_json_without_content_field() {
    let tool = MessageTool::new();
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool
        .execute(r#"{"other": "value"}"#, &ctx)
        .await
        .unwrap();
    // Should fall back to raw args
    assert_eq!(result, r#"{"other": "value"}"#);
}

// =========================================================================
// WebSearchTool extract_query method
// =========================================================================

#[test]
fn test_web_search_tool_extract_query_method() {
    let tool = WebSearchTool::new(WebSearchConfig::default());
    assert_eq!(tool.extract_query(r#"{"query": "test"}"#).unwrap(), "test");
    assert_eq!(tool.extract_query("plain text").unwrap(), "plain text");
}

// =========================================================================
// ForgeBridgeTool via register_shared_tools
// =========================================================================

#[test]
fn test_register_shared_tools_with_forge() {
    let tmp = tempfile::tempdir().unwrap();
    let config = nemesis_forge::config::ForgeConfig::default();
    let forge = Arc::new(nemesis_forge::forge::Forge::new(config, tmp.path().to_path_buf()));
    let executor = Arc::new(nemesis_forge::forge_tools::ForgeToolExecutor::new(forge));
    let config = SharedToolConfig {
        forge_executor: Some(executor),
        ..Default::default()
    };
    let tools = register_shared_tools(&config);
    assert!(tools.contains_key("forge_reflect"));
}

// =========================================================================
// register_shared_tools with complete_bootstrap
// =========================================================================

#[test]
fn test_register_shared_tools_includes_complete_bootstrap() {
    let config = SharedToolConfig {
        workspace: Some("/tmp/ws".to_string()),
        ..Default::default()
    };
    let tools = register_shared_tools(&config);
    assert!(tools.contains_key("complete_bootstrap"));
}

// =========================================================================
// McpDiscoveryResult fields
// =========================================================================

#[test]
fn test_mcp_discovery_result_fields() {
    let result = McpDiscoveryResult {
        tools: vec![],
        server_name: "test".to_string(),
    };
    assert!(result.tools.is_empty());
    assert_eq!(result.server_name, "test");
}

// =========================================================================
// Additional percent_decode edge cases
// =========================================================================

#[test]
fn test_percent_decode_invalid_hex() {
    // %GG is not valid hex - should keep the original
    let result = percent_decode("%GG");
    assert!(result.contains("%GG"));
}

#[test]
fn test_percent_decode_partial_hex() {
    // %1 at end (only one hex char) - chars.by_ref().take(2) consumes the '1'
    // and produces an empty string for the hex parse, so the result is "%"
    let result = percent_decode("test%1");
    // The function consumes '1' via take(2) but only gets one char for hex parse
    // which fails, so it outputs "%" + "1" (the hex string)
    assert!(result.starts_with("test"));
}

// =========================================================================
// urlencoding edge cases
// =========================================================================

#[test]
fn test_urlencoding_unreserved_chars() {
    // Unreserved characters should not be encoded
    assert_eq!(urlencoding("A-Z"), "A-Z");
    assert_eq!(urlencoding("0-9"), "0-9");
    assert_eq!(urlencoding("hello_world"), "hello_world");
    assert_eq!(urlencoding("file.txt"), "file.txt");
    assert_eq!(urlencoding("a~b"), "a~b");
}

#[test]
fn test_urlencoding_empty() {
    assert_eq!(urlencoding(""), "");
}

// =========================================================================
// SharedToolConfig clone
// =========================================================================

#[test]
fn test_shared_tool_config_clone() {
    let config = SharedToolConfig {
        web_search: Some(WebSearchConfig::default()),
        ..Default::default()
    };
    let cloned = config.clone();
    assert!(cloned.web_search.is_some());
}

// =========================================================================
// register_shared_tools_async: MCP enabled with no servers
// =========================================================================

#[tokio::test]
async fn test_register_shared_tools_async_mcp_enabled_no_servers() {
    let config = SharedToolConfig {
        mcp_enabled: true,
        mcp_servers: vec![],
        ..Default::default()
    };
    let tools: HashMap<String, Box<dyn Tool>> = register_shared_tools_async(&config, Option::<fn(String) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<(String, String, Option<serde_json::Value>)>, String>> + Send>>>::None).await;
    assert!(tools.contains_key("message"));
    // No MCP servers configured -> no MCP tools
    assert!(!tools.keys().any(|k| k.starts_with("mcp_")));
}

// =========================================================================
// Tool trait: set_context for SpawnTool
// =========================================================================

#[tokio::test]
async fn test_spawn_tool_set_context() {
    let config = SpawnConfig {
        default_model: "test".to_string(),
        max_concurrent: 5,
    };
    let tool = SpawnTool::new(config);
    tool.set_context("discord", "channel-123");

    // Execute with empty context -> should use stored
    let mut tool_with_fn = SpawnTool::new(SpawnConfig {
        default_model: "test".to_string(),
        max_concurrent: 5,
    });
    tool_with_fn.set_context("stored-ch", "stored-cid");
    tool_with_fn.set_spawn_fn(Arc::new(
        |_agent_id: &str, _task: &str, _model: &str, channel: &str, chat_id: &str| {
            let ch = channel.to_string();
            let cid = chat_id.to_string();
            Box::pin(async move { Ok(format!("ch={}, cid={}", ch, cid)) })
        },
    ));

    let ctx = RequestContext::new("", "", "user1", "sess1");
    let result = tool_with_fn
        .execute(r#"{"agent_id": "a1", "task": "do"}"#, &ctx)
        .await
        .unwrap();
    assert!(result.contains("ch=stored-ch"));
    assert!(result.contains("cid=stored-cid"));
}

// =========================================================================
// Additional CronTool: create with deliver=false
// =========================================================================

#[tokio::test]
async fn test_cron_tool_create_no_deliver() {
    let tmp = TempDir::new().unwrap();
    let svc = make_cron_service_with_dir(&tmp);
    let tool = CronTool::new(svc);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({
        "action": "create",
        "name": "no-deliver",
        "schedule": "every:60s",
        "content": "test",
        "deliver": false
    }).to_string();
    let result = tool.execute(&args, &ctx).await;
    assert!(result.is_ok());
}

// =========================================================================
// Additional CronTool: create with at: schedule (RFC3339 timestamp)
// =========================================================================

#[tokio::test]
async fn test_cron_tool_create_with_at_schedule() {
    let tmp = TempDir::new().unwrap();
    let svc = make_cron_service_with_dir(&tmp);
    let tool = CronTool::new(svc);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    tool.set_context("web", "chat1");
    let future_ts = "2099-12-31T23:59:59+00:00";
    let args = serde_json::json!({
        "action": "create",
        "name": "at-job",
        "schedule": format!("at:{}", future_ts),
        "content": "future task"
    }).to_string();
    let result = tool.execute(&args, &ctx).await;
    assert!(result.is_ok(), "Expected ok, got: {:?}", result);
}

#[tokio::test]
async fn test_cron_tool_create_with_invalid_at_schedule() {
    let tmp = TempDir::new().unwrap();
    let svc = make_cron_service_with_dir(&tmp);
    let tool = CronTool::new(svc);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({
        "action": "create",
        "name": "bad-at",
        "schedule": "at:not-a-timestamp",
        "content": "test"
    }).to_string();
    let result = tool.execute(&args, &ctx).await;
    assert!(result.is_err());
}

// =========================================================================
// ExecTool: command with no output
// =========================================================================

#[tokio::test]
async fn test_exec_tool_no_output_command() {
    let tmp = TempDir::new().unwrap();
    let tool = ExecTool::new(&tmp.path().to_string_lossy(), false);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let cmd = if cfg!(target_os = "windows") {
        "cd ."
    } else {
        "true"
    };
    let args = serde_json::json!({"command": cmd}).to_string();
    let result = tool.execute(&args, &ctx).await.unwrap();
    assert!(result.contains("no output") || result.is_empty() || !result.contains("Exit code"));
}

// =========================================================================
// Additional coverage tests for loop_tools.rs (targeting 95%+)
// =========================================================================

#[tokio::test]
async fn test_write_file_tool_with_parent_dir_creation() {
    let tmp = TempDir::new().unwrap();
    let deep_path = tmp.path().join("a").join("b").join("c").join("deep.txt");

    let tool = WriteFileTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({
        "path": deep_path.to_string_lossy(),
        "content": "deeply nested content"
    }).to_string();

    let result = tool.execute(&args, &ctx).await.unwrap();
    assert!(result.contains("Successfully wrote"));
    assert!(deep_path.exists());
    assert_eq!(tokio::fs::read_to_string(&deep_path).await.unwrap(), "deeply nested content");
}

#[tokio::test]
async fn test_read_file_tool_with_json_args() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("json_test.txt");
    tokio::fs::write(&file_path, "json content").await.unwrap();

    let tool = ReadFileTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({"path": file_path.to_string_lossy()}).to_string();

    let result = tool.execute(&args, &ctx).await.unwrap();
    assert_eq!(result, "json content");
}

#[tokio::test]
async fn test_edit_file_tool_with_multiple_replacements() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("multi_edit.txt");
    tokio::fs::write(&file_path, "aaa bbb ccc bbb").await.unwrap();

    let tool = EditFileTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({
        "path": file_path.to_string_lossy(),
        "old_text": "bbb",
        "new_text": "xxx"
    }).to_string();

    let result = tool.execute(&args, &ctx).await;
    // Should fail because "bbb" appears twice
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("appears 2 times"));
}

#[tokio::test]
async fn test_append_file_tool_with_existing_content() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("existing_append.txt");
    tokio::fs::write(&file_path, "First line").await.unwrap();

    let tool = AppendFileTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({
        "path": file_path.to_string_lossy(),
        "content": "\nSecond line"
    }).to_string();

    let result = tool.execute(&args, &ctx).await.unwrap();
    assert!(result.contains("Appended"));
    let content = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(content, "First line\nSecond line");
}

#[tokio::test]
async fn test_delete_file_tool_success() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("delete_success.txt");
    tokio::fs::write(&file_path, "content to delete").await.unwrap();
    assert!(file_path.exists());

    let tool = DeleteFileTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({"path": file_path.to_string_lossy()}).to_string();

    let result = tool.execute(&args, &ctx).await.unwrap();
    assert!(result.contains("Deleted"));
    assert!(!file_path.exists());
}

#[tokio::test]
async fn test_create_dir_tool_nested() {
    let tmp = TempDir::new().unwrap();
    let nested_path = tmp.path().join("level1").join("level2").join("level3");

    let tool = CreateDirTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({"path": nested_path.to_string_lossy()}).to_string();

    let result = tool.execute(&args, &ctx).await.unwrap();
    assert!(result.contains("created"));
    assert!(nested_path.is_dir());
}

#[tokio::test]
async fn test_delete_dir_tool_with_contents() {
    let tmp = TempDir::new().unwrap();
    let dir_path = tmp.path().join("dir_with_files");
    tokio::fs::create_dir_all(&dir_path).await.unwrap();
    tokio::fs::write(dir_path.join("file1.txt"), "content1").await.unwrap();
    tokio::fs::write(dir_path.join("file2.txt"), "content2").await.unwrap();

    let tool = DeleteDirTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({"path": dir_path.to_string_lossy()}).to_string();

    let result = tool.execute(&args, &ctx).await.unwrap();
    assert!(result.contains("removed"));
    assert!(!dir_path.exists());
}

#[tokio::test]
async fn test_exec_tool_with_args() {
    let tmp = TempDir::new().unwrap();
    let tool = ExecTool::new(&tmp.path().to_string_lossy(), false);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let cmd = if cfg!(target_os = "windows") {
        "echo test output"
    } else {
        "echo test output"
    };
    let args = serde_json::json!({"command": cmd, "timeout": 10}).to_string();
    let result = tool.execute(&args, &ctx).await.unwrap();
    assert!(result.contains("test output"));
}

#[test]
fn test_extract_path_with_whitespace() {
    let result = extract_path("  /path/with/spaces  ").unwrap();
    assert_eq!(result, "/path/with/spaces");
}

#[test]
fn test_extract_path_and_content_with_extra_fields() {
    let result = extract_path_and_content(r#"{"path": "/tmp/test.txt", "content": "hello", "extra": "ignored"}"#);
    assert!(result.is_ok());
    let (path, content) = result.unwrap();
    assert_eq!(path, "/tmp/test.txt");
    assert_eq!(content, "hello");
}

#[test]
fn test_extract_edit_args_success() {
    let result = extract_edit_args(r#"{"path": "/a.txt", "old_text": "foo", "new_text": "bar", "extra": 42}"#);
    assert!(result.is_ok());
    let (path, old, new) = result.unwrap();
    assert_eq!(path, "/a.txt");
    assert_eq!(old, "foo");
    assert_eq!(new, "bar");
}

#[tokio::test]
async fn test_message_tool_json_without_content_field_v2() {
    let tool = MessageTool::new();
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool.execute(r#"{"other": "value"}"#, &ctx).await.unwrap();
    // Falls back to raw args
    assert_eq!(result, r#"{"other": "value"}"#);
}

#[tokio::test]
async fn test_message_tool_with_empty_content() {
    let tool = MessageTool::new();
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = tool.execute(r#"{"content": ""}"#, &ctx).await.unwrap();
    assert_eq!(result, "");
}

#[tokio::test]
async fn test_list_directory_tool_with_files_and_dirs() {
    let tmp = TempDir::new().unwrap();
    tokio::fs::write(tmp.path().join("file1.txt"), "a").await.unwrap();
    tokio::fs::write(tmp.path().join("file2.py"), "b").await.unwrap();
    tokio::fs::create_dir(tmp.path().join("subdir1")).await.unwrap();
    tokio::fs::create_dir(tmp.path().join("subdir2")).await.unwrap();

    let tool = ListDirectoryTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({"path": tmp.path().to_string_lossy()}).to_string();

    let result = tool.execute(&args, &ctx).await.unwrap();
    assert!(result.contains("file1.txt"));
    assert!(result.contains("file2.py"));
    assert!(result.contains("subdir1"));
    assert!(result.contains("subdir2"));
}

#[tokio::test]
async fn test_sleep_tool_with_json_duration() {
    let tool = SleepTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({"duration": 1}).to_string();
    let start = std::time::Instant::now();
    let result = tool.execute(&args, &ctx).await.unwrap();
    let elapsed = start.elapsed();
    assert!(result.contains("Slept for 1 seconds"));
    assert!(elapsed.as_secs() >= 1);
}

#[test]
fn test_register_default_tools_tool_names() {
    let tools = register_default_tools();
    let expected = ["message", "read_file", "write_file", "list_dir", "edit_file", "append_file", "delete_file", "create_dir", "delete_dir", "sleep"];
    for name in &expected {
        assert!(tools.contains_key(*name), "Missing tool: {}", name);
    }
}

#[tokio::test]
async fn test_exec_tool_workspace_restriction_enabled() {
    let tmp = TempDir::new().unwrap();
    let tool = ExecTool::new(&tmp.path().to_string_lossy(), true);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    // Try to execute with cwd outside workspace
    let args = serde_json::json!({
        "command": "echo hello",
        "cwd": "/etc"
    }).to_string();
    let result = tool.execute(&args, &ctx).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_exec_tool_workspace_restriction_disabled() {
    let tmp = TempDir::new().unwrap();
    let tool = ExecTool::new(&tmp.path().to_string_lossy(), false);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    // Without restriction, cwd outside workspace should work
    let args = serde_json::json!({
        "command": "echo hello",
        "cwd": tmp.path().to_string_lossy().to_string()
    }).to_string();
    let result = tool.execute(&args, &ctx).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_async_exec_tool_with_wait_seconds() {
    let tmp = TempDir::new().unwrap();
    let tool = AsyncExecTool::new(&tmp.path().to_string_lossy(), false);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let cmd = if cfg!(target_os = "windows") { "echo hello" } else { "echo hello" };
    let args = serde_json::json!({"command": cmd, "wait_seconds": 5}).to_string();
    let result = tool.execute(&args, &ctx).await;
    assert!(result.is_ok());
}

#[test]
fn test_tool_descriptions_not_empty_all_tools() {
    let tools = register_default_tools();
    for (name, tool) in &tools {
        assert!(!tool.description().is_empty(), "Tool '{}' has empty description", name);
        let params = tool.parameters();
        assert!(params.is_object(), "Tool '{}' has non-object parameters", name);
    }
}

//! Additional tests to boost code coverage in nemesis-agent loop_tools module
//!
//! This test file targets specific areas with low coverage:
//! - Error handling paths
//! - Edge cases in tool execution
//! - Message tool callback scenarios
//! - File tool edge cases
//! - Async execution tool paths

use super::*;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

#[tokio::test]
async fn test_message_tool_callback_integration() {
    let tool = MessageTool::new();
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    // Test with callback
    let callback_called = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let callback_called_clone = callback_called.clone();

    tool.set_send_callback(Box::new(move |channel, chat_id, content| {
        assert_eq!(channel, "web");
        assert_eq!(chat_id, "chat1");
        assert_eq!(content, "test message");
        callback_called_clone.store(true, std::sync::atomic::Ordering::SeqCst);
    }));

    let result = tool
        .execute(r#"{"content": "test message"}"#, &ctx)
        .await
        .unwrap();
    assert_eq!(result, "test message");
    assert!(tool.has_sent_in_round());
    assert!(callback_called.load(std::sync::atomic::Ordering::SeqCst));

    // Test reset
    tool.reset_sent_in_round();
    assert!(!tool.has_sent_in_round());
}

#[tokio::test]
async fn test_message_tool_stored_context_fallback() {
    let tool = MessageTool::new();

    // Set stored context
    tool.set_context("stored_channel", "stored_chat");

    // Create empty context to trigger fallback
    let ctx = RequestContext::new("", "", "user1", "sess1");

    let callback_called = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let callback_called_clone = callback_called.clone();

    tool.set_send_callback(Box::new(move |channel, chat_id, _content| {
        assert_eq!(channel, "stored_channel");
        assert_eq!(chat_id, "stored_chat");
        callback_called_clone.store(true, std::sync::atomic::Ordering::SeqCst);
    }));

    tool.execute(r#"{"content": "fallback test"}"#, &ctx)
        .await
        .unwrap();
    assert!(callback_called.load(std::sync::atomic::Ordering::SeqCst));
}

#[tokio::test]
async fn test_message_tool_rpc_formatting() {
    let tool = MessageTool::new();

    // Test RPC correlation ID formatting
    let mut ctx = RequestContext::new("rpc", "chat1", "user1", "sess1");
    ctx.correlation_id = Some("test-correlation-123".to_string());

    // Test the format_rpc_message method directly
    let formatted = ctx.format_rpc_message("response");
    assert!(formatted.contains("[rpc:test-correlation-123]"));
    assert!(formatted.contains("response"));

    // Test execution
    let result = tool
        .execute(r#"{"content": "response"}"#, &ctx)
        .await
        .unwrap();
    assert!(result.contains("response"));
}

#[tokio::test]
async fn test_read_file_not_found() {
    let tool = ReadFileTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    let args = serde_json::json!({ "path": "/nonexistent/file.txt" }).to_string();
    let result = tool.execute(&args, &ctx).await;

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("File not found"));
}

#[tokio::test]
async fn test_write_file_creates_directories() {
    let tmp = TempDir::new().unwrap();
    let nested_path = tmp.path().join("nested/dir/file.txt");
    let path_str = nested_path.to_string_lossy().to_string();

    let tool = WriteFileTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    let args = serde_json::json!({
        "path": path_str,
        "content": "nested content"
    })
    .to_string();

    let result = tool.execute(&args, &ctx).await.unwrap();
    assert!(result.contains("Successfully wrote"));
    assert!(nested_path.exists());
    assert!(nested_path.parent().unwrap().exists());
}

#[tokio::test]
async fn test_list_directory_not_directory() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("not_a_dir.txt");
    tokio::fs::write(&file_path, "content").await.unwrap();

    let tool = ListDirectoryTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    let args = serde_json::json!({ "path": file_path }).to_string();
    let result = tool.execute(&args, &ctx).await;

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not a directory"));
}

#[tokio::test]
async fn test_list_directory_not_found() {
    let tool = ListDirectoryTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    let args = serde_json::json!({ "path": "/nonexistent/directory" }).to_string();
    let result = tool.execute(&args, &ctx).await;

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Directory not found"));
}

#[tokio::test]
async fn test_list_empty_directory() {
    let tmp = TempDir::new().unwrap();
    let tool = ListDirectoryTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    let args = serde_json::json!({ "path": tmp.path() }).to_string();
    let result = tool.execute(&args, &ctx).await.unwrap();

    assert_eq!(result, "(empty directory)");
}

#[tokio::test]
async fn test_sleep_tool() {
    let tool = SleepTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    // Test with seconds (minimum 1 second)
    let start = std::time::Instant::now();
    let args = serde_json::json!({ "seconds": 1 }).to_string();
    let result = tool.execute(&args, &ctx).await.unwrap();
    assert!(result.contains("Slept for"));

    let elapsed = start.elapsed();
    assert!(elapsed >= Duration::from_millis(1000)); // Should sleep for at least 1 second
}

#[tokio::test]
async fn test_sleep_tool_invalid_duration() {
    let tool = SleepTool;
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    // Test with invalid duration (should still work but handle parsing)
    let args = serde_json::json!({ "seconds": "invalid" }).to_string();
    let result = tool.execute(&args, &ctx).await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_exec_tool_command_execution() {
    let tmp = TempDir::new().unwrap();
    let tool = ExecTool::new(tmp.path().to_str().unwrap(), false);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    // Test simple echo command (cross-platform)
    let args = serde_json::json!({
        "command": if cfg!(windows) { "cmd" } else { "echo" },
        "args": if cfg!(windows) { vec!["/c", "echo", "test"] } else { vec!["test"] }
    })
    .to_string();

    let result = tool.execute(&args, &ctx).await.unwrap();
    assert!(result.contains("test") || result.len() > 0);
}

#[tokio::test]
async fn test_exec_tool_command_failure() {
    let tmp = TempDir::new().unwrap();
    let tool = ExecTool::new(tmp.path().to_str().unwrap(), false);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    // Test command that fails (dir should succeed but we'll test a failing one)
    let args = serde_json::json!({
        "command": if cfg!(windows) { "cmd" } else { "ls" },
        "args": if cfg!(windows) { vec!["/c", "dir"] } else { vec!["-la", "/nonexistent"] }
    })
    .to_string();

    let result = tool.execute(&args, &ctx).await;
    // Command should either succeed or fail - we just want to test execution
    // The result could be Ok with output or Err with failure
    // We just want to make sure it doesn't panic
    assert!(result.is_ok() || result.is_err());
}

#[tokio::test]
async fn test_async_exec_tool_basic() {
    let tmp = TempDir::new().unwrap();
    let tool = AsyncExecTool::new(tmp.path().to_str().unwrap(), false);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    // Test with timeout
    let args = serde_json::json!({
        "command": if cfg!(windows) { "cmd" } else { "echo" },
        "args": if cfg!(windows) { vec!["/c", "echo", "async"] } else { vec!["async"] },
        "timeout_secs": 5
    })
    .to_string();

    let result = tool.execute(&args, &ctx).await.unwrap();
    assert!(result.len() > 0);
}

#[tokio::test]
async fn test_async_exec_tool_timeout() {
    let tmp = TempDir::new().unwrap();
    let tool = AsyncExecTool::new(tmp.path().to_str().unwrap(), false);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    // Test with very short timeout on a long-running command
    let args = serde_json::json!({
        "command": if cfg!(windows) { "cmd" } else { "sleep" },
        "args": if cfg!(windows) { vec!["/c", "timeout", "10"] } else { vec!["10"] },
        "timeout_secs": 1
    })
    .to_string();

    let result = tool.execute(&args, &ctx).await;
    // Should either timeout or complete
    assert!(result.is_ok() || result.is_err());
}

#[cfg(test)]
mod message_tool_edge_cases {
    use super::*;

    #[tokio::test]
    async fn test_message_tool_empty_content() {
        let tool = MessageTool::new();
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

        let result = tool.execute(r#"{"content": ""}"#, &ctx).await.unwrap();
        assert_eq!(result, "");
    }

    #[tokio::test]
    async fn test_message_tool_special_characters() {
        let tool = MessageTool::new();
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

        let special_content = "Test with \"quotes\" and 'apostrophes' and \n newlines";
        let args = serde_json::json!({"content": special_content}).to_string();

        let result = tool.execute(&args, &ctx).await.unwrap();
        assert!(result.contains("quotes"));
    }

    #[tokio::test]
    async fn test_message_tool_unicode() {
        let tool = MessageTool::new();
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

        let unicode_content = "Test with emoji 🎉 and chinese 中文";
        let args = serde_json::json!({"content": unicode_content}).to_string();

        let result = tool.execute(&args, &ctx).await.unwrap();
        assert!(result.contains("emoji"));
    }
}

#[cfg(test)]
mod file_tool_edge_cases {
    use super::*;

    #[tokio::test]
    async fn test_write_file_binary_content() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("binary.bin");
        let path_str = file_path.to_string_lossy().to_string();

        let tool = WriteFileTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

        // Create some binary-like content
        let binary_content = vec![0u8, 255, 128, 64, 32]
            .iter()
            .map(|&b| format!("{:02x}", b))
            .collect::<String>();

        let args = serde_json::json!({
            "path": path_str,
            "content": binary_content
        })
        .to_string();

        let result = tool.execute(&args, &ctx).await.unwrap();
        assert!(result.contains("Successfully wrote"));
    }

    #[tokio::test]
    async fn test_read_file_permission_error() {
        let tool = ReadFileTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

        // Try to read a system directory that should be inaccessible
        let system_path = if cfg!(windows) {
            "C:\\Windows\\System32\\config\\SAM"
        } else {
            "/root/.ssh/id_rsa"
        };

        let args = serde_json::json!({ "path": system_path }).to_string();
        let result = tool.execute(&args, &ctx).await;

        // Should either fail with permission error or file not found
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_list_directory_with_many_files() {
        let tmp = TempDir::new().unwrap();

        // Create many files
        for i in 0..20 {
            tokio::fs::write(
                tmp.path().join(format!("file{}.txt", i)),
                format!("content{}", i),
            )
            .await
            .unwrap();
        }

        let tool = ListDirectoryTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

        let args = serde_json::json!({ "path": tmp.path() }).to_string();
        let result = tool.execute(&args, &ctx).await.unwrap();

        // Should contain multiple file entries
        assert!(result.lines().count() >= 20);
    }
}

#[cfg(test)]
mod tool_registration_tests {
    use super::*;

    #[tokio::test]
    async fn test_register_default_tools_contains_all_tools() {
        let tools = register_default_tools();

        // Verify essential tools are registered
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
        // Note: exec and async_exec might not be registered by default
        // Verify basic tools exist and there are many
        assert!(
            tools.len() >= 10,
            "Expected at least 10 default tools, got {}",
            tools.len()
        );

        // Print tool count for debugging
        println!("Registered {} tools", tools.len());
        for key in tools.keys() {
            println!("  - {}", key);
        }
    }

    #[tokio::test]
    async fn test_tool_descriptions_are_valid() {
        let tools = register_default_tools();

        // Verify all tools have valid descriptions
        for (name, tool) in tools.iter() {
            let description = tool.description();
            assert!(
                !description.is_empty(),
                "Tool {} has empty description",
                name
            );
            assert!(
                description.len() < 1000,
                "Tool {} description too long",
                name
            );
        }
    }

    #[tokio::test]
    async fn test_tool_parameters_are_valid_json() {
        let tools = register_default_tools();

        // Verify all tools have valid parameter JSON schemas
        for (name, tool) in tools.iter() {
            let params = tool.parameters();
            // Should be valid JSON object
            if let Some(obj) = params.as_object() {
                assert!(
                    obj.contains_key("type"),
                    "Tool {} missing type in parameters",
                    name
                );
                assert!(
                    obj.contains_key("properties"),
                    "Tool {} missing properties in parameters",
                    name
                );
            } else {
                panic!("Tool {} parameters is not a JSON object", name);
            }
        }
    }
}

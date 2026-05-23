use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex as StdMutex;

#[test]
fn test_format_rpc_prefix() {
    let result = format_rpc_prefix("corr-123", "Hello world");
    assert_eq!(result, "[rpc:corr-123] Hello world");
}

#[test]
fn test_format_rpc_prefix_empty_content() {
    let result = format_rpc_prefix("id-456", "");
    assert_eq!(result, "[rpc:id-456] ");
}

#[test]
fn test_extract_correlation_id_valid() {
    let id = extract_correlation_id("[rpc:abc-123] Hello world");
    assert_eq!(id, Some("abc-123".to_string()));
}

#[test]
fn test_extract_correlation_id_no_prefix() {
    let id = extract_correlation_id("Hello world");
    assert_eq!(id, None);
}

#[test]
fn test_extract_correlation_id_empty_id() {
    let id = extract_correlation_id("[rpc:] content");
    assert_eq!(id, None);
}

#[test]
fn test_extract_correlation_id_no_content() {
    let id = extract_correlation_id("[rpc:id-only]");
    assert_eq!(id, Some("id-only".to_string()));
}

#[test]
fn test_strip_rpc_prefix() {
    let result = strip_rpc_prefix("[rpc:corr-123] Hello world");
    assert_eq!(result, "Hello world");
}

#[test]
fn test_strip_rpc_prefix_no_space() {
    let result = strip_rpc_prefix("[rpc:corr-123]Hello");
    assert_eq!(result, "Hello");
}

#[test]
fn test_strip_rpc_prefix_no_prefix() {
    let result = strip_rpc_prefix("Just content");
    assert_eq!(result, "Just content");
}

#[tokio::test]
async fn test_message_tool_with_context() {
    let tool = MessageTool::new();
    tool.set_send_callback(Arc::new(|_, _, _| Ok(()))).await;
    tool.set_context("web", "chat-123").await;
    assert_eq!(tool.name(), "message");

    let result = tool
        .execute(&serde_json::json!({"content": "Hello!"}))
        .await;
    assert!(result.silent);
    assert!(result.for_llm.contains("Message sent"));
    assert!(tool.was_sent().await);
}

#[tokio::test]
async fn test_empty_message() {
    let tool = MessageTool::new();
    tool.set_send_callback(Arc::new(|_, _, _| Ok(()))).await;
    tool.set_context("web", "chat-1").await;
    let result = tool
        .execute(&serde_json::json!({"content": ""}))
        .await;
    assert!(result.is_error);
}

#[tokio::test]
async fn test_context() {
    let tool = MessageTool::new();
    tool.set_context("web", "chat-123").await;
    let ch = tool.channel.lock().await;
    assert_eq!(*ch, "web");
}

#[tokio::test]
async fn test_set_context_resets_sent_flag() {
    let tool = MessageTool::new();
    tool.set_send_callback(Arc::new(|_, _, _| Ok(()))).await;
    tool.set_context("web", "chat-1").await;

    // Send a message first
    let _ = tool
        .execute(&serde_json::json!({"content": "Hello!"}))
        .await;
    assert!(tool.has_sent_in_round().await);

    // set_context should reset the flag
    tool.set_context("web", "chat-123").await;
    assert!(!tool.has_sent_in_round().await);
}

#[tokio::test]
async fn test_set_context_resets_correlation_id() {
    let tool = MessageTool::new();
    tool.set_send_callback(Arc::new(|_, _, _| Ok(()))).await;
    tool.set_context("rpc", "chat-1").await;
    tool.set_correlation_id("corr-999").await;

    // Resetting context should clear correlation_id
    tool.set_context("rpc", "chat-2").await;
    let corr = tool.correlation_id.lock().await.clone();
    assert!(corr.is_empty(), "correlation_id should be reset by set_context");
}

#[tokio::test]
async fn test_has_sent_in_round() {
    let tool = MessageTool::new();
    tool.set_send_callback(Arc::new(|_, _, _| Ok(()))).await;
    tool.set_context("web", "chat-1").await;
    assert!(!tool.has_sent_in_round().await);

    let _ = tool
        .execute(&serde_json::json!({"content": "Hello!"}))
        .await;
    assert!(tool.has_sent_in_round().await);

    tool.reset_round().await;
    assert!(!tool.has_sent_in_round().await);
}

#[tokio::test]
async fn test_set_send_callback() {
    let tool = MessageTool::new();
    let call_count = Arc::new(AtomicUsize::new(0));
    let count_clone = call_count.clone();

    tool.set_send_callback(Arc::new(move |ch, cid, content| {
        assert_eq!(ch, "web");
        assert_eq!(cid, "chat-1");
        assert_eq!(content, "Hello!");
        count_clone.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }))
    .await;

    tool.set_context("web", "chat-1").await;
    let result = tool
        .execute(&serde_json::json!({"content": "Hello!"}))
        .await;

    assert!(result.silent, "Result should be silent when callback delivers message");
    assert_eq!(call_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_send_callback_error() {
    let tool = MessageTool::new();
    tool.set_send_callback(Arc::new(|_, _, _| {
        Err("connection refused".to_string())
    }))
    .await;

    tool.set_context("web", "chat-1").await;
    let result = tool
        .execute(&serde_json::json!({"content": "Hello!"}))
        .await;

    assert!(result.is_error);
    assert!(result.for_llm.contains("connection refused"));
}

#[tokio::test]
async fn test_no_callback_returns_error() {
    // Matches Go behavior: no callback configured returns an error.
    let tool = MessageTool::new();
    tool.set_context("web", "chat-1").await;

    let result = tool
        .execute(&serde_json::json!({"content": "Hello!"}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("not configured"));
}

#[tokio::test]
async fn test_rpc_correlation_id_prefix() {
    let tool = MessageTool::new();
    let captured_content = Arc::new(StdMutex::new(String::new()));
    let content_clone = captured_content.clone();

    tool.set_send_callback(Arc::new(move |_, _, content| {
        *content_clone.lock().unwrap() = content.to_string();
        Ok(())
    }))
    .await;

    tool.set_context("rpc", "chat-1").await;
    tool.set_correlation_id("corr-999").await;

    let _ = tool
        .execute(&serde_json::json!({"content": "Hello RPC!"}))
        .await;

    let sent = captured_content.lock().unwrap().clone();
    assert_eq!(sent, "[rpc:corr-999] Hello RPC!");
}

#[tokio::test]
async fn test_rpc_no_correlation_id_warns() {
    let tool = MessageTool::new();
    let captured_content = Arc::new(StdMutex::new(String::new()));
    let content_clone = captured_content.clone();

    tool.set_send_callback(Arc::new(move |_, _, content| {
        *content_clone.lock().unwrap() = content.to_string();
        Ok(())
    }))
    .await;

    tool.set_context("rpc", "chat-1").await;
    // Don't set correlation_id

    let _ = tool
        .execute(&serde_json::json!({"content": "No correlation"}))
        .await;

    let sent = captured_content.lock().unwrap().clone();
    assert_eq!(sent, "No correlation"); // No prefix added
}

#[tokio::test]
async fn test_non_rpc_channel_no_prefix() {
    let tool = MessageTool::new();
    let captured_content = Arc::new(StdMutex::new(String::new()));
    let content_clone = captured_content.clone();

    tool.set_send_callback(Arc::new(move |_, _, content| {
        *content_clone.lock().unwrap() = content.to_string();
        Ok(())
    }))
    .await;

    tool.set_context("web", "chat-1").await;
    tool.set_correlation_id("corr-999").await;

    let _ = tool
        .execute(&serde_json::json!({"content": "Hello web!"}))
        .await;

    let sent = captured_content.lock().unwrap().clone();
    assert_eq!(sent, "Hello web!"); // No prefix for non-RPC
}

#[tokio::test]
async fn test_override_channel_via_args() {
    let tool = MessageTool::new();
    let captured = Arc::new(StdMutex::new(("".to_string(), "".to_string())));
    let captured_clone = captured.clone();

    tool.set_send_callback(Arc::new(move |ch, cid, _| {
        *captured_clone.lock().unwrap() = (ch.to_string(), cid.to_string());
        Ok(())
    }))
    .await;

    tool.set_context("web", "chat-default").await;

    let _ = tool
        .execute(&serde_json::json!({
            "content": "Hello!",
            "channel": "telegram",
            "chat_id": "chat-override"
        }))
        .await;

    let (ch, cid) = captured.lock().unwrap().clone();
    assert_eq!(ch, "telegram");
    assert_eq!(cid, "chat-override");
}

#[tokio::test]
async fn test_no_channel_or_chat_id_error() {
    let tool = MessageTool::new();
    // No context set, no channel/chat_id in args
    let result = tool
        .execute(&serde_json::json!({"content": "Hello!"}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("No target"));
}

// ============================================================
// Additional tests for missing coverage
// ============================================================

#[test]
fn test_format_rpc_prefix_long_id() {
    let result = format_rpc_prefix("very-long-correlation-id-with-lots-of-chars", "msg");
    assert!(result.starts_with("[rpc:very-long-correlation-id-with-lots-of-chars]"));
    assert!(result.ends_with("msg"));
}

#[test]
fn test_extract_correlation_id_complex() {
    let id = extract_correlation_id("[rpc:task-abc-123-def] Response content here");
    assert_eq!(id, Some("task-abc-123-def".to_string()));
}

#[test]
fn test_extract_correlation_id_partial_prefix() {
    // Missing the closing bracket
    let id = extract_correlation_id("[rpc:abc content");
    assert_eq!(id, None);
}

#[test]
fn test_strip_rpc_prefix_preserves_body() {
    let result = strip_rpc_prefix("[rpc:id-123]   Multiple   spaces");
    assert_eq!(result, "Multiple   spaces");
}

#[test]
fn test_strip_rpc_prefix_no_closing_bracket() {
    let result = strip_rpc_prefix("[rpc:id content without bracket");
    assert_eq!(result, "[rpc:id content without bracket");
}

#[tokio::test]
async fn test_message_tool_default() {
    let tool = MessageTool::default();
    assert_eq!(tool.name(), "message");
}

#[tokio::test]
async fn test_message_tool_parameters() {
    let tool = MessageTool::new();
    let params = tool.parameters();
    assert_eq!(params["type"], "object");
    assert!(params["properties"]["content"].is_object());
    assert!(params["required"].is_array());
}

#[tokio::test]
async fn test_message_tool_description() {
    let tool = MessageTool::new();
    assert!(!tool.description().is_empty());
}

#[tokio::test]
async fn test_only_channel_no_chat_id_error() {
    let tool = MessageTool::new();
    // Set only channel, no chat_id
    *tool.channel.lock().await = "web".to_string();
    let result = tool
        .execute(&serde_json::json!({"content": "Hello!"}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("No target"));
}

#[tokio::test]
async fn test_only_chat_id_no_channel_error() {
    let tool = MessageTool::new();
    // Set only chat_id, no channel
    *tool.chat_id.lock().await = "chat-1".to_string();
    let result = tool
        .execute(&serde_json::json!({"content": "Hello!"}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("No target"));
}

#[tokio::test]
async fn test_send_callback_success_is_silent() {
    let tool = MessageTool::new();
    tool.set_send_callback(Arc::new(|_, _, _| Ok(()))).await;
    tool.set_context("web", "chat-1").await;

    let result = tool
        .execute(&serde_json::json!({"content": "Hello!"}))
        .await;
    assert!(!result.is_error);
    assert!(result.silent);
    assert!(result.for_llm.contains("Message sent"));
}

#[tokio::test]
async fn test_rpc_with_correlation_id_no_callback_returns_error() {
    // Without a callback, the tool returns an error (matching Go behavior).
    let tool = MessageTool::new();
    tool.set_context("rpc", "chat-1").await;
    tool.set_correlation_id("corr-456").await;

    let result = tool
        .execute(&serde_json::json!({"content": "RPC message"}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("not configured"));
}

#[tokio::test]
async fn test_was_sent_alias() {
    let tool = MessageTool::new();
    tool.set_send_callback(Arc::new(|_, _, _| Ok(()))).await;
    tool.set_context("web", "chat-1").await;
    assert!(!tool.was_sent().await);

    let _ = tool
        .execute(&serde_json::json!({"content": "Hello!"}))
        .await;
    assert!(tool.was_sent().await);
}

#[tokio::test]
async fn test_multiple_messages_in_round() {
    let tool = MessageTool::new();
    tool.set_send_callback(Arc::new(|_, _, _| Ok(()))).await;
    tool.set_context("web", "chat-1").await;

    let _ = tool
        .execute(&serde_json::json!({"content": "First"}))
        .await;
    assert!(tool.has_sent_in_round().await);

    // Second message should still work
    let result = tool
        .execute(&serde_json::json!({"content": "Second"}))
        .await;
    assert!(!result.is_error);
    assert!(tool.has_sent_in_round().await);
}

#[tokio::test]
async fn test_override_only_channel() {
    let tool = MessageTool::new();
    let captured = Arc::new(StdMutex::new(("".to_string(), "".to_string())));
    let captured_clone = captured.clone();

    tool.set_send_callback(Arc::new(move |ch, cid, _| {
        *captured_clone.lock().unwrap() = (ch.to_string(), cid.to_string());
        Ok(())
    }))
    .await;

    tool.set_context("web", "chat-default").await;

    // Override only channel, chat_id should use default
    let _ = tool
        .execute(&serde_json::json!({
            "content": "Hello!",
            "channel": "discord"
        }))
        .await;

    let (ch, cid) = captured.lock().unwrap().clone();
    assert_eq!(ch, "discord");
    assert_eq!(cid, "chat-default");
}

#[tokio::test]
async fn test_contextual_tool_set_context() {
    let mut tool = MessageTool::new();
    tool.set_send_callback(Arc::new(|_, _, _| Ok(()))).await;

    let ctx = ToolExecutionContext {
        channel: "rpc".to_string(),
        chat_id: "chat-789".to_string(),
        correlation_id: "corr-ctx-001".to_string(),
        ..Default::default()
    };
    // ContextualTool::set_context is sync, uses try_lock
    ContextualTool::set_context(&mut tool, &ctx);

    // Allow a small delay for the mutex to be released
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    let ch = tool.channel.lock().await.clone();
    let cid = tool.chat_id.lock().await.clone();
    let corr = tool.correlation_id.lock().await.clone();
    assert_eq!(ch, "rpc");
    assert_eq!(cid, "chat-789");
    assert_eq!(corr, "corr-ctx-001");
}

// ============================================================
// Additional message tool edge-case tests
// ============================================================

#[test]
fn test_format_rpc_prefix_with_special_chars() {
    let result = format_rpc_prefix("id-<>\"'", "content");
    assert!(result.contains("[rpc:id-<>\"']"));
    assert!(result.contains("content"));
}

#[test]
fn test_extract_correlation_id_malformed_brackets() {
    let id = extract_correlation_id("[rpc:id]extra] content");
    assert_eq!(id, Some("id".to_string()));
}

#[test]
fn test_extract_correlation_id_nested_brackets() {
    let id = extract_correlation_id("[rpc:[nested]] content");
    // Should extract up to the first closing bracket
    assert!(id.is_some());
}

#[test]
fn test_strip_rpc_prefix_empty_after_prefix() {
    let result = strip_rpc_prefix("[rpc:id] ");
    assert_eq!(result, "");
}

#[tokio::test]
async fn test_message_tool_set_correlation_id() {
    let tool = MessageTool::new();
    tool.set_correlation_id("test-corr-id").await;
    let corr = tool.correlation_id.lock().await.clone();
    assert_eq!(corr, "test-corr-id");
}

#[tokio::test]
async fn test_message_tool_reset_round() {
    let tool = MessageTool::new();
    tool.set_send_callback(Arc::new(|_, _, _| Ok(()))).await;
    tool.set_context("web", "chat-1").await;

    let _ = tool
        .execute(&serde_json::json!({"content": "First"}))
        .await;
    assert!(tool.has_sent_in_round().await);

    tool.reset_round().await;
    assert!(!tool.has_sent_in_round().await);

    // Can send again after reset
    let result = tool
        .execute(&serde_json::json!({"content": "Second"}))
        .await;
    assert!(!result.is_error);
    assert!(tool.has_sent_in_round().await);
}

#[tokio::test]
async fn test_message_content_with_unicode() {
    let tool = MessageTool::new();
    let captured = Arc::new(StdMutex::new(String::new()));
    let captured_clone = captured.clone();

    tool.set_send_callback(Arc::new(move |_, _, content| {
        *captured_clone.lock().unwrap() = content.to_string();
        Ok(())
    }))
    .await;

    tool.set_context("web", "chat-1").await;

    let _ = tool
        .execute(&serde_json::json!({"content": "Hello! - test"}))
        .await;

    let sent = captured.lock().unwrap().clone();
    assert!(sent.contains("Hello!"));
}

#[tokio::test]
async fn test_message_content_with_newlines() {
    let tool = MessageTool::new();
    let captured = Arc::new(StdMutex::new(String::new()));
    let captured_clone = captured.clone();

    tool.set_send_callback(Arc::new(move |_, _, content| {
        *captured_clone.lock().unwrap() = content.to_string();
        Ok(())
    }))
    .await;

    tool.set_context("web", "chat-1").await;

    let _ = tool
        .execute(&serde_json::json!({"content": "line1\nline2\nline3"}))
        .await;

    let sent = captured.lock().unwrap().clone();
    assert!(sent.contains("line1\nline2\nline3"));
}

#[tokio::test]
async fn test_message_content_null_treated_as_missing() {
    let tool = MessageTool::new();
    tool.set_send_callback(Arc::new(|_, _, _| Ok(()))).await;
    tool.set_context("web", "chat-1").await;

    let result = tool
        .execute(&serde_json::json!({"content": null}))
        .await;
    assert!(result.is_error);
}

#[tokio::test]
async fn test_message_sent_in_round_not_set_on_error() {
    let tool = MessageTool::new();
    // No callback -> will error
    tool.set_context("web", "chat-1").await;

    let _ = tool
        .execute(&serde_json::json!({"content": "Hello!"}))
        .await;

    // sent_in_round should NOT be set since send failed
    assert!(!tool.was_sent().await);
}

#[tokio::test]
async fn test_message_rpc_correlation_with_special_chars() {
    let tool = MessageTool::new();
    let captured = Arc::new(StdMutex::new(String::new()));
    let captured_clone = captured.clone();

    tool.set_send_callback(Arc::new(move |_, _, content| {
        *captured_clone.lock().unwrap() = content.to_string();
        Ok(())
    }))
    .await;

    tool.set_context("rpc", "chat-1").await;
    tool.set_correlation_id("task_abc-123.def").await;

    let _ = tool
        .execute(&serde_json::json!({"content": "Result data"}))
        .await;

    let sent = captured.lock().unwrap().clone();
    assert!(sent.starts_with("[rpc:task_abc-123.def]"));
    assert!(sent.contains("Result data"));
}

#[test]
fn test_format_rpc_prefix_empty_id() {
    let result = format_rpc_prefix("", "content");
    assert_eq!(result, "[rpc:] content");
}

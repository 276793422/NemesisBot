//! Message tool - sends messages to users via the bus.
//!
//! Provides a `MessageTool` that can send messages to specific channels and chat IDs.
//! Supports a send callback for actual message delivery, round-based send tracking
//! (to detect whether a message was already sent in the current processing round),
//! optional channel/chat_id override per invocation, and automatic correlation ID
//! prefix formatting for RPC channel messages.
//!
//! The tool implements `ContextualTool` to receive per-invocation context (channel,
//! chat_id, correlation_id) from the registry's side-channel, avoiding race conditions
//! from shared mutable state across concurrent requests.

use crate::registry::{ContextualTool, Tool, ToolExecutionContext};
use crate::types::ToolResult;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, warn};

/// Callback for sending messages.
///
/// Receives `(channel, chat_id, content)` and returns an error string on failure,
/// or an empty string on success.
pub type SendCallback = Arc<dyn Fn(&str, &str, &str) -> Result<(), String> + Send + Sync>;

/// Formats content with the `[rpc:<correlation_id>]` prefix used by the RPC channel
/// to route responses back to the correct pending request.
///
/// # Examples
/// ```
/// use nemesis_tools::message::format_rpc_prefix;
/// let result = format_rpc_prefix("corr-123", "Hello world");
/// assert_eq!(result, "[rpc:corr-123] Hello world");
/// ```
pub fn format_rpc_prefix(correlation_id: &str, content: &str) -> String {
    format!("[rpc:{}] {}", correlation_id, content)
}

/// Extracts the correlation ID from content that starts with `[rpc:<id>]`.
///
/// Returns `Some(correlation_id)` if the prefix is found and well-formed,
/// or `None` otherwise.
pub fn extract_correlation_id(content: &str) -> Option<String> {
    let rest = content.strip_prefix("[rpc:")?;
    let end = rest.find(']')?;
    if end == 0 {
        return None;
    }
    Some(rest[..end].to_string())
}

/// Strips the `[rpc:<id>]` prefix from content and returns the actual message body.
///
/// If the content does not start with the RPC prefix, it is returned unchanged.
pub fn strip_rpc_prefix(content: &str) -> String {
    if let Some(rest) = content.strip_prefix("[rpc:") {
        if let Some(end) = rest.find(']') {
            let after = &rest[end + 1..];
            return after.trim_start().to_string();
        }
    }
    content.to_string()
}

/// Message tool - sends responses to users.
///
/// When a `SendCallback` is set, the tool invokes it to deliver the message
/// through the actual message bus. Without a callback, the tool returns an
/// error ("Message sending not configured"), matching Go's behavior.
///
/// For RPC channel messages, the tool automatically prepends the correlation ID
/// prefix `[rpc:<id>]` when a correlation ID is provided via context injection.
///
/// Implements `ContextualTool` to receive per-invocation context from the
/// registry's side-channel. The `correlation_id` is reset on each context
/// injection to prevent cross-request contamination.
pub struct MessageTool {
    channel: Arc<Mutex<String>>,
    chat_id: Arc<Mutex<String>>,
    sent_in_round: Arc<Mutex<bool>>,
    send_callback: Arc<Mutex<Option<SendCallback>>>,
    /// Per-invocation correlation ID, set via ContextualTool::set_context
    /// or the async set_correlation_id helper. Reset to empty on each
    /// set_context call to avoid cross-request race conditions.
    correlation_id: Arc<Mutex<String>>,
}

impl MessageTool {
    /// Create a new message tool with empty defaults and no send callback.
    pub fn new() -> Self {
        Self {
            channel: Arc::new(Mutex::new(String::new())),
            chat_id: Arc::new(Mutex::new(String::new())),
            sent_in_round: Arc::new(Mutex::new(false)),
            send_callback: Arc::new(Mutex::new(None)),
            correlation_id: Arc::new(Mutex::new(String::new())),
        }
    }

    /// Set the message context (channel and chat ID).
    ///
    /// Also resets the `sent_in_round` flag and the `correlation_id`, matching
    /// Go's behavior where `SetContext` is called at the start of each processing round.
    pub async fn set_context(&self, channel: &str, chat_id: &str) {
        *self.channel.lock().await = channel.to_string();
        *self.chat_id.lock().await = chat_id.to_string();
        *self.sent_in_round.lock().await = false;
        *self.correlation_id.lock().await = String::new();
    }

    /// Set the send callback function.
    ///
    /// The callback receives `(channel, chat_id, content)` and should deliver
    /// the message to the appropriate destination. Returns `Ok(())` on success
    /// or `Err(message)` on failure.
    pub async fn set_send_callback(&self, callback: SendCallback) {
        *self.send_callback.lock().await = Some(callback);
    }

    /// Set the correlation ID for RPC channel messages.
    ///
    /// When set and the target channel is "rpc", the message content will be
    /// automatically prefixed with `[rpc:<correlation_id>]`.
    ///
    /// Note: This is a per-invocation value. It is also set via the
    /// `ContextualTool::set_context` method which receives it from the
    /// registry's side-channel.
    pub async fn set_correlation_id(&self, id: &str) {
        *self.correlation_id.lock().await = id.to_string();
    }

    /// Check if a message was already sent in this round.
    ///
    /// Equivalent to Go's `HasSentInRound()`.
    pub async fn has_sent_in_round(&self) -> bool {
        *self.sent_in_round.lock().await
    }

    /// Alias for `has_sent_in_round()` for backward compatibility.
    pub async fn was_sent(&self) -> bool {
        self.has_sent_in_round().await
    }

    /// Reset the sent flag for a new round.
    pub async fn reset_round(&self) {
        *self.sent_in_round.lock().await = false;
    }
}

impl Default for MessageTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for MessageTool {
    fn name(&self) -> &str {
        "message"
    }

    fn description(&self) -> &str {
        "Send a message to user on a chat channel. Use this when you want to communicate something."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "The message content to send"
                },
                "channel": {
                    "type": "string",
                    "description": "Optional: target channel (telegram, whatsapp, etc.)"
                },
                "chat_id": {
                    "type": "string",
                    "description": "Optional: target chat/user ID"
                }
            },
            "required": ["content"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let content = match args["content"].as_str() {
            Some(c) if !c.is_empty() => c,
            _ => return ToolResult::error("content is required"),
        };

        // Resolve channel and chat_id: args override > defaults
        let arg_channel = args["channel"].as_str().unwrap_or("");
        let arg_chat_id = args["chat_id"].as_str().unwrap_or("");

        let channel = if !arg_channel.is_empty() {
            arg_channel.to_string()
        } else {
            self.channel.lock().await.clone()
        };
        let chat_id = if !arg_chat_id.is_empty() {
            arg_chat_id.to_string()
        } else {
            self.chat_id.lock().await.clone()
        };

        if channel.is_empty() || chat_id.is_empty() {
            return ToolResult::error("No target channel/chat specified");
        }

        // Check if callback is configured BEFORE formatting content.
        // This matches Go's behavior: nil callback returns an error.
        let callback_guard = self.send_callback.lock().await;
        if callback_guard.is_none() {
            return ToolResult::error("Message sending not configured");
        }

        // Determine final content - add RPC correlation prefix if needed
        let correlation_id = self.correlation_id.lock().await.clone();
        let final_content = if channel == "rpc" && !correlation_id.is_empty() {
            debug!(
                correlation_id = %correlation_id,
                "MessageTool: Added correlation ID prefix to RPC message"
            );
            format_rpc_prefix(&correlation_id, content)
        } else if channel == "rpc" {
            warn!("MessageTool: No correlation ID in context for RPC channel - response will not be delivered!");
            content.to_string()
        } else {
            content.to_string()
        };

        // Safe to unwrap: we checked is_none() above
        let callback = callback_guard.as_ref().unwrap();
        match callback(&channel, &chat_id, &final_content) {
            Ok(()) => {
                *self.sent_in_round.lock().await = true;
                // Silent: user already received the message directly
                ToolResult {
                    for_llm: format!("Message sent to {}:{}", channel, chat_id),
                    for_user: None,
                    silent: true,
                    is_error: false,
                    is_async: false,
                    task_id: None,
                }
            }
            Err(e) => ToolResult::error(&format!("sending message: {}", e)),
        }
    }
}

/// Implement `ContextualTool` so the registry can inject per-invocation context
/// via its side-channel before each execution. This is the primary mechanism for
/// setting channel, chat_id, and correlation_id without the race conditions that
/// would arise from shared mutable state.
impl ContextualTool for MessageTool {
    fn set_context(&mut self, ctx: &ToolExecutionContext) {
        // Use try_lock since this is called synchronously from the registry.
        // In practice, the registry calls set_context before execute(), so
        // contention is minimal.
        if let Ok(mut ch) = self.channel.try_lock() {
            *ch = ctx.channel.clone();
        }
        if let Ok(mut cid) = self.chat_id.try_lock() {
            *cid = ctx.chat_id.clone();
        }
        if let Ok(mut corr) = self.correlation_id.try_lock() {
            *corr = ctx.correlation_id.clone();
        }
        if let Ok(mut sent) = self.sent_in_round.try_lock() {
            *sent = false;
        }
    }
}

#[cfg(test)]
mod tests {
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
}

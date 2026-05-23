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
                "[Tools] Added correlation ID prefix to RPC message"
            );
            format_rpc_prefix(&correlation_id, content)
        } else if channel == "rpc" {
            warn!("[Tools] No correlation ID in context for RPC channel - response will not be delivered!");
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
mod tests;

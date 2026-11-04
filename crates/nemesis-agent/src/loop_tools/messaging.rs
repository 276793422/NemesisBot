// ---------------------------------------------------------------------------
// 2026-11-05 机械拆分：本文件内容由 loop_tools.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

/// Callback type for the MessageTool to publish outbound messages.
///
/// Arguments: (channel, chat_id, content)
pub type SendCallback = Box<dyn Fn(&str, &str, &str) + Send + Sync>;

/// A tool that sends a message back to the user via the outbound message bus.
///
/// When a `send_callback` is set, the tool will:
/// 1. Extract the content from the arguments
/// 2. Format it with the RPC correlation ID prefix if applicable
/// 3. Call the callback to publish the outbound message
/// 4. Return the content as the tool result
///
/// If no callback is set, it behaves as a simple passthrough (returns content).
pub struct MessageTool {
    /// Optional callback to publish outbound messages.
    send_callback: Arc<Mutex<Option<SendCallback>>>,
    /// Tracks whether a message was already sent in the current round.
    sent_in_round: Arc<std::sync::atomic::AtomicBool>,
    /// Stored channel from set_context (used when RequestContext is insufficient).
    stored_channel: Arc<std::sync::Mutex<String>>,
    /// Stored chat_id from set_context.
    stored_chat_id: Arc<std::sync::Mutex<String>>,
}

impl MessageTool {
    /// Create a new MessageTool without a send callback.
    pub fn new() -> Self {
        Self {
            send_callback: Arc::new(Mutex::new(None)),
            sent_in_round: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            stored_channel: Arc::new(std::sync::Mutex::new(String::new())),
            stored_chat_id: Arc::new(std::sync::Mutex::new(String::new())),
        }
    }

    /// Set the send callback for publishing outbound messages.
    pub fn set_send_callback(&self, callback: SendCallback) {
        let cb = self.send_callback.clone();
        // Use blocking lock since this is called during setup
        if let Ok(mut guard) = cb.try_lock() {
            *guard = Some(callback);
        }
    }

    /// Check whether a message was already sent in this round.
    pub fn has_sent_in_round(&self) -> bool {
        self.sent_in_round
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Reset the sent-in-round flag (called at the start of each LLM iteration).
    pub fn reset_sent_in_round(&self) {
        self.sent_in_round
            .store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Default for MessageTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for MessageTool {
    fn description(&self) -> String {
        "Send a message to user on a chat channel. Use this when you want to communicate something."
            .to_string()
    }

    // P0 vault（D4）：消息发送参与 mass_message 类别限额——群发风暴的
    // 量化刹车（配置缺省全关，声明本身零行为）。
    fn limit_categories(&self) -> &[&str] {
        &["mass_message"]
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "The message content to send to the user"
                }
            },
            "required": ["content"]
        })
    }

    fn set_context(&self, channel: &str, chat_id: &str) {
        if let Ok(mut guard) = self.stored_channel.lock() {
            *guard = channel.to_string();
        }
        if let Ok(mut guard) = self.stored_chat_id.lock() {
            *guard = chat_id.to_string();
        }
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        // Extract content from arguments.
        let content = if let Ok(val) = serde_json::from_str::<serde_json::Value>(args) {
            if let Some(c) = val.get("content").and_then(|v| v.as_str()) {
                c.to_string()
            } else {
                args.to_string()
            }
        } else {
            args.to_string()
        };

        // Use context from RequestContext, falling back to stored context if needed.
        let channel = if context.channel.is_empty() {
            self.stored_channel
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        } else {
            context.channel.clone()
        };
        let chat_id = if context.chat_id.is_empty() {
            self.stored_chat_id
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        } else {
            context.chat_id.clone()
        };

        // If a send callback is registered, publish the outbound message.
        let guard = self.send_callback.lock().await;
        if let Some(ref callback) = *guard {
            // Format with RPC prefix if applicable.
            let formatted = context.format_rpc_message(&content);
            callback(&channel, &chat_id, &formatted);
            self.sent_in_round
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }

        Ok(content)
    }
}

//! NemesisBot - Observer Framework
//!
//! Event-driven observation system for tracking agent conversation lifecycle.
//! Supports both async (Emit) and sync (EmitSync) event delivery.

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::warn;

/// Event type identifiers for conversation lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventType {
    ConversationStart,
    ConversationEnd,
    LlmRequest,
    LlmResponse,
    ToolCall,
}

impl std::fmt::Display for EventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConversationStart => write!(f, "conversation_start"),
            Self::ConversationEnd => write!(f, "conversation_end"),
            Self::LlmRequest => write!(f, "llm_request"),
            Self::LlmResponse => write!(f, "llm_response"),
            Self::ToolCall => write!(f, "tool_call"),
        }
    }
}

/// A conversation event with typed data.
#[derive(Debug, Clone)]
pub struct ConversationEvent {
    pub event_type: EventType,
    pub trace_id: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub data: EventData,
}

/// Typed event data payloads.
#[derive(Debug, Clone)]
pub enum EventData {
    ConversationStart(ConversationStartData),
    ConversationEnd(ConversationEndData),
    LlmRequest(LlmRequestData),
    LlmResponse(LlmResponseData),
    ToolCall(ToolCallData),
}

/// Data for conversation start events.
#[derive(Debug, Clone)]
pub struct ConversationStartData {
    pub session_key: String,
    pub channel: String,
    pub chat_id: String,
    pub sender_id: String,
    pub content: String,
}

/// Data for conversation end events.
#[derive(Debug, Clone)]
pub struct ConversationEndData {
    pub session_key: String,
    pub channel: String,
    pub chat_id: String,
    pub total_rounds: u32,
    pub total_duration: Duration,
    pub content: String,
    pub error: Option<String>,
}

/// Data for LLM request events.
///
/// Mirrors Go `LLMRequestData` from module/observer/observer.go.
/// The `messages` and `tools` fields contain the full conversation context
/// sent to the LLM, while `messages_count` and `tools_count` are convenience
/// fields for quick access.
#[derive(Debug, Clone)]
pub struct LlmRequestData {
    pub round: u32,
    pub model: String,
    pub provider_name: String,
    pub api_key: String,
    pub api_base: String,
    pub http_headers: HashMap<String, String>,
    /// Full provider configuration as a JSON value.
    pub full_config: Option<serde_json::Value>,
    /// Full message list sent to the LLM (serialized as JSON values).
    pub messages: Vec<serde_json::Value>,
    /// Full tool definitions sent to the LLM (serialized as JSON values).
    pub tools: Vec<serde_json::Value>,
    /// Convenience: number of messages (mirrors Go's len(Messages)).
    pub messages_count: usize,
    /// Convenience: number of tools (mirrors Go's len(Tools)).
    pub tools_count: usize,
}

/// Data for LLM response events.
///
/// Mirrors Go `LLMResponseData` from module/observer/observer.go.
/// Contains the full tool calls list and usage info in addition to
/// the convenience `tool_calls_count` field.
#[derive(Debug, Clone)]
pub struct LlmResponseData {
    pub round: u32,
    pub duration: Duration,
    pub content: String,
    /// Full tool calls from the LLM response (serialized as JSON values).
    pub tool_calls: Vec<serde_json::Value>,
    /// Convenience: number of tool calls.
    pub tool_calls_count: usize,
    /// Token usage information.
    pub usage: Option<UsageInfo>,
    pub finish_reason: Option<String>,
    /// Raw HTTP request body sent to the LLM API (for logging).
    pub raw_request_body: Option<serde_json::Value>,
    /// Raw HTTP response body received from the LLM API (for logging).
    pub raw_response_body: Option<String>,
}

/// Token usage information, mirroring Go's `providers.UsageInfo`.
#[derive(Debug, Clone, Default)]
pub struct UsageInfo {
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
    /// Cached prompt tokens (DeepSeek: prompt_cache_hit_tokens, OpenAI: cached_tokens).
    pub cached_tokens: Option<i64>,
    /// Cache creation tokens (Anthropic: cache_creation_input_tokens).
    pub cache_creation_tokens: Option<i64>,
    /// Cache read tokens (Anthropic: cache_read_input_tokens).
    pub cache_read_tokens: Option<i64>,
}

/// Data for tool call events.
///
/// Mirrors Go `ToolCallData` from module/observer/observer.go.
#[derive(Debug, Clone)]
pub struct ToolCallData {
    pub tool_name: String,
    /// Full tool call arguments as a JSON object (mirrors Go's `Arguments map[string]interface{}`).
    pub arguments: HashMap<String, serde_json::Value>,
    pub success: bool,
    pub duration: Duration,
    pub error: Option<String>,
    pub llm_round: u32,
    pub chain_pos: u32,
}

/// Observer trait for receiving conversation events.
#[async_trait]
pub trait Observer: Send + Sync {
    /// Name of the observer for identification.
    fn name(&self) -> &str;

    /// Handle a conversation event.
    async fn on_event(&self, event: ConversationEvent);
}

/// Manager for multiple observers with async and sync delivery.
pub struct Manager {
    observers: Arc<RwLock<Vec<Arc<dyn Observer>>>>,
}

impl Manager {
    /// Create a new observer manager.
    pub fn new() -> Self {
        Self {
            observers: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Register an observer.
    pub async fn register(&self, observer: Arc<dyn Observer>) {
        let mut obs = self.observers.write().await;
        obs.push(observer);
    }

    /// Unregister an observer by name.
    pub async fn unregister(&self, name: &str) {
        let mut obs = self.observers.write().await;
        obs.retain(|o| o.name() != name);
    }

    /// Emit an event to all observers asynchronously.
    /// Each observer runs in its own tokio task.
    /// Matches Go's `Emit()` which spawns a goroutine with `defer recover()`.
    /// Tokio's task runtime already catches panics in spawned tasks.
    pub async fn emit(&self, event: ConversationEvent) {
        let observers = self.observers.read().await;
        for obs in observers.iter() {
            let o = Arc::clone(obs);
            let e = event.clone();
            tokio::spawn(async move {
                o.on_event(e).await;
            });
        }
    }

    /// Emit an event to all observers synchronously.
    /// Use for events where all observers must complete before proceeding.
    /// Matches Go's `EmitSync()` with `defer recover()` wrapping each call.
    /// Each observer is spawned in its own task; if one panics, the rest still run.
    pub async fn emit_sync(&self, event: ConversationEvent) {
        let observers = self.observers.read().await;
        for obs in observers.iter() {
            let o = Arc::clone(obs);
            let e = event.clone();
            let name = o.name().to_string();
            // Spawn and await sequentially, with panic recovery via JoinHandle
            let handle = tokio::spawn(async move {
                o.on_event(e).await;
            });
            if let Err(err) = handle.await {
                if err.is_panic() {
                    warn!("Observer {} panicked during emit_sync", name);
                }
            }
        }
    }

    /// Unregister all observers.
    pub async fn unregister_all(&self) {
        let mut obs = self.observers.write().await;
        obs.clear();
    }

    /// Check if any observers are registered.
    pub async fn has_observers(&self) -> bool {
        let obs = self.observers.read().await;
        !obs.is_empty()
    }
}

impl Default for Manager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;

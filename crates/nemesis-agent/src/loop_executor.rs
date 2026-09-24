//! Type definitions shared by the production agent loop (`crate::r#loop`).
//!
//! This module contains **only** the observer-event and tool-result types that
//! production code imports:
//! - `ObserverUsageInfo` / `ObserverEvent` — emitted by `loop.rs`; consumed by
//!   `session.rs`, `loop_continuation.rs`, and `nemesis-web/src/llm_bridge.rs`
//! - `Observer` — legacy single-observer sink trait (test mocks)
//! - `ToolResult` — rich tool execution result produced by tools in `loop.rs`
//!
//! The former `AgentLoopExecutor` runtime that lived here was removed
//! (2026-09-23): it was superseded by `crate::r#loop::AgentLoop` in an earlier
//! refactor and was never instantiated in production — only its own tests
//! constructed it, so any edit there silently had no effect on live behaviour
//! (route such changes through `loop.rs`).
//!
//! **Do not add runtime behaviour here; put it in `loop.rs`.**

// ===========================================================================
// Observer events (wrapping nemesis-observer types for async emission)
// ===========================================================================

/// Token usage info carried through observer events.
#[derive(Debug, Clone, Default)]
pub struct ObserverUsageInfo {
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
    /// Cached prompt tokens (DeepSeek/OpenAI prefix caching).
    pub cached_tokens: Option<i64>,
    /// Cache creation tokens (Anthropic).
    pub cache_creation_tokens: Option<i64>,
    /// Cache read tokens (Anthropic).
    pub cache_read_tokens: Option<i64>,
}

/// An event emitted by the observer system.
///
/// This wraps the `nemesis_observer::ConversationEvent` types into a
/// self-contained enum that can be emitted synchronously or asynchronously
/// through the `ObserverManager`.
#[derive(Debug, Clone)]
pub enum ObserverEvent {
    /// Conversation started.
    ConversationStart {
        trace_id: String,
        session_key: String,
        channel: String,
        chat_id: String,
        sender_id: String,
        content: String,
    },
    /// Conversation ended.
    ConversationEnd {
        trace_id: String,
        session_key: String,
        total_rounds: u32,
        duration_ms: u64,
        content: String,
        channel: String,
        chat_id: String,
    },
    /// LLM request sent.
    LlmRequest {
        trace_id: String,
        round: u32,
        model: String,
        messages: Vec<serde_json::Value>,
        tools: Vec<serde_json::Value>,
        messages_count: usize,
        tools_count: usize,
        provider_name: String,
        api_key: String,
        api_base: String,
    },
    /// LLM response received.
    LlmResponse {
        trace_id: String,
        round: u32,
        duration_ms: u64,
        has_tool_calls: bool,
        content: String,
        tool_calls: Vec<serde_json::Value>,
        tool_calls_count: usize,
        finish_reason: Option<String>,
        /// Token usage from the provider response.
        usage: Option<ObserverUsageInfo>,
        /// Raw HTTP request body (for raw logging mode).
        raw_request_body: Option<serde_json::Value>,
        /// Raw HTTP response body (for raw logging mode).
        raw_response_body: Option<String>,
    },
    /// Tool call executed.
    ToolCall {
        trace_id: String,
        tool_name: String,
        success: bool,
        duration_ms: u64,
        round: u32,
        arguments: String,
        result: String,
    },
}

impl ObserverEvent {
    /// Convert to a `nemesis_observer::ConversationEvent`.
    pub(crate) fn to_conversation_event(&self) -> nemesis_observer::ConversationEvent {
        use nemesis_observer::*;
        match self {
            ObserverEvent::ConversationStart {
                trace_id,
                session_key,
                channel,
                chat_id,
                sender_id,
                content,
            } => ConversationEvent {
                event_type: EventType::ConversationStart,
                trace_id: trace_id.clone(),
                timestamp: chrono::Local::now(),
                data: EventData::ConversationStart(ConversationStartData {
                    session_key: session_key.clone(),
                    channel: channel.clone(),
                    chat_id: chat_id.clone(),
                    sender_id: sender_id.clone(),
                    content: content.clone(),
                }),
            },
            ObserverEvent::ConversationEnd {
                trace_id,
                session_key,
                total_rounds,
                duration_ms,
                content,
                channel,
                chat_id,
            } => ConversationEvent {
                event_type: EventType::ConversationEnd,
                trace_id: trace_id.clone(),
                timestamp: chrono::Local::now(),
                data: EventData::ConversationEnd(ConversationEndData {
                    session_key: session_key.clone(),
                    channel: channel.clone(),
                    chat_id: chat_id.clone(),
                    total_rounds: *total_rounds,
                    total_duration: std::time::Duration::from_millis(*duration_ms),
                    content: content.clone(),
                    error: None,
                }),
            },
            ObserverEvent::LlmRequest {
                trace_id,
                round,
                model,
                messages,
                tools,
                messages_count,
                tools_count,
                provider_name,
                api_key,
                api_base,
            } => ConversationEvent {
                event_type: EventType::LlmRequest,
                trace_id: trace_id.clone(),
                timestamp: chrono::Local::now(),
                data: EventData::LlmRequest(LlmRequestData {
                    round: *round,
                    model: model.clone(),
                    provider_name: provider_name.clone(),
                    api_key: api_key.clone(),
                    api_base: api_base.clone(),
                    http_headers: std::collections::HashMap::new(),
                    full_config: None,
                    messages: messages.clone(),
                    tools: tools.clone(),
                    messages_count: *messages_count,
                    tools_count: *tools_count,
                }),
            },
            ObserverEvent::LlmResponse {
                trace_id,
                round,
                duration_ms,
                has_tool_calls: _has_tool_calls,
                content,
                tool_calls,
                tool_calls_count,
                finish_reason,
                usage,
                raw_request_body,
                raw_response_body,
            } => ConversationEvent {
                event_type: EventType::LlmResponse,
                trace_id: trace_id.clone(),
                timestamp: chrono::Local::now(),
                data: EventData::LlmResponse(LlmResponseData {
                    round: *round,
                    duration: std::time::Duration::from_millis(*duration_ms),
                    content: content.clone(),
                    tool_calls: tool_calls.clone(),
                    tool_calls_count: *tool_calls_count,
                    usage: usage.as_ref().map(|u| nemesis_observer::UsageInfo {
                        prompt_tokens: u.prompt_tokens,
                        completion_tokens: u.completion_tokens,
                        total_tokens: u.total_tokens,
                        cached_tokens: u.cached_tokens,
                        cache_creation_tokens: u.cache_creation_tokens,
                        cache_read_tokens: u.cache_read_tokens,
                    }),
                    finish_reason: finish_reason.clone(),
                    raw_request_body: raw_request_body.clone(),
                    raw_response_body: raw_response_body.clone(),
                }),
            },
            ObserverEvent::ToolCall {
                trace_id,
                tool_name,
                success,
                duration_ms,
                round,
                arguments,
                result,
            } => {
                // Parse arguments JSON string into HashMap for ToolCallData.
                let args_map: std::collections::HashMap<String, serde_json::Value> =
                    serde_json::from_str(arguments).unwrap_or_default();
                ConversationEvent {
                    event_type: EventType::ToolCall,
                    trace_id: trace_id.clone(),
                    timestamp: chrono::Local::now(),
                    data: EventData::ToolCall(ToolCallData {
                        tool_name: tool_name.clone(),
                        arguments: args_map,
                        success: *success,
                        duration: std::time::Duration::from_millis(*duration_ms),
                        error: if *success { None } else { Some(result.clone()) },
                        llm_round: *round,
                        chain_pos: 0,
                        // Keep the full result on the success path too, so
                        // observers can scan tool outputs (credentials / DLP).
                        result: Some(result.clone()),
                    }),
                }
            }
        }
    }

    /// Convert to a legacy callback (event_type, JSON data) pair.
    ///
    /// Used by `AgentLoop`'s legacy `observer_callback` field to emit
    /// the same events through the simpler callback interface.
    pub(crate) fn to_callback_json(&self) -> (&'static str, serde_json::Value) {
        match self {
            ObserverEvent::ConversationStart {
                trace_id,
                session_key,
                channel,
                chat_id,
                sender_id,
                content,
            } => (
                "conversation_start",
                serde_json::json!({
                    "type": "conversation_start",
                    "trace_id": trace_id,
                    "session_key": session_key,
                    "channel": channel,
                    "chat_id": chat_id,
                    "sender_id": sender_id,
                    "content": content,
                }),
            ),
            ObserverEvent::ConversationEnd {
                trace_id,
                session_key,
                total_rounds,
                duration_ms,
                content,
                channel,
                chat_id,
            } => (
                "conversation_end",
                serde_json::json!({
                    "type": "conversation_end",
                    "trace_id": trace_id,
                    "session_key": session_key,
                    "total_rounds": total_rounds,
                    "duration_ms": duration_ms,
                    "content": content,
                    "channel": channel,
                    "chat_id": chat_id,
                }),
            ),
            ObserverEvent::LlmRequest {
                trace_id,
                round,
                model,
                messages,
                tools,
                messages_count,
                tools_count,
                provider_name,
                api_key,
                api_base,
            } => (
                "llm_request",
                serde_json::json!({
                    "type": "llm_request",
                    "trace_id": trace_id,
                    "round": round,
                    "model": model,
                    "messages_count": messages_count,
                    "tools_count": tools_count,
                    "provider_name": provider_name,
                    "api_key": api_key,
                    "api_base": api_base,
                    "messages": messages,
                    "tools": tools,
                }),
            ),
            ObserverEvent::LlmResponse {
                trace_id,
                round,
                duration_ms,
                has_tool_calls,
                content,
                tool_calls,
                tool_calls_count,
                finish_reason,
                usage,
                raw_request_body: _,
                raw_response_body: _,
            } => (
                "llm_response",
                serde_json::json!({
                    "type": "llm_response",
                    "trace_id": trace_id,
                    "round": round,
                    "duration_ms": duration_ms,
                    "has_tool_calls": has_tool_calls,
                    "content": content,
                    "tool_calls": tool_calls,
                    "tool_calls_count": tool_calls_count,
                    "finish_reason": finish_reason,
                    "usage": usage.as_ref().map(|u| serde_json::json!({
                        "prompt_tokens": u.prompt_tokens,
                        "completion_tokens": u.completion_tokens,
                        "total_tokens": u.total_tokens,
                        "cached_tokens": u.cached_tokens,
                        "cache_creation_tokens": u.cache_creation_tokens,
                        "cache_read_tokens": u.cache_read_tokens,
                    })),
                }),
            ),
            ObserverEvent::ToolCall {
                trace_id,
                tool_name,
                success,
                duration_ms,
                round,
                arguments,
                result,
            } => (
                "tool_call",
                serde_json::json!({
                    "type": "tool_call",
                    "trace_id": trace_id,
                    "tool_name": tool_name,
                    "success": success,
                    "duration_ms": duration_ms,
                    "round": round,
                    "arguments": arguments,
                    "result": result,
                }),
            ),
        }
    }
}

/// Trait for observer event sinks (legacy single-observer interface).
///
/// Prefer using `ObserverManager` from `nemesis-observer` for multi-observer support.
/// This trait is retained for backward compatibility with test mocks.
pub trait Observer: Send + Sync {
    /// Handle an observer event (async, non-blocking).
    fn on_event(&self, event: ObserverEvent);
}

// ===========================================================================
// ToolResult -- complex result types
// ===========================================================================

/// Complex result from tool execution with ForUser/ForLLM/Async separation.
///
/// Mirrors Go's `tools.ToolResult` struct. Tools can produce content for
/// different audiences:
/// - `for_llm`: Content fed back to the LLM for the next iteration
/// - `for_user`: Content sent immediately to the user (if not silent)
/// - `is_async`: Whether the tool result is from an async operation
/// - `task_id`: Task ID for async operations (for continuation snapshots)
/// - `silent`: Whether to suppress user-facing output
#[derive(Debug, Clone)]
pub struct ToolResult {
    /// Content to be fed back to the LLM as a tool result message.
    pub for_llm: String,
    /// Content to be sent to the user immediately (if not silent).
    pub for_user: String,
    /// Whether this is an async result that will complete later.
    pub is_async: bool,
    /// Task ID for async operations (used for continuation snapshots).
    pub task_id: String,
    /// Whether to suppress user-facing output.
    pub silent: bool,
    /// Error, if the tool execution failed.
    pub err: Option<String>,
}

impl Default for ToolResult {
    fn default() -> Self {
        Self {
            for_llm: String::new(),
            for_user: String::new(),
            is_async: false,
            task_id: String::new(),
            silent: true,
            err: None,
        }
    }
}

impl ToolResult {
    /// Create a simple synchronous result — content goes to LLM only.
    /// The final LLM response will be sent to the user.
    pub fn simple(content: String) -> Self {
        Self {
            for_llm: content,
            for_user: String::new(),
            silent: true,
            ..Default::default()
        }
    }

    /// Create a result intended only for the LLM (not shown to user).
    pub fn for_llm_only(content: String) -> Self {
        Self {
            for_llm: content,
            silent: true,
            ..Default::default()
        }
    }

    /// Create an async result with a task ID.
    pub fn async_result(task_id: String, interim_for_user: String) -> Self {
        Self {
            for_llm: format!("Async task submitted: {}", task_id),
            for_user: interim_for_user,
            is_async: true,
            task_id,
            silent: false,
            ..Default::default()
        }
    }

    /// Create an error result.
    pub fn error(err: String) -> Self {
        Self {
            for_llm: format!("Error: {}", err),
            err: Some(err),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests;

//! Request logger observer: adapts RequestLogger to the observer pattern.
//!
//! `RequestLoggerObserver` creates a new `RequestLogger` per conversation
//! (via trace ID mapping) to maintain session isolation. It listens for
//! conversation events and dispatches them to the appropriate logger instance.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::Local;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::request_logger::{
    FinalResponseInfo, LLMRequestInfo, LLMResponseInfo, LocalOperationInfo, LoggingConfig,
    OperationInfo, RequestLogger, UserRequestInfo,
};

/// Conversation event types emitted during the agent loop.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EventType {
    ConversationStart,
    ConversationEnd,
    LLMRequest,
    LLMResponse,
    ToolCall,
}

/// A conversation event carrying typed data.
#[derive(Debug, Clone)]
pub struct ConversationEvent {
    pub event_type: EventType,
    pub trace_id: String,
    pub timestamp: chrono::DateTime<Local>,
    pub data: EventData,
}

/// Typed data payload for conversation events.
#[derive(Debug, Clone)]
pub enum EventData {
    ConversationStart(ConversationStartData),
    ConversationEnd(ConversationEndData),
    LLMRequest(LLMRequestEventData),
    LLMResponse(LLMResponseEventData),
    ToolCall(ToolCallEventData),
}

/// Data for conversation_start events.
#[derive(Debug, Clone)]
pub struct ConversationStartData {
    pub session_key: String,
    pub channel: String,
    pub chat_id: String,
    pub sender_id: String,
    pub content: String,
}

/// Data for conversation_end events.
#[derive(Debug, Clone)]
pub struct ConversationEndData {
    pub session_key: String,
    pub channel: String,
    pub chat_id: String,
    pub total_rounds: usize,
    pub total_duration_ms: u64,
    pub content: String,
    pub is_error: bool,
}

/// Data for llm_request events.
#[derive(Debug, Clone)]
pub struct LLMRequestEventData {
    pub round: usize,
    pub model: String,
    pub provider_name: String,
    pub api_key: String,
    pub api_base: String,
    pub messages_count: usize,
    pub tools_count: usize,
    /// Serialized messages sent to LLM.
    pub messages: Vec<serde_json::Value>,
    /// Serialized tool definitions sent to LLM.
    pub tools: Vec<serde_json::Value>,
}

/// Data for llm_response events.
#[derive(Debug, Clone)]
pub struct LLMResponseEventData {
    pub round: usize,
    pub duration_ms: u64,
    pub content: String,
    pub tool_calls_count: usize,
    pub finish_reason: String,
    /// Serialized tool call details from the response.
    pub tool_calls: Vec<serde_json::Value>,
    /// Token usage from the provider response.
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
    pub cached_tokens: Option<i64>,
    /// Raw HTTP request body (for raw logging mode).
    pub raw_request_body: Option<serde_json::Value>,
    /// Raw HTTP response body (for raw logging mode).
    pub raw_response_body: Option<String>,
}
#[derive(Debug, Clone)]
pub struct ToolCallEventData {
    pub tool_name: String,
    pub success: bool,
    pub duration_ms: u64,
    pub error: String,
    pub llm_round: usize,
    /// Tool arguments (JSON).
    pub arguments: String,
    /// Tool execution result.
    pub result: String,
}

/// Per-conversation state tracked by the observer.
struct ConversationState {
    logger: RequestLogger,
    start_time: chrono::DateTime<Local>,
    /// Timestamp captured at LlmRequest event (for raw logging mode).
    last_request_time: Option<chrono::DateTime<Local>>,
}

/// Observer adapter that creates a new RequestLogger per conversation
/// and dispatches events to it.
pub struct RequestLoggerObserver {
    config: LoggingConfig,
    workspace: PathBuf,
    active: Mutex<HashMap<String, ConversationState>>,
}

impl RequestLoggerObserver {
    /// Create a new RequestLoggerObserver with the given logging config and workspace path.
    pub fn new(config: LoggingConfig, workspace: &Path) -> Self {
        Self {
            config,
            workspace: workspace.to_path_buf(),
            active: Mutex::new(HashMap::new()),
        }
    }

    /// Returns the observer name.
    pub fn name(&self) -> &str {
        "request_logger"
    }

    /// Handle a conversation event.
    pub fn on_event(&self, event: &ConversationEvent) {
        match event.event_type {
            EventType::ConversationStart => {
                if let EventData::ConversationStart(ref data) = event.data {
                    self.handle_conversation_start(&event.trace_id, event.timestamp, data);
                }
            }
            EventType::LLMRequest => {
                if let EventData::LLMRequest(ref data) = event.data {
                    self.handle_llm_request(&event.trace_id, data);
                }
            }
            EventType::LLMResponse => {
                if let EventData::LLMResponse(ref data) = event.data {
                    self.handle_llm_response(&event.trace_id, data);
                }
            }
            EventType::ToolCall => {
                if let EventData::ToolCall(ref data) = event.data {
                    self.handle_tool_call(&event.trace_id, data);
                }
            }
            EventType::ConversationEnd => {
                if let EventData::ConversationEnd(ref data) = event.data {
                    self.handle_conversation_end(&event.trace_id, event.timestamp, data);
                }
            }
        }
    }

    fn handle_conversation_start(
        &self,
        trace_id: &str,
        timestamp: chrono::DateTime<Local>,
        data: &ConversationStartData,
    ) {
        let logger = RequestLogger::new(self.config.clone(), &self.workspace);
        if !logger.is_enabled() {
            return;
        }
        if let Err(e) = logger.create_session() {
            warn!("[RequestLogger] Failed to create logging session: {}", e);
            return;
        }
        logger.log_user_request(&UserRequestInfo {
            timestamp,
            channel: data.channel.clone(),
            sender_id: data.sender_id.clone(),
            chat_id: data.chat_id.clone(),
            content: data.content.clone(),
        });

        let mut active = self.active.lock().unwrap();
        active.insert(
            trace_id.to_string(),
            ConversationState {
                logger,
                start_time: timestamp,
                last_request_time: None,
            },
        );
    }

    fn handle_llm_request(&self, trace_id: &str, data: &LLMRequestEventData) {
        let mut active = self.active.lock().unwrap();
        if let Some(state) = active.get_mut(trace_id) {
            if self.config.save_raw {
                // Raw mode: write request file immediately so it's captured
                // even if the LLM call fails. Record timestamp for response file.
                state.last_request_time = Some(chrono::Local::now());
                let envelope = serde_json::json!({
                    "timestamp": chrono::Local::now().to_rfc3339(),
                    "round": data.round,
                    "body": {
                        "model": data.model,
                        "messages": data.messages,
                        "tools": data.tools,
                        "messages_count": data.messages_count,
                        "tools_count": data.tools_count,
                    }
                });
                state.logger.log_raw_request_envelope(&envelope);
            } else {
                // Convert serde_json::Value messages to LlmMessage structs.
                let messages: Vec<crate::r#loop::LlmMessage> = data.messages.iter()
                    .filter_map(|v| serde_json::from_value(v.clone()).ok())
                    .collect();

                state.logger.log_llm_request(&LLMRequestInfo {
                    round: data.round,
                    timestamp: Local::now(),
                    model: data.model.clone(),
                    provider_name: data.provider_name.clone(),
                    api_key: data.api_key.clone(),
                    api_base: data.api_base.clone(),
                    messages_count: data.messages_count,
                    tools_count: data.tools_count,
                    messages,
                    http_headers: Vec::new(),
                    config: std::collections::HashMap::new(),
                    fallback_attempts: Vec::new(),
                });
            }
        }
    }

    fn handle_llm_response(&self, trace_id: &str, data: &LLMResponseEventData) {
        let mut active = self.active.lock().unwrap();
        if let Some(state) = active.get_mut(trace_id) {
            if self.config.save_raw {
                // Raw mode: write response file only (request already written in handle_llm_request)
                let response_time = chrono::Local::now();
                if let Some(ref resp_body) = data.raw_response_body {
                    state.logger.log_raw_response(resp_body, response_time, data.round, data.duration_ms);
                }
            } else {
                // Markdown mode: existing logic
                let tool_calls: Vec<crate::request_logger::ToolCallDetail> = data.tool_calls.iter()
                    .filter_map(|v| {
                        let id = v.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let name = v.get("name").or_else(|| v.get("function").and_then(|f| f.get("name"))).and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let arguments = v.get("arguments").or_else(|| v.get("function").and_then(|f| f.get("arguments"))).and_then(|v| v.as_str()).unwrap_or("").to_string();
                        if name.is_empty() { None } else { Some(crate::request_logger::ToolCallDetail { id, name, arguments }) }
                    })
                    .collect();

                state.logger.log_llm_response(&LLMResponseInfo {
                    round: data.round,
                    timestamp: Local::now(),
                    duration_ms: data.duration_ms,
                    content: data.content.clone(),
                    tool_calls_count: data.tool_calls_count,
                    finish_reason: data.finish_reason.clone(),
                    tool_calls,
                    usage: crate::request_logger::UsageInfo {
                        prompt_tokens: data.prompt_tokens as u32,
                        completion_tokens: data.completion_tokens as u32,
                        total_tokens: data.total_tokens as u32,
                        cached_tokens: data.cached_tokens.unwrap_or(0) as u32,
                    },
                });
            }
        }
    }

    fn handle_tool_call(&self, trace_id: &str, data: &ToolCallEventData) {
        let mut active = self.active.lock().unwrap();
        if let Some(state) = active.get_mut(trace_id) {
            let op = OperationInfo {
                op_type: "tool_call".to_string(),
                name: data.tool_name.clone(),
                status: if data.success {
                    "Success".to_string()
                } else {
                    "Failed".to_string()
                },
                error: if data.success {
                    String::new()
                } else {
                    data.error.clone()
                },
                duration_ms: data.duration_ms,
                arguments: data.arguments.clone(),
                result: data.result.clone(),
            };
            state.logger.log_local_operations(&LocalOperationInfo {
                round: data.llm_round,
                timestamp: chrono::Local::now(),
                operations: vec![op],
            });
        }
    }

    fn handle_conversation_end(
        &self,
        trace_id: &str,
        timestamp: chrono::DateTime<Local>,
        data: &ConversationEndData,
    ) {
        let mut active = self.active.lock().unwrap();
        if let Some(state) = active.remove(trace_id) {
            let total_duration_ms = if data.total_duration_ms == 0 {
                timestamp
                    .signed_duration_since(state.start_time)
                    .num_milliseconds()
                    .max(0) as u64
            } else {
                data.total_duration_ms
            };

            state.logger.log_final_response(&FinalResponseInfo {
                timestamp,
                total_duration_ms,
                llm_rounds: data.total_rounds,
                content: data.content.clone(),
                channel: data.channel.clone(),
                chat_id: data.chat_id.clone(),
            });
        }
    }

    /// Returns the number of active conversations being tracked.
    pub fn active_count(&self) -> usize {
        self.active.lock().unwrap().len()
    }
}

// ---------------------------------------------------------------------------
// Observer trait implementation — bridges nemesis_observer events to internal
// request_logger_observer events.
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
impl nemesis_observer::Observer for RequestLoggerObserver {
    fn name(&self) -> &str {
        "request_logger"
    }

    async fn on_event(&self, event: nemesis_observer::ConversationEvent) {
        let internal = match convert_event(&event) {
            Some(e) => e,
            None => return,
        };
        self.on_event(&internal);
    }
}

/// Convert a `nemesis_observer::ConversationEvent` into the internal
/// `ConversationEvent` used by `RequestLoggerObserver`.
///
/// Exposed as pub so that other observers (e.g. `ClusterRequestLoggerObserver`
/// in nemesisbot) can reuse the same conversion logic without duplicating it.
pub fn convert_event(src: &nemesis_observer::ConversationEvent) -> Option<ConversationEvent> {
    match &src.data {
        nemesis_observer::EventData::ConversationStart(d) => Some(ConversationEvent {
            event_type: EventType::ConversationStart,
            trace_id: src.trace_id.clone(),
            timestamp: src.timestamp,
            data: EventData::ConversationStart(ConversationStartData {
                session_key: d.session_key.clone(),
                channel: d.channel.clone(),
                chat_id: d.chat_id.clone(),
                sender_id: d.sender_id.clone(),
                content: d.content.clone(),
            }),
        }),
        nemesis_observer::EventData::ConversationEnd(d) => Some(ConversationEvent {
            event_type: EventType::ConversationEnd,
            trace_id: src.trace_id.clone(),
            timestamp: src.timestamp,
            data: EventData::ConversationEnd(ConversationEndData {
                session_key: d.session_key.clone(),
                channel: d.channel.clone(),
                chat_id: d.chat_id.clone(),
                total_rounds: d.total_rounds as usize,
                total_duration_ms: d.total_duration.as_millis() as u64,
                content: d.content.clone(),
                is_error: d.error.is_some(),
            }),
        }),
        nemesis_observer::EventData::LlmRequest(d) => Some(ConversationEvent {
            event_type: EventType::LLMRequest,
            trace_id: src.trace_id.clone(),
            timestamp: src.timestamp,
            data: EventData::LLMRequest(LLMRequestEventData {
                round: d.round as usize,
                model: d.model.clone(),
                provider_name: d.provider_name.clone(),
                api_key: d.api_key.clone(),
                api_base: d.api_base.clone(),
                messages_count: d.messages_count,
                tools_count: d.tools_count,
                messages: d.messages.clone(),
                tools: d.tools.clone(),
            }),
        }),
        nemesis_observer::EventData::LlmResponse(d) => Some(ConversationEvent {
            event_type: EventType::LLMResponse,
            trace_id: src.trace_id.clone(),
            timestamp: src.timestamp,
            data: EventData::LLMResponse(LLMResponseEventData {
                round: d.round as usize,
                duration_ms: d.duration.as_millis() as u64,
                content: d.content.clone(),
                tool_calls_count: d.tool_calls_count,
                finish_reason: d.finish_reason.clone().unwrap_or_default(),
                tool_calls: d.tool_calls.clone(),
                prompt_tokens: d.usage.as_ref().map(|u| u.prompt_tokens).unwrap_or(0),
                completion_tokens: d.usage.as_ref().map(|u| u.completion_tokens).unwrap_or(0),
                total_tokens: d.usage.as_ref().map(|u| u.total_tokens).unwrap_or(0),
                cached_tokens: d.usage.as_ref().and_then(|u| u.cached_tokens),
                raw_request_body: d.raw_request_body.clone(),
                raw_response_body: d.raw_response_body.clone(),
            }),
        }),
        nemesis_observer::EventData::ToolCall(d) => Some(ConversationEvent {
            event_type: EventType::ToolCall,
            trace_id: src.trace_id.clone(),
            timestamp: src.timestamp,
            data: EventData::ToolCall(ToolCallEventData {
                tool_name: d.tool_name.clone(),
                success: d.success,
                duration_ms: d.duration.as_millis() as u64,
                error: d.error.clone().unwrap_or_default(),
                llm_round: d.llm_round as usize,
                arguments: d
                    .arguments
                    .iter()
                    .map(|(k, v)| format!("\"{}\": {}", k, v))
                    .collect::<Vec<_>>()
                    .join(", "),
                result: String::new(),
            }),
        }),
    }
}

#[cfg(test)]
mod tests;

//! Request logger observer: adapts RequestLogger to the observer pattern.
//!
//! `RequestLoggerObserver` creates a new `RequestLogger` per conversation
//! (via trace ID mapping) to maintain session isolation. It listens for
//! conversation events and dispatches them to the appropriate logger instance.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::Utc;
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
    pub timestamp: chrono::DateTime<Utc>,
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
}

/// Data for tool_call events.
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
    operations: HashMap<usize, Vec<OperationInfo>>,
    start_time: chrono::DateTime<Utc>,
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
        timestamp: chrono::DateTime<Utc>,
        data: &ConversationStartData,
    ) {
        let logger = RequestLogger::new(self.config.clone(), &self.workspace);
        if !logger.is_enabled() {
            return;
        }
        if let Err(e) = logger.create_session() {
            warn!("Failed to create logging session: {}", e);
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
                operations: HashMap::new(),
                start_time: timestamp,
            },
        );
    }

    fn handle_llm_request(&self, trace_id: &str, data: &LLMRequestEventData) {
        let active = self.active.lock().unwrap();
        if let Some(state) = active.get(trace_id) {
            // Convert serde_json::Value messages to LlmMessage structs.
            let messages: Vec<crate::r#loop::LlmMessage> = data.messages.iter()
                .filter_map(|v| serde_json::from_value(v.clone()).ok())
                .collect();

            state.logger.log_llm_request(&LLMRequestInfo {
                round: data.round,
                timestamp: Utc::now(),
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

    fn handle_llm_response(&self, trace_id: &str, data: &LLMResponseEventData) {
        let active = self.active.lock().unwrap();
        if let Some(state) = active.get(trace_id) {
            // Convert serde_json::Value tool calls to ToolCallDetail structs.
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
                timestamp: Utc::now(),
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
            state
                .operations
                .entry(data.llm_round)
                .or_insert_with(Vec::new)
                .push(op);
        }
    }

    fn handle_conversation_end(
        &self,
        trace_id: &str,
        timestamp: chrono::DateTime<Utc>,
        data: &ConversationEndData,
    ) {
        let mut active = self.active.lock().unwrap();
        if let Some(state) = active.remove(trace_id) {
            // Flush collected operations per round
            for round in 1..=data.total_rounds {
                if let Some(ops) = state.operations.get(&round) {
                    if !ops.is_empty() {
                        state.logger.log_local_operations(&LocalOperationInfo {
                            round,
                            timestamp,
                            operations: ops.clone(),
                        });
                    }
                }
            }

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
fn convert_event(src: &nemesis_observer::ConversationEvent) -> Option<ConversationEvent> {
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
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_config() -> LoggingConfig {
        LoggingConfig {
            enabled: true,
            detail_level: crate::request_logger::DetailLevel::Full,
            log_dir: "logs/llm".to_string(),
        }
    }

    fn make_start_event(trace: &str, content: &str) -> ConversationEvent {
        ConversationEvent {
            event_type: EventType::ConversationStart,
            trace_id: trace.to_string(),
            timestamp: Utc::now(),
            data: EventData::ConversationStart(ConversationStartData {
                session_key: "test:chat1".to_string(),
                channel: "web".to_string(),
                chat_id: "chat1".to_string(),
                sender_id: "user1".to_string(),
                content: content.to_string(),
            }),
        }
    }

    fn make_llm_request_event(trace: &str, round: usize) -> ConversationEvent {
        ConversationEvent {
            event_type: EventType::LLMRequest,
            trace_id: trace.to_string(),
            timestamp: Utc::now(),
            data: EventData::LLMRequest(LLMRequestEventData {
                round,
                model: "gpt-4".to_string(),
                provider_name: "openai".to_string(),
                api_key: "sk-test".to_string(),
                api_base: "https://api.openai.com".to_string(),
                messages_count: 5,
                tools_count: 3,
                messages: vec![
                    serde_json::json!({"role": "system", "content": "You are helpful"}),
                    serde_json::json!({"role": "user", "content": "Hello"}),
                ],
                tools: vec![
                    serde_json::json!({"type": "function", "function": {"name": "test_tool"}}),
                ],
            }),
        }
    }

    fn make_llm_response_event(trace: &str, round: usize) -> ConversationEvent {
        ConversationEvent {
            event_type: EventType::LLMResponse,
            trace_id: trace.to_string(),
            timestamp: Utc::now(),
            data: EventData::LLMResponse(LLMResponseEventData {
                round,
                duration_ms: 1500,
                content: "Hello!".to_string(),
                tool_calls_count: 0,
                finish_reason: "stop".to_string(),
                tool_calls: vec![],
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
                cached_tokens: None,
            }),
        }
    }

    fn make_tool_call_event(trace: &str, round: usize, tool: &str, success: bool) -> ConversationEvent {
        ConversationEvent {
            event_type: EventType::ToolCall,
            trace_id: trace.to_string(),
            timestamp: Utc::now(),
            data: EventData::ToolCall(ToolCallEventData {
                tool_name: tool.to_string(),
                success,
                duration_ms: 100,
                error: if success { String::new() } else { "error".to_string() },
                llm_round: round,
                arguments: String::new(),
                result: String::new(),
            }),
        }
    }

    fn make_end_event(trace: &str, rounds: usize) -> ConversationEvent {
        ConversationEvent {
            event_type: EventType::ConversationEnd,
            trace_id: trace.to_string(),
            timestamp: Utc::now(),
            data: EventData::ConversationEnd(ConversationEndData {
                session_key: "test:chat1".to_string(),
                channel: "web".to_string(),
                chat_id: "chat1".to_string(),
                total_rounds: rounds,
                total_duration_ms: 3000,
                content: "Final answer.".to_string(),
                is_error: false,
            }),
        }
    }

    #[test]
    fn full_conversation_lifecycle() {
        let tmp = TempDir::new().unwrap();
        let observer = RequestLoggerObserver::new(test_config(), tmp.path());

        assert_eq!(observer.name(), "request_logger");

        // Start conversation
        observer.on_event(&make_start_event("trace-1", "Hello"));
        assert_eq!(observer.active_count(), 1);

        // LLM round
        observer.on_event(&make_llm_request_event("trace-1", 1));
        observer.on_event(&make_llm_response_event("trace-1", 1));

        // End conversation
        observer.on_event(&make_end_event("trace-1", 1));
        assert_eq!(observer.active_count(), 0);

        // Verify files were created in the session directory
        let log_dir = tmp.path().join("logs").join("llm");
        assert!(log_dir.exists());

        // There should be a session directory
        let session_dirs: Vec<_> = std::fs::read_dir(&log_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map_or(false, |ft| ft.is_dir()))
            .collect();

        // At least one session dir
        assert!(!session_dirs.is_empty());
    }

    #[test]
    fn tool_calls_are_logged_on_end() {
        let tmp = TempDir::new().unwrap();
        let observer = RequestLoggerObserver::new(test_config(), tmp.path());

        observer.on_event(&make_start_event("trace-2", "Do something"));

        // Round 1: tool call
        observer.on_event(&make_tool_call_event("trace-2", 1, "calculator", true));
        observer.on_event(&make_tool_call_event("trace-2", 1, "search", false));

        // End
        observer.on_event(&make_end_event("trace-2", 1));
        assert_eq!(observer.active_count(), 0);
    }

    #[test]
    fn disabled_config_does_not_create_session() {
        let config = LoggingConfig {
            enabled: false,
            detail_level: crate::request_logger::DetailLevel::Full,
            log_dir: "logs/llm".to_string(),
        };
        let tmp = TempDir::new().unwrap();
        let observer = RequestLoggerObserver::new(config, tmp.path());

        observer.on_event(&make_start_event("trace-3", "Hello"));
        assert_eq!(observer.active_count(), 0); // Not tracked when disabled
    }

    #[test]
    fn unknown_trace_ignored() {
        let tmp = TempDir::new().unwrap();
        let observer = RequestLoggerObserver::new(test_config(), tmp.path());

        // These events reference a trace that was never started
        observer.on_event(&make_llm_request_event("unknown-trace", 1));
        observer.on_event(&make_llm_response_event("unknown-trace", 1));
        observer.on_event(&make_tool_call_event("unknown-trace", 1, "tool", true));
        observer.on_event(&make_end_event("unknown-trace", 1));

        assert_eq!(observer.active_count(), 0);
    }

    #[test]
    fn multiple_concurrent_conversations() {
        let tmp = TempDir::new().unwrap();
        let observer = RequestLoggerObserver::new(test_config(), tmp.path());

        observer.on_event(&make_start_event("trace-a", "Hello A"));
        observer.on_event(&make_start_event("trace-b", "Hello B"));

        assert_eq!(observer.active_count(), 2);

        observer.on_event(&make_end_event("trace-a", 1));
        assert_eq!(observer.active_count(), 1);

        observer.on_event(&make_end_event("trace-b", 1));
        assert_eq!(observer.active_count(), 0);
    }

    #[test]
    fn full_lifecycle_with_tool_calls_and_response() {
        let tmp = TempDir::new().unwrap();
        let observer = RequestLoggerObserver::new(test_config(), tmp.path());

        let trace = "trace-full";
        observer.on_event(&make_start_event(trace, "Calculate 2+2"));

        // Round 1: LLM request → response with tool calls
        observer.on_event(&make_llm_request_event(trace, 1));
        observer.on_event(&ConversationEvent {
            event_type: EventType::LLMResponse,
            trace_id: trace.to_string(),
            timestamp: Utc::now(),
            data: EventData::LLMResponse(LLMResponseEventData {
                round: 1,
                duration_ms: 2000,
                content: "".to_string(),
                tool_calls_count: 2,
                finish_reason: "tool_calls".to_string(),
                tool_calls: vec![
                    serde_json::json!({"id": "tc1", "function": {"name": "calculator", "arguments": "{\"expr\": \"2+2\"}"}}),
                ],
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
                cached_tokens: None,
            }),
        });

        // Tool call: success
        observer.on_event(&make_tool_call_event(trace, 1, "calculator", true));
        // Tool call: failure
        observer.on_event(&ConversationEvent {
            event_type: EventType::ToolCall,
            trace_id: trace.to_string(),
            timestamp: Utc::now(),
            data: EventData::ToolCall(ToolCallEventData {
                tool_name: "search".to_string(),
                success: false,
                duration_ms: 500,
                error: "Network timeout".to_string(),
                llm_round: 1,
                arguments: "{\"query\": \"test\"}".to_string(),
                result: "Error: Network timeout".to_string(),
            }),
        });

        // Round 2: LLM request → response
        observer.on_event(&make_llm_request_event(trace, 2));
        observer.on_event(&make_llm_response_event(trace, 2));

        // End conversation with error
        observer.on_event(&ConversationEvent {
            event_type: EventType::ConversationEnd,
            trace_id: trace.to_string(),
            timestamp: Utc::now(),
            data: EventData::ConversationEnd(ConversationEndData {
                session_key: "test:chat1".to_string(),
                channel: "web".to_string(),
                chat_id: "chat1".to_string(),
                total_rounds: 2,
                total_duration_ms: 5000,
                content: "The answer is 4.".to_string(),
                is_error: false,
            }),
        });

        assert_eq!(observer.active_count(), 0);
    }

    #[test]
    fn conversation_end_with_error_flag() {
        let tmp = TempDir::new().unwrap();
        let observer = RequestLoggerObserver::new(test_config(), tmp.path());

        let trace = "trace-err";
        observer.on_event(&make_start_event(trace, "Do something"));

        observer.on_event(&ConversationEvent {
            event_type: EventType::ConversationEnd,
            trace_id: trace.to_string(),
            timestamp: Utc::now(),
            data: EventData::ConversationEnd(ConversationEndData {
                session_key: "test:chat1".to_string(),
                channel: "web".to_string(),
                chat_id: "chat1".to_string(),
                total_rounds: 1,
                total_duration_ms: 1000,
                content: "Error: something went wrong".to_string(),
                is_error: true,
            }),
        });

        assert_eq!(observer.active_count(), 0);
    }

    #[test]
    fn llm_response_with_tool_call_details() {
        let tmp = TempDir::new().unwrap();
        let observer = RequestLoggerObserver::new(test_config(), tmp.path());

        let trace = "trace-tc";
        observer.on_event(&make_start_event(trace, "Search for info"));
        observer.on_event(&make_llm_request_event(trace, 1));

        // Response with tool calls
        observer.on_event(&ConversationEvent {
            event_type: EventType::LLMResponse,
            trace_id: trace.to_string(),
            timestamp: Utc::now(),
            data: EventData::LLMResponse(LLMResponseEventData {
                round: 1,
                duration_ms: 3000,
                content: "Let me search for that.".to_string(),
                tool_calls_count: 1,
                finish_reason: "tool_calls".to_string(),
                tool_calls: vec![
                    serde_json::json!({
                        "id": "call_123",
                        "type": "function",
                        "function": {
                            "name": "web_search",
                            "arguments": "{\"query\": \"test query\"}"
                        }
                    }),
                ],
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
                cached_tokens: None,
            }),
        });

        observer.on_event(&make_end_event(trace, 1));
        assert_eq!(observer.active_count(), 0);
    }

    #[test]
    fn tool_call_with_arguments_and_result() {
        let tmp = TempDir::new().unwrap();
        let observer = RequestLoggerObserver::new(test_config(), tmp.path());

        let trace = "trace-args";
        observer.on_event(&make_start_event(trace, "List files"));
        observer.on_event(&make_llm_request_event(trace, 1));
        observer.on_event(&make_llm_response_event(trace, 1));

        // Tool call with full data
        observer.on_event(&ConversationEvent {
            event_type: EventType::ToolCall,
            trace_id: trace.to_string(),
            timestamp: Utc::now(),
            data: EventData::ToolCall(ToolCallEventData {
                tool_name: "list_dir".to_string(),
                success: true,
                duration_ms: 50,
                error: String::new(),
                llm_round: 1,
                arguments: "{\"path\": \"/tmp\"}".to_string(),
                result: "file1.txt\nfile2.txt".to_string(),
            }),
        });

        observer.on_event(&make_end_event(trace, 1));
        assert_eq!(observer.active_count(), 0);
    }
}

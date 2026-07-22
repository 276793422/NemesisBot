//! Cluster request logger observer.
//!
//! Logs full LLM request/response details for cluster agent (B-side of
//! peer_chat) under `workspace/logs/cluster_logs/{device_id}/{ts_ms}_{task_id}/`.
//!
//! Architecture:
//! - Reuses `RequestLogger` from `nemesis-agent` for all file writing logic
//!   (zero duplication of format/json/index logic).
//! - Reuses `convert_event` from `nemesis-agent::request_logger_observer`
//!   for event translation.
//! - Owns only the path strategy (device_id + task_id naming).
//!
//! Task context (task_id + device_id) is set by `cluster_agent_loop` before
//! each task execution and cleared after. The observer reads the current
//! context when handling `ConversationStart` to construct the session path.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::Local;
use tracing::warn;

use nemesis_agent::request_logger::{
    FinalResponseInfo, LLMRequestInfo, LLMResponseInfo, LocalOperationInfo, LoggingConfig,
    OperationInfo, RequestLogger,
};
use nemesis_agent::request_logger_observer::{
    ConversationEndData, ConversationEvent, ConversationStartData, EventData, EventType,
    LLMRequestEventData, LLMResponseEventData, ToolCallEventData, convert_event,
};

/// Fallback device_id directory name when source node is unknown.
const UNKNOWN_DEVICE_DIR: &str = "_unknown";

/// Current task context for path construction.
#[derive(Debug, Clone)]
struct TaskContext {
    task_id: String,
    device_id: String,
}

/// Per-conversation state tracked by the observer.
struct ConversationState {
    logger: RequestLogger,
    start_time: chrono::DateTime<Local>,
    last_request_time: Option<chrono::DateTime<Local>>,
}

/// Observer that logs cluster agent LLM activity under per-device/per-task
/// directories.
pub struct ClusterRequestLoggerObserver {
    config: LoggingConfig,
    workspace: PathBuf,
    /// Current task context — set by cluster_agent_loop before each task.
    current_task: Mutex<Option<TaskContext>>,
    /// trace_id → conversation state.
    active: Mutex<HashMap<String, ConversationState>>,
}

impl ClusterRequestLoggerObserver {
    /// Create a new observer.
    ///
    /// The observer is inert until `set_task_context` is called: any
    /// `ConversationStart` event received without an active task context
    /// is logged under `_unknown/`.
    pub fn new(config: LoggingConfig, workspace: &Path) -> Self {
        Self {
            config,
            workspace: workspace.to_path_buf(),
            current_task: Mutex::new(None),
            active: Mutex::new(HashMap::new()),
        }
    }

    /// Set the current task context.
    ///
    /// Called by `cluster_agent_loop` immediately before invoking the agent
    /// loop for a task. The observer reads `task_id` and `device_id` when
    /// handling the next `ConversationStart` event to construct the log path.
    pub fn set_task_context(&self, task_id: String, device_id: String) {
        let mut ctx = self.current_task.lock().unwrap();
        *ctx = Some(TaskContext { task_id, device_id });
    }

    /// Clear the current task context.
    ///
    /// Called after task execution completes (success or failure). Prevents
    /// the next task from inheriting stale context.
    pub fn clear_task_context(&self) {
        let mut ctx = self.current_task.lock().unwrap();
        *ctx = None;
    }

    /// Emit a synthetic ConversationStart event.
    ///
    /// Why this exists: `AgentLoop::run_with_trace()` and `resume_execution()`
    /// emit LlmRequest/LlmResponse/ToolCall events but NOT ConversationStart.
    /// The main agent's wrapper (`run_agent_loop`) emits start/end around the
    /// run_with_trace call. Cluster agent calls run_with_trace directly, so it
    /// must emit start/end itself — otherwise the observer's `active` map never
    /// gets the trace_id registered and every subsequent event is dropped.
    pub fn emit_conversation_start(
        &self,
        trace_id: &str,
        channel: &str,
        chat_id: &str,
        sender_id: &str,
        content: &str,
    ) {
        let event = ConversationEvent {
            event_type: EventType::ConversationStart,
            trace_id: trace_id.to_string(),
            timestamp: chrono::Local::now(),
            data: EventData::ConversationStart(ConversationStartData {
                session_key: format!("{}:{}", channel, chat_id),
                channel: channel.to_string(),
                chat_id: chat_id.to_string(),
                sender_id: sender_id.to_string(),
                content: content.to_string(),
            }),
        };
        self.dispatch(&event);
    }

    /// Emit a synthetic ConversationEnd event.
    ///
    /// Companion to `emit_conversation_start`. Must be called on every exit
    /// path (success, async, error) to remove the trace_id from the active
    /// map and write the final response file.
    pub fn emit_conversation_end(
        &self,
        trace_id: &str,
        channel: &str,
        chat_id: &str,
        total_rounds: usize,
        content: &str,
        is_error: bool,
    ) {
        let event = ConversationEvent {
            event_type: EventType::ConversationEnd,
            trace_id: trace_id.to_string(),
            timestamp: chrono::Local::now(),
            data: EventData::ConversationEnd(ConversationEndData {
                session_key: format!("{}:{}", channel, chat_id),
                channel: channel.to_string(),
                chat_id: chat_id.to_string(),
                total_rounds,
                total_duration_ms: 0,
                content: content.to_string(),
                is_error,
            }),
        };
        self.dispatch(&event);
    }

    /// Returns the number of active conversations being tracked.
    #[allow(dead_code)]
    pub fn active_count(&self) -> usize {
        self.active.lock().unwrap().len()
    }

    /// Construct the session directory path from the current task context.
    ///
    /// Returns `(base_dir, session_name)` where:
    /// - `base_dir` = `workspace/logs/cluster_logs/{device_id or "_unknown"}`
    /// - `session_name` = `"{ts_ms}_{sanitized_task_id}"` (or random fallback
    ///   when no task context is set, used by `_unknown` path)
    fn build_session_paths(&self) -> (PathBuf, Option<String>) {
        let ctx = self.current_task.lock().unwrap().clone();

        let device_id = ctx
            .as_ref()
            .map(|c| c.device_id.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| UNKNOWN_DEVICE_DIR.to_string());

        let base_dir = self
            .workspace
            .join("logs")
            .join("cluster_logs")
            .join(RequestLogger::sanitize_filename(&device_id));

        let session_name = ctx.map(|c| {
            let ts = Local::now().format("%Y-%m-%d_%H-%M-%S-%3f").to_string();
            // %3f gives 6-digit nanoseconds; trim to milliseconds (3 digits).
            // Actually %3f is already 3-digit milliseconds in chrono.
            let task_part = RequestLogger::sanitize_filename(&c.task_id);
            format!("{}_{}", ts, task_part)
        });

        (base_dir, session_name)
    }

    /// Handle conversation start: create logger, build session dir, log user request.
    fn handle_conversation_start(
        &self,
        trace_id: &str,
        timestamp: chrono::DateTime<Local>,
        data: &ConversationStartData,
    ) {
        let (base_dir, session_name) = self.build_session_paths();
        let logger = RequestLogger::new_with_paths(self.config.clone(), base_dir, session_name);
        if !logger.is_enabled() {
            return;
        }
        if let Err(e) = logger.create_session() {
            warn!(
                "[ClusterRequestLoggerObserver] Failed to create session for trace {}: {}",
                trace_id, e
            );
            return;
        }
        logger.log_user_request(&nemesis_agent::request_logger::UserRequestInfo {
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
            state.last_request_time = Some(Local::now());

            if self.config.save_raw {
                // Raw mode: log full envelope including raw messages and tools.
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
                // Markdown mode: convert serde_json::Value messages to LlmMessage structs.
                let messages: Vec<nemesis_agent::r#loop::LlmMessage> = data
                    .messages
                    .iter()
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
                let response_time = chrono::Local::now();
                if let Some(ref resp_body) = data.raw_response_body {
                    state.logger.log_raw_response(
                        resp_body,
                        response_time,
                        data.round,
                        data.duration_ms,
                    );
                }
            } else {
                let tool_calls: Vec<nemesis_agent::request_logger::ToolCallDetail> = data
                    .tool_calls
                    .iter()
                    .filter_map(|v| {
                        let id = v
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let name = v
                            .get("name")
                            .or_else(|| v.get("function").and_then(|f| f.get("name")))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let arguments = v
                            .get("arguments")
                            .or_else(|| v.get("function").and_then(|f| f.get("arguments")))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        if name.is_empty() {
                            None
                        } else {
                            Some(nemesis_agent::request_logger::ToolCallDetail {
                                id,
                                name,
                                arguments,
                            })
                        }
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
                    usage: nemesis_agent::request_logger::UsageInfo {
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

    /// Internal synchronous event dispatch.
    fn dispatch(&self, event: &ConversationEvent) {
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
}

#[async_trait::async_trait]
impl nemesis_observer::Observer for ClusterRequestLoggerObserver {
    fn name(&self) -> &str {
        "cluster_request_logger"
    }

    async fn on_event(&self, event: nemesis_observer::ConversationEvent) {
        let internal = match convert_event(&event) {
            Some(e) => e,
            None => return,
        };
        self.dispatch(&internal);
    }
}

#[cfg(test)]
mod tests;

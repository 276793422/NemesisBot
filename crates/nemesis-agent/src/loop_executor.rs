//! Full agent loop executor that integrates with the message bus.
//!
//! `AgentLoopExecutor` receives `InboundMessage`s from a tokio mpsc channel,
//! processes them through the LLM + tool loop, and publishes `OutboundMessage`s
//! back through an outbound mpsc channel. This is the async, bus-integrated
//! counterpart to the synchronous `AgentLoop` in `crate::r#loop`.
//!
//! # Architecture
//!
//! The executor mirrors the Go `AgentLoop` pattern:
//! - `run_agent_loop` is the core LLM + tool iteration loop
//! - `call_llm_with_fallback` provides fallback-chain with cooldown tracking
//! - `call_llm_with_retry` adds context-window error retry with compression
//! - `update_tool_contexts` injects channel/chatID into context-aware tools
//! - `ToolResult` provides ForUser/ForLLM/Async result types
//! - Session persistence and summarization triggers are integrated

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::context::RequestContext;
use crate::instance::AgentInstance;
use crate::r#loop::{LlmMessage, LlmProvider, Tool};
use crate::session::SessionStore;
use crate::types::{AgentConfig, AgentState, ToolCallInfo, ToolCallResult};

// ===========================================================================
// Configuration types
// ===========================================================================

/// Configuration for the loop executor.
#[derive(Debug, Clone)]
pub struct ExecutorConfig {
    /// LLM model identifier.
    pub model: String,
    /// Maximum number of LLM tool-calling iterations per request.
    pub max_turns: u32,
    /// System prompt injected at the start of every conversation.
    pub system_prompt: Option<String>,
    /// Channel buffer size for internal event collection.
    pub event_buffer_size: usize,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            model: "gpt-4".to_string(),
            max_turns: 10,
            system_prompt: None,
            event_buffer_size: 64,
        }
    }
}

// ===========================================================================
// Concurrent mode
// ===========================================================================

/// Session busy state for concurrency control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConcurrentMode {
    /// Reject new messages when session is busy.
    Reject,
    /// Queue messages when session is busy.
    Queue,
}

impl Default for ConcurrentMode {
    fn default() -> Self {
        Self::Reject
    }
}

/// Busy message returned when session is busy.
const BUSY_MESSAGE: &str = "AI is processing a previous request, please try again later";

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
// Fallback chain types
// ===========================================================================

/// A fallback candidate for the LLM call.
#[derive(Debug, Clone)]
pub struct FallbackCandidate {
    pub provider: String,
    pub model: String,
}

/// Result from a fallback chain execution.
#[derive(Debug)]
pub struct FallbackResult {
    pub response: crate::r#loop::LlmResponse,
    pub provider: String,
    pub model: String,
    pub attempts: usize,
}

// ===========================================================================
// ContextualTool trait
// ===========================================================================

/// Trait for tools that need channel/chatID context injection.
///
/// Mirrors Go's `tools.ContextualTool` interface. Tools that implement
/// this trait receive the current channel and chat_id before each request
/// so they can route messages correctly (e.g., message, spawn, cluster_rpc).
pub trait ContextualTool: Tool {
    /// Set the execution context (channel + chat_id) for the tool.
    fn set_context(&self, channel: &str, chat_id: &str);
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

// ===========================================================================
// FallbackExecutor -- fallback chain with cooldown tracking
// ===========================================================================

/// Cooldown duration for a failed fallback candidate (5 seconds).
const FALLBACK_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(5);

/// Tracks cooldown state for fallback candidates.
#[derive(Debug)]
struct CooldownEntry {
    /// When the candidate last failed.
    last_failure: std::time::Instant,
}

/// Executes LLM calls with fallback chain and cooldown tracking.
///
/// Mirrors Go's `fallback.Execute` pattern:
/// 1. Try each candidate in order
/// 2. Skip candidates that are in cooldown
/// 3. If all fail, return the last error
/// 4. Track cooldowns to avoid hammering failed providers
pub struct FallbackExecutor {
    /// Cooldown entries keyed by "provider/model".
    cooldowns: std::sync::Mutex<HashMap<String, CooldownEntry>>,
}

impl FallbackExecutor {
    /// Create a new fallback executor.
    pub fn new() -> Self {
        Self {
            cooldowns: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Execute the fallback chain.
    ///
    /// Calls `try_fn` for each candidate in order. If a candidate fails,
    /// its cooldown is set. Candidates in cooldown are skipped.
    /// Returns the first successful result or the last error.
    pub async fn execute<F, Fut>(
        &self,
        candidates: &[FallbackCandidate],
        try_fn: F,
    ) -> Result<FallbackResult, String>
    where
        F: Fn(String, String) -> Fut,
        Fut: std::future::Future<Output = Result<crate::r#loop::LlmResponse, String>>,
    {
        let mut last_error = String::from("No candidates available");
        let mut attempts = 0;

        for candidate in candidates {
            let key = format!("{}/{}", candidate.provider, candidate.model);

            // Check cooldown.
            {
                let cooldowns = self.cooldowns.lock().unwrap();
                if let Some(entry) = cooldowns.get(&key) {
                    if entry.last_failure.elapsed() < FALLBACK_COOLDOWN {
                        debug!(
                            "[FallbackExecutor] Skipping fallback candidate {} (cooldown remaining: {:?})",
                            key,
                            FALLBACK_COOLDOWN - entry.last_failure.elapsed()
                        );
                        continue;
                    }
                }
            }

            attempts += 1;
            match try_fn(candidate.provider.clone(), candidate.model.clone()).await {
                Ok(response) => {
                    // Clear any cooldown for this candidate on success.
                    {
                        let mut cooldowns = self.cooldowns.lock().unwrap();
                        cooldowns.remove(&key);
                    }
                    return Ok(FallbackResult {
                        response,
                        provider: candidate.provider.clone(),
                        model: candidate.model.clone(),
                        attempts,
                    });
                }
                Err(err) => {
                    warn!(
                        "[FallbackExecutor] Fallback candidate {} failed: {}",
                        key, err
                    );
                    // Set cooldown.
                    {
                        let mut cooldowns = self.cooldowns.lock().unwrap();
                        cooldowns.insert(
                            key,
                            CooldownEntry {
                                last_failure: std::time::Instant::now(),
                            },
                        );
                    }
                    last_error = err;
                }
            }
        }

        Err(last_error)
    }
}

impl Default for FallbackExecutor {
    fn default() -> Self {
        Self::new()
    }
}

// ===========================================================================
// Session persistence
// ===========================================================================

/// Manages session save and summarization triggers.
///
/// Mirrors Go's `agent.Sessions.Save` + `maybeSummarize` pattern.
/// After each agent loop completion, the session can be persisted to disk
/// and summarization can be triggered if thresholds are exceeded.
pub struct SessionPersistence {
    /// Optional session store for disk persistence.
    session_store: Option<Arc<crate::session::SessionStore>>,
    /// Optional summarizer for LLM-driven summarization.
    summarizer: Option<crate::session::Summarizer>,
}

impl SessionPersistence {
    /// Create a new session persistence manager without disk storage.
    pub fn new_in_memory() -> Self {
        Self {
            session_store: None,
            summarizer: None,
        }
    }

    /// Create with disk storage.
    pub fn with_storage(
        session_store: Arc<crate::session::SessionStore>,
        summarizer: crate::session::Summarizer,
    ) -> Self {
        Self {
            session_store: Some(session_store),
            summarizer: Some(summarizer),
        }
    }

    /// Save a session to disk (if configured).
    pub fn save_session(&self, session_key: &str) -> Result<(), String> {
        if let Some(ref store) = self.session_store {
            store.save(session_key)
        } else {
            Ok(())
        }
    }

    /// Maybe trigger summarization for a session.
    ///
    /// Returns true if summarization was triggered.
    pub fn maybe_summarize(
        &self,
        session_key: &str,
        channel: &str,
        chat_id: &str,
        history: &[crate::types::ConversationTurn],
        context_window: usize,
    ) -> bool {
        if let Some(ref summarizer) = self.summarizer {
            summarizer.maybe_summarize(session_key, channel, chat_id, history, context_window)
        } else {
            false
        }
    }
}

// ===========================================================================
// Internal types
// ===========================================================================

/// Wrapper type for DashMap-based session store, kept internal to simplify imports.
type DashMapSessions = dashmap::DashMap<String, AgentInstance>;

// ===========================================================================
// AgentLoopExecutor
// ===========================================================================

/// Full agent loop executor that integrates with the message bus.
///
/// This struct owns the LLM provider, tool registry, and channels for
/// receiving inbound messages and sending outbound messages. The `run`
/// method is the main entry point that continuously processes messages.
pub struct AgentLoopExecutor {
    /// LLM provider for generating responses.
    provider: Arc<dyn LlmProvider>,
    /// Tool registry: name -> tool implementation.
    tools: HashMap<String, Arc<dyn Tool>>,
    /// Receiver for inbound messages.
    inbound_rx: mpsc::Receiver<nemesis_types::channel::InboundMessage>,
    /// Sender for outbound messages.
    outbound_tx: mpsc::Sender<nemesis_types::channel::OutboundMessage>,
    /// Executor configuration.
    config: ExecutorConfig,
    /// Agent configuration derived from executor config.
    agent_config: AgentConfig,
    /// Active agent instances, keyed by session_key.
    instances: Arc<DashMapSessions>,
    /// Fallback candidates for LLM calls.
    fallback_candidates: Vec<FallbackCandidate>,
    /// Fallback executor with cooldown tracking.
    fallback_executor: FallbackExecutor,
    /// Concurrent request mode.
    concurrent_mode: ConcurrentMode,
    /// Queue size for queue mode.
    queue_size: usize,
    /// Optional observer for lifecycle events (legacy single-observer).
    observer: Option<Arc<dyn Observer>>,
    /// Optional multi-observer manager from nemesis-observer.
    observer_manager: Option<Arc<nemesis_observer::Manager>>,
    /// Session busy state tracking.
    busy_sessions: Arc<dashmap::DashSet<String>>,
    /// Session persistence manager.
    session_persistence: SessionPersistence,
    /// Context window size for summarization thresholds.
    context_window: usize,
    /// Optional continuation manager for async cluster RPC.
    continuation_manager: Option<Arc<crate::loop_continuation::ContinuationManager>>,
    /// Optional session store for persisting continuation replies to
    /// sessions/ files alongside session_logs/.
    session_store: Option<Arc<SessionStore>>,
    /// Optional data store for recording LLM usage statistics.
    data_store: Option<Arc<nemesis_data::DataStore>>,
}

impl AgentLoopExecutor {
    /// Create a new loop executor.
    ///
    /// # Arguments
    /// * `provider` - The LLM provider to use for generating responses.
    /// * `inbound_rx` - Channel receiver for inbound messages.
    /// * `outbound_tx` - Channel sender for outbound messages.
    /// * `config` - Executor configuration.
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        inbound_rx: mpsc::Receiver<nemesis_types::channel::InboundMessage>,
        outbound_tx: mpsc::Sender<nemesis_types::channel::OutboundMessage>,
        config: ExecutorConfig,
    ) -> Self {
        let agent_config = AgentConfig {
            model: config.model.clone(),
            system_prompt: config.system_prompt.clone(),
            max_turns: config.max_turns,
            tools: Vec::new(),
        };
        Self {
            provider,
            tools: HashMap::new(),
            inbound_rx,
            outbound_tx,
            config,
            agent_config,
            instances: Arc::new(DashMapSessions::new()),
            fallback_candidates: Vec::new(),
            fallback_executor: FallbackExecutor::new(),
            concurrent_mode: ConcurrentMode::default(),
            queue_size: 8,
            observer: None,
            observer_manager: None,
            busy_sessions: Arc::new(dashmap::DashSet::new()),
            session_persistence: SessionPersistence::new_in_memory(),
            context_window: 128_000,
            continuation_manager: None,
            session_store: None,
            data_store: None,
        }
    }

    /// Register a tool with the executor.
    pub fn register_tool(&mut self, name: impl Into<String>, tool: Arc<dyn Tool>) {
        let name = name.into();
        self.agent_config.tools.push(name.clone());
        self.tools.insert(name, tool);
    }

    /// Set the fallback candidates for LLM calls.
    pub fn set_fallback_candidates(&mut self, candidates: Vec<FallbackCandidate>) {
        self.fallback_candidates = candidates;
    }

    /// Set the concurrent request mode.
    pub fn set_concurrent_mode(&mut self, mode: ConcurrentMode, queue_size: usize) {
        self.concurrent_mode = mode;
        self.queue_size = queue_size;
    }

    /// Set the observer for lifecycle events (legacy single-observer).
    pub fn set_observer(&mut self, observer: Arc<dyn Observer>) {
        self.observer = Some(observer);
    }

    /// Set the observer manager for multi-observer support.
    /// Mirrors Go's `SetObserverManager()`.
    pub fn set_observer_manager(&mut self, mgr: Arc<nemesis_observer::Manager>) {
        self.observer_manager = Some(mgr);
    }

    /// Set the data store for recording LLM usage statistics.
    pub fn set_data_store(&mut self, store: Arc<nemesis_data::DataStore>) {
        self.data_store = Some(store);
    }

    /// Get the observer manager, if set.
    /// Mirrors Go's `GetObserverManager()`.
    pub fn get_observer_manager(&self) -> Option<&Arc<nemesis_observer::Manager>> {
        self.observer_manager.as_ref()
    }

    /// Check if any observers (legacy or manager) are registered.
    #[allow(dead_code)]
    fn has_observers(&self) -> bool {
        self.observer.is_some() || self.observer_manager.is_some()
    }

    /// Set the session persistence manager.
    pub fn set_session_persistence(&mut self, persistence: SessionPersistence) {
        self.session_persistence = persistence;
    }

    /// Set the context window size for summarization thresholds.
    pub fn set_context_window(&mut self, window: usize) {
        self.context_window = window;
    }

    /// Set the continuation manager for async cluster RPC.
    pub fn set_continuation_manager(
        &mut self,
        manager: Arc<crate::loop_continuation::ContinuationManager>,
    ) {
        self.continuation_manager = Some(manager);
    }

    /// Set the session store for persisting continuation replies.
    pub fn set_session_store(&mut self, store: Arc<SessionStore>) {
        self.session_store = Some(store);
    }

    /// Generate a trace ID for a conversation.
    fn generate_trace_id(session_key: &str) -> String {
        format!(
            "{}-{}",
            session_key,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        )
    }

    /// Try to acquire a session for processing. Returns false if busy.
    fn try_acquire_session(&self, session_key: &str) -> bool {
        match self.concurrent_mode {
            ConcurrentMode::Reject => {
                if self.busy_sessions.contains(session_key) {
                    return false;
                }
                self.busy_sessions.insert(session_key.to_string());
                true
            }
            ConcurrentMode::Queue => {
                if self.busy_sessions.contains(session_key) {
                    return false;
                }
                self.busy_sessions.insert(session_key.to_string());
                true
            }
        }
    }

    /// Emit an observer event to legacy observer (synchronous).
    fn emit_event(&self, event: ObserverEvent) {
        if let Some(ref observer) = self.observer {
            observer.on_event(event);
        }
    }

    /// Emit an observer event synchronously via manager (for start/end events).
    ///
    /// Blocks until all observers in the manager have processed the event.
    /// Also forwards to the legacy single observer if set.
    async fn emit_sync_event(&self, event: ObserverEvent) {
        if let Some(ref mgr) = self.observer_manager {
            let conv_event = event.to_conversation_event();
            mgr.emit_sync(conv_event).await;
        }
        if let Some(ref observer) = self.observer {
            observer.on_event(event);
        }
    }

    /// Emit an observer event asynchronously via manager (for request/response/tool events).
    ///
    /// Non-blocking: each observer runs in its own tokio task.
    /// Also forwards to the legacy single observer if set.
    fn emit_async_event(&self, event: ObserverEvent) {
        if let Some(ref mgr) = self.observer_manager {
            let conv_event = event.to_conversation_event();
            let mgr = Arc::clone(mgr);
            tokio::spawn(async move {
                mgr.emit(conv_event).await;
            });
        }
        if let Some(ref observer) = self.observer {
            observer.on_event(event);
        }
    }

    /// Run the main loop, processing inbound messages until the channel closes.
    ///
    /// This method blocks until the inbound channel is closed and drained.
    pub async fn run(&mut self) {
        info!("[AgentLoopExecutor] starting");

        while let Some(msg) = self.inbound_rx.recv().await {
            debug!("[AgentLoopExecutor] Received inbound message from channel={}", msg.channel);
            self.process_message(msg).await;
        }

        info!("[AgentLoopExecutor] stopped (inbound channel closed)");
    }

    /// Process a single inbound message through the agent loop.
    ///
    /// This method:
    /// 1. Gets or creates an agent instance for the session
    /// 2. Checks session busy state
    /// 3. Runs the LLM + tool loop
    /// 4. Publishes the final response as an OutboundMessage
    pub async fn process_message(&self, msg: nemesis_types::channel::InboundMessage) {
        let request_ctx = RequestContext {
            channel: msg.channel.clone(),
            chat_id: msg.chat_id.clone(),
            user: msg.sender_id.clone(),
            session_key: msg.session_key.clone(),
            correlation_id: if msg.correlation_id.is_empty() {
                None
            } else {
                Some(msg.correlation_id.clone())
            },
            async_callback: None,
        };

        // Check for cluster continuation prefix.
        if msg.channel == "system"
            && msg.sender_id.starts_with(nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX)
        {
            let task_id = &msg.sender_id[nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX.len()..];
            debug!(
                "[AgentLoopExecutor] Cluster continuation message received: task_id={}",
                task_id
            );

            // Extract task response and status from the message content.
            let task_response = &msg.content;
            let task_failed = msg.metadata.contains_key("error");
            let task_error = msg.metadata.get("error").map(|s| s.as_str());

            if let Some(ref cont_mgr) = self.continuation_manager {
                crate::loop_continuation::handle_cluster_continuation(
                    cont_mgr.as_ref(),
                    task_id,
                    task_response,
                    task_failed,
                    task_error,
                    self.provider.as_ref(),
                    &self.config.model,
                    &self.tools,
                    &self.outbound_tx,
                    self.observer_manager.clone(),
                    self.session_store.as_ref().map(|v| v.as_ref()),
                )
                .await;
            } else {
                warn!(
                    "[AgentLoopExecutor] Cluster continuation received but no ContinuationManager configured: task_id={}",
                    task_id
                );
            }
            return;
        }

        // Check session busy state.
        if !self.try_acquire_session(&msg.session_key) {
            warn!(
                "[AgentLoopExecutor] Session busy, returning busy message: session_key={}",
                msg.session_key
            );
            let outbound = nemesis_types::channel::OutboundMessage {
                channel: request_ctx.channel.clone(),
                chat_id: request_ctx.chat_id.clone(),
                content: BUSY_MESSAGE.to_string(),
                message_type: String::new(),
            };
            if let Err(e) = self.outbound_tx.send(outbound).await {
                warn!("[AgentLoopExecutor] Failed to send busy message: {}", e);
            }
            return;
        }

        // Ensure session is released when done.
        let session_key_clone = msg.session_key.clone();
        let busy_sessions = self.busy_sessions.clone();
        struct Guard {
            key: String,
            map: Arc<dashmap::DashSet<String>>,
        }
        impl Drop for Guard {
            fn drop(&mut self) {
                self.map.remove(&self.key);
            }
        }
        let _guard = Guard {
            key: session_key_clone,
            map: busy_sessions,
        };

        // Update tool contexts with the current channel/chat_id.
        self.update_tool_contexts(&msg.channel, &msg.chat_id);

        // Get or create an agent instance for this session.
        let instance = self
            .instances
            .entry(msg.session_key.clone())
            .or_insert_with(|| AgentInstance::new(self.agent_config.clone()));
        let instance = instance.downgrade();

        // Generate trace ID.
        let trace_id = Self::generate_trace_id(&msg.session_key);

        // Emit conversation_start event (synchronous).
        self.emit_sync_event(ObserverEvent::ConversationStart {
            trace_id: trace_id.clone(),
            session_key: msg.session_key.clone(),
            channel: msg.channel.clone(),
            chat_id: msg.chat_id.clone(),
            sender_id: msg.sender_id.clone(),
            content: msg.content.clone(),
        }).await;

        let conversation_start = std::time::Instant::now();

        // Record last channel for heartbeat notifications.
        if !msg.channel.is_empty()
            && !msg.chat_id.is_empty()
            && !nemesis_types::constants::is_internal_channel(&msg.channel)
        {
            let channel_key = format!("{}:{}", msg.channel, msg.chat_id);
            debug!("[AgentLoopExecutor] Recording last channel: {}", channel_key);
        }

        // Add user message to instance history.
        instance.add_user_message(&msg.content);

        // Run the LLM + tool iteration loop with context window retry.
        let (raw_content, total_iterations) = self
            .run_llm_iteration(&instance, &request_ctx, &trace_id)
            .await;

        let final_content = self.check_iteration_limit(&raw_content, total_iterations);

        instance.set_state(AgentState::Idle);

        // Save final assistant message to session.
        instance.add_assistant_message(&final_content, Vec::new(), None);

        // Save session to disk.
        if let Err(e) = self
            .session_persistence
            .save_session(&msg.session_key)
        {
            warn!("[AgentLoopExecutor] Failed to save session: {}", e);
        }

        // Maybe trigger summarization.
        let history = instance.get_history();
        self.session_persistence.maybe_summarize(
            &msg.session_key,
            &msg.channel,
            &msg.chat_id,
            &history,
            self.context_window,
        );

        let conversation_duration = conversation_start.elapsed();

        // Emit conversation_end event (synchronous).
        self.emit_sync_event(ObserverEvent::ConversationEnd {
            trace_id: trace_id.clone(),
            session_key: msg.session_key.clone(),
            total_rounds: total_iterations,
            duration_ms: conversation_duration.as_millis() as u64,
            content: final_content.clone(),
            channel: msg.channel.clone(),
            chat_id: msg.chat_id.clone(),
        }).await;

        // Log response.
        let response_preview = nemesis_types::utils::truncate(&final_content, 120);
        info!(
            "[AgentLoopExecutor] Response: {} (session={}, iterations={}, len={})",
            response_preview,
            msg.session_key,
            total_iterations,
            final_content.len()
        );

        // Publish the outbound message.
        let response_content = self.format_response(&final_content, &request_ctx);
        let outbound = nemesis_types::channel::OutboundMessage {
            channel: request_ctx.channel.clone(),
            chat_id: request_ctx.chat_id.clone(),
            content: response_content,
            message_type: String::new(),
        };

        if let Err(e) = self.outbound_tx.send(outbound).await {
            warn!("[AgentLoopExecutor] Failed to send outbound message: {}", e);
        }
    }

    /// Run the LLM iteration loop with tool handling and context window retry.
    ///
    /// Mirrors Go's `runLLMIteration`. Returns (final_content, iteration_count).
    /// The iteration loop:
    /// 1. Builds messages from instance history
    /// 2. Calls LLM (with fallback chain)
    /// 3. If context error, compress and retry
    /// 4. If tool calls, execute them and continue
    /// 5. If no tool calls, return the final content
    async fn run_llm_iteration(
        &self,
        instance: &AgentInstance,
        context: &RequestContext,
        trace_id: &str,
    ) -> (String, u32) {
        let mut iteration = 0u32;
        let mut final_content = String::new();
        let max_iterations = self.config.max_turns;
        let max_retries = 2u32;

        while iteration < max_iterations {
            iteration += 1;

            debug!(
                "[AgentLoopExecutor] LLM iteration {}/{}: session={}",
                iteration, max_iterations, context.session_key
            );

            // Build the message list from instance history.
            let messages = self.build_messages(instance);
            debug!("[AgentLoopExecutor] Sending {} messages to LLM", messages.len());

            // Build tool definitions from registered tools for LLM function calling.
            // Sort by name for a stable order — see loop.rs for rationale (model tool-order
            // sensitivity on deepseek-v4-flash).
            let tool_defs: Vec<crate::types::ToolDefinition> = {
                let mut names: Vec<&String> = self.tools.keys().collect();
                names.sort();
                names.into_iter()
                    .filter_map(|name| self.tools.get(name).map(|tool| (name, tool)))
                    .map(|(name, tool)| {
                        crate::types::ToolDefinition {
                            tool_type: "function".to_string(),
                            function: crate::types::ToolFunctionDef {
                                name: name.clone(),
                                description: tool.description(),
                                parameters: tool.parameters(),
                            },
                        }
                    })
                    .collect()
            };

            // Emit LLM request event (asynchronous).
            let msg_values: Vec<serde_json::Value> = messages.iter()
                .filter_map(|m| serde_json::to_value(m).ok())
                .collect();
            let tool_values: Vec<serde_json::Value> = tool_defs.iter()
                .filter_map(|t| serde_json::to_value(t).ok())
                .collect();
            self.emit_async_event(ObserverEvent::LlmRequest {
                trace_id: trace_id.to_string(),
                round: iteration,
                model: self.config.model.clone(),
                messages_count: messages.len(),
                tools_count: tool_defs.len(),
                messages: msg_values,
                tools: tool_values,
                provider_name: String::new(),
                api_key: String::new(),
                api_base: String::new(),
            });

            // Call LLM with fallback chain and context window retry.
            instance.set_state(AgentState::Thinking);
            let round_start = std::time::Instant::now();

            let mut response = self
                .call_llm_with_retry(instance, messages, max_retries, context, trace_id, iteration, Some(crate::types::ChatOptions::default()), tool_defs)
                .await;

            let round_duration = round_start.elapsed();

            // Emit LLM response event (asynchronous).
            let tc_values: Vec<serde_json::Value> = response.tool_calls.iter()
                .filter_map(|tc| serde_json::to_value(tc).ok())
                .collect();
            let tc_count = response.tool_calls.len();
            self.emit_async_event(ObserverEvent::LlmResponse {
                trace_id: trace_id.to_string(),
                round: iteration,
                duration_ms: round_duration.as_millis() as u64,
                has_tool_calls: !response.tool_calls.is_empty(),
                content: response.content.clone(),
                tool_calls: tc_values,
                tool_calls_count: tc_count,
                finish_reason: if response.finished { Some("stop".to_string()) } else { None },
                usage: response.usage.clone(),
                raw_request_body: response.raw_request_body.take(),
                raw_response_body: response.raw_response_body.take(),
            });

            // Record usage statistics if data store is available.
            if let Some(ref ds) = self.data_store {
                if let Some(ref usage) = response.usage {
                    let log = nemesis_data::RequestLog {
                        id: 0,
                        trace_id: trace_id.to_string(),
                        model: self.config.model.clone(),
                        provider_type: String::new(),
                        input_tokens: usage.prompt_tokens,
                        output_tokens: usage.completion_tokens,
                        cache_creation_tokens: usage.cache_creation_tokens.unwrap_or(0),
                        cache_read_tokens: usage.cache_read_tokens.or(usage.cached_tokens).unwrap_or(0),
                        total_cost_usd: 0.0,
                        latency_ms: round_duration.as_millis() as i64,
                        status_code: if response.content.starts_with("Error:") { 500 } else { 200 },
                        error_message: None,
                        is_streaming: false,
                        created_at: chrono::Local::now().timestamp(),
                    };
                    if let Err(e) = ds.insert_request_log(&log) {
                        tracing::warn!("[AgentLoopExecutor] Failed to record usage: {e}");
                    }
                }
            }

            // Check if no tool calls - we're done.
            if response.tool_calls.is_empty() || response.finished {
                final_content = response.content.clone();
                debug!(
                    "[AgentLoopExecutor] LLM response without tool calls (direct answer, {} chars)",
                    final_content.len()
                );
                break;
            }

            // Log tool calls.
            let tool_names: Vec<&str> = response.tool_calls.iter().map(|tc| tc.name.as_str()).collect();
            info!(
                "[AgentLoopExecutor] LLM requested tool calls: {:?} (iteration={})",
                tool_names, iteration
            );

            // Build assistant message with tool calls.
            let assistant_content = response.content.clone();
            let tool_calls = response.tool_calls.clone();
            instance.add_assistant_message(&assistant_content, tool_calls.clone(), response.reasoning_content.clone());

            // Execute tool calls with complex result handling.
            instance.set_state(AgentState::ExecutingTool);
            for (chain_pos, tc) in tool_calls.iter().enumerate() {
                let tool_start = std::time::Instant::now();
                info!("[AgentLoopExecutor] Tool call: {} (id={})", tc.name, tc.id);

                // Execute the tool with context.
                let tool_result = self
                    .execute_tool_with_result(tc, context)
                    .await;
                let tool_duration = tool_start.elapsed();

                // Emit observer event (asynchronous).
                let success = tool_result.err.is_none();
                let result_str = tool_result.err.clone().unwrap_or_else(|| tool_result.for_llm.clone());
                self.emit_async_event(ObserverEvent::ToolCall {
                    trace_id: trace_id.to_string(),
                    tool_name: tc.name.clone(),
                    success,
                    duration_ms: tool_duration.as_millis() as u64,
                    round: iteration,
                    arguments: tc.arguments.clone(),
                    result: result_str,
                });

                // Save continuation snapshot for async tools.
                if tool_result.is_async && !tool_result.task_id.is_empty() {
                    if let Some(ref cont_mgr) = self.continuation_manager {
                        let current_messages = self.build_messages(instance);
                        cont_mgr
                            .save_continuation(
                                &tool_result.task_id,
                                current_messages,
                                &tc.id,
                                &context.channel,
                                &context.chat_id,
                                &context.session_key,
                            )
                            .await;
                        info!(
                            "[AgentLoopExecutor] Continuation snapshot saved: task_id={}",
                            tool_result.task_id
                        );
                    }
                }

                // Send ForUser content to user immediately if not Silent.
                if !tool_result.silent && !tool_result.for_user.is_empty() {
                    let outbound = nemesis_types::channel::OutboundMessage {
                        channel: context.channel.clone(),
                        chat_id: context.chat_id.clone(),
                        content: tool_result.for_user.clone(),
                        message_type: String::new(),
                    };
                    if let Err(e) = self.outbound_tx.send(outbound).await {
                        warn!("[AgentLoopExecutor] Failed to send tool result to user: {}", e);
                    }
                    debug!(
                        "[AgentLoopExecutor] Sent tool result to user: tool={}, len={}",
                        tc.name,
                        tool_result.for_user.len()
                    );
                }

                // Determine content for LLM based on tool result.
                let content_for_llm = if tool_result.for_llm.is_empty() {
                    tool_result
                        .err
                        .unwrap_or_default()
                } else {
                    tool_result.for_llm
                };

                // Feed the tool result back into the instance history.
                instance.add_tool_result(&tc.id, &content_for_llm);

                // Suppress unused variable warning.
                let _ = chain_pos;
            }
        }

        (final_content, iteration)
    }

    /// Check if iteration limit was exceeded and return appropriate message.
    fn check_iteration_limit(&self, final_content: &str, iterations: u32) -> String {
        if iterations >= self.config.max_turns && final_content.is_empty() {
            format!("Max iterations ({}) reached without final response", self.config.max_turns)
        } else {
            final_content.to_string()
        }
    }

    /// Execute a single tool and return a complex ToolResult.
    ///
    /// Mirrors Go's tool execution with `ExecuteWithContext` and the
    /// async callback pattern.
    async fn execute_tool_with_result(
        &self,
        tc: &ToolCallInfo,
        context: &RequestContext,
    ) -> ToolResult {
        match self.tools.get(&tc.name) {
            Some(tool) => match tool.execute(&tc.arguments, context).await {
                Ok(output) => ToolResult::simple(output),
                Err(err) => ToolResult::error(err),
            },
            None => ToolResult::error(format!("Unknown tool '{}'", tc.name)),
        }
    }

    /// Call the LLM with fallback chain support.
    ///
    /// Mirrors Go's `callLLM` closure in `runLLMIteration`:
    /// - If fallback candidates are configured, uses FallbackExecutor
    /// - Otherwise, calls the primary provider directly
    /// - Returns `Err` when the LLM call fails (enables error-based retry detection)
    async fn call_llm_with_fallback(
        &self,
        messages: &[LlmMessage],
        options: Option<crate::types::ChatOptions>,
        tool_defs: Vec<crate::types::ToolDefinition>,
    ) -> Result<crate::r#loop::LlmResponse, String> {
        if self.fallback_candidates.len() > 1 {
            let provider = self.provider.clone();
            let model = self.config.model.clone();
            let messages_owned = messages.to_vec();
            let opts = options.clone();
            let tools_clone = tool_defs.clone();

            let result = self
                .fallback_executor
                .execute(&self.fallback_candidates, |candidate_provider, candidate_model| {
                    let prov = provider.clone();
                    let msgs = messages_owned.clone();
                    let o = opts.clone();
                    let t = tools_clone.clone();
                    let m = if candidate_provider.is_empty() {
                        model.clone()
                    } else {
                        candidate_model.clone()
                    };
                    async move { prov.chat(&m, msgs, o, t).await }
                })
                .await;

            match result {
                Ok(fb_result) => {
                    if !fb_result.provider.is_empty() && fb_result.attempts > 1 {
                        info!(
                            "[AgentLoopExecutor] Fallback: succeeded with {}/{} after {} attempts",
                            fb_result.provider, fb_result.model, fb_result.attempts
                        );
                    }
                    Ok(fb_result.response)
                }
                Err(err) => Err(err),
            }
        } else {
            self.provider
                .chat(&self.config.model, messages.to_vec(), options, tool_defs)
                .await
        }
    }

    /// Call the LLM with context window retry logic.
    ///
    /// Mirrors Go's retry loop in `runLLMIteration`. If the LLM returns an
    /// error containing context/token/length keywords, compresses the history
    /// and retries up to `max_retries` times.
    async fn call_llm_with_retry(
        &self,
        instance: &AgentInstance,
        messages: Vec<LlmMessage>,
        max_retries: u32,
        context: &RequestContext,
        trace_id: &str,
        iteration: u32,
        options: Option<crate::types::ChatOptions>,
        tool_defs: Vec<crate::types::ToolDefinition>,
    ) -> crate::r#loop::LlmResponse {
        let mut current_messages = messages;
        let mut retry_count = 0u32;

        loop {
            let result = self.call_llm_with_fallback(&current_messages, options.clone(), tool_defs.clone()).await;

            match result {
                Ok(response) => {
                    return response;
                }
                Err(ref err) => {
                    // Check the ERROR (not response content) for context window keywords.
                    // Mirrors Go's behavior: check the error return from the LLM call.
                    let err_lower = err.to_lowercase();
                    let is_context_error = err_lower.contains("token")
                        || err_lower.contains("context")
                        || err_lower.contains("invalid_parameter")
                        || err_lower.contains("length")
                        || err_lower.contains("maximum")
                        || err_lower.contains("context_length_exceeded");

                    // If not a context error, or we've exhausted retries, return error as response.
                    if !is_context_error || retry_count >= max_retries {
                        warn!("[AgentLoopExecutor] LLM call failed (non-recoverable): {}", err);
                        return crate::r#loop::LlmResponse {
                            content: format!("Error: {}", err),
                            tool_calls: Vec::new(),
                            finished: true,
                            reasoning_content: None,
                            usage: None,
                            raw_request_body: None,
                            raw_response_body: None,
                        };
                    }

                    warn!(
                        "[AgentLoopExecutor] Context window error detected, attempting compression (retry {}/{})",
                        retry_count, max_retries
                    );

                    // Notify user about compression on first retry (skip internal channels).
                    if retry_count == 0 && !nemesis_types::constants::is_internal_channel(&context.channel)
                    {
                        let outbound = nemesis_types::channel::OutboundMessage {
                            channel: context.channel.clone(),
                            chat_id: context.chat_id.clone(),
                            content: "Context window exceeded. Compressing history and retrying..."
                                .to_string(),
                            message_type: String::new(),
                        };
                        let _ = self.outbound_tx.send(outbound).await;
                    }

                    // Force history compression (keeps system prompt + last 50% of turns).
                    instance.compress_history();

                    // Rebuild messages after compression.
                    current_messages = self.build_messages(instance);

                    // Emit a new LLM request for the retry (asynchronous).
                    let retry_msg_values: Vec<serde_json::Value> = current_messages.iter()
                        .filter_map(|m| serde_json::to_value(m).ok())
                        .collect();
                    self.emit_async_event(ObserverEvent::LlmRequest {
                        trace_id: trace_id.to_string(),
                        round: iteration,
                        model: self.config.model.clone(),
                        messages_count: current_messages.len(),
                        tools_count: 0,
                        messages: retry_msg_values,
                        tools: vec![],
                        provider_name: String::new(),
                        api_key: String::new(),
                        api_base: String::new(),
                    });

                    retry_count += 1;
                }
            }
        }
    }

    /// Handle a batch of tool calls, returning results for each.
    pub async fn handle_tool_calls(
        &self,
        tool_calls: &[ToolCallInfo],
        context: &RequestContext,
        trace_id: &str,
        round: u32,
    ) -> Vec<ToolCallResult> {
        let mut results = Vec::with_capacity(tool_calls.len());

        for tc in tool_calls {
            let tool_start = std::time::Instant::now();
            info!("[AgentLoopExecutor] Executing tool: {} (id={})", tc.name, tc.id);

            let result = match self.tools.get(&tc.name) {
                Some(tool) => match tool.execute(&tc.arguments, context).await {
                    Ok(output) => {
                        debug!("[AgentLoopExecutor] Tool {} returned: {} bytes", tc.name, output.len());
                        ToolCallResult {
                            tool_name: tc.name.clone(),
                            result: output,
                            is_error: false,
                        }
                    }
                    Err(err) => {
                        warn!("[AgentLoopExecutor] Tool {} error: {}", tc.name, err);
                        ToolCallResult {
                            tool_name: tc.name.clone(),
                            result: format!("Tool error: {}", err),
                            is_error: true,
                        }
                    }
                },
                None => {
                    warn!("[AgentLoopExecutor] Unknown tool: {}", tc.name);
                    ToolCallResult {
                        tool_name: tc.name.clone(),
                        result: format!("Error: Unknown tool '{}'", tc.name),
                        is_error: true,
                    }
                }
            };

            let tool_duration = tool_start.elapsed();

            // Emit tool call event.
            self.emit_event(ObserverEvent::ToolCall {
                trace_id: trace_id.to_string(),
                tool_name: tc.name.clone(),
                success: !result.is_error,
                duration_ms: tool_duration.as_millis() as u64,
                round,
                arguments: tc.arguments.clone(),
                result: result.result.clone(),
            });

            results.push(result);
        }

        results
    }

    /// Build the LLM message list from the instance conversation history.
    ///
    /// Injects an ephemeral "Current Time" system message immediately before
    /// the latest user message. The historical prefix (system prompt + earlier
    /// turns) stays byte-identical across requests, preserving prompt cache
    /// hits; only the trailing user message and the time marker are billed
    /// at the cache-miss rate.
    fn build_messages(&self, instance: &AgentInstance) -> Vec<LlmMessage> {
        let history = instance.get_history();
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M (%A)").to_string();
        let time_msg = LlmMessage {
            role: "system".to_string(),
            content: format!("# Current Time\n{}", now),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        };

        let turn_to_msg = |turn: crate::types::ConversationTurn| LlmMessage {
            role: turn.role,
            content: turn.content,
            tool_calls: if turn.tool_calls.is_empty() {
                None
            } else {
                Some(turn.tool_calls)
            },
            tool_call_id: turn.tool_call_id,
            reasoning_content: turn.reasoning_content,
        };

        // Find the last user message index and inject time_msg just before it.
        // Only inject when there is a system prompt at history[0] to protect
        // (otherwise there's no cached prefix to preserve).
        let last_user_idx = history
            .iter()
            .rposition(|t| t.role == "user")
            .filter(|&i| i > 0)
            .filter(|_| history.first().map_or(false, |t| t.role == "system"));

        match last_user_idx {
            Some(idx) => {
                let mut messages: Vec<LlmMessage> =
                    Vec::with_capacity(history.len() + 1);
                messages.extend(history[..idx].into_iter().cloned().map(turn_to_msg));
                messages.push(time_msg);
                messages.extend(history[idx..].into_iter().cloned().map(turn_to_msg));
                messages
            }
            None => history.into_iter().map(turn_to_msg).collect(),
        }
    }

    /// Format response with RPC correlation ID prefix if needed.
    fn format_response(&self, content: &str, context: &RequestContext) -> String {
        context.format_rpc_message(content)
    }

    /// Update tool contexts for channel/chatID awareness.
    ///
    /// Mirrors Go's `updateToolContexts`. Iterates through known context-aware
    /// tools (message, spawn, subagent, cluster_rpc) and injects the current
    /// channel and chat_id. This must be called before each agent loop iteration
    /// so tools know where to route their output.
    pub fn update_tool_contexts(&self, channel: &str, chat_id: &str) {
        // List of tool names that support context injection.
        let context_tool_names = ["message", "spawn", "subagent", "cluster_rpc"];

        for tool_name in &context_tool_names {
            if let Some(tool) = self.tools.get(*tool_name) {
                // Call set_context on the tool (default no-op, overridden by context-aware tools).
                tool.set_context(channel, chat_id);
                debug!(
                    "[AgentLoopExecutor] Updated tool context: tool={}, channel={}, chat_id={}",
                    tool_name, channel, chat_id
                );
            }
        }
    }

    /// Run the full agent loop pipeline for a single message.
    ///
    /// This is the high-level entry point that:
    /// 1. Gets or creates an agent instance for the session
    /// 2. Runs the LLM + tool loop with the user message
    /// 3. Returns the final response content
    ///
    /// Unlike `process_message`, this method does not publish outbound messages.
    pub async fn run_agent_loop(
        &self,
        session_key: &str,
        user_message: &str,
        context: &RequestContext,
    ) -> Result<String, String> {
        // Get or create an agent instance for this session.
        let instance = self
            .instances
            .entry(session_key.to_string())
            .or_insert_with(|| AgentInstance::new(self.agent_config.clone()));
        let instance = instance.downgrade();

        let trace_id = Self::generate_trace_id(session_key);

        // Emit conversation start.
        self.emit_event(ObserverEvent::ConversationStart {
            trace_id: trace_id.clone(),
            session_key: session_key.to_string(),
            channel: context.channel.clone(),
            chat_id: context.chat_id.clone(),
            sender_id: String::new(),
            content: user_message.to_string(),
        });

        let conv_start = std::time::Instant::now();

        // Update tool contexts.
        self.update_tool_contexts(&context.channel, &context.chat_id);

        // Add user message to instance history.
        instance.add_user_message(user_message);
        instance.set_state(AgentState::Thinking);

        let (final_content, turns_used) = self
            .run_llm_iteration(&instance, context, &trace_id)
            .await;

        instance.set_state(AgentState::Idle);

        // Save session.
        if let Err(e) = self.session_persistence.save_session(session_key) {
            warn!("[AgentLoopExecutor] Failed to save session: {}", e);
        }

        // Emit conversation end.
        self.emit_event(ObserverEvent::ConversationEnd {
            trace_id: trace_id.clone(),
            session_key: session_key.to_string(),
            total_rounds: turns_used,
            duration_ms: conv_start.elapsed().as_millis() as u64,
            content: final_content.clone(),
            channel: context.channel.clone(),
            chat_id: context.chat_id.clone(),
        });

        Ok(final_content)
    }

    /// Process a message and publish the response as an outbound message.
    ///
    /// This is a convenience wrapper around `run_agent_loop` that handles
    /// session management and outbound publishing.
    pub async fn process_and_publish(
        &self,
        session_key: &str,
        user_message: &str,
        context: &RequestContext,
    ) -> Result<String, String> {
        let result = self.run_agent_loop(session_key, user_message, context).await?;

        // Publish the outbound message.
        let outbound = nemesis_types::channel::OutboundMessage {
            channel: context.channel.clone(),
            chat_id: context.chat_id.clone(),
            content: result.clone(),
            message_type: String::new(),
        };

        if let Err(e) = self.outbound_tx.send(outbound).await {
            warn!("[AgentLoopExecutor] Failed to send outbound message: {}", e);
        }

        Ok(result)
    }

    /// Returns a reference to the tool registry.
    pub fn tools(&self) -> &HashMap<String, Arc<dyn Tool>> {
        &self.tools
    }

    /// Returns a reference to the executor config.
    pub fn config(&self) -> &ExecutorConfig {
        &self.config
    }

    /// Returns a reference to the fallback candidates.
    pub fn fallback_candidates(&self) -> &[FallbackCandidate] {
        &self.fallback_candidates
    }

    /// Returns a reference to the continuation manager.
    pub fn continuation_manager(
        &self,
    ) -> Option<&Arc<crate::loop_continuation::ContinuationManager>> {
        self.continuation_manager.as_ref()
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests;

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
    },
    /// Conversation ended.
    ConversationEnd {
        trace_id: String,
        session_key: String,
        total_rounds: u32,
        duration_ms: u64,
    },
    /// LLM request sent.
    LlmRequest {
        trace_id: String,
        round: u32,
        model: String,
    },
    /// LLM response received.
    LlmResponse {
        trace_id: String,
        round: u32,
        duration_ms: u64,
        has_tool_calls: bool,
    },
    /// Tool call executed.
    ToolCall {
        trace_id: String,
        tool_name: String,
        success: bool,
        duration_ms: u64,
        round: u32,
    },
}

impl ObserverEvent {
    /// Convert to a `nemesis_observer::ConversationEvent`.
    fn to_conversation_event(&self) -> nemesis_observer::ConversationEvent {
        use nemesis_observer::*;
        match self {
            ObserverEvent::ConversationStart {
                trace_id,
                session_key,
                channel,
                chat_id,
            } => ConversationEvent {
                event_type: EventType::ConversationStart,
                trace_id: trace_id.clone(),
                timestamp: chrono::Utc::now(),
                data: EventData::ConversationStart(ConversationStartData {
                    session_key: session_key.clone(),
                    channel: channel.clone(),
                    chat_id: chat_id.clone(),
                    sender_id: String::new(),
                    content: String::new(),
                }),
            },
            ObserverEvent::ConversationEnd {
                trace_id,
                session_key,
                total_rounds,
                duration_ms,
            } => ConversationEvent {
                event_type: EventType::ConversationEnd,
                trace_id: trace_id.clone(),
                timestamp: chrono::Utc::now(),
                data: EventData::ConversationEnd(ConversationEndData {
                    session_key: session_key.clone(),
                    channel: String::new(),
                    chat_id: String::new(),
                    total_rounds: *total_rounds,
                    total_duration: std::time::Duration::from_millis(*duration_ms),
                    content: String::new(),
                    error: None,
                }),
            },
            ObserverEvent::LlmRequest {
                trace_id,
                round,
                model,
            } => ConversationEvent {
                event_type: EventType::LlmRequest,
                trace_id: trace_id.clone(),
                timestamp: chrono::Utc::now(),
                data: EventData::LlmRequest(LlmRequestData {
                    round: *round,
                    model: model.clone(),
                    provider_name: String::new(),
                    api_key: String::new(),
                    api_base: String::new(),
                    http_headers: std::collections::HashMap::new(),
                    full_config: None,
                    messages: vec![],
                    tools: vec![],
                    messages_count: 0,
                    tools_count: 0,
                }),
            },
            ObserverEvent::LlmResponse {
                trace_id,
                round,
                duration_ms,
                has_tool_calls,
            } => ConversationEvent {
                event_type: EventType::LlmResponse,
                trace_id: trace_id.clone(),
                timestamp: chrono::Utc::now(),
                data: EventData::LlmResponse(LlmResponseData {
                    round: *round,
                    duration: std::time::Duration::from_millis(*duration_ms),
                    content: String::new(),
                    tool_calls: vec![],
                    tool_calls_count: if *has_tool_calls { 1 } else { 0 },
                    usage: None,
                    finish_reason: None,
                }),
            },
            ObserverEvent::ToolCall {
                trace_id,
                tool_name,
                success,
                duration_ms,
                round,
            } => ConversationEvent {
                event_type: EventType::ToolCall,
                trace_id: trace_id.clone(),
                timestamp: chrono::Utc::now(),
                data: EventData::ToolCall(ToolCallData {
                    tool_name: tool_name.clone(),
                    arguments: std::collections::HashMap::new(),
                    success: *success,
                    duration: std::time::Duration::from_millis(*duration_ms),
                    error: None,
                    llm_round: *round,
                    chain_pos: 0,
                }),
            },
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
                            "Skipping fallback candidate {} (cooldown remaining: {:?})",
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
                        "Fallback candidate {} failed: {}",
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
        info!("AgentLoopExecutor starting");

        while let Some(msg) = self.inbound_rx.recv().await {
            debug!("Received inbound message from channel={}", msg.channel);
            self.process_message(msg).await;
        }

        info!("AgentLoopExecutor stopped (inbound channel closed)");
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
                "Cluster continuation message received: task_id={}",
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
                )
                .await;
            } else {
                warn!(
                    "Cluster continuation received but no ContinuationManager configured: task_id={}",
                    task_id
                );
            }
            return;
        }

        // Check session busy state.
        if !self.try_acquire_session(&msg.session_key) {
            warn!(
                "Session busy, returning busy message: session_key={}",
                msg.session_key
            );
            let outbound = nemesis_types::channel::OutboundMessage {
                channel: request_ctx.channel.clone(),
                chat_id: request_ctx.chat_id.clone(),
                content: BUSY_MESSAGE.to_string(),
                message_type: String::new(),
            };
            if let Err(e) = self.outbound_tx.send(outbound).await {
                warn!("Failed to send busy message: {}", e);
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
        }).await;

        let conversation_start = std::time::Instant::now();

        // Record last channel for heartbeat notifications.
        if !msg.channel.is_empty()
            && !msg.chat_id.is_empty()
            && !nemesis_types::constants::is_internal_channel(&msg.channel)
        {
            let channel_key = format!("{}:{}", msg.channel, msg.chat_id);
            debug!("Recording last channel: {}", channel_key);
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
        instance.add_assistant_message(&final_content, Vec::new());

        // Save session to disk.
        if let Err(e) = self
            .session_persistence
            .save_session(&msg.session_key)
        {
            warn!("Failed to save session: {}", e);
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
        }).await;

        // Log response.
        let response_preview = if final_content.len() > 120 {
            format!("{}...", &final_content[..120])
        } else {
            final_content.clone()
        };
        info!(
            "Response: {} (session={}, iterations={}, len={})",
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
            warn!("Failed to send outbound message: {}", e);
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
                "LLM iteration {}/{}: session={}",
                iteration, max_iterations, context.session_key
            );

            // Build the message list from instance history.
            let messages = self.build_messages(instance);
            debug!("Sending {} messages to LLM", messages.len());

            // Emit LLM request event (asynchronous).
            self.emit_async_event(ObserverEvent::LlmRequest {
                trace_id: trace_id.to_string(),
                round: iteration,
                model: self.config.model.clone(),
            });

            // Call LLM with fallback chain and context window retry.
            instance.set_state(AgentState::Thinking);
            let round_start = std::time::Instant::now();

            // Build tool definitions from registered tools for LLM function calling.
            let tool_defs: Vec<crate::types::ToolDefinition> = self.tools.iter()
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
                .collect();

            let response = self
                .call_llm_with_retry(instance, messages, max_retries, context, trace_id, iteration, Some(crate::types::ChatOptions::default()), tool_defs)
                .await;

            let round_duration = round_start.elapsed();

            // Emit LLM response event (asynchronous).
            self.emit_async_event(ObserverEvent::LlmResponse {
                trace_id: trace_id.to_string(),
                round: iteration,
                duration_ms: round_duration.as_millis() as u64,
                has_tool_calls: !response.tool_calls.is_empty(),
            });

            // Check if no tool calls - we're done.
            if response.tool_calls.is_empty() || response.finished {
                final_content = response.content.clone();
                debug!(
                    "LLM response without tool calls (direct answer, {} chars)",
                    final_content.len()
                );
                break;
            }

            // Log tool calls.
            let tool_names: Vec<&str> = response.tool_calls.iter().map(|tc| tc.name.as_str()).collect();
            info!(
                "LLM requested tool calls: {:?} (iteration={})",
                tool_names, iteration
            );

            // Build assistant message with tool calls.
            let assistant_content = response.content.clone();
            let tool_calls = response.tool_calls.clone();
            instance.add_assistant_message(&assistant_content, tool_calls.clone());

            // Execute tool calls with complex result handling.
            instance.set_state(AgentState::ExecutingTool);
            for (chain_pos, tc) in tool_calls.iter().enumerate() {
                let tool_start = std::time::Instant::now();
                info!("Tool call: {} (id={})", tc.name, tc.id);

                // Execute the tool with context.
                let tool_result = self
                    .execute_tool_with_result(tc, context)
                    .await;
                let tool_duration = tool_start.elapsed();

                // Emit observer event (asynchronous).
                let success = tool_result.err.is_none();
                self.emit_async_event(ObserverEvent::ToolCall {
                    trace_id: trace_id.to_string(),
                    tool_name: tc.name.clone(),
                    success,
                    duration_ms: tool_duration.as_millis() as u64,
                    round: iteration,
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
                            )
                            .await;
                        info!(
                            "Continuation snapshot saved: task_id={}",
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
                        warn!("Failed to send tool result to user: {}", e);
                    }
                    debug!(
                        "Sent tool result to user: tool={}, len={}",
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
                            "Fallback: succeeded with {}/{} after {} attempts",
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
                        warn!("LLM call failed (non-recoverable): {}", err);
                        return crate::r#loop::LlmResponse {
                            content: format!("Error: {}", err),
                            tool_calls: Vec::new(),
                            finished: true,
                        };
                    }

                    warn!(
                        "Context window error detected, attempting compression (retry {}/{})",
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
                    self.emit_async_event(ObserverEvent::LlmRequest {
                        trace_id: trace_id.to_string(),
                        round: iteration,
                        model: self.config.model.clone(),
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
            info!("Executing tool: {} (id={})", tc.name, tc.id);

            let result = match self.tools.get(&tc.name) {
                Some(tool) => match tool.execute(&tc.arguments, context).await {
                    Ok(output) => {
                        debug!("Tool {} returned: {} bytes", tc.name, output.len());
                        ToolCallResult {
                            tool_name: tc.name.clone(),
                            result: output,
                            is_error: false,
                        }
                    }
                    Err(err) => {
                        warn!("Tool {} error: {}", tc.name, err);
                        ToolCallResult {
                            tool_name: tc.name.clone(),
                            result: format!("Tool error: {}", err),
                            is_error: true,
                        }
                    }
                },
                None => {
                    warn!("Unknown tool: {}", tc.name);
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
            });

            results.push(result);
        }

        results
    }

    /// Build the LLM message list from the instance conversation history.
    fn build_messages(&self, instance: &AgentInstance) -> Vec<LlmMessage> {
        instance
            .get_history()
            .into_iter()
            .map(|turn| LlmMessage {
                role: turn.role,
                content: turn.content,
                tool_calls: if turn.tool_calls.is_empty() {
                    None
                } else {
                    Some(turn.tool_calls)
                },
                tool_call_id: turn.tool_call_id,
            })
            .collect()
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
                    "Updated tool context: tool={}, channel={}, chat_id={}",
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
            warn!("Failed to save session: {}", e);
        }

        // Emit conversation end.
        self.emit_event(ObserverEvent::ConversationEnd {
            trace_id: trace_id.clone(),
            session_key: session_key.to_string(),
            total_rounds: turns_used,
            duration_ms: conv_start.elapsed().as_millis() as u64,
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
            warn!("Failed to send outbound message: {}", e);
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
mod tests {
    use super::*;
    use crate::r#loop::LlmResponse;
    use async_trait::async_trait;

    /// Mock LLM provider that returns pre-configured responses in sequence.
    struct MockProvider {
        responses: std::sync::Mutex<Vec<LlmResponse>>,
    }

    impl MockProvider {
        fn new(responses: Vec<LlmResponse>) -> Self {
            Self {
                responses: std::sync::Mutex::new(responses),
            }
        }
    }

    #[async_trait]
    impl LlmProvider for MockProvider {
        async fn chat(&self, _model: &str, _messages: Vec<LlmMessage>, _options: Option<crate::types::ChatOptions>, _tools: Vec<crate::types::ToolDefinition>) -> Result<LlmResponse, String> {
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                Ok(LlmResponse {
                    content: "No more responses".to_string(),
                    tool_calls: Vec::new(),
                    finished: true,
                })
            } else {
                Ok(responses.remove(0))
            }
        }
    }

    /// Mock tool for testing.
    struct MockTool {
        result: String,
    }

    #[async_trait]
    impl Tool for MockTool {
        async fn execute(
            &self,
            _args: &str,
            _context: &RequestContext,
        ) -> Result<String, String> {
            Ok(self.result.clone())
        }
    }

    /// Mock observer for testing.
    struct MockObserver {
        events: std::sync::Mutex<Vec<String>>,
    }

    impl MockObserver {
        fn new() -> Self {
            Self {
                events: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    impl Observer for MockObserver {
        fn on_event(&self, event: ObserverEvent) {
            let label = match &event {
                ObserverEvent::ConversationStart { .. } => "conversation_start",
                ObserverEvent::ConversationEnd { .. } => "conversation_end",
                ObserverEvent::LlmRequest { .. } => "llm_request",
                ObserverEvent::LlmResponse { .. } => "llm_response",
                ObserverEvent::ToolCall { .. } => "tool_call",
            };
            self.events.lock().unwrap().push(label.to_string());
        }
    }

    fn make_inbound(
        content: &str,
        channel: &str,
        correlation_id: &str,
    ) -> nemesis_types::channel::InboundMessage {
        nemesis_types::channel::InboundMessage {
            channel: channel.to_string(),
            sender_id: "user1".to_string(),
            chat_id: "chat1".to_string(),
            content: content.to_string(),
            media: vec![],
            session_key: "test:chat1".to_string(),
            correlation_id: correlation_id.to_string(),
            metadata: std::collections::HashMap::new(),
        }
    }

    fn test_executor_config() -> ExecutorConfig {
        ExecutorConfig {
            model: "test-model".to_string(),
            max_turns: 5,
            system_prompt: Some("You are a test assistant.".to_string()),
            event_buffer_size: 16,
        }
    }

    #[tokio::test]
    async fn test_simple_text_response() {
        let provider = Arc::new(MockProvider::new(vec![LlmResponse {
            content: "Hello!".to_string(),
            tool_calls: Vec::new(),
            finished: true,
        }]));
        let (inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, mut outbound_rx) = mpsc::channel(16);

        let mut executor =
            AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());

        // Send a message.
        inbound_tx
            .send(make_inbound("Hi", "web", ""))
            .await
            .unwrap();
        drop(inbound_tx); // Close to terminate executor.

        executor.run().await;

        // Should have received one outbound message.
        let msg = outbound_rx.recv().await.unwrap();
        assert_eq!(msg.channel, "web");
        assert_eq!(msg.chat_id, "chat1");
        assert_eq!(msg.content, "Hello!");
    }

    #[tokio::test]
    async fn test_tool_call_and_final_response() {
        let provider = Arc::new(MockProvider::new(vec![
            // First call: tool call.
            LlmResponse {
                content: String::new(),
                tool_calls: vec![ToolCallInfo {
                    id: "tc_1".to_string(),
                    name: "search".to_string(),
                    arguments: r#"{"query":"test"}"#.to_string(),
                }],
                finished: false,
            },
            // Second call: final text.
            LlmResponse {
                content: "Found results.".to_string(),
                tool_calls: Vec::new(),
                finished: true,
            },
        ]));

        let (inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, mut outbound_rx) = mpsc::channel(16);

        let mut executor =
            AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());
        executor.register_tool(
            "search",
            Arc::new(MockTool {
                result: "search results".to_string(),
            }),
        );

        inbound_tx
            .send(make_inbound("Search for test", "web", ""))
            .await
            .unwrap();
        drop(inbound_tx);

        executor.run().await;

        let msg = outbound_rx.recv().await.unwrap();
        assert_eq!(msg.content, "Found results.");
    }

    #[tokio::test]
    async fn test_rpc_correlation_id_formatting() {
        let provider = Arc::new(MockProvider::new(vec![LlmResponse {
            content: "Pong".to_string(),
            tool_calls: Vec::new(),
            finished: true,
        }]));

        let (_inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, mut outbound_rx) = mpsc::channel(16);

        let executor =
            AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());

        // Process an RPC message directly.
        executor
            .process_message(make_inbound("Ping", "rpc", "corr-42"))
            .await;

        let msg = outbound_rx.recv().await.unwrap();
        assert_eq!(msg.content, "[rpc:corr-42] Pong");
        assert_eq!(msg.channel, "rpc");
    }

    #[tokio::test]
    async fn test_unknown_tool_returns_error_in_result() {
        let provider = Arc::new(MockProvider::new(vec![
            LlmResponse {
                content: String::new(),
                tool_calls: vec![ToolCallInfo {
                    id: "tc_1".to_string(),
                    name: "nonexistent".to_string(),
                    arguments: "{}".to_string(),
                }],
                finished: false,
            },
            LlmResponse {
                content: "Tool not found.".to_string(),
                tool_calls: Vec::new(),
                finished: true,
            },
        ]));

        let (_inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, mut outbound_rx) = mpsc::channel(16);

        let executor =
            AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());

        executor
            .process_message(make_inbound("Do something", "web", ""))
            .await;

        let msg = outbound_rx.recv().await.unwrap();
        assert_eq!(msg.content, "Tool not found.");
    }

    #[tokio::test]
    async fn test_max_turns_limit() {
        // Responses that always request a tool call.
        let responses: Vec<LlmResponse> = (0..20)
            .map(|_| LlmResponse {
                content: String::new(),
                tool_calls: vec![ToolCallInfo {
                    id: "tc_loop".to_string(),
                    name: "calculator".to_string(),
                    arguments: "{}".to_string(),
                }],
                finished: false,
            })
            .collect();

        let provider = Arc::new(MockProvider::new(responses));
        let (inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, mut outbound_rx) = mpsc::channel(16);

        let mut config = test_executor_config();
        config.max_turns = 3;

        let mut executor =
            AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, config);
        executor.register_tool(
            "calculator",
            Arc::new(MockTool {
                result: "0".to_string(),
            }),
        );

        inbound_tx
            .send(make_inbound("Loop test", "web", ""))
            .await
            .unwrap();
        drop(inbound_tx);

        executor.run().await;

        let msg = outbound_rx.recv().await.unwrap();
        assert!(
            msg.content.contains("Max iterations") || msg.content.is_empty(),
            "Expected max iterations error, got: {}",
            msg.content
        );
    }

    #[tokio::test]
    async fn test_session_busy_reject_mode() {
        let provider = Arc::new(MockProvider::new(vec![
            // First response: has a tool call (will take time).
            LlmResponse {
                content: String::new(),
                tool_calls: vec![ToolCallInfo {
                    id: "tc_1".to_string(),
                    name: "slow_tool".to_string(),
                    arguments: "{}".to_string(),
                }],
                finished: false,
            },
            // Second response: final.
            LlmResponse {
                content: "Done.".to_string(),
                tool_calls: Vec::new(),
                finished: true,
            },
        ]));

        let (_inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, mut outbound_rx) = mpsc::channel(16);

        let executor =
            AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());

        // Manually mark the session as busy.
        executor.busy_sessions.insert("test:chat1".to_string());

        // Processing should return busy message.
        executor
            .process_message(make_inbound("Hello", "web", ""))
            .await;

        let msg = outbound_rx.recv().await.unwrap();
        assert!(
            msg.content.contains("processing a previous request"),
            "Expected busy message, got: {}",
            msg.content
        );

        // Clean up.
        executor.busy_sessions.remove("test:chat1");
    }

    #[tokio::test]
    async fn test_observer_events_emitted() {
        let observer = Arc::new(MockObserver::new());

        let provider = Arc::new(MockProvider::new(vec![LlmResponse {
            content: "Hello!".to_string(),
            tool_calls: Vec::new(),
            finished: true,
        }]));

        let (inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, mut outbound_rx) = mpsc::channel(16);

        let mut executor =
            AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());
        executor.set_observer(observer.clone());

        inbound_tx
            .send(make_inbound("Hi", "web", ""))
            .await
            .unwrap();
        drop(inbound_tx);

        executor.run().await;

        let _msg = outbound_rx.recv().await.unwrap();

        // Check observer events.
        let events = observer.events.lock().unwrap();
        assert!(events.contains(&"conversation_start".to_string()));
        assert!(events.contains(&"conversation_end".to_string()));
        assert!(events.contains(&"llm_request".to_string()));
        assert!(events.contains(&"llm_response".to_string()));
    }

    #[tokio::test]
    async fn test_process_and_publish() {
        let provider = Arc::new(MockProvider::new(vec![LlmResponse {
            content: "Published response".to_string(),
            tool_calls: Vec::new(),
            finished: true,
        }]));

        let (_inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, mut outbound_rx) = mpsc::channel(16);

        let executor =
            AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());

        let context = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = executor
            .process_and_publish("sess1", "Hello", &context)
            .await
            .unwrap();

        assert_eq!(result, "Published response");

        let msg = outbound_rx.recv().await.unwrap();
        assert_eq!(msg.content, "Published response");
        assert_eq!(msg.channel, "web");
    }

    #[test]
    fn test_tool_result_simple() {
        let result = ToolResult::simple("hello".to_string());
        assert_eq!(result.for_llm, "hello");
        assert!(result.for_user.is_empty());
        assert!(result.silent);
        assert!(!result.is_async);
        assert!(result.err.is_none());
    }

    #[test]
    fn test_tool_result_for_llm_only() {
        let result = ToolResult::for_llm_only("internal data".to_string());
        assert_eq!(result.for_llm, "internal data");
        assert!(result.for_user.is_empty());
        assert!(result.silent);
    }

    #[test]
    fn test_tool_result_async() {
        let result = ToolResult::async_result(
            "task-123".to_string(),
            "Processing your request...".to_string(),
        );
        assert!(result.is_async);
        assert_eq!(result.task_id, "task-123");
        assert_eq!(result.for_user, "Processing your request...");
        assert!(!result.silent);
    }

    #[test]
    fn test_tool_result_error() {
        let result = ToolResult::error("Something went wrong".to_string());
        assert!(result.err.is_some());
        assert!(result.for_llm.contains("Something went wrong"));
    }

    #[test]
    fn test_fallback_executor_single_candidate_success() {
        let executor = FallbackExecutor::new();
        let candidates = vec![FallbackCandidate {
            provider: "test".to_string(),
            model: "model-1".to_string(),
        }];

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(executor.execute(&candidates, |_p, _m| async {
            Ok(LlmResponse {
                content: "success".to_string(),
                tool_calls: Vec::new(),
                finished: true,
            })
        }));

        assert!(result.is_ok());
        let fb = result.unwrap();
        assert_eq!(fb.model, "model-1");
        assert_eq!(fb.response.content, "success");
        assert_eq!(fb.attempts, 1);
    }

    #[test]
    fn test_fallback_executor_all_fail() {
        let executor = FallbackExecutor::new();
        let candidates = vec![
            FallbackCandidate {
                provider: "test".to_string(),
                model: "model-1".to_string(),
            },
            FallbackCandidate {
                provider: "test".to_string(),
                model: "model-2".to_string(),
            },
        ];

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(executor.execute(&candidates, |_p, _m| async {
            Err("provider error".to_string())
        }));

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "provider error");
    }

    #[test]
    fn test_session_persistence_in_memory() {
        let persistence = SessionPersistence::new_in_memory();
        // Save should succeed silently without a store.
        assert!(persistence.save_session("test").is_ok());
        // maybe_summarize should return false without a summarizer.
        assert!(!persistence.maybe_summarize("test", "web", "chat1", &[], 128000));
    }

    #[test]
    fn test_is_internal_channel() {
        assert!(nemesis_types::constants::is_internal_channel("cli"));
        assert!(nemesis_types::constants::is_internal_channel("system"));
        assert!(nemesis_types::constants::is_internal_channel("subagent"));
        assert!(!nemesis_types::constants::is_internal_channel("web"));
        assert!(!nemesis_types::constants::is_internal_channel("rpc"));
        assert!(!nemesis_types::constants::is_internal_channel("discord"));
    }

    // --- Additional executor tests ---

    #[test]
    fn test_executor_config_default() {
        let config = ExecutorConfig::default();
        assert_eq!(config.model, "gpt-4");
        assert_eq!(config.max_turns, 10);
        assert!(config.system_prompt.is_none());
        assert_eq!(config.event_buffer_size, 64);
    }

    #[test]
    fn test_concurrent_mode_default() {
        assert_eq!(ConcurrentMode::default(), ConcurrentMode::Reject);
    }

    #[test]
    fn test_tool_result_default() {
        let result = ToolResult::default();
        assert!(result.for_llm.is_empty());
        assert!(result.for_user.is_empty());
        assert!(result.silent);
        assert!(!result.is_async);
        assert!(result.task_id.is_empty());
        assert!(result.err.is_none());
    }

    #[test]
    fn test_fallback_candidate_debug() {
        let candidate = FallbackCandidate {
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
        };
        let debug_str = format!("{:?}", candidate);
        assert!(debug_str.contains("openai"));
        assert!(debug_str.contains("gpt-4"));
    }

    #[test]
    fn test_fallback_executor_no_candidates() {
        let executor = FallbackExecutor::new();
        let candidates: Vec<FallbackCandidate> = vec![];

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(executor.execute(&candidates, |_p, _m| async {
            Ok(LlmResponse {
                content: "should not reach".to_string(),
                tool_calls: Vec::new(),
                finished: true,
            })
        }));

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "No candidates available");
    }

    #[test]
    fn test_fallback_executor_first_fails_second_succeeds() {
        let executor = FallbackExecutor::new();
        let candidates = vec![
            FallbackCandidate {
                provider: "test".to_string(),
                model: "model-1".to_string(),
            },
            FallbackCandidate {
                provider: "test".to_string(),
                model: "model-2".to_string(),
            },
        ];

        let rt = tokio::runtime::Runtime::new().unwrap();
        let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        let result = rt.block_on(executor.execute(&candidates, move |_p, m| {
            let count = call_count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let m_owned = m.to_string();
            async move {
                if count == 0 {
                    Err("first failed".to_string())
                } else {
                    Ok(LlmResponse {
                        content: format!("success from {}", m_owned),
                        tool_calls: Vec::new(),
                        finished: true,
                    })
                }
            }
        }));

        assert!(result.is_ok());
        let fb = result.unwrap();
        assert_eq!(fb.model, "model-2");
        assert_eq!(fb.attempts, 2);
        assert_eq!(fb.response.content, "success from model-2");
    }

    #[test]
    fn test_fallback_executor_default() {
        let executor = FallbackExecutor::default();
        let candidates = vec![FallbackCandidate {
            provider: "test".to_string(),
            model: "model-1".to_string(),
        }];

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(executor.execute(&candidates, |_p, _m| async {
            Ok(LlmResponse {
                content: "ok".to_string(),
                tool_calls: Vec::new(),
                finished: true,
            })
        }));

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_executor_register_tool() {
        let provider = Arc::new(MockProvider::new(vec![]));
        let (_inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, _outbound_rx) = mpsc::channel(16);

        let mut executor =
            AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());
        assert!(executor.tools.is_empty());

        executor.register_tool("test_tool", Arc::new(MockTool { result: "ok".to_string() }));
        assert_eq!(executor.tools.len(), 1);
        assert!(executor.tools.contains_key("test_tool"));
        assert!(executor.agent_config.tools.contains(&"test_tool".to_string()));
    }

    #[tokio::test]
    async fn test_executor_set_fallback_candidates() {
        let provider = Arc::new(MockProvider::new(vec![]));
        let (_inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, _outbound_rx) = mpsc::channel(16);

        let mut executor =
            AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());
        assert!(executor.fallback_candidates.is_empty());

        executor.set_fallback_candidates(vec![
            FallbackCandidate { provider: "p1".to_string(), model: "m1".to_string() },
            FallbackCandidate { provider: "p2".to_string(), model: "m2".to_string() },
        ]);
        assert_eq!(executor.fallback_candidates.len(), 2);
    }

    #[tokio::test]
    async fn test_executor_set_concurrent_mode() {
        let provider = Arc::new(MockProvider::new(vec![]));
        let (_inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, _outbound_rx) = mpsc::channel(16);

        let mut executor =
            AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());
        executor.set_concurrent_mode(ConcurrentMode::Queue, 16);
        assert_eq!(executor.concurrent_mode, ConcurrentMode::Queue);
        assert_eq!(executor.queue_size, 16);
    }

    #[tokio::test]
    async fn test_executor_set_observer() {
        let provider = Arc::new(MockProvider::new(vec![]));
        let (_inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, _outbound_rx) = mpsc::channel(16);

        let mut executor =
            AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());
        assert!(executor.observer.is_none());

        executor.set_observer(Arc::new(MockObserver::new()));
        assert!(executor.observer.is_some());
    }

    #[tokio::test]
    async fn test_executor_set_context_window() {
        let provider = Arc::new(MockProvider::new(vec![]));
        let (_inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, _outbound_rx) = mpsc::channel(16);

        let mut executor =
            AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());
        assert_eq!(executor.context_window, 128_000);

        executor.set_context_window(64000);
        assert_eq!(executor.context_window, 64000);
    }

    #[tokio::test]
    async fn test_executor_set_continuation_manager() {
        let provider = Arc::new(MockProvider::new(vec![]));
        let (_inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, _outbound_rx) = mpsc::channel(16);

        let mut executor =
            AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());
        assert!(executor.continuation_manager.is_none());

        executor.set_continuation_manager(Arc::new(
            crate::loop_continuation::ContinuationManager::new()
        ));
        assert!(executor.continuation_manager.is_some());
    }

    #[test]
    fn test_observer_event_conversation_start() {
        let event = ObserverEvent::ConversationStart {
            trace_id: "t1".to_string(),
            session_key: "s1".to_string(),
            channel: "web".to_string(),
            chat_id: "chat1".to_string(),
        };
        let conv_event = event.to_conversation_event();
        assert_eq!(conv_event.event_type, nemesis_observer::EventType::ConversationStart);
    }

    #[test]
    fn test_observer_event_conversation_end() {
        let event = ObserverEvent::ConversationEnd {
            trace_id: "t1".to_string(),
            session_key: "s1".to_string(),
            total_rounds: 3,
            duration_ms: 1500,
        };
        let conv_event = event.to_conversation_event();
        assert_eq!(conv_event.event_type, nemesis_observer::EventType::ConversationEnd);
    }

    #[test]
    fn test_observer_event_llm_request() {
        let event = ObserverEvent::LlmRequest {
            trace_id: "t1".to_string(),
            round: 1,
            model: "gpt-4".to_string(),
        };
        let conv_event = event.to_conversation_event();
        assert_eq!(conv_event.event_type, nemesis_observer::EventType::LlmRequest);
    }

    #[test]
    fn test_observer_event_llm_response() {
        let event = ObserverEvent::LlmResponse {
            trace_id: "t1".to_string(),
            round: 1,
            duration_ms: 200,
            has_tool_calls: true,
        };
        let conv_event = event.to_conversation_event();
        assert_eq!(conv_event.event_type, nemesis_observer::EventType::LlmResponse);
    }

    #[test]
    fn test_observer_event_tool_call() {
        let event = ObserverEvent::ToolCall {
            trace_id: "t1".to_string(),
            tool_name: "search".to_string(),
            success: true,
            duration_ms: 50,
            round: 1,
        };
        let conv_event = event.to_conversation_event();
        assert_eq!(conv_event.event_type, nemesis_observer::EventType::ToolCall);
    }

    #[tokio::test]
    async fn test_process_message_empty_response() {
        let provider = Arc::new(MockProvider::new(vec![LlmResponse {
            content: String::new(),
            tool_calls: Vec::new(),
            finished: true,
        }]));

        let (_inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, _outbound_rx) = mpsc::channel(16);

        let executor =
            AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());

        // Empty response should not panic
        executor
            .process_message(make_inbound("Hello", "web", ""))
            .await;
    }

    #[tokio::test]
    async fn test_session_persistence_with_store() {
        use crate::session::Summarizer;

        let provider = Arc::new(MockProvider::new(vec![]));
        let (_inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, _outbound_rx) = mpsc::channel(16);

        let session_store = Arc::new(crate::session::SessionStore::new_in_memory());
        let summarizer = Summarizer::new_silent(
            provider.clone(),
            "test-model".to_string(),
            128000,
            session_store.clone(),
        );

        let mut executor =
            AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());
        executor.set_session_persistence(SessionPersistence::with_storage(session_store, summarizer));
    }

    #[test]
    fn test_generate_trace_id() {
        let id = AgentLoopExecutor::generate_trace_id("test-session");
        assert!(id.starts_with("test-session-"));
        assert!(id.len() > "test-session-".len());
    }

    // --- Additional executor coverage tests ---

    #[test]
    fn test_tool_result_from_async_extra() {
        let result = ToolResult::async_result("task-42".to_string(), "waiting...".to_string());
        assert!(result.is_async);
        assert_eq!(result.task_id, "task-42");
        assert_eq!(result.for_user, "waiting...");
    }

    #[test]
    fn test_fallback_result_debug() {
        let fr = FallbackResult {
            provider: "test".to_string(),
            model: "model-1".to_string(),
            response: LlmResponse {
                content: "hello".to_string(),
                tool_calls: vec![],
                finished: true,
            },
            attempts: 1,
        };
        let debug_str = format!("{:?}", fr);
        assert!(debug_str.contains("model-1"));
    }

    #[tokio::test]
    async fn test_executor_process_message_llm_error() {
        struct ErrorProvider;
        #[async_trait]
        impl LlmProvider for ErrorProvider {
            async fn chat(&self, _model: &str, _messages: Vec<LlmMessage>, _options: Option<crate::types::ChatOptions>, _tools: Vec<crate::types::ToolDefinition>) -> Result<LlmResponse, String> {
                Err("LLM failed".to_string())
            }
        }

        let provider = Arc::new(ErrorProvider);
        let (_inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, mut outbound_rx) = mpsc::channel(16);

        let executor =
            AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());

        executor
            .process_message(make_inbound("Hello", "web", ""))
            .await;

        let msg = outbound_rx.recv().await.unwrap();
        assert!(msg.content.contains("Error") || msg.content.contains("LLM failed"));
    }

    #[tokio::test]
    async fn test_executor_set_session_persistence() {
        let provider = Arc::new(MockProvider::new(vec![]));
        let (_inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, _outbound_rx) = mpsc::channel(16);

        let mut executor =
            AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());
        // Default session persistence is in-memory (not None)
        executor.set_session_persistence(SessionPersistence::with_storage(
            Arc::new(crate::session::SessionStore::new_in_memory()),
            crate::session::Summarizer::new_silent(
                Arc::new(MockProvider::new(vec![])),
                "test-model".to_string(),
                128000,
                Arc::new(crate::session::SessionStore::new_in_memory()),
            ),
        ));
    }

    #[tokio::test]
    async fn test_executor_set_observer_manager() {
        let provider = Arc::new(MockProvider::new(vec![]));
        let (_inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, _outbound_rx) = mpsc::channel(16);

        let mut executor =
            AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());

        executor.set_observer_manager(Arc::new(nemesis_observer::Manager::new()));
        assert!(executor.get_observer_manager().is_some());
    }

    #[tokio::test]
    async fn test_process_and_publish_with_tool_call() {
        let provider = Arc::new(MockProvider::new(vec![
            LlmResponse {
                content: String::new(),
                tool_calls: vec![ToolCallInfo {
                    id: "tc_1".to_string(),
                    name: "test_tool".to_string(),
                    arguments: "{}".to_string(),
                }],
                finished: false,
            },
            LlmResponse {
                content: "Tool done.".to_string(),
                tool_calls: vec![],
                finished: true,
            },
        ]));

        let (_inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, mut outbound_rx) = mpsc::channel(16);

        let mut executor =
            AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());
        executor.register_tool("test_tool", Arc::new(MockTool { result: "ok".to_string() }));

        let context = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = executor
            .process_and_publish("sess1", "Do something", &context)
            .await
            .unwrap();

        assert_eq!(result, "Tool done.");
        let msg = outbound_rx.recv().await.unwrap();
        assert_eq!(msg.content, "Tool done.");
    }

    #[tokio::test]
    async fn test_executor_runs_with_channel_close() {
        let provider = Arc::new(MockProvider::new(vec![LlmResponse {
            content: "Done.".to_string(),
            tool_calls: vec![],
            finished: true,
        }]));

        let (inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, mut outbound_rx) = mpsc::channel(16);

        let mut executor =
            AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());

        inbound_tx.send(make_inbound("Hi", "web", "")).await.unwrap();
        drop(inbound_tx);

        executor.run().await;

        let msg = outbound_rx.recv().await.unwrap();
        assert_eq!(msg.content, "Done.");
    }

    #[tokio::test]
    async fn test_executor_context_window_error_retry() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct ContextErrorProvider {
            call_count: AtomicUsize,
        }
        #[async_trait]
        impl LlmProvider for ContextErrorProvider {
            async fn chat(&self, _model: &str, _messages: Vec<LlmMessage>, _options: Option<crate::types::ChatOptions>, _tools: Vec<crate::types::ToolDefinition>) -> Result<LlmResponse, String> {
                let count = self.call_count.fetch_add(1, Ordering::SeqCst);
                if count == 0 {
                    Err("context_length_exceeded".to_string())
                } else {
                    Ok(LlmResponse {
                        content: "Recovered.".to_string(),
                        tool_calls: vec![],
                        finished: true,
                    })
                }
            }
        }

        let provider = Arc::new(ContextErrorProvider { call_count: AtomicUsize::new(0) });
        let (_inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, mut outbound_rx) = mpsc::channel(16);

        let executor =
            AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());

        executor.process_message(make_inbound("Big query", "web", "")).await;

        // First message should be about compression
        let _msg1 = outbound_rx.recv().await.unwrap();
        // Second message should be the actual response
        let msg2 = outbound_rx.recv().await.unwrap();
        assert_eq!(msg2.content, "Recovered.");
    }

    #[test]
    fn test_session_persistence_save_no_store() {
        let persistence = SessionPersistence::new_in_memory();
        assert!(persistence.save_session("test").is_ok());
    }

    #[test]
    fn test_session_persistence_no_summarizer() {
        let persistence = SessionPersistence::new_in_memory();
        let history: Vec<crate::types::ConversationTurn> = vec![];
        assert!(!persistence.maybe_summarize("test", "web", "chat1", &history, 128000));
    }

    // --- Additional coverage for loop_executor ---

    #[tokio::test]
    async fn test_run_agent_loop_simple() {
        let (inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, mut outbound_rx) = mpsc::channel(16);
        drop(inbound_tx);

        let provider = Arc::new(MockProvider::new(vec![
            crate::r#loop::LlmResponse {
                content: "Agent loop result".to_string(),
                tool_calls: Vec::new(),
                finished: true,
            },
        ]));
        let mut executor = AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, ExecutorConfig::default());

        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = executor.run_agent_loop("sess1", "Hello", &ctx).await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("Agent loop result"));
    }

    #[tokio::test]
    async fn test_run_agent_loop_with_tool_call() {
        let (inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, _outbound_rx) = mpsc::channel(16);
        drop(inbound_tx);

        let provider = Arc::new(MockProvider::new(vec![
            crate::r#loop::LlmResponse {
                content: String::new(),
                tool_calls: vec![ToolCallInfo {
                    id: "tc_1".to_string(),
                    name: "calculator".to_string(),
                    arguments: r#"{"expr":"1+1"}"#.to_string(),
                }],
                finished: false,
            },
            crate::r#loop::LlmResponse {
                content: "The answer is 2".to_string(),
                tool_calls: Vec::new(),
                finished: true,
            },
        ]));
        let mut executor = AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, ExecutorConfig::default());
        executor.register_tool("calculator", Arc::new(MockTool { result: "2".to_string() }));

        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = executor.run_agent_loop("sess-tools", "What is 1+1?", &ctx).await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("The answer is 2"));
    }

    #[tokio::test]
    async fn test_run_agent_loop_max_iterations() {
        let (inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, _outbound_rx) = mpsc::channel(16);
        drop(inbound_tx);

        // Every LLM response returns tool calls - the loop will exhaust max_turns
        let infinite_response = crate::r#loop::LlmResponse {
            content: String::new(),
            tool_calls: vec![ToolCallInfo {
                id: "tc_loop".to_string(),
                name: "loop_tool".to_string(),
                arguments: "{}".to_string(),
            }],
            finished: false,
        };
        let responses: Vec<_> = (0..15).map(|_| infinite_response.clone()).collect();
        let provider = Arc::new(MockProvider::new(responses));

        let mut config = ExecutorConfig::default();
        config.max_turns = 3;
        let mut executor = AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, config);
        executor.register_tool("loop_tool", Arc::new(MockTool { result: "0".to_string() }));

        let ctx = RequestContext::new("web", "chat1", "user1", "sess-loop");
        let result = executor.run_agent_loop("sess-loop", "Loop test", &ctx).await;
        assert!(result.is_ok());
        // run_agent_loop does not call check_iteration_limit, so it returns
        // empty content when max turns is reached with only tool calls
        let content = result.unwrap();
        assert!(content.is_empty() || content.contains("No more responses"),
            "Expected empty or exhaustion, got: {}", content);
    }

    #[tokio::test]
    async fn test_check_iteration_limit_hit() {
        let (inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, _outbound_rx) = mpsc::channel(16);
        drop(inbound_tx);

        let provider = Arc::new(MockProvider::new(vec![]));
        let mut config = ExecutorConfig::default();
        config.max_turns = 5;
        let executor = AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, config);

        let result = executor.check_iteration_limit("", 5);
        assert!(result.contains("Max iterations"));
    }

    #[tokio::test]
    async fn test_check_iteration_limit_not_hit() {
        let (inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, _outbound_rx) = mpsc::channel(16);
        drop(inbound_tx);

        let provider = Arc::new(MockProvider::new(vec![]));
        let executor = AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, ExecutorConfig::default());

        let result = executor.check_iteration_limit("Normal response", 3);
        assert_eq!(result, "Normal response");
    }

    #[tokio::test]
    async fn test_handle_tool_calls_batch() {
        let (inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, _outbound_rx) = mpsc::channel(16);
        drop(inbound_tx);

        let provider = Arc::new(MockProvider::new(vec![]));
        let mut executor = AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, ExecutorConfig::default());
        executor.register_tool("tool_a", Arc::new(MockTool { result: "A result".to_string() }));
        executor.register_tool("tool_b", Arc::new(MockTool { result: "B result".to_string() }));

        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let tool_calls = vec![
            ToolCallInfo {
                id: "tc_1".to_string(),
                name: "tool_a".to_string(),
                arguments: "{}".to_string(),
            },
            ToolCallInfo {
                id: "tc_2".to_string(),
                name: "tool_b".to_string(),
                arguments: "{}".to_string(),
            },
        ];
        let results = executor.handle_tool_calls(&tool_calls, &ctx, "trace-1", 1).await;
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].result, "A result");
        assert_eq!(results[1].result, "B result");
    }

    #[tokio::test]
    async fn test_handle_tool_calls_unknown_tool() {
        let (inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, _outbound_rx) = mpsc::channel(16);
        drop(inbound_tx);

        let provider = Arc::new(MockProvider::new(vec![]));
        let executor = AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, ExecutorConfig::default());

        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let tool_calls = vec![ToolCallInfo {
            id: "tc_1".to_string(),
            name: "nonexistent".to_string(),
            arguments: "{}".to_string(),
        }];
        let results = executor.handle_tool_calls(&tool_calls, &ctx, "trace-1", 1).await;
        assert_eq!(results.len(), 1);
        assert!(results[0].is_error);
        assert!(results[0].result.contains("Unknown tool"));
    }

    #[tokio::test]
    async fn test_update_tool_contexts() {
        let (inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, _outbound_rx) = mpsc::channel(16);
        drop(inbound_tx);

        let provider = Arc::new(MockProvider::new(vec![]));
        let executor = AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, ExecutorConfig::default());

        // Should not panic even without context-aware tools
        executor.update_tool_contexts("web", "chat1");
    }

    #[test]
    fn test_tool_result_for_llm_only_values() {
        let result = ToolResult::for_llm_only("LLM content".to_string());
        assert_eq!(result.for_llm, "LLM content");
        assert!(result.silent);
        assert!(!result.is_async);
    }

    #[test]
    fn test_tool_result_async_result_values() {
        let result = ToolResult::async_result("task-123".to_string(), "Working on it...".to_string());
        assert!(result.is_async);
        assert_eq!(result.task_id, "task-123");
        assert!(!result.silent);
        assert_eq!(result.for_user, "Working on it...");
    }

    #[test]
    fn test_fallback_result_fields() {
        let result = FallbackResult {
            response: crate::r#loop::LlmResponse {
                content: "Hello".to_string(),
                tool_calls: Vec::new(),
                finished: true,
            },
            provider: "provider1".to_string(),
            model: "model1".to_string(),
            attempts: 2,
        };
        assert_eq!(result.provider, "provider1");
        assert_eq!(result.attempts, 2);
    }

    #[tokio::test]
    async fn test_call_llm_with_fallback_no_candidates() {
        let (inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, _outbound_rx) = mpsc::channel(16);
        drop(inbound_tx);

        let provider = Arc::new(MockProvider::new(vec![
            crate::r#loop::LlmResponse {
                content: "Direct response".to_string(),
                tool_calls: Vec::new(),
                finished: true,
            },
        ]));
        let executor = AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, ExecutorConfig::default());

        let messages = vec![crate::r#loop::LlmMessage {
            role: "user".to_string(),
            content: "Hello".to_string(),
            tool_calls: None,
            tool_call_id: None,
        }];
        let result = executor.call_llm_with_fallback(&messages, None, vec![]).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().content, "Direct response");
    }

    #[test]
    fn test_fallback_candidate_clone() {
        let candidate = FallbackCandidate {
            provider: "prov1".to_string(),
            model: "mod1".to_string(),
        };
        let cloned = candidate.clone();
        assert_eq!(cloned.provider, "prov1");
        assert_eq!(cloned.model, "mod1");
    }

    #[test]
    fn test_executor_config_clone() {
        let config = ExecutorConfig {
            model: "gpt-4".to_string(),
            max_turns: 5,
            system_prompt: Some("You are helpful".to_string()),
            event_buffer_size: 32,
        };
        let cloned = config.clone();
        assert_eq!(cloned.model, "gpt-4");
        assert_eq!(cloned.max_turns, 5);
    }

    #[test]
    fn test_session_persistence_with_storage_creation() {
        use async_trait::async_trait;

        struct SilentProvider;
        #[async_trait]
        impl crate::r#loop::LlmProvider for SilentProvider {
            async fn chat(
                &self,
                _model: &str,
                _messages: Vec<crate::r#loop::LlmMessage>,
                _options: Option<crate::types::ChatOptions>,
                _tools: Vec<crate::types::ToolDefinition>,
            ) -> Result<crate::r#loop::LlmResponse, String> {
                Ok(crate::r#loop::LlmResponse {
                    content: "summary".to_string(),
                    tool_calls: Vec::new(),
                    finished: true,
                })
            }
        }

        let store = Arc::new(crate::session::SessionStore::new_in_memory());
        let summarizer = crate::session::Summarizer::new_silent(
            Arc::new(SilentProvider),
            "test-model".to_string(),
            128000,
            store.clone(),
        );
        let persistence = SessionPersistence::with_storage(store, summarizer);
        // save_session should work
        assert!(persistence.save_session("test-key").is_ok());
    }

    // --- Additional coverage tests ---

    #[test]
    fn test_concurrent_mode_variants() {
        assert_eq!(ConcurrentMode::Reject, ConcurrentMode::Reject);
        assert_eq!(ConcurrentMode::Queue, ConcurrentMode::Queue);
        assert_ne!(ConcurrentMode::Reject, ConcurrentMode::Queue);
    }

    #[test]
    fn test_tool_result_debug() {
        let result = ToolResult::simple("test content".to_string());
        let debug = format!("{:?}", result);
        assert!(debug.contains("test content"));
    }

    #[test]
    fn test_tool_result_error_debug() {
        let result = ToolResult::error("something failed".to_string());
        let debug = format!("{:?}", result);
        assert!(debug.contains("something failed"));
    }

    #[test]
    fn test_tool_result_async_debug() {
        let result = ToolResult::async_result("task-99".to_string(), "processing".to_string());
        let debug = format!("{:?}", result);
        assert!(debug.contains("task-99"));
    }

    #[test]
    fn test_observer_event_all_variants_debug() {
        let start = ObserverEvent::ConversationStart {
            trace_id: "t1".to_string(),
            session_key: "s1".to_string(),
            channel: "web".to_string(),
            chat_id: "c1".to_string(),
        };
        assert!(format!("{:?}", start).contains("t1"));

        let end = ObserverEvent::ConversationEnd {
            trace_id: "t2".to_string(),
            session_key: "s2".to_string(),
            total_rounds: 5,
            duration_ms: 1000,
        };
        assert!(format!("{:?}", end).contains("t2"));

        let req = ObserverEvent::LlmRequest {
            trace_id: "t3".to_string(),
            round: 1,
            model: "gpt-4".to_string(),
        };
        assert!(format!("{:?}", req).contains("gpt-4"));

        let resp = ObserverEvent::LlmResponse {
            trace_id: "t4".to_string(),
            round: 2,
            duration_ms: 500,
            has_tool_calls: false,
        };
        assert!(format!("{:?}", resp).contains("t4"));

        let tool = ObserverEvent::ToolCall {
            trace_id: "t5".to_string(),
            tool_name: "read_file".to_string(),
            success: true,
            duration_ms: 10,
            round: 1,
        };
        assert!(format!("{:?}", tool).contains("read_file"));
    }

    #[test]
    fn test_fallback_executor_cooldown_skips() {
        let executor = FallbackExecutor::new();
        let candidates = vec![
            FallbackCandidate {
                provider: "test".to_string(),
                model: "model-1".to_string(),
            },
            FallbackCandidate {
                provider: "test".to_string(),
                model: "model-2".to_string(),
            },
        ];

        let rt = tokio::runtime::Runtime::new().unwrap();
        // First call: model-1 fails, model-2 succeeds
        let result = rt.block_on(executor.execute(&candidates, |_p, m| {
            let m_owned = m.to_string();
            async move {
                if m_owned == "model-1" {
                    Err("fail".to_string())
                } else {
                    Ok(LlmResponse {
                        content: "ok".to_string(),
                        tool_calls: Vec::new(),
                        finished: true,
                    })
                }
            }
        }));
        assert!(result.is_ok());
        assert_eq!(result.unwrap().model, "model-2");
    }

    #[tokio::test]
    async fn test_executor_context_window_default() {
        let provider = Arc::new(MockProvider::new(vec![]));
        let (_inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, _outbound_rx) = mpsc::channel(16);

        let executor =
            AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, ExecutorConfig::default());
        assert_eq!(executor.context_window, 128_000);
    }

    #[test]
    fn test_session_persistence_with_storage_save() {
        use async_trait::async_trait;

        struct SilentProvider;
        #[async_trait]
        impl crate::r#loop::LlmProvider for SilentProvider {
            async fn chat(
                &self,
                _model: &str,
                _messages: Vec<crate::r#loop::LlmMessage>,
                _options: Option<crate::types::ChatOptions>,
                _tools: Vec<crate::types::ToolDefinition>,
            ) -> Result<crate::r#loop::LlmResponse, String> {
                Ok(crate::r#loop::LlmResponse {
                    content: "summary".to_string(),
                    tool_calls: Vec::new(),
                    finished: true,
                })
            }
        }

        let store = Arc::new(crate::session::SessionStore::new_in_memory());
        let summarizer = crate::session::Summarizer::new_silent(
            Arc::new(SilentProvider),
            "test-model".to_string(),
            128000,
            store.clone(),
        );
        let persistence = SessionPersistence::with_storage(store, summarizer);

        // Save a session
        assert!(persistence.save_session("test-session").is_ok());
    }

    #[tokio::test]
    async fn test_call_llm_with_fallback_with_candidates() {
        let (inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, _outbound_rx) = mpsc::channel(16);
        drop(inbound_tx);

        let provider = Arc::new(MockProvider::new(vec![
            crate::r#loop::LlmResponse {
                content: "Fallback response".to_string(),
                tool_calls: Vec::new(),
                finished: true,
            },
        ]));
        let mut executor = AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, ExecutorConfig::default());
        executor.set_fallback_candidates(vec![
            FallbackCandidate { provider: "p1".to_string(), model: "m1".to_string() },
        ]);

        let messages = vec![crate::r#loop::LlmMessage {
            role: "user".to_string(),
            content: "Hello".to_string(),
            tool_calls: None,
            tool_call_id: None,
        }];
        let result = executor.call_llm_with_fallback(&messages, None, vec![]).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_observer_event_to_conversation_event_all() {
        // ConversationEnd
        let event = ObserverEvent::ConversationEnd {
            trace_id: "t1".to_string(),
            session_key: "s1".to_string(),
            total_rounds: 5,
            duration_ms: 2000,
        };
        let ce = event.to_conversation_event();
        assert_eq!(ce.event_type, nemesis_observer::EventType::ConversationEnd);

        // LlmRequest
        let event = ObserverEvent::LlmRequest {
            trace_id: "t2".to_string(),
            round: 3,
            model: "gpt-4".to_string(),
        };
        let ce = event.to_conversation_event();
        assert_eq!(ce.event_type, nemesis_observer::EventType::LlmRequest);

        // LlmResponse with no tool calls
        let event = ObserverEvent::LlmResponse {
            trace_id: "t3".to_string(),
            round: 1,
            duration_ms: 100,
            has_tool_calls: false,
        };
        let ce = event.to_conversation_event();
        assert_eq!(ce.event_type, nemesis_observer::EventType::LlmResponse);

        // ToolCall
        let event = ObserverEvent::ToolCall {
            trace_id: "t4".to_string(),
            tool_name: "write_file".to_string(),
            success: false,
            duration_ms: 50,
            round: 2,
        };
        let ce = event.to_conversation_event();
        assert_eq!(ce.event_type, nemesis_observer::EventType::ToolCall);
    }

    #[tokio::test]
    async fn test_executor_register_multiple_tools() {
        let provider = Arc::new(MockProvider::new(vec![]));
        let (_inbound_tx, inbound_rx) = mpsc::channel(16);
        let (outbound_tx, _outbound_rx) = mpsc::channel(16);

        let mut executor =
            AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());

        executor.register_tool("tool1", Arc::new(MockTool { result: "r1".to_string() }));
        executor.register_tool("tool2", Arc::new(MockTool { result: "r2".to_string() }));
        executor.register_tool("tool3", Arc::new(MockTool { result: "r3".to_string() }));

        assert_eq!(executor.tools.len(), 3);
        assert!(executor.tools.contains_key("tool1"));
        assert!(executor.tools.contains_key("tool2"));
        assert!(executor.tools.contains_key("tool3"));
    }

    #[test]
    fn test_generate_trace_id_uniqueness() {
        let id1 = AgentLoopExecutor::generate_trace_id("session-1");
        let id2 = AgentLoopExecutor::generate_trace_id("session-1");
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_executor_config_custom() {
        let config = ExecutorConfig {
            model: "custom-model".to_string(),
            max_turns: 20,
            system_prompt: Some("Be helpful".to_string()),
            event_buffer_size: 128,
        };
        assert_eq!(config.model, "custom-model");
        assert_eq!(config.max_turns, 20);
        assert_eq!(config.event_buffer_size, 128);
    }

    #[test]
    fn test_fallback_executor_clears_cooldown_on_success() {
        let executor = FallbackExecutor::new();
        let candidates = vec![
            FallbackCandidate {
                provider: "test".to_string(),
                model: "model-1".to_string(),
            },
        ];

        let rt = tokio::runtime::Runtime::new().unwrap();

        // First call succeeds
        let result = rt.block_on(executor.execute(&candidates, |_p, _m| async {
            Ok(LlmResponse {
                content: "success".to_string(),
                tool_calls: Vec::new(),
                finished: true,
            })
        }));
        assert!(result.is_ok());

        // Second call should also succeed (no cooldown from previous success)
        let result2 = rt.block_on(executor.execute(&candidates, |_p, _m| async {
            Ok(LlmResponse {
                content: "success2".to_string(),
                tool_calls: Vec::new(),
                finished: true,
            })
        }));
        assert!(result2.is_ok());
    }
}

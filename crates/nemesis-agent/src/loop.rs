//! Agent loop: core execution engine that drives LLM + tool interactions.
//!
//! The loop processes messages through these stages:
//!
//! 1. Build context from conversation history
//! 2. Call the LLM provider
//! 3. If the response contains tool calls, execute them and append results
//! 4. Repeat until a plain text response is produced or `max_turns` is reached
//!
//! # Bus-integrated mode
//!
//! The `AgentLoop` can be used in two ways:
//!
//! - **Standalone mode**: Direct calls via `run()`, `process_direct()`, etc.
//! - **Bus-integrated mode**: Continuous consumption from a message bus via
//!   `run_bus_arc()`.
//!
//! In bus-integrated mode, the loop connects to an `mpsc` inbound/outbound
//! channel pair and handles the full Go `AgentLoop` lifecycle including
//! system message routing, history requests, cluster continuation, slash
//! commands, session busy management, summarization, and startup info.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, warn};

use crate::context::RequestContext;
use crate::instance::AgentInstance;
use crate::registry::AgentRegistry;
use crate::session::{SessionStore, estimate_tokens_for_turns_projected};
use crate::types::{AgentConfig, AgentEvent, ToolCallInfo, ToolCallResult};
use nemesis_routing::{AgentDef, RouteConfig, RouteInput as RoutingRouteInput, RouteResolver};

/// Grace-round nudge injected when the tool-call budget is exhausted (②).
/// The model gets one extra round to synthesize a final answer from the work
/// already done, instead of hard-stopping with "Max iterations reached". This
/// is a TRANSIENT system message — appended to the built message list for the
/// grace round only, never persisted to instance history or session_log.
const GRACE_ROUND_NUDGE: &str = "工具调用预算已用尽，不要再调用任何工具。请基于已完成的工作给出最终答复：总结完成了什么、还有什么没做、需要用户做哪些决定。";

/// Max retries for transient LLM errors (network / stream / 5xx) before giving
/// up (③). Retries do NOT consume the `turns_used` budget — the increment at
/// the end of an iteration happens once regardless of how many retries it took
/// to get a successful response.
const MAX_TRANSIENT_RETRIES: u32 = 3;

/// ⑩ Per-session compaction tracking for graded tiers + stuck self-check.
/// Keyed by session; lives on `AgentLoop`.
#[derive(Default)]
struct CompactState {
    /// Whether the soft-tier (50%) notice has already been emitted this session
    /// (one-shot; do not nag).
    soft_noticed: bool,
    /// Token estimate at the time of the last summarization. The next
    /// summarization-triggered call compares against this to detect whether
    /// summarization is keeping up.
    last_summary_tokens: usize,
    /// Consecutive summarizations that failed to meaningfully reduce the
    /// prompt. At [`COMPACT_STUCK_LIMIT`] we pause auto-summarization.
    consecutive_failures: u32,
    /// Auto-summarization paused (stuck). Re-checked each call; cleared once
    /// the prompt drops back below the summarize threshold.
    stuck: bool,
}

/// ⑩ Soft tier: prompt at this fraction of context_window emits a one-shot
/// info notice (no summarization, cache-stable prefix intact).
const COMPACT_SOFT_RATIO: usize = 50; // % of context_window
/// ⑩ Summarize tier: at this fraction, trigger summarization.
const COMPACT_SUMMARIZE_RATIO: usize = 75; // % (unchanged from legacy behavior)
/// ⑩ Stuck: a summarization counts as ineffective when the prompt afterwards
/// is still at least this fraction of its pre-summarization size.
const COMPACT_STUCK_PLATEAU_RATIO: usize = 90; // %
/// ⑩ Stuck limit: after this many consecutive ineffective summarizations,
/// pause auto-summarization and warn.
const COMPACT_STUCK_LIMIT: u32 = 2;

/// Target number of trailing messages kept verbatim (not summarized) when the
/// summary cache advances. The tail `history[C..]` is held at ~`K_TARGET`
/// messages by `maybe_update_summary` (C = len - K_TARGET). Small enough that
/// the LLM always sees recent context verbatim, large enough to ride out a
/// few tool-call rounds between summarizations.
const K_TARGET: usize = 6;

/// Aggressive verbatim-tail size used by `force_compression` (the last-resort
/// retry path on LLM context errors). Smaller than `K_TARGET` because the
/// situation is already an emergency — prefer a larger summarized prefix over
/// failing the request. A second compression folds everything into the summary
/// (tail → 0).
const SMALL_K_FORCE: usize = 2;

/// Session-keyed state maps on `AgentLoop` (`compact_state`, `summarizing`) are
/// size-bounded: when one exceeds this many entries we clear it wholesale. These
/// maps are best-effort — losing an entry just re-learns the state on the next
/// relevant call — and a wholesale clear under size pressure avoids the
/// unbounded-growth anti-pattern (one entry per session, never evicted, leaking
/// for the whole bot-process lifetime).
pub(crate) const SESSION_STATE_MAX_ENTRIES: usize = 512;

/// ⑩ Pure predicate: did a summarization fail to meaningfully reduce the
/// prompt? True when there was a prior summarization (`last_summary_tokens > 0`)
/// AND the current prompt is still at least [`COMPACT_STUCK_PLATEAU_RATIO`]% of
/// the pre-summarization size. Extracted so the threshold logic is unit-testable.
fn summarize_was_ineffective(last_summary_tokens: usize, current_tokens: usize) -> bool {
    last_summary_tokens > 0
        && current_tokens >= last_summary_tokens * COMPACT_STUCK_PLATEAU_RATIO / 100
}

/// Voice-playback suffix appended to the last user message when voice mode is
/// on (transient — never persisted to instance history / session_log).
/// T8 (U9 ②): hoisted from an inline literal so the replay ledger records the
/// exact same bytes the provider saw (single source of truth).
pub(crate) const VOICE_PLAYBACK_SUFFIX: &str = "（语音播报模式已开启，请用简洁、便于口语播报的方式回复，避免使用代码块、表格等不适合语音的内容。）";

/// Fold the summary cache over the history + enforce tool-pair consistency —
/// the persisted-derived projection every main-loop LLM request starts from.
///
/// T8 (U9 ②): extracted from `build_messages_with_memory` so the replay
/// ledger (`crate::replay`) rebuilds requests through the SAME code path —
/// production build and audit replay cannot drift apart (single source of
/// truth; same lesson as the T7 memory-tool schema-drift fix).
///
/// `summary` is `(text, covers_up_to)`; `None` sends the history verbatim.
/// `covers_up_to` indexes the full history vector including the system prompt
/// at index 0. The system prompt is never summarized — it is rebuilt as the
/// leading system message with the summary appended (so the cached prefix
/// stays stable between summary updates).
pub(crate) fn project_history_for_request(
    history: &[crate::types::ConversationTurn],
    summary: Option<(&str, usize)>,
) -> Vec<crate::types::ConversationTurn> {
    let mut turns: Vec<crate::types::ConversationTurn> = if let Some((text, covers_up_to)) = summary
    {
        let c_idx = covers_up_to.min(history.len());
        // Verbatim tail starts at c_idx but never re-includes the system
        // prompt at index 0 (it is rebuilt as the leading message below).
        let tail_start = c_idx.max(1).min(history.len());
        let summary_block = format!("\n\n## Summary of Previous Conversation\n\n{}", text);

        let mut out: Vec<crate::types::ConversationTurn> =
            Vec::with_capacity(history.len() - tail_start + 1);
        if history.first().map_or(false, |t| t.role == "system") {
            // Merge the summary into the configured system prompt (history[0]).
            let mut sys = history[0].clone();
            sys.content.push_str(&summary_block);
            out.push(sys);
        } else {
            // No system prompt at history[0]: emit the summary as a
            // dedicated leading system turn so the provider still sees it.
            out.push(crate::types::ConversationTurn {
                role: "system".to_string(),
                content: summary_block,
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: chrono::Local::now().to_rfc3339(),
                reasoning_content: None,
                tool_name: None,
                tool_result_projection: None,
            });
        }
        out.extend(history[tail_start..].iter().cloned());
        out
    } else {
        history.to_vec()
    };

    // X1 (U3 projection prune): tool results fold to their bounded
    // model-facing form HERE — history keeps the originals (recoverable
    // mid-sections, branchable history), the provider never sees an
    // oversized tool result. Recorded override wins (spill locator / guard
    // nudges — not recomputable); else the pure prune recompute. Idempotent:
    // prune output stays under the inline threshold, so old sessions whose
    // tool content is already pruned pass through byte-untouched. Because
    // replay rebuilds through this same function, the fold automatically
    // applies to audit replay too (no injection-ledger entry needed — the
    // transform is a pure function of the history state).
    for turn in &mut turns {
        if turn.role == "tool" {
            turn.content = turn.model_facing_content().into_owned();
        }
    }

    // Enforce tool-pair consistency at the LLM boundary. Upstream paths
    // (summarization, session save/load) can leave an assistant tool_call
    // whose result was dropped — or vice versa — and providers then reject
    // the whole request with 400 "insufficient tool messages following
    // tool_calls". Every main-loop LLM call's messages flow through here,
    // so repairing this local copy is the universal guarantee: the
    // provider never sees an inconsistent sequence, regardless of which
    // upstream path produced the history. (Non-destructive: the instance's
    // own history is untouched; only the outgoing view is cleaned.)
    crate::types::repair_tool_message_pairs(&mut turns);
    turns
}

/// A simplified LLM message used for building requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMessage {
    pub role: String,
    pub content: String,
    pub tool_calls: Option<Vec<ToolCallInfo>>,
    pub tool_call_id: Option<String>,
    /// Reasoning content from thinking-mode models, passed back to the API.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reasoning_content: Option<String>,
}

/// A simplified LLM response.
#[derive(Debug, Clone)]
pub struct LlmResponse {
    /// Text content of the response. May be empty if tool_calls are present.
    pub content: String,
    /// Tool calls requested by the LLM, if any.
    pub tool_calls: Vec<ToolCallInfo>,
    /// Whether the LLM indicated it is finished (no more tool calls).
    pub finished: bool,
    /// Reasoning content from thinking-mode models.
    pub reasoning_content: Option<String>,
    /// Token usage from the provider response.
    pub usage: Option<crate::loop_executor::ObserverUsageInfo>,
    /// Raw HTTP request body (for raw logging mode).
    pub raw_request_body: Option<serde_json::Value>,
    /// Raw HTTP response body (for raw logging mode).
    pub raw_response_body: Option<String>,
}

/// Trait for LLM providers used by the agent loop.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Send a chat request and return the response, or an error if the call fails.
    ///
    /// The agent loop uses the `Err` variant to detect context-window errors
    /// (token limit, context length exceeded, etc.) and trigger history compression.
    ///
    /// The `options` parameter controls generation parameters (temperature, max_tokens, etc.).
    /// Pass `None` to use provider defaults.
    ///
    /// The `tools` parameter provides tool definitions for function calling.
    async fn chat(
        &self,
        model: &str,
        messages: Vec<LlmMessage>,
        options: Option<crate::types::ChatOptions>,
        tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String>;
}

/// A previewable file change, used by the checkpoint (edit safety net) to
/// snapshot a file's pre-edit state so a `/rewind` can restore it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileChange {
    /// Path the tool will modify (as given in args; resolved against workspace
    /// root at snapshot/restore time).
    pub path: String,
    /// Kind of change — determines how a rewind restores it.
    pub kind: FileChangeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FileChangeKind {
    /// File did not exist before the edit; rewind deletes it.
    Create,
    /// File existed and is being modified; rewind restores old content.
    Modify,
    /// File existed and is being deleted; rewind restores old content.
    Delete,
}

/// Trait for tools that can be executed by the agent loop.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Execute the tool with the given arguments, returning a string result.
    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String>;

    /// Set the execution context (channel + chat_id) for context-aware tools.
    ///
    /// This is called before each LLM iteration to inject the current channel
    /// and chat_id into tools that need them for routing (e.g., message, spawn,
    /// cluster_rpc). The default implementation is a no-op; tools that need
    /// context should override this method.
    fn set_context(&self, _channel: &str, _chat_id: &str) {}

    /// Return a human-readable description of this tool for the LLM.
    /// Mirrors Go's Tool.Description() string.
    fn description(&self) -> String {
        String::new()
    }

    /// Return the JSON schema for this tool's parameters.
    /// Mirrors Go's Tool.Parameters() map[string]interface{}.
    /// Should return a serde_json::Value representing an OpenAI-compatible
    /// JSON Schema object (e.g., {"type": "object", "properties": {...}}).
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {}})
    }

    /// Preview the file change this call would make, for checkpointing (the edit
    /// safety net). Synchronous — parse `args` (the same JSON string passed to
    /// `execute`) to determine the target path and change kind only; the
    /// checkpoint store reads the file's current content separately (async).
    /// Returns `None` for read-only tools or non-file tools (default), so only
    /// writer tools opt in. Never panic on malformed args — return `None`.
    fn preview(&self, _args: &str) -> Option<FileChange> {
        None
    }

    /// U5 (sixth batch): whether this tool is a pure read with no side effects
    /// (filesystem read, list, search, web fetch — safe to run concurrently
    /// with other read-only calls in the same tool batch). Default `false`
    /// — FAIL-CLOSED: a tool that has not declared itself read-only never
    /// joins the parallel pool, so a latent writer can't slip in. Writer
    /// tools and `exec` (even `cat`) stay `false`.
    fn is_read_only(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// Internal channel detection
// ---------------------------------------------------------------------------

/// Check if a channel is internal (not user-facing).
pub fn is_internal_channel(channel: &str) -> bool {
    matches!(channel, "cli" | "system" | "subagent")
}

// ---------------------------------------------------------------------------
// Session busy state management
// ---------------------------------------------------------------------------

/// Busy message returned when session is busy.
pub const BUSY_MESSAGE: &str =
    "\u{23f3} AI is processing a previous request, please try again later";

/// Concurrent request handling mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConcurrentMode {
    /// Reject new messages when session is busy (default; legacy behavior).
    Reject,
    /// Queue messages when session is busy — processed after the current turn.
    Queue,
    /// Queue + steer: `!`-prefixed messages are injected into the RUNNING
    /// turn before its next LLM call (I1 / U7).
    Steer,
}

/// V5 (2026-08-23): outcome of the synchronous inbound gate (`gate_inbound`).
enum GateOutcome {
    /// `cluster_continuation` marker — the pump handles it inline via
    /// `dispatch_continuation` (serial, unchanged from the legacy loop).
    Continuation(String),
    /// Short-circuit reply (busy receipt / busy bounce / queue-full / slash
    /// command response). No session was acquired.
    Immediate {
        agent_id: String,
        response: String,
    },
    /// System (non-continuation) / history-request passthrough — async
    /// handling, no session semantics.
    Ungated,
    /// Normal chat message: the session is already acquired and the cancel
    /// token minted. The tail (`process_admitted`) owns release + drain.
    Admitted(TurnAdmission),
}

/// V5: admission minted by the gate — everything the turn tail needs that
/// the gate acquired on the message's behalf.
struct TurnAdmission {
    agent_id: String,
    session_key: String,
    cancel_token: tokio_util::sync::CancellationToken,
}

/// Parse the config string. Unknown values fall back to Reject (fail-safe
/// to legacy behavior) with a warn at the call site.
pub fn parse_concurrent_mode(s: &str) -> ConcurrentMode {
    match s.trim().to_lowercase().as_str() {
        "queue" => ConcurrentMode::Queue,
        "steer" => ConcurrentMode::Steer,
        _ => ConcurrentMode::Reject,
    }
}

impl Default for ConcurrentMode {
    fn default() -> Self {
        Self::Reject
    }
}

/// Tracks busy state for sessions.
pub struct SessionBusyTracker {
    busy: dashmap::DashSet<String>,
    #[allow(dead_code)] // Reserved for future concurrent-mode-aware queue logic
    mode: ConcurrentMode,
    #[allow(dead_code)] // Reserved for future concurrent-mode-aware queue logic
    queue_size: usize,
}

impl SessionBusyTracker {
    /// Create a new tracker with the given mode.
    pub fn new(mode: ConcurrentMode, queue_size: usize) -> Self {
        Self {
            busy: dashmap::DashSet::new(),
            mode,
            queue_size,
        }
    }

    /// Try to acquire a session for processing. Returns false if busy and mode is Reject.
    pub fn try_acquire(&self, session_key: &str) -> bool {
        if self.busy.contains(session_key) {
            return false;
        }
        self.busy.insert(session_key.to_string());
        true
    }

    /// Release a session after processing.
    pub fn release(&self, session_key: &str) {
        self.busy.remove(session_key);
    }

    /// Check whether a session is currently busy.
    pub fn is_busy(&self, session_key: &str) -> bool {
        self.busy.contains(session_key)
    }
}

// ---------------------------------------------------------------------------
// ProcessOptions -- options for how a message is processed
// ---------------------------------------------------------------------------

/// Configuration for how a message is processed through the agent loop.
#[derive(Debug, Clone)]
pub struct ProcessOptions {
    /// Session identifier for history/context.
    pub session_key: String,
    /// Target channel for tool execution.
    pub channel: String,
    /// Target chat ID for tool execution.
    pub chat_id: String,
    /// User message content.
    pub user_message: String,
    /// Response when LLM returns empty.
    pub default_response: String,
    /// Whether to trigger summarization.
    pub enable_summary: bool,
    /// Whether to send response via bus.
    pub send_response: bool,
    /// If true, don't load session history (for heartbeat).
    pub no_history: bool,
    /// Trace ID for observer events.
    pub trace_id: String,
}

impl Default for ProcessOptions {
    fn default() -> Self {
        Self {
            session_key: String::new(),
            channel: String::new(),
            chat_id: String::new(),
            user_message: String::new(),
            default_response: "I've completed processing but have no response to give.".to_string(),
            enable_summary: true,
            send_response: false,
            no_history: false,
            trace_id: String::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Per-session busy state with queue length
// ---------------------------------------------------------------------------

/// Per-session busy state with queue length.
#[derive(Debug, Clone, Default)]
struct SessionBusyState {
    busy: bool,
    queue_length: usize,
}

// ---------------------------------------------------------------------------
// MessageTool sent-in-round tracking (mirrors Go's MessageTool.HasSentInRound)
// ---------------------------------------------------------------------------

/// Tracks whether a message has already been sent in the current LLM round.
/// This prevents double-sending when the agent loop also publishes outbound.
#[derive(Debug, Default)]
struct SentInRoundTracker {
    /// session_key -> whether a tool already sent a message this round.
    sent: parking_lot::Mutex<std::collections::HashSet<String>>,
}

impl SentInRoundTracker {
    fn new() -> Self {
        Self::default()
    }

    /// Mark that a message was sent for the given session key this round.
    fn mark_sent(&self, session_key: &str) {
        self.sent.lock().insert(session_key.to_string());
    }

    /// Check if a message was already sent for the given session key.
    fn has_sent_in_round(&self, session_key: &str) -> bool {
        self.sent.lock().contains(session_key)
    }

    /// Clear the sent flag for a session (start of new round).
    fn clear(&self, session_key: &str) {
        self.sent.lock().remove(session_key);
    }

    /// Clear all sent flags.
    #[allow(dead_code)]
    fn clear_all(&self) {
        self.sent.lock().clear();
    }
}

// ---------------------------------------------------------------------------
// AgentLoop -- core execution engine
// ---------------------------------------------------------------------------

/// The core agent execution loop.
///
/// In standalone mode, this wraps a single LLM provider, tool registry,
/// and agent config. In bus-integrated mode, it additionally owns a
/// registry of agent instances, a message bus adapter, summarizer,
/// and session busy tracker.

/// U5 (sixth batch): one parallel-executed read-only call's result, captured
/// for serial guard replay in source order. `validation_failed` lets the
/// serial loop replay the `validation_failures` counter exactly as the serial
/// path would (Invalid → +1; Valid/Fixed → reset to 0).
struct PrecomputedTool {
    result: String,
    validation_failed: bool,
    duration_ms: u64,
}

pub struct AgentLoop {
    // --- Standalone fields (always present) ---
    /// LLM provider for generating responses.
    /// Wrapped in `RwLock<Arc<...>>` for runtime provider swapping (model switch).
    /// Spawned tasks clone the Arc (cheap), so in-flight requests finish with the
    /// old provider while new requests use the updated one.
    provider: parking_lot::RwLock<Arc<dyn LlmProvider>>,
    /// Active model name, kept in sync with the provider above.
    /// Separated from `config.model` so runtime swaps don't need `&mut self`.
    active_model: parking_lot::RwLock<String>,
    /// Tool registry: name -> tool implementation.
    /// Each tool is wrapped in `Arc` so the map can be cloned and shared
    /// with spawned tasks without requiring `Box` clone support.
    /// Wrapped in `RwLock` for interior mutability — MCP hot-reload needs
    /// to register new tools from `&self` methods (inside the run loop).
    tools: parking_lot::RwLock<HashMap<String, Arc<dyn Tool>>>,
    /// Agent configuration.
    config: AgentConfig,

    // --- Bus-integrated fields (optional) ---
    /// Outbound message sender for bus mode.
    outbound_tx: Option<tokio::sync::mpsc::Sender<nemesis_types::channel::OutboundMessage>>,
    /// Agent registry for multi-agent routing.
    registry: Option<Arc<AgentRegistry>>,
    /// State manager for recording last channel/chat ID (persistent on disk).
    state_manager: Option<Arc<nemesis_state::workspace_state::WorkspaceStateManager>>,
    /// Session store for persistent history.
    session_store: Option<Arc<SessionStore>>,
    /// Running flag for the bus consumption loop.
    running: AtomicBool,
    /// Per-session busy state with queue length tracking.
    session_busy: parking_lot::Mutex<HashMap<String, SessionBusyState>>,
    /// Session busy check: mode-aware I1 (U7).
    /// Concurrent request handling mode.
    concurrent_mode: ConcurrentMode,
    /// Re-injection sender into the agent's own inbound mpsc (round-5 review
    /// fix). The queue-drain path uses it to hand the queued head back to the
    /// normal `run_bus_*` consumer instead of recursing inline — so the reply
    /// gets the SAME post-processing as any other message (rpc correlation
    /// prefix, sent_in_round check+clear, error→"Error processing message"
    /// conversion + capture flush, meta.model). Deliberately NOT the bus
    /// broadcast: re-publishing on the bus would re-match workflow
    /// message-triggers (double firing). `None` in standalone mode (no drain
    /// consumer exists) and until the adapter wires it.
    reinject_tx: parking_lot::RwLock<
        Option<tokio::sync::mpsc::Sender<nemesis_types::channel::InboundMessage>>,
    >,
    /// Configured queue size for queue mode. Stored for config/logging parity
    /// but NOT read: busy-queueing lives in `crate::inbox` (capacity-bounded
    /// FIFO per session), not in the session_busy map (see
    /// `try_start_session`'s comment for why the old counter path was
    /// removed). Remove this field (+ the `new_bus` param + call sites) if
    /// queue mode is permanently retired.
    #[allow(dead_code)]
    queue_size: usize,
    /// Maximum concurrent cluster continuation tasks.
    /// 0 = inline execution in the main loop (no spawn, serialized).
    /// >0 = spawn with semaphore-controlled concurrency.
    max_continuation_permits: usize,
    /// Semaphore for limiting concurrent continuation spawns.
    /// `None` when `max_continuation_permits == 0` (inline mode).
    continuation_semaphore: Option<Arc<tokio::sync::Semaphore>>,
    /// Tracks which sessions are currently being summarized.
    /// Wrapped in `Arc` so the flag can be cleared from a spawned task
    /// after summarization completes (mirrors Go's `defer al.summarizing.Delete()`).
    summarizing: Arc<parking_lot::Mutex<HashMap<String, bool>>>,
    /// ⑩ Per-session compaction state for graded tiers (soft/summarize) and
    /// stuck self-check. See `maybe_summarize`.
    compact_state: Arc<parking_lot::Mutex<HashMap<String, CompactState>>>,
    /// Channel manager reference (for channel listing commands).
    channel_manager_channels: parking_lot::Mutex<Vec<String>>,
    /// Tracks whether a message tool already sent a response this round.
    /// Mirrors Go's MessageTool.HasSentInRound() / alreadySent check.
    sent_in_round: SentInRoundTracker,
    /// Route resolver for multi-agent message routing.
    /// Mirrors Go's al.registry (RouteResolver). When set, process_inbound_message
    /// uses the full 7-level priority cascade instead of the default-agent fallback.
    route_resolver: Option<RouteResolver>,
    /// Optional observer event callback (mirrors Go's observerMgr).
    /// Called at conversation_start, conversation_end, llm_request, llm_response, tool_call.
    observer_callback: Option<Arc<dyn Fn(&str, &serde_json::Value) + Send + Sync>>,
    /// Continuation manager for cluster RPC async callbacks.
    continuation_manager: Option<Arc<crate::loop_continuation::ContinuationManager>>,
    /// Cluster reference for cross-node communication.
    /// Stored as `Arc<dyn Any + Send + Sync>` to avoid a circular dependency
    /// on the `nemesis-cluster` crate. The caller can downcast to the concrete
    /// cluster type. Mirrors Go's `AgentLoop.cluster`.
    cluster: Option<Arc<dyn std::any::Any + Send + Sync>>,
    /// Observer manager for Phase 5 event emission.
    /// Mirrors Go's `AgentLoop.observerMgr`.
    observer_manager: Option<Arc<nemesis_observer::Manager>>,
    /// Security plugin for pre-execution tool safety checks.
    /// Mirrors Go's SecurityPlugin registered via PluginManager.
    #[cfg(feature = "security")]
    security_plugin: Option<Arc<nemesis_security::pipeline::SecurityPlugin>>,
    #[cfg(not(feature = "security"))]
    #[allow(dead_code)]
    security_plugin: Option<()>,
    /// MCP Manager for dynamic tool discovery and hot-reload.
    mcp_manager: Option<std::sync::Mutex<nemesis_mcp::manager::McpManager>>,
    /// Snapshot of registered MCP tool names and descriptions.
    /// Shared with McpListTool so it can list MCP tools without accessing the full tool registry.
    mcp_tool_snapshot: Arc<parking_lot::RwLock<Vec<(String, String)>>>,
    /// Optional data store for recording LLM usage statistics.
    data_store: Option<Arc<nemesis_data::DataStore>>,
    /// Forge instance for experience collection during tool execution.
    #[cfg(feature = "forge")]
    forge: Option<Arc<nemesis_forge::forge::Forge>>,
    #[cfg(not(feature = "forge"))]
    #[allow(dead_code)] // placeholder when forge feature is off
    forge: Option<()>,
    /// Per-session cancellation tokens. When a user requests cancellation,
    /// the token for the corresponding session is cancelled, causing the
    /// LLM loop to break at the next check point.
    cancel_tokens: dashmap::DashMap<String, tokio_util::sync::CancellationToken>,
    /// Checkpoint store for the edit safety net. When attached, every writer
    /// tool call snapshots the file's pre-edit content before execution, so a
    /// rewind can restore it. RwLock so it can be attached from `&self` (the
    /// gateway sets it after construction).
    checkpoint_store: parking_lot::RwLock<Option<Arc<crate::checkpoint::CheckpointStore>>>,
    /// Monotonic turn counter for checkpoints (one per inbound message). Global
    /// across sessions in this MVP — adequate for single-session deployments;
    /// multi-session isolation is a documented follow-up.
    turn_counter: std::sync::atomic::AtomicUsize,
    /// K1a (U14): user tool hooks — pre runs after the fixed security gate,
    /// post runs after execute and before Forge. RwLock so hooks can be
    /// registered from `&self` post-construction (K2 hooks.json wiring).
    /// See `crate::hooks` module doc for the full 布点图.
    tool_hooks: parking_lot::RwLock<crate::hooks::ToolHookManager>,
    /// K1b (U14): LLM-call-level hooks — pre may append reminder messages
    /// (visible in request_log), post may allow/replace/retry/block the
    /// response. See `crate::hooks` module doc（LLM 调用级布点）.
    llm_hooks: parking_lot::RwLock<crate::hooks::LlmHookManager>,
    /// K2 (U14): prompt/turn lifecycle hooks — on_user_prompt runs in
    /// `run_with_trace` BEFORE the message enters history (blocked prompts
    /// are never seen by the model), on_turn_end runs after the final
    /// answer is accepted, before the turn ends. Primary consumer: the CC
    /// hooks.json dialect bridge (`crate::cc_hooks`).
    lifecycle_hooks: parking_lot::RwLock<crate::hooks::LifecycleHookManager>,
    /// Memory tool executor reference, so the gateway can attach an approval
    /// gate post-construction (memory_store/forget require interactive approval).
    #[cfg(feature = "memory")]
    memory_executor:
        parking_lot::RwLock<Option<Arc<nemesis_memory::memory_tools::MemoryToolExecutor>>>,
    #[cfg(not(feature = "memory"))]
    #[allow(dead_code)] // placeholder when memory feature is off
    memory_executor: parking_lot::RwLock<Option<()>>,
    /// P3.1 (sixth batch): memory manager for the AUTO-INJECT channel —
    /// read-only retrieval (top-K vector search over the current user
    /// message) feeding the `# Memory Context` snapshot section. Deliberately
    /// SEPARATE from `memory_executor`: that one gates store/forget behind
    /// interactive approval; auto-inject is pure retrieval and must not trip
    /// the approval gate. `None` (default) disables injection entirely.
    #[cfg(feature = "memory")]
    memory_inject_manager:
        parking_lot::RwLock<Option<Arc<nemesis_memory::manager::MemoryManager>>>,
    #[cfg(not(feature = "memory"))]
    #[allow(dead_code)] // placeholder when memory feature is off
    memory_inject_manager: parking_lot::RwLock<Option<()>>,
    /// P3.1: auto_inject flag + top_k loaded from config.enhanced_memory.json
    /// by the factory (`set_memory_inject`). Tuple so both values travel
    /// together on the one setter. Default (false, 3) = feature off.
    memory_inject_cfg: parking_lot::RwLock<(bool, usize)>,
    /// Capability tier (small-model-tool-robustness plan, Phase 4a). Resolved at
    /// construction from the active model's `model_tier` config (see
    /// [`nemesis_types::capability`]). Drives tool-set size (Phase 3),
    /// validation-retry budget (Phase 2), and format-repair gating (Phase 5).
    /// `RwLock` so it can be re-resolved if the active model switches at runtime.
    tier: parking_lot::RwLock<nemesis_types::capability::ModelTier>,
    /// Path to config.json — the single source of truth for per-model
    /// `model_tier`. `None` in standalone mode (no config.json to watch). Set
    /// by `agent_factory`; used by `refresh_active_tier` / `check_config_reload`
    /// so dashboard-added models and CLI `model set-tier` are picked up live,
    /// with no stale snapshot.
    config_path: parking_lot::RwLock<Option<std::path::PathBuf>>,
    /// G4 (U4): root directory for tool-result spill files
    /// (`<home>/logs/spill`). `None` disables spilling (results fall back to
    /// the G3 prune tier). Set via `set_spill_root` by the agent factory.
    spill_root: parking_lot::RwLock<Option<std::path::PathBuf>>,
    /// H3 (P2.2): skills loader for the catalog digest. `None` disables the
    /// digest injection entirely. Set via `set_skills_loader`.
    skills_loader: parking_lot::RwLock<Option<Arc<nemesis_skills::loader::SkillsLoader>>>,
    /// H3 (P2.2): digest emission handle. Round-5: stateless under I2
    /// merged-snapshot semantics (sections re-render from disk every build;
    /// the old per-session hash map gated nothing and was removed — see
    /// skills_digest.rs module doc).
    skills_digest_state: std::sync::Arc<crate::skills_digest::DigestState>,
    /// I1 (U7): per-session message inbox for Queue/Steer modes. Unused in
    /// Reject mode (kept anyway — trivial cost, simplifies mode switching).
    inbox: crate::inbox::SharedInbox,
    /// V5 (2026-08-23): abort handles of the Queue/Steer pump's spawned turn
    /// tasks. The pump itself is aborted by the adapter on stop; without
    /// tracking, those spawned turns would be orphaned and keep publishing
    /// replies after the "stop". `stop()` aborts them (mirrors the serial
    /// pump where aborting the one task killed the in-flight turn). Reject
    /// mode never spawns — stays empty.
    turn_task_handles: parking_lot::Mutex<Vec<tokio::task::AbortHandle>>,
    /// H5 (U18): workspace root for the AGENTS.md/CLAUDE.md instruction
    /// chain. `None` disables the instructions section of the merged
    /// context digest.
    workspace_root: parking_lot::RwLock<Option<std::path::PathBuf>>,
    /// Full-review M4: context-snapshot message role ("user" default;
    /// "system" restores the pre-I2 shape for strict chat templates that
    /// reject adjacent user/user pairs).
    snapshot_role: parking_lot::RwLock<String>,
    /// X2 (U8 refinement): whether interactive approval (desktop popup
    /// adapter wired to the auditor by the gateway) is reachable. Rendered
    /// into the merged context snapshot's `# Runtime Policy` section. The
    /// guardian line reads the security plugin's live judge; the tier line
    /// reads the live capability tier — all three are state (no clocks), so
    /// the section renders deterministically: same state ⇒ same bytes.
    interactive_approval: parking_lot::RwLock<bool>,
    /// Y1 (Phase4-a): per-tool description embedding cache (tool name →
    /// (description bytes, vector)) for semantic doc folding. Entries
    /// re-embed only when a tool's description text changes, so after the
    /// first round folding adds no embed calls beyond the query itself.
    /// Read only on the memory-feature path (the embed backend lives in
    /// nemesis-memory); `allow(dead_code)` keeps the no-default-features
    /// build warning-clean.
    #[cfg_attr(not(feature = "memory"), allow(dead_code))]
    tool_vec_cache:
        parking_lot::RwLock<std::collections::HashMap<String, (String, Vec<f32>)>>,
    /// Last-seen mtime of config.json; `check_config_reload` compares against
    /// this each round to detect on-disk changes without re-reading every turn.
    config_mtime: parking_lot::RwLock<Option<std::time::SystemTime>>,
    /// 全局急停状态（kill switch）。触发后，循环在每轮顶部 break、并在工具
    /// 分发前拒绝调用。`None`（standalone/测试）时永不阻塞 = 零行为变化。
    /// 以 `Option<Arc<...>>` 形态持有，工厂每次重建 loop 时从
    /// `SharedResources.estop` 重新绑定到**同一个** Arc——所以急停状态在
    /// agent 重启后自动保持。
    estop: parking_lot::RwLock<Option<Arc<crate::estop::EstopState>>>,
}

impl AgentLoop {
    /// Switch the active model by alias (resolved via `config.models`) or literal
    /// model id. Returns the resolved model id. Unknown aliases are used as-is
    /// (so `/model deepseek-v4-pro` works even without an alias entry).
    pub fn set_active_model(&self, alias_or_model: &str) -> String {
        let model = self
            .config
            .models
            .get(alias_or_model)
            .cloned()
            .unwrap_or_else(|| alias_or_model.to_string());
        *self.active_model.write() = model.clone();
        info!(
            "[AgentLoop] Active model set to {} (via '{}')",
            model, alias_or_model
        );
        // Phase 4a: re-resolve capability tier for the new model.
        self.refresh_active_tier();
        model
    }

    /// Available model aliases (from config.models), for `/model` listing.
    pub fn model_aliases(&self) -> Vec<String> {
        self.config.models.keys().cloned().collect()
    }
    /// Create a new agent loop with the given provider and configuration (standalone mode).
    pub fn new(provider: Box<dyn LlmProvider>, config: AgentConfig) -> Self {
        let model = config.model.clone();
        info!("[AgentLoop] Created in standalone mode, model={}", model);
        Self {
            provider: parking_lot::RwLock::new(Arc::from(provider)),
            active_model: parking_lot::RwLock::new(config.model.clone()),
            tools: parking_lot::RwLock::new(HashMap::new()),
            config,
            outbound_tx: None,
            registry: None,
            state_manager: None,
            session_store: None,
            running: AtomicBool::new(false),
            session_busy: parking_lot::Mutex::new(HashMap::new()),
            concurrent_mode: ConcurrentMode::Reject,
            reinject_tx: parking_lot::RwLock::new(None),
            queue_size: crate::inbox::DEFAULT_QUEUE_SIZE,
            max_continuation_permits: 0,
            continuation_semaphore: None,
            summarizing: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            compact_state: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            channel_manager_channels: parking_lot::Mutex::new(Vec::new()),
            sent_in_round: SentInRoundTracker::new(),
            route_resolver: None,
            observer_callback: None,
            continuation_manager: None,
            cluster: None,
            observer_manager: None,
            security_plugin: None,
            mcp_manager: None,
            mcp_tool_snapshot: Arc::new(parking_lot::RwLock::new(Vec::new())),
            data_store: None,
            forge: None,
            cancel_tokens: dashmap::DashMap::new(),
            checkpoint_store: parking_lot::RwLock::new(None),
            turn_counter: std::sync::atomic::AtomicUsize::new(0),
            tool_hooks: parking_lot::RwLock::new(crate::hooks::ToolHookManager::new()),
            llm_hooks: parking_lot::RwLock::new(crate::hooks::LlmHookManager::new()),
            lifecycle_hooks: parking_lot::RwLock::new(crate::hooks::LifecycleHookManager::new()),
            memory_executor: parking_lot::RwLock::new(None),
            #[cfg(feature = "memory")]
            memory_inject_manager: parking_lot::RwLock::new(None),
            #[cfg(not(feature = "memory"))]
            memory_inject_manager: parking_lot::RwLock::new(None),
            memory_inject_cfg: parking_lot::RwLock::new((false, 3)),
            tier: parking_lot::RwLock::new(nemesis_types::capability::ModelTier::Big),
            config_path: parking_lot::RwLock::new(None),
            spill_root: parking_lot::RwLock::new(None),
            skills_loader: parking_lot::RwLock::new(None),
            skills_digest_state: std::sync::Arc::new(crate::skills_digest::DigestState::new()),
            inbox: std::sync::Arc::new(crate::inbox::Inbox::new(crate::inbox::DEFAULT_QUEUE_SIZE)),
            turn_task_handles: parking_lot::Mutex::new(Vec::new()),
            workspace_root: parking_lot::RwLock::new(None),
            snapshot_role: parking_lot::RwLock::new("user".to_string()),
            interactive_approval: parking_lot::RwLock::new(false),
            tool_vec_cache: parking_lot::RwLock::new(std::collections::HashMap::new()),
            config_mtime: parking_lot::RwLock::new(None),
            estop: parking_lot::RwLock::new(None),
        }
    }

    /// Attach a checkpoint store for the edit safety net. When set, every writer
    /// tool call (write_file/edit_file/append_file/delete_file) snapshots the
    /// file's pre-edit content before execution, so a rewind can restore it.
    pub fn set_checkpoint_store(&self, store: Arc<crate::checkpoint::CheckpointStore>) {
        *self.checkpoint_store.write() = Some(store);
    }

    /// 绑定全局急停状态。工厂每次重建 loop 都调一次，所以急停状态在 agent
    /// 重启后自动保持（状态本体在 `SharedResources` 上，不在 loop 上）。
    pub fn set_estop(&self, estop: Arc<crate::estop::EstopState>) {
        *self.estop.write() = Some(estop);
    }

    /// K1a (U14): 注册一个用户工具钩子。pre 在固定 security 闸之后、工具
    /// 执行之前运行；post 在执行之后、Forge 记录之前运行（详见
    /// `crate::hooks` 模块文档）。RwLock 注册——运行中随时可挂。
    pub fn add_tool_hook(&self, hook: Arc<dyn crate::hooks::ToolHook>) {
        self.tool_hooks.write().add(hook);
    }

    /// K1b (U14): 注册一个 LLM 调用级钩子。pre 在 messages 组装后、
    /// LlmRequest observer 事件前运行（可 Append 提醒消息 / 拦下本轮）；
    /// post 在响应错误恢复后、LlmResponse observer 事件前运行（可
    /// Allow/Replace/有限 Retry/Block）。详见 `crate::hooks` 模块文档。
    pub fn add_llm_hook(&self, hook: Arc<dyn crate::hooks::LlmHook>) {
        self.llm_hooks.write().add(hook);
    }

    /// K2 (U14): 注册一个 prompt/turn 生命周期钩子。on_user_prompt 在
    /// `run_with_trace` 顶部、消息进 history **之前**运行（拦截则模型
    /// 永远看不到该消息）；on_turn_end 在最终答案被接受后、Done 事件前
    /// 运行（可注入 feedback 要求再答一轮，预算封顶 fail-open）。
    pub fn add_lifecycle_hook(&self, hook: Arc<dyn crate::hooks::LifecycleHook>) {
        self.lifecycle_hooks.write().add(hook);
    }

    /// Wire the re-injection sender for the queue-drain path (round-5 review
    /// fix). The adapter passes a clone of the SAME mpsc sender that feeds
    /// `run_bus_arc`'s receiver, so a drained queued head re-enters the normal
    /// consumer loop and its reply gets full post-processing (rpc prefix,
    /// sent_in_round, error conversion). Call after the channel pair is
    /// created, before `run_bus_*` starts consuming.
    pub fn set_reinject_tx(
        &self,
        tx: tokio::sync::mpsc::Sender<nemesis_types::channel::InboundMessage>,
    ) {
        *self.reinject_tx.write() = Some(tx);
    }

    /// Stash the memory tool executor so the gateway can later attach an approval
    /// gate via `set_memory_approval_gate`. Called by the factory after building
    /// the shared tool config.
    #[cfg(feature = "memory")]
    pub fn set_memory_executor(&self, exec: Arc<nemesis_memory::memory_tools::MemoryToolExecutor>) {
        *self.memory_executor.write() = Some(exec);
    }

    /// P3.1 (sixth batch): wire the auto-inject channel — the memory manager
    /// (read-only retrieval) plus the `auto_inject`/`top_k` flags read from
    /// `config.enhanced_memory.json`. Passing `auto_inject=false` (the
    /// default everywhere) keeps the loop byte-identical to pre-P3.1.
    /// The `manager` param is cfg-gated: absent (and so not suppressable)
    /// under `--no-default-features` — memory injection is a no-op there.
    #[cfg(feature = "memory")]
    pub fn set_memory_inject(
        &self,
        manager: Option<Arc<nemesis_memory::manager::MemoryManager>>,
        auto_inject: bool,
        top_k: usize,
    ) {
        *self.memory_inject_manager.write() = manager;
        *self.memory_inject_cfg.write() = (auto_inject, top_k);
    }

    /// P3.1 stub (memory feature off): accepts only the flags (the manager
    /// param doesn't exist without the memory crate). Injection stays off —
    /// `memory_inject_cfg` records `(false, _)` from the real builder, but
    /// `prefetch_memory_context` returns None regardless (no manager).
    #[cfg(not(feature = "memory"))]
    pub fn set_memory_inject(&self, auto_inject: bool, top_k: usize) {
        *self.memory_inject_cfg.write() = (auto_inject, top_k);
    }

    /// Attach an approval gate to the memory executor (if one was stashed). After
    /// this, agent `memory_store`/`memory_forget` calls require approval.
    #[cfg(feature = "memory")]
    pub fn set_memory_approval_gate(
        &self,
        gate: Arc<dyn nemesis_memory::memory_tools::MemoryApprovalGate>,
    ) {
        if let Some(ref exec) = *self.memory_executor.read() {
            exec.set_approval_gate(gate);
        }
    }

    /// Rewind the workspace to the start of turn `from_turn`: restores every file
    /// changed at or after that turn to its pre-edit content (the edit safety
    /// net). Returns `(written, deleted)` paths. Errors if no checkpoint store is
    /// attached. Conversation rewinding (truncating session history) is handled
    /// by the caller — this only restores code.
    pub async fn rewind(&self, from_turn: usize) -> Result<(Vec<String>, Vec<String>), String> {
        let cp = self
            .checkpoint_store
            .read()
            .as_ref()
            .cloned()
            .ok_or("checkpoint store not attached")?;
        Ok(cp.restore_code(from_turn).await)
    }

    /// List checkpoint turns (for a rewind picker UI). Empty if no store attached.
    pub fn checkpoint_list(&self) -> Vec<crate::checkpoint::CheckpointMeta> {
        match self.checkpoint_store.read().as_ref() {
            Some(cp) => cp.list_meta(),
            None => Vec::new(),
        }
    }

    /// Create a new agent loop in bus-integrated mode.
    ///
    /// This mirrors Go's `NewAgentLoop()`. It sets up:
    /// - Agent registry with a default "main" agent
    /// - Session store for persistent history
    /// - Outbound channel for publishing responses
    /// - Session busy tracker
    /// - Route resolver with a default single-agent configuration
    pub fn new_bus(
        provider: Box<dyn LlmProvider>,
        config: AgentConfig,
        outbound_tx: tokio::sync::mpsc::Sender<nemesis_types::channel::OutboundMessage>,
        concurrent_mode: ConcurrentMode,
        queue_size: usize,
        max_continuation_permits: usize,
    ) -> Self {
        let registry = Arc::new(AgentRegistry::with_default(config.clone()));
        let session_store = Arc::new(SessionStore::new_in_memory());

        // Build a default route resolver with a single "main" agent.
        // This can be overridden via set_route_resolver() for multi-agent setups.
        let default_route_config = RouteConfig {
            bindings: Vec::new(),
            agents: vec![AgentDef {
                id: "main".to_string(),
                is_default: true,
            }],
            dm_scope: "main".to_string(),
        };

        let continuation_semaphore = if max_continuation_permits > 0 {
            Some(Arc::new(tokio::sync::Semaphore::new(
                max_continuation_permits,
            )))
        } else {
            None
        };

        let model = config.model.clone();
        info!(
            "[AgentLoop] Created in bus mode, model={}, concurrent_mode={:?}, queue_size={}, max_continuation_permits={}",
            model, concurrent_mode, queue_size, max_continuation_permits
        );

        Self {
            provider: parking_lot::RwLock::new(Arc::from(provider)),
            active_model: parking_lot::RwLock::new(config.model.clone()),
            tools: parking_lot::RwLock::new(HashMap::new()),
            config,
            outbound_tx: Some(outbound_tx),
            registry: Some(registry),
            state_manager: None,
            session_store: Some(session_store),
            running: AtomicBool::new(false),
            session_busy: parking_lot::Mutex::new(HashMap::new()),
            concurrent_mode,
            reinject_tx: parking_lot::RwLock::new(None),
            queue_size,
            max_continuation_permits,
            continuation_semaphore,
            summarizing: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            compact_state: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            channel_manager_channels: parking_lot::Mutex::new(Vec::new()),
            sent_in_round: SentInRoundTracker::new(),
            route_resolver: Some(RouteResolver::new(default_route_config)),
            observer_callback: None,
            continuation_manager: None,
            cluster: None,
            observer_manager: None,
            security_plugin: None,
            mcp_manager: None,
            mcp_tool_snapshot: Arc::new(parking_lot::RwLock::new(Vec::new())),
            data_store: None,
            forge: None,
            cancel_tokens: dashmap::DashMap::new(),
            checkpoint_store: parking_lot::RwLock::new(None),
            turn_counter: std::sync::atomic::AtomicUsize::new(0),
            tool_hooks: parking_lot::RwLock::new(crate::hooks::ToolHookManager::new()),
            llm_hooks: parking_lot::RwLock::new(crate::hooks::LlmHookManager::new()),
            lifecycle_hooks: parking_lot::RwLock::new(crate::hooks::LifecycleHookManager::new()),
            memory_executor: parking_lot::RwLock::new(None),
            #[cfg(feature = "memory")]
            memory_inject_manager: parking_lot::RwLock::new(None),
            #[cfg(not(feature = "memory"))]
            memory_inject_manager: parking_lot::RwLock::new(None),
            memory_inject_cfg: parking_lot::RwLock::new((false, 3)),
            tier: parking_lot::RwLock::new(nemesis_types::capability::ModelTier::Big),
            config_path: parking_lot::RwLock::new(None),
            spill_root: parking_lot::RwLock::new(None),
            skills_loader: parking_lot::RwLock::new(None),
            skills_digest_state: std::sync::Arc::new(crate::skills_digest::DigestState::new()),
            inbox: std::sync::Arc::new(crate::inbox::Inbox::new(queue_size.max(1))),
            turn_task_handles: parking_lot::Mutex::new(Vec::new()),
            workspace_root: parking_lot::RwLock::new(None),
            snapshot_role: parking_lot::RwLock::new("user".to_string()),
            interactive_approval: parking_lot::RwLock::new(false),
            tool_vec_cache: parking_lot::RwLock::new(std::collections::HashMap::new()),
            config_mtime: parking_lot::RwLock::new(None),
            estop: parking_lot::RwLock::new(None),
        }
    }

    // -----------------------------------------------------------------------
    // Continuation dispatch
    // -----------------------------------------------------------------------

    /// Dispatch a cluster continuation: inline (permits=0) or spawned (permits>0).
    /// Called from both `run_bus_owned` (test) and `run_bus_arc` (production).
    async fn dispatch_continuation(
        &self,
        task_id: String,
        msg: &nemesis_types::channel::InboundMessage,
    ) {
        let task_response = msg.content.clone();
        let task_metadata = msg.metadata.clone();
        let task_failed = task_metadata
            .get("status")
            .map(|s| s == "error")
            .unwrap_or(false);

        if self.max_continuation_permits == 0 {
            // Inline: process directly in the main loop (no spawn).
            // The main loop is blocked until continuation completes,
            // ensuring serialized execution with no resource contention.
            let task_error = task_metadata.get("error").map(|s| s.as_str());
            if let Some(ref mgr) = self.continuation_manager {
                if let Some(ref tx) = self.outbound_tx {
                    // Clone data from RwLock guards before .await — guards are !Send
                    // and cannot be held across yield points in an async fn.
                    let provider = self.provider.read().clone();
                    let model = self.active_model.read().clone();
                    let tools = self.tools.read().clone();

                    crate::loop_continuation::handle_cluster_continuation(
                        mgr.as_ref(),
                        &task_id,
                        &task_response,
                        task_failed,
                        task_error,
                        provider.as_ref(),
                        &model,
                        &tools,
                        tx,
                        self.observer_manager.clone(),
                        self.session_store.as_ref().map(|v| v.as_ref()),
                    )
                    .await;
                }
            }
        } else {
            // Spawn with semaphore-controlled concurrency.
            let task_error = task_metadata.get("error").cloned();
            let provider = self.provider.read().clone();
            let model = self.active_model.read().clone();
            let tools = self.tools.read().clone();
            let outbound_tx = self.outbound_tx.clone();
            let continuation_manager = self.continuation_manager.clone();
            let observer_manager = self.observer_manager.clone();
            let session_store = self.session_store.clone();
            let semaphore = self.continuation_semaphore.clone().unwrap();

            tokio::spawn(async move {
                let _permit = semaphore.acquire().await.unwrap();
                if let Some(ref mgr) = continuation_manager {
                    if let Some(ref tx) = outbound_tx {
                        crate::loop_continuation::handle_cluster_continuation(
                            mgr.as_ref(),
                            &task_id,
                            &task_response,
                            task_failed,
                            task_error.as_deref(),
                            provider.as_ref(),
                            &model,
                            &tools,
                            tx,
                            observer_manager,
                            session_store.as_ref().map(|v| v.as_ref()),
                        )
                        .await;
                    }
                }
            });
        }
    }

    // -----------------------------------------------------------------------
    // Registration methods
    // -----------------------------------------------------------------------

    /// Register a tool with the agent loop (standalone mode).
    pub fn register_tool(&mut self, name: String, tool: Box<dyn Tool>) {
        debug!("[AgentLoop] Registered tool: {}", name);
        self.tools.write().insert(name, Arc::from(tool));
    }

    /// Register a tool across all agents in the registry (bus mode).
    /// Mirrors Go's `AgentLoop.RegisterTool()`.
    pub fn register_tool_shared(&mut self, name: String, tool: Box<dyn Tool>) {
        debug!("[AgentLoop] Registered shared tool: {}", name);
        self.tools.write().insert(name, Arc::from(tool));
    }

    // [ClusterService-Full] 完整方案预留：动态移除工具
    // 当前未启用，原因：避免影响 LLM 提示词缓存命中率
    // 启用条件：当 LLM 提供商支持按工具分组缓存或工具定义独立缓存时
    /// Remove a tool by name from the registry.
    /// Returns true if the tool was found and removed.
    pub fn remove_tool_shared(&mut self, name: &str) -> bool {
        if self.tools.write().remove(name).is_some() {
            debug!("[AgentLoop] Removed shared tool: {}", name);
            true
        } else {
            debug!("[AgentLoop] Tool '{}' not found, nothing to remove", name);
            false
        }
    }

    /// Return the number of registered tools.
    pub fn tool_count(&self) -> usize {
        self.tools.read().len()
    }

    /// Return the names of all registered tools.
    pub fn tool_names(&self) -> Vec<String> {
        self.tools.read().keys().cloned().collect()
    }

    /// Enable automatic MCP tool reload via mtime-based change detection.
    ///
    /// Creates an `McpManager` for the given config path, discovers tools from
    /// all currently configured servers, and registers them. On each LLM round,
    /// the manager checks if the config file changed and loads new servers.
    pub fn enable_mcp_reload(&mut self, config_path: std::path::PathBuf) {
        let mgr = nemesis_mcp::manager::McpManager::new(config_path);
        if mgr.is_enabled() {
            for server in mgr.list_servers().to_vec() {
                let server_name = server.name.clone();
                match tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(mgr.discover_tools(&server))
                }) {
                    Ok(tools) => {
                        let count = tools.len();
                        for tool in tools {
                            let def = tool.definition();
                            let name = def.name.clone();
                            self.register_tool(
                                name,
                                Box::new(crate::mcp_bridge::McpToolBridge::new(tool)),
                            );
                        }
                        info!(
                            "[AgentLoop] MCP: registered {} tools from '{}'",
                            count, server_name
                        );
                    }
                    Err(e) => {
                        warn!(
                            "[AgentLoop] MCP: server '{}' discovery failed: {}",
                            server_name, e
                        );
                    }
                }
            }
            self.mcp_manager = Some(std::sync::Mutex::new(mgr));
            info!("[AgentLoop] MCP dynamic reload enabled (mtime-based)");
        } else {
            // Store manager even when disabled so we can detect future enable via config change
            self.mcp_manager = Some(std::sync::Mutex::new(mgr));
            info!("[AgentLoop] MCP config disabled; reload watcher active for future changes");
        }
        self.refresh_mcp_snapshot();
    }

    /// Check MCP config for changes and register tools from new servers.
    /// Uses interior mutability since the run loop borrows `&self`.
    fn check_mcp_reload(&self) {
        let mgr = match self.mcp_manager.as_ref() {
            Some(m) => m,
            None => return,
        };

        let changed = {
            match mgr.lock() {
                Ok(mut m) => m.check_config_changed(),
                Err(_) => return,
            }
        };

        if !changed {
            return;
        }

        // Collect existing MCP tool prefixes to detect what's new
        let registered: Vec<String> = self
            .tools
            .read()
            .keys()
            .filter(|k| k.starts_with("mcp_"))
            .map(|k| {
                // "mcp_<srv>_<tool>" → "mcp_<srv>_"
                let chars: Vec<char> = k.chars().collect();
                let underscores: Vec<usize> = chars
                    .iter()
                    .enumerate()
                    .filter(|&(_, &c)| c == '_')
                    .map(|(i, _)| i)
                    .collect();
                if underscores.len() >= 2 {
                    k[..underscores[2]].to_string()
                } else {
                    k.clone()
                }
            })
            .collect();

        let new_servers: Vec<_> = {
            match mgr.lock() {
                Ok(m) => m
                    .find_new_servers(&registered)
                    .into_iter()
                    .cloned()
                    .collect(),
                Err(_) => return,
            }
        };

        for server in new_servers {
            let server_name = server.name.clone();
            let tools = match mgr.lock() {
                Ok(m) => tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(m.discover_tools(&server))
                }),
                Err(_) => continue,
            };

            match tools {
                Ok(tools) => {
                    let count = tools.len();
                    for tool in tools {
                        let name = tool.definition().name.clone();
                        // tools is behind Arc, need interior mutability for self.tools
                        // Use the atomic swap pattern via tools_mut
                        self.tools.write().insert(
                            name,
                            Arc::from(Box::new(crate::mcp_bridge::McpToolBridge::new(tool))
                                as Box<dyn Tool>),
                        );
                    }
                    info!(
                        "[AgentLoop] MCP reload: registered {} tools from '{}'",
                        count, server_name
                    );
                }
                Err(e) => {
                    warn!(
                        "[AgentLoop] MCP reload: server '{}' failed: {}",
                        server_name, e
                    );
                }
            }
        }
        self.refresh_mcp_snapshot();
    }

    /// Refresh the MCP tool snapshot from the tool registry.
    fn refresh_mcp_snapshot(&self) {
        let snapshot: Vec<(String, String)> = self
            .tools
            .read()
            .iter()
            .filter(|(name, _)| name.starts_with("mcp_"))
            .map(|(name, tool)| (name.clone(), tool.description()))
            .collect();
        *self.mcp_tool_snapshot.write() = snapshot;
    }

    /// Return a shared reference to the MCP tool snapshot.
    /// Used to wire up McpListTool.
    pub fn mcp_tool_snapshot(&self) -> Arc<parking_lot::RwLock<Vec<(String, String)>>> {
        self.mcp_tool_snapshot.clone()
    }

    /// Set the channel manager reference for listing enabled channels.
    /// Mirrors Go's `SetChannelManager()`.
    pub fn set_channel_manager(&self, enabled_channels: Vec<String>) {
        *self.channel_manager_channels.lock() = enabled_channels;
    }

    /// Set the state manager for recording last channel/chat ID.
    /// Mirrors Go's `state.NewManager(workspace)`.
    pub fn set_state_manager(
        &mut self,
        mgr: Arc<nemesis_state::workspace_state::WorkspaceStateManager>,
    ) {
        self.state_manager = Some(mgr);
        debug!("[AgentLoop] State manager configured");
    }

    /// Set the observer callback for event emission.
    /// Mirrors Go's `SetObserverManager()`.
    pub fn set_observer_callback(
        &mut self,
        cb: Arc<dyn Fn(&str, &serde_json::Value) + Send + Sync>,
    ) {
        self.observer_callback = Some(cb);
        debug!("[AgentLoop] Observer callback configured");
    }

    /// Set the route resolver for multi-agent message routing.
    /// Mirrors Go's `AgentLoop.registry` (RouteResolver).
    /// When set, `process_inbound_message` uses the full 7-level priority
    /// cascade to determine agent and session key.
    pub fn set_route_resolver(&mut self, resolver: RouteResolver) {
        self.route_resolver = Some(resolver);
        info!("[AgentLoop] Route resolver configured");
    }

    /// Set the cluster reference.
    ///
    /// Accepts an `Arc<dyn Any + Send + Sync>` to avoid a compile-time dependency
    /// on the `nemesis-cluster` crate. The concrete cluster instance should be
    /// wrapped with `Arc::new(cluster) as Arc<dyn Any + Send + Sync>`.
    /// Mirrors Go's `AgentLoop.cluster` field assignment.
    pub fn set_cluster(&mut self, cluster: Arc<dyn std::any::Any + Send + Sync>) {
        self.cluster = Some(cluster);
    }

    /// Get the cluster reference, if set.
    ///
    /// Returns `Option<&Arc<dyn Any + Send + Sync>>`. The caller is responsible
    /// for downcasting to the concrete cluster type. Mirrors Go's `GetCluster()`.
    pub fn get_cluster(&self) -> Option<&Arc<dyn std::any::Any + Send + Sync>> {
        self.cluster.as_ref()
    }

    /// Set the observer manager for Phase 5 event emission.
    /// Mirrors Go's `SetObserverManager()`.
    pub fn set_observer_manager(&mut self, mgr: Arc<nemesis_observer::Manager>) {
        self.observer_manager = Some(mgr);
    }

    /// Set the security plugin for pre-execution tool safety checks.
    /// Mirrors Go's SecurityPlugin registered via PluginManager.
    #[cfg(feature = "security")]
    pub fn set_security_plugin(&mut self, plugin: Arc<nemesis_security::pipeline::SecurityPlugin>) {
        self.security_plugin = Some(plugin);
    }

    /// Set the session store, replacing the default in-memory store.
    /// Call this to enable disk-persisted conversation history.
    pub fn set_session_store(&mut self, store: Arc<crate::session::SessionStore>) {
        self.session_store = Some(store);
    }

    /// Get the session store, if one is configured.
    ///
    /// Used by callers outside the main agent loop (e.g. cluster_agent) that need
    /// to read/write history via the same SessionStore the loop would use.
    pub fn session_store(&self) -> Option<&Arc<crate::session::SessionStore>> {
        self.session_store.as_ref()
    }

    /// Set the continuation manager for async cluster RPC callbacks.
    ///
    /// When set, `cluster_continuation` messages intercepted by the bus loop
    /// will trigger snapshot loading and LLM resumption.
    pub fn set_continuation_manager(
        &mut self,
        manager: Arc<crate::loop_continuation::ContinuationManager>,
    ) {
        self.continuation_manager = Some(manager);
    }

    /// Set the data store for recording LLM usage statistics.
    pub fn set_data_store(&mut self, store: Arc<nemesis_data::DataStore>) {
        self.data_store = Some(store);
    }

    /// Set the Forge instance for experience collection.
    #[cfg(feature = "forge")]
    pub fn set_forge(&mut self, forge: Arc<nemesis_forge::forge::Forge>) {
        self.forge = Some(forge);
    }

    /// Swap the LLM provider and model at runtime. Takes effect immediately
    /// for the next LLM call. In-flight requests continue with the old provider.
    pub fn set_provider_and_model(&self, provider: Arc<dyn LlmProvider>, model: String) {
        *self.provider.write() = provider;
        *self.active_model.write() = model;
        tracing::info!("[AgentLoop] Provider swapped at runtime");
        // Phase 4a: re-resolve capability tier for the new model.
        self.refresh_active_tier();
    }

    /// Get the observer manager, if set.
    /// Mirrors Go's `GetObserverManager()`.
    pub fn get_observer_manager(&self) -> Option<&Arc<nemesis_observer::Manager>> {
        self.observer_manager.as_ref()
    }

    /// Get the agent registry (bus mode).
    pub fn get_registry(&self) -> Option<&Arc<AgentRegistry>> {
        self.registry.as_ref()
    }

    /// Get a clone of the provider Arc.
    pub fn provider_arc(&self) -> Arc<dyn LlmProvider> {
        self.provider.read().clone()
    }

    /// Get a mutable reference to the agent config.
    pub fn config_mut(&mut self) -> &mut AgentConfig {
        &mut self.config
    }

    // -----------------------------------------------------------------------
    // Bus-integrated main loop
    // -----------------------------------------------------------------------

    /// Run the main bus consumption loop (takes ownership of the receiver).
    ///
    /// This is the preferred entry point for bus-integrated mode.
    /// Mirrors Go's `AgentLoop.Run(ctx)`. Continuously consumes inbound
    /// messages, processes them, and publishes outbound responses.
    /// Stops when `stop()` is called or the inbound channel closes.
    ///
    /// Test-only variant; production code uses `run_bus_arc`.
    /// Post-turn finish (V5: extracted verbatim from the original run_bus
    /// bodies so the serial pump and spawned turn tasks share one tail):
    /// error funnel + capture flush, sent-in-round check, RPC correlation
    /// prefix, outbound publish.
    ///
    /// `check_sent_in_round`: Reject (serial — no turn can overlap) keeps the
    /// historical check+clear. Queue/Steer Immediate replies (busy receipts,
    /// slash responses) pass false: they can run while the session's turn is
    /// in flight, and touching that turn's sent-in-round flag mid-flight
    /// would corrupt its end-of-turn publish decision.
    async fn finish_message(
        &self,
        msg: &nemesis_types::channel::InboundMessage,
        response: String,
        err: Option<String>,
        check_sent_in_round: bool,
    ) {
        let response = match err {
            Some(e) => {
                // [capture] Agent error funnel: the full error
                // becomes the user-visible response. Flush the
                // session's captured evidence + the complete error
                // text (the user sees a short "Error: ..."; this
                // keeps the full source string for root-causing).
                if let Some(sink) = crate::capture_sink::CaptureSink::global() {
                    sink.flush(&msg.session_key, "agent_error", None, Some(e.as_str()));
                }
                format!("Error processing message: {}", e)
            }
            None => response,
        };

        if response.is_empty() {
            return;
        }

        if check_sent_in_round {
            // Check if a tool (e.g., MessageTool) already sent a response for this
            // session in the current round. Mirrors Go's alreadySent check.
            let already_sent = self.sent_in_round.has_sent_in_round(&msg.session_key);
            // Only clear this session's flag, not all sessions.
            // Go clears per-tool-instance state, so clearing only the current
            // session preserves other sessions' sent-in-round tracking.
            self.sent_in_round.clear(&msg.session_key);

            if already_sent {
                debug!(
                    "[AgentLoop] Skipping outbound publish: message tool already sent response for session {}",
                    msg.session_key
                );
                return;
            }
        }

        if let Some(ref tx) = self.outbound_tx {
            // For RPC channel, add correlation ID prefix if not already present.
            let final_content = if msg.channel == "rpc"
                && !msg.correlation_id.is_empty()
                && !response.starts_with(&format!("[rpc:{}]", msg.correlation_id))
            {
                format!("[rpc:{}] {}", msg.correlation_id, response)
            } else {
                response
            };

            info!(
                "[AgentLoop] Response message     to {}:{}: {}",
                msg.channel,
                msg.chat_id,
                truncate(&final_content, 80)
            );

            let outbound = nemesis_types::channel::OutboundMessage {
                channel: msg.channel.clone(),
                chat_id: msg.chat_id.clone(),
                content: final_content,
                message_type: String::new(),
                meta: nemesis_types::channel::OutboundMeta {
                    model: Some(self.current_display_model()),
                },
            };
            if let Err(e) = tx.send(outbound).await {
                warn!("[AgentLoop] Failed to send outbound message: {}", e);
            }
        }
    }

    /// Spawn a turn task (Queue/Steer modes) and track its abort handle so
    /// `stop()` can cancel in-flight turns. Finished handles are pruned on
    /// each insert, keeping the vec bounded by live turns.
    fn spawn_turn_task<F>(&self, fut: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let handle = tokio::spawn(fut);
        let mut handles = self.turn_task_handles.lock();
        handles.retain(|h| !h.is_finished());
        handles.push(handle.abort_handle());
    }

    /// Shared bus pump (V5): mode-dependent dispatch.
    ///
    /// Reject (default): serial — byte-identical to the historical loop;
    /// each turn completes before the next message is processed.
    ///
    /// Queue/Steer: the synchronous gate (`gate_inbound`) runs inline in the
    /// pump (routing + busy check + inbox parking + receipts), then each
    /// admitted turn runs as a tracked spawned task so a long turn cannot
    /// starve the gate — this is what makes the U7 inbox reachable from
    /// production channels. Same-session ordering holds because the gate
    /// acquires the session BEFORE spawning; cross-session turns run
    /// concurrently. Continuations stay inline (serialized with the pump).
    async fn run_bus_impl(
        self: Arc<Self>,
        mut inbound_rx: tokio::sync::mpsc::Receiver<nemesis_types::channel::InboundMessage>,
    ) {
        self.running.store(true, Ordering::Release);
        info!("[AgentLoop] Bus consumption loop started");

        while self.running.load(Ordering::Acquire) {
            match inbound_rx.recv().await {
                Some(msg) => match self.concurrent_mode {
                    ConcurrentMode::Reject => {
                        let (agent_id, response, err) = self.process_inbound_message(&msg).await;

                        // Check for cluster continuation marker.
                        if agent_id == "__continuation__" {
                            let task_id = response;
                            info!(
                                "[AgentLoop] Handling cluster continuation for task {} (permits={})",
                                task_id, self.max_continuation_permits
                            );
                            self.dispatch_continuation(task_id, &msg).await;
                            continue;
                        }

                        self.finish_message(&msg, response, err, true).await;
                    }
                    ConcurrentMode::Queue | ConcurrentMode::Steer => match self.gate_inbound(&msg) {
                        GateOutcome::Continuation(task_id) => {
                            info!(
                                "[AgentLoop] Handling cluster continuation for task {} (permits={})",
                                task_id, self.max_continuation_permits
                            );
                            self.dispatch_continuation(task_id, &msg).await;
                        }
                        GateOutcome::Immediate {
                            agent_id: _,
                            response,
                        } => {
                            // Busy receipt / slash reply — publish inline.
                            // Never touches sent_in_round (may overlap the
                            // session's running turn; see finish_message).
                            self.finish_message(&msg, response, None, false).await;
                        }
                        GateOutcome::Ungated => {
                            let this = self.clone();
                            let m = msg.clone();
                            self.spawn_turn_task(async move {
                                let (_, response, err) = this.process_ungated(&m).await;
                                this.finish_message(&m, response, err, true).await;
                            });
                        }
                        GateOutcome::Admitted(admission) => {
                            let this = self.clone();
                            let m = msg.clone();
                            self.spawn_turn_task(async move {
                                let (_, response, err) =
                                    this.process_admitted(&m, admission).await;
                                this.finish_message(&m, response, err, true).await;
                            });
                        }
                    },
                },
                None => {
                    // Channel closed.
                    break;
                }
            }
        }

        info!("[AgentLoop] Bus consumption loop stopped");
        self.running.store(false, Ordering::Release);
    }

    /// Run the main bus consumption loop (takes ownership of the receiver).
    ///
    /// This is the preferred entry point for bus-integrated mode.
    /// Mirrors Go's `AgentLoop.Run(ctx)`. Continuously consumes inbound
    /// messages, processes them, and publishes outbound responses.
    /// Stops when `stop()` is called or the inbound channel closes.
    ///
    /// Test-only variant; production code uses `run_bus_arc`.
    #[cfg(test)]
    pub async fn run_bus_owned(
        self,
        inbound_rx: tokio::sync::mpsc::Receiver<nemesis_types::channel::InboundMessage>,
    ) {
        std::sync::Arc::new(self).run_bus_impl(inbound_rx).await;
    }

    /// Same as `run_bus_owned` but takes `Arc<Self>` so the AgentLoop can be
    /// shared with other components (e.g. heartbeat handler) while the bus
    /// loop is running.
    pub async fn run_bus_arc(
        self: Arc<Self>,
        inbound_rx: tokio::sync::mpsc::Receiver<nemesis_types::channel::InboundMessage>,
    ) {
        self.run_bus_impl(inbound_rx).await;
    }

    /// Stop the bus consumption loop.
    /// Mirrors Go's `AgentLoop.Stop()`.
    pub fn stop(&self) {
        info!("[AgentLoop] Stop requested");
        self.running.store(false, Ordering::Release);
        // V5 (2026-08-23): kill Queue/Steer mode's spawned turn tasks. The
        // adapter aborts the pump task itself; without this the spawned turns
        // would be orphaned (holding Arc clones) and keep running/publishing
        // after the stop.
        let mut handles = self.turn_task_handles.lock();
        for h in handles.drain(..) {
            h.abort();
        }
    }

    /// Check whether the loop is currently running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    /// Clear all session busy states.
    ///
    /// Called after a forced stop (task abort) to release sessions that were
    /// mid-processing when the agent was killed.  Without this, those sessions
    /// remain permanently locked ("busy") and all subsequent messages for them
    /// are rejected.
    pub fn clear_session_busy(&self) {
        let mut map = self.session_busy.lock();
        let count = map.len();
        map.clear();
        if count > 0 {
            tracing::warn!(
                "[AgentLoop] Cleared {} session busy states (agent was stopped mid-processing)",
                count
            );
        }
    }

    // -----------------------------------------------------------------------
    // Observer event emission helpers
    // -----------------------------------------------------------------------

    /// Emit an observer event synchronously (for conversation start/end).
    ///
    /// Forwards to both the Phase 5 observer manager and the legacy
    /// `observer_callback`.
    async fn emit_observer_sync(&self, event: crate::loop_executor::ObserverEvent) {
        if let Some(ref mgr) = self.observer_manager {
            let conv_event = event.to_conversation_event();
            mgr.emit_sync(conv_event).await;
        }
        if let Some(ref cb) = self.observer_callback {
            let (event_type, data) = event.to_callback_json();
            cb(event_type, &data);
        }
    }

    // -----------------------------------------------------------------------
    // Cluster continuation handling
    // -----------------------------------------------------------------------

    /// Handle a cluster continuation by loading the snapshot, resuming the LLM
    /// loop, and sending the final response.
    ///
    /// NOTE: The main run_bus_owned loop calls the free function
    /// `crate::loop_continuation::handle_cluster_continuation` directly instead
    /// of this method. Similarly, maybe_summarize calls the standalone
    /// `summarize_history_owned` / `summarize_multipart_owned` / `summarize_batch_owned`
    /// free functions. These self methods are kept as reference implementations
    /// matching the Go AgentLoop method signatures.
    #[allow(dead_code)]
    async fn handle_cluster_continuation(
        &self,
        task_id: &str,
        original_msg: &nemesis_types::channel::InboundMessage,
    ) {
        if let Some(ref mgr) = self.continuation_manager {
            let task_response = &original_msg.content;
            let task_failed = original_msg
                .metadata
                .get("status")
                .map(|s| s == "error")
                .unwrap_or(false);
            let task_error = original_msg.metadata.get("error").map(|s| s.as_str());

            // Clone provider and model before .await (RwLock guards are not Send).
            let cont_provider = self.provider.read().clone();
            let cont_model = self.active_model.read().clone();
            if let Some(ref tx) = self.outbound_tx {
                crate::loop_continuation::handle_cluster_continuation(
                    mgr.as_ref(),
                    task_id,
                    task_response,
                    task_failed,
                    task_error,
                    cont_provider.as_ref(),
                    &cont_model,
                    &self.tools,
                    tx,
                    self.observer_manager.clone(),
                    self.session_store.as_ref().map(|v| v.as_ref()),
                )
                .await;
            }
        } else {
            warn!(
                "[AgentLoop] No continuation manager configured, cannot handle continuation for task_id={}",
                task_id
            );
        }
    }

    // -----------------------------------------------------------------------
    // Direct processing (bypass bus)
    // -----------------------------------------------------------------------

    /// Process a direct message without the bus.
    /// Mirrors Go's `ProcessDirect()`.
    pub async fn process_direct(&self, content: &str, session_key: &str) -> Result<String, String> {
        self.process_direct_with_channel(content, session_key, "cli", "direct")
            .await
    }

    /// Process a direct message with explicit channel/chat ID.
    /// Mirrors Go's `ProcessDirectWithChannel()`.
    pub async fn process_direct_with_channel(
        &self,
        content: &str,
        session_key: &str,
        channel: &str,
        chat_id: &str,
    ) -> Result<String, String> {
        let trace_id = format!(
            "direct-{}-{}",
            session_key,
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let start_time = std::time::Instant::now();

        // Emit conversation_start observer event.
        self.emit_observer_sync(crate::loop_executor::ObserverEvent::ConversationStart {
            trace_id: trace_id.clone(),
            session_key: session_key.to_string(),
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
            sender_id: "direct".to_string(),
            content: content.to_string(),
        })
        .await;

        let instance = self.get_or_create_instance(session_key);
        let context = RequestContext::new(channel, chat_id, "cron", session_key);

        let token = tokio_util::sync::CancellationToken::new();
        let events = self
            .run_with_trace(&instance, content, &context, &trace_id, false, &token, None)
            .await;

        // Extract final response for the conversation end event.
        let final_response = events
            .iter()
            .rev()
            .find_map(|e| {
                if let AgentEvent::Done(msg) = e {
                    Some(msg.clone())
                } else {
                    None
                }
            })
            .unwrap_or_default();

        // Emit conversation_end observer event.
        let duration_ms = start_time.elapsed().as_millis() as u64;
        let rounds = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::ToolCall(_)))
            .count() as u32
            + 1;
        self.emit_observer_sync(crate::loop_executor::ObserverEvent::ConversationEnd {
            trace_id: trace_id.clone(),
            session_key: session_key.to_string(),
            total_rounds: rounds,
            duration_ms,
            content: final_response,
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
        })
        .await;

        // Extract final response from events.
        for event in events.iter().rev() {
            if let AgentEvent::Done(msg) = event {
                return Ok(msg.clone());
            }
        }
        for event in events.iter().rev() {
            if let AgentEvent::Error(msg) = event {
                return Err(msg.clone());
            }
        }
        Ok(String::new())
    }

    /// Process a heartbeat request without session history.
    /// Each heartbeat is independent and doesn't accumulate context.
    /// Mirrors Go's `ProcessHeartbeat()`.
    pub async fn process_heartbeat(
        &self,
        content: &str,
        channel: &str,
        chat_id: &str,
    ) -> Result<String, String> {
        let trace_id = format!(
            "heartbeat-{}-{}",
            chat_id,
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let start_time = std::time::Instant::now();

        // Emit conversation_start observer event.
        self.emit_observer_sync(crate::loop_executor::ObserverEvent::ConversationStart {
            trace_id: trace_id.clone(),
            session_key: "heartbeat".to_string(),
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
            sender_id: "heartbeat".to_string(),
            content: content.to_string(),
        })
        .await;

        // Heartbeat uses a fresh temporary instance, no history.
        let config = AgentConfig {
            model: self.active_model.read().clone(),
            system_prompt: self.config.system_prompt.clone(),
            max_turns: self.config.max_turns,
            tools: self.config.tools.clone(),
            models: self.config.models.clone(),
        };
        let instance = AgentInstance::new(config);
        let context = RequestContext::new(channel, chat_id, "heartbeat", "heartbeat");

        let token = tokio_util::sync::CancellationToken::new();
        let events = self
            .run_with_trace(&instance, content, &context, &trace_id, false, &token, None)
            .await;

        // Extract final response for the conversation end event.
        let final_response = events
            .iter()
            .rev()
            .find_map(|e| {
                if let AgentEvent::Done(msg) = e {
                    Some(msg.clone())
                } else {
                    None
                }
            })
            .unwrap_or_default();

        // Emit conversation_end observer event.
        let duration_ms = start_time.elapsed().as_millis() as u64;
        let rounds = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::ToolCall(_)))
            .count() as u32
            + 1;
        self.emit_observer_sync(crate::loop_executor::ObserverEvent::ConversationEnd {
            trace_id: trace_id.clone(),
            session_key: "heartbeat".to_string(),
            total_rounds: rounds,
            duration_ms,
            content: final_response,
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
        })
        .await;

        for event in events.iter().rev() {
            if let AgentEvent::Done(msg) = event {
                return Ok(msg.clone());
            }
        }
        Ok("I've completed processing but have no response to give.".to_string())
    }

    // -----------------------------------------------------------------------
    // Inbound message processing (bus mode)
    // -----------------------------------------------------------------------

    /// Process an inbound message from the bus.
    ///
    /// Returns (agent_id, response_content, optional_error).
    /// Mirrors Go's `processMessage()`.
    async fn process_inbound_message(
        &self,
        msg: &nemesis_types::channel::InboundMessage,
    ) -> (String, String, Option<String>) {
        // V5 (2026-08-23): gate first (sync classification + session
        // acquire), then the matching tail. Reject mode's serial pump and
        // all direct callers (heartbeat, tests, inline queue-drain fallback)
        // still enter here — behavior identical to the pre-split monolith.
        match self.gate_inbound(msg) {
            GateOutcome::Continuation(task_id) => {
                ("__continuation__".to_string(), task_id, None)
            }
            GateOutcome::Immediate { agent_id, response } => (agent_id, response, None),
            GateOutcome::Ungated => self.process_ungated(msg).await,
            GateOutcome::Admitted(admission) => self.process_admitted(msg, admission).await,
        }
    }

    /// Turn preamble shared by every gated path (V5: extracted verbatim from
    /// the head of the pre-split `process_inbound_message`, so gate-time
    /// short-circuits — busy receipts, slash replies — behave exactly as
    /// before: the monolith opened the checkpoint turn and logged BEFORE any
    /// classification).
    fn turn_preamble(&self, msg: &nemesis_types::channel::InboundMessage) {
        // Open a checkpoint turn for the edit safety net (so writer-tool changes
        // during this message can be rewound). No-op when no store is attached.
        let cp_turn = self
            .turn_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        {
            let cp = self.checkpoint_store.read().as_ref().cloned();
            if let Some(cp) = cp {
                cp.begin(cp_turn, &msg.content);
            }
        }

        info!(
            "[AgentLoop] Processing message from {}:{}: {}",
            msg.channel,
            msg.sender_id,
            truncate(&msg.content, 80)
        );
    }

    /// Resolve (agent_id, session_key) for a message (V5: extracted verbatim
    /// from the routing block of the pre-split `process_inbound_message`).
    fn route_message(
        &self,
        msg: &nemesis_types::channel::InboundMessage,
    ) -> (String, String) {
        // Resolve agent and session via route resolver.
        // Mirrors Go's processMessage: al.registry.ResolveRoute(RouteInput{...})
        let (agent_id, session_key) = if let Some(ref resolver) = self.route_resolver {
            // Build the routing input from message metadata, matching Go's extractPeer/extractParentPeer.
            let peer_kind = msg.metadata.get("peer_kind").cloned();
            let peer_id = msg.metadata.get("peer_id").cloned().or_else(|| {
                // Fallback: if peer_kind is "direct" use sender_id, else use chat_id
                if let Some(kind) = &peer_kind {
                    if kind == "direct" {
                        Some(msg.sender_id.clone())
                    } else {
                        Some(msg.chat_id.clone())
                    }
                } else {
                    None
                }
            });
            let parent_peer_kind = msg.metadata.get("parent_peer_kind").cloned();
            let parent_peer_id = msg.metadata.get("parent_peer_id").cloned();

            let route_input = RoutingRouteInput {
                channel: msg.channel.clone(),
                account_id: msg.metadata.get("account_id").cloned().unwrap_or_default(),
                peer_kind,
                peer_id,
                parent_peer_kind,
                parent_peer_id,
                guild_id: msg.metadata.get("guild_id").cloned(),
                team_id: msg.metadata.get("team_id").cloned(),
                identity_links: std::collections::HashMap::new(),
            };
            let route = resolver.resolve(&route_input);

            // Use routed session key, but honor pre-set agent-scoped keys
            // (mirrors Go's logic for ProcessDirect/cron).
            let session_key =
                if !msg.session_key.is_empty() && msg.session_key.starts_with("agent:") {
                    msg.session_key.clone()
                } else {
                    route.session_key.clone()
                };

            info!(
                "[AgentLoop] Routed message: agent_id={}, session_key={}, matched_by={}",
                route.agent_id, session_key, route.matched_by
            );

            (route.agent_id, session_key)
        } else {
            // Fallback when no route resolver is configured (standalone mode).
            let agent_id = self
                .registry
                .as_ref()
                .and_then(|r| r.default_agent_id())
                .unwrap_or_else(|| "main".to_string());

            let peer = extract_peer(msg);
            let session_key =
                if !msg.session_key.is_empty() && msg.session_key.starts_with("agent:") {
                    msg.session_key.clone()
                } else {
                    format!("{}:{}", msg.channel, peer)
                };

            info!(
                "[AgentLoop] Routed message (no resolver): agent_id={}, session_key={}",
                agent_id, session_key
            );

            (agent_id, session_key)
        };
        (agent_id, session_key)
    }

    /// Synchronous inbound gate (V5 / B5 真机揭的接线 bug，2026-08-23):
    /// 生产 pump（`run_bus_*`）原是纯串行消费者——`process_inbound_message`
    /// 把整个回合 await 到底才 recv 下一条消息，busy 闸门对用户消息永远
    /// 不可达（U7 inbox 因此在真机上从未生效，只有单测手动占住 session
    /// 验证过）。Queue/Steer 模式下 pump 现在同步跑这个闸门（同 session
    /// 保序：忙时的第二条消息必见 busy → 入队/回执），回合在独立 task
    /// 里并发跑；Reject 模式维持原串行路径（行为零变化）。
    ///
    /// 闸门只做同步分类 + session 获取；async 处理（system/history/回合
    /// 本体）留给 tail。slash 命令在这里同步执行并短路（原路径在 busy
    /// 检查前同步返回；且命令可能有副作用，tail 不得重跑）。
    fn gate_inbound(&self, msg: &nemesis_types::channel::InboundMessage) -> GateOutcome {
        self.turn_preamble(msg);

        // Route system messages.
        if msg.channel == "system" {
            // Cluster continuation — the pump handles via dispatch_continuation.
            if msg
                .sender_id
                .starts_with(nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX)
            {
                let task_id =
                    &msg.sender_id[nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX.len()..];
                debug!(
                    "[AgentLoop] Cluster continuation message intercepted, task_id={}",
                    task_id
                );
                return GateOutcome::Continuation(task_id.to_string());
            }
            return GateOutcome::Ungated;
        }

        // History request.
        if let Some(request_type) = msg.metadata.get("request_type") {
            if request_type == "history" {
                return GateOutcome::Ungated;
            }
        }

        // Slash commands.
        if let Some(response) = self.handle_command_with_context(&msg.content, &msg.channel) {
            return GateOutcome::Immediate {
                agent_id: String::new(),
                response,
            };
        }

        let (agent_id, session_key) = self.route_message(msg);

        // Session busy check — I1 (U7) mode-aware:
        //   Reject (default): legacy BUSY_MESSAGE bounce, byte-identical.
        //   Queue/Steer: park the message in the session inbox instead of
        //   bouncing. The running turn claims it (steer: next LLM call; queue:
        //   turn end starts a new one with the head). Capacity-bounded with a
        //   clear receipt so the rule is discoverable.
        if !self.try_acquire_session(&session_key) {
            match self.concurrent_mode {
                ConcurrentMode::Reject => {
                    warn!(
                        "[AgentLoop] Session busy, returning busy message: session_key={}, mode={:?}",
                        session_key, self.concurrent_mode
                    );
                    return GateOutcome::Immediate {
                        agent_id,
                        response: BUSY_MESSAGE.to_string(),
                    };
                }
                ConcurrentMode::Queue | ConcurrentMode::Steer => {
                    let queued = crate::inbox::QueuedMessage {
                        msg: nemesis_types::channel::InboundMessage {
                            channel: msg.channel.clone(),
                            sender_id: msg.sender_id.clone(),
                            chat_id: msg.chat_id.clone(),
                            content: msg.content.clone(),
                            media: msg.media.clone(),
                            session_key: msg.session_key.clone(),
                            correlation_id: msg.correlation_id.clone(),
                            metadata: msg.metadata.clone(),
                            voice_playback: msg.voice_playback,
                        },
                        timestamp: chrono::Local::now().to_rfc3339(),
                    };
                    // Second-pass review fix: the `!`-prefix steer channel
                    // exists ONLY in Steer mode. Queue mode is pure
                    // queueing — enqueue_for_mode routes everything to
                    // next_turn when steer is disabled.
                    match self.inbox.enqueue_for_mode(
                        &session_key,
                        queued,
                        self.concurrent_mode == ConcurrentMode::Steer,
                    ) {
                        crate::inbox::EnqueueOutcome::QueuedForNextTurn => {
                            info!(
                                "[AgentLoop] Session busy — message queued for next turn: session_key={}",
                                session_key
                            );
                            return GateOutcome::Immediate {
                                agent_id,
                                response: "⏳ 当前正在处理上一条消息。你的消息已排队，将在本轮结束后继续处理。".to_string(),
                            };
                        }
                        crate::inbox::EnqueueOutcome::QueuedForNextStep => {
                            info!(
                                "[AgentLoop] Session busy — steer message queued for in-turn injection: session_key={}",
                                session_key
                            );
                            return GateOutcome::Immediate {
                                agent_id,
                                response: "⚡ 已接收为紧急插话（消息以 ! 开头），将在 AI 的下一步思考前注入。非紧急消息请去掉 ! 前缀排队等待。".to_string(),
                            };
                        }
                        crate::inbox::EnqueueOutcome::Rejected => {
                            warn!(
                                "[AgentLoop] Session busy and inbox full — message refused: session_key={}",
                                session_key
                            );
                            return GateOutcome::Immediate {
                                agent_id,
                                response: "⏳ 排队已满，消息未能接收。请等当前任务完成后再发。".to_string(),
                            };
                        }
                    }
                }
            }
        }

        // Create a cancellation token for this session (V5: minted in the
        // gate so the spawned turn tail starts with everything the
        // pre-split monolith had at this point).
        let cancel_token = self.create_cancel_token(&session_key);
        GateOutcome::Admitted(TurnAdmission {
            agent_id,
            session_key,
            cancel_token,
        })
    }

    /// Ungated tail: system (non-continuation) + history-request handling
    /// (V5: extracted verbatim from the pre-split monolith's early returns).
    async fn process_ungated(
        &self,
        msg: &nemesis_types::channel::InboundMessage,
    ) -> (String, String, Option<String>) {
        if msg.channel == "system" {
            let (resp, err) = self.process_system_message(msg).await;
            return (String::new(), resp, err);
        }

        // History request.
        if let Some(request_type) = msg.metadata.get("request_type") {
            if request_type == "history" {
                self.handle_history_request(msg).await;
                return (String::new(), String::new(), None);
            }
        }

        // The gate classified everything else; unreachable in practice.
        (String::new(), String::new(), None)
    }

    /// Admitted turn tail (V5): preprocess → run turn → cleanup/release →
    /// inbox drain. The session is ALREADY acquired and the cancel token
    /// minted by `gate_inbound`; this fn owns release + reinject (the drain
    /// block runs unconditionally after the turn so an admitted session can
    /// never leak busy).
    async fn process_admitted(
        &self,
        msg: &nemesis_types::channel::InboundMessage,
        admission: TurnAdmission,
    ) -> (String, String, Option<String>) {
        let TurnAdmission {
            agent_id,
            session_key,
            cancel_token,
        } = admission;

        let voice_playback = msg.voice_playback.unwrap_or(false);
        // @file expansion: inline referenced file contents before sending to LLM.
        let processed_content = crate::message_preprocess::expand_at_files(
            &msg.content,
            &std::env::current_dir().unwrap_or_default(),
        );
        let cron_job_id = msg.metadata.get("cron_job_id").map(|s| s.as_str());
        let cron_job_name = msg.metadata.get("cron_job_name").map(|s| s.as_str());
        // T3 (U12): per-fire tool-round budget set by the gateway cron fire
        // handler from the job's max_rounds payload. Absent/unparsable → None
        // → the turn runs under the global max_turns.
        let cron_max_rounds = msg
            .metadata
            .get("cron_max_rounds")
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|v| *v > 0);
        let result = self
            .run_agent_loop_internal(
                &session_key,
                &processed_content,
                &msg.channel,
                &msg.chat_id,
                voice_playback,
                &cancel_token,
                cron_job_id,
                cron_job_name,
                cron_max_rounds,
            )
            .await;

        // Clean up cancellation token and release session.
        self.remove_cancel_token(&session_key);
        self.release_session(&session_key);

        // I1 (U7) post-turn inbox handling:
        //   - Unconsumed next-step (steer) messages ALWAYS transfer back to
        //     the next-turn queue — both for cancelled turns AND for turns
        //     that finished normally with a steer arriving too late for the
        //     escape hatch (second-pass review fix: the cancelled-only
        //     transfer left stale steers in next_step, which a LATER turn
        //     would claim out of context). Transferred messages become the
        //     next turn's input — nothing is lost, nothing arrives stale.
        //   - Completed turn with queued next-turn messages: requeue the head
        //     through the normal inbound path (fresh busy acquire),
        //     serialized behind this turn because the session was already
        //     released.
        self.inbox.transfer_next_step_to_next_turn(&session_key);
        if matches!(
            self.concurrent_mode,
            ConcurrentMode::Queue | ConcurrentMode::Steer
        ) {
            if let Some(head) = self.inbox.claim_next_turn_head(&session_key) {
                info!(
                    "[AgentLoop] processing queued next-turn message: session_key={}, len={}",
                    session_key,
                    head.msg.content.len()
                );
                // ROUND-5 REVIEW FIX: re-inject the queued head into the
                // agent's own inbound mpsc (the same one `run_bus_*`
                // consumes) instead of recursing inline. The normal consumer
                // then applies the SAME post-processing as any other message:
                // [rpc:{cid}] correlation prefix, sent_in_round check+clear,
                // error → "Error processing message" + capture flush, and
                // meta.model. The previous inline recursion published the
                // reply raw — an rpc-channel queued reply lost its prefix
                // (RPCChannel::send drops unprefixed replies) and a FAILED
                // queued turn produced no user-visible output at all.
                // H3 (full review): the queued message wraps the ORIGINAL
                // InboundMessage whole (round-5), so replaying is a clone
                // with only the session_key normalized to this queue's key.
                //
                // NOT bus.publish_inbound (broadcast): that would re-match
                // workflow message-triggers for an already-matched message
                // (double firing). The agent's private mpsc has no other
                // subscriber.
                //
                // try_send (never a blocking await on a full channel from
                // inside the sole consumer task): capacity 1024 vs inbox cap
                // 8/session makes a full channel unreachable in practice; on
                // the theoretical overflow the message is logged and dropped
                // rather than deadlocking the loop.
                let mut inbound = head.msg.clone();
                inbound.session_key = session_key.to_string();
                let reinject = self.reinject_tx.read().clone();
                match reinject {
                    Some(tx) => {
                        if let Err(e) = tx.try_send(inbound) {
                            // No consumer (standalone/tests without run_bus)
                            // or channel full — surface loudly either way.
                            warn!(
                                "[AgentLoop] failed to re-inject queued message (session_key={}, err={}) — message dropped",
                                session_key, e
                            );
                        }
                    }
                    None => {
                        // Standalone mode / tests without the mpsc wired:
                        // process inline as before so the message is not
                        // silently lost, with the error path at least
                        // converted to a user-visible reply.
                        warn!(
                            "[AgentLoop] reinject_tx not set — processing queued message inline (post-processing skipped), session_key={}",
                            session_key
                        );
                        let fut = Box::pin(self.process_inbound_message(&inbound));
                        let (_id2, resp2, err2) = fut.await;
                        let content = match err2 {
                            Some(e) => format!("Error processing message: {}", e),
                            None => resp2,
                        };
                        if !content.is_empty() {
                            if let Some(ref tx) = self.outbound_tx {
                                let outbound = nemesis_types::channel::OutboundMessage {
                                    channel: head.msg.channel.clone(),
                                    chat_id: head.msg.chat_id.clone(),
                                    content,
                                    message_type: String::new(),
                                    meta: Default::default(),
                                };
                                let _ = tx.send(outbound).await;
                            }
                        }
                    }
                }
            }
            // Drop empty queues (housekeeping; keeps the map bounded).
            if self.inbox.pending(&session_key) == (0, 0) {
                self.inbox.clear(&session_key);
            }
        }

        match result {
            Ok(response) => (agent_id, response, None),
            Err(e) => (agent_id, String::new(), Some(e)),
        }
    }

    // -----------------------------------------------------------------------
    // System message routing
    // -----------------------------------------------------------------------

    /// Process a system message.
    /// Mirrors Go's `processSystemMessage()`.
    async fn process_system_message(
        &self,
        msg: &nemesis_types::channel::InboundMessage,
    ) -> (String, Option<String>) {
        if msg.channel != "system" {
            return (
                String::new(),
                Some(format!(
                    "processSystemMessage called with non-system channel: {}",
                    msg.channel
                )),
            );
        }

        info!(
            "[AgentLoop] Processing system message: sender_id={}, chat_id={}",
            msg.sender_id, msg.chat_id
        );

        // Parse origin channel from chat_id (format: "channel:chat_id").
        let (origin_channel, origin_chat_id) = if let Some(idx) = msg.chat_id.find(':') {
            (&msg.chat_id[..idx], msg.chat_id[idx + 1..].to_string())
        } else {
            ("cli", msg.chat_id.clone())
        };

        // Skip internal channels.
        if is_internal_channel(origin_channel) {
            info!(
                "[AgentLoop] Subagent completed (internal channel): content_len={}",
                msg.content.len()
            );
            return (String::new(), None);
        }

        // Use default agent session key.
        let session_key = build_agent_main_session_key("main");

        // Extract subagent result from message content.
        // Format: "Task 'label' completed.\n\nResult:\n<actual content>"
        // Mirrors Go's: if idx := strings.Index(content, "Result:\n"); idx >= 0 { content = content[idx+8:] }
        let content = if let Some(idx) = msg.content.find("Result:\n") {
            &msg.content[idx + 8..]
        } else {
            &msg.content
        };

        let cancel_token = tokio_util::sync::CancellationToken::new();
        let cron_job_id = msg.metadata.get("cron_job_id").map(|s| s.as_str());
        let cron_job_name = msg.metadata.get("cron_job_name").map(|s| s.as_str());
        let result = self
            .run_agent_loop_internal(
                &session_key,
                &format!("[System: {}] {}", msg.sender_id, content),
                origin_channel,
                &origin_chat_id,
                false,
                &cancel_token,
                cron_job_id,
                cron_job_name,
                None,
            )
            .await;

        match result {
            Ok(response) => (response, None),
            Err(e) => (String::new(), Some(e)),
        }
    }

    // -----------------------------------------------------------------------
    // History request handling
    // -----------------------------------------------------------------------

    /// Handle a history request by reading from session and publishing response.
    /// Mirrors Go's `handleHistoryRequest()`.
    async fn handle_history_request(&self, msg: &nemesis_types::channel::InboundMessage) {
        #[derive(Deserialize)]
        struct HistoryRequest {
            #[serde(default)]
            request_id: String,
            #[serde(default)]
            limit: Option<usize>,
            before_index: Option<usize>,
        }

        let req: HistoryRequest = match serde_json::from_str(&msg.content) {
            Ok(r) => r,
            Err(e) => {
                error!("[AgentLoop] Failed to parse history request: {}", e);
                self.publish_history_response(
                    &msg.chat_id,
                    "",
                    &Vec::<serde_json::Value>::new(),
                    false,
                    0,
                    0,
                )
                .await;
                return;
            }
        };

        let limit = req.limit.unwrap_or(20);
        let agent_id = self
            .registry
            .as_ref()
            .and_then(|r| r.default_agent_id())
            .unwrap_or_else(|| "main".to_string());
        // Multi-session: if the client sent a session_id, derive
        // `agent:main:session:{sid}` (agent: prefix → adopted by loop.rs:1623,
        // bypasses routing). Otherwise fall back to the default "legacy"
        // conversation — MUST match server.rs process_messages' fallback so
        // history-read and chat-write share the same key (else the default
        // conversation's history wouldn't reload).
        let session_key = match msg.metadata.get("session_id") {
            Some(sid) if !sid.is_empty() => format!(
                "agent:main:session:{}",
                crate::session::SessionStore::sanitize_session_id(sid)
            ),
            _ => format!("agent:{}:session:legacy", agent_id),
        };

        // Read history from chat log (separate from session store).
        let (page, total_count, has_more, oldest_index) =
            crate::chat_log::read_chat_log(&session_key, limit, req.before_index);

        self.publish_history_response(
            &msg.chat_id,
            &req.request_id,
            &page,
            has_more,
            oldest_index,
            total_count,
        )
        .await;
    }

    /// Publish a history response via the outbound channel.
    /// Mirrors Go's `publishHistoryResponse()`.
    async fn publish_history_response(
        &self,
        chat_id: &str,
        request_id: &str,
        messages: &[serde_json::Value],
        has_more: bool,
        oldest_index: usize,
        total_count: usize,
    ) {
        let response_data = serde_json::json!({
            "request_id": request_id,
            "messages": messages,
            "has_more": has_more,
            "oldest_index": oldest_index,
            "total_count": total_count,
        });

        let content = match serde_json::to_string(&response_data) {
            Ok(c) => c,
            Err(e) => {
                error!("[AgentLoop] Failed to marshal history response: {}", e);
                return;
            }
        };

        if let Some(ref tx) = self.outbound_tx {
            let outbound = nemesis_types::channel::OutboundMessage {
                channel: "web".to_string(),
                chat_id: chat_id.to_string(),
                content,
                message_type: "history".to_string(),
                meta: Default::default(),
            };
            if let Err(e) = tx.send(outbound).await {
                warn!("[AgentLoop] Failed to send history response: {}", e);
            }
        } else {
            warn!("[AgentLoop] publish_history_response: no outbound_tx available");
        }

        debug!(
            "[AgentLoop] History response published: chat_id={}, request_id={}, total_count={}, has_more={}",
            chat_id, request_id, total_count, has_more
        );
    }

    // -----------------------------------------------------------------------
    // State recording
    // -----------------------------------------------------------------------

    /// Record the last active channel for crash recovery.
    /// Mirrors Go's `state.Manager.SetLastChannel()`.
    pub fn record_last_channel(&self, channel: &str) {
        if let Some(ref mgr) = self.state_manager {
            if let Err(e) = mgr.set_last_channel(channel) {
                tracing::warn!("[AgentLoop] Failed to persist last channel: {}", e);
            }
        }
    }

    /// Record the last active chat ID for crash recovery.
    /// Mirrors Go's `state.Manager.SetLastChatID()`.
    pub fn record_last_chat_id(&self, chat_id: &str) {
        if let Some(ref mgr) = self.state_manager {
            if let Err(e) = mgr.set_last_chat_id(chat_id) {
                tracing::warn!("[AgentLoop] Failed to persist last chat ID: {}", e);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Session busy state management
    // -----------------------------------------------------------------------

    /// Get or create the busy state for a session.
    /// Mirrors Go's `getSessionBusyState()`.
    pub fn get_session_busy_state(&self, session_key: &str) -> (bool, usize) {
        let map = self.session_busy.lock();
        match map.get(session_key) {
            Some(state) => (state.busy, state.queue_length),
            None => (false, 0),
        }
    }

    /// Try to acquire a session for processing.
    /// Returns true if acquired, false if busy (and queue is full in queue mode).
    /// Mirrors Go's `tryAcquireSession()`.
    pub fn try_acquire_session(&self, session_key: &str) -> bool {
        let mut map = self.session_busy.lock();
        let state = map.entry(session_key.to_string()).or_default();

        if !state.busy {
            state.busy = true;
            return true;
        }

        // Session is busy — reject HERE. Queueing is NOT done at this layer:
        // the old Queue path incremented queue_length WITHOUT storing the
        // message, and release_session kept busy while queue_length>0 → the
        // session could deadlock (no turn ever acquires to drain the
        // counter). Busy-queueing lives in `crate::inbox` (FIFO per session),
        // driven by `gate_inbound`'s busy branch — reachable since V5's
        // gate-in-pump restructure (the gate runs before the turn task is
        // spawned, so a busy session is detected while its turn is still
        // running). (queue_length stays 0, so release_session naturally sets
        // busy=false.)
        let _ = self.concurrent_mode;
        false
    }

    /// Release a session after processing.
    /// Returns true if there are queued requests remaining.
    /// Mirrors Go's `releaseSession()`.
    pub fn release_session(&self, session_key: &str) -> bool {
        let mut map = self.session_busy.lock();
        if let Some(state) = map.get_mut(session_key) {
            if state.queue_length > 0 {
                state.queue_length -= 1;
                // Keep busy since there are queued requests.
                return true;
            }
            state.busy = false;
        }
        false
    }

    /// Check whether a session is currently busy.
    pub fn is_session_busy(&self, session_key: &str) -> bool {
        let map = self.session_busy.lock();
        map.get(session_key).map_or(false, |s| s.busy)
    }

    /// Get the queue length for a session.
    pub fn session_queue_length(&self, session_key: &str) -> usize {
        let map = self.session_busy.lock();
        map.get(session_key).map_or(0, |s| s.queue_length)
    }

    /// U7 dashboard visibility (G1): one-call snapshot of a session's inbox
    /// state for the `agent.inbox_status` WSAPI command. Read-only — claims
    /// and transfers stay exclusively in the turn lifecycle paths.
    pub fn inbox_status(&self, session_key: &str) -> crate::inbox::InboxStatus {
        let (next_turn, next_step) = self.inbox.pending(session_key);
        crate::inbox::InboxStatus {
            next_turn,
            next_step,
            capacity: self.inbox.capacity(),
            busy: self.is_session_busy(session_key),
            mode: match self.concurrent_mode {
                ConcurrentMode::Reject => "reject",
                ConcurrentMode::Queue => "queue",
                ConcurrentMode::Steer => "steer",
            },
        }
    }

    // -----------------------------------------------------------------------
    // Session cancellation
    // -----------------------------------------------------------------------

    /// Cancel an in-progress session by session_key.
    ///
    /// If the session is currently being processed by the LLM loop, this
    /// triggers the cancellation token, causing the loop to break at the
    /// next check point (after the current LLM call or tool execution).
    ///
    /// Returns true if a cancellation token was found and cancelled.
    pub fn cancel_session(&self, session_key: &str) -> bool {
        if let Some(token) = self.cancel_tokens.get(session_key) {
            token.cancel();
            info!(
                "[AgentLoop] Session cancellation requested: {}",
                session_key
            );
            true
        } else {
            debug!("[AgentLoop] No active session to cancel: {}", session_key);
            false
        }
    }

    /// Cancel all in-progress sessions.
    ///
    /// Returns the number of sessions that were cancelled.
    pub fn cancel_all_sessions(&self) -> usize {
        let mut count = 0;
        for entry in self.cancel_tokens.iter() {
            entry.value().cancel();
            count += 1;
        }
        if count > 0 {
            info!("[AgentLoop] Cancelled {} active session(s)", count);
        }
        count
    }

    /// Create and store a cancellation token for a session.
    /// Returns the token for the caller to pass into the processing pipeline.
    fn create_cancel_token(&self, session_key: &str) -> tokio_util::sync::CancellationToken {
        let token = tokio_util::sync::CancellationToken::new();
        self.cancel_tokens
            .insert(session_key.to_string(), token.clone());
        token
    }

    /// Remove the cancellation token for a session after processing completes.
    fn remove_cancel_token(&self, session_key: &str) {
        self.cancel_tokens.remove(session_key);
    }

    // -----------------------------------------------------------------------
    // Summarization
    // -----------------------------------------------------------------------

    /// Advance the summary cache if the verbatim tail is over the context
    /// threshold.
    ///
    /// Inline-summarization pipeline (replaces Go's `maybeSummarize`). The
    /// summary cache covers `history[..covers_up_to]`; `build_messages` sends
    /// `history[covers_up_to..]` verbatim. Token pressure is therefore on the
    /// *tail* (what the LLM actually receives), so the threshold is evaluated
    /// against the tail, not the full history. When the tail exceeds the
    /// threshold and is longer than `K_TARGET`, the cache advances to
    /// `covers_up_to = len - K_TARGET` and the newly-covered prefix is folded
    /// into the summary. History is never mutated (append-only); bounding is
    /// the session store's job.
    ///
    /// Persistence: this updates the in-memory cache on the instance. The save
    /// path persists the cache alongside the full history (see S3.3).
    async fn maybe_update_summary(
        &self,
        instance: &AgentInstance,
        session_key: &str,
        channel: &str,
        chat_id: &str,
    ) {
        let history = instance.get_history();
        // U16 (sixth batch): prefer the active model's per-model
        // `context_window` from config.json over the instance's 32000
        // default (the S1-S7 leftover). Falls back to the instance value
        // when unset/standalone — behavior unchanged for configs without
        // the field.
        let context_window = self
            .current_context_window()
            .unwrap_or_else(|| instance.context_window());

        // C indexes the full history (system prompt at index 0); the verbatim
        // tail build_messages sends is history[C..]. Clamp to history length
        // for safety against a stale cache index.
        let cache = instance.get_summary_cache();
        let c = cache
            .as_ref()
            .map(|c| c.covers_up_to)
            .filter(|&c| c >= 1)
            .unwrap_or(0)
            .min(history.len());
        let existing_summary = cache.as_ref().map(|c| c.text.as_str()).unwrap_or("");

        // Token pressure is on the tail (system + summary + history[C..] is
        // what build_messages emits). The covered prefix is already folded into
        // the summary, so it does not count toward pressure. X1: measured over
        // the MODEL-FACING projection — history keeps tool originals since the
        // size gates moved to build_messages, so the raw estimate would count
        // a 70KB original the provider only ever sees as a bounded locator.
        let tail_tokens = estimate_tokens_for_turns_projected(&history[c..]);
        let tail_len = history.len().saturating_sub(c);
        let soft = context_window * COMPACT_SOFT_RATIO / 100;
        let threshold = context_window * COMPACT_SUMMARIZE_RATIO / 100;
        // Summarize runs only when the tail is over threshold AND long enough to
        // shrink (more than K_TARGET messages past C). A short tail that is huge
        // in tokens (a large system prompt or an early oversized tool result)
        // can't be helped by advancing C — leave it to force_compression.
        let will_summarize = tail_tokens >= threshold && tail_len > K_TARGET;

        // ⑩ Graded tiers (soft / summarize) + stuck self-check on the tail.
        // Soft is info-log-only. The stuck counter only ticks when summarize
        // will ACTUALLY run — otherwise a chronically-over-threshold tail that
        // is too short to summarize (e.g. a big system prompt with few turns)
        // would tick the counter without ever attempting a summarize and pause
        // summarization before it gets the chance.
        let mut paused_stuck = false;
        {
            let mut states = self.compact_state.lock();
            if states.len() > SESSION_STATE_MAX_ENTRIES {
                states.clear();
            }
            let st = states.entry(session_key.to_string()).or_default();

            if tail_tokens >= soft && !st.soft_noticed {
                st.soft_noticed = true;
                info!(
                    "[AgentLoop] context tail at ~{}% of window ({} / {}); summarization will trigger at {}%",
                    tail_tokens * 100 / context_window.max(1),
                    tail_tokens,
                    context_window,
                    COMPACT_SUMMARIZE_RATIO
                );
            }

            if will_summarize {
                if summarize_was_ineffective(st.last_summary_tokens, tail_tokens) {
                    st.consecutive_failures += 1;
                } else {
                    st.consecutive_failures = 0;
                }
                if st.consecutive_failures >= COMPACT_STUCK_LIMIT {
                    if !st.stuck {
                        warn!(
                            "[AgentLoop] compaction stuck: summarization has not reduced the tail {} times in a row; pausing auto-summarization (raise context_window or reduce tool output)",
                            st.consecutive_failures
                        );
                        st.stuck = true;
                    }
                    paused_stuck = true;
                } else {
                    st.last_summary_tokens = tail_tokens;
                }
            } else if tail_tokens < threshold {
                // Breathing room — clear the stuck latch.
                st.consecutive_failures = 0;
                st.stuck = false;
            }
        }
        if paused_stuck {
            return;
        }

        if !will_summarize {
            return;
        }

        let summarize_key = format!("main:{}", session_key);
        {
            let mut map = self.summarizing.lock();
            if map.len() > SESSION_STATE_MAX_ENTRIES {
                map.clear();
            }
            if map.contains_key(&summarize_key) {
                return;
            }
            map.insert(summarize_key.clone(), true);
        }

        // New boundary: cover everything except the last K_TARGET messages
        // (the verbatim tail kept for continuity). new_C > c is guaranteed by
        // the tail_len > K_TARGET check above, so we always advance and never
        // re-summarize the same prefix. Adjust so the tail doesn't start mid
        // tool_call/result pair (keeps the pair verbatim, not dropped by repair).
        let new_c = tool_safe_boundary(&history, history.len() - K_TARGET);

        let provider = self.provider.read().clone();
        let model = self.active_model.read().clone();
        let outbound_tx = self.outbound_tx.clone();
        let summarizing_flag = self.summarizing.clone();
        let observer_mgr = self.observer_manager.clone();
        let channel_owned = channel.to_string();
        let chat_id_owned = chat_id.to_string();
        let clear_key = summarize_key.clone();

        if !is_internal_channel(&channel_owned) {
            if let Some(ref tx) = outbound_tx {
                let outbound = nemesis_types::channel::OutboundMessage {
                    channel: channel_owned.clone(),
                    chat_id: chat_id_owned.clone(),
                    content: "Memory threshold reached. Optimizing conversation history..."
                        .to_string(),
                    message_type: String::new(),
                    meta: Default::default(),
                };
                let _ = tx.send(outbound).await;
            }
        }

        // Fold the prefix history[..new_c] into the summary, merged with the
        // existing summary (which already covers history[..c]). summarize the
        // FULL prefix from source each time (no "keep last N" — that would
        // leave a gap between the summary and the verbatim tail).
        let prefix_refs: Vec<&crate::types::ConversationTurn> = history[..new_c].iter().collect();
        let summary = summarize_prefix_owned(
            &prefix_refs,
            existing_summary,
            context_window,
            self.current_summarizer_prefix_reuse(),
            provider.as_ref(),
            &model,
            observer_mgr,
        )
        .await;

        if let Some(summary) = summary {
            instance.set_summary_cache(Some(crate::instance::SummaryCache {
                covers_up_to: new_c,
                text: summary,
            }));
        } else {
            // 2026-08-25 摘要静默失败修复：失败（或无可摘要内容）时绝不推进
            // covers —— 否则被折叠的上下文会静默丢失。Loud warn，不静默。
            warn!(
                "[AgentLoop] auto-summarization produced no summary for {} (LLM failure or no valid content); keeping full history, covers_up_to unchanged",
                session_key
            );
        }

        {
            let mut map = summarizing_flag.lock();
            map.remove(&clear_key);
        }
    }

    /// Force-compress by aggressively advancing the summary cache.
    ///
    /// Last-resort path used when the LLM reports a context error and the
    /// caller retries. Does NOT mutate history (history is append-only); it
    /// shrinks what `build_messages` emits by advancing `covers_up_to` and
    /// recomputing the summary over the larger covered prefix.
    ///
    /// Progressive: each call shrinks the verbatim tail further. The first call
    /// reduces the tail to [`SMALL_K_FORCE`] messages; if that still isn't
    /// enough (the caller will retry), the next call folds everything into the
    /// summary (tail → 0). Bounded by the caller's retry limit.
    pub async fn force_compression(&self, instance: &AgentInstance) {
        let history = instance.get_history();
        let cache = instance.get_summary_cache();
        let current_c = cache
            .as_ref()
            .map(|c| c.covers_up_to)
            .filter(|&c| c >= 1)
            .unwrap_or(0)
            .min(history.len());
        let existing_summary = cache.as_ref().map(|c| c.text.as_str()).unwrap_or("");

        // Shrink the verbatim tail: to SMALL_K_FORCE if it's still large,
        // otherwise cover everything (tail → 0) as the final resort.
        let current_tail_len = history.len().saturating_sub(current_c);
        let raw_c = if current_tail_len > SMALL_K_FORCE {
            history.len() - SMALL_K_FORCE
        } else {
            history.len()
        };
        // Keep tool_call/result pairs intact in the tail (don't let the boundary
        // drop an orphan result that the summary can't capture).
        let new_c = tool_safe_boundary(&history, raw_c);
        // Must advance and have a non-empty prefix to summarize.
        if new_c <= current_c || new_c == 0 {
            return;
        }

        let provider = self.provider.read().clone();
        let model = self.active_model.read().clone();
        let observer_mgr = self.observer_manager.clone();

        // Fold the prefix history[..new_c] into the summary, merged with the
        // existing summary (which covers history[..current_c]).
        let prefix_refs: Vec<&crate::types::ConversationTurn> = history[..new_c].iter().collect();
        let summary = summarize_prefix_owned(
            &prefix_refs,
            existing_summary,
            // U16: per-model context_window when declared (same preference
            // order as the threshold computation above).
            self.current_context_window()
                .unwrap_or_else(|| instance.context_window()),
            // T4: per-model prefix-reuse switch (false → old bare shape).
            self.current_summarizer_prefix_reuse(),
            provider.as_ref(),
            &model,
            observer_mgr,
        )
        .await;

        if let Some(summary) = summary {
            instance.set_summary_cache(Some(crate::instance::SummaryCache {
                covers_up_to: new_c,
                text: summary,
            }));
            info!(
                "[AgentLoop] Force-compressed: covers_up_to {} -> {} (verbatim tail {} -> {} messages)",
                current_c,
                new_c,
                current_tail_len,
                history.len() - new_c
            );
        } else {
            // 2026-08-25 摘要静默失败修复：force 路径同样绝不推进 covers。
            // 调用方的有界重试会放弃并向上报错（响亮失败优于静默失忆）。
            warn!(
                "[AgentLoop] force-compression produced no summary (LLM failure or no valid content); covers_up_to stays {}, the caller's bounded retry will surface the error",
                current_c
            );
        }
    }

    // -----------------------------------------------------------------------
    // Internal agent loop execution
    // -----------------------------------------------------------------------

    /// Get or create an AgentInstance for the given session key.
    fn get_or_create_instance(&self, session_key: &str) -> AgentInstance {
        let config = AgentConfig {
            model: self.active_model.read().clone(),
            system_prompt: self.config.system_prompt.clone(),
            max_turns: self.config.max_turns,
            tools: self.config.tools.clone(),
            models: self.config.models.clone(),
        };
        let instance = AgentInstance::new(config);

        // Restore history + summary cache from session store if available.
        // Mirrors Go's `agent.Sessions.Get(sessionKey)` in `getOrCreateInstance`.
        if let Some(ref store) = self.session_store {
            let stored = store.get_or_create(session_key);
            let existing_summary = store.get_summary(session_key);
            let covers = store.get_summary_covers_up_to(session_key);
            if !stored.messages.is_empty() {
                let history: Vec<crate::types::ConversationTurn> =
                    stored.messages.into_iter().map(|m| m.into()).collect();
                instance.set_history(history);
            }
            // Restore the summary cache. covers_up_to indexes the full history
            // (system prompt at index 0). For new-format files it is stored
            // explicitly; for legacy files (pre-refactor, field absent → None)
            // we map it so build_messages sends ALL loaded messages verbatim
            // while injecting the summary as floating context for older content
            // that was truncated out under the old regime.
            if !existing_summary.is_empty() {
                let history = instance.get_history();
                if !history.is_empty() {
                    let c = match covers {
                        Some(c) => c.clamp(1, history.len()),
                        None => history
                            .iter()
                            .take_while(|t| t.role == "system")
                            .count()
                            .max(1),
                    };
                    instance.set_summary_cache(Some(crate::instance::SummaryCache {
                        covers_up_to: c,
                        text: existing_summary,
                    }));
                }
            }
        }

        instance
    }

    /// Run the agent loop for a specific session.
    /// Mirrors Go's `runAgentLoop()`.
    async fn run_agent_loop_internal(
        &self,
        session_key: &str,
        user_message: &str,
        channel: &str,
        chat_id: &str,
        voice_playback: bool,
        cancel_token: &tokio_util::sync::CancellationToken,
        cron_job_id: Option<&str>,
        cron_job_name: Option<&str>,
        turn_budget: Option<u32>,
    ) -> Result<String, String> {
        // Round-5 fix: cron-originated turns are exempt from boundary events,
        // same as heartbeat. A recurring cron job targeting a persistent
        // session would grow its boundary sidecar (3+ rows per fire) without
        // bound — the exact unbounded-growth failure the heartbeat exemption
        // exists to prevent. The gateway cron handler (gateway.rs) delivers
        // via the bus with metadata cron_job_id, which flows here through
        // the caller; the CronTool path passes user="cron" (detected below).
        let is_cron_turn = cron_job_id.is_some();
        // Generate trace ID and emit conversation_start event.
        let trace_id = format!(
            "{}-{}",
            session_key,
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let start_time = std::time::Instant::now();

        // Emit conversation_start observer event.
        self.emit_observer_sync(crate::loop_executor::ObserverEvent::ConversationStart {
            trace_id: trace_id.clone(),
            session_key: session_key.to_string(),
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
            sender_id: "agent".to_string(),
            content: user_message.to_string(),
        })
        .await;

        // Record last channel (skip internal channels).
        if !channel.is_empty() && !chat_id.is_empty() && !is_internal_channel(channel) {
            let channel_key = format!("{}:{}", channel, chat_id);
            self.record_last_channel(&channel_key);
        }

        let instance = self.get_or_create_instance(session_key);
        let mut context = RequestContext::new(channel, chat_id, "agent", session_key);
        if is_cron_turn {
            // Round-5 fix: propagate cron origin so run_llm_loop's boundary
            // gating can exempt it (see log_boundaries).
            context.user = "cron".to_string();
        }

        let events = self
            .run_with_trace(
                &instance,
                user_message,
                &context,
                &trace_id,
                voice_playback,
                cancel_token,
                turn_budget,
            )
            .await;

        // Maybe trigger summarization.
        self.maybe_update_summary(&instance, session_key, channel, chat_id)
            .await;

        // Persist to session store. Post-refactor (inline-summarization): the
        // store holds the FULL instance history (system + user + assistant +
        // tool calls/results), not just a user/assistant log, so the summary
        // cache's covers_up_to index stays coherent across turns (the instance
        // is rebuilt from the store each turn). The user-facing conversation
        // log is written separately to chat_log below.
        //
        // (Pre-refactor this appended only user + final assistant, mirroring
        // Go's runAgentLoop. That left tool context out of the session file
        // and made a persistent summary cache incoherent — the root cause of
        // the "summary not injected / silent amnesia" bug this refactor fixes.)

        // Extract final response once (shared by session store, chat log, and observer).
        let final_response = events
            .iter()
            .rev()
            .find_map(|e| {
                if let AgentEvent::Done(msg) = e {
                    Some(msg.clone())
                } else if let AgentEvent::Error(msg) = e {
                    Some(msg.clone())
                } else {
                    None
                }
            })
            .unwrap_or_default();

        if let Some(ref store) = self.session_store {
            // Ensure session exists in store.
            store.get_or_create(session_key);

            // Persist the summary cache (text + covers_up_to) BEFORE the
            // history: set_history's trim_to_limit reads covers_up_to to decide
            // which oldest messages are safe to drop (only from the covered
            // prefix) and adjusts it downward by the number dropped. Setting
            // covers first means the trim operates on this turn's correct value
            // and leaves a coherent store (messages + covers aligned). Setting
            // it AFTER would clobber the trim's adjustment and, for long
            // conversations (>MAX_STORED_MESSAGES), leave covers_up_to too large
            // so build_messages drops the verbatim tail.
            match instance.get_summary_cache() {
                Some(cache) => {
                    store.set_summary(session_key, &cache.text);
                    store.set_summary_covers_up_to(session_key, Some(cache.covers_up_to));
                }
                None => {
                    store.set_summary(session_key, "");
                    store.set_summary_covers_up_to(session_key, None);
                }
            }

            // Persist the full instance history (the store is the single source
            // of truth the next turn's get_or_create_instance reloads). trim runs
            // inside, bounding to MAX_STORED_MESSAGES and adjusting covers_up_to.
            let stored: Vec<crate::session::StoredMessage> = instance
                .get_history()
                .iter()
                .map(crate::session::StoredMessage::from)
                .collect();
            store.set_history(session_key, stored);

            if let Err(e) = store.save(session_key) {
                warn!(
                    "[AgentLoop] Failed to persist session history for {}: {}",
                    session_key, e
                );
            }
        }

        // Append to chat log (independent of session store).
        crate::chat_log::append_chat_log_full(
            session_key,
            "user",
            user_message,
            None,
            cron_job_id,
            cron_job_name,
        );
        crate::chat_log::append_chat_log_full(
            session_key,
            "assistant",
            &final_response,
            Some(&self.current_display_model()),
            cron_job_id,
            cron_job_name,
        );

        // Emit conversation_end observer event.
        let duration_ms = start_time.elapsed().as_millis() as u64;
        let rounds = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::ToolCall(_)))
            .count() as u32
            + 1;
        self.emit_observer_sync(crate::loop_executor::ObserverEvent::ConversationEnd {
            trace_id: trace_id.clone(),
            session_key: session_key.to_string(),
            total_rounds: rounds,
            duration_ms,
            content: final_response,
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
        })
        .await;

        // Extract final response.
        for event in events.iter().rev() {
            if let AgentEvent::Done(msg) = event {
                return Ok(msg.clone());
            }
        }
        for event in events.iter().rev() {
            if let AgentEvent::Error(msg) = event {
                return Err(msg.clone());
            }
        }

        Ok("I've completed processing but have no response to give.".to_string())
    }

    // -----------------------------------------------------------------------
    // Standalone run loop
    // -----------------------------------------------------------------------

    /// Run the agent loop to process a user message (standalone mode).
    ///
    /// Returns a vector of events produced during execution.
    pub async fn run(
        &self,
        instance: &AgentInstance,
        user_message: &str,
        context: &RequestContext,
    ) -> Vec<AgentEvent> {
        let trace_id = format!(
            "run-{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let token = tokio_util::sync::CancellationToken::new();
        self.run_with_trace(
            instance,
            user_message,
            context,
            &trace_id,
            false,
            &token,
            None,
        )
        .await
    }

    /// Run the agent loop with a specific trace ID for observer event correlation.
    ///
    /// This is the actual implementation that emits observer events for:
    /// - LLM request (before calling the provider)
    /// - LLM response (after receiving the response)
    /// - Tool call (after each tool execution)
    pub async fn run_with_trace(
        &self,
        instance: &AgentInstance,
        user_message: &str,
        context: &RequestContext,
        trace_id: &str,
        voice_playback: bool,
        cancel_token: &tokio_util::sync::CancellationToken,
        turn_budget: Option<u32>,
    ) -> Vec<AgentEvent> {
        // K2 (U14): prompt-level lifecycle hooks (CC SessionStart +
        // UserPromptSubmit dialect events). Runs BEFORE `add_user_message`
        // so a blocked prompt NEVER enters history — the model doesn't see
        // it, matching CC's block semantics. `resume_execution` does not
        // pass through here (no new user prompt → no event), by design.
        {
            let lifecycle = self.lifecycle_hooks.read().snapshot();
            if !lifecycle.is_empty() {
                let prompt = crate::hooks::HookPrompt {
                    session_key: context.session_key.clone(),
                    channel: context.channel.clone(),
                    chat_id: context.chat_id.clone(),
                    prompt: user_message.to_string(),
                };
                if let Some(reason) = crate::hooks::run_user_prompt_hooks(&lifecycle, &prompt).await
                {
                    warn!(
                        "[AgentLoop] prompt hook blocked the message for session '{}': {}",
                        context.session_key, reason
                    );
                    return vec![AgentEvent::Done(format!(
                        "⛔ HOOK BLOCKED: {} — A registered hook denied this prompt. Adjust the hook policy and resend.",
                        reason
                    ))];
                }
            }
        }

        // Add user message to instance history.
        instance.add_user_message(user_message);
        instance.set_state(crate::types::AgentState::Thinking);

        self.run_llm_loop(
            instance,
            context,
            trace_id,
            voice_playback,
            cancel_token,
            turn_budget,
        )
        .await
    }

    /// Resume execution from a previously saved conversation state.
    ///
    /// Unlike `run_with_trace()`, this does NOT inject a user message.
    /// The instance should already have history loaded (via `set_history()`)
    /// and a tool result injected (via `add_tool_result()`).
    pub async fn resume_execution(
        &self,
        instance: &AgentInstance,
        context: &RequestContext,
        trace_id: &str,
    ) -> Vec<AgentEvent> {
        instance.set_state(crate::types::AgentState::Thinking);
        let token = tokio_util::sync::CancellationToken::new();
        self.run_llm_loop(instance, context, trace_id, false, &token, None)
            .await
    }

    /// Core LLM loop shared by `run_with_trace()` and `resume_execution()`.
    ///
    /// `turn_budget` (T3/U12): per-turn tool-round override. When set (>0) it
    /// REPLACES `config.max_turns` for this turn — the per-fire budget of a
    /// cron continuation. Exhaustion reuses the grace-round semantics (one
    /// finalize round, then a resumable stop with reason `budget_exhausted`).
    async fn run_llm_loop(
        &self,
        instance: &AgentInstance,
        context: &RequestContext,
        trace_id: &str,
        voice_playback: bool,
        cancel_token: &tokio_util::sync::CancellationToken,
        turn_budget: Option<u32>,
    ) -> Vec<AgentEvent> {
        let mut events = Vec::new();

        // max_tokens: per-model `max_output_tokens` from config if declared
        // (each model's real output ceiling, not a blanket 8192 — large files
        // write in one shot instead of truncating); else 8192. temperature 0.7.
        let chat_opts = crate::types::ChatOptions {
            max_tokens: Some(self.current_max_tokens().unwrap_or(8192)),
            temperature: Some(0.7),
            // H4 (U16 half): per-model reasoning effort from config.json.
            reasoning_effort: self.current_reasoning_effort(),
            ..Default::default()
        };

        // I3 (U9): durable turn boundary markers. Heartbeat sessions are
        // exempt — they run periodically and would grow heartbeat.jsonl
        // without bound (3+ boundary lines per beat, forever).
        // 4th-pass fix: the heartbeat exemption keys on the CONTEXT user
        // marker (set by process_heartbeat), not the session_key — a real
        // user session could legitimately be named "agent:heartbeat" and
        // would have been silently exempted from boundary logging.
        // Round-5 fix: cron-originated turns are exempt too (user=="cron",
        // set by run_agent_loop_internal when cron_job_id metadata is
        // present) — a recurring cron on a persistent session grows the
        // boundary sidecar unboundedly otherwise.
        let log_boundaries = context.user != "heartbeat"
            && context.user != "cron"
            && !is_internal_channel(&context.channel);
        if log_boundaries {
            crate::chat_log::append_boundary_event(&context.session_key, "turn_start", "");
        }
        let mut turns_used = 0u32;
        // Phase 2 (small-model-tool-robustness): per-request consecutive
        // validation-failure counter. Reset on any successful (valid or
        // auto-fixed) tool call; incremented on each schema violation. When it
        // reaches the tier budget the loop stops, preventing a struggling model
        // from burning max_turns on the same malformed call.
        let mut validation_failures = 0u32;
        // Continue-generation budget for max_tokens truncation: when output
        // hits the token cap it's cut mid-way (often mid tool-call JSON).
        // Instead of routing the broken call through the validation budget
        // (which force-stops with a misleading "args invalid" error — Big tier
        // = 0 retries), append partial content + a "continue" prompt and
        // re-loop. Mirrors 内部专项 (MAX_CONTINUATIONS=5) / nanobot (3).
        let mut length_continuations = 0u32;
        const MAX_LENGTH_CONTINUATIONS: u32 = 5;
        // ② Grace-round latch. When the tool-call budget is exhausted we grant
        // one extra round (with GRACE_ROUND_NUDGE injected) so the model can
        // synthesize a final answer from completed work; a second hit stops
        // resumably instead of hard-crashing with "Max iterations reached".
        let mut grace_round = false;
        // Turn-scoped guards (⑥ alternating loop, ⑦ degenerate output). Fresh
        // per request — no state crosses requests.
        let mut turn_guard = crate::turn_guard::TurnGuard::new();
        // ⑦ Degenerate-answer nudge awaiting re-injection. Transient — kept out
        // of instance history / session_log; re-applied after each build_messages
        // until the model gives a visible answer or the retry budget runs out.
        let mut degenerate_nudge_pending: Option<String> = None;
        // ⑧ Pending cross-round prose-repetition nudge (same transient pattern:
        // re-applied after each build_messages, never persisted to history).
        let mut repetition_nudge_pending: Option<String> = None;
        // I1 (U7): one-shot escape-hatch latch (see the Accept branch).
        let mut steer_escape_used = false;
        // L2 (full review): terminal reason recorded AT the break site
        // instead of post-hoc string sniffing (a model reply containing
        // the paused-after wording would have been misclassified).
        let mut terminal_reason: Option<&'static str> = None;
        // K2 (U14): turn-end hook (CC Stop) continue budget. Each
        // `Continue` demand injects the hook feedback as a user message and
        // grants one more round; exhausted → stop anyway (fail-open, same
        // discipline as MAX_LLM_HOOK_RETRIES).
        let mut turn_end_continues: u32 = 0;

        // K1b (U14): labeled so the LLM post-hook retry loop (deep inside,
        // around the guarded re-call) can abort the turn with `break 'turn`.
        // Bare `break`s elsewhere keep targeting their nearest loop — this
        // label only ADDS a way to name the turn loop, changing nothing else.
        'turn: loop {
            // Auto-reload MCP tools if config file changed.
            self.check_mcp_reload();
            // Phase 4a: re-resolve capability tier if config.json changed on
            // disk (dashboard model add, CLI `model set-tier` while running).
            self.check_config_reload();

            // Check cancellation at the top of each iteration.
            if cancel_token.is_cancelled() {
                info!(
                    "[AgentLoop] LLM loop cancelled at top of iteration, turns_used={}",
                    turns_used
                );
                events.push(AgentEvent::Done("已取消".to_string()));
                break;
            }

            // 全局急停检查：触发则立刻结束当前轮。未接线（None）时整块跳过。
            let estop_engaged = self
                .estop
                .read()
                .as_ref()
                .map(|e| e.is_engaged())
                .unwrap_or(false);
            if estop_engaged {
                info!(
                    "[AgentLoop] E-stop engaged at top of iteration, turns_used={}",
                    turns_used
                );
                events.push(AgentEvent::Done(
                    "⛔ 已急停 (E-STOP) — 已停止当前任务。发送 `nemesisbot estop --release` 恢复。"
                        .to_string(),
                ));
                break;
            }

            // ①/② max_turns cap + grace round. max_turns == 0 means unlimited
            // (opt-in). T3 (U12): when a per-turn budget override is set
            // (cron continuation's max_rounds), it REPLACES the global cap for
            // this turn. On the first hit we grant one grace round (with
            // GRACE_ROUND_NUDGE injected below) so the model can finalize from
            // completed work; a second hit stops resumably — no work is lost.
            let effective_max_turns = turn_budget.unwrap_or(self.config.max_turns);
            if effective_max_turns > 0 && turns_used >= effective_max_turns {
                if !grace_round {
                    grace_round = true;
                    info!(
                        "[AgentLoop] max_turns ({}) reached after {} turns; granting one grace round to finalize",
                        effective_max_turns, turns_used
                    );
                    // Fall through: this iteration runs as the grace round.
                } else if turn_budget.is_some() {
                    warn!(
                        "[AgentLoop] paused after {} tool-call rounds (per-turn budget exhausted, grace round spent)",
                        effective_max_turns
                    );
                    // T3 (U12): budget-driven stop. The job that fired this
                    // turn is NOT deleted — the next fire re-budgets, so the
                    // message says so instead of suggesting a config change.
                    terminal_reason = Some("budget_exhausted");
                    events.push(AgentEvent::Done(format!(
                        "已在定时任务预算 {} 轮工具调用后暂停，已完成的工作已保存。定时任务未被删除，下次触发时会重新获得预算。",
                        effective_max_turns
                    )));
                    break;
                } else {
                    warn!(
                        "[AgentLoop] paused after {} tool-call rounds (grace round exhausted)",
                        effective_max_turns
                    );
                    terminal_reason = Some("max_turns");
                    events.push(AgentEvent::Done(format!(
                        "已在 {} 轮工具调用后暂停，已完成的工作已保存。发送下一条消息可继续，或调大 max_tool_iterations（设为 0 表示不限）。",
                        effective_max_turns
                    )));
                    break;
                }
            }

            // I1 (U7): inbox claim — before EVERY LLM call of this turn, take
            // all pending steer messages (next-step) into history as real user
            // messages (persisted: they ARE genuine user input). Placement
            // after the existing history = same position as the time/env
            // injection's protected prefix zone (appended user turn), so the
            // provider prefix stays stable.
            //
            // ROUND-5 EFFICIENCY FIX: claim BEFORE build_messages (it used to
            // run after, so every steered round built the full message list
            // twice — skills catalog scan + instruction-chain file IO + 2
            // sha256s — and threw the first build away). One build, always.
            let steer_batch = self.inbox.claim_next_step(&context.session_key);
            if !steer_batch.is_empty() {
                for m in &steer_batch {
                    // L4 (full review) + round-5: strip the marker via the
                    // SINGLE shared rule (inbox::strip_steer_marker) — it is a
                    // ROUTING signal, not content, and the same message must
                    // arrive marker-free whether injected in-turn (here) or
                    // replayed post-turn (drain path).
                    let content = crate::inbox::strip_steer_marker(&m.msg.content).to_string();
                    instance.add_user_message(&content);
                    crate::chat_log::append_chat_log(
                        &context.session_key,
                        "user",
                        &format!("[steer] {}", content),
                    );
                    info!(
                        "[AgentLoop] steer message injected before LLM call: session_key={}, len={}",
                        context.session_key,
                        m.msg.content.len()
                    );
                    if log_boundaries {
                        crate::chat_log::append_boundary_event(
                            &context.session_key,
                            "steer_injected",
                            &format!("len={}", m.msg.content.len()),
                        );
                    }
                }
            }

            // P3.1 (sixth batch): auto-inject memory prefetch — async search
            // against the CURRENT (latest) user message, done OUTSIDE
            // build_messages (which is sync; search is async). Per round: the
            // latest user message changes when steer messages land, so
            // re-prefetching per LLM round keeps the section in sync with
            // what the model is about to see. Off (default) ⇒ None ⇒ the
            // build is byte-identical to pre-P3.1.
            let memory_hits: Option<Vec<String>> = self
                .prefetch_memory_context(instance)
                .await;

            // Build the message list from instance history (AFTER the steer
            // claim so injected turns are already included).
            //
            // T8 (U9 ②): the annotated build + the injection records below
            // form this round's projection ledger — everything a later
            // byte-exact replay needs beyond the session store (the
            // transient injections are never persisted). See `crate::replay`.
            let (mut messages, build_annotation) =
                self.build_messages_with_memory_annotated(instance, memory_hits.as_deref());
            let mut replay_injections: Vec<crate::replay::InjectionRecord> = Vec::new();
            if let Some(idx) = build_annotation.digest_index {
                replay_injections.push(crate::replay::InjectionRecord {
                    index: idx,
                    role: messages[idx].role.clone(),
                    source: crate::replay::INJECTION_CONTEXT_DIGEST.to_string(),
                    content: messages[idx].content.clone(),
                });
            }
            let mut replay_voice: Option<crate::replay::VoiceAppend> = None;

            // Voice playback prompt injection: append to last user message (not stored in history).
            if voice_playback {
                if let Some(pos) = messages.iter().rposition(|m| m.role == "user") {
                    messages[pos].content.push_str(VOICE_PLAYBACK_SUFFIX);
                    replay_voice = Some(crate::replay::VoiceAppend {
                        index: pos,
                        suffix: VOICE_PLAYBACK_SUFFIX.to_string(),
                    });
                }
            }

            // ② Grace-round nudge. Transient — NOT persisted to instance history
            // or session_log; only this turn's message list carries it.
            if grace_round {
                messages.push(LlmMessage {
                    role: "system".to_string(),
                    content: GRACE_ROUND_NUDGE.to_string(),
                    tool_calls: None,
                    tool_call_id: None,
                    reasoning_content: None,
                });
                replay_injections.push(crate::replay::InjectionRecord {
                    index: messages.len() - 1,
                    role: "system".to_string(),
                    source: crate::replay::INJECTION_GRACE_NUDGE.to_string(),
                    content: GRACE_ROUND_NUDGE.to_string(),
                });
            }

            // ⑦ Re-inject a pending degenerate-answer nudge (transient, like the
            // grace nudge — never persisted to instance history / session_log).
            if let Some(nudge) = &degenerate_nudge_pending {
                messages.push(LlmMessage {
                    role: "user".to_string(),
                    content: nudge.clone(),
                    tool_calls: None,
                    tool_call_id: None,
                    reasoning_content: None,
                });
                replay_injections.push(crate::replay::InjectionRecord {
                    index: messages.len() - 1,
                    role: "user".to_string(),
                    source: crate::replay::INJECTION_DEGENERATE_NUDGE.to_string(),
                    content: nudge.clone(),
                });
            }

            // ⑧ Re-inject a pending prose-repetition nudge (transient).
            if let Some(nudge) = &repetition_nudge_pending {
                messages.push(LlmMessage {
                    role: "system".to_string(),
                    content: nudge.clone(),
                    tool_calls: None,
                    tool_call_id: None,
                    reasoning_content: None,
                });
                replay_injections.push(crate::replay::InjectionRecord {
                    index: messages.len() - 1,
                    role: "system".to_string(),
                    source: crate::replay::INJECTION_REPETITION_NUDGE.to_string(),
                    content: nudge.clone(),
                });
            }

            debug!("[AgentLoop] Sending {} messages to LLM", messages.len());

            // K1b (U14): LLM-call-level pre hooks. Runs AFTER messages are
            // built (nudges included) and BEFORE the LlmRequest observer
            // event — appended messages land in request_log and in the T8
            // replay ledger (byte-exact replay keeps holding).
            {
                let llm_hooks = self.llm_hooks.read().snapshot();
                if !llm_hooks.is_empty() {
                    let hook_call = crate::hooks::HookLlmCall {
                        model: self.active_model.read().clone(),
                        session_key: context.session_key.clone(),
                        round: turns_used as usize + 1,
                    };
                    match crate::hooks::run_llm_pre_hooks(&llm_hooks, &hook_call, &messages).await {
                        Ok(appended) => {
                            for m in appended {
                                replay_injections.push(crate::replay::InjectionRecord {
                                    index: messages.len(),
                                    role: m.role.clone(),
                                    source: crate::replay::INJECTION_LLM_HOOK.to_string(),
                                    content: m.content.clone(),
                                });
                                messages.push(m);
                            }
                        }
                        Err(reason) => {
                            warn!(
                                "[AgentLoop] LLM hook blocked the call, turns_used={}: {}",
                                turns_used, reason
                            );
                            events.push(AgentEvent::Done(format!(
                                "⛔ HOOK BLOCKED: {} — A registered LLM hook denied this round. Do NOT retry unless the user changes the hook policy.",
                                reason
                            )));
                            break;
                        }
                    }
                }
            }

            // Build tool definitions from registered tools for LLM function calling.
            // Mirrors Go's ToolRegistry.ToProviderDefs() which calls tool.Description() and tool.Parameters().
            // Sort by name so the order is stable across runs — a deterministic
            // tool order gives reproducible behaviour and avoids unnecessary prompt
            // variation between requests.
            // Y1 (Phase4-a): fold AFTER the tier filter — description text only,
            // byte-identical passthrough whenever folding is off/degrades.
            let tool_defs: Vec<crate::types::ToolDefinition> =
                self.apply_tool_doc_folding(self.build_tool_defs(), instance);
            debug!(
                "[AgentLoop] Sending {} tool definitions to LLM",
                tool_defs.len()
            );

            // Emit LLM request observer event.
            let msg_values: Vec<serde_json::Value> = messages
                .iter()
                .filter_map(|m| serde_json::to_value(m).ok())
                .collect();
            let tool_values: Vec<serde_json::Value> = tool_defs
                .iter()
                .filter_map(|t| serde_json::to_value(t).ok())
                .collect();
            // Extract model string before emit so RwLockReadGuard doesn't span the await.
            let active_model = self.active_model.read().clone();
            self.emit_observer_sync(crate::loop_executor::ObserverEvent::LlmRequest {
                trace_id: trace_id.to_string(),
                round: turns_used + 1,
                model: active_model.clone(),
                messages_count: messages.len(),
                tools_count: tool_defs.len(),
                messages: msg_values,
                tools: tool_values,
                provider_name: String::new(),
                api_key: String::new(),
                api_base: String::new(),
            })
            .await;

            // Call LLM.
            instance.set_state(crate::types::AgentState::Thinking);
            let round_start = std::time::Instant::now();
            // Clone provider Arc so RwLock guard is dropped before .await.
            let active_provider = self.provider.read().clone();

            // 订阅急停状态——LLM 调用进行中若触发急停，能即时打断：select 命中
            // estop arm 后，chat future 被 drop → reqwest 取消在途 HTTP 请求。
            // `None`（未接线）时该 arm 永不 resolve（pending），等价于没这条 arm。
            // 注意：subscribe() 返回的是 owned Receiver（不借用 guard），所以这里
            // 拿完就能放掉 estop 的读锁。
            let mut estop_rx = self.estop.read().as_ref().map(|e| e.subscribe());

            // I3 (U9): durable llm_request marker (model + size estimate,
            // no bodies). Heartbeat/internal-channel exemption (see
            // turn_start).
            //
            // T8 (U9 ②): projection-ledger sidecar for this round — the
            // durable record of every non-persisted injection (full bodies),
            // enabling byte-exact replay from the session store. Same
            // cron/heartbeat/internal exemption as the marker above: those
            // turns recur forever and would grow the ledger unboundedly.
            if log_boundaries {
                crate::chat_log::append_boundary_event(
                    &context.session_key,
                    "llm_request",
                    &format!(
                        "model={} messages={} turns_used={}",
                        self.active_model.read(),
                        messages.len(),
                        turns_used
                    ),
                );
                crate::replay::append_projection_record(&crate::replay::RequestProjectionRecord {
                    trace_id: trace_id.to_string(),
                    session_key: context.session_key.clone(),
                    round: turns_used as usize + 1,
                    ts: crate::replay::now_rfc3339(),
                    messages_count: messages.len(),
                    roles: messages.iter().map(|m| m.role.clone()).collect(),
                    history_len_at_build: build_annotation.history_len,
                    injections: replay_injections,
                    voice_append: replay_voice,
                    summary_as_of: build_annotation.summary_as_of.clone(),
                });
            }

            // Use tokio::select! to allow cancellation / e-stop during the LLM call.
            let chat_result = tokio::select! {
                result = active_provider.chat(&active_model, messages, Some(chat_opts.clone()), tool_defs) => result,
                _ = cancel_token.cancelled() => {
                    info!("[AgentLoop] LLM call cancelled while waiting for response, turns_used={}", turns_used);
                    events.push(AgentEvent::Done("已取消".to_string()));
                    break;
                }
                // 急停：watch 翻成 engaged 才 resolve。（K1b 起该等待逻辑抽成
                // wait_estop_engaged 共享——hook 重呼路径用同一段，防漂移。）
                _ = Self::wait_estop_engaged(estop_rx.as_mut()) => {
                    info!(
                        "[AgentLoop] E-stop engaged during LLM call, turns_used={}",
                        turns_used
                    );
                    events.push(AgentEvent::Done(
                        "⛔ 已急停 (E-STOP) — LLM 调用已中断。发送 `nemesisbot estop --release` 恢复。"
                            .to_string(),
                    ));
                    break;
                }
            };

            let mut response = match chat_result {
                Ok(resp) => resp,
                Err(err) => {
                    let err_lower = err.to_lowercase();
                    let is_context_error = ["token", "context", "length", "invalid"]
                        .iter()
                        .any(|keyword| err_lower.contains(keyword));

                    if is_context_error {
                        // Mirrors Go's retry-with-compression logic (loop_executor.go).
                        // Attempt up to 2 retries with progressive history compression.
                        let mut retry_count = 0u32;
                        let max_retries = 2u32;
                        let mut retry_err = err.clone();
                        let mut got_response = None;

                        // Notify user about compression.
                        info!(
                            "[AgentLoop] LLM context error, attempting compression and retry: {}",
                            retry_err
                        );

                        while retry_count < max_retries {
                            retry_count += 1;

                            // Force-compress: advance the summary cache (tail → SMALL_K_FORCE,
                            // then → 0 on a second pass). History is not mutated.
                            self.force_compression(instance).await;

                            // Rebuild messages from compressed history.
                            let mut compressed_messages = self.build_messages(instance);

                            // Re-apply voice playback prompt after compression.
                            if voice_playback {
                                if let Some(last_user) = compressed_messages
                                    .iter_mut()
                                    .rev()
                                    .find(|m| m.role == "user")
                                {
                                    last_user.content.push_str("（语音播报模式已开启，请用简洁、便于口语播报的方式回复，避免使用代码块、表格等不适合语音的内容。）");
                                }
                            }
                            debug!(
                                "[AgentLoop] Retry {}: sending {} messages after compression",
                                retry_count,
                                compressed_messages.len()
                            );

                            let retry_tool_defs: Vec<crate::types::ToolDefinition> = self
                                .tools
                                .read()
                                .iter()
                                .map(|(name, tool)| crate::types::ToolDefinition {
                                    tool_type: "function".to_string(),
                                    function: crate::types::ToolFunctionDef {
                                        name: name.clone(),
                                        description: tool.description(),
                                        parameters: tool.parameters(),
                                    },
                                })
                                .collect();

                            match active_provider
                                .chat(
                                    &active_model,
                                    compressed_messages,
                                    Some(chat_opts.clone()),
                                    retry_tool_defs,
                                )
                                .await
                            {
                                Ok(resp) => {
                                    got_response = Some(resp);
                                    break;
                                }
                                Err(e) => {
                                    retry_err = e;
                                    warn!(
                                        "[AgentLoop] LLM retry {} failed: {}",
                                        retry_count, retry_err
                                    );
                                }
                            }
                        }

                        match got_response {
                            Some(resp) => resp,
                            None => {
                                warn!("[AgentLoop] All LLM retries exhausted: {}", retry_err);
                                let error_round = turns_used + 1;
                                let error_duration = round_start.elapsed();
                                self.emit_observer_sync(
                                    crate::loop_executor::ObserverEvent::LlmResponse {
                                        trace_id: trace_id.to_string(),
                                        round: error_round,
                                        duration_ms: error_duration.as_millis() as u64,
                                        has_tool_calls: false,
                                        content: format!("Error: {}", retry_err),
                                        tool_calls: vec![],
                                        tool_calls_count: 0,
                                        finish_reason: Some("error".to_string()),
                                        usage: None,
                                        raw_request_body: None,
                                        raw_response_body: None,
                                    },
                                )
                                .await;
                                instance.add_assistant_message(
                                    &format!("Error: {}", retry_err),
                                    Vec::new(),
                                    None,
                                );
                                // [capture] LLM retries exhausted (context-error
                                // retry path). Flush the full retry_err — the raw
                                // provider error, likely source of the user-visible
                                // "tools" wording — plus trace_id to correlate with
                                // request_logs/' now-complete failed round (组1).
                                if let Some(sink) = crate::capture_sink::CaptureSink::global() {
                                    sink.flush(
                                        &context.session_key,
                                        "llm_retry_exhausted",
                                        Some(trace_id),
                                        Some(retry_err.as_str()),
                                    );
                                }
                                let formatted =
                                    context.format_rpc_message(&format!("Error: {}", retry_err));
                                events.push(AgentEvent::Error(formatted));
                                break;
                            }
                        }
                    } else {
                        // ③ Transient-error retry (network / stream / 5xx). Retries
                        // do NOT consume turns_used — the per-iteration increment
                        // below happens once regardless of how many retries it took
                        // to get a successful response. Messages + tool_defs are
                        // rebuilt fresh because the first-attempt values were moved
                        // into the failed call.
                        let is_transient_error = [
                            "timeout",
                            "timed out",
                            "connection reset",
                            "broken pipe",
                            "connect error",
                            "connection refused",
                            "temporarily unavailable",
                            "reset by peer",
                            "502",
                            "503",
                            "504",
                            "service unavailable",
                        ]
                        .iter()
                        .any(|k| err_lower.contains(k));

                        let mut last_err = err.clone();
                        let mut maybe_resp: Option<LlmResponse> = None;

                        if is_transient_error {
                            info!(
                                "[AgentLoop] LLM transient error, retrying up to {} times: {}",
                                MAX_TRANSIENT_RETRIES, last_err
                            );
                            let mut retries = 0u32;
                            while retries < MAX_TRANSIENT_RETRIES {
                                retries += 1;
                                let r_msgs = self.build_messages(instance);
                                let r_tools: Vec<crate::types::ToolDefinition> = self
                                    .tools
                                    .read()
                                    .iter()
                                    .map(|(name, tool)| crate::types::ToolDefinition {
                                        tool_type: "function".to_string(),
                                        function: crate::types::ToolFunctionDef {
                                            name: name.clone(),
                                            description: tool.description(),
                                            parameters: tool.parameters(),
                                        },
                                    })
                                    .collect();
                                match active_provider
                                    .chat(&active_model, r_msgs, Some(chat_opts.clone()), r_tools)
                                    .await
                                {
                                    Ok(resp) => {
                                        maybe_resp = Some(resp);
                                        break;
                                    }
                                    Err(e) => {
                                        last_err = e;
                                        warn!(
                                            "[AgentLoop] transient retry {}/{} failed: {}",
                                            retries, MAX_TRANSIENT_RETRIES, last_err
                                        );
                                    }
                                }
                            }
                        }

                        if let Some(resp) = maybe_resp {
                            resp
                        } else {
                            // Non-transient error, or transient retries exhausted.
                            warn!("[AgentLoop] LLM call failed: {}", last_err);
                            let error_round = turns_used + 1;
                            let error_duration = round_start.elapsed();
                            self.emit_observer_sync(
                                crate::loop_executor::ObserverEvent::LlmResponse {
                                    trace_id: trace_id.to_string(),
                                    round: error_round,
                                    duration_ms: error_duration.as_millis() as u64,
                                    has_tool_calls: false,
                                    content: format!("Error: {}", last_err),
                                    tool_calls: vec![],
                                    tool_calls_count: 0,
                                    finish_reason: Some("error".to_string()),
                                    usage: None,
                                    raw_request_body: None,
                                    raw_response_body: None,
                                },
                            )
                            .await;
                            instance.add_assistant_message(
                                &format!("Error: {}", last_err),
                                Vec::new(),
                                None,
                            );
                            // [capture] Non-transient error or transient retries
                            // exhausted. Flush full last_err + trace_id.
                            if let Some(sink) = crate::capture_sink::CaptureSink::global() {
                                sink.flush(
                                    &context.session_key,
                                    "llm_call_failed",
                                    Some(trace_id),
                                    Some(last_err.as_str()),
                                );
                            }
                            let formatted =
                                context.format_rpc_message(&format!("Error: {}", last_err));
                            events.push(AgentEvent::Error(formatted));
                            break;
                        }
                    }
                }
            };

            // K1b (U14): LLM-call-level post hooks — the「拦思考」layer. Runs
            // AFTER the built-in error recovery, BEFORE the LlmResponse
            // observer event / turns_used increment, so every downstream
            // consumer (observer, usage, tool execution) sees the final
            // decision. Retry re-calls carry the same cancel/e-stop select
            // guard and emit their own LlmRequest observer event (visible in
            // request_log; no extra T8 ledger record — same shape as the
            // built-in transient retries).
            {
                let llm_hooks = self.llm_hooks.read().snapshot();
                if !llm_hooks.is_empty() {
                    let mut hook_retries: u32 = 0;
                    loop {
                        let hook_call = crate::hooks::HookLlmCall {
                            model: active_model.clone(),
                            session_key: context.session_key.clone(),
                            round: turns_used as usize + 1,
                        };
                        // Pass a clone: on fail-open paths (budget exhausted /
                        // retry call failed) `response` still holds the
                        // original and needs no restore.
                        match crate::hooks::run_llm_post_hooks(
                            &llm_hooks,
                            &hook_call,
                            response.clone(),
                        )
                        .await
                        {
                            crate::hooks::PostLlmOutcome::Allow(final_resp) => {
                                response = final_resp;
                                break;
                            }
                            crate::hooks::PostLlmOutcome::Block { reason } => {
                                warn!(
                                    "[AgentLoop] LLM hook blocked the response, turns_used={}: {}",
                                    turns_used, reason
                                );
                                events.push(AgentEvent::Done(format!(
                                    "⛔ HOOK BLOCKED: {} — A registered LLM hook terminated this round. Inform the user.",
                                    reason
                                )));
                                break 'turn;
                            }
                            crate::hooks::PostLlmOutcome::Retry { reason } => {
                                if hook_retries >= crate::hooks::MAX_LLM_HOOK_RETRIES {
                                    warn!(
                                        "[AgentLoop] LLM hook retry budget exhausted ({}), allowing the previous response",
                                        crate::hooks::MAX_LLM_HOOK_RETRIES
                                    );
                                    break;
                                }
                                hook_retries += 1;
                                // Regenerate: rebuild messages (the first
                                // attempt's were moved into the call), append
                                // the hook feedback, re-call under the same
                                // cancel/e-stop guard.
                                let mut r_msgs = self.build_messages(instance);
                                r_msgs.push(LlmMessage {
                                    role: "system".to_string(),
                                    content: format!("# Hook feedback: {}", reason),
                                    tool_calls: None,
                                    tool_call_id: None,
                                    reasoning_content: None,
                                });
                                // Y1 (Phase4-a): fold the retry call's defs with
                                // the same gates/rendering as the main call —
                                // same query ⇒ same fold bytes.
                                let r_tools = self.apply_tool_doc_folding(
                                    self.build_tool_defs(),
                                    instance,
                                );
                                self.emit_observer_sync(
                                    crate::loop_executor::ObserverEvent::LlmRequest {
                                        trace_id: trace_id.to_string(),
                                        round: turns_used + 1,
                                        model: active_model.clone(),
                                        messages_count: r_msgs.len(),
                                        tools_count: r_tools.len(),
                                        messages: r_msgs
                                            .iter()
                                            .filter_map(|m| serde_json::to_value(m).ok())
                                            .collect(),
                                        tools: r_tools
                                            .iter()
                                            .filter_map(|t| serde_json::to_value(t).ok())
                                            .collect(),
                                        provider_name: String::new(),
                                        api_key: String::new(),
                                        api_base: String::new(),
                                    },
                                )
                                .await;
                                let mut r_estop_rx =
                                    self.estop.read().as_ref().map(|e| e.subscribe());
                                let r = tokio::select! {
                                    res = active_provider.chat(&active_model, r_msgs, Some(chat_opts.clone()), r_tools) => res,
                                    _ = cancel_token.cancelled() => {
                                        info!("[AgentLoop] Hook retry call cancelled, turns_used={}", turns_used);
                                        events.push(AgentEvent::Done("已取消".to_string()));
                                        break 'turn;
                                    }
                                    _ = Self::wait_estop_engaged(r_estop_rx.as_mut()) => {
                                        info!(
                                            "[AgentLoop] E-stop engaged during hook retry call, turns_used={}",
                                            turns_used
                                        );
                                        events.push(AgentEvent::Done(
                                            "⛔ 已急停 (E-STOP) — LLM 调用已中断。发送 `nemesisbot estop --release` 恢复。"
                                                .to_string(),
                                        ));
                                        break 'turn;
                                    }
                                };
                                match r {
                                    Ok(resp) => {
                                        info!(
                                            "[AgentLoop] Hook retry {} succeeded, re-checking response",
                                            hook_retries
                                        );
                                        response = resp;
                                        continue;
                                    }
                                    Err(e) => {
                                        // Fail-open: keep the response the hook
                                        // rejected — a failed re-call must not
                                        // lose the round's only answer.
                                        warn!(
                                            "[AgentLoop] Hook retry call failed, keeping previous response: {}",
                                            e
                                        );
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            turns_used += 1;

            // Emit LLM response observer event.
            let round_duration = round_start.elapsed();
            let tc_values: Vec<serde_json::Value> = response
                .tool_calls
                .iter()
                .filter_map(|tc| serde_json::to_value(tc).ok())
                .collect();
            let tc_count = response.tool_calls.len();
            self.emit_observer_sync(crate::loop_executor::ObserverEvent::LlmResponse {
                trace_id: trace_id.to_string(),
                round: turns_used,
                duration_ms: round_duration.as_millis() as u64,
                has_tool_calls: !response.tool_calls.is_empty(),
                content: response.content.clone(),
                tool_calls: tc_values,
                tool_calls_count: tc_count,
                finish_reason: if response.finished {
                    Some("stop".to_string())
                } else {
                    None
                },
                usage: response.usage.clone(),
                raw_request_body: response.raw_request_body.take(),
                raw_response_body: response.raw_response_body.take(),
            })
            .await;

            // Record usage statistics if data store is available.
            if let Some(ref ds) = self.data_store {
                if let Some(ref usage) = response.usage {
                    let log = nemesis_data::RequestLog {
                        id: 0,
                        trace_id: trace_id.to_string(),
                        model: self.active_model.read().clone(),
                        provider_type: String::new(),
                        input_tokens: usage.prompt_tokens,
                        output_tokens: usage.completion_tokens,
                        cache_creation_tokens: usage.cache_creation_tokens.unwrap_or(0),
                        cache_read_tokens: usage
                            .cache_read_tokens
                            .or(usage.cached_tokens)
                            .unwrap_or(0),
                        total_cost_usd: 0.0,
                        latency_ms: round_duration.as_millis() as i64,
                        status_code: if response.content.starts_with("Error:") {
                            500
                        } else {
                            200
                        },
                        error_message: None,
                        is_streaming: false,
                        created_at: chrono::Local::now().timestamp(),
                    };
                    if let Err(e) = ds.insert_request_log(&log) {
                        tracing::warn!("[AgentLoop] Failed to record usage: {e}");
                    }
                }
            }

            // Continue-generation on max_tokens truncation (内部专项/nanobot
            // pattern). When completion hits the cap, output is cut mid-way —
            // often mid tool-call JSON, which args_validator would report as
            // "Arguments are not valid JSON" and burn the validation budget
            // (Big tier = 0 retries → instant force-stop with a misleading
            // error). Detect it here: drop the truncated tool calls (executing
            // them would write a partial file / run half-formed args), keep
            // partial content, append a "continue" prompt, re-loop. Bounded so
            // a genuinely too-large file surfaces a clear error, not a loop.
            // NOTE: detection assumes chat_opts.max_tokens is Some — the agent
            // always sets Some(8192) when building chat_opts. If that ever
            // becomes None (provider's own default cap), gate on is_some():
            // the 8192 fallback could false-positive against a higher cap.
            let token_cap = chat_opts.max_tokens.unwrap_or(8192) as u64;
            let hit_cap = response
                .usage
                .as_ref()
                .map(|u| (u.completion_tokens as u64) >= token_cap)
                .unwrap_or(false);
            if hit_cap {
                if length_continuations < MAX_LENGTH_CONTINUATIONS {
                    length_continuations += 1;
                    warn!(
                        "[AgentLoop] response truncated at max_tokens cap ({token_cap}); \
                         continue-generation {length_continuations}/{MAX_LENGTH_CONTINUATIONS}"
                    );
                    instance.add_assistant_message(
                        &response.content,
                        Vec::new(),
                        response.reasoning_content.clone(),
                    );
                    instance.add_user_message(
                        "Output limit reached. Continue exactly where you left off — \
                         no recap, no apology. If you were writing a large file, \
                         break it into smaller writes.",
                    );
                    continue;
                }
                // Budget exhausted: clear, non-misleading error.
                warn!(
                    "[AgentLoop] length-continuation budget exhausted; \
                     output keeps exceeding max_tokens ({token_cap})"
                );
                let notice = format!(
                    "输出反复超过 max_tokens 上限（{token_cap}）被截断，文件可能太大。\
                     请调大 max_tokens，或让我分段写入。"
                );
                instance.add_assistant_message(&notice, Vec::new(), None);
                events.push(AgentEvent::Error(context.format_rpc_message(&notice)));
                break;
            }
            // Complete (non-truncated) response — reset the counter.
            length_continuations = 0;

            // ⑧ Cross-round prose repetition: if the model's content is
            // near-identical to the previous round's, queue a transient nudge
            // for the next build. Catches "saying the same thing while churning
            // tools" — a loop ⑥ cannot see (it watches tool results, not prose).
            if let Some(nudge) = turn_guard.check_text_repetition(&response.content) {
                info!("[AgentLoop] loop guard: response content repeating across rounds; nudging");
                repetition_nudge_pending = Some(nudge);
            } else {
                repetition_nudge_pending = None;
            }

            if response.tool_calls.is_empty() || response.finished {
                // No tool calls: candidate final response. ⑦ Check for degenerate
                // (empty / whitespace-only / reasoning-only) content and nudge
                // the model to retry before accepting. Skipped for heartbeat —
                // an empty heartbeat response means "nothing to do", a valid
                // outcome, not a broken answer.
                let content = response.content.clone();
                if context.user == "heartbeat" {
                    instance.add_assistant_message(
                        &content,
                        Vec::new(),
                        response.reasoning_content.clone(),
                    );
                    let formatted = context.format_rpc_message(&content);
                    events.push(AgentEvent::Done(formatted));
                    break;
                }
                match turn_guard.check_final_answer(&content) {
                    crate::turn_guard::FinalAnswerVerdict::Accept => {
                        instance.add_assistant_message(
                            &content,
                            Vec::new(),
                            response.reasoning_content.clone(),
                        );
                        // I1 (U7) turn escape hatch: the model is about to
                        // finish, but an unclaimed steer message arrived in
                        // the last moments — hand it to the model for one
                        // more round instead of answering past it (dsh
                        // turn-stopping semantics: pending next-step input
                        // keeps the turn open). At most once per turn
                        // (steer_escape_used) so `!`-spam cannot loop the
                        // turn forever.
                        if !steer_escape_used
                            && self.inbox.has_next_step(&context.session_key)
                            && self.concurrent_mode == ConcurrentMode::Steer
                        {
                            steer_escape_used = true;
                            info!(
                                "[AgentLoop] escape hatch: pending steer at turn end, one more round"
                            );
                            // Loop again — the claim at the top of the next
                            // iteration injects the steer message(s).
                            continue;
                        }
                        // K2 (U14): turn-end lifecycle hooks (CC Stop
                        // dialect event). Runs after the assistant message
                        // is recorded, before the Done event. `Continue`
                        // injects the hook feedback as a user message and
                        // grants one more round, bounded by
                        // MAX_TURN_END_CONTINUES (exhausted → stop anyway,
                        // fail-open). Only the normal Accept path fires —
                        // heartbeat (its own branch above), GiveUp and
                        // error/stop paths do not.
                        {
                            let lifecycle = self.lifecycle_hooks.read().snapshot();
                            if !lifecycle.is_empty() {
                                let end = crate::hooks::HookTurnEnd {
                                    session_key: context.session_key.clone(),
                                    channel: context.channel.clone(),
                                    chat_id: context.chat_id.clone(),
                                    final_content: content.clone(),
                                    stop_hook_active: turn_end_continues > 0,
                                };
                                if let crate::hooks::TurnEndDecision::Continue { feedback } =
                                    crate::hooks::run_turn_end_hooks(&lifecycle, &end).await
                                {
                                    if turn_end_continues < crate::hooks::MAX_TURN_END_CONTINUES {
                                        turn_end_continues += 1;
                                        info!(
                                            "[AgentLoop] turn-end hook blocked stopping \
                                             ({}/{}, session '{}') — one more round",
                                            turn_end_continues,
                                            crate::hooks::MAX_TURN_END_CONTINUES,
                                            context.session_key
                                        );
                                        instance.add_user_message(&feedback);
                                        continue 'turn;
                                    }
                                    warn!(
                                        "[AgentLoop] turn-end hook keeps blocking stop after {} \
                                         continues; stopping anyway (fail-open), session '{}'",
                                        crate::hooks::MAX_TURN_END_CONTINUES,
                                        context.session_key
                                    );
                                }
                            }
                        }
                        let formatted = context.format_rpc_message(&content);
                        events.push(AgentEvent::Done(formatted));
                        break;
                    }
                    crate::turn_guard::FinalAnswerVerdict::RetryWithNudge(nudge) => {
                        warn!(
                            "[AgentLoop] degenerate final answer (empty/no visible text); nudging retry"
                        );
                        // Record the empty attempt in history, then queue the
                        // nudge for transient re-injection on the next build.
                        instance.add_assistant_message(
                            &content,
                            Vec::new(),
                            response.reasoning_content.clone(),
                        );
                        degenerate_nudge_pending = Some(nudge);
                        continue;
                    }
                    crate::turn_guard::FinalAnswerVerdict::GiveUp(notice) => {
                        warn!(
                            "[AgentLoop] degenerate final answer retry budget exhausted; giving up"
                        );
                        instance.add_assistant_message(&notice, Vec::new(), None);
                        let formatted = context.format_rpc_message(&notice);
                        events.push(AgentEvent::Done(formatted));
                        break;
                    }
                }
            }

            // Model produced tool calls → it is making progress. Clear any
            // pending degenerate-answer nudge (⑦) so it stops nagging while the
            // model works — tool work is the opposite of a degenerate empty
            // final answer.
            degenerate_nudge_pending = None;

            // Record the assistant's response with tool calls.
            let tool_calls = response.tool_calls.clone();
            let assistant_content = response.content.clone();
            instance.add_assistant_message(
                &assistant_content,
                tool_calls.clone(),
                response.reasoning_content.clone(),
            );
            events.push(AgentEvent::ToolCall(tool_calls.clone()));

            // Execute each tool call.
            instance.set_state(crate::types::AgentState::ExecutingTool);
            let mut hit_async = false;
            // Outer-scope turn-stop latch. Set inside the tool-call for-loop
            // (where `break` can only exit the batch, not the outer LLM loop) by
            // ⑥ escalation OR validation-budget exhaustion. Checked after the
            // for-loop to actually end the turn. Without this two-step, those
            // `break`s only stopped the current batch and the model was called
            // again — escalation fired every round without stopping (observed
            // 43× in a deployed test), and "validation stopping loop" was a lie.
            let mut force_stop: Option<AgentEvent> = None;
            // U5 (sixth batch): precompute execution for an ALL-read-only batch
            // (≥2 calls, every tool is_read_only). The for-loop then replays the
            // serial guards on the precomputed results in source order — the
            // audit chain stays ordered = model source order (roadmap risk 3).
            // cluster_rpc/exec/writers are never read-only → this stays None for
            // those batches → the loop below runs byte-identical to pre-U5.
            // `None` also when a cancel/estop is already engaged at batch start
            // (the for-loop's per-item check handles that case unchanged).
            let precomputed: Option<Vec<PrecomputedTool>> = if tool_calls.len() >= 2
                && !cancel_token.is_cancelled()
                && !self
                    .estop
                    .read()
                    .as_ref()
                    .map(|e| e.is_engaged())
                    .unwrap_or(false)
                && tool_calls
                    .iter()
                    .all(|tc| self.tool_is_read_only(&tc.name))
            {
                let pc = self.precompute_readonly_batch(&tool_calls, context).await;
                Some(pc)
            } else {
                None
            };
            // U5: in the parallel path, cancel/estop are NOT re-checked per item
            // — the batch was checkpointed non-cancelled above and runs to
            // completion (goal §四 documented semantic: a cancel arriving during
            // the parallel window takes effect on the NEXT turn, not mid-batch).
            // The serial path (precomputed.is_none()) keeps the per-item checks
            // byte-identical.
            let skip_cancel_estop = precomputed.is_some();
            for (batch_idx, tc) in tool_calls.iter().enumerate() {
                // Check cancellation before each tool execution.
                if !skip_cancel_estop && cancel_token.is_cancelled() {
                    info!(
                        "[AgentLoop] LLM loop cancelled before tool execution: {}, turns_used={}",
                        tc.name, turns_used
                    );
                    events.push(AgentEvent::Done("已取消".to_string()));
                    break;
                }

                // 全局急停检查：触发则拒绝后续工具调用并结束当前轮。
                let estop_engaged = self
                    .estop
                    .read()
                    .as_ref()
                    .map(|e| e.is_engaged())
                    .unwrap_or(false);
                if estop_engaged {
                    info!(
                        "[AgentLoop] E-stop engaged before tool execution: {}, turns_used={}",
                        tc.name, turns_used
                    );
                    events.push(AgentEvent::Done(
                        "⛔ 已急停 (E-STOP) — 工具调用已拒绝。发送 `nemesisbot estop --release` 恢复。"
                            .to_string(),
                    ));
                    break;
                }

                let tool_start = std::time::Instant::now();
                // Phase 2 (small-model-tool-robustness): validate args against
                // the tool's schema before dispatch. Catches B-class failures;
                // auto-fixes high-confidence field-name typos (edit distance ≤2);
                // otherwise bounces a structured error back to the model so it
                // can self-correct on the next round.
                //
                // U5 (sixth batch): when `precomputed` is Some, the execution
                // already ran concurrently (above) — replay its result + the
                // `validation_failures` counter increment here, then fall
                // through to the SAME serial guards (observer/capture/
                // turn_guard/spill/escalation). Guards run in source order
                // because join_all preserves iteration order. `tool_duration`
                // carries the REAL per-task wall time (measured in the pool),
                // not this near-zero clone.
                let (result, tool_duration_ms) = if let Some(ref pc) = precomputed {
                    let p = &pc[batch_idx];
                    if p.validation_failed {
                        validation_failures += 1;
                    } else {
                        validation_failures = 0;
                    }
                    (p.result.clone(), p.duration_ms)
                } else {
                    let r = match self.check_tool_args(tc) {
                        crate::args_validator::Outcome::Valid => {
                            validation_failures = 0;
                            self.handle_tool_call(tc, context).await
                        }
                        crate::args_validator::Outcome::Fixed(fixed_args) => {
                            validation_failures = 0;
                            info!(
                                "[AgentLoop] Auto-fixed args for tool '{}' (id={})",
                                tc.name, tc.id
                            );
                            let mut fixed = tc.clone();
                            fixed.arguments = fixed_args;
                            self.handle_tool_call(&fixed, context).await
                        }
                        crate::args_validator::Outcome::Invalid { message, class } => {
                            validation_failures += 1;
                            warn!(
                                "[AgentLoop] Arg validation failed for tool '{}' (id={}, class={}): {}",
                                tc.name, tc.id, class, message
                            );
                            format!("Tool error: {}", message)
                        }
                    };
                    (r, tool_start.elapsed().as_millis() as u64)
                };
                let tool_duration = std::time::Duration::from_millis(tool_duration_ms);
                let tool_success =
                    !result.starts_with("Error:") && !result.starts_with("Tool error:");

                // Emit tool call observer event.
                self.emit_observer_sync(crate::loop_executor::ObserverEvent::ToolCall {
                    trace_id: trace_id.to_string(),
                    tool_name: tc.name.clone(),
                    success: tool_success,
                    duration_ms: tool_duration.as_millis() as u64,
                    round: turns_used,
                    arguments: tc.arguments.clone(),
                    result: result.clone(),
                })
                .await;

                // [capture] Record the full pre-truncation tool result. loop.rs
                // does NOT truncate tool results before they enter the context,
                // so this is what catches a bloated output blowing out the
                // context window (the suspected bug trigger). No-op unless
                // capture is enabled; flushed only on a later failure signal.
                if let Some(sink) = crate::capture_sink::CaptureSink::global() {
                    sink.record_tool(
                        &context.session_key,
                        crate::capture_sink::ToolCapture {
                            tool_name: tc.name.clone(),
                            arguments: tc.arguments.clone(),
                            result: result.clone(),
                            success: tool_success,
                            duration_ms: tool_duration.as_millis() as u64,
                            error: if tool_success {
                                String::new()
                            } else {
                                result.clone()
                            },
                            llm_round: turns_used as usize,
                            ts: String::new(),
                        },
                    );
                }

                // Check for async cluster_rpc result — save continuation snapshot.
                //
                // Plan C (template-based UX): the cluster_rpc tool encodes the
                // peer's display name as the 4th part of the marker so we can
                // render a human-friendly "waiting" message here without an
                // extra cluster lookup (this crate can't depend on
                // nemesis-cluster). The full LLM-generated persona response
                // was deferred — it would double cross-node latency and
                // complicate the continuation snapshot. See loop_tools.rs
                // for the encoding site.
                //
                // Format: `__ASYNC__:{task_id}:{target_id}:{target_name}`
                // Older senders may omit the name part (3-segment format),
                // in which case we fall back to the bare target_id.
                if result.starts_with("__ASYNC__:") {
                    let parts: Vec<String> = result.splitn(4, ':').map(|s| s.to_string()).collect();
                    if parts.len() >= 3 {
                        let task_id = parts[1].clone();
                        let target_id = parts[2].clone();
                        let target_name = parts
                            .get(3)
                            .cloned()
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| target_id.clone());
                        if let Some(ref mgr) = self.continuation_manager {
                            // Get messages up to this point (including the assistant's tool_call).
                            // We use build_messages() to convert history → LlmMessage format.
                            let messages = self.build_messages(instance);
                            let channel = context.channel.clone();
                            let chat_id = context.chat_id.clone();
                            let session_key = context.session_key.clone();

                            // Save continuation snapshot (spawns a tokio task for disk write)
                            let mgr = mgr.clone();
                            let tc_id = tc.id.clone();
                            let msgs = messages.clone();
                            let task_id_spawn = task_id.clone();
                            tokio::spawn(async move {
                                mgr.save_continuation(
                                    &task_id_spawn,
                                    msgs,
                                    &tc_id,
                                    &channel,
                                    &chat_id,
                                    &session_key,
                                )
                                .await;
                            });

                            info!(
                                "[AgentLoop] Continuation saved for async cluster_rpc: task_id={}, tool_call_id={}",
                                task_id, tc.id
                            );
                        }

                        // Return an intermediate message to the user and stop processing.
                        // The continuation will resume when the callback arrives.
                        //
                        // NOTE: `is_async_done` in nemesisbot/src/cluster_agent.rs detects
                        // this async path via the `__CLUSTER_ASYNC__` marker in conversation
                        // history, NOT by matching this message text. So the wording here
                        // is free to change without breaking multi-hop (A→B→C) detection.
                        //
                        // The template is deliberately persona-agnostic — this code has
                        // no knowledge of which AI identity is currently loaded (IDENTITY.md
                        // is applied at the LLM layer, not here). Address terms like "老爷"
                        // belong in the persona file, not in hardcoded system messages.
                        // The task_id is omitted from user-visible copy — it's an internal
                        // correlation ID with no meaning to the user.
                        let intermediate = format!("已经联系 {} 了，稍等~", target_name);
                        instance.add_tool_result(&tc.id, &format!(
                            "Request accepted by {}. Task ID: {} | __CLUSTER_ASYNC__{{\"task_id\":\"{}\",\"target\":\"{}\"}}",
                            target_id, task_id, task_id, target_id
                        ));

                        let formatted = context.format_rpc_message(&intermediate);
                        events.push(AgentEvent::Done(formatted));
                        hit_async = true;
                        break;
                    }
                }

                let tool_result = ToolCallResult {
                    tool_name: tc.name.clone(),
                    result: result.clone(),
                    is_error: false,
                };
                events.push(AgentEvent::ToolResult(tool_result));

                // ⑤/⑥ Loop guards — mutually exclusive per call (success vs error).
                // Use the shared helper so ExecTool's `Ok("Exit code: N")` for
                // non-zero exits is detected as a failure — otherwise build
                // loops look like success and the guards never fire.
                let tool_succeeded = !crate::turn_guard::tool_result_indicates_error(&result);

                // ⑤ Repeat-success guard: a write-like tool succeeding with
                // identical args is a no-op / write loop → append a nudge.
                //
                // NOTE: keys on `tc.arguments` (the model's ORIGINAL args), not
                // the validator's auto-fixed args. Intentional — if the model
                // keeps re-sending the same (typo'd) args, that IS the repeat
                // we want to catch, regardless of the per-call auto-fix.
                // Detection stays consistent; the signature is the pre-fix form.
                // X1 (U3 projection prune): the pure tool output, kept for
                // history. The ⑤/⑤′/⑥ guard nudges below decorate only the
                // MODEL-facing text; `guard_nudged` marks that a decoration
                // fired (the decorated form cannot be recomputed from the
                // original later, so it must be recorded as the projection
                // override — see the gate block below).
                let original_result = result.clone();
                let mut guard_nudged = false;
                let result = if tool_succeeded {
                    match turn_guard.record_write_success(&tc.name, &tc.arguments) {
                        Some(nudge) => {
                            info!(
                                "[AgentLoop] loop guard: '{}' repeated an identical write; nudging",
                                tc.name
                            );
                            guard_nudged = true;
                            format!("{}\n{}", result, nudge)
                        }
                        None => result,
                    }
                } else {
                    result
                };

                // ⑤′ Read-side repeat guard: a non-write tool succeeding with
                // identical args repeatedly is a re-query loop — the model is
                // not consuming the result it already has. Advisory nudge only
                // (never blocks). Same NOTE as ⑤ above: keys on the model's
                // ORIGINAL args, not the auto-fixed form.
                let result = if tool_succeeded {
                    match turn_guard.record_read_success(&tc.name, &tc.arguments) {
                        Some(nudge) => {
                            info!(
                                "[AgentLoop] loop guard: '{}' repeated an identical read; nudging",
                                tc.name
                            );
                            guard_nudged = true;
                            format!("{}\n{}", result, nudge)
                        }
                        None => result,
                    }
                } else {
                    result
                };

                // G3 (U3) + G4 (U4) + X1 (U3 projection prune): model-free
                // size gates, computed here but applied at the PROJECTION
                // (build_messages), not at history-write time. History keeps
                // `original_result` (the mid-section stays recoverable for
                // history replay / session branching); `gate_text` is the
                // bounded model-facing form the OLD code stored in history —
                // byte-identical to pre-X1 behavior:
                //   >= SPILL_THRESHOLD_CHARS  → spill whole text to disk, keep
                //                               preview + locator (readable back
                //                               via read_file offset/limit).
                //   > MAX_TOOL_RESULT_INLINE_CHARS (and no spill) → head +
                //                               marker + tail prune.
                // Spill is best-effort: a storage failure falls through to the
                // prune tier (a spill failure must never lose a successful
                // tool call's content outright).
                let mut spill_applied = false;
                let gate_text: String = {
                    let spill_root = self.spill_root.read().clone();
                    let spilled = spill_root.as_ref().and_then(|root| {
                        let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S%3f").to_string();
                        match crate::spill::spill_tool_result(
                            &result,
                            &tc.name,
                            root,
                            &context.session_key,
                            &stamp,
                            &tc.id,
                        ) {
                            crate::spill::SpillOutcome::Spilled(text) => Some(text),
                            crate::spill::SpillOutcome::SpillFailed => {
                                warn!(
                                    "[AgentLoop] tool result spill failed for '{}' — falling back to prune tier",
                                    tc.name
                                );
                                None
                            }
                            crate::spill::SpillOutcome::BelowThreshold => None,
                        }
                    });
                    match spilled {
                        Some(text) => {
                            info!(
                                "[AgentLoop] tool result spilled to disk: '{}' result >= {} chars",
                                tc.name,
                                crate::spill::SPILL_THRESHOLD_CHARS
                            );
                            spill_applied = true;
                            text
                        }
                        None => match crate::prune::prune_tool_result(&result, &tc.name) {
                            Some(pruned) => {
                                info!(
                                    "[AgentLoop] tool result pruned: '{}' result exceeded {} chars",
                                    tc.name,
                                    crate::prune::MAX_TOOL_RESULT_INLINE_CHARS
                                );
                                pruned
                            }
                            None => result,
                        },
                    }
                };

                // ⑥ Alternating-loop guard: per-turn (tool, error) failure
                // frequency, NOT reset by intervening successes (also handles ④
                // storm — consecutive identical — internally). On a repeated
                // failure, append a nudge so the model sees it in the error.
                // Signature input is the post-gate text — same as pre-X1.
                let error_for_guard: Option<&str> =
                    if tool_succeeded { None } else { Some(&gate_text) };
                let nudge6 = turn_guard
                    .record_tool_outcome(&tc.name, error_for_guard)
                    .map(|nudge| {
                        info!(
                            "[AgentLoop] loop guard: '{}' repeating the same failure within this turn; nudging",
                            tc.name
                        );
                        nudge
                    });

                // X1: the recorded projection override — only when the final
                // model-facing text cannot be recomputed from the original
                // later: the spill tier (locator path embeds a wall-clock
                // stamp) or any guard-nudge decoration (⑤/⑤′/⑥ — dynamic
                // per-turn state). Otherwise None and build_messages
                // recomputes the pure prune (deterministic, ledger-free).
                let projection: Option<String> =
                    if spill_applied || guard_nudged || nudge6.is_some() {
                        Some(match &nudge6 {
                            Some(nudge) => format!("{}\n{}", gate_text, nudge),
                            None => gate_text.clone(),
                        })
                    } else {
                        None
                    };
                instance.add_tool_result_projected(&tc.id, &original_result, &tc.name, projection);

                // H5 (U18): touch-driven instruction-chain invalidation. A
                // successful read_file/write_file/edit_file may have touched
                // a file on the workspace instruction chain — invalidate the
                // context digests so the next build re-reads the chain.
                // (File-level check only: the re-read happens at injection
                // time. Rare + cheap: only fires for these three tools and
                // only when the path matches a chain file name.)
                if tool_succeeded
                    && matches!(tc.name.as_str(), "read_file" | "write_file" | "edit_file")
                {
                    if let Some(args_val) = serde_json::from_str::<serde_json::Value>(
                        &tc.arguments,
                    )
                    .ok()
                    {
                        if let Some(path_str) =
                            args_val.get("path").and_then(|v| v.as_str())
                        {
                            let touched = std::path::PathBuf::from(path_str);
                            let ws_root = self.workspace_root.read().clone();
                            if let Some(ref root) = ws_root {
                                // Chain files are <dir>/AGENTS.md or CLAUDE.md
                                // under the workspace — check by file name to
                                // avoid re-reading the whole chain on every
                                // file op (the full path_is_on_chain check
                                // happens against the loaded chain at
                                // injection; here the name match is the
                                // conservative trigger).
                                let name = touched
                                    .file_name()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_default();
                                if name == "AGENTS.md" || name == "CLAUDE.md" {
                                    let chain = crate::workspace_instructions::load_instruction_chain(
                                        root, root,
                                    );
                                    if crate::workspace_instructions::path_is_on_chain(
                                        &chain, &touched,
                                    ) {
                                        info!(
                                            "[AgentLoop] instruction-chain file touched: {} — context digest invalidated",
                                            touched.display()
                                        );
                                        self.invalidate_context_digests();
                                    }
                                }
                            }
                        }
                    }
                }

                // ⑥ Escalation: same (tool, error) failed past the hard-stop
                // threshold → nudges are being ignored. Latch a stop event and
                // break the tool batch; the outer-scope check after this for-loop
                // ends the turn (a bare `break` here only exits the batch, not
                // the LLM loop).
                if let Some(msg) = turn_guard.escalation_check() {
                    warn!(
                        "[AgentLoop] loop guard escalation: stopping turn to avoid burning max_turns on a stuck loop"
                    );
                    force_stop = Some(AgentEvent::Done(context.format_rpc_message(&msg)));
                    break;
                }

                // Phase 2: bound consecutive validation failures so a struggling
                // model cannot burn the whole max_turns budget on the same
                // malformed arguments. Same latch pattern as escalation — a bare
                // `break` here used to only exit the batch while the outer LLM
                // loop kept calling the model (the "stopping loop" log was a lie,
                // observed in a deployed cluster test). Now it actually ends the
                // turn, giving the model exactly `validation_retry_budget` retries.
                if validation_failures >= self.validation_retry_budget() {
                    warn!(
                        "[AgentLoop] Validation retry budget exhausted ({}); stopping turn.",
                        validation_failures
                    );
                    force_stop = Some(AgentEvent::Error(format!(
                        "工具参数校验连续失败 {} 次，已停止重试。最近工具：'{}'。",
                        validation_failures, tc.name
                    )));
                    break;
                }
            }

            if hit_async {
                break;
            }

            // Outer-scope turn stop latched from inside the tool-call for-loop
            // (⑥ escalation OR validation-budget exhaustion). A bare `break` in
            // that for-loop only exits the batch; this actually ends the turn,
            // emitting a single terminal event.
            if let Some(ev) = force_stop {
                events.push(ev);
                break;
            }
        }

        instance.set_state(crate::types::AgentState::Idle);

        // I3 (U9): durable turn_end marker with the terminal reason.
        // L2 (full review): the reason comes from the break-site latch
        // (terminal_reason), not from sniffing the Done text — a model
        // reply containing the paused-after wording can no longer be
        // misclassified as max_turns.
        let end_reason = if cancel_token.is_cancelled() {
            "cancelled"
        } else {
            terminal_reason.unwrap_or("done")
        };
        if log_boundaries {
            crate::chat_log::append_boundary_event(&context.session_key, "turn_end", end_reason);
        } else if terminal_reason == Some("budget_exhausted") {
            // T3 (U12): cron turns are exempt from per-turn boundary events
            // (a recurring job would grow the sidecar unboundedly), but a
            // budget-exhausted stop is a rare, one-shot terminal fact worth
            // exactly one marker — the budget's observability requirement.
            crate::chat_log::append_boundary_event(&context.session_key, "turn_end", end_reason);
        }

        events
    }

    // -----------------------------------------------------------------------
    // Tool handling
    // -----------------------------------------------------------------------

    /// U5 (sixth batch): is `name` a registered read-only tool? Looks up the
    /// agent-side registry and asks the tool's `is_read_only()`. Fail-closed:
    /// unknown tools and writer tools return false → never join the parallel
    /// pool. When executor separation is ON, the MOVE_TOOLS (incl. read_file/
    /// list_dir/grep) are `RemoteExecutorTool` instances whose `is_read_only()`
    /// is the default `false` — so an executor-separated batch naturally
    /// falls back to serial here, no separate check needed.
    fn tool_is_read_only(&self, name: &str) -> bool {
        match self.tools.read().get(name) {
            Some(t) => t.is_read_only(),
            None => false,
        }
    }

    /// U5: the result of one parallel-executed read-only call, captured for
    /// serial guard replay in source order. `validation_failed` lets the
    /// serial loop replay the `validation_failures` counter exactly as the
    /// serial path would (Invalid → +1; Valid/Fixed → reset to 0).

    /// U5: concurrently execute an ALL-read-only batch (validated read-only
    /// by the caller). Each task runs the SAME execution path as the serial
    /// loop's match (`check_tool_args` → `handle_tool_call` or synthesize an
    /// Invalid error), gated by a 4-permit semaphore. Returns results in
    /// SOURCE ORDER (`join_all` preserves iteration order) so the serial
    /// guard-replay keeps the audit chain ordered = model source order
    /// (roadmap risk 3 hard constraint). cluster_rpc/exec/writers are never
    /// here (not read-only) → `__ASYNC__` continuation and executor paths
    /// are structurally excluded.
    async fn precompute_readonly_batch(
        &self,
        tool_calls: &[ToolCallInfo],
        context: &RequestContext,
    ) -> Vec<PrecomputedTool> {
        let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
        let futs = tool_calls.iter().map(|tc| {
            let sem = sem.clone();
            let tc = tc.clone();
            async move {
                let _permit = sem.acquire().await.ok();
                let start = std::time::Instant::now();
                let (result, validation_failed) = match self.check_tool_args(&tc) {
                    crate::args_validator::Outcome::Valid => {
                        (self.handle_tool_call(&tc, context).await, false)
                    }
                    crate::args_validator::Outcome::Fixed(fixed_args) => {
                        info!(
                            "[AgentLoop] Auto-fixed args for tool '{}' (id={})",
                            tc.name, tc.id
                        );
                        let mut fixed = tc.clone();
                        fixed.arguments = fixed_args;
                        (self.handle_tool_call(&fixed, context).await, false)
                    }
                    crate::args_validator::Outcome::Invalid { message, .. } => {
                        warn!(
                            "[AgentLoop] Arg validation failed for tool '{}' (id={}): {}",
                            tc.name, tc.id, message
                        );
                        (format!("Tool error: {}", message), true)
                    }
                };
                PrecomputedTool {
                    result,
                    validation_failed,
                    duration_ms: start.elapsed().as_millis() as u64,
                }
            }
        });
        futures::future::join_all(futs).await
    }

    /// Build the LLM-visible tool definitions from the registry.
    ///
    /// Extracted verbatim from `run_llm_loop` (K1b, U14) so the post-LLM
    /// hook retry re-call rebuilds them from the same single source instead
    /// of duplicating the tier-filter/sort block (the transient-retry path
    /// predates the extraction and keeps its inline copy — untouched).
    ///
    /// Mirrors Go's ToolRegistry.ToProviderDefs() which calls
    /// tool.Description() and tool.Parameters(). Sort by name so the order is
    /// stable across runs — a deterministic tool order gives reproducible
    /// behaviour and avoids unnecessary prompt variation between requests.
    fn build_tool_defs(&self) -> Vec<crate::types::ToolDefinition> {
        let tools_guard = self.tools.read();
        // Phase 3 (small-model-tool-robustness): tier-based toolset.
        // Empty allowed-list (Big/Auto) = show everything; Mini/Normal
        // see a restricted set to reduce small-model cognitive load.
        let allowed = nemesis_types::capability::tier_allowed_tools(*self.tier.read());
        let mut names: Vec<&String> = tools_guard.keys().collect();
        names.sort();
        names
            .into_iter()
            .filter(|name| allowed.is_empty() || allowed.contains(&name.as_str()))
            .filter_map(|name| tools_guard.get(name).map(|tool| (name, tool)))
            .map(|(name, tool)| crate::types::ToolDefinition {
                tool_type: "function".to_string(),
                function: crate::types::ToolFunctionDef {
                    name: name.clone(),
                    description: tool.description(),
                    parameters: tool.parameters(),
                },
            })
            .collect()
    }

    /// Y1 (Phase4-a): read `agents.tool_doc_folding` from config.json FRESH
    /// each call (same pattern as [`current_summarizer_prefix_reuse`] — the
    /// dashboard/CLI can flip it while the gateway runs). Absent section,
    /// unreadable file, or a standalone loop (no config_path) →
    /// `(false, default)` — folding off, tool defs byte-identical.
    pub(crate) fn current_tool_doc_folding(&self) -> (bool, usize) {
        const OFF: (bool, usize) = (false, crate::tool_doc_folding::DEFAULT_EXPAND_TOP_N);
        let path = match self.config_path.read().clone() {
            Some(p) => p,
            None => return OFF,
        };
        let v = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
        let Some(v) = v else {
            return OFF;
        };
        let sec = v.get("agents").and_then(|a| a.get("tool_doc_folding"));
        let enabled = sec
            .and_then(|s| s.get("enabled"))
            .and_then(|b| b.as_bool())
            .unwrap_or(false);
        let top_n = sec
            .and_then(|s| s.get("expand_top_n"))
            .and_then(|n| n.as_u64())
            .map(|n| n as usize)
            .unwrap_or(crate::tool_doc_folding::DEFAULT_EXPAND_TOP_N);
        (enabled, top_n)
    }

    /// Y1 (Phase4-a): semantic tool-documentation folding, applied AFTER the
    /// tier filter and ORTHOGONAL to tool supply — folded tools stay
    /// callable with their full parameter schema; only the description text
    /// shrinks. Returns the defs byte-unchanged unless EVERY gate opens:
    /// config enabled, tier != Mini (the 13-tool core set has nothing to
    /// save), a wired memory manager (P3.1 embed backend), a non-empty
    /// latest user message, and successful embeddings for the query and
    /// every tool description (all-or-nothing — a tool that cannot be ranked
    /// must not be folded). Deterministic for a given (descriptions, query,
    /// embed backend) triple, so the two call sites (main call + hook retry
    /// re-call) and any rebuild that re-derives defs render the same bytes.
    fn apply_tool_doc_folding(
        &self,
        defs: Vec<crate::types::ToolDefinition>,
        instance: &AgentInstance,
    ) -> Vec<crate::types::ToolDefinition> {
        #[cfg(feature = "memory")]
        let (enabled, top_n) = self.current_tool_doc_folding();
        #[cfg(not(feature = "memory"))]
        let (enabled, _) = self.current_tool_doc_folding();
        if !enabled {
            return defs;
        }
        if *self.tier.read() == nemesis_types::capability::ModelTier::Mini {
            return defs;
        }
        #[cfg(feature = "memory")]
        {
            // Latest USER message is the ranking signal (same choice as
            // `prefetch_memory_context`).
            let query = instance
                .get_history()
                .iter()
                .rev()
                .find(|t| t.role == "user")
                .map(|t| t.content.clone())
                .unwrap_or_default();
            if query.trim().is_empty() {
                return defs;
            }
            let manager = match self.memory_inject_manager.read().clone() {
                Some(m) => m,
                None => {
                    debug!(
                        "[AgentLoop] tool_doc_folding enabled but no memory manager (embed \
                         backend) wired — leaving docs unfolded"
                    );
                    return defs;
                }
            };
            let query_vec = match manager.embed_text(&query) {
                Some(v) => v,
                None => {
                    debug!(
                        "[AgentLoop] tool_doc_folding: query embedding failed — leaving docs \
                         unfolded"
                    );
                    return defs;
                }
            };
            let mut sims: std::collections::HashMap<String, f32> =
                std::collections::HashMap::with_capacity(defs.len());
            {
                // Cache guard held only for the embed-and-rank pass (the fold
                // render itself touches no shared state).
                let mut cache = self.tool_vec_cache.write();
                for d in &defs {
                    let cached = cache.get(&d.function.name);
                    let vec = match cached {
                        Some((desc, v)) if desc == &d.function.description => v.clone(),
                        _ => match manager.embed_text(&d.function.description) {
                            Some(v) => {
                                cache.insert(
                                    d.function.name.clone(),
                                    (d.function.description.clone(), v.clone()),
                                );
                                v
                            }
                            None => {
                                debug!(
                                    "[AgentLoop] tool_doc_folding: embedding failed for tool \
                                     {} — leaving docs unfolded",
                                    d.function.name
                                );
                                return defs;
                            }
                        },
                    };
                    let sim = crate::tool_doc_folding::cosine(&query_vec, &vec);
                    sims.insert(d.function.name.clone(), sim);
                }
            }
            crate::tool_doc_folding::fold_tool_defs(defs, &sims, top_n)
        }
        #[cfg(not(feature = "memory"))]
        {
            // No embed backend compiled in — folding cannot rank; passthrough.
            let _ = instance;
            defs
        }
    }

    /// Wait until the e-stop watch flips to engaged, or forever if no
    /// receiver is wired (`None` → pending, i.e. the select arm never fires).
    ///
    /// Extracted verbatim from the estop arm of the primary LLM-call select
    /// (K1b, U14) — the post-LLM hook retry re-call needs the identical
    /// subscribe-window-safe wait (if already engaged at subscribe time,
    /// return immediately instead of waiting for the NEXT change).
    async fn wait_estop_engaged(rx: Option<&mut tokio::sync::watch::Receiver<bool>>) {
        match rx {
            Some(rx) => {
                // 订阅时已经 engaged（checkpoint A → subscribe 之间的窗口）
                // → 立刻 return，否则 changed() 会干等下一次变化、漏掉这次。
                if *rx.borrow() {
                    return;
                }
                while rx.changed().await.is_ok() {
                    if *rx.borrow() {
                        return;
                    }
                }
            }
            None => {
                std::future::pending::<()>().await;
            }
        }
    }

    /// Execute a single tool call.
    pub async fn handle_tool_call(
        &self,
        tool_call: &ToolCallInfo,
        context: &RequestContext,
    ) -> String {
        info!(
            "[AgentLoop] Executing tool: {} (id={})",
            tool_call.name, tool_call.id
        );

        // 全局急停：触发时拒绝所有工具分发。这是 handle_tool_call 公开入口级
        // 的防御深度——与 run_llm_loop 里的批次检查点互补，任何调用方都吃到。
        let estop_engaged = self
            .estop
            .read()
            .as_ref()
            .map(|e| e.is_engaged())
            .unwrap_or(false);
        if estop_engaged {
            warn!(
                "[AgentLoop] E-stop engaged — tool {} refused.",
                tool_call.name
            );
            return "⛔ ESTOP: 已急停 — 工具调用已被拒绝 (e-stop engaged). 不要重试；告知用户当前处于急停状态，等待释放。"
                .to_string();
        }

        // Pre-execution security check (mirrors Go's PluginableTool.Execute → PluginManager → SecurityPlugin).
        #[cfg(feature = "security")]
        {
            if let Some(ref security) = self.security_plugin {
                let args_value = serde_json::from_str::<serde_json::Value>(&tool_call.arguments)
                    .unwrap_or(serde_json::Value::Null);
                let invocation = nemesis_security::types::ToolInvocation {
                    tool_name: tool_call.name.clone(),
                    args: args_value,
                    user: String::new(),
                    source: context.channel.clone(),
                    metadata: std::collections::HashMap::new(),
                };
                let (allowed, reason) = security.execute(&invocation);
                if !allowed {
                    let reason_str =
                        reason.unwrap_or_else(|| "operation denied by security policy".to_string());
                    warn!(
                        "[AgentLoop] Security blocked tool {}: {}",
                        tool_call.name, reason_str
                    );
                    // Use a very explicit prefix so the LLM cannot misinterpret this
                    // as a generic error (e.g. "file not found"). The LLM must
                    // understand that the USER or SECURITY POLICY blocked the action.
                    return format!(
                        "⛔ SECURITY BLOCKED: {} — The user or security policy denied this operation. Do NOT retry. Inform the user that the operation was rejected.",
                        reason_str
                    );
                }
                // P5: guardian (LLM safety judge) review for CRITICAL tools. Runs only
                // after the rule layers allow, and only for CRITICAL operations (cost
                // bounded). A Deny verdict blocks; errors/Allow proceed (the guardian
                // only escalates — rules already denied cases returned above).
                if security.is_critical_tool(&tool_call.name) {
                    if let Some(judge) = security.judge() {
                        let req = nemesis_security::guardian::JudgeRequest {
                            action: tool_call.name.clone(),
                            risk_level: "critical".to_string(),
                            transcript: tool_call.arguments.clone(),
                        };
                        if let Ok(v) = judge.judge(&req).await {
                            if v.outcome == nemesis_security::guardian::JudgeOutcome::Deny {
                                warn!(
                                    "[AgentLoop] Guardian denied critical tool {}: {}",
                                    tool_call.name, v.rationale
                                );
                                return format!(
                                    "⛔ GUARDIAN DENIED: {} — The safety judge flagged this critical operation as unsafe. Do NOT retry. Inform the user.",
                                    v.rationale
                                );
                            }
                        }
                    }
                }
            }
        }

        // K1a (U14): user tool hooks. Pre hooks run here — AFTER the fixed
        // security gate above (which stays inline as the de-facto pre[0]; see
        // crate::hooks module doc for why it wasn't converted to a trait
        // object) and BEFORE context injection / checkpoint / execute.
        // Ordered, first Block wins. Fires on every dispatch attempt,
        // including unknown tool names (a hook may deny what the model
        // *tried* to call — mirrors CC PreToolUse).
        let hook_call = crate::hooks::HookToolCall {
            name: tool_call.name.clone(),
            arguments: tool_call.arguments.clone(),
            channel: context.channel.clone(),
            chat_id: context.chat_id.clone(),
            session_key: context.session_key.clone(),
        };
        {
            let hooks = self.tool_hooks.read().snapshot();
            if let Some(reason) = crate::hooks::run_pre_hooks(&hooks, &hook_call).await {
                warn!(
                    "[AgentLoop] Hook blocked tool {}: {}",
                    tool_call.name, reason
                );
                return format!(
                    "⛔ HOOK BLOCKED: {} — A registered hook denied this operation. Do NOT retry unless the user changes the hook policy. Inform the user if this keeps blocking.",
                    reason
                );
            }
        }

        // Inject channel/chat_id into context-aware tools before execution.
        // Mirrors loop_executor.rs:1634 which calls set_context for AgentLoopExecutor.
        {
            let guard = self.tools.read();
            if let Some(tool) = guard.get(&tool_call.name) {
                tool.set_context(&context.channel, &context.chat_id);
            }
        }

        #[cfg(feature = "forge")]
        let tool_start = std::time::Instant::now();
        let tool_opt = self.tools.read().get(&tool_call.name).cloned();
        // Checkpoint capture: if the tool previews a file change, snapshot its
        // pre-edit content (the edit safety net) before execution modifies it.
        // Read-only / non-file tools return None from preview and are skipped.
        if let Some(ref tool) = tool_opt {
            if let Some(change) = tool.preview(&tool_call.arguments) {
                // Drop the read guard before awaiting so the future stays Send
                // (RwLockReadGuard is not Send and cannot cross an await point).
                let cp_opt = {
                    let guard = self.checkpoint_store.read();
                    guard.as_ref().cloned()
                };
                if let Some(cp) = cp_opt {
                    cp.snapshot(&change).await;
                }
            }
        }
        let tool_was_registered = tool_opt.is_some();
        let result = match tool_opt {
            Some(tool) => match tool.execute(&tool_call.arguments, context).await {
                Ok(result) => {
                    debug!(
                        "[AgentLoop] Tool {} returned: {} bytes",
                        tool_call.name,
                        result.len()
                    );
                    result
                }
                Err(err) => {
                    warn!("[AgentLoop] Tool {} error: {}", tool_call.name, err);
                    format!("Tool error: {}", err)
                }
            },
            None => {
                warn!("[AgentLoop] Unknown tool: {}", tool_call.name);
                format!("Error: Unknown tool '{}'", tool_call.name)
            }
        };

        // K1a (U14): user post-tool hooks — pipeline, each hook sees the
        // current (possibly already-replaced) result, all hooks run. Only
        // when the tool actually executed (Pre/Post pairing — the
        // unknown-tool path never dispatched). Runs BEFORE Forge so the
        // recorded experience matches the final result.
        let result = if tool_was_registered {
            let hooks = self.tool_hooks.read().snapshot();
            crate::hooks::run_post_hooks(&hooks, &hook_call, result).await
        } else {
            result
        };

        // Record experience for Forge self-learning (non-blocking).
        #[cfg(feature = "forge")]
        {
            if let Some(ref forge) = self.forge {
                // Gate on the runtime master switch — without this, experiences
                // are recorded on every tool call even when forge.enabled=false.
                if forge.is_enabled() {
                    // Truncate payloads: despite the "summary" field name the
                    // previous code stored the FULL args/result (read_file/exec
                    // could be megabytes). 500 chars is enough for reflection
                    // stats. Note: the dedup hash below still uses the full
                    // parsed `args`, so hashing is unaffected.
                    let trunc = |s: &str| -> String { s.chars().take(500).collect() };
                    let exp = nemesis_types::forge::Experience {
                        id: uuid::Uuid::new_v4().to_string(),
                        tool_name: tool_call.name.clone(),
                        input_summary: trunc(&tool_call.arguments),
                        output_summary: trunc(&result),
                        success: !result.contains("SECURITY BLOCKED")
                            && !result.contains("Tool error:"),
                        duration_ms: tool_start.elapsed().as_millis() as u64,
                        timestamp: chrono::Local::now().to_rfc3339(),
                        session_key: format!("{}:{}", context.channel, context.chat_id),
                    };
                    let args = serde_json::from_str(&tool_call.arguments)
                        .unwrap_or(serde_json::Value::Null);
                    let _ = forge.collector().record_with_args(exp, &args).await;
                }
            }
        }

        result
    }

    /// Phase 2: check a tool call's arguments against the registered tool's
    /// schema. Returns Valid / Fixed / Invalid. Unknown tools return Valid so
    /// the existing unknown-tool path in `handle_tool_call` reports them
    /// (class C, not a schema failure).
    fn check_tool_args(&self, tool_call: &ToolCallInfo) -> crate::args_validator::Outcome {
        let schema_opt = self
            .tools
            .read()
            .get(&tool_call.name)
            .map(|t| t.parameters());
        match schema_opt {
            Some(schema) => crate::args_validator::check(&schema, &tool_call.arguments),
            None => crate::args_validator::Outcome::Valid,
        }
    }

    /// Phase 2: per-request consecutive-validation-failure budget, tier-aware.
    /// Mini models get 3, Normal 2, Big 1.
    fn validation_retry_budget(&self) -> u32 {
        (*self.tier.read()).validation_retry_budget()
    }

    /// Phase 4a: capability tier currently in effect (small-model-tool-robustness).
    pub fn tier(&self) -> nemesis_types::capability::ModelTier {
        *self.tier.read()
    }

    /// Phase 4a: override the capability tier (e.g. after resolving it from the
    /// active model config at construction, or after `model set-tier`).
    pub fn set_tier(&self, tier: nemesis_types::capability::ModelTier) {
        info!("[AgentLoop] Capability tier set: {}", tier);
        *self.tier.write() = tier;
    }

    /// Phase 4a: set the config.json path. After this, the tier is re-resolved
    /// live from config.json on every model switch and whenever the file's mtime
    /// changes (dashboard model add, CLI `model set-tier`). config.json is the
    /// single source of truth — there is no stale per-model snapshot to keep in
    /// sync. Called by `agent_factory` at gateway startup.
    pub fn set_config_path(&self, path: std::path::PathBuf) {
        *self.config_path.write() = Some(path);
    }

    /// G4 (U4): enable tool-result spill with the given root directory
    /// (expected `<home>/logs/spill`). Results above the spill threshold are
    /// written whole under this root and the conversation keeps a bounded
    /// preview + locator. Without a call, spilling stays disabled and results
    /// use only the G3 prune tier.
    pub fn set_spill_root(&self, root: std::path::PathBuf) {
        *self.spill_root.write() = Some(root);
    }

    /// D2 (2026-08-24 arch review): read back the configured spill root, if
    /// any. Diagnostics/test seam for verifying factory wiring — both the
    /// main and cluster factories point at `<home>/logs/spill`.
    pub fn spill_root_path(&self) -> Option<std::path::PathBuf> {
        self.spill_root.read().clone()
    }

    /// H3 (P2.2): enable skills-catalog digest injection by providing the
    /// loader. The digest is injected (same point as the time/env hint) only
    /// when the catalog changed since the last injection for this session.
    pub fn set_skills_loader(
        &self,
        loader: Arc<nemesis_skills::loader::SkillsLoader>,
    ) {
        *self.skills_loader.write() = Some(loader);
    }

    /// H5 (U18): enable the workspace instruction-chain section by giving
    /// the workspace root (chain root = this dir; the chain itself is read
    /// per-injection).
    pub fn set_workspace_root(&self, root: std::path::PathBuf) {
        *self.workspace_root.write() = Some(root);
    }

    /// Full-review M4: set the context-snapshot message role ("user" |
    /// "system"). Anything unrecognized stays/becomes "user" (default).
    pub fn set_snapshot_role(&self, role: &str) {
        let r = if role.eq_ignore_ascii_case("system") { "system" } else { "user" };
        *self.snapshot_role.write() = r.to_string();
    }

    /// X2 (U8 refinement): tell the loop whether interactive approval
    /// (desktop popup) is wired. Called by the gateway after it attaches
    /// the approval adapter; renders into the `# Runtime Policy` snapshot
    /// section. Default `false` (standalone / non-desktop builds).
    pub fn set_interactive_approval(&self, enabled: bool) {
        *self.interactive_approval.write() = enabled;
    }

    /// H5 (U18): touch-driven digest invalidation. Called by the dispatch
    /// path when a read_file/write_file/edit_file call touched a file that
    /// is on the session's instruction chain — the next build_messages
    /// re-reads the chain and re-injects. `session_key` is the stable key
    /// build_messages derived (first-user-content hash); the dispatch path
    /// does not know it, so this drops the WHOLE digest state entry list is
    /// not viable — instead we clear all session keys whose chain contains
    /// the touched path. Simplest correct form given the keying: clear ALL
    /// entries (re-inject once for every live session on the next build) —
    /// file touches on the chain are rare, so over-invalidation is cheap.
    pub fn invalidate_context_digests(&self) {
        self.skills_digest_state.clear_all();
    }

    /// Phase 4a: re-resolve the capability tier against the currently active
    /// model by reading config.json live. Per-model `model_tier`/`real_name`/
    /// `model_size_b` are honoured; a missing/unreadable config falls back to
    /// the name heuristic. Called after every model switch and on config change.
    fn refresh_active_tier(&self) {
        let path = match self.config_path.read().clone() {
            Some(p) => p,
            None => return, // standalone mode — keep the startup tier
        };
        let active = self.active_model.read().clone();
        let tier = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .map(|v| nemesis_types::capability::resolve_active_tier(&v, &active))
            .unwrap_or_else(|| {
                nemesis_types::capability::detect_tier(&nemesis_types::capability::TierHint {
                    full_model: Some(active.clone()),
                    real_name: None,
                    size_b: None,
                })
            });
        if *self.tier.read() != tier {
            info!(
                "[AgentLoop] Active model '{}' → capability tier {} (re-resolved from config.json)",
                active, tier
            );
            *self.tier.write() = tier;
        }
    }

    /// Resolve the active model's display id (`provider/name`, e.g.
    /// `deepseek/deepseek-v4-flash`) for the per-message "供应商·模型名"
    /// badge. Reads config.json fresh each call (called once per assistant
    /// turn, negligible cost) — no cached field, so it can never go stale when
    /// the model switches. Falls back to the bare `active_model` when config
    /// is unavailable (standalone mode) or the entry isn't found.
    pub(crate) fn current_display_model(&self) -> String {
        let active = self.active_model.read().clone();
        let path = match self.config_path.read().clone() {
            Some(p) => p,
            None => return active,
        };
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .map(|v| nemesis_types::capability::resolve_display_model(&v, &active))
            .unwrap_or(active)
    }

    /// Resolve the active model's per-model output token cap (`max_output_tokens`)
    /// from config.json, used as the chat request's `max_tokens`. Reads config
    /// fresh each call (like `current_display_model`). `None` when config is
    /// unavailable (standalone mode) or the field is absent — caller falls back
    /// to the 8192 default. Lets each model declare its real output ceiling so
    /// large files write in one shot instead of truncating at a blanket cap.
    /// H4 (U16 half): the active model's reasoning-effort tier from
    /// config.json (`model set-effort`). None when unset/"off"/standalone.
    pub(crate) fn current_reasoning_effort(&self) -> Option<String> {
        let active = self.active_model.read().clone();
        let path = match self.config_path.read().clone() {
            Some(p) => p,
            None => return None,
        };
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| nemesis_types::capability::resolve_reasoning_effort(&v, &active))
    }

    pub(crate) fn current_max_tokens(&self) -> Option<u32> {
        let active = self.active_model.read().clone();
        let path = match self.config_path.read().clone() {
            Some(p) => p,
            None => return None,
        };
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| nemesis_types::capability::resolve_max_output_tokens(&v, &active))
            .map(|n| n as u32)
            // ↑ i64 (from JSON) → u32; max_output_tokens is a non-negative count
    }

    /// U16 (sixth batch): resolve the active model's per-model
    /// `context_window` (input token capacity) from config.json. Reads config
    /// fresh each call (same pattern as `current_max_tokens`). `None` when
    /// unset/standalone — callers keep their existing default. This closes
    /// the S1-S7 leftover: the compaction thresholds in `maybe_summarize`
    /// were computed against a hardcoded 32000 regardless of the model's
    /// real window (a 200K-window model compacted 6× too early).
    pub(crate) fn current_context_window(&self) -> Option<usize> {
        let active = self.active_model.read().clone();
        let path = match self.config_path.read().clone() {
            Some(p) => p,
            None => return None,
        };
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| nemesis_types::capability::resolve_context_window(&v, &active))
            .map(|w| w as usize)
    }

    /// T4 (U1): per-model summarizer prefix-reuse switch from config.json
    /// (`summarizer_prefix_reuse`, default true — the main model keeps the
    /// G1 prefix-reuse summary shape). `false` → the summary request falls
    /// back to the pre-G1 shape (single bare user message with
    /// `role: content` text concatenation), for cheap summarizer models that
    /// break the assumed warm KV prefix. Reads config fresh each call (same
    /// pattern as [`current_max_tokens`]); standalone (no config_path) →
    /// default true.
    pub(crate) fn current_summarizer_prefix_reuse(&self) -> bool {
        let active = self.active_model.read().clone();
        let path = match self.config_path.read().clone() {
            Some(p) => p,
            None => return true,
        };
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| {
                nemesis_types::capability::resolve_summarizer_prefix_reuse(&v, &active)
            })
            .unwrap_or(true)
    }

    /// Phase 4a: detect config.json on-disk changes (by mtime) and re-resolve
    /// the active model's tier if it changed. Runs once per LLM round, next to
    /// `check_mcp_reload`. Picks up dashboard model additions and CLI
    /// `model set-tier` while the gateway is running.
    pub(crate) fn check_config_reload(&self) {
        let path = match self.config_path.read().clone() {
            Some(p) => p,
            None => return,
        };
        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        {
            let mut last = self.config_mtime.write();
            if mtime == *last {
                return; // unchanged since last check
            }
            *last = mtime;
        }
        debug!("[AgentLoop] config.json mtime changed; re-resolving capability tier");
        self.refresh_active_tier();
    }

    /// P3.1 (sixth batch): retrieve relevant long-term memories for the
    /// LATEST user message (vector top-K, score-thresholded, deduped by
    /// pairwise cosine > 0.92 — goal §三 semantics). `None` whenever the
    /// feature is off (default), the manager/vector store is absent, or no
    /// hit clears the bar — `None` keeps build_messages byte-identical.
    #[cfg_attr(not(feature = "memory"), allow(unused_variables))]
    async fn prefetch_memory_context(&self, instance: &AgentInstance) -> Option<Vec<String>> {
        #[cfg(feature = "memory")]
        {
            let (auto, top_k) = *self.memory_inject_cfg.read();
            if !auto {
                return None;
            }
            // Latest USER message is the retrieval signal.
            let history = instance.get_history();
            let query = history
                .iter()
                .rev()
                .find(|t| t.role == "user")
                .map(|t| t.content.clone())?;
            if query.trim().is_empty() {
                return None;
            }
            let mgr = self.memory_inject_manager.read().clone()?;
            let result = mgr.search(&query, None, top_k.max(1) + 2).await.ok()?;
            // Score threshold: vector scores are cosine (0..1) when the store
            // is active; keep hits >= 0.35 — low enough to catch related
            // memory, high enough to skip noise. The keyword-fallback path
            // yields score 0.0 (no semantic confidence) → below bar.
            const MIN_SCORE: f64 = 0.35;
            let mut scored: Vec<(f64, String)> = result
                .entries
                .into_iter()
                .filter_map(|e| {
                    let s = e.score;
                    if s < MIN_SCORE {
                        return None;
                    }
                    let content = e.entry.content;
                    let cut = content.char_indices().nth(300).map(|(i, _)| i).unwrap_or(content.len());
                    Some((s, content[..cut].to_string()))
                })
                .collect();
            // Sort best-first, cap at top_k.
            scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            // Pairwise dedup: drop a weaker hit whose cosine with a kept hit
            // exceeds 0.92. Cheap at top_k+2 candidates.
            let mut kept: Vec<(f64, String)> = Vec::new();
            'cand: for cand in scored {
                for k in &kept {
                    if textwise_similar(&cand.1, &k.1) > 0.92 {
                        continue 'cand;
                    }
                }
                kept.push(cand);
                if kept.len() >= top_k.max(1) {
                    break;
                }
            }
            if kept.is_empty() {
                return None;
            }
            Some(kept.into_iter().map(|(_, c)| c).collect())
        }
        #[cfg(not(feature = "memory"))]
        {
            None
        }
    }

    /// Build the LLM message list from the instance conversation history.
    ///
    /// Injects an ephemeral "# Current Time / # Environment" system message
    /// immediately before the latest user message. The historical prefix (system
    /// prompt + earlier turns) stays byte-identical across requests, preserving
    /// prompt-cache hits; only the trailing user message and the dynamic marker
    /// are billed at the cache-miss rate. The platform/shell hint steers the
    /// model away from interactive commands that hang the exec tool (e.g. bare
    /// Windows `date` vs `date /t`) — small-model-tool-robustness plan Phase 1.
    ///
    /// P3.1 (sixth batch): `memory_hits` (caller-side async prefetch — search
    /// is async, this fn is sync) renders a `# Memory Context` section into
    /// the merged snapshot. `None`/empty = no section, byte-identical to
    /// pre-P3.1 output.
    pub fn build_messages(&self, instance: &AgentInstance) -> Vec<LlmMessage> {
        self.build_messages_with_memory(instance, None)
    }

    /// P3.1 companion: same as [`build_messages`] plus an optional
    /// pre-fetched memory-hit section.
    pub fn build_messages_with_memory(
        &self,
        instance: &AgentInstance,
        memory_hits: Option<&[String]>,
    ) -> Vec<LlmMessage> {
        self.build_messages_with_memory_annotated(instance, memory_hits)
            .0
    }

    /// T8 (U9 ②) companion: [`build_messages_with_memory`] plus a
    /// [`crate::replay::BuildAnnotation`] recording everything a later
    /// byte-exact replay needs that is NOT derivable from the final session
    /// file — the digest injection's final-vec position, the folded history
    /// length, and the summary-cache state AS OF this build (the session
    /// file's final summary may have advanced later in the same turn).
    /// Byte-identical output to the unannotated build; the annotation rides
    /// alongside and never feeds the provider.
    pub fn build_messages_with_memory_annotated(
        &self,
        instance: &AgentInstance,
        memory_hits: Option<&[String]>,
    ) -> (Vec<LlmMessage>, crate::replay::BuildAnnotation) {
        let history = instance.get_history();

        // Inline-summary pipeline. When a summary cache is active, its `text`
        // folds the covered prefix `history[..covers_up_to]` into the leading
        // system message, and `history[covers_up_to..]` is sent verbatim. Every
        // message is either summarized (in `text`) or verbatim — no gap, no
        // overlap. With no active cache this degrades to sending the entire
        // history verbatim, byte-identical to pre-refactor behavior.
        let cache = instance.get_summary_cache();
        let active_cache = cache
            .as_ref()
            .filter(|c| !c.text.is_empty() && c.covers_up_to >= 1);

        let turns = project_history_for_request(
            &history,
            active_cache.map(|c| (c.text.as_str(), c.covers_up_to)),
        );

        let mut annotation = crate::replay::BuildAnnotation {
            digest_index: None,
            history_len: turns.len(),
            summary_as_of: active_cache.map(|c| crate::replay::SummaryAsOf {
                covers_up_to: c.covers_up_to,
                text: c.text.clone(),
            }),
        };

        // I2 (U8): time/env becomes the FIRST section of the merged context
        // snapshot (was a standalone system-role dyn_msg). Minute granularity:
        // the timestamp truncates to the minute so a burst of calls within
        // the same minute does not churn the digest (dsh runtime-context
        // snapshot discipline: identical content ⇒ no re-injection).
        let now = chrono::Local::now()
            .format("%Y-%m-%d %H:%M (%A)")
            .to_string();
        #[cfg(target_os = "windows")]
        let env_hint = "platform: windows\ndefault_shell: cmd\ntime_cmd: use `date /t` or `echo %date% %time%` or PowerShell `Get-Date`";
        #[cfg(not(target_os = "windows"))]
        let env_hint = "platform: unix\ndefault_shell: sh\ntime_cmd: use `date`";
        let snapshot_section = format!(
            "# Current Time / Environment snapshot\n{}\n# Environment\n{}\n(本快照取代之前的时间/环境快照)",
            now, env_hint
        );

        let turn_to_msg = |turn: &crate::types::ConversationTurn| LlmMessage {
            role: turn.role.clone(),
            content: turn.content.clone(),
            tool_calls: if turn.tool_calls.is_empty() {
                None
            } else {
                Some(turn.tool_calls.clone())
            },
            tool_call_id: turn.tool_call_id.clone(),
            reasoning_content: turn.reasoning_content.clone(),
        };

        // Inject dyn_msg just before the last user message, but only when there
        // is a system prompt at turns[0] to protect (otherwise there's no
        // cached prefix to preserve).
        //
        // H3 (P2.2) + H5 (U18) + I2 (U8): the skills-catalog digest, the
        // workspace instruction chain, AND the time/env snapshot ride ONE
        // injection point as a single MERGED message (sections inside one
        // <system-reminder> wrapper), with the same prefix-protection
        // condition. Re-emitted on EVERY build (not persisted in history) —
        // deterministic rendering keeps it byte-identical while nothing
        // changed, which is what preserves the provider prefix. Sections
        // re-read from disk each build, so file touches are picked up
        // naturally (H5's invalidate call is a structural no-op).
        let context_digest_msg: Option<LlmMessage> = {
            let loader = self.skills_loader.read().clone();
            let ws_root = self.workspace_root.read().clone();
            // Build the merged content: time/env snapshot (I2) + skills
            // section (if any) + workspace instructions section (if any).
            // The snapshot is ALWAYS present (time always renders), so the
            // merged message exists for every session.
            let mut sections: Vec<String> = vec![snapshot_section.clone()];
            if let Some(ref l) = loader {
                let infos = l.list_skills();
                if !infos.is_empty() {
                    let catalog = crate::skills_digest::catalog_from_skills_infos(&infos);
                    let rendered = crate::skills_digest::render_skills_digest(&catalog);
                    sections.push(crate::skills_digest::digest_message(&rendered));
                }
            }
            if let Some(ref root) = ws_root {
                let cwd = root.clone(); // workspace root ≈ conversation cwd
                let chain = crate::workspace_instructions::load_instruction_chain(root, &cwd);
                let rendered = crate::workspace_instructions::render_instructions_section(&chain);
                if !rendered.is_empty() {
                    sections.push(rendered);
                }
            }
            // P3.1 (sixth batch): pre-fetched memory hits as a section. The
            // caller (run_llm_loop) did the async search against the CURRENT
            // user message; here we only render. Empty/None ⇒ no section ⇒
            // byte-identical output to auto_inject=false.
            if let Some(hits) = memory_hits {
                if !hits.is_empty() {
                    let body = hits
                        .iter()
                        .map(|h| format!("- {}", h))
                        .collect::<Vec<_>>()
                        .join("\n");
                    sections.push(format!(
                        "# Memory Context\n{body}\n\n(以上是自动检索到的相关长期记忆，可能与当前对话有关，也可能无关——自行判断取舍。)"
                    ));
                }
            }
            // X2 (U8 refinement): runtime policy facts as the LAST section.
            // All three inputs are plain state rendered without clocks —
            // deterministic (same state ⇒ identical bytes, so the merged
            // message stays byte-stable between turns and the historical
            // prefix before it is untouched either way):
            //   approval — live wiring flag (gateway sets after attaching
            //     the desktop popup adapter);
            //   guardian — live judge presence on the security plugin
            //     (feature-off builds render "off");
            //   model_tier — the live capability tier (auto re-resolves on
            //     config reload; the snapshot picks the new value up at the
            //     next build).
            #[cfg(feature = "security")]
            let guardian_on = self
                .security_plugin
                .as_ref()
                .is_some_and(|p| p.judge().is_some());
            #[cfg(not(feature = "security"))]
            let guardian_on = false;
            let approval_on = *self.interactive_approval.read();
            let tier_now = *self.tier.read();
            sections.push(format!(
                "# Runtime Policy\napproval: {}\nguardian: {}\nmodel_tier: {}\n(当前审批/守护/模型档位运行时策略快照；策略变更后下一次构建生效。)",
                if approval_on {
                    "interactive（ask 规则触发弹窗审批）"
                } else {
                    "off（无交互审批，ask 规则按默认策略处理）"
                },
                if guardian_on {
                    "on（CRITICAL 操作语义二审）"
                } else {
                    "off"
                },
                tier_now,
            ));
            if sections.is_empty() {
                None
            } else {
                let merged = format!(
                    "<system-reminder>\n{}\n</system-reminder>",
                    sections.join("\n\n")
                );
                self.skills_digest_state
                    .should_inject("", &merged) // stateless since round-5
                    .map(|m| LlmMessage {
                        // I2 (U8): user-role snapshot (was system) — the
                        // system prompt stays byte-frozen; dynamic facts
                        // arrive as conversation messages. M4: role is
                        // configurable for strict chat templates that
                        // reject adjacent user/user pairs.
                        role: self.snapshot_role.read().clone(),
                        content: m,
                        tool_calls: None,
                        tool_call_id: None,
                        reasoning_content: None,
                    })
            }
        };

        let last_user_idx = turns
            .iter()
            .rposition(|t| t.role == "user")
            .filter(|&i| i > 0)
            .filter(|_| turns.first().map_or(false, |t| t.role == "system"));

        match last_user_idx {
            Some(idx) => {
                let mut messages: Vec<LlmMessage> =
                    Vec::with_capacity(turns.len() + 1 + context_digest_msg.is_some() as usize);
                messages.extend(turns[..idx].iter().map(turn_to_msg));
                if let Some(d) = context_digest_msg {
                    messages.push(d);
                    // T8 (U9 ②): final-vec position of the digest injection —
                    // recorded because it is rebuilt-on-every-request and never
                    // persisted, so replay must re-insert it at this position.
                    annotation.digest_index = Some(messages.len() - 1);
                }
                messages.extend(turns[idx..].iter().map(turn_to_msg));
                (messages, annotation)
            }
            None => (turns.iter().map(turn_to_msg).collect(), annotation),
        }
    }

    // -----------------------------------------------------------------------
    // Slash command handling
    // -----------------------------------------------------------------------

    /// Process an inbound message through routing and slash command handling (standalone).
    ///
    /// Returns (agent_id, response_content, handled).
    pub fn process_message(
        &self,
        content: &str,
        context: &RequestContext,
    ) -> (String, String, bool) {
        // Check for cluster continuation prefix.
        if context.channel == "system"
            && content.starts_with(nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX)
        {
            debug!(
                "[AgentLoop] Cluster continuation message intercepted: {}",
                content
            );
            return (String::new(), String::new(), true);
        }

        // Check for slash commands.
        if let Some(response) = self.handle_command(content) {
            return (String::new(), response, true);
        }

        (String::new(), String::new(), false)
    }

    /// Handle slash commands embedded in message content (standalone, no context).
    pub fn handle_command(&self, content: &str) -> Option<String> {
        self.handle_command_with_context(content, "")
    }

    /// Handle slash commands with optional channel context.
    /// Mirrors Go's `handleCommand()`.
    fn handle_command_with_context(&self, content: &str, current_channel: &str) -> Option<String> {
        let content = content.trim();
        if !content.starts_with('/') {
            return None;
        }

        let parts: Vec<&str> = content.split_whitespace().collect();
        if parts.is_empty() {
            return None;
        }

        match parts[0] {
            "/help" => Some(
                "Commands: /show [model|channel|agents], /list [tools|models], /model <alias>, /help".to_string(),
            ),
            "/model" => {
                if parts.len() < 2 {
                    let current = self.active_model.read().clone();
                    let aliases = self.model_aliases();
                    Some(format!(
                        "Current model: {}\nAliases: {} (or pass any model id)",
                        current,
                        if aliases.is_empty() {
                            "(none configured)".to_string()
                        } else {
                            aliases.join(", ")
                        }
                    ))
                } else {
                    let new_model = self.set_active_model(parts[1]);
                    Some(format!("✓ Model switched to: {}", new_model))
                }
            }
            "/show" => {
                if parts.len() < 2 {
                    return Some("Usage: /show [model|channel|agents]".to_string());
                }
                match parts[1] {
                    "model" => Some(format!("Current model: {}", self.active_model.read())),
                    "channel" => Some(format!("Current channel: {}", current_channel)),
                    "agents" => {
                        let agent_ids = self
                            .registry
                            .as_ref()
                            .map(|r| r.list_agent_ids())
                            .unwrap_or_default();
                        if agent_ids.is_empty() {
                            let guard = self.tools.read();
                            let tool_names: Vec<&str> =
                                guard.keys().map(|s| s.as_str()).collect();
                            Some(format!("Registered agents (tools): {}", tool_names.join(", ")))
                        } else {
                            Some(format!("Registered agents: {}", agent_ids.join(", ")))
                        }
                    }
                    _ => Some(format!("Unknown show target: {}", parts[1])),
                }
            }
            "/list" => {
                if parts.len() < 2 {
                    return Some("Usage: /list [models|channels|agents|tools]".to_string());
                }
                match parts[1] {
                    "tools" => {
                        let guard = self.tools.read();
                        let tool_names: Vec<&str> =
                            guard.keys().map(|s| s.as_str()).collect();
                        Some(format!("Available tools: {}", tool_names.join(", ")))
                    }
                    "model" | "models" => Some(format!(
                        "Current model: {} (configured in config.json)",
                        self.active_model.read()
                    )),
                    "channels" => {
                        let channels = self.channel_manager_channels.lock();
                        if channels.is_empty() {
                            Some("No channels enabled".to_string())
                        } else {
                            Some(format!("Enabled channels: {}", channels.join(", ")))
                        }
                    }
                    "agents" => {
                        let agent_ids = self
                            .registry
                            .as_ref()
                            .map(|r| r.list_agent_ids())
                            .unwrap_or_default();
                        if agent_ids.is_empty() {
                            let guard = self.tools.read();
                            let tool_names: Vec<&str> =
                                guard.keys().map(|s| s.as_str()).collect();
                            Some(format!("Registered agents: {}", tool_names.join(", ")))
                        } else {
                            Some(format!("Registered agents: {}", agent_ids.join(", ")))
                        }
                    }
                    _ => Some(format!("Unknown list target: {}", parts[1])),
                }
            }
            "/switch" => {
                if parts.len() < 4 || parts[2] != "to" {
                    return Some("Usage: /switch [model|channel] to <name>".to_string());
                }
                let target = parts[1];
                let value = parts[3];

                match target {
                    "model" => {
                        let old_model = self.active_model.read().clone();
                        Some(format!(
                            "Model switch requested: {} -> {} (restart required for persistent change)",
                            old_model, value
                        ))
                    }
                    "channel" => Some(format!("Target channel switched to: {}", value)),
                    _ => Some(format!("Unknown switch target: {}", target)),
                }
            }
            _ => None,
        }
    }

    // -----------------------------------------------------------------------
    // Startup info
    // -----------------------------------------------------------------------

    /// Get startup information about the agent loop for logging.
    /// Mirrors Go's `GetStartupInfo()`.
    pub fn get_startup_info(&self) -> serde_json::Value {
        let guard = self.tools.read();
        let tool_names: Vec<&str> = guard.keys().map(|s| s.as_str()).collect();

        let agent_ids = self
            .registry
            .as_ref()
            .map(|r| r.list_agent_ids())
            .unwrap_or_default();

        serde_json::json!({
            "tools": {
                "count": tool_names.len(),
                "names": tool_names,
            },
            "agents": {
                "count": agent_ids.len(),
                "ids": agent_ids,
            },
            "model": self.active_model.read().to_string(),
            "max_turns": self.config.max_turns,
            "system_prompt_configured": self.config.system_prompt.is_some(),
        })
    }

    // -----------------------------------------------------------------------
    // Accessors
    // -----------------------------------------------------------------------

    /// Returns a reference to the tool registry.
    pub fn tools(&self) -> parking_lot::RwLockReadGuard<'_, HashMap<String, Arc<dyn Tool>>> {
        self.tools.read()
    }

    /// Returns a reference to the agent config.
    pub fn config(&self) -> &AgentConfig {
        &self.config
    }

    /// Mark that a message was sent for the given session in the current round.
    /// Used by tools like MessageTool to prevent double-sending. Mirrors Go's
    /// MessageTool.sentInRound.
    pub fn mark_sent_in_round(&self, session_key: &str) {
        self.sent_in_round.mark_sent(session_key);
    }

    /// Check if a message was already sent in the current round for a session.
    pub fn has_sent_in_round(&self, session_key: &str) -> bool {
        self.sent_in_round.has_sent_in_round(session_key)
    }
}

// ---------------------------------------------------------------------------
// Standalone summarization helpers (usable from spawned tasks)
// ---------------------------------------------------------------------------

/// Adjust a summarize boundary so the verbatim tail `history[new_c..]` doesn't
/// START in the middle of a tool_call/result pair.
///
/// `summarize_prefix_owned` only folds user/assistant `content` into the
/// summary — tool_calls and tool results are not summarized. If the boundary
/// landed between an assistant tool_call (covered by the summary only as text
/// content, which is often empty for a pure tool-call turn) and its tool
/// result, `repair_tool_message_pairs` would drop the orphan result from the
/// tail and the whole interaction would vanish from the LLM's view. Backing
/// `new_c` up past any leading tool messages moves the parent assistant (and
/// sibling results) into the verbatim tail, keeping the pair intact. (The old
/// `truncate_with_tool_pairs` did the equivalent by prepending the parent.)
///
/// The summary still covers `history[..returned_new_c]` and the tail is
/// `history[returned_new_c..]` — the gap-free invariant holds; the tail just
/// grows slightly past `K_TARGET` when a pair straddles the boundary.
fn tool_safe_boundary(history: &[crate::types::ConversationTurn], mut new_c: usize) -> usize {
    while new_c > 0 && new_c < history.len() && history[new_c].role == "tool" {
        new_c -= 1;
    }
    new_c
}

/// Summarize a contiguous prefix of the conversation, merging any existing
/// summary.
///
/// Summarizes **all** of `messages` (no internal "keep last N" step) — the
/// caller has already chosen the verbatim tail boundary (`K_TARGET`), so every
/// message passed in is meant to be folded into the summary. Keeping a "last N"
/// here would leave a gap between the summary and the verbatim tail. Reuses the
/// multipart/batch machinery; merges `existing_summary` (which covers messages
/// before this prefix) into the result.
///
/// G1 (U1) prefix-reuse: the summary request is built as
/// `[system, ...original covered messages..., instruction]` — the same leading
/// messages the main loop sends (byte-equal per message), so the provider's
/// warm KV prefix from the last routed request is REUSED rather than
/// invalidated (dsh compaction-basic's "genuine prefix" principle). The old
/// form (single bare user message with `role: content` text concatenation)
/// shared no prefix with real requests and destroyed structure (tool_calls
/// flattened to text).
///
/// Returns `Some(summary)` if a non-empty summary was produced, `None`
/// otherwise. **None 的两种含义都不允许调用方推进 covers_up_to**：
/// (a) 前缀里没有可摘要的 user/assistant 内容；(b) 摘要 LLM 调用失败
/// （2026-08-25 摘要静默失败修复：此前 batch 失败返回空字符串，multipart
/// 拿两段空串去 merge，merge 模型回"你没把摘要贴给我"，这段**失败回复**
/// 被当摘要存进 store、covers 照常推进——被折叠的上下文静默丢失。现在
/// 失败一路传播为 None，调用方保持原 history 不折叠并 warn。）
///
/// T4 (U1) per-model switch: `prefix_reuse == false` falls back to the
/// pre-G1 shape (`summarize_bare_concat_owned`) — per-model config
/// `summarizer_prefix_reuse: false`, for cheap summarizer models that break
/// the assumed warm KV prefix. Default (true) keeps the prefix-reuse shape.
async fn summarize_prefix_owned(
    messages: &[&crate::types::ConversationTurn],
    existing_summary: &str,
    context_window: usize,
    prefix_reuse: bool,
    provider: &dyn LlmProvider,
    model: &str,
    observer_manager: Option<Arc<nemesis_observer::Manager>>,
) -> Option<String> {
    // Oversized message guard.
    let max_msg_tokens = context_window / 2;
    let mut valid_messages: Vec<&crate::types::ConversationTurn> = Vec::new();
    let mut omitted = false;

    for m in messages {
        if m.role != "user" && m.role != "assistant" {
            continue;
        }
        let msg_tokens = crate::session::estimate_tokens(&m.content);
        if msg_tokens > max_msg_tokens {
            omitted = true;
            continue;
        }
        valid_messages.push(m);
    }

    if valid_messages.is_empty() {
        return None;
    }

    let final_summary = if !prefix_reuse {
        // T4 (U1): old shape — single bare user message, no structure.
        summarize_bare_concat_owned(
            &valid_messages,
            existing_summary,
            provider,
            model,
            observer_manager,
        )
        .await
    } else {
        // G1: the system prompt anchoring the prefix. `messages` is
        // history[..new_c]; history[0] is the system turn — include it verbatim
        // (WITHOUT the summary block the main loop appends: that would leak the
        // old summary into the prefix and change it between rounds).
        let system_msg: Option<LlmMessage> = messages
            .first()
            .filter(|m| m.role == "system")
            .map(|m| conversation_turn_to_llm_message(m));

        if valid_messages.len() > 10 {
            summarize_multipart_owned(
                system_msg.as_ref(),
                &valid_messages,
                existing_summary,
                provider,
                model,
                observer_manager,
            )
            .await
        } else {
            summarize_batch_owned(
                system_msg.as_ref(),
                &valid_messages,
                existing_summary,
                provider,
                model,
                observer_manager,
            )
            .await
        }
    };

    let final_summary = match final_summary {
        Some(s) if omitted && !s.is_empty() => Some(format!(
            "{}\n[Note: Some oversized messages were omitted from this summary for efficiency.]",
            s
        )),
        other => other,
    };

    final_summary.filter(|s| !s.is_empty())
}

/// G1 (U1): build an LLM wire message from a ConversationTurn preserving the
/// original structure (role/content/tool_calls/tool_call_id/reasoning) — the
/// same projection `build_messages`'s `turn_to_msg` performs. Shared by the
/// summarizers so the summary request's prefix messages are byte-equal to the
/// main loop's.
fn conversation_turn_to_llm_message(turn: &crate::types::ConversationTurn) -> LlmMessage {
    LlmMessage {
        role: turn.role.clone(),
        content: turn.content.clone(),
        tool_calls: if turn.tool_calls.is_empty() {
            None
        } else {
            Some(turn.tool_calls.clone())
        },
        tool_call_id: turn.tool_call_id.clone(),
        reasoning_content: turn.reasoning_content.clone(),
    }
}

/// The trailing instruction for a G1 prefix-reuse summary request. Kept in one
/// place so the batch and multipart paths emit the identical instruction.
const SUMMARIZE_INSTRUCTION: &str = "请对以上对话片段做一份简明摘要，保留核心上下文与关键要点，供后续对话作为前情提要使用。";

/// T4 (U1): pre-G1 summary shape, restored as the per-model fallback
/// (`summarizer_prefix_reuse: false`).
///
/// This is the OLD request form the G1 refactor replaced: a single bare user
/// message whose content is the covered messages flattened as
/// `role: content` text lines, plus the instruction (and any existing-summary
/// context). It shares NO prefix with real requests and destroys structure
/// (tool_calls flatten to text) — which is exactly why it is NOT the default.
/// It remains useful for cheap summarizer models whose warm-KV-prefix
/// assumption G1 relies on does not hold (different tokenizer, no prompt
/// caching): a shape-neutral single message is the lowest-common-denominator
/// request those models handle reliably. The G1 prefix-reuse path
/// (summarize_multipart_owned / summarize_batch_owned) stays the default for
/// the main model; this function is invoked ONLY when the per-model switch
/// opts out.
/// Returns `Some(summary)` on a non-empty LLM reply; `None` on LLM failure or
/// empty reply (failure must propagate — never fold history behind a failed
/// summary; see the 2026-08-25 fix note on `summarize_prefix_owned`).
async fn summarize_bare_concat_owned(
    messages: &[&crate::types::ConversationTurn],
    existing_summary: &str,
    provider: &dyn LlmProvider,
    model: &str,
    observer_manager: Option<Arc<nemesis_observer::Manager>>,
) -> Option<String> {
    let mut content = String::new();
    if !existing_summary.is_empty() {
        content.push_str(&format!(
            "Existing context (summary of the earlier conversation, merge with the new summary): {}\n\n",
            existing_summary
        ));
    }
    for m in messages {
        content.push_str(&format!("{}: {}\n", m.role, m.content));
    }
    content.push_str(SUMMARIZE_INSTRUCTION);

    let llm_messages = vec![LlmMessage {
        role: "user".to_string(),
        content,
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
    }];

    let response = emit_observer_events_around_llm(
        observer_manager.as_ref(),
        "summarize-bare-concat",
        model,
        provider.chat(model, llm_messages, None, vec![]),
    )
    .await;

    match response {
        Some(Ok(resp)) if !resp.content.is_empty() => Some(resp.content),
        Some(Ok(_)) => None,
        Some(Err(e)) => {
            warn!(
                "[AgentLoop] summarize_bare_concat_owned LLM call failed (summary NOT produced, history stays unfolded): {}",
                e
            );
            None
        }
        None => None,
    }
}

/// Multi-part summarization (standalone, works in spawned task).
///
/// G1 (U1): each part is summarized as `[system?, ...part messages,
/// instruction]` — a part is a contiguous slice of the covered prefix, so its
/// message list is a true ordered prefix subset of the main request's history
/// (prefix-cache friendly in the same way).
///
/// Returns `Some` only when BOTH parts produced real summaries. If either
/// part's LLM call fails → `None` (whole summarization fails; the caller must
/// keep the history unfolded — a half-empty merge prompt makes the merge model
/// answer with a "you didn't paste the summaries" complaint, and storing that
/// complaint as the summary silently amnesiates the covered prefix; this is
/// the exact production failure found in the legacy session 2026-08-25).
async fn summarize_multipart_owned(
    system_msg: Option<&LlmMessage>,
    messages: &[&crate::types::ConversationTurn],
    existing_summary: &str,
    provider: &dyn LlmProvider,
    model: &str,
    observer_manager: Option<Arc<nemesis_observer::Manager>>,
) -> Option<String> {
    let mid = messages.len() / 2;
    let part1 = &messages[..mid];
    let part2 = &messages[mid..];

    let s1 = summarize_batch_owned(system_msg, part1, existing_summary, provider, model, observer_manager.clone()).await;
    let s2 = summarize_batch_owned(system_msg, part2, "", provider, model, observer_manager.clone()).await;

    let (s1, s2) = match (s1, s2) {
        (Some(a), Some(b)) => (a, b),
        (failed, _) => {
            warn!(
                "[AgentLoop] multipart summarization aborted: one part failed to summarize ({}); summary NOT produced, history stays unfolded",
                if failed.is_none() { "part 1" } else { "part 2" }
            );
            return None;
        }
    };

    // Merge via LLM.
    let merge_prompt = format!(
        "Merge these two conversation summaries into one cohesive summary:\n\n1: {}\n\n2: {}",
        s1, s2
    );

    let llm_messages = vec![LlmMessage {
        role: "user".to_string(),
        content: merge_prompt,
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
    }];

    let response = emit_observer_events_around_llm(
        observer_manager.as_ref(),
        "summarize-multipart-merge",
        model,
        provider.chat(model, llm_messages, None, vec![]),
    )
    .await;

    match response {
        Some(Ok(resp)) if !resp.content.is_empty() => Some(resp.content),
        Some(Ok(_)) => {
            // Empty merge reply: both parts are validated non-empty —
            // concatenate them instead of losing the coverage.
            Some(format!("{}\n\n{}", s1, s2))
        }
        _ => {
            // Merge call failed: same fallback — both parts hold real
            // summaries, so concatenation preserves the information (the
            // next compaction round will re-fold them into a merged one).
            warn!(
                "[AgentLoop] multipart merge LLM call failed; falling back to concatenating the two (validated) part summaries"
            );
            Some(format!("{}\n\n{}", s1, s2))
        }
    }
}

/// Single-batch summarization (standalone, works in spawned task).
///
/// G1 (U1): the request is `[system?, ...covered messages (original
/// structure), instruction]` — a genuine prefix of the conversation plus the
/// trailing instruction, replacing the old single bare user message with
/// `role: content` text concatenation.
///
/// Returns `Some` on a non-empty LLM reply; `None` on LLM failure or empty
/// reply (failure propagates — see the 2026-08-25 fix note on
/// `summarize_prefix_owned`).
async fn summarize_batch_owned(
    system_msg: Option<&LlmMessage>,
    batch: &[&crate::types::ConversationTurn],
    existing_summary: &str,
    provider: &dyn LlmProvider,
    model: &str,
    observer_manager: Option<Arc<nemesis_observer::Manager>>,
) -> Option<String> {
    let mut messages: Vec<LlmMessage> = Vec::with_capacity(batch.len() + 2);
    if let Some(sys) = system_msg {
        messages.push(sys.clone());
    }
    for m in batch {
        messages.push(conversation_turn_to_llm_message(m));
    }
    // Trailing instruction (merged with any existing-summary context so the
    // fold still carries prior coverage).
    let mut instruction = String::new();
    if !existing_summary.is_empty() {
        instruction.push_str(&format!(
            "Existing context (summary of the earlier conversation, merge with the new summary): {}\n\n",
            existing_summary
        ));
    }
    instruction.push_str(SUMMARIZE_INSTRUCTION);
    messages.push(LlmMessage {
        role: "user".to_string(),
        content: instruction,
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
    });

    let response = emit_observer_events_around_llm(
        observer_manager.as_ref(),
        "summarize-batch",
        model,
        provider.chat(model, messages, None, vec![]),
    )
    .await;

    match response {
        Some(Ok(resp)) if !resp.content.is_empty() => Some(resp.content),
        Some(Ok(_)) => None,
        Some(Err(e)) => {
            warn!(
                "[AgentLoop] summarize_batch_owned LLM call failed (summary NOT produced, history stays unfolded): {}",
                e
            );
            None
        }
        None => None,
    }
}

/// Emit observer events (ConversationStart, LlmRequest, LlmResponse, ConversationEnd)
/// around a synchronous LLM call closure. Used by standalone summarization functions.
async fn emit_observer_events_around_llm<Fut>(
    observer_manager: Option<&Arc<nemesis_observer::Manager>>,
    label: &str,
    model: &str,
    llm_call: Fut,
) -> Option<Result<LlmResponse, String>>
where
    Fut: std::future::Future<Output = Result<LlmResponse, String>>,
{
    use crate::loop_executor::ObserverEvent;

    let trace_id = format!(
        "{}-{}",
        label,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );

    // Emit ConversationStart + LlmRequest before the call (await, no block_in_place).
    if let Some(mgr) = observer_manager {
        let start_event = ObserverEvent::ConversationStart {
            trace_id: trace_id.clone(),
            session_key: label.to_string(),
            channel: String::new(),
            chat_id: String::new(),
            sender_id: "summarizer".to_string(),
            content: String::new(),
        };
        mgr.emit(start_event.to_conversation_event()).await;

        let request_event = ObserverEvent::LlmRequest {
            trace_id: trace_id.clone(),
            round: 0,
            model: model.to_string(),
            messages: vec![],
            tools: vec![],
            messages_count: 0,
            tools_count: 0,
            provider_name: String::new(),
            api_key: String::new(),
            api_base: String::new(),
        };
        mgr.emit(request_event.to_conversation_event()).await;
    }

    // Execute the LLM call (async, no block_on).
    let start = std::time::Instant::now();
    let mut response = llm_call.await;
    let duration_ms = start.elapsed().as_millis() as u64;

    let (response_content, raw_req, raw_resp) = match &mut response {
        Ok(r) => {
            let content = r.content.clone();
            let req = r.raw_request_body.take();
            let resp = r.raw_response_body.take();
            (content, req, resp)
        }
        Err(_) => (String::new(), None, None),
    };

    // Emit LlmResponse + ConversationEnd after the call (await, sequential —
    // LlmResponse fully processed before ConversationEnd, as the old emit_sync intended).
    if let Some(mgr) = observer_manager {
        let response_event = ObserverEvent::LlmResponse {
            trace_id: trace_id.clone(),
            round: 0,
            duration_ms,
            has_tool_calls: false,
            content: response_content.clone(),
            tool_calls: vec![],
            tool_calls_count: 0,
            finish_reason: Some("stop".to_string()),
            usage: None,
            raw_request_body: raw_req,
            raw_response_body: raw_resp,
        };
        mgr.emit(response_event.to_conversation_event()).await;

        let end_event = ObserverEvent::ConversationEnd {
            trace_id,
            session_key: label.to_string(),
            total_rounds: 1,
            duration_ms,
            content: response_content,
            channel: String::new(),
            chat_id: String::new(),
        };
        mgr.emit(end_event.to_conversation_event()).await;
    }

    Some(response)
}

// ---------------------------------------------------------------------------
// Cluster integration helpers
// ---------------------------------------------------------------------------

/// Extract the task ID from a cluster continuation sender ID.
///
/// The format is `cluster_continuation:{taskID}`.
#[cfg(test)]
pub fn extract_continuation_task_id(sender_id: &str) -> Option<&str> {
    sender_id.strip_prefix(nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX)
}

/// Extract a peer identifier from an inbound message.
///
/// Looks at metadata fields to determine the originating peer.
/// Mirrors Go's `extractPeer`:
/// - If `peer_kind` is set, uses `peer_id` (falls back to sender_id for "direct", chat_id otherwise)
/// - If no metadata, returns sender_id
pub fn extract_peer(msg: &nemesis_types::channel::InboundMessage) -> String {
    if let Some(peer_kind) = msg.metadata.get("peer_kind") {
        if !peer_kind.is_empty() {
            let peer_id = msg.metadata.get("peer_id").cloned().unwrap_or_else(|| {
                if peer_kind == "direct" {
                    msg.sender_id.clone()
                } else {
                    msg.chat_id.clone()
                }
            });
            return format!("{}:{}", peer_kind, peer_id);
        }
    }
    msg.sender_id.clone()
}

/// Extract the parent peer identifier from an inbound message.
///
/// Used for routing in nested or forwarded messages.
/// Mirrors Go's `extractParentPeer`.
#[cfg(test)]
pub fn extract_parent_peer(msg: &nemesis_types::channel::InboundMessage) -> Option<String> {
    let parent_kind = msg.metadata.get("parent_peer_kind")?;
    let parent_id = msg.metadata.get("parent_peer_id")?;
    if parent_kind.is_empty() || parent_id.is_empty() {
        return None;
    }
    Some(format!("{}:{}", parent_kind, parent_id))
}

/// Route input for agent resolution.
///
/// This is a legacy compatibility type. For new code, use
/// [`nemesis_routing::RouteInput`] directly with [`RouteResolver`].
#[cfg(test)]
#[derive(Debug, Clone)]
pub struct RouteInput {
    pub channel: String,
    pub account_id: Option<String>,
    pub peer: String,
    pub parent_peer: Option<String>,
    pub guild_id: Option<String>,
    pub team_id: Option<String>,
}

/// Resolved route for a message.
///
/// This is a legacy compatibility type. For new code, use
/// [`nemesis_routing::ResolvedRoute`] directly.
#[cfg(test)]
#[derive(Debug, Clone)]
pub struct RouteOutput {
    pub agent_id: String,
    pub session_key: String,
    pub matched_by: String,
}

/// Resolve the route for a message to determine which agent and session to use.
///
/// Uses the full `RouteResolver` with a default single-agent configuration.
/// The peer field is parsed from the format "kind:id" to extract peer_kind and peer_id.
/// Mirrors Go's `al.registry.ResolveRoute(routing.RouteInput{...})`.
#[cfg(test)]
pub fn resolve_route(input: &RouteInput) -> RouteOutput {
    // Parse peer from "kind:id" format (as produced by extract_peer).
    let (peer_kind, peer_id) = if let Some(colon_pos) = input.peer.find(':') {
        let kind = input.peer[..colon_pos].to_string();
        let id = input.peer[colon_pos + 1..].to_string();
        (Some(kind), Some(id))
    } else {
        // Treat as just an ID with no kind
        (None, Some(input.peer.clone()))
    };

    // Parse parent_peer from "kind:id" format.
    let (parent_peer_kind, parent_peer_id) = input
        .parent_peer
        .as_ref()
        .and_then(|pp| {
            if let Some(colon_pos) = pp.find(':') {
                Some((
                    Some(pp[..colon_pos].to_string()),
                    Some(pp[colon_pos + 1..].to_string()),
                ))
            } else {
                None
            }
        })
        .unwrap_or((None, None));

    let route_input = RoutingRouteInput {
        channel: input.channel.clone(),
        account_id: input.account_id.clone().unwrap_or_default(),
        peer_kind,
        peer_id,
        parent_peer_kind,
        parent_peer_id,
        guild_id: input.guild_id.clone(),
        team_id: input.team_id.clone(),
        identity_links: std::collections::HashMap::new(),
    };

    // Build a default resolver with a single "main" agent and no bindings.
    let config = RouteConfig {
        bindings: Vec::new(),
        agents: vec![AgentDef {
            id: "main".to_string(),
            is_default: true,
        }],
        dm_scope: "main".to_string(),
    };
    let resolver = RouteResolver::new(config);
    let route = resolver.resolve(&route_input);

    RouteOutput {
        agent_id: route.agent_id,
        session_key: route.session_key,
        matched_by: route.matched_by,
    }
}

/// Build an agent-scoped main session key.
///
/// Format: `agent:{agent_id}:main`
pub fn build_agent_main_session_key(agent_id: &str) -> String {
    format!("agent:{}:main", agent_id)
}

// ---------------------------------------------------------------------------
// Message formatting utilities
// ---------------------------------------------------------------------------

/// Format messages for log output, truncating long content.
///
/// Returns a human-readable multi-line representation of the message list
/// suitable for debug logging.
#[cfg(test)]
pub fn format_messages_for_log(messages: &[LlmMessage]) -> String {
    if messages.is_empty() {
        return "[]".to_string();
    }

    let mut result = String::from("[\n");
    for (i, msg) in messages.iter().enumerate() {
        result.push_str(&format!("  [{}] Role: {}\n", i, msg.role));

        if let Some(ref tool_calls) = msg.tool_calls {
            result.push_str("  ToolCalls:\n");
            for tc in tool_calls {
                let args_preview = truncate(&tc.arguments, 200);
                result.push_str(&format!("    - ID: {}, Name: {}\n", tc.id, tc.name));
                result.push_str(&format!("      Arguments: {}\n", args_preview));
            }
        }

        if !msg.content.is_empty() {
            let content_preview = truncate(&msg.content, 200);
            result.push_str(&format!("  Content: {}\n", content_preview));
        }

        if let Some(ref tcid) = msg.tool_call_id {
            result.push_str(&format!("  ToolCallID: {}\n", tcid));
        }

        result.push('\n');
    }
    result.push(']');
    result
}

/// Format tools for log output.
#[cfg(test)]
pub fn format_tools_for_log(tools: &[ToolCallInfo]) -> String {
    if tools.is_empty() {
        return "[]".to_string();
    }
    let mut result = String::from("[\n");
    for tc in tools {
        let args_preview = truncate(&tc.arguments, 200);
        result.push_str(&format!(
            "  - ID: {}, Name: {}, Args: {}\n",
            tc.id, tc.name, args_preview
        ));
    }
    result.push(']');
    result
}

/// Truncate a string to a maximum byte length, appending "..." if truncated.
/// UTF-8 safe: finds the nearest char boundary before slicing.
pub fn truncate(s: &str, max_len: usize) -> String {
    nemesis_types::utils::truncate(s, max_len)
}

/// P3.1 (sixth batch): cheap text similarity (char-bigram Jaccard) for the
/// auto-inject dedup pass. Approximates "same memory, near-identical text"
/// without another embedding call — the dedup bar (0.92) is deliberately
/// high so only true near-duplicates are dropped.
// Only called from the `#[cfg(feature = "memory")]` dedup pass — same gate
// pattern as `prefetch_memory_context` above, so memory-off builds stay
// warning-free.
#[cfg_attr(not(feature = "memory"), allow(dead_code))]
fn textwise_similar(a: &str, b: &str) -> f64 {
    let bigrams = |s: &str| -> std::collections::HashSet<(char, char)> {
        let t: Vec<char> = s.chars().collect();
        t.windows(2).map(|w| (w[0], w[1])).collect()
    };
    let (ba, bb) = (bigrams(a), bigrams(b));
    if ba.is_empty() && bb.is_empty() {
        return 1.0; // both empty (or single-char) → identical
    }
    if ba.is_empty() || bb.is_empty() {
        return 0.0;
    }
    let inter = ba.intersection(&bb).count() as f64;
    let union = ba.union(&bb).count() as f64;
    inter / union
}

#[cfg(test)]
mod inbox_tests;
// S9 (quality-hardening goal 冲刺 S9): 独立测试文件挂载（声明式，无内联测试）。
#[cfg(test)]
mod s9_tests;
#[cfg(test)]
mod tests;

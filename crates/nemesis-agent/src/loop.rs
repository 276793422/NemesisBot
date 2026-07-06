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
use tracing::{debug, info, warn, error};

use crate::context::RequestContext;
use crate::instance::AgentInstance;
use crate::registry::AgentRegistry;
use crate::session::{SessionStore, estimate_tokens_for_turns};
use crate::types::{AgentConfig, AgentEvent, ToolCallInfo, ToolCallResult};
use nemesis_routing::{RouteResolver, RouteInput as RoutingRouteInput, RouteConfig, AgentDef};

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
pub const BUSY_MESSAGE: &str = "\u{23f3} AI is processing a previous request, please try again later";

/// Concurrent request handling mode.
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
            default_response: "I've completed processing but have no response to give."
                .to_string(),
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
    /// Concurrent request handling mode.
    concurrent_mode: ConcurrentMode,
    /// Queue size for queue mode.
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
    /// Memory tool executor reference, so the gateway can attach an approval
    /// gate post-construction (memory_store/forget require interactive approval).
    #[cfg(feature = "memory")]
    memory_executor: parking_lot::RwLock<Option<Arc<nemesis_memory::memory_tools::MemoryToolExecutor>>>,
    #[cfg(not(feature = "memory"))]
    #[allow(dead_code)] // placeholder when memory feature is off
    memory_executor: parking_lot::RwLock<Option<()>>,
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
    /// Last-seen mtime of config.json; `check_config_reload` compares against
    /// this each round to detect on-disk changes without re-reading every turn.
    config_mtime: parking_lot::RwLock<Option<std::time::SystemTime>>,
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
            tools: parking_lot::RwLock::new(HashMap::new()),            config,
            outbound_tx: None,
            registry: None,
            state_manager: None,
            session_store: None,
            running: AtomicBool::new(false),
            session_busy: parking_lot::Mutex::new(HashMap::new()),
            concurrent_mode: ConcurrentMode::Reject,
            queue_size: 8,
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
            memory_executor: parking_lot::RwLock::new(None),
            tier: parking_lot::RwLock::new(nemesis_types::capability::ModelTier::Big),
            config_path: parking_lot::RwLock::new(None),
            config_mtime: parking_lot::RwLock::new(None),
        }
    }

    /// Attach a checkpoint store for the edit safety net. When set, every writer
    /// tool call (write_file/edit_file/append_file/delete_file) snapshots the
    /// file's pre-edit content before execution, so a rewind can restore it.
    pub fn set_checkpoint_store(&self, store: Arc<crate::checkpoint::CheckpointStore>) {
        *self.checkpoint_store.write() = Some(store);
    }

    /// Stash the memory tool executor so the gateway can later attach an approval
    /// gate via `set_memory_approval_gate`. Called by the factory after building
    /// the shared tool config.
    #[cfg(feature = "memory")]
    pub fn set_memory_executor(&self, exec: Arc<nemesis_memory::memory_tools::MemoryToolExecutor>) {
        *self.memory_executor.write() = Some(exec);
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
    pub async fn rewind(
        &self,
        from_turn: usize,
    ) -> Result<(Vec<String>, Vec<String>), String> {
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
            Some(Arc::new(tokio::sync::Semaphore::new(max_continuation_permits)))
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
            tools: parking_lot::RwLock::new(HashMap::new()),            config,
            outbound_tx: Some(outbound_tx),
            registry: Some(registry),
            state_manager: None,
            session_store: Some(session_store),
            running: AtomicBool::new(false),
            session_busy: parking_lot::Mutex::new(HashMap::new()),
            concurrent_mode,
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
            memory_executor: parking_lot::RwLock::new(None),
            tier: parking_lot::RwLock::new(nemesis_types::capability::ModelTier::Big),
            config_path: parking_lot::RwLock::new(None),
            config_mtime: parking_lot::RwLock::new(None),
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
        let task_failed = task_metadata.get("status").map(|s| s == "error").unwrap_or(false);

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
                            self.register_tool(name, Box::new(crate::mcp_bridge::McpToolBridge::new(tool)));
                        }
                        info!("[AgentLoop] MCP: registered {} tools from '{}'", count, server_name);
                    }
                    Err(e) => {
                        warn!("[AgentLoop] MCP: server '{}' discovery failed: {}", server_name, e);
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
        let registered: Vec<String> = self.tools.read().keys()
            .filter(|k| k.starts_with("mcp_"))
            .map(|k| {
                // "mcp_<srv>_<tool>" → "mcp_<srv>_"
                let chars: Vec<char> = k.chars().collect();
                let underscores: Vec<usize> = chars.iter().enumerate()
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
                Ok(m) => m.find_new_servers(&registered).into_iter().cloned().collect(),
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
                        self.tools.write().insert(name, Arc::from(Box::new(crate::mcp_bridge::McpToolBridge::new(tool)) as Box<dyn Tool>));
                    }
                    info!("[AgentLoop] MCP reload: registered {} tools from '{}'", count, server_name);
                }
                Err(e) => {
                    warn!("[AgentLoop] MCP reload: server '{}' failed: {}", server_name, e);
                }
            }
        }
        self.refresh_mcp_snapshot();
    }

    /// Refresh the MCP tool snapshot from the tool registry.
    fn refresh_mcp_snapshot(&self) {
        let snapshot: Vec<(String, String)> = self.tools.read().iter()
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
    pub fn set_state_manager(&mut self, mgr: Arc<nemesis_state::workspace_state::WorkspaceStateManager>) {
        self.state_manager = Some(mgr);
        debug!("[AgentLoop] State manager configured");
    }

    /// Set the observer callback for event emission.
    /// Mirrors Go's `SetObserverManager()`.
    pub fn set_observer_callback(&mut self, cb: Arc<dyn Fn(&str, &serde_json::Value) + Send + Sync>) {
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
    #[cfg(test)]
    pub async fn run_bus_owned(
        self,
        mut inbound_rx: tokio::sync::mpsc::Receiver<nemesis_types::channel::InboundMessage>,
    ) {
        self.running.store(true, Ordering::Release);

        while self.running.load(Ordering::Acquire) {
            match inbound_rx.recv().await {
                Some(msg) => {
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

                    let response = match err {
                        Some(e) => format!("Error processing message: {}", e),
                        None => response,
                    };

                    if !response.is_empty() {
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
                        } else if let Some(ref tx) = self.outbound_tx {
                            // For RPC channel, add correlation ID prefix if not already present.
                            let final_content = if msg.channel == "rpc"
                                && !msg.correlation_id.is_empty()
                                && !response.starts_with(&format!(
                                    "[rpc:{}]",
                                    msg.correlation_id
                                ))
                            {
                                format!("[rpc:{}] {}", msg.correlation_id, response)
                            } else {
                                response
                            };

                            info!(
                                "[AgentLoop] Response message     to {}:{}: {}",
                                msg.channel, msg.chat_id, truncate(&final_content, 80)
                            );

                            let outbound = nemesis_types::channel::OutboundMessage {
                                channel: msg.channel.clone(),
                                chat_id: msg.chat_id.clone(),
                                content: final_content,
                                message_type: String::new(),
                            };
                            if let Err(e) = tx.send(outbound).await {
                                warn!("[AgentLoop] Failed to send outbound message: {}", e);
                            }
                        }
                    }
                }
                None => {
                    // Channel closed.
                    break;
                }
            }
        }

        self.running.store(false, Ordering::Release);
    }

    /// Same as `run_bus_owned` but takes `Arc<Self>` so the AgentLoop can be
    /// shared with other components (e.g. heartbeat handler) while the bus
    /// loop is running.
    pub async fn run_bus_arc(
        self: Arc<Self>,
        mut inbound_rx: tokio::sync::mpsc::Receiver<nemesis_types::channel::InboundMessage>,
    ) {
        self.running.store(true, Ordering::Release);
        info!("[AgentLoop] Bus consumption loop started");

        while self.running.load(Ordering::Acquire) {
            match inbound_rx.recv().await {
                Some(msg) => {
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

                    let response = match err {
                        Some(e) => format!("Error processing message: {}", e),
                        None => response,
                    };

                    if !response.is_empty() {
                        let already_sent = self.sent_in_round.has_sent_in_round(&msg.session_key);
                        self.sent_in_round.clear(&msg.session_key);

                        if already_sent {
                            debug!(
                                "[AgentLoop] Skipping outbound publish: message tool already sent response for session {}",
                                msg.session_key
                            );
                        } else if let Some(ref tx) = self.outbound_tx {
                            let final_content = if msg.channel == "rpc"
                                && !msg.correlation_id.is_empty()
                                && !response.starts_with(&format!(
                                    "[rpc:{}]",
                                    msg.correlation_id
                                ))
                            {
                                format!("[rpc:{}] {}", msg.correlation_id, response)
                            } else {
                                response
                            };

                            info!(
                                "[AgentLoop] Response message     to {}:{}: {}",
                                msg.channel, msg.chat_id, truncate(&final_content, 80)
                            );

                            let outbound = nemesis_types::channel::OutboundMessage {
                                channel: msg.channel.clone(),
                                chat_id: msg.chat_id.clone(),
                                content: final_content,
                                message_type: String::new(),
                            };
                            if let Err(e) = tx.send(outbound).await {
                                warn!("[AgentLoop] Failed to send outbound message: {}", e);
                            }
                        }
                    }
                }
                None => {
                    break;
                }
            }
        }

        info!("[AgentLoop] Bus consumption loop stopped");
        self.running.store(false, Ordering::Release);
    }

    /// Stop the bus consumption loop.
    /// Mirrors Go's `AgentLoop.Stop()`.
    pub fn stop(&self) {
        info!("[AgentLoop] Stop requested");
        self.running.store(false, Ordering::Release);
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
            let task_failed = original_msg.metadata.get("status")
                .map(|s| s == "error")
                .unwrap_or(false);
            let task_error = original_msg.metadata.get("error")
                .map(|s| s.as_str());

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
    pub async fn process_direct(
        &self,
        content: &str,
        session_key: &str,
    ) -> Result<String, String> {
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
        let trace_id = format!("direct-{}-{}", session_key, chrono::Local::now().timestamp_nanos_opt().unwrap_or(0));
        let start_time = std::time::Instant::now();

        // Emit conversation_start observer event.
        self.emit_observer_sync(crate::loop_executor::ObserverEvent::ConversationStart {
            trace_id: trace_id.clone(),
            session_key: session_key.to_string(),
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
            sender_id: "direct".to_string(),
            content: content.to_string(),
        }).await;

        let instance = self.get_or_create_instance(session_key);
        let context = RequestContext::new(channel, chat_id, "cron", session_key);

        let token = tokio_util::sync::CancellationToken::new();
        let events = self.run_with_trace(&instance, content, &context, &trace_id, false, &token).await;

        // Extract final response for the conversation end event.
        let final_response = events.iter().rev()
            .find_map(|e| if let AgentEvent::Done(msg) = e { Some(msg.clone()) } else { None })
            .unwrap_or_default();

        // Emit conversation_end observer event.
        let duration_ms = start_time.elapsed().as_millis() as u64;
        let rounds = events.iter().filter(|e| matches!(e, AgentEvent::ToolCall(_))).count() as u32 + 1;
        self.emit_observer_sync(crate::loop_executor::ObserverEvent::ConversationEnd {
            trace_id: trace_id.clone(),
            session_key: session_key.to_string(),
            total_rounds: rounds,
            duration_ms,
            content: final_response,
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
        }).await;

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
        let trace_id = format!("heartbeat-{}-{}", chat_id, chrono::Local::now().timestamp_nanos_opt().unwrap_or(0));
        let start_time = std::time::Instant::now();

        // Emit conversation_start observer event.
        self.emit_observer_sync(crate::loop_executor::ObserverEvent::ConversationStart {
            trace_id: trace_id.clone(),
            session_key: "heartbeat".to_string(),
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
            sender_id: "heartbeat".to_string(),
            content: content.to_string(),
        }).await;

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
        let events = self.run_with_trace(&instance, content, &context, &trace_id, false, &token).await;

        // Extract final response for the conversation end event.
        let final_response = events.iter().rev()
            .find_map(|e| if let AgentEvent::Done(msg) = e { Some(msg.clone()) } else { None })
            .unwrap_or_default();

        // Emit conversation_end observer event.
        let duration_ms = start_time.elapsed().as_millis() as u64;
        let rounds = events.iter().filter(|e| matches!(e, AgentEvent::ToolCall(_))).count() as u32 + 1;
        self.emit_observer_sync(crate::loop_executor::ObserverEvent::ConversationEnd {
            trace_id: trace_id.clone(),
            session_key: "heartbeat".to_string(),
            total_rounds: rounds,
            duration_ms,
            content: final_response,
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
        }).await;

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
        let content_preview = truncate(&msg.content, 80);

        // Open a checkpoint turn for the edit safety net (so writer-tool changes
        // during this message can be rewound). No-op when no store is attached.
        let cp_turn = self.turn_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        {
            let cp = self.checkpoint_store.read().as_ref().cloned();
            if let Some(cp) = cp {
                cp.begin(cp_turn, &msg.content);
            }
        }

        info!(
            "[AgentLoop] Processing message from {}:{}: {}",
            msg.channel, msg.sender_id, content_preview
        );

        // Route system messages.
        if msg.channel == "system" {
            // Cluster continuation — return special marker for the bus loop to handle.
            if msg.sender_id
                .starts_with(nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX)
            {
                let task_id = &msg.sender_id[nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX.len()..];
                debug!("[AgentLoop] Cluster continuation message intercepted, task_id={}", task_id);
                return ("__continuation__".to_string(), task_id.to_string(), None);
            }
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

        // Slash commands.
        if let Some(response) = self.handle_command_with_context(&msg.content, &msg.channel) {
            return (String::new(), response, None);
        }

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
            let session_key = if !msg.session_key.is_empty()
                && msg.session_key.starts_with("agent:")
            {
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
            let session_key = if !msg.session_key.is_empty()
                && msg.session_key.starts_with("agent:")
            {
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

        // Session busy check.
        if !self.try_acquire_session(&session_key) {
            warn!(
                "[AgentLoop] Session busy, returning busy message: session_key={}, mode={:?}",
                session_key, self.concurrent_mode
            );
            return (agent_id, BUSY_MESSAGE.to_string(), None);
        }

        // Create cancellation token for this session.
        let cancel_token = self.create_cancel_token(&session_key);

        // Process with the loop, then release.
        let voice_playback = msg.voice_playback.unwrap_or(false);
        // @file expansion: inline referenced file contents before sending to LLM.
        let processed_content = crate::message_preprocess::expand_at_files(
            &msg.content,
            &std::env::current_dir().unwrap_or_default(),
        );
        let result = self
            .run_agent_loop_internal(&session_key, &processed_content, &msg.channel, &msg.chat_id, voice_playback, &cancel_token)
            .await;

        // Clean up cancellation token and release session.
        self.remove_cancel_token(&session_key);
        self.release_session(&session_key);

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
            (
                &msg.chat_id[..idx],
                msg.chat_id[idx + 1..].to_string(),
            )
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
        let result = self
            .run_agent_loop_internal(
                &session_key,
                &format!("[System: {}] {}", msg.sender_id, content),
                origin_channel,
                &origin_chat_id,
                false,
                &cancel_token,
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
                ).await;
                return;
            }
        };

        let limit = req.limit.unwrap_or(20);
        let agent_id = self
            .registry
            .as_ref()
            .and_then(|r| r.default_agent_id())
            .unwrap_or_else(|| "main".to_string());
        let session_key = build_agent_main_session_key(&agent_id);

        // Read history from chat log (separate from session store).
        let (page, total_count, has_more, oldest_index) = crate::chat_log::read_chat_log(
            &session_key, limit, req.before_index,
        );

        self.publish_history_response(
            &msg.chat_id,
            &req.request_id,
            &page,
            has_more,
            oldest_index,
            total_count,
        ).await;
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

        // Session is busy.
        match self.concurrent_mode {
            ConcurrentMode::Reject => false,
            ConcurrentMode::Queue => {
                if state.queue_length >= self.queue_size {
                    return false;
                }
                state.queue_length += 1;
                false
            }
        }
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
            info!("[AgentLoop] Session cancellation requested: {}", session_key);
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
        self.cancel_tokens.insert(session_key.to_string(), token.clone());
        token
    }

    /// Remove the cancellation token for a session after processing completes.
    fn remove_cancel_token(&self, session_key: &str) {
        self.cancel_tokens.remove(session_key);
    }

    // -----------------------------------------------------------------------
    // Summarization
    // -----------------------------------------------------------------------

    /// Trigger summarization if thresholds are met.
    /// Mirrors Go's `maybeSummarize()`.
    ///
    /// In Go this runs in a goroutine (`go func()`) so it doesn't block the
    /// response. We mirror this by spawning a tokio task when summarization
    /// is needed.
    fn maybe_summarize(&self, instance: &AgentInstance, session_key: &str, channel: &str, chat_id: &str) {
        let history = instance.get_history();
        let context_window = instance.context_window();
        let token_estimate = estimate_tokens_for_turns(&history);
        let soft = context_window * COMPACT_SOFT_RATIO / 100;
        let threshold = context_window * COMPACT_SUMMARIZE_RATIO / 100;

        // ⑩ Graded tiers (soft / summarize) + stuck self-check. One critical
        // section. Soft is info-log-only (must NOT pollute the model's input or
        // session_log). Stuck pauses auto-summarization when it isn't reducing
        // the prompt, so we don't loop summarizing forever.
        let mut paused_stuck = false;
        {
            let mut states = self.compact_state.lock();
            // ⑩ Bound growth: clear wholesale under size pressure (best-effort).
            if states.len() > SESSION_STATE_MAX_ENTRIES {
                states.clear();
            }
            let st = states.entry(session_key.to_string()).or_default();

            if token_estimate >= soft && !st.soft_noticed {
                st.soft_noticed = true;
                info!(
                    "[AgentLoop] context at ~{}% of window ({} / {}); summarization will trigger at {}%",
                    token_estimate * 100 / context_window.max(1),
                    token_estimate, context_window, COMPACT_SUMMARIZE_RATIO
                );
            }

            if token_estimate >= threshold && history.len() > 20 {
                if summarize_was_ineffective(st.last_summary_tokens, token_estimate) {
                    st.consecutive_failures += 1;
                } else {
                    st.consecutive_failures = 0;
                }
                if st.consecutive_failures >= COMPACT_STUCK_LIMIT {
                    if !st.stuck {
                        warn!(
                            "[AgentLoop] compaction stuck: summarization has not reduced the prompt {} times in a row; pausing auto-summarization (raise context_window or reduce tool output)",
                            st.consecutive_failures
                        );
                        st.stuck = true;
                    }
                    paused_stuck = true;
                } else {
                    st.last_summary_tokens = token_estimate;
                }
            } else if token_estimate < threshold {
                // Breathing room — clear the stuck latch.
                st.consecutive_failures = 0;
                st.stuck = false;
            }
        }
        if paused_stuck {
            return;
        }

        if history.len() <= 20 && token_estimate <= threshold {
            return;
        }

        let summarize_key = format!("main:{}", session_key);
        {
            let mut map = self.summarizing.lock();
            // Bound growth: clear wholesale under size pressure. Best-effort —
            // the worst case is one redundant concurrent summarization, which
            // overwrites benignly.
            if map.len() > SESSION_STATE_MAX_ENTRIES {
                map.clear();
            }
            if map.contains_key(&summarize_key) {
                return;
            }
            map.insert(summarize_key.clone(), true);
        }

        // Clone all data needed by the spawned task.
        let provider = self.provider.read().clone();         // Arc clone
        let model = self.active_model.read().clone();
        let outbound_tx = self.outbound_tx.clone();   // Option<Sender> clone
        let session_store = self.session_store.clone(); // Option<Arc<SessionStore>> clone
        let summarizing_flag = self.summarizing.clone(); // Arc clone for clearing after completion
        let observer_mgr = self.observer_manager.clone(); // Option<Arc<Manager>> clone
        let history_clone = history;
        let existing_summary = instance.get_summary();
        let session_key_owned = session_key.to_string();
        let channel_owned = channel.to_string();
        let chat_id_owned = chat_id.to_string();
        let clear_key = summarize_key.clone();

        // Spawn async summarization task, mirroring Go's `go func()`.
        tokio::spawn(async move {
            // Notify user if non-internal channel.
            if !is_internal_channel(&channel_owned) {
                if let Some(ref tx) = outbound_tx {
                    let outbound = nemesis_types::channel::OutboundMessage {
                        channel: channel_owned.clone(),
                        chat_id: chat_id_owned.clone(),
                        content: "Memory threshold reached. Optimizing conversation history..."
                            .to_string(),
                        message_type: String::new(),
                    };
                    let _ = tx.send(outbound).await;
                }
            }

            // Perform summarization (self-contained, no &self needed).
            let summary = summarize_history_owned(
                &history_clone,
                &existing_summary,
                context_window,
                provider.as_ref(),
                &model,
                observer_mgr,
            );

            if let Some(summary) = summary {
                // Save summary to session store if available.
                if let Some(ref store) = session_store {
                    let stored_messages: Vec<crate::session::StoredMessage> = history_clone
                        .iter()
                        .map(|m| crate::session::StoredMessage::from(m))
                        .collect();

                    // Keep last 4 messages for continuity, preserving tool message pairs.
                    let retained = truncate_with_tool_pairs(&stored_messages, 4);

                    store.set_history(&session_key_owned, retained);
                    store.set_summary(&session_key_owned, &summary);
                    let _ = store.save(&session_key_owned);
                }
            }

            // Clear the summarizing flag so this session can be re-summarized later.
            // Mirrors Go's `defer al.summarizing.Delete(summarizeKey)`.
            {
                let mut map = summarizing_flag.lock();
                map.remove(&clear_key);
            }
        });
    }

    /// Summarize the conversation history for a session.
    /// Mirrors Go's `summarizeSession()`.
    ///
    /// NOTE: The main loop uses the standalone free functions instead (see
    /// `summarize_history_owned`). Kept as reference implementation.
    #[allow(dead_code)]
    fn summarize_session(&self, instance: &AgentInstance, _session_key: &str) {
        let history = instance.get_history();

        // Keep last 4 messages for continuity.
        if history.len() <= 4 {
            return;
        }

        let to_summarize = &history[..history.len() - 4];

        // Oversized message guard.
        let max_msg_tokens = instance.context_window() / 2;
        let mut valid_messages: Vec<&crate::types::ConversationTurn> = Vec::new();
        let mut omitted = false;

        for m in to_summarize {
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
            return;
        }

        // Multi-part summarization.
        let final_summary = if valid_messages.len() > 10 {
            self.summarize_multipart(&valid_messages)
        } else {
            let existing = instance.get_summary();
            self.summarize_batch(&valid_messages, &existing)
        };

        let final_summary = if omitted && !final_summary.is_empty() {
            format!(
                "{}\n[Note: Some oversized messages were omitted from this summary for efficiency.]",
                final_summary
            )
        } else {
            final_summary
        };

        if !final_summary.is_empty() {
            instance.set_summary(&final_summary);
            instance.truncate_to(4);
        }
    }

    /// Force-compress conversation history by aggressively dropping oldest 50% of messages.
    ///
    /// This is used as a last resort when the context window is exceeded and retry
    /// with compression is needed. Mirrors Go's `forceCompression()`.
    ///
    /// The resulting history structure matches Go's pattern:
    /// 1. System prompt (first message if role == "system")
    /// 2. Compression note
    /// 3. Second half of conversation (kept portion)
    /// 4. Last message (explicitly preserved regardless of the split point)
    pub fn force_compression(&self, instance: &AgentInstance) {
        let history = instance.get_history();
        if history.len() <= 4 {
            return;
        }

        // Keep system prompt (usually [0]) and the very last message (user's trigger).
        // We want to drop the oldest half of the *conversation*.
        // Assuming [0] is system, [1:] is conversation.
        if history.len() < 2 {
            return;
        }
        let conversation = &history[1..history.len() - 1];
        if conversation.is_empty() {
            return;
        }

        let mid = conversation.len() / 2;
        let dropped_count = mid;

        // New history structure:
        // 1. System Prompt
        // 2. [Compression note]
        // 3. Second half of conversation (kept from mid onwards)
        // 4. Last message (always preserved)
        let kept_conversation = &conversation[mid..];

        let mut retained = Vec::new();

        // Always keep the system prompt (first message if role == "system").
        if !history.is_empty() && history[0].role == "system" {
            retained.push(history[0].clone());
        }

        // Add a compression note as a system message.
        let note = crate::types::ConversationTurn {
            role: "system".to_string(),
            content: format!(
                "[System: Emergency compression dropped {} oldest messages due to context limit]",
                dropped_count
            ),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: chrono::Local::now().to_rfc3339(),
            reasoning_content: None,
        };
        retained.push(note);

        // Append the kept portion of conversation.
        for msg in kept_conversation {
            retained.push(msg.clone());
        }

        // Always append the very last message from the original history.
        // This matches Go's explicit `history[len(history)-1]` preservation.
        let last_msg = history.last().unwrap();
        // Only add if not already the last element in retained (avoid duplication).
        if retained.last().map(|m| m.content.as_str()) != Some(&last_msg.content) {
            retained.push(last_msg.clone());
        }

        crate::types::repair_tool_message_pairs(&mut retained);

        let total = history.len();
        instance.set_history(retained);
        info!(
            "[AgentLoop] Force-compressed history: {} messages -> {} messages (dropped {})",
            total,
            instance.get_history().len(),
            dropped_count
        );
    }

    /// Multi-part summarization: split, summarize each half, merge.
    /// NOTE: See `summarize_multipart_owned` for the standalone version used by the main loop.
    #[allow(dead_code)]
    fn summarize_multipart(&self, messages: &[&crate::types::ConversationTurn]) -> String {
        let mid = messages.len() / 2;
        let part1 = &messages[..mid];
        let part2 = &messages[mid..];

        let s1 = self.summarize_batch(part1, "");
        let s2 = self.summarize_batch(part2, "");

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

        let p = self.provider.read().clone();
        let m = self.active_model.read().clone();
        let response = block_on_llm_chat(&*p, &m, llm_messages);

        match response {
            Some(Ok(resp)) if !resp.content.is_empty() => resp.content,
            _ => format!("{} {}", s1, s2),
        }
    }

    /// Summarize a batch of messages using the LLM.
    /// Mirrors Go's `summarizeBatch()`.
    /// NOTE: See `summarize_batch_owned` for the standalone version used by the main loop.
    #[allow(dead_code)]
    fn summarize_batch(&self, batch: &[&crate::types::ConversationTurn], existing_summary: &str) -> String {
        let mut prompt = String::from(
            "Provide a concise summary of this conversation segment, preserving core context and key points.\n",
        );
        if !existing_summary.is_empty() {
            prompt.push_str(&format!("Existing context: {}\n", existing_summary));
        }
        prompt.push_str("\nCONVERSATION:\n");
        for m in batch {
            prompt.push_str(&format!("{}: {}\n", m.role, m.content));
        }

        let messages = vec![LlmMessage {
            role: "user".to_string(),
            content: prompt,
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        }];

        let p = self.provider.read().clone();
        let m = self.active_model.read().clone();
        let response = block_on_llm_chat(&*p, &m, messages);

        match response {
            Some(Ok(resp)) => resp.content,
            Some(Err(e)) => {
                debug!("[AgentLoop] summarize_batch LLM call failed: {}", e);
                String::new()
            }
            None => String::new(),
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

        // Restore history from session store if available.
        // Mirrors Go's `agent.Sessions.Get(sessionKey)` in `getOrCreateInstance`.
        if let Some(ref store) = self.session_store {
            let stored = store.get_or_create(session_key);
            let existing_summary = store.get_summary(session_key);
            if !stored.messages.is_empty() {
                let history: Vec<crate::types::ConversationTurn> = stored.messages
                    .into_iter()
                    .map(|m| m.into())
                    .collect();
                instance.set_history(history);
            }
            if !existing_summary.is_empty() {
                instance.set_summary(&existing_summary);
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
    ) -> Result<String, String> {
        // Generate trace ID and emit conversation_start event.
        let trace_id = format!("{}-{}", session_key, chrono::Local::now().timestamp_nanos_opt().unwrap_or(0));
        let start_time = std::time::Instant::now();

        // Emit conversation_start observer event.
        self.emit_observer_sync(crate::loop_executor::ObserverEvent::ConversationStart {
            trace_id: trace_id.clone(),
            session_key: session_key.to_string(),
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
            sender_id: "agent".to_string(),
            content: user_message.to_string(),
        }).await;

        // Record last channel (skip internal channels).
        if !channel.is_empty() && !chat_id.is_empty() && !is_internal_channel(channel) {
            let channel_key = format!("{}:{}", channel, chat_id);
            self.record_last_channel(&channel_key);
        }

        let instance = self.get_or_create_instance(session_key);
        let context = RequestContext::new(channel, chat_id, "agent", session_key);

        let events = self.run_with_trace(&instance, user_message, &context, &trace_id, voice_playback, cancel_token).await;

        // Maybe trigger summarization.
        self.maybe_summarize(&instance, session_key, channel, chat_id);

        // Persist to session store — mirrors Go's runAgentLoop exactly:
        //   Line 104: agent.Sessions.AddMessage(sessionKey, "user", userMessage)
        //   Line 151: agent.Sessions.AddMessage(sessionKey, "assistant", finalContent)
        //   Line 152: agent.Sessions.Save(sessionKey)
        //
        // Session file only stores user + final assistant (conversation log).
        // Instance history (in-memory) keeps all messages for LLM context.
        // These are intentionally separate, matching Go's architecture.

        // Extract final response once (shared by session store, chat log, and observer).
        let final_response = events.iter().rev()
            .find_map(|e| if let AgentEvent::Done(msg) = e { Some(msg.clone()) }
                          else if let AgentEvent::Error(msg) = e { Some(msg.clone()) }
                          else { None })
            .unwrap_or_default();

        if let Some(ref store) = self.session_store {
            // Ensure session exists in store.
            store.get_or_create(session_key);

            // Add user message.
            store.add_message(session_key, "user", user_message);

            // Add final assistant response.
            store.add_message(session_key, "assistant", &final_response);

            // Save summary if available.
            let summary = instance.get_summary();
            if !summary.is_empty() {
                store.set_summary(session_key, &summary);
            }

            if let Err(e) = store.save(session_key) {
                warn!("[AgentLoop] Failed to persist session history for {}: {}", session_key, e);
            }
        }

        // Append to chat log (independent of session store).
        crate::chat_log::append_chat_log(session_key, "user", user_message);
        crate::chat_log::append_chat_log(session_key, "assistant", &final_response);

        // Emit conversation_end observer event.
        let duration_ms = start_time.elapsed().as_millis() as u64;
        let rounds = events.iter().filter(|e| matches!(e, AgentEvent::ToolCall(_))).count() as u32 + 1;
        self.emit_observer_sync(crate::loop_executor::ObserverEvent::ConversationEnd {
            trace_id: trace_id.clone(),
            session_key: session_key.to_string(),
            total_rounds: rounds,
            duration_ms,
            content: final_response,
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
        }).await;

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
        let trace_id = format!("run-{}", chrono::Local::now().timestamp_nanos_opt().unwrap_or(0));
        let token = tokio_util::sync::CancellationToken::new();
        self.run_with_trace(instance, user_message, context, &trace_id, false, &token).await
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
    ) -> Vec<AgentEvent> {
        // Add user message to instance history.
        instance.add_user_message(user_message);
        instance.set_state(crate::types::AgentState::Thinking);

        self.run_llm_loop(instance, context, trace_id, voice_playback, cancel_token).await
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
        self.run_llm_loop(instance, context, trace_id, false, &token).await
    }

    /// Core LLM loop shared by `run_with_trace()` and `resume_execution()`.
    async fn run_llm_loop(
        &self,
        instance: &AgentInstance,
        context: &RequestContext,
        trace_id: &str,
        voice_playback: bool,
        cancel_token: &tokio_util::sync::CancellationToken,
    ) -> Vec<AgentEvent> {
        let mut events = Vec::new();

        // Chat options matching Go's defaults: max_tokens: 8192, temperature: 0.7.
        let chat_opts = crate::types::ChatOptions {
            max_tokens: Some(8192),
            temperature: Some(0.7),
            ..Default::default()
        };

        let mut turns_used = 0u32;
        // Phase 2 (small-model-tool-robustness): per-request consecutive
        // validation-failure counter. Reset on any successful (valid or
        // auto-fixed) tool call; incremented on each schema violation. When it
        // reaches the tier budget the loop stops, preventing a struggling model
        // from burning max_turns on the same malformed call.
        let mut validation_failures = 0u32;
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

        loop {
            // Auto-reload MCP tools if config file changed.
            self.check_mcp_reload();
            // Phase 4a: re-resolve capability tier if config.json changed on
            // disk (dashboard model add, CLI `model set-tier` while running).
            self.check_config_reload();

            // Check cancellation at the top of each iteration.
            if cancel_token.is_cancelled() {
                info!("[AgentLoop] LLM loop cancelled at top of iteration, turns_used={}", turns_used);
                events.push(AgentEvent::Done("已取消".to_string()));
                break;
            }

            // ①/② max_turns cap + grace round. max_turns == 0 means unlimited
            // (opt-in). On the first hit we grant one grace round (with
            // GRACE_ROUND_NUDGE injected below) so the model can finalize from
            // completed work; a second hit stops resumably — no work is lost.
            if self.config.max_turns > 0 && turns_used >= self.config.max_turns {
                if !grace_round {
                    grace_round = true;
                    info!(
                        "[AgentLoop] max_turns ({}) reached after {} turns; granting one grace round to finalize",
                        self.config.max_turns, turns_used
                    );
                    // Fall through: this iteration runs as the grace round.
                } else {
                    warn!(
                        "[AgentLoop] paused after {} tool-call rounds (grace round exhausted)",
                        self.config.max_turns
                    );
                    events.push(AgentEvent::Done(format!(
                        "已在 {} 轮工具调用后暂停，已完成的工作已保存。发送下一条消息可继续，或调大 max_tool_iterations（设为 0 表示不限）。",
                        self.config.max_turns
                    )));
                    break;
                }
            }

            // Build the message list from instance history.
            let mut messages = self.build_messages(instance);

            // Voice playback prompt injection: append to last user message (not stored in history).
            if voice_playback {
                if let Some(last_user) = messages.iter_mut().rev().find(|m| m.role == "user") {
                    last_user.content.push_str("（语音播报模式已开启，请用简洁、便于口语播报的方式回复，避免使用代码块、表格等不适合语音的内容。）");
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
            }

            debug!("[AgentLoop] Sending {} messages to LLM", messages.len());

            // Build tool definitions from registered tools for LLM function calling.
            // Mirrors Go's ToolRegistry.ToProviderDefs() which calls tool.Description() and tool.Parameters().
            // Sort by name so the order is stable across runs — a deterministic
            // tool order gives reproducible behaviour and avoids unnecessary prompt
            // variation between requests.
            let tool_defs: Vec<crate::types::ToolDefinition> = {
                let tools_guard = self.tools.read();
                // Phase 3 (small-model-tool-robustness): tier-based toolset.
                // Empty allowed-list (Big/Auto) = show everything; Mini/Normal
                // see a restricted set to reduce small-model cognitive load.
                let allowed = nemesis_types::capability::tier_allowed_tools(*self.tier.read());
                let mut names: Vec<&String> = tools_guard.keys().collect();
                names.sort();
                names.into_iter()
                    .filter(|name| allowed.is_empty() || allowed.contains(&name.as_str()))
                    .filter_map(|name| tools_guard.get(name).map(|tool| (name, tool)))
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
            debug!("[AgentLoop] Sending {} tool definitions to LLM", tool_defs.len());

            // Emit LLM request observer event.
            let msg_values: Vec<serde_json::Value> = messages.iter()
                .filter_map(|m| serde_json::to_value(m).ok())
                .collect();
            let tool_values: Vec<serde_json::Value> = tool_defs.iter()
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
            }).await;

            // Call LLM.
            instance.set_state(crate::types::AgentState::Thinking);
            let round_start = std::time::Instant::now();
            // Clone provider Arc so RwLock guard is dropped before .await.
            let active_provider = self.provider.read().clone();

            // Use tokio::select! to allow cancellation during the LLM call.
            let chat_result = tokio::select! {
                result = active_provider.chat(&active_model, messages, Some(chat_opts.clone()), tool_defs) => result,
                _ = cancel_token.cancelled() => {
                    info!("[AgentLoop] LLM call cancelled while waiting for response, turns_used={}", turns_used);
                    events.push(AgentEvent::Done("已取消".to_string()));
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

                            // Force-compress: drop oldest 50% of messages.
                            self.force_compression(instance);

                            // Rebuild messages from compressed history.
                            let mut compressed_messages = self.build_messages(instance);

                            // Re-apply voice playback prompt after compression.
                            if voice_playback {
                                if let Some(last_user) = compressed_messages.iter_mut().rev().find(|m| m.role == "user") {
                                    last_user.content.push_str("（语音播报模式已开启，请用简洁、便于口语播报的方式回复，避免使用代码块、表格等不适合语音的内容。）");
                                }
                            }
                            debug!(
                                "[AgentLoop] Retry {}: sending {} messages after compression",
                                retry_count,
                                compressed_messages.len()
                            );

                            let retry_tool_defs: Vec<crate::types::ToolDefinition> = self.tools.read().iter()
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

                            match active_provider.chat(&active_model, compressed_messages, Some(chat_opts.clone()), retry_tool_defs).await {
                                Ok(resp) => {
                                    got_response = Some(resp);
                                    break;
                                }
                                Err(e) => {
                                    retry_err = e;
                                    warn!("[AgentLoop] LLM retry {} failed: {}", retry_count, retry_err);
                                }
                            }
                        }

                        match got_response {
                            Some(resp) => resp,
                            None => {
                                warn!("[AgentLoop] All LLM retries exhausted: {}", retry_err);
                                let error_round = turns_used + 1;
                                let error_duration = round_start.elapsed();
                                self.emit_observer_sync(crate::loop_executor::ObserverEvent::LlmResponse {
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
                                }).await;
                                instance.add_assistant_message(
                                    &format!("Error: {}", retry_err),
                                    Vec::new(),
                                    None,
                                );
                                let formatted = context.format_rpc_message(&format!("Error: {}", retry_err));
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
                            "timeout", "timed out", "connection reset",
                            "broken pipe", "connect error", "connection refused",
                            "temporarily unavailable", "reset by peer",
                            "502", "503", "504", "service unavailable",
                        ].iter().any(|k| err_lower.contains(k));

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
                                let r_tools: Vec<crate::types::ToolDefinition> = self.tools.read().iter()
                                    .map(|(name, tool)| crate::types::ToolDefinition {
                                        tool_type: "function".to_string(),
                                        function: crate::types::ToolFunctionDef {
                                            name: name.clone(),
                                            description: tool.description(),
                                            parameters: tool.parameters(),
                                        },
                                    })
                                    .collect();
                                match active_provider.chat(&active_model, r_msgs, Some(chat_opts.clone()), r_tools).await {
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
                            self.emit_observer_sync(crate::loop_executor::ObserverEvent::LlmResponse {
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
                            }).await;
                            instance.add_assistant_message(&format!("Error: {}", last_err), Vec::new(), None);
                            let formatted = context.format_rpc_message(&format!("Error: {}", last_err));
                            events.push(AgentEvent::Error(formatted));
                            break;
                        }
                    }
                }
            };
            turns_used += 1;

            // Emit LLM response observer event.
            let round_duration = round_start.elapsed();
            let tc_values: Vec<serde_json::Value> = response.tool_calls.iter()
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
                finish_reason: if response.finished { Some("stop".to_string()) } else { None },
                usage: response.usage.clone(),
                raw_request_body: response.raw_request_body.take(),
                raw_response_body: response.raw_response_body.take(),
            }).await;

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
                        cache_read_tokens: usage.cache_read_tokens.or(usage.cached_tokens).unwrap_or(0),
                        total_cost_usd: 0.0,
                        latency_ms: round_duration.as_millis() as i64,
                        status_code: if response.content.starts_with("Error:") { 500 } else { 200 },
                        error_message: None,
                        is_streaming: false,
                        created_at: chrono::Local::now().timestamp(),
                    };
                    if let Err(e) = ds.insert_request_log(&log) {
                        tracing::warn!("[AgentLoop] Failed to record usage: {e}");
                    }
                }
            }

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
                    instance.add_assistant_message(&content, Vec::new(), response.reasoning_content.clone());
                    let formatted = context.format_rpc_message(&content);
                    events.push(AgentEvent::Done(formatted));
                    break;
                }
                match turn_guard.check_final_answer(&content) {
                    crate::turn_guard::FinalAnswerVerdict::Accept => {
                        instance.add_assistant_message(&content, Vec::new(), response.reasoning_content.clone());
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
                        instance.add_assistant_message(&content, Vec::new(), response.reasoning_content.clone());
                        degenerate_nudge_pending = Some(nudge);
                        continue;
                    }
                    crate::turn_guard::FinalAnswerVerdict::GiveUp(notice) => {
                        warn!("[AgentLoop] degenerate final answer retry budget exhausted; giving up");
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
            instance.add_assistant_message(&assistant_content, tool_calls.clone(), response.reasoning_content.clone());
            events.push(AgentEvent::ToolCall(tool_calls.clone()));

            // Execute each tool call.
            instance.set_state(crate::types::AgentState::ExecutingTool);
            let mut hit_async = false;
            for tc in &tool_calls {
                // Check cancellation before each tool execution.
                if cancel_token.is_cancelled() {
                    info!("[AgentLoop] LLM loop cancelled before tool execution: {}, turns_used={}", tc.name, turns_used);
                    events.push(AgentEvent::Done("已取消".to_string()));
                    break;
                }

                let tool_start = std::time::Instant::now();
                // Phase 2 (small-model-tool-robustness): validate args against
                // the tool's schema before dispatch. Catches B-class failures;
                // auto-fixes high-confidence field-name typos (edit distance ≤2);
                // otherwise bounces a structured error back to the model so it
                // can self-correct on the next round.
                let result = match self.check_tool_args(tc) {
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
                let tool_duration = tool_start.elapsed();

                // Emit tool call observer event.
                self.emit_observer_sync(crate::loop_executor::ObserverEvent::ToolCall {
                    trace_id: trace_id.to_string(),
                    tool_name: tc.name.clone(),
                    success: !result.starts_with("Error:") && !result.starts_with("Tool error:"),
                    duration_ms: tool_duration.as_millis() as u64,
                    round: turns_used,
                    arguments: tc.arguments.clone(),
                    result: result.clone(),
                }).await;

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
                    let parts: Vec<String> = result.splitn(4, ':')
                        .map(|s| s.to_string())
                        .collect();
                    if parts.len() >= 3 {
                        let task_id = parts[1].clone();
                        let target_id = parts[2].clone();
                        let target_name = parts.get(3)
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
                                ).await;
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
                        let intermediate = format!(
                            "已经联系 {} 了，稍等~",
                            target_name
                        );
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
                let result = if tool_succeeded {
                    match turn_guard.record_write_success(&tc.name, &tc.arguments) {
                        Some(nudge) => {
                            info!(
                                "[AgentLoop] loop guard: '{}' repeated an identical write; nudging",
                                tc.name
                            );
                            format!("{}\n{}", result, nudge)
                        }
                        None => result,
                    }
                } else {
                    result
                };

                // ⑥ Alternating-loop guard: per-turn (tool, error) failure
                // frequency, NOT reset by intervening successes (also handles ④
                // storm — consecutive identical — internally). On a repeated
                // failure, append a nudge so the model sees it in the error.
                let error_for_guard: Option<&str> = if tool_succeeded { None } else { Some(&result) };
                let fed_result = match turn_guard.record_tool_outcome(&tc.name, error_for_guard) {
                    Some(nudge) => {
                        info!(
                            "[AgentLoop] loop guard: '{}' repeating the same failure within this turn; nudging",
                            tc.name
                        );
                        format!("{}\n{}", result, nudge)
                    }
                    None => result,
                };
                instance.add_tool_result(&tc.id, &fed_result);

                // ⑥ Escalation: same (tool, error) failed past the hard-stop
                // threshold → nudges are being ignored. Stop the turn now to
                // bound the worst-case cost (a stuck small model would otherwise
                // run all the way to max_turns). Resumable, like the grace round.
                if let Some(msg) = turn_guard.escalation_check() {
                    warn!(
                        "[AgentLoop] loop guard escalation: stopping turn to avoid burning max_turns on a stuck loop"
                    );
                    let formatted = context.format_rpc_message(&msg);
                    events.push(AgentEvent::Done(formatted));
                    break;
                }

                // Phase 2: bound consecutive validation failures so a struggling
                // model cannot burn the whole max_turns budget on the same
                // malformed arguments.
                if validation_failures >= self.validation_retry_budget() {
                    warn!(
                        "[AgentLoop] Validation retry budget exhausted ({}); stopping loop.",
                        validation_failures
                    );
                    events.push(AgentEvent::Error(format!(
                        "工具参数校验连续失败 {} 次，已停止重试。最近工具：'{}'。",
                        validation_failures, tc.name
                    )));
                    break;
                }
            }

            if hit_async {
                break;
            }
        }

        instance.set_state(crate::types::AgentState::Idle);
        events
    }

    // -----------------------------------------------------------------------
    // Tool handling
    // -----------------------------------------------------------------------

    /// Execute a single tool call.
    pub async fn handle_tool_call(
        &self,
        tool_call: &ToolCallInfo,
        context: &RequestContext,
    ) -> String {
        info!("[AgentLoop] Executing tool: {} (id={})", tool_call.name, tool_call.id);

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
                let reason_str = reason.unwrap_or_else(|| "operation denied by security policy".to_string());
                warn!("[AgentLoop] Security blocked tool {}: {}", tool_call.name, reason_str);
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
        let result = match tool_opt {
            Some(tool) => match tool.execute(&tool_call.arguments, context).await {
                Ok(result) => {
                    debug!("[AgentLoop] Tool {} returned: {} bytes", tool_call.name, result.len());
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

        // Record experience for Forge self-learning (non-blocking).
        #[cfg(feature = "forge")]
        {
            if let Some(ref forge) = self.forge {
                let exp = nemesis_types::forge::Experience {
                    id: uuid::Uuid::new_v4().to_string(),
                    tool_name: tool_call.name.clone(),
                    input_summary: tool_call.arguments.clone(),
                    output_summary: result.clone(),
                    success: !result.contains("SECURITY BLOCKED") && !result.contains("Tool error:"),
                    duration_ms: tool_start.elapsed().as_millis() as u64,
                    timestamp: chrono::Local::now().to_rfc3339(),
                    session_key: format!("{}:{}", context.channel, context.chat_id),
                };
                let args = serde_json::from_str(&tool_call.arguments)
                    .unwrap_or(serde_json::Value::Null);
                let _ = forge.collector().record_with_args(exp, &args).await;
            }
        }

        result
    }

    /// Phase 2: check a tool call's arguments against the registered tool's
    /// schema. Returns Valid / Fixed / Invalid. Unknown tools return Valid so
    /// the existing unknown-tool path in `handle_tool_call` reports them
    /// (class C, not a schema failure).
    fn check_tool_args(&self, tool_call: &ToolCallInfo) -> crate::args_validator::Outcome {
        let schema_opt = self.tools.read().get(&tool_call.name).map(|t| t.parameters());
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
                nemesis_types::capability::detect_tier(
                    &nemesis_types::capability::TierHint {
                        full_model: Some(active.clone()),
                        real_name: None,
                        size_b: None,
                    },
                )
            });
        if *self.tier.read() != tier {
            info!(
                "[AgentLoop] Active model '{}' → capability tier {} (re-resolved from config.json)",
                active, tier
            );
            *self.tier.write() = tier;
        }
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
        let mtime = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok();
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

    /// Build the LLM message list from the instance conversation history.
    ///
    /// Injects an ephemeral "# Current Time / # Environment" system message
    /// immediately before the latest user message. The historical prefix (system
    /// prompt + earlier turns) stays byte-identical across requests, preserving
    /// prompt-cache hits; only the trailing user message and the dynamic marker
    /// are billed at the cache-miss rate. The platform/shell hint steers the
    /// model away from interactive commands that hang the exec tool (e.g. bare
    /// Windows `date` vs `date /t`) — small-model-tool-robustness plan Phase 1.
    pub fn build_messages(&self, instance: &AgentInstance) -> Vec<LlmMessage> {
        let history = instance.get_history();

        let now = chrono::Local::now().format("%Y-%m-%d %H:%M (%A)").to_string();
        #[cfg(target_os = "windows")]
        let env_hint = "platform: windows\ndefault_shell: cmd\ntime_cmd: use `date /t` or `echo %date% %time%` or PowerShell `Get-Date`";
        #[cfg(not(target_os = "windows"))]
        let env_hint = "platform: unix\ndefault_shell: sh\ntime_cmd: use `date`";
        let dyn_msg = LlmMessage {
            role: "system".to_string(),
            content: format!("# Current Time\n{}\n# Environment\n{}", now, env_hint),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        };

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
        // is a system prompt at history[0] to protect (otherwise there's no
        // cached prefix to preserve).
        let last_user_idx = history
            .iter()
            .rposition(|t| t.role == "user")
            .filter(|&i| i > 0)
            .filter(|_| history.first().map_or(false, |t| t.role == "system"));

        match last_user_idx {
            Some(idx) => {
                let mut messages: Vec<LlmMessage> = Vec::with_capacity(history.len() + 1);
                messages.extend(history[..idx].iter().map(turn_to_msg));
                messages.push(dyn_msg);
                messages.extend(history[idx..].iter().map(turn_to_msg));
                messages
            }
            None => history.iter().map(turn_to_msg).collect(),
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
            debug!("[AgentLoop] Cluster continuation message intercepted: {}", content);
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

/// Truncate message list to last `keep_count`, preserving tool message pairs.
/// Operates on `StoredMessage` (session store layer).
fn truncate_with_tool_pairs(
    messages: &[crate::session::StoredMessage],
    keep_count: usize,
) -> Vec<crate::session::StoredMessage> {
    if messages.len() <= keep_count {
        return messages.to_vec();
    }

    let start = messages.len() - keep_count;
    let mut retained: Vec<crate::session::StoredMessage> = messages[start..].to_vec();

    while !retained.is_empty() && retained[0].role == "tool" {
        let tool_call_id = retained[0].tool_call_id.clone();

        if let Some(ref tc_id) = tool_call_id {
            let mut found = false;
            if start > 0 {
                for i in (0..start).rev() {
                    if messages[i].role == "assistant" {
                        if messages[i].tool_calls.iter().any(|tc| tc.id == *tc_id) {
                            retained.insert(0, messages[i].clone());
                            found = true;
                            break;
                        }
                    }
                }
            }
            if found {
                break;
            }
        }
        retained.remove(0);
    }

    if !retained.is_empty() {
        // Check ALL assistant messages for incomplete tool_calls.
        // An assistant has tool_calls but no corresponding tool responses
        // means the responses were cut off by truncation.
        let n = retained.len();
        for i in 0..n {
            if retained[i].role == "assistant" && !retained[i].tool_calls.is_empty() {
                let call_ids: Vec<&str> = retained[i].tool_calls.iter().map(|tc| tc.id.as_str()).collect();
                let has_responses = retained[i + 1..].iter().any(|m| {
                    m.role == "tool"
                        && m.tool_call_id.as_ref().map_or(false, |id| call_ids.contains(&id.as_str()))
                });
                if !has_responses {
                    retained[i].tool_calls.clear();
                }
            }
        }
    }

    retained
}

/// Standalone summarization function that can run in a spawned task.
/// Takes ownership of all data it needs (history, provider Arc, model).
/// Returns `Some(summary)` if summarization was performed, `None` if skipped.
fn summarize_history_owned(
    history: &[crate::types::ConversationTurn],
    existing_summary: &str,
    context_window: usize,
    provider: &dyn LlmProvider,
    model: &str,
    observer_manager: Option<Arc<nemesis_observer::Manager>>,
) -> Option<String> {
    // Keep last 4 messages for continuity.
    if history.len() <= 4 {
        return None;
    }

    let to_summarize = &history[..history.len() - 4];

    // Oversized message guard.
    let max_msg_tokens = context_window / 2;
    let mut valid_messages: Vec<&crate::types::ConversationTurn> = Vec::new();
    let mut omitted = false;

    for m in to_summarize {
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

    // Multi-part summarization.
    let final_summary = if valid_messages.len() > 10 {
        summarize_multipart_owned(&valid_messages, provider, model, observer_manager)
    } else {
        summarize_batch_owned(&valid_messages, existing_summary, provider, model, observer_manager)
    };

    let final_summary = if omitted && !final_summary.is_empty() {
        format!(
            "{}\n[Note: Some oversized messages were omitted from this summary for efficiency.]",
            final_summary
        )
    } else {
        final_summary
    };

    if final_summary.is_empty() {
        None
    } else {
        Some(final_summary)
    }
}

/// Multi-part summarization (standalone, works in spawned task).
fn summarize_multipart_owned(
    messages: &[&crate::types::ConversationTurn],
    provider: &dyn LlmProvider,
    model: &str,
    observer_manager: Option<Arc<nemesis_observer::Manager>>,
) -> String {
    let mid = messages.len() / 2;
    let part1 = &messages[..mid];
    let part2 = &messages[mid..];

    let s1 = summarize_batch_owned(part1, "", provider, model, observer_manager.clone());
    let s2 = summarize_batch_owned(part2, "", provider, model, observer_manager.clone());

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
        || block_on_llm_chat(provider, model, llm_messages),
    );

    match response {
        Some(Ok(resp)) if !resp.content.is_empty() => resp.content,
        _ => format!("{} {}", s1, s2),
    }
}

/// Single-batch summarization (standalone, works in spawned task).
fn summarize_batch_owned(
    batch: &[&crate::types::ConversationTurn],
    existing_summary: &str,
    provider: &dyn LlmProvider,
    model: &str,
    observer_manager: Option<Arc<nemesis_observer::Manager>>,
) -> String {
    let mut prompt = String::from(
        "Provide a concise summary of this conversation segment, preserving core context and key points.\n",
    );
    if !existing_summary.is_empty() {
        prompt.push_str(&format!("Existing context: {}\n", existing_summary));
    }
    prompt.push_str("\nCONVERSATION:\n");
    for m in batch {
        prompt.push_str(&format!("{}: {}\n", m.role, m.content));
    }

    let messages = vec![LlmMessage {
        role: "user".to_string(),
        content: prompt,
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
    }];

    let response = emit_observer_events_around_llm(
        observer_manager.as_ref(),
        "summarize-batch",
        model,
        || block_on_llm_chat(provider, model, messages),
    );

    match response {
        Some(Ok(resp)) => resp.content,
        Some(Err(e)) => {
            debug!("[AgentLoop] summarize_batch_owned LLM call failed: {}", e);
            String::new()
        }
        None => String::new(),
    }
}

// ---------------------------------------------------------------------------
// Helper: block_on for LLM calls from sync context
// ---------------------------------------------------------------------------

/// Run an async LLM call in a blocking context.
/// Uses tokio::task::block_in_place if inside a runtime, otherwise creates one.
fn block_on_llm_chat(
    provider: &dyn LlmProvider,
    model: &str,
    messages: Vec<LlmMessage>,
) -> Option<Result<LlmResponse, String>> {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            Some(tokio::task::block_in_place(|| {
                handle.block_on(provider.chat(model, messages, None, vec![]))
            }))
        }
        Err(_) => {
            let rt = match tokio::runtime::Runtime::new() {
                Ok(r) => r,
                Err(_) => return None,
            };
            Some(rt.block_on(provider.chat(model, messages, None, vec![])))
        }
    }
}

/// Emit observer events (ConversationStart, LlmRequest, LlmResponse, ConversationEnd)
/// around a synchronous LLM call closure. Used by standalone summarization functions.
fn emit_observer_events_around_llm<F>(
    observer_manager: Option<&Arc<nemesis_observer::Manager>>,
    label: &str,
    model: &str,
    llm_call: F,
) -> Option<Result<LlmResponse, String>>
where
    F: FnOnce() -> Option<Result<LlmResponse, String>>,
{
    use crate::loop_executor::ObserverEvent;

    // Generate trace_id for this summarization LLM call.
    let trace_id = format!(
        "{}-{}",
        label,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );

    // Emit ConversationStart and LlmRequest before the call.
    if let Some(mgr) = observer_manager {
        let start_event = ObserverEvent::ConversationStart {
            trace_id: trace_id.clone(),
            session_key: label.to_string(),
            channel: String::new(),
            chat_id: String::new(),
            sender_id: "summarizer".to_string(),
            content: String::new(),
        };
        let conv_event = start_event.to_conversation_event();
        match tokio::runtime::Handle::try_current() {
            Ok(_handle) => {
                // Inside a tokio runtime — must use block_in_place to avoid
                // "Cannot start a runtime from within a runtime" panic.
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(mgr.emit_sync(conv_event));
                });
            }
            Err(_) => {
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(mgr.emit_sync(conv_event));
            }
        }

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
        let conv_event = request_event.to_conversation_event();
        let mgr_clone = Arc::clone(mgr);
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(async move {
                    mgr_clone.emit(conv_event).await;
                });
            }
            Err(_) => {
                // No runtime available, just skip async emit for request
            }
        }
    }

    // Execute the LLM call.
    let start = std::time::Instant::now();
    let mut response = llm_call();
    let duration_ms = start.elapsed().as_millis() as u64;

    // Extract response content and raw fields for observer events.
    let (response_content, raw_req, raw_resp) = match &mut response {
        Some(Ok(r)) => {
            let content = r.content.clone();
            let req = r.raw_request_body.take();
            let resp = r.raw_response_body.take();
            (content, req, resp)
        }
        _ => (String::new(), None, None),
    };

    // Emit LlmResponse and ConversationEnd after the call.
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
        let conv_event = response_event.to_conversation_event();
        // Use emit_sync (not async) to guarantee LlmResponse is fully
        // processed before ConversationEnd removes the ConversationState.
        match tokio::runtime::Handle::try_current() {
            Ok(_handle) => {
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(mgr.emit_sync(conv_event));
                });
            }
            Err(_) => {
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(mgr.emit_sync(conv_event));
            }
        }

        let end_event = ObserverEvent::ConversationEnd {
            trace_id,
            session_key: label.to_string(),
            total_rounds: 1,
            duration_ms,
            content: response_content,
            channel: String::new(),
            chat_id: String::new(),
        };
        let conv_event = end_event.to_conversation_event();
        match tokio::runtime::Handle::try_current() {
            Ok(_handle) => {
                // Inside a tokio runtime — must use block_in_place to avoid
                // "Cannot start a runtime from within a runtime" panic.
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(mgr.emit_sync(conv_event));
                });
            }
            Err(_) => {
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(mgr.emit_sync(conv_event));
            }
        }
    }

    response
}

// ---------------------------------------------------------------------------
// Cluster integration helpers
// ---------------------------------------------------------------------------

/// Extract the task ID from a cluster continuation sender ID.
///
/// The format is `cluster_continuation:{taskID}`.
#[cfg(test)]
pub fn extract_continuation_task_id(sender_id: &str) -> Option<&str> {
    sender_id
        .strip_prefix(nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX)
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
    let (parent_peer_kind, parent_peer_id) = input.parent_peer.as_ref().and_then(|pp| {
        if let Some(colon_pos) = pp.find(':') {
            Some((Some(pp[..colon_pos].to_string()), Some(pp[colon_pos + 1..].to_string())))
        } else {
            None
        }
    }).unwrap_or((None, None));

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
                result.push_str(&format!(
                    "    - ID: {}, Name: {}\n",
                    tc.id, tc.name
                ));
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

#[cfg(test)]
mod tests;

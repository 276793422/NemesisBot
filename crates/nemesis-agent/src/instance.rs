//! Agent instance: manages conversation history and state for a single session.
//!
//! Each `AgentInstance` tracks the full conversation history for one session.
//! History is strictly append-only in memory (summarization/compression no
//! longer mutate it); bounding is the session store's responsibility (drop
//! oldest with a C-aware index adjustment — see `SessionStore::trim_to_limit`).

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::types::{AgentConfig, AgentState, ConversationTurn, ToolCallInfo};
use tracing::debug;

/// Monotonically increasing instance counter for unique IDs.
static INSTANCE_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Cached conversation summary covering a contiguous prefix of history.
///
/// Invariant: `text` summarizes `history[..covers_up_to]`, and `build_messages`
/// sends `history[covers_up_to..]` verbatim — every message is either
/// summarized (in `text`) or sent verbatim, with no gap and no overlap.
///
/// `covers_up_to` is an index into the full `AgentInstance` history vector
/// (including the system prompt at index 0). The sole writer is the
/// summarization path (`maybe_update_summary` / `force_compression`);
/// `build_messages` only reads.
#[derive(Debug, Clone)]
pub struct SummaryCache {
    /// Summary covers `history[..covers_up_to]`.
    pub covers_up_to: usize,
    /// The summary text (empty summary ⇒ no cache).
    pub text: String,
}

/// An agent instance that manages conversation history for a single session.
pub struct AgentInstance {
    /// Unique instance identifier.
    id: u64,
    /// Agent configuration.
    config: AgentConfig,
    /// Conversation history.
    history: Mutex<Vec<ConversationTurn>>,
    /// Current agent state.
    state: Mutex<AgentState>,
    /// Optional metadata attached to this instance.
    metadata: Mutex<serde_json::Value>,
    /// Cached summary covering a history prefix (`SummaryCache`). None when
    /// no summary is cached. See [`SummaryCache`] for the gap-free invariant.
    summary_cache: Mutex<Option<SummaryCache>>,
    /// Context window size for token-based summarization thresholds.
    context_window: usize,
    /// Workspace directory path for this agent.
    /// Mirrors Go's AgentInstance.Workspace.
    workspace: PathBuf,
    /// Maximum tool-call iterations per request.
    /// Mirrors Go's AgentInstance.MaxIterations (default 20).
    max_iterations: u32,
    /// Sub-agent allow list (agent IDs or "*" for all).
    /// Mirrors Go's AgentInstance.Subagents.
    subagents: Mutex<Vec<String>>,
    /// Skills filter: only load skills matching these names.
    /// Mirrors Go's AgentInstance.SkillsFilter.
    skills_filter: Mutex<Vec<String>>,
    /// Fallback model candidates for retry on provider errors.
    /// Mirrors Go's AgentInstance.Candidates.
    fallback_candidates: Mutex<Vec<String>>,
    /// Provider metadata (name, masked API key, base URL) for logging.
    /// Mirrors Go's AgentInstance.ProviderMeta.
    provider_meta: Mutex<Option<serde_json::Value>>,
}

impl AgentInstance {
    /// Create a new agent instance with the given configuration.
    pub fn new(config: AgentConfig) -> Self {
        let id = INSTANCE_COUNTER.fetch_add(1, Ordering::Relaxed);
        debug!("[AgentInstance] Created instance id={}", id);
        let instance = Self {
            id,
            config,
            history: Mutex::new(Vec::new()),
            state: Mutex::new(AgentState::Idle),
            metadata: Mutex::new(serde_json::Value::Null),
            summary_cache: Mutex::new(None),
            context_window: 32000,
            workspace: PathBuf::new(),
            max_iterations: 60,
            subagents: Mutex::new(Vec::new()),
            skills_filter: Mutex::new(Vec::new()),
            fallback_candidates: Mutex::new(Vec::new()),
            provider_meta: Mutex::new(None),
        };

        // Inject system prompt if configured.
        if let Some(ref prompt) = instance.config.system_prompt {
            let system_turn = ConversationTurn {
                role: "system".to_string(),
                content: prompt.clone(),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: chrono::Local::now().to_rfc3339(),
                reasoning_content: None,
                tool_name: None,
                tool_result_projection: None,
                image_refs: Vec::new(),
            };
            instance.history.lock().unwrap().push(system_turn);
        }

        instance
    }

    /// Returns the unique instance ID.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Returns a reference to the agent configuration.
    pub fn config(&self) -> &AgentConfig {
        &self.config
    }

    /// Returns the current agent state.
    pub fn state(&self) -> AgentState {
        *self.state.lock().unwrap()
    }

    /// Set the agent state.
    pub fn set_state(&self, new_state: AgentState) {
        *self.state.lock().unwrap() = new_state;
    }

    /// Transition to Thinking state. Returns false if the current state is not Idle.
    pub fn start_thinking(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if *state == AgentState::Idle {
            *state = AgentState::Thinking;
            true
        } else {
            false
        }
    }

    /// Transition to ExecutingTool state. Returns false if the current state is not Thinking.
    pub fn start_tool_execution(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if *state == AgentState::Thinking {
            *state = AgentState::ExecutingTool;
            true
        } else {
            false
        }
    }

    /// Transition to Responding state.
    pub fn start_responding(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if *state == AgentState::Thinking || *state == AgentState::ExecutingTool {
            *state = AgentState::Responding;
            true
        } else {
            false
        }
    }

    /// Transition back to Idle state.
    pub fn finish(&self) {
        *self.state.lock().unwrap() = AgentState::Idle;
    }

    /// Add a user message to the conversation history.
    pub fn add_user_message(&self, content: &str) {
        let turn = ConversationTurn {
            role: "user".to_string(),
            content: content.to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: chrono::Local::now().to_rfc3339(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
            image_refs: Vec::new(),
        };
        self.push_turn(turn);
    }

    /// T5（多模态）：带图片路径引用的 user 消息变体（`add_user_message`
    /// 签名不动；引用进 `ConversationTurn.image_refs`，build_messages 每轮
    /// 水合重读）。
    pub fn add_user_message_with_images(&self, content: &str, image_refs: &[String]) {
        let turn = ConversationTurn {
            role: "user".to_string(),
            content: content.to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: chrono::Local::now().to_rfc3339(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
            image_refs: image_refs.to_vec(),
        };
        self.push_turn(turn);
    }

    /// Add an assistant message (with optional tool calls) to the history.
    pub fn add_assistant_message(
        &self,
        content: &str,
        tool_calls: Vec<ToolCallInfo>,
        reasoning_content: Option<String>,
    ) {
        let turn = ConversationTurn {
            role: "assistant".to_string(),
            content: content.to_string(),
            tool_calls,
            tool_call_id: None,
            timestamp: chrono::Local::now().to_rfc3339(),
            reasoning_content,
            tool_name: None,
            tool_result_projection: None,
            image_refs: Vec::new(),
        };
        self.push_turn(turn);
    }

    /// Add a tool result message to the history.
    pub fn add_tool_result(&self, tool_call_id: &str, content: &str) {
        let turn = ConversationTurn {
            role: "tool".to_string(),
            content: content.to_string(),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.to_string()),
            timestamp: chrono::Local::now().to_rfc3339(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
            image_refs: Vec::new(),
        };
        self.push_turn(turn);
    }

    /// X1 (U3 projection prune): add a tool result keeping the ORIGINAL
    /// content in history, with the bounded model-facing projection carried
    /// alongside instead of replacing the content.
    ///
    /// `tool_name` feeds the deterministic prune-marker recompute in
    /// [`ConversationTurn::model_facing_content`] (the main agent loop knows
    /// the name at dispatch time). `projection` is the recorded override —
    /// set ONLY when the model-facing text cannot be recomputed from the
    /// original later: the spill tier (locator path embeds a wall-clock
    /// stamp) or any turn-guard nudge decoration (⑤/⑤′/⑥ — dynamic per-turn
    /// state). `None` ⇒ build-time projection recomputes the pure prune.
    pub fn add_tool_result_projected(
        &self,
        tool_call_id: &str,
        original: &str,
        tool_name: &str,
        projection: Option<String>,
    ) {
        let turn = ConversationTurn {
            role: "tool".to_string(),
            content: original.to_string(),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.to_string()),
            timestamp: chrono::Local::now().to_rfc3339(),
            reasoning_content: None,
            tool_name: Some(tool_name.to_string()),
            tool_result_projection: projection,
            image_refs: Vec::new(),
        };
        self.push_turn(turn);
    }

    /// Replace the content of an existing tool result with matching tool_call_id.
    ///
    /// Semantics:
    /// - If exactly one tool message with this ID exists, replace its content in place.
    /// - If multiple tool messages with this ID exist (abnormal — e.g., an async
    ///   placeholder left in the snapshot plus a freshly injected result), replace
    ///   the last one and drop earlier duplicates.
    /// - If no matching tool message exists, push a new one (fallback to add).
    ///
    /// This is the correct way to inject a cluster_rpc callback result on resume:
    /// the async placeholder saved in the snapshot must be overwritten, not appended
    /// (appending produces two tool messages with the same tool_call_id, which LLM
    /// APIs reject with HTTP 400 "Messages with role 'tool' must be a response to a
    /// preceding message with 'tool_calls'").
    pub fn replace_tool_result(&self, tool_call_id: &str, content: &str) {
        let mut history = self.history.lock().unwrap();

        let matching: Vec<usize> = history
            .iter()
            .enumerate()
            .filter(|(_, t)| t.role == "tool" && t.tool_call_id.as_deref() == Some(tool_call_id))
            .map(|(i, _)| i)
            .collect();

        if matching.is_empty() {
            let turn = ConversationTurn {
                role: "tool".to_string(),
                content: content.to_string(),
                tool_calls: Vec::new(),
                tool_call_id: Some(tool_call_id.to_string()),
                timestamp: chrono::Local::now().to_rfc3339(),
                reasoning_content: None,
                tool_name: None,
                tool_result_projection: None,
                image_refs: Vec::new(),
            };
            history.push(turn);
            return;
        }

        let last_idx = *matching.last().unwrap();
        history[last_idx].content = content.to_string();
        history[last_idx].timestamp = chrono::Local::now().to_rfc3339();

        for &idx in matching.iter().rev().skip(1) {
            history.remove(idx);
        }
    }

    /// Get a clone of the full conversation history.
    pub fn get_history(&self) -> Vec<ConversationTurn> {
        self.history.lock().unwrap().clone()
    }

    /// Clear all history except the system prompt.
    pub fn clear_history(&self) {
        let mut history = self.history.lock().unwrap();
        let system_prompt = history
            .iter()
            .position(|t| t.role == "system")
            .and_then(|idx| history.get(idx).cloned());
        history.clear();
        if let Some(sp) = system_prompt {
            history.push(sp);
        }
    }

    /// Compress history by keeping the system prompt and the last 50% of turns.
    ///
    /// Mirrors Go's `forceCompression()`:
    /// 1. Keeps the first message (system prompt)
    /// 2. Keeps the last 50% of conversation turns
    /// 3. Inserts a `[Session compressed at {timestamp}]` note at the compression point
    pub fn compress_history(&self) {
        let mut history = self.history.lock().unwrap();
        if history.len() <= 2 {
            // Not enough to compress
            return;
        }

        debug!(
            "[AgentInstance] Compressing history for instance id={}, {} turns",
            self.id,
            history.len()
        );

        // Find the system prompt (first message with role "system").
        let system_prompt = history.iter().find(|t| t.role == "system").cloned();

        // Collect non-system turns.
        let non_system: Vec<ConversationTurn> = history
            .iter()
            .filter(|t| t.role != "system")
            .cloned()
            .collect();

        if non_system.is_empty() {
            return;
        }

        // Keep the last 50% of non-system turns.
        let keep_count = (non_system.len() / 2).max(1);
        let start = non_system.len().saturating_sub(keep_count);

        // Build compressed history.
        *history = Vec::new();

        // 1. System prompt first.
        if let Some(sp) = system_prompt {
            history.push(sp);
        }

        // 2. Compression note.
        let timestamp = chrono::Local::now().to_rfc3339();
        let compression_note = ConversationTurn {
            role: "system".to_string(),
            content: format!("[Session compressed at {}]", timestamp),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: timestamp.clone(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
            image_refs: Vec::new(),
        };
        history.push(compression_note);

        // 3. Last 50% of turns.
        for turn in non_system.into_iter().skip(start) {
            history.push(turn);
        }
    }

    /// Replace the entire history with a new set of turns.
    ///
    /// Preserves the system prompt that was set during `AgentInstance::new()`.
    /// When session data is loaded from disk (which only contains user + assistant
    /// messages), the system prompt must not be lost.
    pub fn set_history(&self, new_history: Vec<ConversationTurn>) {
        let mut history = self.history.lock().unwrap();
        debug!(
            "[AgentInstance] Setting history for instance id={}, new_len={}",
            self.id,
            new_history.len()
        );
        let old_system_prompt = history.first().filter(|t| t.role == "system").cloned();
        *history = new_history;
        if let Some(sp) = old_system_prompt {
            // Only insert the old system prompt if new_history doesn't already have one.
            // This handles both cases: session restore from disk (no system prompt in
            // loaded data) and callers like force_compression that already include one.
            let has_system = history.first().is_some_and(|t| t.role == "system");
            if !has_system {
                history.insert(0, sp);
            }
        }
    }

    /// Truncate history to keep only the last N messages.
    pub fn truncate_to(&self, keep_last: usize) {
        let mut history = self.history.lock().unwrap();
        if history.len() > keep_last {
            let start = history.len() - keep_last;
            let kept: Vec<ConversationTurn> = history.drain(start..).collect();
            *history = kept;
        }
    }

    /// Get the cached summary covering a history prefix, if any.
    ///
    /// Returns a clone of the cached [`SummaryCache`] (text + `covers_up_to`),
    /// or `None` when no summary is cached.
    pub fn get_summary_cache(&self) -> Option<SummaryCache> {
        self.summary_cache.lock().unwrap().clone()
    }

    /// Set or clear the cached summary covering a history prefix.
    ///
    /// Pass `None` to clear. The caller is responsible for maintaining the
    /// invariant that `text` summarizes `history[..covers_up_to]` (see
    /// [`SummaryCache`]).
    pub fn set_summary_cache(&self, cache: Option<SummaryCache>) {
        *self.summary_cache.lock().unwrap() = cache;
    }

    /// Get the context window size.
    pub fn context_window(&self) -> usize {
        self.context_window
    }

    /// Set the context window size.
    pub fn set_context_window(&mut self, window: usize) {
        self.context_window = window;
    }

    /// Get the number of non-system messages in history.
    pub fn message_count(&self) -> usize {
        self.history
            .lock()
            .unwrap()
            .iter()
            .filter(|t| t.role != "system")
            .count()
    }

    /// Set arbitrary metadata JSON for this instance.
    pub fn set_metadata(&self, value: serde_json::Value) {
        *self.metadata.lock().unwrap() = value;
    }

    /// Get a clone of the current metadata.
    pub fn metadata(&self) -> serde_json::Value {
        self.metadata.lock().unwrap().clone()
    }

    // -----------------------------------------------------------------------
    // Workspace field (mirrors Go's AgentInstance.Workspace)
    // -----------------------------------------------------------------------

    /// Get a reference to the workspace path.
    pub fn workspace(&self) -> &PathBuf {
        &self.workspace
    }

    /// Set the workspace path.
    pub fn set_workspace(&mut self, path: PathBuf) {
        self.workspace = path;
    }

    // -----------------------------------------------------------------------
    // MaxIterations field (mirrors Go's AgentInstance.MaxIterations)
    // -----------------------------------------------------------------------

    /// Get the maximum tool-call iterations per request.
    pub fn max_iterations(&self) -> u32 {
        self.max_iterations
    }

    /// Set the maximum tool-call iterations per request.
    pub fn set_max_iterations(&mut self, max: u32) {
        self.max_iterations = max;
    }

    // -----------------------------------------------------------------------
    // Subagents field (mirrors Go's AgentInstance.Subagents)
    // -----------------------------------------------------------------------

    /// Get a clone of the sub-agent allow list.
    pub fn subagents(&self) -> Vec<String> {
        self.subagents.lock().unwrap().clone()
    }

    /// Set the sub-agent allow list.
    pub fn set_subagents(&self, agents: Vec<String>) {
        *self.subagents.lock().unwrap() = agents;
    }

    // -----------------------------------------------------------------------
    // SkillsFilter field (mirrors Go's AgentInstance.SkillsFilter)
    // -----------------------------------------------------------------------

    /// Get a clone of the skills filter.
    pub fn skills_filter(&self) -> Vec<String> {
        self.skills_filter.lock().unwrap().clone()
    }

    /// Set the skills filter.
    pub fn set_skills_filter(&self, filter: Vec<String>) {
        *self.skills_filter.lock().unwrap() = filter;
    }

    // -----------------------------------------------------------------------
    // FallbackCandidates field (mirrors Go's AgentInstance.Candidates)
    // -----------------------------------------------------------------------

    /// Get a clone of the fallback model candidates.
    pub fn fallback_candidates(&self) -> Vec<String> {
        self.fallback_candidates.lock().unwrap().clone()
    }

    /// Set the fallback model candidates.
    pub fn set_fallback_candidates(&self, candidates: Vec<String>) {
        *self.fallback_candidates.lock().unwrap() = candidates;
    }

    // -----------------------------------------------------------------------
    // ProviderMeta field (mirrors Go's AgentInstance.ProviderMeta)
    // -----------------------------------------------------------------------

    /// Get a clone of the provider metadata.
    pub fn provider_meta(&self) -> Option<serde_json::Value> {
        self.provider_meta.lock().unwrap().clone()
    }

    /// Set the provider metadata.
    pub fn set_provider_meta(&self, meta: serde_json::Value) {
        *self.provider_meta.lock().unwrap() = Some(meta);
    }

    /// Internal helper: push a turn. History is strictly append-only —
    /// bounding (with C-aware index adjustment) is the session store's job
    /// (`SessionStore::trim_to_limit`), never the instance's, so that the
    /// summary cache's `covers_up_to` index stays valid as history grows.
    fn push_turn(&self, turn: ConversationTurn) {
        let mut history = self.history.lock().unwrap();
        history.push(turn);
    }
}

#[cfg(test)]
mod tests;

// S9 (quality-hardening goal 冲刺 S9): 独立测试文件挂载（声明式，无内联测试）。
#[cfg(test)]
mod s9_tests;

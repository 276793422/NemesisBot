//! Agent instance: manages conversation history and state for a single session.
//!
//! Each `AgentInstance` tracks the full conversation history for one session,
//! enforces a maximum history length with truncation, and exposes state
//! transitions used by the agent loop.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crate::types::{AgentConfig, AgentState, ConversationTurn, ToolCallInfo};
use tracing::debug;

/// Default maximum number of conversation turns to keep (excluding system prompt).
const DEFAULT_MAX_HISTORY: usize = 100;

/// Monotonically increasing instance counter for unique IDs.
static INSTANCE_COUNTER: AtomicU64 = AtomicU64::new(1);

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
    /// Maximum number of turns to retain (excluding system).
    max_history: usize,
    /// Optional metadata attached to this instance.
    metadata: Mutex<serde_json::Value>,
    /// Summary of compressed older messages.
    summary: Mutex<String>,
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
            max_history: DEFAULT_MAX_HISTORY,
            metadata: Mutex::new(serde_json::Value::Null),
            summary: Mutex::new(String::new()),
            context_window: 32000,
            workspace: PathBuf::new(),
            max_iterations: 20,
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
                timestamp: chrono::Utc::now().to_rfc3339(),
                reasoning_content: None,
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
            timestamp: chrono::Utc::now().to_rfc3339(),
            reasoning_content: None,
        };
        self.push_turn(turn);
    }

    /// Add an assistant message (with optional tool calls) to the history.
    pub fn add_assistant_message(&self, content: &str, tool_calls: Vec<ToolCallInfo>, reasoning_content: Option<String>) {
        let turn = ConversationTurn {
            role: "assistant".to_string(),
            content: content.to_string(),
            tool_calls,
            tool_call_id: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
            reasoning_content,
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
            timestamp: chrono::Utc::now().to_rfc3339(),
            reasoning_content: None,
        };
        self.push_turn(turn);
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

        debug!("[AgentInstance] Compressing history for instance id={}, {} turns", self.id, history.len());

        // Find the system prompt (first message with role "system").
        let system_prompt = history
            .iter()
            .find(|t| t.role == "system")
            .cloned();

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
        let timestamp = chrono::Utc::now().to_rfc3339();
        let compression_note = ConversationTurn {
            role: "system".to_string(),
            content: format!("[Session compressed at {}]", timestamp),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: timestamp.clone(),
            reasoning_content: None,
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
        debug!("[AgentInstance] Setting history for instance id={}, new_len={}", self.id, new_history.len());
        let old_system_prompt = history.first().filter(|t| t.role == "system").cloned();
        *history = new_history;
        if let Some(sp) = old_system_prompt {
            // Only insert the old system prompt if new_history doesn't already have one.
            // This handles both cases: session restore from disk (no system prompt in
            // loaded data) and callers like force_compression that already include one.
            let has_system = history.first().map_or(false, |t| t.role == "system");
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

    /// Get the current summary of compressed older messages.
    pub fn get_summary(&self) -> String {
        self.summary.lock().unwrap().clone()
    }

    /// Set the summary of compressed older messages.
    pub fn set_summary(&self, summary: &str) {
        *self.summary.lock().unwrap() = summary.to_string();
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
        self.history.lock().unwrap()
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

    /// Internal helper: push a turn and apply truncation if needed.
    fn push_turn(&self, turn: ConversationTurn) {
        let mut history = self.history.lock().unwrap();

        // Count non-system turns.
        let non_system_count = history.iter().filter(|t| t.role != "system").count();

        // If we are at capacity and this is a non-system turn, truncate.
        if turn.role != "system" && non_system_count >= self.max_history {
            self.truncate_history(&mut history);
        }

        history.push(turn);
    }

    /// Truncate history: keep the system prompt (if any) and the most recent turns.
    fn truncate_history(&self, history: &mut std::sync::MutexGuard<'_, Vec<ConversationTurn>>) {
        // Find the system prompt.
        let system_prompt = history
            .iter()
            .find(|t| t.role == "system")
            .cloned();

        // Keep the last half of max_history non-system turns.
        let keep_count = self.max_history / 2;
        let non_system: Vec<ConversationTurn> = history
            .iter()
            .filter(|t| t.role != "system")
            .cloned()
            .collect();

        history.clear();
        if let Some(sp) = system_prompt {
            history.push(sp);
        }
        let start = non_system.len().saturating_sub(keep_count);
        history.extend(non_system.into_iter().skip(start));
    }
}

#[cfg(test)]
mod tests;

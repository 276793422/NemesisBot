//! Agent instance: manages conversation history and state for a single session.
//!
//! Each `AgentInstance` tracks the full conversation history for one session,
//! enforces a maximum history length with truncation, and exposes state
//! transitions used by the agent loop.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crate::types::{AgentConfig, AgentState, ConversationTurn, ToolCallInfo};

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
        };
        self.push_turn(turn);
    }

    /// Add an assistant message (with optional tool calls) to the history.
    pub fn add_assistant_message(&self, content: &str, tool_calls: Vec<ToolCallInfo>) {
        let turn = ConversationTurn {
            role: "assistant".to_string(),
            content: content.to_string(),
            tool_calls,
            tool_call_id: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
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
        };
        history.push(compression_note);

        // 3. Last 50% of turns.
        for turn in non_system.into_iter().skip(start) {
            history.push(turn);
        }
    }

    /// Replace the entire history with a new set of turns.
    pub fn set_history(&self, new_history: Vec<ConversationTurn>) {
        *self.history.lock().unwrap() = new_history;
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
mod tests {
    use super::*;

    fn test_config() -> AgentConfig {
        AgentConfig {
            model: "test-model".to_string(),
            system_prompt: Some("You are a test assistant.".to_string()),
            max_turns: 5,
            tools: vec!["search".to_string()],
        }
    }

    #[test]
    fn new_instance_has_system_prompt() {
        let instance = AgentInstance::new(test_config());
        let history = instance.get_history();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].role, "system");
        assert_eq!(history[0].content, "You are a test assistant.");
        assert_eq!(instance.state(), AgentState::Idle);
    }

    #[test]
    fn add_messages_and_get_history() {
        let instance = AgentInstance::new(test_config());
        instance.add_user_message("Hello");
        instance.add_assistant_message("Hi there!", Vec::new());

        let history = instance.get_history();
        // system + user + assistant = 3
        assert_eq!(history.len(), 3);
        assert_eq!(history[1].role, "user");
        assert_eq!(history[1].content, "Hello");
        assert_eq!(history[2].role, "assistant");
        assert_eq!(history[2].content, "Hi there!");
    }

    #[test]
    fn add_tool_result() {
        let instance = AgentInstance::new(test_config());
        let tool_calls = vec![ToolCallInfo {
            id: "tc_1".to_string(),
            name: "search".to_string(),
            arguments: r#"{"query":"rust"}"#.to_string(),
        }];
        instance.add_assistant_message("", tool_calls);
        instance.add_tool_result("tc_1", "Results for rust");

        let history = instance.get_history();
        assert_eq!(history.len(), 3); // system + assistant + tool
        let tool_turn = &history[2];
        assert_eq!(tool_turn.role, "tool");
        assert_eq!(tool_turn.tool_call_id.as_deref(), Some("tc_1"));
        assert_eq!(tool_turn.content, "Results for rust");
    }

    #[test]
    fn state_transitions() {
        let instance = AgentInstance::new(test_config());
        assert_eq!(instance.state(), AgentState::Idle);

        // Idle -> Thinking
        assert!(instance.start_thinking());
        assert_eq!(instance.state(), AgentState::Thinking);

        // Cannot transition from Thinking to Thinking again
        assert!(!instance.start_thinking());

        // Thinking -> ExecutingTool
        assert!(instance.start_tool_execution());
        assert_eq!(instance.state(), AgentState::ExecutingTool);

        // ExecutingTool -> Responding
        assert!(instance.start_responding());
        assert_eq!(instance.state(), AgentState::Responding);

        // Responding -> Idle
        instance.finish();
        assert_eq!(instance.state(), AgentState::Idle);
    }

    #[test]
    fn clear_history_preserves_system_prompt() {
        let instance = AgentInstance::new(test_config());
        instance.add_user_message("Hello");
        instance.add_assistant_message("Hi!", Vec::new());
        assert_eq!(instance.get_history().len(), 3);

        instance.clear_history();
        let history = instance.get_history();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].role, "system");
    }

    #[test]
    fn compress_history_keeps_system_and_half_turns() {
        let instance = AgentInstance::new(test_config());
        // Add 6 turns: u1, a1, u2, a2, u3, a3
        instance.add_user_message("u1");
        instance.add_assistant_message("a1", Vec::new());
        instance.add_user_message("u2");
        instance.add_assistant_message("a2", Vec::new());
        instance.add_user_message("u3");
        instance.add_assistant_message("a3", Vec::new());
        // system + 6 turns = 7
        assert_eq!(instance.get_history().len(), 7);

        instance.compress_history();
        let history = instance.get_history();

        // system prompt + compression note + last 3 of 6 turns = 5
        // keep_count = 6/2 = 3, start = 6-3 = 3, skip(3) yields 3 turns: a2, u3, a3
        assert_eq!(history.len(), 5);
        // First message is system prompt
        assert_eq!(history[0].role, "system");
        assert!(history[0].content.contains("test assistant"));
        // Second message is compression note
        assert_eq!(history[1].role, "system");
        assert!(history[1].content.contains("[Session compressed at"));
        // skip(3) removes u1, a1, u2, keeps a2, u3, a3
        assert!(history[2].content.contains("a2"));
    }

    #[test]
    fn compress_history_noop_on_short_history() {
        let instance = AgentInstance::new(test_config());
        instance.add_user_message("Hello");
        assert_eq!(instance.get_history().len(), 2);

        instance.compress_history();
        let history = instance.get_history();
        // Should remain unchanged (too short to compress)
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].role, "system");
        assert_eq!(history[1].content, "Hello");
    }

    // --- Additional instance tests ---

    #[test]
    fn new_instance_without_system_prompt() {
        let config = AgentConfig {
            model: "test".to_string(),
            system_prompt: None,
            max_turns: 5,
            tools: vec![],
        };
        let instance = AgentInstance::new(config);
        assert!(instance.get_history().is_empty());
        assert_eq!(instance.message_count(), 0);
    }

    #[test]
    fn instance_unique_ids() {
        let a = AgentInstance::new(test_config());
        let b = AgentInstance::new(test_config());
        assert_ne!(a.id(), b.id());
        assert!(a.id() > 0);
        assert!(b.id() > 0);
    }

    #[test]
    fn instance_config_access() {
        let instance = AgentInstance::new(test_config());
        assert_eq!(instance.config().model, "test-model");
        assert_eq!(instance.config().max_turns, 5);
    }

    #[test]
    fn instance_state_transitions_invalid() {
        let instance = AgentInstance::new(test_config());

        // Cannot go to ExecutingTool from Idle
        assert!(!instance.start_tool_execution());

        // Cannot go to Responding from Idle
        assert!(!instance.start_responding());

        // Can go to Idle from any state via finish
        instance.finish();
        assert_eq!(instance.state(), AgentState::Idle);
    }

    #[test]
    fn instance_state_thinking_to_responding() {
        let instance = AgentInstance::new(test_config());
        instance.start_thinking();
        // Can go directly from Thinking to Responding
        assert!(instance.start_responding());
        assert_eq!(instance.state(), AgentState::Responding);
    }

    #[test]
    fn instance_add_messages_increments_count() {
        let instance = AgentInstance::new(test_config());

        assert_eq!(instance.message_count(), 0);
        instance.add_user_message("Hello");
        assert_eq!(instance.message_count(), 1);
        instance.add_assistant_message("Hi", Vec::new());
        assert_eq!(instance.message_count(), 2);
        instance.add_tool_result("tc_1", "Result");
        assert_eq!(instance.message_count(), 3);
    }

    #[test]
    fn instance_message_count_excludes_system() {
        let instance = AgentInstance::new(test_config());
        // System prompt is added automatically
        assert_eq!(instance.get_history().len(), 1);
        assert_eq!(instance.message_count(), 0); // system excluded
    }

    #[test]
    fn instance_set_and_get_summary() {
        let instance = AgentInstance::new(test_config());
        assert!(instance.get_summary().is_empty());

        instance.set_summary("Previous conversation summary");
        assert_eq!(instance.get_summary(), "Previous conversation summary");
    }

    #[test]
    fn instance_context_window() {
        let mut instance = AgentInstance::new(test_config());
        assert_eq!(instance.context_window(), 32000);

        instance.set_context_window(64000);
        assert_eq!(instance.context_window(), 64000);
    }

    #[test]
    fn instance_metadata() {
        let instance = AgentInstance::new(test_config());

        // Default metadata is Null
        assert!(instance.metadata().is_null());

        instance.set_metadata(serde_json::json!({"key": "value"}));
        let meta = instance.metadata();
        assert_eq!(meta["key"], "value");
    }

    #[test]
    fn instance_workspace() {
        let mut instance = AgentInstance::new(test_config());
        assert!(instance.workspace().as_os_str().is_empty());

        instance.set_workspace(PathBuf::from("/tmp/workspace"));
        assert_eq!(instance.workspace(), &PathBuf::from("/tmp/workspace"));
    }

    #[test]
    fn instance_max_iterations() {
        let mut instance = AgentInstance::new(test_config());
        assert_eq!(instance.max_iterations(), 20);

        instance.set_max_iterations(50);
        assert_eq!(instance.max_iterations(), 50);
    }

    #[test]
    fn instance_subagents() {
        let instance = AgentInstance::new(test_config());
        assert!(instance.subagents().is_empty());

        instance.set_subagents(vec!["agent_a".to_string(), "agent_b".to_string()]);
        let agents = instance.subagents();
        assert_eq!(agents.len(), 2);
        assert!(agents.contains(&"agent_a".to_string()));
        assert!(agents.contains(&"agent_b".to_string()));
    }

    #[test]
    fn instance_skills_filter() {
        let instance = AgentInstance::new(test_config());
        assert!(instance.skills_filter().is_empty());

        instance.set_skills_filter(vec!["skill1".to_string()]);
        let filter = instance.skills_filter();
        assert_eq!(filter.len(), 1);
        assert!(filter.contains(&"skill1".to_string()));
    }

    #[test]
    fn instance_fallback_candidates() {
        let instance = AgentInstance::new(test_config());
        assert!(instance.fallback_candidates().is_empty());

        instance.set_fallback_candidates(vec!["model_a".to_string(), "model_b".to_string()]);
        let candidates = instance.fallback_candidates();
        assert_eq!(candidates.len(), 2);
    }

    #[test]
    fn instance_provider_meta() {
        let instance = AgentInstance::new(test_config());
        assert!(instance.provider_meta().is_none());

        instance.set_provider_meta(serde_json::json!({"name": "openai"}));
        let meta = instance.provider_meta();
        assert!(meta.is_some());
        assert_eq!(meta.unwrap()["name"], "openai");
    }

    #[test]
    fn instance_truncate_to() {
        let instance = AgentInstance::new(test_config());
        for i in 0..10 {
            instance.add_user_message(&format!("msg_{}", i));
        }
        // system + 10 user messages = 11
        assert_eq!(instance.get_history().len(), 11);

        instance.truncate_to(5);
        let history = instance.get_history();
        assert_eq!(history.len(), 5);
    }

    #[test]
    fn instance_truncate_to_more_than_history() {
        let instance = AgentInstance::new(test_config());
        instance.add_user_message("msg1");
        instance.add_user_message("msg2");
        // system + 2 = 3

        instance.truncate_to(100);
        assert_eq!(instance.get_history().len(), 3); // No change
    }

    #[test]
    fn instance_set_history() {
        let instance = AgentInstance::new(test_config());

        let new_history = vec![
            ConversationTurn {
                role: "system".to_string(),
                content: "Custom system".to_string(),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: String::new(),
            },
            ConversationTurn {
                role: "user".to_string(),
                content: "Custom user".to_string(),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: String::new(),
            },
        ];

        instance.set_history(new_history);
        let history = instance.get_history();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].content, "Custom system");
        assert_eq!(history[1].content, "Custom user");
    }

    #[test]
    fn instance_clear_history_no_system_prompt() {
        let config = AgentConfig {
            model: "test".to_string(),
            system_prompt: None,
            max_turns: 5,
            tools: vec![],
        };
        let instance = AgentInstance::new(config);
        instance.add_user_message("Hello");
        instance.add_assistant_message("Hi", Vec::new());

        instance.clear_history();
        assert!(instance.get_history().is_empty());
    }

    #[test]
    fn instance_history_truncation_at_max_capacity() {
        let instance = AgentInstance::new(test_config());

        // DEFAULT_MAX_HISTORY is 100. Push 101 non-system turns.
        for i in 0..101 {
            instance.add_user_message(&format!("msg_{}", i));
        }

        let history = instance.get_history();
        // Should have been truncated (system + kept turns < 102)
        assert!(history.len() < 102);
        // System prompt should still be first
        assert_eq!(history[0].role, "system");
        // Most recent messages should be preserved
        let last_content = history.last().unwrap().content.clone();
        assert!(last_content.starts_with("msg_"));
    }

    #[test]
    fn instance_compress_with_many_turns() {
        let instance = AgentInstance::new(test_config());
        // Add 20 turns
        for i in 0..20 {
            instance.add_user_message(&format!("u{}", i));
            instance.add_assistant_message(&format!("a{}", i), Vec::new());
        }
        // system + 40 turns = 41
        assert_eq!(instance.get_history().len(), 41);

        instance.compress_history();
        let history = instance.get_history();
        // Should be significantly smaller
        assert!(history.len() < 41);
        // System prompt preserved
        assert_eq!(history[0].role, "system");
        // Compression note present
        assert!(history[1].content.contains("[Session compressed at"));
    }

    #[test]
    fn instance_tool_result_message() {
        let instance = AgentInstance::new(test_config());
        let tool_calls = vec![ToolCallInfo {
            id: "tc_abc".to_string(),
            name: "calculator".to_string(),
            arguments: "{}".to_string(),
        }];
        instance.add_assistant_message("", tool_calls);
        instance.add_tool_result("tc_abc", "42");

        let history = instance.get_history();
        let tool_msg = history.iter().find(|t| t.role == "tool").unwrap();
        assert_eq!(tool_msg.tool_call_id.as_deref(), Some("tc_abc"));
        assert_eq!(tool_msg.content, "42");
    }

    #[test]
    fn instance_assistant_with_multiple_tool_calls() {
        let instance = AgentInstance::new(test_config());
        let tool_calls = vec![
            ToolCallInfo {
                id: "tc_1".to_string(),
                name: "search".to_string(),
                arguments: r#"{"q":"rust"}"#.to_string(),
            },
            ToolCallInfo {
                id: "tc_2".to_string(),
                name: "calculator".to_string(),
                arguments: r#"{"expr":"2+2"}"#.to_string(),
            },
        ];
        instance.add_assistant_message("Let me help", tool_calls);

        let history = instance.get_history();
        let assistant_msg = history.iter().find(|t| t.role == "assistant").unwrap();
        assert_eq!(assistant_msg.tool_calls.len(), 2);
        assert_eq!(assistant_msg.tool_calls[0].name, "search");
        assert_eq!(assistant_msg.tool_calls[1].name, "calculator");
    }
}

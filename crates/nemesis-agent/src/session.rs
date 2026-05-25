//! Session management and LLM-driven conversation summarization.
//!
//! Provides:
//! - `Session` / `SessionManager` for tracking active sessions
//! - `SessionStore` for persistent conversation history with disk storage
//! - `Summarizer` for LLM-driven multi-part session summarization
//! - Token estimation and force compression utilities

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::r#loop::{LlmMessage, LlmProvider};
use crate::loop_executor::ObserverEvent;
use crate::types::{ChatOptions, ConversationTurn};

// ---------------------------------------------------------------------------
// Session (active session tracking)
// ---------------------------------------------------------------------------

/// A single active session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// Unique session key (e.g. "channel:chat_id").
    pub session_key: String,
    /// Channel this session belongs to (e.g. "web", "rpc", "discord").
    pub channel: String,
    /// Chat or conversation identifier.
    pub chat_id: String,
    /// Whether the session is currently processing a request.
    pub busy: bool,
    /// Timestamp of the last activity on this session.
    pub last_active: DateTime<Utc>,
    /// Last channel used (for crash recovery).
    pub last_channel: Option<String>,
    /// Last chat ID used (for crash recovery).
    pub last_chat_id: Option<String>,
}

impl Session {
    /// Create a new session with the given key, channel, and chat ID.
    pub fn new(session_key: &str, channel: &str, chat_id: &str) -> Self {
        Self {
            session_key: session_key.to_string(),
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
            busy: false,
            last_active: Utc::now(),
            last_channel: None,
            last_chat_id: None,
        }
    }

    /// Touch the session, updating last_active to now.
    pub fn touch(&mut self) {
        self.last_active = Utc::now();
    }
}

// ---------------------------------------------------------------------------
// SessionManager (active session tracking)
// ---------------------------------------------------------------------------

/// Manages active sessions with concurrent access.
pub struct SessionManager {
    /// Internal DashMap store.
    sessions: DashMap<String, Session>,
    /// Default expiration timeout for cleanup.
    default_timeout: Duration,
}

impl SessionManager {
    /// Create a new session manager with a default expiration timeout.
    pub fn new(default_timeout: Duration) -> Self {
        Self {
            sessions: DashMap::new(),
            default_timeout,
        }
    }

    /// Create a session manager with a 30-minute default timeout.
    pub fn with_default_timeout() -> Self {
        Self::new(Duration::from_secs(30 * 60))
    }

    /// Get an existing session or create a new one for the given key.
    pub fn get_or_create(&self, session_key: &str, channel: &str, chat_id: &str) -> Session {
        if let Some(mut session) = self.sessions.get_mut(session_key) {
            session.touch();
            return session.clone();
        }
        let session = Session::new(session_key, channel, chat_id);
        self.sessions.insert(session_key.to_string(), session.clone());
        session
    }

    /// Mark a session as busy.
    pub fn set_busy(&self, session_key: &str, busy: bool) -> bool {
        if let Some(mut session) = self.sessions.get_mut(session_key) {
            session.busy = busy;
            session.touch();
            true
        } else {
            false
        }
    }

    /// Check whether a session is currently busy.
    pub fn is_busy(&self, session_key: &str) -> Option<bool> {
        self.sessions.get(session_key).map(|s| s.busy)
    }

    /// Record the last active channel for crash recovery.
    pub fn set_last_channel(&self, session_key: &str, channel: &str) {
        if let Some(mut session) = self.sessions.get_mut(session_key) {
            session.last_channel = Some(channel.to_string());
            session.touch();
        }
    }

    /// Record the last active chat ID for crash recovery.
    pub fn set_last_chat_id(&self, session_key: &str, chat_id: &str) {
        if let Some(mut session) = self.sessions.get_mut(session_key) {
            session.last_chat_id = Some(chat_id.to_string());
            session.touch();
        }
    }

    /// Remove and return expired sessions.
    pub fn cleanup_expired(&self) -> Vec<Session> {
        self.cleanup_expired_with_timeout(self.default_timeout)
    }

    /// Remove and return expired sessions based on a custom timeout.
    pub fn cleanup_expired_with_timeout(&self, timeout: Duration) -> Vec<Session> {
        let now = Utc::now();
        let keys_to_remove: Vec<String> = self
            .sessions
            .iter()
            .filter(|entry| {
                let elapsed = now - entry.value().last_active;
                elapsed.num_seconds() as u64 > timeout.as_secs()
            })
            .map(|entry| entry.key().clone())
            .collect();

        let mut removed = Vec::new();
        for key in keys_to_remove {
            if let Some((_, session)) = self.sessions.remove(&key) {
                removed.push(session);
            }
        }
        removed
    }

    /// Get the number of active sessions.
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// Check if there are no active sessions.
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    /// Check whether a session exists.
    pub fn contains(&self, session_key: &str) -> bool {
        self.sessions.contains_key(session_key)
    }

    /// Remove a specific session.
    pub fn remove(&self, session_key: &str) -> Option<Session> {
        self.sessions.remove(session_key).map(|(_, v)| v)
    }
}

// ---------------------------------------------------------------------------
// StoredSession (persistent conversation data)
// ---------------------------------------------------------------------------

/// Persistent session data stored on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredSession {
    /// Session key.
    pub key: String,
    /// Conversation messages.
    pub messages: Vec<StoredMessage>,
    /// Current summary of older messages.
    #[serde(default)]
    pub summary: String,
    /// When this session was created.
    pub created: DateTime<Utc>,
    /// When this session was last updated.
    pub updated: DateTime<Utc>,
}

/// A single message in stored session history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub tool_calls: Vec<StoredToolCall>,
    pub tool_call_id: Option<String>,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reasoning_content: Option<String>,
}

/// Stored tool call info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

impl From<&ConversationTurn> for StoredMessage {
    fn from(turn: &ConversationTurn) -> Self {
        Self {
            role: turn.role.clone(),
            content: turn.content.clone(),
            tool_calls: turn.tool_calls.iter().map(|tc| StoredToolCall {
                id: tc.id.clone(),
                name: tc.name.clone(),
                arguments: tc.arguments.clone(),
            }).collect(),
            tool_call_id: turn.tool_call_id.clone(),
            timestamp: turn.timestamp.clone(),
            // Do NOT persist reasoning_content — Go's session does not store it,
            // and including it bloats session files with internal model thinking.
            reasoning_content: None,
        }
    }
}

impl From<StoredMessage> for ConversationTurn {
    fn from(msg: StoredMessage) -> Self {
        Self {
            role: msg.role,
            content: msg.content,
            tool_calls: msg.tool_calls.into_iter().map(|tc| crate::types::ToolCallInfo {
                id: tc.id,
                name: tc.name,
                arguments: tc.arguments,
            }).collect(),
            tool_call_id: msg.tool_call_id,
            timestamp: msg.timestamp,
            reasoning_content: msg.reasoning_content,
        }
    }
}

// ---------------------------------------------------------------------------
// SessionStore (persistent conversation history with disk storage)
// ---------------------------------------------------------------------------

/// Manages persistent session data with optional disk storage.
///
/// Mirrors Go's `session.SessionManager` with:
/// - History get/set/truncate
/// - Summary get/set
/// - Disk persistence (JSON files)
/// - Atomic file writes
pub struct SessionStore {
    sessions: std::sync::RwLock<HashMap<String, StoredSession>>,
    storage_dir: Option<PathBuf>,
}

impl SessionStore {
    /// Create a new session store without disk persistence.
    pub fn new_in_memory() -> Self {
        Self {
            sessions: std::sync::RwLock::new(HashMap::new()),
            storage_dir: None,
        }
    }

    /// Create a new session store with disk persistence.
    pub fn new_with_storage(storage_dir: impl AsRef<Path>) -> Self {
        let dir = storage_dir.as_ref().to_path_buf();
        let _ = std::fs::create_dir_all(&dir);
        let store = Self {
            sessions: std::sync::RwLock::new(HashMap::new()),
            storage_dir: Some(dir),
        };
        store.load_from_disk();
        store
    }

    /// Get or create a stored session.
    pub fn get_or_create(&self, key: &str) -> StoredSession {
        let sessions = self.sessions.read().unwrap();
        if let Some(session) = sessions.get(key) {
            return session.clone();
        }
        drop(sessions);

        let session = StoredSession {
            key: key.to_string(),
            messages: Vec::new(),
            summary: String::new(),
            created: Utc::now(),
            updated: Utc::now(),
        };
        self.sessions.write().unwrap().insert(key.to_string(), session.clone());
        session
    }

    /// Get the conversation history for a session.
    pub fn get_history(&self, key: &str) -> Vec<StoredMessage> {
        self.sessions.read().unwrap()
            .get(key)
            .map(|s| s.messages.clone())
            .unwrap_or_default()
    }

    /// Set the conversation history for a session.
    pub fn set_history(&self, key: &str, messages: Vec<StoredMessage>) {
        if let Some(session) = self.sessions.write().unwrap().get_mut(key) {
            session.messages = messages;
            session.updated = Utc::now();
        }
    }

    /// Append a single message to a session's history.
    /// Mirrors Go's `agent.Sessions.AddMessage(sessionKey, role, content)`.
    pub fn add_message(&self, key: &str, role: &str, content: &str) {
        if let Some(session) = self.sessions.write().unwrap().get_mut(key) {
            session.messages.push(StoredMessage {
                role: role.to_string(),
                content: content.to_string(),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: chrono::Utc::now().to_rfc3339(),
                reasoning_content: None,
            });
            session.updated = Utc::now();
        }
    }

    /// Get the summary for a session.
    pub fn get_summary(&self, key: &str) -> String {
        self.sessions.read().unwrap()
            .get(key)
            .map(|s| s.summary.clone())
            .unwrap_or_default()
    }

    /// Set the summary for a session.
    pub fn set_summary(&self, key: &str, summary: &str) {
        if let Some(session) = self.sessions.write().unwrap().get_mut(key) {
            session.summary = summary.to_string();
            session.updated = Utc::now();
        }
    }

    /// Truncate the history, keeping only the last N messages.
    pub fn truncate_history(&self, key: &str, keep_last: usize) {
        if let Some(session) = self.sessions.write().unwrap().get_mut(key) {
            if session.messages.len() > keep_last {
                let start = session.messages.len() - keep_last;
                session.messages = session.messages.split_off(start);
                session.updated = Utc::now();
            }
        }
    }

    /// Save a session to disk.
    pub fn save(&self, key: &str) -> Result<(), String> {
        let storage_dir = match &self.storage_dir {
            Some(d) => d.clone(),
            None => return Ok(()),
        };

        let snapshot = {
            let sessions = self.sessions.read().unwrap();
            match sessions.get(key) {
                Some(s) => s.clone(),
                None => return Ok(()),
            }
        };

        let data = serde_json::to_string_pretty(&snapshot)
            .map_err(|e| format!("serialize error: {}", e))?;

        let filename = sanitize_filename(key);
        if filename == "." || filename == ".." || filename.contains('/') || filename.contains('\\') {
            return Err("invalid session key for filename".into());
        }

        let session_path = storage_dir.join(format!("{}.json", filename));

        // Atomic write: write to temp file, then rename.
        let tmp_name = format!("session-{}-{}.tmp", filename, std::process::id());
        let tmp_path = storage_dir.join(&tmp_name);

        std::fs::write(&tmp_path, &data)
            .map_err(|e| format!("write temp error: {}", e))?;

        std::fs::rename(&tmp_path, &session_path)
            .map_err(|e| {
                let _ = std::fs::remove_file(&tmp_path);
                format!("rename error: {}", e)
            })?;

        Ok(())
    }

    /// Load all sessions from disk.
    fn load_from_disk(&self) {
        let storage_dir = match &self.storage_dir {
            Some(d) => d,
            None => return,
        };

        let entries = match std::fs::read_dir(storage_dir) {
            Ok(e) => e,
            Err(_) => return,
        };

        let mut loaded = 0u32;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }

            let data = match std::fs::read_to_string(&path) {
                Ok(d) => d,
                Err(_) => continue,
            };

            match serde_json::from_str::<StoredSession>(&data) {
                Ok(session) => {
                    self.sessions.write().unwrap().insert(session.key.clone(), session);
                    loaded += 1;
                }
                Err(_) => continue,
            }
        }

        if loaded > 0 {
            info!("[SessionStore] Loaded {} sessions from disk", loaded);
        }
    }

    /// Get the number of stored sessions.
    pub fn len(&self) -> usize {
        self.sessions.read().unwrap().len()
    }

    /// Check if the store is empty.
    pub fn is_empty(&self) -> bool {
        self.sessions.read().unwrap().is_empty()
    }

    /// Check if a session exists.
    pub fn contains(&self, key: &str) -> bool {
        self.sessions.read().unwrap().contains_key(key)
    }

    /// Remove a session from memory (does not delete from disk).
    pub fn remove(&self, key: &str) -> Option<StoredSession> {
        self.sessions.write().unwrap().remove(key)
    }
}

/// Sanitize a session key for use as a filename.
/// Replaces ':' (volume separator on Windows) with '_'.
fn sanitize_filename(key: &str) -> String {
    key.replace(':', "_")
        .replace('\\', "_")
        .replace('/', "_")
}

// ---------------------------------------------------------------------------
// Token estimation
// ---------------------------------------------------------------------------

/// Estimate the token count for a string.
///
/// Uses a heuristic of approximately 2.5 characters per token,
/// which accounts for CJK characters and other overheads.
pub fn estimate_tokens(text: &str) -> usize {
    let char_count = text.chars().count();
    char_count * 2 / 5
}

/// Estimate the total token count for a list of conversation turns.
pub fn estimate_tokens_for_turns(turns: &[ConversationTurn]) -> usize {
    turns.iter().map(|t| estimate_tokens(&t.content)).sum()
}

/// Estimate tokens for stored messages.
pub fn estimate_tokens_for_messages(messages: &[StoredMessage]) -> usize {
    messages.iter().map(|m| estimate_tokens(&m.content)).sum()
}

// ---------------------------------------------------------------------------
// Summarizer (LLM-driven conversation summarization)
// ---------------------------------------------------------------------------

/// Callback trait for outbound notifications during summarization.
pub trait SummarizationNotifier: Send + Sync {
    /// Send a notification message.
    fn notify(&self, channel: &str, chat_id: &str, content: &str);
}

/// A no-op notifier that does nothing.
pub struct NullNotifier;

impl SummarizationNotifier for NullNotifier {
    fn notify(&self, _channel: &str, _chat_id: &str, _content: &str) {}
}

/// LLM-driven conversation summarizer.
///
/// Mirrors Go's `summarizeSession`, `summarizeBatch`, `maybeSummarize`,
/// and `forceCompression` functions.
pub struct Summarizer {
    provider: Arc<dyn LlmProvider>,
    model: String,
    context_window: usize,
    session_store: Arc<SessionStore>,
    notifier: Box<dyn SummarizationNotifier>,
    /// Tracks which sessions are currently being summarized to prevent concurrent summarization.
    summarizing: Arc<DashMap<String, bool>>,
    /// Observer manager for emitting conversation events during summarization LLM calls.
    observer_manager: Option<Arc<nemesis_observer::Manager>>,
}

impl Summarizer {
    /// Create a new summarizer.
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        model: String,
        context_window: usize,
        session_store: Arc<SessionStore>,
        notifier: Box<dyn SummarizationNotifier>,
        observer_manager: Option<Arc<nemesis_observer::Manager>>,
    ) -> Self {
        Self {
            provider,
            model,
            context_window,
            session_store,
            notifier,
            summarizing: Arc::new(DashMap::new()),
            observer_manager,
        }
    }

    /// Create a summarizer with a null notifier (for testing).
    pub fn new_silent(
        provider: Arc<dyn LlmProvider>,
        model: String,
        context_window: usize,
        session_store: Arc<SessionStore>,
    ) -> Self {
        Self::new(
            provider,
            model,
            context_window,
            session_store,
            Box::new(NullNotifier),
            None,
        )
    }

    /// Check if summarization should be triggered based on message count and token estimate.
    ///
    /// Mirrors Go's `maybeSummarize` threshold check.
    pub fn should_summarize(
        &self,
        history: &[ConversationTurn],
        context_window: usize,
    ) -> bool {
        let token_estimate = estimate_tokens_for_turns(history);
        let threshold = context_window * 75 / 100;
        history.len() > 20 || token_estimate > threshold
    }

    /// Trigger summarization if thresholds are met.
    ///
    /// Mirrors Go's `maybeSummarize`. Returns true if summarization was triggered.
    pub fn maybe_summarize(
        &self,
        session_key: &str,
        channel: &str,
        chat_id: &str,
        history: &[ConversationTurn],
        context_window: usize,
    ) -> bool {
        if !self.should_summarize(history, context_window) {
            return false;
        }

        // Prevent concurrent summarization of the same session.
        let summarize_key = format!("{}:{}", self.model, session_key);
        if self.summarizing.contains_key(&summarize_key) {
            return false;
        }
        self.summarizing.insert(summarize_key.clone(), true);

        // Notify user about summarization (only for non-internal channels).
        if !is_internal_channel(channel) {
            self.notifier.notify(
                channel,
                chat_id,
                "Memory threshold reached. Optimizing conversation history...",
            );
        }

        // Perform summarization synchronously (in the Go code this runs in a goroutine).
        self.summarize_session(session_key, history);

        self.summarizing.remove(&summarize_key);
        true
    }

    /// Summarize the conversation history for a session.
    ///
    /// Mirrors Go's `summarizeSession`. This is the main summarization logic:
    /// 1. Keep the last 4 messages for continuity
    /// 2. Filter to user/assistant messages only
    /// 3. Guard against oversized messages
    /// 4. For >10 messages, use multi-part summarization (split, summarize, merge)
    /// 5. For <=10 messages, summarize in one batch
    ///
    /// Returns the generated summary, or empty string if summarization was skipped.
    pub fn summarize_session(
        &self,
        session_key: &str,
        history: &[ConversationTurn],
    ) -> String {
        // Need at least 5 messages to summarize (keep last 4).
        if history.len() <= 4 {
            return String::new();
        }

        let to_summarize = &history[..history.len() - 4];
        let existing_summary = self.session_store.get_summary(session_key);

        // Filter to user/assistant only, guard against oversized messages.
        let max_msg_tokens = self.context_window / 2;
        let mut valid_messages: Vec<&ConversationTurn> = Vec::new();
        let mut omitted = false;

        for m in to_summarize {
            if m.role != "user" && m.role != "assistant" {
                continue;
            }
            let msg_tokens = estimate_tokens(&m.content);
            if msg_tokens > max_msg_tokens {
                omitted = true;
                continue;
            }
            valid_messages.push(m);
        }

        if valid_messages.is_empty() {
            return String::new();
        }

        // Multi-part summarization for large conversations.
        let final_summary = if valid_messages.len() > 10 {
            self.summarize_multipart(&valid_messages)
        } else {
            self.summarize_batch(&valid_messages, &existing_summary)
        };

        // Add omission note if needed.
        let final_summary = if omitted && !final_summary.is_empty() {
            format!(
                "{}\n[Note: Some oversized messages were omitted from this summary for efficiency.]",
                final_summary
            )
        } else {
            final_summary
        };

        // Update session store.
        if !final_summary.is_empty() {
            // Convert history to stored messages and truncate.
            let stored: Vec<StoredMessage> = history.iter().map(|t| t.into()).collect();
            self.session_store.set_history(session_key, stored);

            // Keep only last 4 messages.
            // NOTE: 此处有与 loop.rs::maybe_summarize 相同的 tool 对完整性问题。
            // 当前为死代码（仅测试使用）。若未来启用，需同步修复。
            let truncated: Vec<StoredMessage> = history[history.len().saturating_sub(4)..]
                .iter()
                .map(|t| t.into())
                .collect();
            self.session_store.set_history(session_key, truncated);
            self.session_store.set_summary(session_key, &final_summary);

            if let Err(e) = self.session_store.save(session_key) {
                warn!("[SessionStore] Failed to save session after summarization: {}", e);
            }
        }

        final_summary
    }

    /// Multi-part summarization: split into two halves, summarize each, then merge.
    fn summarize_multipart(&self, messages: &[&ConversationTurn]) -> String {
        let mid = messages.len() / 2;
        let part1 = &messages[..mid];
        let part2 = &messages[mid..];

        let s1 = self.summarize_batch(part1, "");
        let s2 = self.summarize_batch(part2, "");

        // Merge the two summaries via LLM.
        let merge_prompt = format!(
            "Merge these two conversation summaries into one cohesive summary:\n\n1: {}\n\n2: {}",
            s1, s2
        );

        let messages = vec![LlmMessage {
            role: "user".to_string(),
            content: merge_prompt,
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        }];

        // Use tokio runtime for the async LLM call.
        // Summarization uses conservative parameters matching Go:
        // max_tokens=1024, temperature=0.3 (deterministic, concise output).
        let summarize_opts = Some(ChatOptions {
            max_tokens: Some(1024),
            temperature: Some(0.3),
            ..Default::default()
        });

        // Generate trace_id and emit observer events around the LLM call.
        let trace_id = Self::generate_trace_id("summarize-multipart");
        self.emit_observer_sync_event(ObserverEvent::ConversationStart {
            trace_id: trace_id.clone(),
            session_key: "summarize-multipart".to_string(),
            channel: String::new(),
            chat_id: String::new(),
            sender_id: "summarizer".to_string(),
            content: String::new(),
        });
        self.emit_observer_async_event(ObserverEvent::LlmRequest {
            trace_id: trace_id.clone(),
            round: 0,
            model: self.model.clone(),
            messages: vec![],
            tools: vec![],
            messages_count: 0,
            tools_count: 0,
            provider_name: String::new(),
            api_key: String::new(),
            api_base: String::new(),
        });
        let start = std::time::Instant::now();
        let response = tokio_block_on(async {
            self.provider.chat(&self.model, messages, summarize_opts, vec![]).await
        });
        let duration_ms = start.elapsed().as_millis() as u64;
        let response_content = response.as_ref().ok().map(|r| r.content.clone()).unwrap_or_default();
        self.emit_observer_async_event(ObserverEvent::LlmResponse {
            trace_id: trace_id.clone(),
            round: 0,
            duration_ms,
            has_tool_calls: false,
            content: response_content.clone(),
            tool_calls: vec![],
            tool_calls_count: 0,
            finish_reason: Some("stop".to_string()),
            usage: None,
        });
        self.emit_observer_sync_event(ObserverEvent::ConversationEnd {
            trace_id,
            session_key: "summarize-multipart".to_string(),
            total_rounds: 1,
            duration_ms,
            content: response_content,
            channel: String::new(),
            chat_id: String::new(),
        });

        match response {
            Ok(resp) if !resp.content.is_empty() => resp.content,
            Ok(_) => format!("{} {}", s1, s2),
            Err(_) => format!("{} {}", s1, s2),
        }
    }

    /// Summarize a batch of messages using the LLM.
    ///
    /// Mirrors Go's `summarizeBatch`.
    fn summarize_batch(&self, batch: &[&ConversationTurn], existing_summary: &str) -> String {
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

        // Summarization uses conservative parameters matching Go:
        // max_tokens=1024, temperature=0.3 (deterministic, concise output).
        let summarize_opts = Some(ChatOptions {
            max_tokens: Some(1024),
            temperature: Some(0.3),
            ..Default::default()
        });

        // Generate trace_id and emit observer events around the LLM call.
        let trace_id = Self::generate_trace_id("summarize-batch");
        self.emit_observer_sync_event(ObserverEvent::ConversationStart {
            trace_id: trace_id.clone(),
            session_key: "summarize-batch".to_string(),
            channel: String::new(),
            chat_id: String::new(),
            sender_id: "summarizer".to_string(),
            content: String::new(),
        });
        self.emit_observer_async_event(ObserverEvent::LlmRequest {
            trace_id: trace_id.clone(),
            round: 0,
            model: self.model.clone(),
            messages: vec![],
            tools: vec![],
            messages_count: 0,
            tools_count: 0,
            provider_name: String::new(),
            api_key: String::new(),
            api_base: String::new(),
        });
        let start = std::time::Instant::now();
        let response = tokio_block_on(async {
            self.provider.chat(&self.model, messages, summarize_opts, vec![]).await
        });
        let duration_ms = start.elapsed().as_millis() as u64;
        let response_content = response.as_ref().ok().map(|r| r.content.clone()).unwrap_or_default();
        self.emit_observer_async_event(ObserverEvent::LlmResponse {
            trace_id: trace_id.clone(),
            round: 0,
            duration_ms,
            has_tool_calls: false,
            content: response_content.clone(),
            tool_calls: vec![],
            tool_calls_count: 0,
            finish_reason: Some("stop".to_string()),
            usage: None,
        });
        self.emit_observer_sync_event(ObserverEvent::ConversationEnd {
            trace_id,
            session_key: "summarize-batch".to_string(),
            total_rounds: 1,
            duration_ms,
            content: response_content,
            channel: String::new(),
            chat_id: String::new(),
        });

        match response {
            Ok(resp) => resp.content,
            Err(_) => String::new(),
        }
    }

    /// Generate a trace ID for summarization observer events.
    fn generate_trace_id(label: &str) -> String {
        format!(
            "{}-{}",
            label,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        )
    }

    /// Emit an observer event synchronously (for ConversationStart/End).
    fn emit_observer_sync_event(&self, event: ObserverEvent) {
        if let Some(ref mgr) = self.observer_manager {
            let conv_event = event.to_conversation_event();
            tokio_block_on(async { mgr.emit_sync(conv_event).await });
        }
    }

    /// Emit an observer event asynchronously (for LlmRequest/Response).
    fn emit_observer_async_event(&self, event: ObserverEvent) {
        if let Some(ref mgr) = self.observer_manager {
            let conv_event = event.to_conversation_event();
            let mgr = Arc::clone(mgr);
            // Use tokio_block_on since we may be in a sync context
            tokio_block_on(async {
                tokio::spawn(async move {
                    mgr.emit(conv_event).await;
                })
                .await
                .ok()
            });
        }
    }
}

/// Helper to run an async LLM call in a blocking context.
fn tokio_block_on<F: std::future::Future>(future: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            // We're inside a tokio runtime. Use block_in_place to avoid deadlock.
            tokio::task::block_in_place(|| handle.block_on(future))
        }
        Err(_) => {
            // No tokio runtime; create one.
            tokio::runtime::Runtime::new()
                .expect("Failed to create tokio runtime")
                .block_on(future)
        }
    }
}

// ---------------------------------------------------------------------------
// Force compression (emergency context reduction)
// ---------------------------------------------------------------------------

/// Force compress a conversation by dropping the oldest 50% of messages.
///
/// Keeps the system prompt (first message), adds a compression note,
/// keeps the second half of conversation, and keeps the last message.
///
/// Mirrors Go's `forceCompression`.
pub fn force_compress_turns(history: &[ConversationTurn]) -> Vec<ConversationTurn> {
    if history.len() <= 4 {
        return history.to_vec();
    }

    // Keep first (system) and last (trigger) messages.
    let conversation = &history[1..history.len() - 1];
    if conversation.is_empty() {
        return history.to_vec();
    }

    let mid = conversation.len() / 2;
    let dropped_count = mid;
    let kept_conversation = &conversation[mid..];

    let mut new_history = Vec::new();

    // System prompt.
    new_history.push(history[0].clone());

    // Compression note.
    new_history.push(ConversationTurn {
        role: "system".to_string(),
        content: format!(
            "[System: Emergency compression dropped {} oldest messages due to context limit]",
            dropped_count
        ),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: chrono::Utc::now().to_rfc3339(),
        reasoning_content: None,
    });

    // Kept conversation.
    new_history.extend(kept_conversation.iter().cloned());

    // Last message.
    new_history.push(history[history.len() - 1].clone());

    info!(
        "[SessionStore] Forced compression: dropped {} messages, new history has {} messages",
        dropped_count,
        new_history.len()
    );

    new_history
}

// ---------------------------------------------------------------------------
// Internal channel check
// ---------------------------------------------------------------------------

/// Check if a channel is internal (not user-facing).
pub fn is_internal_channel(channel: &str) -> bool {
    matches!(channel, "cli" | "system" | "subagent")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

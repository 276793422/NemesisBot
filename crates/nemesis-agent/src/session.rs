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
            info!("Loaded {} sessions from disk", loaded);
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
}

impl Summarizer {
    /// Create a new summarizer.
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        model: String,
        context_window: usize,
        session_store: Arc<SessionStore>,
        notifier: Box<dyn SummarizationNotifier>,
    ) -> Self {
        Self {
            provider,
            model,
            context_window,
            session_store,
            notifier,
            summarizing: Arc::new(DashMap::new()),
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
            let truncated: Vec<StoredMessage> = history[history.len().saturating_sub(4)..]
                .iter()
                .map(|t| t.into())
                .collect();
            self.session_store.set_history(session_key, truncated);
            self.session_store.set_summary(session_key, &final_summary);

            if let Err(e) = self.session_store.save(session_key) {
                warn!("Failed to save session after summarization: {}", e);
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
        }];

        // Use tokio runtime for the async LLM call.
        // Summarization uses conservative parameters matching Go:
        // max_tokens=1024, temperature=0.3 (deterministic, concise output).
        let summarize_opts = Some(ChatOptions {
            max_tokens: Some(1024),
            temperature: Some(0.3),
            ..Default::default()
        });
        let response = tokio_block_on(async {
            self.provider.chat(&self.model, messages, summarize_opts, vec![]).await
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
        }];

        // Summarization uses conservative parameters matching Go:
        // max_tokens=1024, temperature=0.3 (deterministic, concise output).
        let summarize_opts = Some(ChatOptions {
            max_tokens: Some(1024),
            temperature: Some(0.3),
            ..Default::default()
        });
        let response = tokio_block_on(async {
            self.provider.chat(&self.model, messages, summarize_opts, vec![]).await
        });

        match response {
            Ok(resp) => resp.content,
            Err(_) => String::new(),
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
    });

    // Kept conversation.
    new_history.extend(kept_conversation.iter().cloned());

    // Last message.
    new_history.push(history[history.len() - 1].clone());

    info!(
        "Forced compression: dropped {} messages, new history has {} messages",
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
mod tests {
    use super::*;
    

    // --- Session tests ---

    #[test]
    fn test_session_new() {
        let session = Session::new("web:chat1", "web", "chat1");
        assert_eq!(session.session_key, "web:chat1");
        assert_eq!(session.channel, "web");
        assert_eq!(session.chat_id, "chat1");
        assert!(!session.busy);
        assert!(session.last_channel.is_none());
        assert!(session.last_chat_id.is_none());
    }

    // --- SessionManager tests ---

    #[test]
    fn test_session_manager_get_or_create() {
        let mgr = SessionManager::with_default_timeout();
        assert!(mgr.is_empty());

        let session = mgr.get_or_create("web:chat1", "web", "chat1");
        assert_eq!(session.session_key, "web:chat1");
        assert_eq!(mgr.len(), 1);

        // Second call returns same session.
        let session2 = mgr.get_or_create("web:chat1", "web", "chat1");
        assert_eq!(mgr.len(), 1);
        assert_eq!(session2.session_key, session.session_key);
    }

    #[test]
    fn test_session_manager_set_busy() {
        let mgr = SessionManager::with_default_timeout();
        mgr.get_or_create("web:chat1", "web", "chat1");

        assert_eq!(mgr.is_busy("web:chat1"), Some(false));
        assert!(mgr.set_busy("web:chat1", true));
        assert_eq!(mgr.is_busy("web:chat1"), Some(true));

        assert!(!mgr.set_busy("nonexistent", true));
        assert_eq!(mgr.is_busy("nonexistent"), None);
    }

    #[test]
    fn test_session_manager_last_channel_chat_id() {
        let mgr = SessionManager::with_default_timeout();
        mgr.get_or_create("web:chat1", "web", "chat1");

        mgr.set_last_channel("web:chat1", "telegram");
        mgr.set_last_chat_id("web:chat1", "chat42");

        let session = mgr.get_or_create("web:chat1", "web", "chat1");
        assert_eq!(session.last_channel.as_deref(), Some("telegram"));
        assert_eq!(session.last_chat_id.as_deref(), Some("chat42"));
    }

    #[test]
    fn test_session_manager_cleanup_expired() {
        let mgr = SessionManager::new(Duration::from_millis(50));
        mgr.get_or_create("web:chat1", "web", "chat1");

        // Force session into the past.
        {
            let mut session = mgr.sessions.get_mut("web:chat1").unwrap();
            session.last_active = Utc::now() - chrono::Duration::seconds(60);
        }

        let removed = mgr.cleanup_expired();
        assert_eq!(removed.len(), 1);
        assert!(mgr.is_empty());
    }

    // --- SessionStore tests ---

    #[test]
    fn test_session_store_in_memory() {
        let store = SessionStore::new_in_memory();

        let session = store.get_or_create("test:key1");
        assert_eq!(session.key, "test:key1");
        assert!(session.messages.is_empty());
        assert!(session.summary.is_empty());
    }

    #[test]
    fn test_session_store_history() {
        let store = SessionStore::new_in_memory();
        store.get_or_create("test:key1");

        let messages = vec![
            StoredMessage {
                role: "user".to_string(),
                content: "Hello".to_string(),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: "2026-01-01T00:00:00Z".to_string(),
            },
            StoredMessage {
                role: "assistant".to_string(),
                content: "Hi there!".to_string(),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: "2026-01-01T00:00:01Z".to_string(),
            },
        ];

        store.set_history("test:key1", messages.clone());
        let history = store.get_history("test:key1");
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].content, "Hello");
        assert_eq!(history[1].content, "Hi there!");
    }

    #[test]
    fn test_session_store_summary() {
        let store = SessionStore::new_in_memory();
        store.get_or_create("test:key1");

        assert!(store.get_summary("test:key1").is_empty());

        store.set_summary("test:key1", "This is a summary of the conversation.");
        assert_eq!(
            store.get_summary("test:key1"),
            "This is a summary of the conversation."
        );
    }

    #[test]
    fn test_session_store_truncate() {
        let store = SessionStore::new_in_memory();
        store.get_or_create("test:key1");

        let messages: Vec<StoredMessage> = (0..10)
            .map(|i| StoredMessage {
                role: "user".to_string(),
                content: format!("msg_{}", i),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: "2026-01-01T00:00:00Z".to_string(),
            })
            .collect();

        store.set_history("test:key1", messages);
        store.truncate_history("test:key1", 4);

        let history = store.get_history("test:key1");
        assert_eq!(history.len(), 4);
        assert_eq!(history[0].content, "msg_6");
        assert_eq!(history[3].content, "msg_9");
    }

    #[test]
    fn test_session_store_disk_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new_with_storage(dir.path());

        store.get_or_create("disk:key1");
        store.set_summary("disk:key1", "Test summary");
        store.set_history("disk:key1", vec![
            StoredMessage {
                role: "user".to_string(),
                content: "Hello from disk".to_string(),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: "2026-01-01T00:00:00Z".to_string(),
            },
        ]);
        store.save("disk:key1").unwrap();

        // Create a new store from the same directory.
        let store2 = SessionStore::new_with_storage(dir.path());
        assert!(store2.contains("disk:key1"));
        assert_eq!(store2.get_summary("disk:key1"), "Test summary");
        let history = store2.get_history("disk:key1");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].content, "Hello from disk");
    }

    #[test]
    fn test_session_store_save_invalid_key() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new_with_storage(dir.path());
        // The key ".." should be rejected (it becomes "." after sanitize, which is rejected).
        store.get_or_create("..");
        let result = store.save("..");
        assert!(result.is_err());
    }

    #[test]
    fn test_session_store_no_persistence() {
        let store = SessionStore::new_in_memory();
        store.get_or_create("mem:key1");
        // save should succeed silently when no storage dir.
        assert!(store.save("mem:key1").is_ok());
    }

    // --- Token estimation tests ---

    #[test]
    fn test_estimate_tokens_empty() {
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn test_estimate_tokens_ascii() {
        // "Hello world" = 11 chars, 11*2/5 = 4
        assert_eq!(estimate_tokens("Hello world"), 4);
    }

    #[test]
    fn test_estimate_tokens_for_turns() {
        let turns = vec![
            ConversationTurn {
                role: "user".to_string(),
                content: "Hello".to_string(),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: String::new(),
            },
            ConversationTurn {
                role: "assistant".to_string(),
                content: "World".to_string(),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: String::new(),
            },
        ];
        // "Hello" = 5 chars, "World" = 5 chars, total = 10, 10*2/5 = 4
        assert_eq!(estimate_tokens_for_turns(&turns), 4);
    }

    // --- Force compression tests ---

    #[test]
    fn test_force_compress_short() {
        let history: Vec<ConversationTurn> = (0..4)
            .map(|i| ConversationTurn {
                role: if i == 0 { "system" } else { "user" }.to_string(),
                content: format!("msg_{}", i),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: String::new(),
            })
            .collect();

        let result = force_compress_turns(&history);
        assert_eq!(result.len(), 4);
        assert_eq!(result, history);
    }

    #[test]
    fn test_force_compress_long() {
        let history: Vec<ConversationTurn> = (0..10)
            .map(|i| ConversationTurn {
                role: if i == 0 {
                    "system".to_string()
                } else {
                    format!("role_{}", i)
                },
                content: format!("msg_{}", i),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: String::new(),
            })
            .collect();

        let result = force_compress_turns(&history);
        assert!(result.len() < history.len());
        assert_eq!(result[0].content, "msg_0"); // System prompt kept
        assert!(result[1].content.contains("Emergency compression")); // Compression note
        assert_eq!(result.last().unwrap().content, "msg_9"); // Last message kept
    }

    // --- Sanitize filename tests ---

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("web:chat1"), "web_chat1");
        assert_eq!(sanitize_filename("rpc:12345"), "rpc_12345");
        assert_eq!(sanitize_filename("simple"), "simple");
        assert_eq!(sanitize_filename("a\\b/c:d"), "a_b_c_d");
    }

    // --- Internal channel tests ---

    #[test]
    fn test_is_internal_channel() {
        assert!(is_internal_channel("cli"));
        assert!(is_internal_channel("system"));
        assert!(is_internal_channel("subagent"));
        assert!(!is_internal_channel("web"));
        assert!(!is_internal_channel("rpc"));
        assert!(!is_internal_channel("discord"));
    }

    // --- StoredMessage conversion tests ---

    #[test]
    fn test_stored_message_roundtrip() {
        let turn = ConversationTurn {
            role: "assistant".to_string(),
            content: "Let me search for that.".to_string(),
            tool_calls: vec![crate::types::ToolCallInfo {
                id: "tc_1".to_string(),
                name: "search".to_string(),
                arguments: r#"{"query":"rust"}"#.to_string(),
            }],
            tool_call_id: None,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
        };

        let stored: StoredMessage = (&turn).into();
        assert_eq!(stored.role, "assistant");
        assert_eq!(stored.tool_calls.len(), 1);

        let back: ConversationTurn = stored.into();
        assert_eq!(back.role, "assistant");
        assert_eq!(back.tool_calls.len(), 1);
        assert_eq!(back.tool_calls[0].name, "search");
    }

    // --- Additional session coverage tests ---

    #[test]
    fn test_session_touch_updates_last_active() {
        let mut session = Session::new("web:chat1", "web", "chat1");
        let before = session.last_active;
        session.touch();
        assert!(session.last_active >= before);
    }

    #[test]
    fn test_session_serialization_roundtrip() {
        let session = Session::new("web:chat1", "web", "chat1");
        let json = serde_json::to_string(&session).unwrap();
        let parsed: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.session_key, "web:chat1");
        assert_eq!(parsed.channel, "web");
        assert_eq!(parsed.chat_id, "chat1");
    }

    #[test]
    fn test_session_manager_contains() {
        let mgr = SessionManager::with_default_timeout();
        assert!(!mgr.contains("web:chat1"));
        mgr.get_or_create("web:chat1", "web", "chat1");
        assert!(mgr.contains("web:chat1"));
    }

    #[test]
    fn test_session_manager_remove() {
        let mgr = SessionManager::with_default_timeout();
        mgr.get_or_create("web:chat1", "web", "chat1");
        assert!(mgr.contains("web:chat1"));

        let removed = mgr.remove("web:chat1");
        assert!(removed.is_some());
        assert!(!mgr.contains("web:chat1"));

        let removed_again = mgr.remove("web:chat1");
        assert!(removed_again.is_none());
    }

    #[test]
    fn test_session_manager_cleanup_with_timeout_no_expired() {
        let mgr = SessionManager::new(Duration::from_secs(3600));
        mgr.get_or_create("web:chat1", "web", "chat1");
        let removed = mgr.cleanup_expired();
        assert!(removed.is_empty());
        assert_eq!(mgr.len(), 1);
    }

    #[test]
    fn test_session_manager_set_last_channel_nonexistent() {
        let mgr = SessionManager::with_default_timeout();
        // Should not panic when setting channel on nonexistent session
        mgr.set_last_channel("nonexistent", "web");
        mgr.set_last_chat_id("nonexistent", "chat1");
    }

    #[test]
    fn test_session_store_set_history_nonexistent() {
        let store = SessionStore::new_in_memory();
        // Setting history on nonexistent session should do nothing
        store.set_history("nonexistent", vec![StoredMessage {
            role: "user".to_string(),
            content: "test".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
        }]);
        assert!(store.get_history("nonexistent").is_empty());
    }

    #[test]
    fn test_session_store_set_summary_nonexistent() {
        let store = SessionStore::new_in_memory();
        store.set_summary("nonexistent", "test summary");
        assert!(store.get_summary("nonexistent").is_empty());
    }

    #[test]
    fn test_session_store_truncate_fewer_than_keep() {
        let store = SessionStore::new_in_memory();
        store.get_or_create("test:trunc");
        store.set_history("test:trunc", vec![StoredMessage {
            role: "user".to_string(),
            content: "msg".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
        }]);
        store.truncate_history("test:trunc", 10);
        let history = store.get_history("test:trunc");
        assert_eq!(history.len(), 1); // not truncated
    }

    #[test]
    fn test_session_store_contains() {
        let store = SessionStore::new_in_memory();
        assert!(!store.contains("test:contains"));
        store.get_or_create("test:contains");
        assert!(store.contains("test:contains"));
    }

    #[test]
    fn test_session_store_remove() {
        let store = SessionStore::new_in_memory();
        store.get_or_create("test:remove");
        assert!(store.contains("test:remove"));
        let removed = store.remove("test:remove");
        assert!(removed.is_some());
        assert!(!store.contains("test:remove"));
    }

    #[test]
    fn test_session_store_len_and_empty() {
        let store = SessionStore::new_in_memory();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
        store.get_or_create("test:1");
        assert!(!store.is_empty());
        assert_eq!(store.len(), 1);
        store.get_or_create("test:2");
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn test_session_store_save_nonexistent_session() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new_with_storage(dir.path());
        // Saving nonexistent session should succeed silently
        assert!(store.save("nonexistent").is_ok());
    }

    #[test]
    fn test_stored_session_serialization() {
        let session = StoredSession {
            key: "test:ser".to_string(),
            messages: vec![StoredMessage {
                role: "user".to_string(),
                content: "hello".to_string(),
                tool_calls: vec![StoredToolCall {
                    id: "tc_1".to_string(),
                    name: "test".to_string(),
                    arguments: "{}".to_string(),
                }],
                tool_call_id: Some("tc_1".to_string()),
                timestamp: "2026-01-01T00:00:00Z".to_string(),
            }],
            summary: "test summary".to_string(),
            created: Utc::now(),
            updated: Utc::now(),
        };
        let json = serde_json::to_string(&session).unwrap();
        let parsed: StoredSession = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.key, "test:ser");
        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].tool_calls.len(), 1);
    }

    #[test]
    fn test_estimate_tokens_for_messages() {
        let messages = vec![
            StoredMessage {
                role: "user".to_string(),
                content: "Hello world".to_string(),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: String::new(),
            },
        ];
        let tokens = estimate_tokens_for_messages(&messages);
        assert!(tokens > 0);
    }

    #[test]
    fn test_force_compress_exact_boundary() {
        // Test with exactly 5 messages (boundary for the > 4 check)
        let history: Vec<ConversationTurn> = (0..5)
            .map(|i| ConversationTurn {
                role: if i == 0 { "system" } else { "user" }.to_string(),
                content: format!("msg_{}", i),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: String::new(),
            })
            .collect();
        let result = force_compress_turns(&history);
        assert!(result.len() <= history.len());
    }

    #[test]
    fn test_force_compress_empty_conversation() {
        // History with just system and one message (no "conversation" part)
        let history = vec![
            ConversationTurn {
                role: "system".to_string(),
                content: "You are helpful".to_string(),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: String::new(),
            },
            ConversationTurn {
                role: "user".to_string(),
                content: "hello".to_string(),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: String::new(),
            },
        ];
        let result = force_compress_turns(&history);
        // With only 2 messages (<=4), should return unchanged
        assert_eq!(result.len(), 2);
    }

    // --- Additional session coverage tests ---

    #[test]
    fn test_session_fields_after_create() {
        let session = Session::new("web:chat1", "web", "chat1");
        assert_eq!(session.session_key, "web:chat1");
        assert_eq!(session.channel, "web");
        assert_eq!(session.chat_id, "chat1");
        assert!(!session.busy);
    }

    #[test]
    fn test_session_json_roundtrip() {
        let session = Session::new("web:chat1", "web", "chat1");
        let json = serde_json::to_string(&session).unwrap();
        let parsed: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.session_key, "web:chat1");
        assert_eq!(parsed.channel, "web");
        assert_eq!(parsed.chat_id, "chat1");
    }

    #[test]
    fn test_session_manager_with_default_timeout() {
        let mgr = SessionManager::with_default_timeout();
        let session = mgr.get_or_create("test-key", "web", "chat1");
        assert_eq!(session.session_key, "test-key");
        assert!(!session.busy);
    }

    #[test]
    fn test_session_manager_set_last_channel() {
        let mgr = SessionManager::with_default_timeout();
        mgr.get_or_create("_default", "cli", "direct");
        mgr.set_last_channel("_default", "discord");
        let session = mgr.get_or_create("_default", "cli", "direct");
        assert_eq!(session.last_channel.as_deref(), Some("discord"));
    }

    #[test]
    fn test_session_manager_set_last_chat_id() {
        let mgr = SessionManager::with_default_timeout();
        mgr.get_or_create("_default", "cli", "direct");
        mgr.set_last_chat_id("_default", "chat-99");
        let session = mgr.get_or_create("_default", "cli", "direct");
        assert_eq!(session.last_chat_id.as_deref(), Some("chat-99"));
    }

    #[test]
    fn test_session_store_new_in_memory() {
        let store = SessionStore::new_in_memory();
        let data = store.get_or_create("test-key");
        assert!(data.messages.is_empty());
    }

    #[test]
    fn test_session_store_set_and_get_history() {
        let store = SessionStore::new_in_memory();
        store.get_or_create("test-key"); // Must create first
        let messages = vec![
            StoredMessage {
                role: "user".to_string(),
                content: "hello".to_string(),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: "2026-01-01T00:00:00Z".to_string(),
            },
            StoredMessage {
                role: "assistant".to_string(),
                content: "hi there".to_string(),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: "2026-01-01T00:00:01Z".to_string(),
            },
        ];
        store.set_history("test-key", messages);
        let data = store.get_or_create("test-key");
        assert_eq!(data.messages.len(), 2);
        assert_eq!(data.messages[0].content, "hello");
    }

    #[test]
    fn test_session_store_set_and_get_summary() {
        let store = SessionStore::new_in_memory();
        store.get_or_create("test-key"); // Must create first
        store.set_summary("test-key", "This is a summary of the conversation.");
        let summary = store.get_summary("test-key");
        assert_eq!(summary, "This is a summary of the conversation.");
    }

    #[test]
    fn test_session_store_get_summary_nonexistent() {
        let store = SessionStore::new_in_memory();
        let summary = store.get_summary("nonexistent");
        assert!(summary.is_empty());
    }

    #[test]
    fn test_estimate_tokens_basic() {
        assert_eq!(estimate_tokens(""), 0);
        // estimate_tokens uses char_count * 2 / 5, so needs at least 3 chars for > 0
        assert!(estimate_tokens("hello world") > 0);
    }

    #[test]
    fn test_estimate_tokens_for_turns_basic() {
        let turns = vec![
            ConversationTurn {
                role: "user".to_string(),
                content: "Hello world".to_string(),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: String::new(),
            },
        ];
        let tokens = estimate_tokens_for_turns(&turns);
        assert!(tokens > 0);
    }

    #[test]
    fn test_estimate_tokens_for_turns_empty() {
        let turns: Vec<ConversationTurn> = vec![];
        let tokens = estimate_tokens_for_turns(&turns);
        assert_eq!(tokens, 0);
    }

    #[test]
    fn test_stored_message_from_conversation_turn() {
        let turn = ConversationTurn {
            role: "user".to_string(),
            content: "hello".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
        };
        let stored: StoredMessage = StoredMessage::from(&turn);
        assert_eq!(stored.role, "user");
        assert_eq!(stored.content, "hello");
        assert_eq!(stored.timestamp, "2026-01-01T00:00:00Z");
    }

    // --- Additional session coverage ---

    #[test]
    fn test_session_store_truncate_history() {
        let store = SessionStore::new_in_memory();
        store.get_or_create("test-key");
        let msgs: Vec<StoredMessage> = (0..10).map(|i| StoredMessage {
            role: "user".to_string(),
            content: format!("msg {}", i),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
        }).collect();
        store.set_history("test-key", msgs);
        store.truncate_history("test-key", 3);
        let data = store.get_or_create("test-key");
        assert_eq!(data.messages.len(), 3);
        assert_eq!(data.messages[0].content, "msg 7");
    }

    #[test]
    fn test_session_store_truncate_empty() {
        let store = SessionStore::new_in_memory();
        store.get_or_create("test-key");
        store.truncate_history("test-key", 5);
        let data = store.get_or_create("test-key");
        assert!(data.messages.is_empty());
    }

    #[test]
    fn test_session_store_truncate_to_zero() {
        let store = SessionStore::new_in_memory();
        store.get_or_create("test-key");
        let msgs: Vec<StoredMessage> = (0..5).map(|i| StoredMessage {
            role: "user".to_string(),
            content: format!("msg {}", i),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
        }).collect();
        store.set_history("test-key", msgs);
        store.truncate_history("test-key", 0);
        let data = store.get_or_create("test-key");
        assert!(data.messages.is_empty());
    }

    #[test]
    fn test_session_manager_get_or_create_default() {
        let mgr = SessionManager::with_default_timeout();
        let s1 = mgr.get_or_create("_default", "cli", "direct");
        let s2 = mgr.get_or_create("_default", "web", "chat1");
        assert_eq!(s1.session_key, s2.session_key);
    }

    #[test]
    fn test_force_compress_with_many_messages() {
        let mut history: Vec<ConversationTurn> = vec![
            ConversationTurn {
                role: "system".to_string(),
                content: "You are helpful".to_string(),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: String::new(),
            },
        ];
        for i in 0..20 {
            history.push(ConversationTurn {
                role: if i % 2 == 0 { "user" } else { "assistant" }.to_string(),
                content: format!("message {}", i),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: String::new(),
            });
        }
        let result = force_compress_turns(&history);
        // Should be compressed: system + compression note + kept half of conversation + last
        // Original: 1 system + 20 messages = 21 total
        // conversation = 19 messages, mid = 9, kept = 10, plus system + note + last = 13
        assert!(result.len() < history.len());
        assert!(result.len() <= 14);
        assert_eq!(result[0].role, "system");
    }

    #[test]
    fn test_session_store_get_summary_default_empty() {
        let store = SessionStore::new_in_memory();
        let data = store.get_or_create("test-key");
        assert!(data.summary.is_empty());
    }

    #[test]
    fn test_stored_session_debug() {
        let session = StoredSession {
            key: "test-key".to_string(),
            messages: Vec::new(),
            summary: String::new(),
            created: chrono::Utc::now(),
            updated: chrono::Utc::now(),
        };
        let debug_str = format!("{:?}", session);
        assert!(debug_str.contains("test-key"));
    }

    // --- Additional coverage for session and summarizer ---

    use async_trait::async_trait;

    /// A null LLM provider for testing summarization.
    struct NullLlmProvider;

    #[async_trait]
    impl LlmProvider for NullLlmProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
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

    #[test]
    fn test_session_store_disk_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let store = SessionStore::new_with_storage(tmp.path());

        store.get_or_create("web:chat1");
        let messages: Vec<StoredMessage> = (0..5)
            .map(|i| StoredMessage {
                role: if i % 2 == 0 { "user" } else { "assistant" }.to_string(),
                content: format!("Message {}", i),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: chrono::Utc::now().to_rfc3339(),
            })
            .collect();
        store.set_history("web:chat1", messages);
        store.set_summary("web:chat1", "A summary of the conversation");
        store.save("web:chat1").unwrap();

        // Create a new store from the same disk to verify persistence
        let store2 = SessionStore::new_with_storage(tmp.path());
        let loaded = store2.get_history("web:chat1");
        assert_eq!(loaded.len(), 5);
        assert_eq!(store2.get_summary("web:chat1"), "A summary of the conversation");
    }

    #[test]
    fn test_session_store_disk_corrupted_file() {
        let tmp = tempfile::tempdir().unwrap();
        // Write a corrupted JSON file
        let corrupted_path = tmp.path().join("corrupted.json");
        std::fs::write(&corrupted_path, "not valid json").unwrap();

        // Should not panic on load
        let store = SessionStore::new_with_storage(tmp.path());
        assert!(store.is_empty());
    }

    #[test]
    fn test_session_store_disk_save_invalid_chars() {
        let tmp = tempfile::tempdir().unwrap();
        let store = SessionStore::new_with_storage(tmp.path());
        store.get_or_create("test/session");

        // Keys with slashes should be sanitized for filename
        let result = store.save("test/session");
        assert!(result.is_ok());
    }

    #[test]
    fn test_session_store_remove_with_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let store = SessionStore::new_with_storage(tmp.path());
        store.get_or_create("key1");
        store.set_summary("key1", "summary1");

        let removed = store.remove("key1");
        assert!(removed.is_some());
        assert!(!store.contains("key1"));
        assert!(store.get_history("key1").is_empty());
    }

    #[test]
    fn test_session_store_len_empty_combined() {
        let store = SessionStore::new_in_memory();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);

        store.get_or_create("key1");
        assert!(!store.is_empty());
        assert_eq!(store.len(), 1);

        store.get_or_create("key2");
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn test_session_store_get_or_create_multiple() {
        let store = SessionStore::new_in_memory();
        let s1 = store.get_or_create("key1");
        assert!(s1.messages.is_empty());

        // Second call returns existing
        let s2 = store.get_or_create("key1");
        assert!(s2.messages.is_empty());
    }

    #[test]
    fn test_session_store_truncate_exact() {
        let store = SessionStore::new_in_memory();
        store.get_or_create("key1");
        let msgs: Vec<StoredMessage> = (0..5)
            .map(|i| StoredMessage {
                role: "user".to_string(),
                content: format!("msg {}", i),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: String::new(),
            })
            .collect();
        store.set_history("key1", msgs);

        // Truncate to exactly 3
        store.truncate_history("key1", 3);
        let history = store.get_history("key1");
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].content, "msg 2");
        assert_eq!(history[2].content, "msg 4");
    }

    #[test]
    fn test_stored_message_from_conversation_turn_with_tools() {
        let turn = ConversationTurn {
            role: "assistant".to_string(),
            content: "Using tool".to_string(),
            tool_calls: vec![crate::types::ToolCallInfo {
                id: "tc_1".to_string(),
                name: "read_file".to_string(),
                arguments: r#"{"path":"/test"}"#.to_string(),
            }],
            tool_call_id: Some("tc_1".to_string()),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
        };

        let stored: StoredMessage = StoredMessage::from(&turn);
        assert_eq!(stored.role, "assistant");
        assert_eq!(stored.content, "Using tool");
        assert_eq!(stored.tool_calls.len(), 1);
        assert_eq!(stored.tool_calls[0].id, "tc_1");
        assert_eq!(stored.tool_call_id, Some("tc_1".to_string()));
    }

    #[test]
    fn test_stored_message_into_conversation_turn() {
        let stored = StoredMessage {
            role: "user".to_string(),
            content: "Hello".to_string(),
            tool_calls: vec![StoredToolCall {
                id: "tc_1".to_string(),
                name: "echo".to_string(),
                arguments: "{}".to_string(),
            }],
            tool_call_id: None,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
        };

        let turn: ConversationTurn = ConversationTurn::from(stored);
        assert_eq!(turn.role, "user");
        assert_eq!(turn.content, "Hello");
        assert_eq!(turn.tool_calls.len(), 1);
        assert_eq!(turn.tool_calls[0].name, "echo");
    }

    #[test]
    fn test_stored_tool_call_debug() {
        let tc = StoredToolCall {
            id: "tc_1".to_string(),
            name: "tool1".to_string(),
            arguments: "{}".to_string(),
        };
        let debug_str = format!("{:?}", tc);
        assert!(debug_str.contains("tc_1"));
        assert!(debug_str.contains("tool1"));
    }

    #[test]
    fn test_estimate_tokens_for_messages_empty() {
        let messages: Vec<StoredMessage> = Vec::new();
        assert_eq!(estimate_tokens_for_messages(&messages), 0);
    }

    #[test]
    fn test_estimate_tokens_for_messages_with_content() {
        let messages = vec![StoredMessage {
            role: "user".to_string(),
            content: "Hello world".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
        }];
        let tokens = estimate_tokens_for_messages(&messages);
        assert!(tokens > 0);
    }

    #[test]
    fn test_session_manager_cleanup_with_custom_timeout() {
        let manager = SessionManager::new(std::time::Duration::from_secs(0)); // Immediate timeout
        manager.get_or_create("s1", "web", "chat1");
        manager.get_or_create("s2", "web", "chat2");

        // With 0 timeout, sessions should be expired immediately
        // Just verify it doesn't panic
        manager.cleanup_expired();
    }

    #[test]
    fn test_session_manager_set_busy_and_check() {
        let manager = SessionManager::new(std::time::Duration::from_secs(3600));
        let session = manager.get_or_create("s1", "web", "chat1");
        assert!(!session.busy);

        manager.set_busy("s1", true);
        assert_eq!(manager.is_busy("s1"), Some(true));

        manager.set_busy("s1", false);
        assert_eq!(manager.is_busy("s1"), Some(false));
    }

    #[test]
    fn test_session_manager_set_busy_nonexistent() {
        let manager = SessionManager::new(std::time::Duration::from_secs(3600));
        // Should not panic, returns false
        assert!(!manager.set_busy("nonexistent", true));
    }

    #[test]
    fn test_session_manager_get_session_nonexistent() {
        let manager = SessionManager::new(std::time::Duration::from_secs(3600));
        assert!(manager.is_busy("nonexistent").is_none());
    }

    #[test]
    fn test_sanitize_filename_special_chars() {
        assert_eq!(sanitize_filename("web:chat1"), "web_chat1");
        assert_eq!(sanitize_filename("a/b\\c"), "a_b_c");
        assert_eq!(sanitize_filename("normal"), "normal");
    }

    #[test]
    fn test_session_store_new_with_storage_creates_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let sub_dir = tmp.path().join("sessions");
        let store = SessionStore::new_with_storage(&sub_dir);
        assert!(sub_dir.exists());
        assert!(store.is_empty());
    }

    #[test]
    fn test_summarizer_should_summarize_short_history() {
        let store = Arc::new(SessionStore::new_in_memory());
        let summarizer = Summarizer::new_silent(
            Arc::new(NullLlmProvider),
            "test-model".to_string(),
            128000,
            store,
        );
        let history: Vec<ConversationTurn> = (0..5)
            .map(|i| ConversationTurn {
                role: "user".to_string(),
                content: format!("Short {}", i),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: String::new(),
            })
            .collect();
        assert!(!summarizer.should_summarize(&history, 128000));
    }

    #[test]
    fn test_summarizer_should_summarize_long_history() {
        let store = Arc::new(SessionStore::new_in_memory());
        let summarizer = Summarizer::new_silent(
            Arc::new(NullLlmProvider),
            "test-model".to_string(),
            128000,
            store,
        );
        // Create history with enough messages and tokens
        let history: Vec<ConversationTurn> = (0..30)
            .map(|i| ConversationTurn {
                role: "user".to_string(),
                content: format!("A longer message with more content to increase token estimation significantly {}", i),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: String::new(),
            })
            .collect();
        // 30 messages > 20 threshold
        assert!(summarizer.should_summarize(&history, 128000));
    }

    #[test]
    fn test_null_notifier() {
        let notifier = NullNotifier;
        // Should not panic
        notifier.notify("web", "chat1", "test message");
    }

    // --- Summarizer coverage tests ---

    #[test]
    fn test_summarizer_should_summarize_by_token_threshold() {
        let store = Arc::new(SessionStore::new_in_memory());
        let summarizer = Summarizer::new_silent(
            Arc::new(NullLlmProvider),
            "test-model".to_string(),
            100, // Very small context window
            store,
        );
        // Create history with enough tokens to exceed 75% of 100 = 75 tokens
        let history: Vec<ConversationTurn> = (0..5)
            .map(|i| ConversationTurn {
                role: "user".to_string(),
                content: format!("A reasonably long message with enough text to exceed token threshold {}", i),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: String::new(),
            })
            .collect();
        // Token threshold is 100 * 75 / 100 = 75 tokens
        // With content ~70 chars each, 5 * 70 * 2/5 = 140 tokens > 75
        assert!(summarizer.should_summarize(&history, 100));
    }

    #[test]
    fn test_summarizer_should_not_summarize_empty() {
        let store = Arc::new(SessionStore::new_in_memory());
        let summarizer = Summarizer::new_silent(
            Arc::new(NullLlmProvider),
            "test-model".to_string(),
            128000,
            store,
        );
        let history: Vec<ConversationTurn> = vec![];
        assert!(!summarizer.should_summarize(&history, 128000));
    }

    #[test]
    fn test_summarizer_summarize_session_too_few_messages() {
        let store = Arc::new(SessionStore::new_in_memory());
        let summarizer = Summarizer::new_silent(
            Arc::new(NullLlmProvider),
            "test-model".to_string(),
            128000,
            store,
        );
        let history: Vec<ConversationTurn> = (0..4)
            .map(|i| ConversationTurn {
                role: "user".to_string(),
                content: format!("msg {}", i),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: String::new(),
            })
            .collect();
        // Only 4 messages (<=4), so summarize_session returns empty
        let result = summarizer.summarize_session("test:session", &history);
        assert!(result.is_empty());
    }

    #[test]
    fn test_summarizer_summarize_session_all_system_messages() {
        let store = Arc::new(SessionStore::new_in_memory());
        let summarizer = Summarizer::new_silent(
            Arc::new(NullLlmProvider),
            "test-model".to_string(),
            128000,
            store,
        );
        let history: Vec<ConversationTurn> = (0..10)
            .map(|i| ConversationTurn {
                role: "system".to_string(),
                content: format!("system msg {}", i),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: String::new(),
            })
            .collect();
        // All system messages -> none pass the user/assistant filter
        let result = summarizer.summarize_session("test:sys", &history);
        assert!(result.is_empty());
    }

    #[test]
    fn test_summarizer_summarize_session_basic() {
        let store = Arc::new(SessionStore::new_in_memory());
        let summarizer = Summarizer::new_silent(
            Arc::new(NullLlmProvider),
            "test-model".to_string(),
            128000,
            store,
        );
        let history: Vec<ConversationTurn> = (0..8)
            .map(|i| ConversationTurn {
                role: if i % 2 == 0 { "user" } else { "assistant" }.to_string(),
                content: format!("Conversation message {}", i),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: String::new(),
            })
            .collect();
        let result = summarizer.summarize_session("test:basic", &history);
        // NullLlmProvider returns "summary"
        assert_eq!(result, "summary");
    }

    #[test]
    fn test_summarizer_maybe_summarize_internal_channel() {
        let store = Arc::new(SessionStore::new_in_memory());
        struct CountingNotifier {
            count: std::sync::atomic::AtomicUsize,
        }
        impl SummarizationNotifier for CountingNotifier {
            fn notify(&self, _channel: &str, _chat_id: &str, _content: &str) {
                self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let notifier = Box::new(CountingNotifier {
            count: std::sync::atomic::AtomicUsize::new(0),
        });
        let summarizer = Summarizer::new(
            Arc::new(NullLlmProvider),
            "test-model".to_string(),
            128000,
            store,
            notifier,
        );
        // Internal channels should not trigger notification
        let history: Vec<ConversationTurn> = (0..30)
            .map(|i| ConversationTurn {
                role: "user".to_string(),
                content: format!("Message {}", i),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: String::new(),
            })
            .collect();
        let result = summarizer.maybe_summarize("test:cli", "cli", "direct", &history, 128000);
        assert!(result);
    }

    #[test]
    fn test_summarizer_maybe_summarize_not_triggered() {
        let store = Arc::new(SessionStore::new_in_memory());
        let summarizer = Summarizer::new_silent(
            Arc::new(NullLlmProvider),
            "test-model".to_string(),
            128000,
            store,
        );
        let history: Vec<ConversationTurn> = (0..5)
            .map(|i| ConversationTurn {
                role: "user".to_string(),
                content: format!("Short {}", i),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: String::new(),
            })
            .collect();
        assert!(!summarizer.maybe_summarize("test:short", "web", "chat1", &history, 128000));
    }

    #[test]
    fn test_force_compress_three_messages() {
        let history = vec![
            ConversationTurn {
                role: "system".to_string(),
                content: "You are helpful".to_string(),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: String::new(),
            },
            ConversationTurn {
                role: "user".to_string(),
                content: "hello".to_string(),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: String::new(),
            },
            ConversationTurn {
                role: "assistant".to_string(),
                content: "hi".to_string(),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: String::new(),
            },
        ];
        // 3 messages (<=4), should return unchanged
        let result = force_compress_turns(&history);
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_force_compress_preserves_system_and_last() {
        let mut history: Vec<ConversationTurn> = vec![
            ConversationTurn {
                role: "system".to_string(),
                content: "System prompt".to_string(),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: String::new(),
            },
        ];
        for i in 0..10 {
            history.push(ConversationTurn {
                role: "user".to_string(),
                content: format!("msg {}", i),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: String::new(),
            });
        }
        let result = force_compress_turns(&history);
        assert_eq!(result[0].content, "System prompt");
        assert!(result[1].content.contains("Emergency compression"));
        assert_eq!(result.last().unwrap().content, "msg 9");
    }

    #[test]
    fn test_session_store_get_or_create_creates_with_timestamps() {
        let store = SessionStore::new_in_memory();
        let session = store.get_or_create("test:ts");
        assert!(session.created <= Utc::now());
        assert!(session.updated <= Utc::now());
    }

    #[test]
    fn test_session_store_get_history_nonexistent_key() {
        let store = SessionStore::new_in_memory();
        let history = store.get_history("nonexistent");
        assert!(history.is_empty());
    }

    #[test]
    fn test_stored_session_default_summary() {
        let store = SessionStore::new_in_memory();
        let session = store.get_or_create("test:default");
        assert!(session.summary.is_empty());
    }

    #[test]
    fn test_session_manager_cleanup_expired_with_timeout_expired() {
        let mgr = SessionManager::new(Duration::from_millis(10));
        mgr.get_or_create("web:chat1", "web", "chat1");
        mgr.get_or_create("web:chat2", "web", "chat2");

        // Force sessions into the past
        {
            let mut s1 = mgr.sessions.get_mut("web:chat1").unwrap();
            s1.last_active = Utc::now() - chrono::Duration::seconds(60);
        }
        {
            let mut s2 = mgr.sessions.get_mut("web:chat2").unwrap();
            s2.last_active = Utc::now() - chrono::Duration::seconds(60);
        }

        let removed = mgr.cleanup_expired_with_timeout(Duration::from_millis(10));
        assert_eq!(removed.len(), 2);
        assert!(mgr.is_empty());
    }

    #[test]
    fn test_session_manager_multiple_sessions() {
        let mgr = SessionManager::with_default_timeout();
        mgr.get_or_create("web:chat1", "web", "chat1");
        mgr.get_or_create("web:chat2", "web", "chat2");
        mgr.get_or_create("rpc:chat3", "rpc", "chat3");

        assert_eq!(mgr.len(), 3);
        assert!(mgr.contains("web:chat1"));
        assert!(mgr.contains("web:chat2"));
        assert!(mgr.contains("rpc:chat3"));

        mgr.remove("web:chat1");
        assert_eq!(mgr.len(), 2);
        assert!(!mgr.contains("web:chat1"));
    }

    #[test]
    fn test_session_store_disk_save_and_reload_multiple() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new_with_storage(dir.path());

        for i in 0..3 {
            let key = format!("multi:key{}", i);
            store.get_or_create(&key);
            store.set_summary(&key, &format!("Summary {}", i));
            store.save(&key).unwrap();
        }

        // Reload
        let store2 = SessionStore::new_with_storage(dir.path());
        assert_eq!(store2.len(), 3);
        for i in 0..3 {
            let key = format!("multi:key{}", i);
            assert!(store2.contains(&key));
            assert_eq!(store2.get_summary(&key), format!("Summary {}", i));
        }
    }
}

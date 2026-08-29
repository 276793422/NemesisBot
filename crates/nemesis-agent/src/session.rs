//! Session management and LLM-driven conversation summarization.
//!
//! Provides:
//! - `Session` / `SessionManager` for tracking active sessions
//! - `SessionStore` for persistent conversation history with disk storage
//! - Self-heal rebuild: a store entry missing past the 7-day TTL is
//!   reconstructed from the append-only chat_log on next access (see
//!   [`SessionStore::rebuild_from_chat_log`])
//! - `Summarizer` for LLM-driven multi-part session summarization
//! - Token estimation and force compression utilities

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Local};
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
    pub last_active: DateTime<Local>,
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
            last_active: Local::now(),
            last_channel: None,
            last_chat_id: None,
        }
    }

    /// Touch the session, updating last_active to now.
    pub fn touch(&mut self) {
        self.last_active = Local::now();
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
        self.sessions
            .insert(session_key.to_string(), session.clone());
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
        let now = Local::now();
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
    /// Number of leading messages (into the full instance history, including
    /// the system prompt at index 0) that `summary` covers. `None` on legacy
    /// session files written before this field existed — loaded as `None`,
    /// treated by the pipeline as "summary covers everything before the loaded
    /// tail". Added in S1; not yet consumed.
    #[serde(default)]
    pub summary_covers_up_to: Option<usize>,
    /// When this session was created.
    pub created: DateTime<Local>,
    /// When this session was last updated.
    pub updated: DateTime<Local>,
}

/// A single message in stored session history.
/// `PartialEq`/`Eq` derive: pure-data struct, added for Z1 fork tests
/// (prefix == source-prefix assertions); no behavioral change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredMessage {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub tool_calls: Vec<StoredToolCall>,
    pub tool_call_id: Option<String>,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reasoning_content: Option<String>,
    /// X1 (U3 projection prune): tool name for role="tool" turns (feeds the
    /// deterministic prune-marker recompute). See `ConversationTurn::tool_name`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_name: Option<String>,
    /// X1 (U3 projection prune): recorded model-facing override for oversized
    /// tool results (spill locator / guard nudges). Must round-trip the store
    /// or a reload would silently change what the model sees. See
    /// `ConversationTurn::tool_result_projection`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_result_projection: Option<String>,
}

/// Stored tool call info.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
            tool_calls: turn
                .tool_calls
                .iter()
                .map(|tc| StoredToolCall {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    arguments: tc.arguments.clone(),
                })
                .collect(),
            tool_call_id: turn.tool_call_id.clone(),
            timestamp: turn.timestamp.clone(),
            // Do NOT persist reasoning_content — Go's session does not store it,
            // and including it bloats session files with internal model thinking.
            reasoning_content: None,
            tool_name: turn.tool_name.clone(),
            tool_result_projection: turn.tool_result_projection.clone(),
        }
    }
}

impl From<StoredMessage> for ConversationTurn {
    fn from(msg: StoredMessage) -> Self {
        Self {
            role: msg.role,
            content: msg.content,
            tool_calls: msg
                .tool_calls
                .into_iter()
                .map(|tc| crate::types::ToolCallInfo {
                    id: tc.id,
                    name: tc.name,
                    arguments: tc.arguments,
                })
                .collect(),
            tool_call_id: msg.tool_call_id,
            timestamp: msg.timestamp,
            reasoning_content: msg.reasoning_content,
            tool_name: msg.tool_name,
            tool_result_projection: msg.tool_result_projection,
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

        // Z1 (Phase4-d): disk fallback on an in-memory miss. A session file
        // can legitimately appear AFTER this store was constructed — the
        // canonical case is `nemesisbot session fork`, which a CLI process
        // writes while the gateway keeps running. Without this fallback the
        // running gateway would materialize an EMPTY session for the forked
        // key and later `save()` would overwrite the fork file with that
        // near-empty history (the fork silently lost). Construction-time
        // `load_from_disk` still covers the restart path; this covers the
        // live path. Corrupt/absent file → same empty-session behavior as
        // before.
        if let Some(dir) = &self.storage_dir {
            let path = dir.join(format!("{}.json", sanitize_filename(key)));
            if let Ok(data) = std::fs::read_to_string(&path) {
                if let Ok(session) = serde_json::from_str::<StoredSession>(&data) {
                    self.sessions
                        .write()
                        .unwrap()
                        .insert(key.to_string(), session.clone());
                    return session;
                }
            }
        }

        // 2026-08-25 自愈重建（TTL 生命周期不对称修复）: memory AND disk both
        // missed. Before defaulting to an EMPTY session — the exact mechanism
        // behind 失忆会话 (UI shows chat_log history while the model context is
        // gone: `cleanup_old_sessions` evicts the store json after 7 idle days
        // but chat_log never expires) and behind permanent store↔log divergence
        // (continuing such a session used to rebuild context from scratch) —
        // try replaying the append-only chat_log. Persisted immediately, same
        // reasoning as the Z1 disk fallback above. In-memory-only stores (no
        // storage_dir) skip this: they are never TTL-evicted either.
        if self.storage_dir.is_some() {
            if let Some(session) = self.rebuild_from_chat_log(key) {
                self.sessions
                    .write()
                    .unwrap()
                    .insert(key.to_string(), session.clone());
                let _ = self.save(key);
                return session;
            }
        }

        let session = StoredSession {
            key: key.to_string(),
            messages: Vec::new(),
            summary: String::new(),
            summary_covers_up_to: None,
            created: Local::now(),
            updated: Local::now(),
        };
        self.sessions
            .write()
            .unwrap()
            .insert(key.to_string(), session.clone());
        session
    }

    /// 2026-08-25 自愈重建: rebuild a missing store entry by replaying the
    /// session's append-only chat_log (`session_logs/{key}.jsonl`).
    ///
    /// Why this exists: `cleanup_old_sessions` (startup + daily midnight,
    /// 7-day TTL) deletes ONLY the store json — chat_log never expires. Left
    /// as-is, any session idle past the TTL becomes "UI shows history, model
    /// remembers nothing" (失忆会话), and continuing it rebuilt context from
    /// zero while chat_log kept growing (permanent divergence; real cases
    /// found in production 2026-08-25). With this hook the chat_log is the
    /// durable source of truth and the store degrades to a rebuildable cache.
    ///
    /// Callers: only `get_or_create`, after the in-memory map AND the disk
    /// json both missed — a live store file always wins, so this can never
    /// clobber existing context (including Z1 fork files, which land via the
    /// disk fallback above).
    ///
    /// Fidelity limits (accepted, documented): chat_log only ever recorded
    /// user/assistant rows, so a rebuild carries no tool-call history and no
    /// summary; the agent instance re-injects the current system prompt on
    /// load (its `set_history` inserts the missing system row at [0]).
    /// Replayed rows keep their original timestamps; `updated` is stamped
    /// now so the rebuilt file survives a fresh TTL cycle. Long logs replay
    /// only their newest [`MAX_STORED_MESSAGES`] rows — the same bound the
    /// store itself enforces.
    fn rebuild_from_chat_log(&self, key: &str) -> Option<StoredSession> {
        // read_chat_log(limit, None) returns the NEWEST `limit` rows.
        let (rows, total, _, _) =
            crate::chat_log::read_chat_log(key, Self::MAX_STORED_MESSAGES, None);
        let messages = projected_messages_from_rows(&rows);
        if messages.is_empty() {
            return None; // no chat_log either → caller falls through to a fresh session
        }
        let created = messages
            .first()
            .and_then(|m| DateTime::parse_from_rfc3339(&m.timestamp).ok())
            .map(|dt| dt.with_timezone(&Local))
            .unwrap_or_else(Local::now);
        let session = StoredSession {
            key: key.to_string(),
            messages,
            summary: String::new(),
            summary_covers_up_to: None,
            created,
            updated: Local::now(),
        };
        info!(
            key = %key,
            replayed = session.messages.len(),
            log_total = total,
            "[SessionStore] self-heal: store json missing, rebuilt context from chat_log"
        );
        Some(session)
    }

    /// Get the conversation history for a session.
    pub fn get_history(&self, key: &str) -> Vec<StoredMessage> {
        self.sessions
            .read()
            .unwrap()
            .get(key)
            .map(|s| s.messages.clone())
            .unwrap_or_default()
    }

    /// Set the conversation history for a session.
    pub fn set_history(&self, key: &str, messages: Vec<StoredMessage>) {
        let capture_on = crate::capture_sink::CaptureSink::enabled();
        if let Some(session) = self.sessions.write().unwrap().get_mut(key) {
            // [capture] set_history is a wholesale replace (no merge). The old
            // maybe_summarize stale-snapshot overwrite race is now structurally
            // impossible (post inline-summarization refactor: the summarize path
            // no longer writes a truncated history back to the store), but the
            // capture is retained as a general write-timeline diagnostic.
            let (before_len, overwrite, first_role, last_role, incoming_hash) = if capture_on {
                let before = session.messages.len();
                let h = Self::hash_messages(&messages);
                (
                    before,
                    messages.len() < before,
                    messages.first().map(|m| m.role.clone()),
                    messages.last().map(|m| m.role.clone()),
                    h,
                )
            } else {
                (0usize, false, None, None, String::new())
            };
            session.messages = messages;
            Self::trim_to_limit(session);
            session.updated = Local::now();
            if capture_on {
                if let Some(sink) = crate::capture_sink::CaptureSink::global() {
                    sink.record_session_write(
                        key,
                        crate::capture_sink::SessionWriteCapture {
                            writer: "set_history".to_string(),
                            op: "set_history".to_string(),
                            before_len: Some(before_len),
                            after_len: Some(session.messages.len()),
                            first_role,
                            last_role,
                            messages_hash: incoming_hash,
                            overwrite_detected: overwrite,
                            ts: String::new(),
                        },
                    );
                }
            }
        }
    }

    /// Append a single message to a session's history.
    /// Mirrors Go's `agent.Sessions.AddMessage(sessionKey, role, content)`.
    pub fn add_message(&self, key: &str, role: &str, content: &str) {
        let capture_on = crate::capture_sink::CaptureSink::enabled();
        if let Some(session) = self.sessions.write().unwrap().get_mut(key) {
            session.messages.push(StoredMessage {
                role: role.to_string(),
                content: content.to_string(),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: chrono::Local::now().to_rfc3339(),
                reasoning_content: None,
                tool_name: None,
                tool_result_projection: None,
            });
            Self::trim_to_limit(session);
            session.updated = Local::now();
            // [capture] main-loop append. Pairing with set_history records
            // reveals main vs summarize write ordering on the timeline.
            if capture_on {
                let after_len = session.messages.len();
                let hash = Self::hash_messages(&session.messages);
                if let Some(sink) = crate::capture_sink::CaptureSink::global() {
                    sink.record_session_write(
                        key,
                        crate::capture_sink::SessionWriteCapture {
                            writer: "add_message".to_string(),
                            op: format!("add_message:{}", role),
                            before_len: Some(after_len.saturating_sub(1)),
                            after_len: Some(after_len),
                            first_role: session.messages.first().map(|m| m.role.clone()),
                            last_role: Some(role.to_string()),
                            messages_hash: hash,
                            overwrite_detected: false,
                            ts: String::new(),
                        },
                    );
                }
            }
        }
    }

/// Maximum number of messages kept in a stored session (disk + in-memory).
///
/// When exceeded, the oldest messages are dropped — but only from the
/// summary-covered prefix (`index < summary_covers_up_to`), and the index is
/// adjusted down by the number dropped so the cache stays coherent. The
/// verbatim tail (`index >= covers_up_to`, ≈ K_target ≪ this limit) is never
/// touched. See [`SessionStore::trim_to_limit`].
pub(crate) const MAX_STORED_MESSAGES: usize = 1000;

/// Drop oldest messages when a session exceeds [`MAX_STORED_MESSAGES`],
/// adjusting `summary_covers_up_to` so the cache index stays coherent.
///
/// Only messages the summary already covers (index < `covers_up_to`) are
/// eligible to drop — the verbatim tail is never touched. If there is no
/// summary yet (`covers_up_to` is `None` / 0), nothing is dropped: we'd rather
/// overshoot the soft limit than silently lose unsaved context. A summary gets
/// computed as the conversation grows, after which drops become safe.
///
/// This is the SOLE history-trimming mechanism post-refactor (the instance's
/// history is append-only). Called from `set_history` and `add_message`.
fn trim_to_limit(session: &mut StoredSession) {
    if session.messages.len() <= Self::MAX_STORED_MESSAGES {
        return;
    }
    let overflow = session.messages.len() - Self::MAX_STORED_MESSAGES;
    let c = session.summary_covers_up_to.unwrap_or(0);
    // Clamp to the covered prefix so we never drop verbatim-tail messages.
    let drop_n = overflow.min(c);
    if drop_n == 0 {
        return;
    }
    session.messages.drain(0..drop_n);
    session.summary_covers_up_to = Some(c - drop_n);
}

/// [capture] Stable hash of a message vec's (role, content) sequence —
/// lets the write timeline distinguish wholesale replacement from
/// in-place growth. Associated fn (no &self) so callers reuse it.
fn hash_messages(messages: &[StoredMessage]) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        for m in messages {
            m.role.hash(&mut hasher);
            m.content.hash(&mut hasher);
        }
        format!("{:016x}", hasher.finish())
    }

    /// Sanitize a client-provided session id for embedding in a session_key.
    /// Allows `[A-Za-z0-9_-]` only; everything else (incl. `:`, `/`, `\`)
    /// becomes `_`. Prevents path traversal / key injection from the WS client
    /// (the id flows into filenames: `sessions/{key}.json`, `session_logs/{key}.jsonl`).
    pub fn sanitize_session_id(sid: &str) -> String {
        sid.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    }

    /// Get the summary for a session.
    pub fn get_summary(&self, key: &str) -> String {
        self.sessions
            .read()
            .unwrap()
            .get(key)
            .map(|s| s.summary.clone())
            .unwrap_or_default()
    }

    /// Set the summary for a session.
    pub fn set_summary(&self, key: &str, summary: &str) {
        if let Some(session) = self.sessions.write().unwrap().get_mut(key) {
            session.summary = summary.to_string();
            session.updated = Local::now();
        }
    }

    /// Get the `summary_covers_up_to` index for a session.
    ///
    /// Returns `None` when unset (including legacy session files that predate
    /// the field — serde loads them with the default `None`).
    pub fn get_summary_covers_up_to(&self, key: &str) -> Option<usize> {
        self.sessions
            .read()
            .unwrap()
            .get(key)
            .and_then(|s| s.summary_covers_up_to)
    }

    /// Set the `summary_covers_up_to` index for a session. Pass `None` to clear.
    pub fn set_summary_covers_up_to(&self, key: &str, covers: Option<usize>) {
        if let Some(session) = self.sessions.write().unwrap().get_mut(key) {
            session.summary_covers_up_to = covers;
            session.updated = Local::now();
        }
    }

    /// Truncate the history, keeping only the last N messages.
    pub fn truncate_history(&self, key: &str, keep_last: usize) {
        if let Some(session) = self.sessions.write().unwrap().get_mut(key) {
            if session.messages.len() > keep_last {
                let start = session.messages.len() - keep_last;
                session.messages = session.messages.split_off(start);
                session.updated = Local::now();
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
        if filename == "." || filename == ".." || filename.contains('/') || filename.contains('\\')
        {
            return Err("invalid session key for filename".into());
        }

        let session_path = storage_dir.join(format!("{}.json", filename));

        // Atomic write: write to temp file, then rename. The counter suffix
        // prevents same-key concurrent saves (main loop + summarize task) from
        // trampling each other's temp file — the root cause of the save race.
        static SAVE_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SAVE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp_name = format!("session-{}-{}-{}.tmp", filename, std::process::id(), n);
        let tmp_path = storage_dir.join(&tmp_name);

        std::fs::write(&tmp_path, &data).map_err(|e| format!("write temp error: {}", e))?;

        std::fs::rename(&tmp_path, &session_path).map_err(|e| {
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
                    self.sessions
                        .write()
                        .unwrap()
                        .insert(session.key.clone(), session);
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

    /// Whether a session FILE for `key` exists on disk, regardless of the
    /// in-memory cache. Z1 (Phase4-d): fork key-uniqueness checks both this
    /// and `contains`, so a store constructed before a file appeared (the
    /// running gateway) still refuses to collide with it.
    pub fn file_exists(&self, key: &str) -> bool {
        match &self.storage_dir {
            Some(dir) => dir.join(format!("{}.json", sanitize_filename(key))).exists(),
            None => false,
        }
    }

    /// Remove a session from memory (does not delete from disk).
    pub fn remove(&self, key: &str) -> Option<StoredSession> {
        self.sessions.write().unwrap().remove(key)
    }

    /// Delete a session completely: in-memory cache + on-disk
    /// `sessions/*.json` (LLM context) + `session_logs/*.jsonl` (user-facing
    /// history). Used by Dashboard multi-session management. Returns whether
    /// the session was present in memory. Best-effort on disk (errors logged,
    /// not fatal — a missing file is not an error).
    pub fn delete_session(&self, key: &str) -> bool {
        let existed = self.sessions.write().unwrap().remove(key).is_some();

        // 1. sessions/{sanitize}.json (LLM context store)
        if let Some(dir) = &self.storage_dir {
            let path = dir.join(format!("{}.json", sanitize_filename(key)));
            if let Err(e) = std::fs::remove_file(&path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    warn!(
                        file = %path.display(),
                        error = %e,
                        "[SessionStore] delete_session: failed to remove json"
                    );
                }
            }
        }

        // 2. session_logs/{safe}.jsonl (user-facing chat history)
        crate::chat_log::delete_chat_log(key);

        existed
    }

    /// Clear a session's messages + summary but keep the key (conversation
    /// stays usable, history emptied). Used by "clear" in session management.
    ///
    /// FIX (2026-08-25 两存储分叉摸底): the on-disk file MUST go too. The
    /// old version only cleared the in-memory entry, so `sessions/*.json`
    /// still carried the full history — after a gateway restart the store
    /// reloaded it and the LLM context "came back to life" while chat_log
    /// had already been truncated by the handler, diverging the two stores
    /// in the worst direction (model remembers everything the user just
    /// cleared; UI shows nothing). Removing the file is safe: the next
    /// `get_or_create` lazily rebuilds an empty session. Known boundary
    /// (shared with `delete_session`): a turn already in flight for this
    /// key may save() the old history back at its end — management ops vs
    /// concurrent turns is a pre-existing race, unchanged here.
    pub fn clear_session(&self, key: &str) {
        // Drop the in-memory entry (next get_or_create rebuilds an empty one).
        self.sessions.write().unwrap().remove(key);
        if let Some(dir) = &self.storage_dir {
            let path = dir.join(format!("{}.json", sanitize_filename(key)));
            if let Err(e) = std::fs::remove_file(&path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    warn!(
                        file = %path.display(),
                        error = %e,
                        "[SessionStore] clear_session: failed to remove json"
                    );
                }
            }
        }
    }

    /// Migrate the legacy single-session main (`agent:main:main`) to the
    /// multi-session format (`agent:main:session:legacy`). Idempotent +
    /// best-effort: only renames when the legacy target is absent, and a
    /// rename failure just warns (original files stay, data not lost).
    /// Called once at gateway startup, before SessionStore loads from disk.
    pub fn migrate_legacy_main(storage_dir: &Path) {
        use nemesis_path::default_path_manager;
        let logs_dir = default_path_manager().sessions_log_dir();

        // session_logs/agent_main_main.jsonl → agent_main_session_legacy.jsonl
        let main_log = logs_dir.join("agent_main_main.jsonl");
        let legacy_log = logs_dir.join("agent_main_session_legacy.jsonl");
        if main_log.exists() && !legacy_log.exists() {
            match std::fs::rename(&main_log, &legacy_log) {
                Ok(_) => info!(
                    "[migrate] session_logs: agent_main_main.jsonl → agent_main_session_legacy.jsonl"
                ),
                Err(e) => warn!("[migrate] failed to rename session log: {}", e),
            }
        }

        // sessions/agent_main_main.json → agent_main_session_legacy.json (改 key 字段)
        let main_json = storage_dir.join("agent_main_main.json");
        let legacy_json = storage_dir.join("agent_main_session_legacy.json");
        if main_json.exists() && !legacy_json.exists() {
            match std::fs::read_to_string(&main_json) {
                Ok(data) => {
                    if let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&data) {
                        if v.get("key").and_then(|k| k.as_str()) == Some("agent:main:main") {
                            v["key"] =
                                serde_json::Value::String("agent:main:session:legacy".to_string());
                        }
                        if let Ok(out) = serde_json::to_string_pretty(&v) {
                            if std::fs::write(&legacy_json, out).is_ok() {
                                let _ = std::fs::remove_file(&main_json);
                                info!(
                                    "[migrate] sessions: agent_main_main.json → agent_main_session_legacy.json"
                                );
                            }
                        }
                    }
                }
                Err(e) => warn!("[migrate] failed to read main session json: {}", e),
            }
        }
    }

    /// Delete sessions whose `updated` timestamp is older than `max_age_days` days.
    ///
    /// Walks the on-disk directory (when storage_dir is set), parses each JSON file's
    /// `updated` field, deletes files older than the threshold, and removes the
    /// corresponding in-memory entries. Returns the number of sessions deleted.
    ///
    /// Sessions with corrupt/unparseable JSON or missing `updated` are left alone
    /// (matches load_from_disk's tolerant stance). In-memory-only stores (no
    /// storage_dir) return 0 without doing anything.
    ///
    /// Used by cluster_agent and the main AgentLoop at startup and via daily cron
    /// to bound disk usage from accumulated peer_chat history files.
    ///
    /// Lifecycle note (2026-08-25): this deletes ONLY the store json — the
    /// append-only chat_log (`session_logs/*.jsonl`) never expires, so an
    /// evicted session keeps its UI history while model context is gone.
    /// That asymmetry used to produce 失忆会话 and permanent store↔log
    /// divergence; it is now healed on next access by
    /// [`SessionStore::rebuild_from_chat_log`] (store = rebuildable cache,
    /// chat_log = source of truth).
    pub fn cleanup_old_sessions(&self, max_age_days: u64) -> usize {
        self.cleanup_old_sessions_detailed(max_age_days).len()
    }

    /// 同 [`Self::cleanup_old_sessions`]，但返回被删会话的**原始 session key**
    /// （供 CC `SessionEnd` 钩子逐会话触发——2026-08-29 T3）。
    pub fn cleanup_old_sessions_detailed(&self, max_age_days: u64) -> Vec<String> {
        let storage_dir = match &self.storage_dir {
            Some(d) => d.clone(),
            None => return Vec::new(),
        };

        let entries = match std::fs::read_dir(&storage_dir) {
            Ok(e) => e,
            Err(e) => {
                warn!(
                    dir = %storage_dir.display(),
                    error = %e,
                    "[SessionStore] cleanup: failed to read storage dir"
                );
                return Vec::new();
            }
        };

        let now = Local::now();
        let mut deleted = 0usize;
        let mut keys_to_drop: Vec<String> = Vec::new();

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }

            // Read the file content to extract `updated` timestamp.
            let data = match std::fs::read_to_string(&path) {
                Ok(d) => d,
                Err(_) => continue,
            };

            let parsed: serde_json::Value = match serde_json::from_str(&data) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let updated_str = match parsed.get("updated").and_then(|v| v.as_str()) {
                Some(s) => s,
                None => continue,
            };

            let updated = match chrono::DateTime::parse_from_rfc3339(updated_str) {
                Ok(dt) => dt.with_timezone(&Local),
                Err(_) => continue,
            };

            let age_days = now.signed_duration_since(updated).num_days();
            if age_days > max_age_days as i64 {
                // Delete from disk.
                if let Err(e) = std::fs::remove_file(&path) {
                    warn!(
                        file = %path.display(),
                        error = %e,
                        "[SessionStore] cleanup: failed to delete file"
                    );
                    continue;
                }

                // Record the session key (from parsed JSON) for in-memory removal.
                if let Some(key) = parsed.get("key").and_then(|v| v.as_str()) {
                    keys_to_drop.push(key.to_string());
                }
                deleted += 1;
            }
        }

        // Drop in-memory cache entries.
        if !keys_to_drop.is_empty() {
            let mut sessions = self.sessions.write().unwrap();
            for key in &keys_to_drop {
                sessions.remove(key);
            }
        }

        if deleted > 0 {
            info!(
                deleted,
                remaining = self.len(),
                max_age_days,
                "[SessionStore] cleanup complete"
            );
        }

        keys_to_drop
    }
}

/// Sanitize a session key for use as a filename.
/// Replaces ':' (volume separator on Windows) with '_'.
fn sanitize_filename(key: &str) -> String {
    key.replace(':', "_").replace('\\', "_").replace('/', "_")
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

/// X1 (U3 projection prune): estimate over the MODEL-FACING projection.
/// Compaction pressure must track what the provider actually receives —
/// since the size gates moved to the projection, history keeps full
/// originals and the raw estimate would over-trigger summarization for a
/// 70KB tool result the model only ever sees as a 2KB spill locator.
pub fn estimate_tokens_for_turns_projected(turns: &[ConversationTurn]) -> usize {
    turns
        .iter()
        .map(|t| estimate_tokens(&t.model_facing_content()))
        .sum()
}

/// Estimate tokens for stored messages.
#[cfg(test)]
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
    pub fn should_summarize(&self, history: &[ConversationTurn], context_window: usize) -> bool {
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
    pub fn summarize_session(&self, session_key: &str, history: &[ConversationTurn]) -> String {
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
            // 当前为死代码（仅测试使用）。若未来启用，除 tool 对问题外还必须
            // 同步采用 loop.rs 2026-08-25 的失败传播语义（summarize_batch/
            // multipart 返回 Option，任一部分失败绝不写 summary/推进 covers）——
            // 本路径 summarize_batch 失败仍返回空串，会复刻"摘要静默失败"事故。
            let truncated: Vec<StoredMessage> = history[history.len().saturating_sub(4)..]
                .iter()
                .map(|t| t.into())
                .collect();
            self.session_store.set_history(session_key, truncated);
            self.session_store.set_summary(session_key, &final_summary);

            if let Err(e) = self.session_store.save(session_key) {
                warn!(
                    "[SessionStore] Failed to save session after summarization: {}",
                    e
                );
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
        let mut response = tokio_block_on(async {
            self.provider
                .chat(&self.model, messages, summarize_opts, vec![])
                .await
        });
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
            raw_request_body: raw_req,
            raw_response_body: raw_resp,
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
        let mut response = tokio_block_on(async {
            self.provider
                .chat(&self.model, messages, summarize_opts, vec![])
                .await
        });
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
            raw_request_body: raw_req,
            raw_response_body: raw_resp,
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
        timestamp: chrono::Local::now().to_rfc3339(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
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

/// 2026-08-25 fork 第三轮: shared "jsonl rows → StoredMessage" mapping for
/// EVERY path that materializes store content FROM the chat_log (the
/// self-heal rebuild AND `session_fork::fork_session`'s new-session store
/// side). One mapping, two consumers — the fork's store must be
/// byte-identical to what a later self-heal rebuild of the fork's jsonl
/// would produce, or the fork's model context would silently change the
/// day its store json ages out of the 7-day TTL.
///
/// Semantics (identical to the original inline rebuild code): rows pass the
/// single-source-of-truth predicate [`crate::chat_log::is_projected_chat_row`]
/// (empty-content assistant intermediates skipped); tool_calls stay empty
/// (chat_log never recorded them); timestamps keep their jsonl values.
pub(crate) fn projected_messages_from_rows(rows: &[serde_json::Value]) -> Vec<StoredMessage> {
    rows.iter()
        .filter_map(|v| {
            let role = v.get("role").and_then(|r| r.as_str()).unwrap_or("");
            let content = v.get("content").and_then(|c| c.as_str()).unwrap_or("");
            if !crate::chat_log::is_projected_chat_row(role, content) {
                return None;
            }
            Some(StoredMessage {
                role: role.to_string(),
                content: content.to_string(),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: v
                    .get("timestamp")
                    .and_then(|t| t.as_str())
                    .unwrap_or_default()
                    .to_string(),
                reasoning_content: None,
                tool_name: None,
                tool_result_projection: None,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

// S9 (quality-hardening goal 冲刺 S9): 独立测试文件挂载（声明式，无内联测试）。
#[cfg(test)]
mod s9_tests;

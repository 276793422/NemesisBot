//! Session management and LLM-driven conversation summarization.
//!
//! Provides:
//! - `Session` / `SessionManager` for tracking active sessions
//! - `SessionStore` for persistent conversation history with disk storage
//! - Self-heal rebuild: a store entry missing past the 7-day TTL is
//!   reconstructed from the append-only chat_log on next access (see
//!   [`SessionStore::rebuild_from_chat_log`])
//! - Token estimation and force compression utilities
//!
//! （对话摘要的 LLM 驱动实现已收敛到 `loop::compact` 的六节结构化摘要——
//! 旧 `Summarizer` 批式路径 2026-09-28 随真源归一退役。）

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Local};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::types::ConversationTurn;

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
                // BUG-20261102-326：保号比较。`num_seconds() as u64` 会把负
                // 差值（时钟回拨 / 跨机迁移恢复时 last_active 在未来）回绕成
                // 巨值恒真——刚活跃的会话被立即误清。与 BUG-20261024-11
                // （裸加法回绕）同族：有符号差值严禁塞进无符号比较。
                elapsed.num_seconds() > timeout.as_secs() as i64
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
    /// L6++：项目归属缓存镜像（真相源 = chat_log sidecar meta；None =
    /// 对话组/未绑定）。旧文件无此键 → default None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// L6++：canonical 项目目录（与 project_id 同批写入的缓存镜像）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
    /// 会话资产区绑定缓存镜像（2026-11-12；真相源 = chat_log sidecar meta
    /// 的 `home_agent_id`）。None = 未绑定（存量 lenient，资产落全局）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_agent_id: Option<String>,
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
    /// T5/T6（多模态）：图片路径引用（与 `ConversationTurn.image_refs` 同源，
    /// store round-trip 必须保留否则重载后图片丢失）。旧文件无此键 → default 空。
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub image_refs: Vec<String>,
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
            image_refs: turn.image_refs.clone(),
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
            image_refs: msg.image_refs,
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
            if let Ok(data) = std::fs::read_to_string(&path)
                && let Ok(session) = serde_json::from_str::<StoredSession>(&data)
            {
                self.sessions
                    .write()
                    .unwrap()
                    .insert(key.to_string(), session.clone());
                return session;
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
        if self.storage_dir.is_some()
            && let Some(session) = self.rebuild_from_chat_log(key)
        {
            self.sessions
                .write()
                .unwrap()
                .insert(key.to_string(), session.clone());
            // BUG-20261028-95：自愈重建的立即持久化失败不得静默——落盘失败
            // 时下次 get_or_create 会再重建一遍，故障（磁盘满/权限）完全
            // 不可见。对齐调用方 save 的 warn 风格留可见信号。
            if let Err(e) = self.save(key) {
                tracing::warn!("[session-store] Failed to persist rebuilt session {key}: {e}");
            }
            return session;
        }

        let session = StoredSession {
            key: key.to_string(),
            messages: Vec::new(),
            summary: String::new(),
            summary_covers_up_to: None,
            created: Local::now(),
            updated: Local::now(),
            project_id: None,
            project_path: None,
            // 新会话默认未绑定；分配点 = sessions.create（写 meta），
            // 这里只是镜像初值（真相源在 sidecar meta）。
            home_agent_id: None,
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
        // L6++：项目归属真相源在 sidecar meta——重建顺带水合缓存镜像，保证
        // store json 与 meta 不漂移（否则 TTL 重建后归属镜像悄悄丢失）。
        let meta = crate::chat_log::read_session_meta_full(key);
        let session = StoredSession {
            key: key.to_string(),
            messages,
            summary: String::new(),
            summary_covers_up_to: None,
            created,
            updated: Local::now(),
            project_id: meta.as_ref().and_then(|m| m.project_id.clone()),
            project_path: meta.as_ref().and_then(|m| m.project_path.clone()),
            // L6++ 同款：重建顺带水合 home 绑定镜像（真相源在 sidecar meta，
            // 防止 TTL 重建后镜像悄悄丢失）。
            home_agent_id: meta.as_ref().and_then(|m| m.home_agent_id.clone()),
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
            if capture_on && let Some(sink) = crate::capture_sink::CaptureSink::global() {
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
                image_refs: Vec::new(),
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
    ///
    /// D-6（复核 2026-09-16）：与 `nemesis_utils::sanitize::sanitize_path_segment`
    /// （SAN-05 真相源）是**两道串联防线**而非重复实现：本 fn 先消毒客户端
    /// 注入的 id 段（防 session_key 结构注入——key 形如 `chan:{id}`，id 里
    /// 混入 `:` 会伪造通道前缀），key 整体落盘前再过白名单真相源。本输出
    /// 只含 `[A-Za-z0-9_-]`，过真相源白名单是恒等映射——两道串联后字节
    /// 稳定，存量文件名不受影响。
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

    /// L6++：设置项目归属缓存镜像（真相源在 chat_log sidecar meta；本字段
    /// 只为免 sidecar 读）。内存变更不落盘——调用方 save（对齐
    /// set_summary_covers_up_to 的「内存变更 + 调用方落盘」契约）。Pass
    /// `None` to clear.
    pub fn set_project(&self, key: &str, project_id: Option<String>, project_path: Option<String>) {
        if let Some(session) = self.sessions.write().unwrap().get_mut(key) {
            session.project_id = project_id;
            session.project_path = project_path;
            session.updated = Local::now();
        }
    }

    /// L6++：读取项目归属缓存镜像（None, None = 对话组/未绑定/会话不存在）。
    pub fn get_project(&self, key: &str) -> (Option<String>, Option<String>) {
        self.sessions
            .read()
            .unwrap()
            .get(key)
            .map(|s| (s.project_id.clone(), s.project_path.clone()))
            .unwrap_or((None, None))
    }

    /// 会话资产区绑定缓存镜像（2026-11-12；真相源在 chat_log sidecar meta）。
    /// 内存变更不落盘——调用方 save（同 set_project 契约）。
    pub fn set_home_agent(&self, key: &str, home_agent_id: Option<String>) {
        if let Some(session) = self.sessions.write().unwrap().get_mut(key) {
            session.home_agent_id = home_agent_id;
            session.updated = Local::now();
        }
    }

    /// 读取会话资产区绑定缓存镜像（None = 未绑定/会话不存在）。
    pub fn get_home_agent(&self, key: &str) -> Option<String> {
        self.sessions
            .read()
            .unwrap()
            .get(key)
            .and_then(|s| s.home_agent_id.clone())
    }

    /// Truncate the history, keeping only the last N messages.
    pub fn truncate_history(&self, key: &str, keep_last: usize) {
        if let Some(session) = self.sessions.write().unwrap().get_mut(key)
            && session.messages.len() > keep_last
        {
            let dropped = session.messages.len() - keep_last;
            session.messages = session.messages.split_off(dropped);
            // 从头部丢弃 → 摘要覆盖索引同步前移（与 trim_to_limit 同一不变量）；
            // 否则 covers 残留旧值指向新历史起点之外，build_messages 会把
            // 从未被摘要的消息当成已覆盖丢弃。
            if let Some(c) = session.summary_covers_up_to {
                session.summary_covers_up_to = Some(c.saturating_sub(dropped));
            }
            session.updated = Local::now();
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

        // 落盘委托 REL-002 权威 helper（唯一 tmp + fsync + 失败清理；BUG-51
        // 同族迁移）。旧手搓版无 fsync：rename 落盘后进程崩溃可能留下空/半截
        // JSON；且错误文案、tmp 命名与权威 helper 双源分裂。helper 内部唯一
        // tmp 同样防同键并发 save（主循环 + summarize 任务）互相踩 tmp。
        nemesis_utils::file::write_file_atomic(
            &session_path.to_string_lossy(),
            data.as_bytes(),
            0o644,
        )
        .map_err(|e| format!("save session {}: {}", session_path.display(), e))
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
            Some(dir) => dir
                .join(format!("{}.json", sanitize_filename(key)))
                .exists(),
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
        let existed = self.detach_session(key);
        // session_logs/{safe}.jsonl (user-facing chat history)
        crate::chat_log::delete_chat_log(key);
        existed
    }

    /// Detach a session's LLM context without touching the user-facing
    /// history: in-memory cache + on-disk `sessions/*.json` only. Used by the
    /// session trash path (2026-10-30) — the chat_log three-file set moves to
    /// `logs/trash/sessions/` for restore, while the context store does NOT
    /// (it is derivable from the jsonl via the self-heal rebuild path, so
    /// restoring doesn't need it). Returns whether the session was present
    /// in memory (same contract as [`Self::delete_session`]).
    pub fn detach_session(&self, key: &str) -> bool {
        let existed = self.sessions.write().unwrap().remove(key).is_some();

        // sessions/{sanitize}.json (LLM context store)
        if let Some(dir) = &self.storage_dir {
            let path = dir.join(format!("{}.json", sanitize_filename(key)));
            if let Err(e) = std::fs::remove_file(&path)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                warn!(
                    file = %path.display(),
                    error = %e,
                    "[SessionStore] detach_session: failed to remove json"
                );
            }
        }
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
            if let Err(e) = std::fs::remove_file(&path)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                warn!(
                    file = %path.display(),
                    error = %e,
                    "[SessionStore] clear_session: failed to remove json"
                );
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
                        if let Ok(out) = serde_json::to_string_pretty(&v)
                            // 原子写（同文件 save 路径 756 行同源）：裸写半截
                            // 的 legacy_json 会 satisfy exists() 闸——迁移入口
                            // 条件 `main_json.exists() && !legacy_json.exists()`
                            // 永不重真，迁移被永久跳过（旧 key 会话失联）。
                            && nemesis_utils::file::write_file_atomic(
                                &legacy_json.to_string_lossy(),
                                out.as_bytes(),
                                0o644,
                            )
                            .is_ok()
                        {
                            let _ = std::fs::remove_file(&main_json);
                            info!(
                                "[migrate] sessions: agent_main_main.json → agent_main_session_legacy.json"
                            );
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
    /// （供方言 `SessionEnd` 钩子逐会话触发——2026-08-29 T3）。
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
///
/// SAN-05：薄包装委托仓内单一真相源（原实现
/// `replace([':', '\\', '/'], "_")` 对运行时键族映射一致；`..`/超长输入
/// 由白名单公共函数兜住）。
fn sanitize_filename(key: &str) -> String {
    nemesis_utils::sanitize::sanitize_path_segment(key)
}

// ---------------------------------------------------------------------------
// Token estimation
// ---------------------------------------------------------------------------

/// Estimate the token count for a string.
///
/// 五桶折算 + 密集形态补收（实报锚定方案 Layer 2 统一保守版，2026-11-09；
/// 验证证据：`docs/INFO/2026-11-09_估算公式V1离线对照验证.md`——纯折算差分
/// 中位 5.9%，折算+补收 9.9% 有向偏保守；判定面/观测面/展示面同源消费本
/// 函数）。
///
/// 结构：先把文本折算成「加权字符数」再除 4（ASCII 4 chars/token 基准，
/// 常见 CJK 1 token/字，罕见区按保守加权），再对机器形态密集串（长编码串/
/// 纯数字组/JSON 结构符）补收差额。区间表描述的是**文字形态**（所有现代
/// 分词器的共同行为），密集补收描述的是**编码本身的密度属性**（base64
/// 6bit/字符、JSON 结构符独立成 token）——两者都不绑定任何一家的词表，
/// 分词器换代不失效。
///
/// # 历史（2026-09-27 M6 注记的正式反转）
///
/// 旧实现 chars×2/5（0.4 token/char 英文经验值）对 CJK 低估 ~2.5 倍、对
/// 机器形态文本低估 20-30%（`compact.rs` 在案实测 1.88 倍）。当时「刻意
/// 不改估算器」的顾虑是调系数会全局移动压缩阈值并牵连 prompt cache 预热
/// 判定；2026-11-09 实报锚定架构落地后，本函数只出现在「实报缺席」与
/// 「增量窗口」两个补位场景（每轮被 provider usage 清零，永不作为真相源），
/// 阈值漂移顾虑由锚点机制化解，本替换为该决策的正式反转。
pub fn estimate_tokens(text: &str) -> usize {
    // 加权字符数：ASCII 快路（len 即加权，零分类开销覆盖大部分纯文本）；
    // 非 ASCII 逐字符按五桶加权。
    let weighted: usize = if text.is_ascii() {
        text.len()
    } else {
        text.chars()
            .map(|ch| 1 + cjk_bucket_extra_weight(ch as u32))
            .sum()
    };
    weighted.div_ceil(4) + dense_form_extra_tokens(text)
}

/// 五桶加权增量（桶权重 - 基础权重 1）。区间表移植自 openclaw
/// `cjk-chars`（直系前身的最终形态）；桶序即匹配序（先命中先得，桶间
/// 区间互不重叠）。
fn cjk_bucket_extra_weight(cp: u32) -> usize {
    const BUCKET1: [(u32, u32); 5] = [
        (0x00B7, 0x00B7), // 间隔号
        (0x3000, 0x319F), // CJK 标点/假名
        (0x4E00, 0x9FA5), // 基本汉字
        (0xAC00, 0xD7AF), // 谚文
        (0xFF01, 0xFF60), // 全角形式
    ];
    const BUCKET2: [(u32, u32); 9] = [
        (0x1100, 0x11FF), // 谚文字母
        (0x2E80, 0x2FFF), // 部首/字形描述
        (0x31A0, 0x4DFF), // 声调扩展/扩展 A/注音/番文
        (0x9FA6, 0x9FFF), // 汉字扩展尾
        (0xA000, 0xA4FF), // 彝文
        (0xA700, 0xA707), // 声调字母
        (0xA960, 0xA97F), // 谚文扩展 A
        (0xD7B0, 0xD7FF), // 谚文扩展 B
        (0xF900, 0xFAFF), // 兼容表意
    ];
    const BUCKET3: [(u32, u32); 9] = [
        (0x02C7, 0x02C7), // 卡罗号
        (0x02C9, 0x02CB), // 长音/双 GRAVE
        (0x02D9, 0x02D9), // 上点
        (0x02EA, 0x02EB), // 声调（闽语）
        (0x0305, 0x0305), // 上划线
        (0x0323, 0x0323), // 下点
        (0xFE10, 0xFE4F), // 竖排标点
        (0xFF61, 0xFFDC), // 半角片假名/半角谚文
        (0xFFE0, 0xFFE6), // 半角全角符号
    ];
    const BUCKET4: [(u32, u32); 1] = [(0x1D360, 0x1D371)]; // 计数杆筹码
    const BUCKET5: [(u32, u32); 7] = [
        (0x16FE0, 0x16FFF), // 周易/太玄占
        (0x17000, 0x18D0F), // 西夏文
        (0x1AFF0, 0x1AFFF), // 注音扩展
        (0x1B000, 0x1B2FF), // 假名补充/川盐符号
        (0x1F200, 0x1F2FF), // 封闭 CJK 象形
        (0x20000, 0x2FA1F), // CJK 扩展 B-F
        (0x30000, 0x3347F), // CJK 扩展 G-H
    ];
    // 桶权重：常见 CJK ×4（1.0 token/字）、罕见 BMP ×12（3.0）、twoToken
    // ×8（2.0）、补充区 ×12（3.0）、罕见补充 ×16（4.0）。
    const EXTRA: [usize; 5] = [3, 11, 7, 11, 15];
    for (i, table) in [
        &BUCKET1[..],
        &BUCKET2[..],
        &BUCKET3[..],
        &BUCKET4[..],
        &BUCKET5[..],
    ]
    .into_iter()
    .enumerate()
    {
        for &(lo, hi) in table {
            if (lo..=hi).contains(&cp) {
                return EXTRA[i];
            }
        }
    }
    0
}

/// 密集形态补收（差额制——只收超出 `chars/4` 基线的部分，与折算不重复
/// 计费）。三类（单遍字节扫描，无 regex 依赖；密集形态字符全为 ASCII，
/// UTF-8 多字节序列的各字节 ≥ 0x80 不会误命中）：
/// - 长编码串 `[A-Za-z0-9+/_=-]{16,}`：切段逐段 ceil(len/4)（字母连段/
///   1-3 位数字组/单字符），补收 = 分段和 − 串长/4 基线；
/// - 短串内的 ≥4 位纯数字组：ceil(len/3) − len/4 基线（数字约 3 字符/
///   token；≥4 位门槛防止 prose 里的短数字被误收）；
/// - JSON 结构符 `{ } [ ] : ,`：每个 +3/4 token（实分词各占 1 token，
///   基线只给 1/4）。
fn dense_form_extra_tokens(text: &str) -> usize {
    const B64_MIN_RUN: usize = 16;
    const DIGIT_MIN_RUN: usize = 4;

    let b = text.as_bytes();
    let n = b.len();
    let is_b64ish = |c: u8| {
        c.is_ascii_alphanumeric() || c == b'+' || c == b'/' || c == b'_' || c == b'-' || c == b'='
    };
    let is_structural = |c: u8| matches!(c, b'{' | b'}' | b'[' | b']' | b':' | b',');

    let mut extra = 0usize;
    let mut struct_quarters = 0usize;
    let mut i = 0usize;
    while i < n {
        let c = b[i];
        if c >= 0x80 {
            // UTF-8 多字节序列：逐字节跳过（任一字节都不满足 ASCII 谓词）。
            i += 1;
            continue;
        }
        if is_b64ish(c) {
            let start = i;
            while i < n && b[i] < 0x80 && is_b64ish(b[i]) {
                i += 1;
            }
            let run_len = i - start;
            if run_len >= B64_MIN_RUN {
                // 长编码串：切段（字母连段 / 1-3 位数字组 / 单字符）逐段
                // ceil(/4)，补收与基线的差额。
                let mut j = start;
                let mut dense = 0usize;
                while j < i {
                    let cj = b[j];
                    if cj.is_ascii_alphabetic() {
                        let s = j;
                        while j < i && b[j].is_ascii_alphabetic() {
                            j += 1;
                        }
                        dense += (j - s).div_ceil(4);
                    } else if cj.is_ascii_digit() {
                        let s = j;
                        while j < i && b[j].is_ascii_digit() && j - s < 3 {
                            j += 1;
                        }
                        dense += 1; // ceil(≤3 / 4) = 1
                    } else {
                        j += 1;
                        dense += 1; // 单字符（+/=/- 等）独立成 token
                    }
                }
                extra += dense.saturating_sub(run_len.div_ceil(4));
            } else {
                // 短串：只收内嵌 ≥4 位纯数字组的差额（prose 连字符词/
                // 短标识符不误收）。
                let mut j = start;
                while j < i {
                    if b[j].is_ascii_digit() {
                        let s = j;
                        while j < i && b[j].is_ascii_digit() {
                            j += 1;
                        }
                        let d = j - s;
                        if d >= DIGIT_MIN_RUN {
                            extra += d.div_ceil(3).saturating_sub(d.div_ceil(4));
                        }
                    } else {
                        j += 1;
                    }
                }
            }
        } else if is_structural(c) {
            struct_quarters += 3; // 实分词 1 token − 基线 1/4 = 3/4
            i += 1;
        } else {
            i += 1;
        }
    }
    extra + struct_quarters / 4
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
        image_refs: Vec::new(),
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
///
/// 委托仓内单一真相源 `nemesis_types::is_internal_channel`
/// （BUG-20261025-46 收编，本文件原为手抄 `matches!` 副本）。
pub fn is_internal_channel(channel: &str) -> bool {
    nemesis_types::is_internal_channel(channel)
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
/// F-E（2026-09-04 四轮盲审）：行内 `"images": [路径...]` 数组（T6 起写入）
/// 必须还原进 `image_refs`——硬编码 `Vec::new()` 会让 self-heal 重建和
/// fork_session 的图片引用凭空消失（store 里有图、jsonl 里也有图、重建后
/// 没图；vision=no 投影/重水合都失去依据）。
pub(crate) fn projected_messages_from_rows(rows: &[serde_json::Value]) -> Vec<StoredMessage> {
    rows.iter()
        .filter_map(|v| {
            let role = v.get("role").and_then(|r| r.as_str()).unwrap_or("");
            let content = v.get("content").and_then(|c| c.as_str()).unwrap_or("");
            if !crate::chat_log::is_projected_chat_row(role, content) {
                return None;
            }
            let image_refs: Vec<String> = v
                .get("images")
                .and_then(|i| i.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|p| p.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
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
                image_refs,
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

// 覆盖率补充批次：自愈重建 / legacy 迁移双 store / cleanup 失败面 /
// delete·clear 删除失败 warn / capture sink 记账 / force_compress 两形态。
#[cfg(test)]
mod cov_tests;

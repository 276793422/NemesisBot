//! Session management with persistence, expiry, and multi-session operations.

use chrono::{DateTime, Local};
use dashmap::DashMap;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// A tool call within a message (mirrors providers::ToolCall for session storage).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub call_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<FunctionCall>,
}

/// A function call within a tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

/// A chat message in a session.
///
/// Supports full message content including tool calls and tool call IDs,
/// matching the Go `providers.Message` structure used by the session manager.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tool_calls: Vec<ToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

/// Session data (Go-style session keyed by "channel:chatID").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSession {
    pub key: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub created: String,
    pub updated: String,
}

/// Connection session (WebSocket session tracking).
#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub channel: String,
    pub sender_id: String,
    pub chat_id: String,
    pub created_at: DateTime<Local>,
    pub last_active: DateTime<Local>,
    pub metadata: HashMap<String, String>,
}

/// Session manager combining both chat sessions and connection sessions.
pub struct SessionMgr {
    // Connection sessions (WebSocket)
    sessions: DashMap<String, Session>,
    timeout: Duration,
    session_counter: AtomicU64,

    // Chat sessions (persistent, Go-style)
    chat_sessions: Mutex<HashMap<String, ChatSession>>,
    storage_path: Option<PathBuf>,
    /// 同键并发保存串行闸（BUG-20261030-213）：快照与落盘必须在同一把闸内
    /// 完成——快照在状态锁内、落盘在锁外的旧形态下，两线程同键并发保存时
    /// 旧快照的 rename 可能晚于新快照落盘（last-rename-wins = 已完成保存的
    /// 消息在重启后丢失）。BUG-20261025-51 修了撕裂写（唯一临时名），本闸
    /// 补齐「rename 序 ≠ 快照序」的残余竞态。状态锁不在闸内（mutation 路径
    /// 不被落盘阻塞），闸只串行化逐次保存的「快照→rename」对。
    save_gate: Mutex<()>,
    /// 测试钩子（仅 test 构建）：快照→落盘之间注入**一次性**停顿（首个
    /// 保存消费后归零，后续保存不受影响），确定性复现并发保存交错
    /// （release 构建不存在此字段）。
    #[cfg(test)]
    save_delay_ms: std::sync::atomic::AtomicU64,
}

impl SessionMgr {
    /// Create a new session manager with timeout for connection sessions.
    pub fn new(timeout: Duration) -> Self {
        Self {
            sessions: DashMap::new(),
            timeout,
            session_counter: AtomicU64::new(0),
            chat_sessions: Mutex::new(HashMap::new()),
            storage_path: None,
            save_gate: Mutex::new(()),
            #[cfg(test)]
            save_delay_ms: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Create with persistent storage for chat sessions.
    pub fn with_storage(timeout: Duration, storage_path: &str) -> Self {
        let mut mgr = Self::new(timeout);
        if !storage_path.is_empty() {
            let path = PathBuf::from(storage_path);
            let _ = fs::create_dir_all(&path);
            mgr.storage_path = Some(path);
            let _ = mgr.load_chat_sessions();
        }
        mgr
    }

    /// Create with default timeout.
    pub fn with_default_timeout() -> Self {
        Self::new(Duration::from_secs(3600))
    }

    // ==================== Connection Sessions ====================

    /// Create a new connection session.
    pub fn create_session(&self, channel: &str, sender_id: &str, chat_id: &str) -> Session {
        let id = format!(
            "sess_{}",
            self.session_counter.fetch_add(1, Ordering::SeqCst)
        );
        let now = Local::now();
        let session = Session {
            id,
            channel: channel.to_string(),
            sender_id: sender_id.to_string(),
            chat_id: chat_id.to_string(),
            created_at: now,
            last_active: now,
            metadata: HashMap::new(),
        };
        self.sessions.insert(session.id.clone(), session.clone());
        session
    }

    /// Get a connection session by ID.
    pub fn get(&self, id: &str) -> Option<Session> {
        self.sessions.get(id).map(|r| r.value().clone())
    }

    /// Remove a connection session.
    pub fn remove(&self, id: &str) -> Option<Session> {
        self.sessions.remove(id).map(|(_, v)| v)
    }

    /// Touch session (update last active).
    pub fn touch(&self, id: &str) {
        if let Some(mut s) = self.sessions.get_mut(id) {
            s.last_active = Local::now();
        }
    }

    /// Get active session count.
    pub fn count(&self) -> usize {
        self.sessions.len()
    }

    /// Cleanup expired connection sessions.
    pub fn cleanup_expired(&self) -> usize {
        let now = Local::now();
        let timeout =
            chrono::Duration::from_std(self.timeout).unwrap_or(chrono::Duration::hours(1));
        let expired: Vec<String> = self
            .sessions
            .iter()
            .filter(|r| now.signed_duration_since(r.last_active) > timeout)
            .map(|r| r.id.clone())
            .collect();
        let count = expired.len();
        for id in &expired {
            self.sessions.remove(id);
        }
        count
    }

    /// Get session statistics.
    pub fn stats(&self) -> HashMap<String, usize> {
        let mut map = HashMap::new();
        map.insert("active_sessions".to_string(), self.sessions.len());
        map.insert("chat_sessions".to_string(), self.chat_sessions.lock().len());
        map
    }

    /// Shutdown all connection sessions.
    pub fn shutdown(&self) {
        self.sessions.clear();
    }

    // ==================== Chat Sessions (Go-style) ====================

    /// Get or create a chat session by key.
    pub fn get_or_create_chat(&self, key: &str) -> ChatSession {
        let mut sessions = self.chat_sessions.lock();
        if let Some(session) = sessions.get(key) {
            return session.clone();
        }
        let now = Local::now().to_rfc3339();
        let session = ChatSession {
            key: key.to_string(),
            messages: Vec::new(),
            summary: None,
            created: now.clone(),
            updated: now,
        };
        sessions.insert(key.to_string(), session.clone());
        session
    }

    /// Add a message to a chat session.
    pub fn add_message(&self, session_key: &str, role: &str, content: &str) {
        let msg = Message {
            role: role.to_string(),
            content: content.to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: Some(Local::now().to_rfc3339()),
        };
        self.add_full_message(session_key, msg);
    }

    /// Add a complete message with tool calls and tool call ID to the session.
    ///
    /// This mirrors the Go `AddFullMessage(sessionKey, msg)` method. It is used
    /// to save the full conversation flow including tool calls and tool results.
    /// If the message has no timestamp, the current time is assigned automatically.
    /// If the session does not exist yet, it is created.
    pub fn add_full_message(&self, session_key: &str, mut msg: Message) {
        let mut sessions = self.chat_sessions.lock();
        let session = sessions.entry(session_key.to_string()).or_insert_with(|| {
            let now = Local::now().to_rfc3339();
            ChatSession {
                key: session_key.to_string(),
                messages: Vec::new(),
                summary: None,
                created: now.clone(),
                updated: now,
            }
        });

        if msg.timestamp.is_none() {
            msg.timestamp = Some(Local::now().to_rfc3339());
        }

        session.messages.push(msg);
        session.updated = Local::now().to_rfc3339();
        drop(sessions);
        if let Err(e) = self.save_chat_session(session_key) {
            tracing::warn!("[session] Failed to save chat session {session_key}: {e}");
        }
    }

    /// Get chat history for a session key.
    pub fn get_history(&self, key: &str) -> Vec<Message> {
        let sessions = self.chat_sessions.lock();
        match sessions.get(key) {
            Some(session) => session.messages.clone(),
            None => Vec::new(),
        }
    }

    /// Set chat history for a session key.
    pub fn set_history(&self, key: &str, messages: Vec<Message>) {
        let mut sessions = self.chat_sessions.lock();
        if let Some(session) = sessions.get_mut(key) {
            session.messages = messages;
            session.updated = Local::now().to_rfc3339();
        }
        drop(sessions);
        if let Err(e) = self.save_chat_session(key) {
            tracing::warn!("[session] Failed to save chat session {key}: {e}");
        }
    }

    /// Get the summary for a chat session.
    pub fn get_summary(&self, key: &str) -> Option<String> {
        let sessions = self.chat_sessions.lock();
        sessions.get(key).and_then(|s| s.summary.clone())
    }

    /// Set the summary for a chat session.
    pub fn set_summary(&self, key: &str, summary: &str) {
        let mut sessions = self.chat_sessions.lock();
        if let Some(session) = sessions.get_mut(key) {
            session.summary = Some(summary.to_string());
            session.updated = Local::now().to_rfc3339();
        }
        drop(sessions);
        if let Err(e) = self.save_chat_session(key) {
            tracing::warn!("[session] Failed to save chat session {key}: {e}");
        }
    }

    /// Truncate history to keep only the last N messages.
    pub fn truncate_history(&self, key: &str, keep_last: usize) {
        let mut sessions = self.chat_sessions.lock();
        if let Some(session) = sessions.get_mut(key) {
            if keep_last == 0 {
                session.messages.clear();
            } else if session.messages.len() > keep_last {
                let start = session.messages.len() - keep_last;
                session.messages = session.messages[start..].to_vec();
            }
            session.updated = Local::now().to_rfc3339();
        }
        drop(sessions);
        if let Err(e) = self.save_chat_session(key) {
            tracing::warn!("[session] Failed to save chat session {key}: {e}");
        }
    }

    /// Save a single chat session to disk atomically.
    ///
    /// Uses tempfile + fsync + atomic rename, mirroring the Go `SessionManager.Save`
    /// implementation. This ensures that a crash mid-write never corrupts the
    /// existing session file.
    fn save_chat_session(&self, key: &str) -> Result<(), String> {
        let storage = match &self.storage_path {
            Some(p) => p,
            None => return Ok(()),
        };

        // BUG-20261030-213：闸在快照前取、持到函数尾——「快照→rename」对
        // 整体串行化，rename 序 = 快照序（旧形态两把锁夹缝里的交错见字段
        // 注释）。锁序恒为 save_gate → chat_sessions，无反向路径，无死锁。
        let _save_gate = self.save_gate.lock();

        // Snapshot under lock, then perform slow file I/O after unlock.
        let (snapshot, session_path) = {
            let sessions = self.chat_sessions.lock();
            let session = match sessions.get(key) {
                Some(s) => s,
                None => return Ok(()),
            };

            let filename = sanitize_filename(key);
            if filename == "." || filename.contains('/') || filename.contains('\\') {
                return Err("invalid session key for filename".to_string());
            }

            // Deep-copy the session so we can release the lock before I/O.
            let snap = session.clone();
            let path = storage.join(format!("{}.json", filename));
            (snap, path)
        };
        // 状态锁已释放；落盘前的停顿（测试钩子）不持状态锁。

        let data =
            serde_json::to_string_pretty(&snapshot).map_err(|e| format!("serialize: {}", e))?;

        // 测试钩子：确定性制造「快照后、落盘前」的并发窗口（仅 test
        // 构建；一次性——首个保存消费后归零，不影响并发方的保存时序）。
        #[cfg(test)]
        {
            let d = self
                .save_delay_ms
                .swap(0, std::sync::atomic::Ordering::SeqCst);
            if d > 0 {
                std::thread::sleep(Duration::from_millis(d));
            }
        }

        // BUG-20261025-51：落盘委托 REL-002 权威 helper（唯一临时名
        // `.tmp-{pid}-{seq}-{nanos}`）。旧手搓固定 `{key}.json.tmp` 在同键
        // 并发写（loop 线程 × web rewind/compact）时 File::create 截断互踩，
        // 混合字节 rename 落盘 = 损坏 JSON，下次启动 warn 跳过即历史丢失。
        nemesis_utils::file::write_file_atomic(
            &session_path.to_string_lossy(),
            data.as_bytes(),
            0o644,
        )
    }

    /// Load all chat sessions from disk.
    fn load_chat_sessions(&self) -> Result<(), String> {
        let storage = match &self.storage_path {
            Some(p) => p,
            None => return Ok(()),
        };

        let entries = fs::read_dir(storage).map_err(|e| format!("read dir: {}", e))?;

        let mut sessions = self.chat_sessions.lock();
        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let path = entry.path();
            if !path.extension().map(|e| e == "json").unwrap_or(false) {
                continue;
            }
            // BUG-20261025-44：坏档不再静默吞。写侧早已 tempfile+fsync+rename
            // 原子，读侧遇到的损坏只剩外部篡改/历史残留/磁盘问题——正是需要
            // 第一个可见信号的场景（与 auth.json / 分享注册表 / checkpoint
            // 镜像同族口径）。跳过语义不变：不炸、不进索引。
            let data = match fs::read_to_string(&path) {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!("[session] Failed to read {}: {}", path.display(), e);
                    continue;
                }
            };
            match serde_json::from_str::<ChatSession>(&data) {
                Ok(session) => {
                    sessions.insert(session.key.clone(), session);
                }
                Err(e) => {
                    tracing::warn!(
                        "[session] Corrupt session file skipped {}: {}",
                        path.display(),
                        e
                    );
                }
            }
        }

        Ok(())
    }
}

/// Sanitize a session key for use as a filename (replace ':' with '_').
///
/// H1（devtool-upgrade 阶段 2）起 pub：todowrite（nemesis-agent loop_tools）
/// 的存储文件名 `todo_{safe_session_key}.json` 用同款规则——两处已统一为
/// 仓内单一真相源 `nemesis_utils::sanitize::sanitize_path_segment`（SAN-05
/// 白名单；对运行时键族映射与旧 `replace(':')` 一致，另兜 `..`/超长）。
pub fn sanitize_filename(key: &str) -> String {
    nemesis_utils::sanitize::sanitize_path_segment(key)
}

#[cfg(test)]
mod tests;

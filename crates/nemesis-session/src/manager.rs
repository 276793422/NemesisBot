//! Session management with persistence, expiry, and multi-session operations.

use chrono::{DateTime, Local};
use dashmap::DashMap;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
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
        let id = format!("sess_{}", self.session_counter.fetch_add(1, Ordering::SeqCst));
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
        let timeout = chrono::Duration::from_std(self.timeout).unwrap_or(chrono::Duration::hours(1));
        let expired: Vec<String> = self.sessions
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
        let _ = self.save_chat_session(session_key);
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
        let _ = self.save_chat_session(key);
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
        let _ = self.save_chat_session(key);
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
        let _ = self.save_chat_session(key);
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
        // Lock is released here; all I/O below is lock-free.

        let data = serde_json::to_string_pretty(&snapshot)
            .map_err(|e| format!("serialize: {}", e))?;

        // Write to a temporary file first.
        let temp_path = session_path.with_extension("json.tmp");
        {
            let mut file = std::fs::File::create(&temp_path)
                .map_err(|e| format!("create temp: {}", e))?;
            file.write_all(data.as_bytes())
                .map_err(|e| format!("write temp: {}", e))?;
            file.sync_all()
                .map_err(|e| format!("fsync temp: {}", e))?;
        }

        // Atomic rename (on Windows this replaces the destination).
        std::fs::rename(&temp_path, &session_path)
            .map_err(|e| format!("rename: {}", e))?;

        Ok(())
    }

    /// Load all chat sessions from disk.
    fn load_chat_sessions(&self) -> Result<(), String> {
        let storage = match &self.storage_path {
            Some(p) => p,
            None => return Ok(()),
        };

        let entries = fs::read_dir(storage)
            .map_err(|e| format!("read dir: {}", e))?;

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
            if let Ok(data) = fs::read_to_string(&path) {
                if let Ok(session) = serde_json::from_str::<ChatSession>(&data) {
                    sessions.insert(session.key.clone(), session);
                }
            }
        }

        Ok(())
    }
}

/// Sanitize a session key for use as a filename (replace ':' with '_').
fn sanitize_filename(key: &str) -> String {
    key.replace(':', "_")
}

#[cfg(test)]
mod tests;

//! Session management with persistence, expiry, and multi-session operations.

use chrono::{DateTime, Utc};
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
    pub created_at: DateTime<Utc>,
    pub last_active: DateTime<Utc>,
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
        let now = Utc::now();
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
            s.last_active = Utc::now();
        }
    }

    /// Get active session count.
    pub fn count(&self) -> usize {
        self.sessions.len()
    }

    /// Cleanup expired connection sessions.
    pub fn cleanup_expired(&self) -> usize {
        let now = Utc::now();
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
        let now = Utc::now().to_rfc3339();
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
            timestamp: Some(Utc::now().to_rfc3339()),
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
            let now = Utc::now().to_rfc3339();
            ChatSession {
                key: session_key.to_string(),
                messages: Vec::new(),
                summary: None,
                created: now.clone(),
                updated: now,
            }
        });

        if msg.timestamp.is_none() {
            msg.timestamp = Some(Utc::now().to_rfc3339());
        }

        session.messages.push(msg);
        session.updated = Utc::now().to_rfc3339();
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
            session.updated = Utc::now().to_rfc3339();
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
            session.updated = Utc::now().to_rfc3339();
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
            session.updated = Utc::now().to_rfc3339();
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
mod tests {
    use super::*;

    #[test]
    fn test_create_and_get() {
        let mgr = SessionMgr::new(Duration::from_secs(3600));
        let session = mgr.create_session("web", "user1", "chat1");
        assert_eq!(session.channel, "web");
        let found = mgr.get(&session.id).unwrap();
        assert_eq!(found.sender_id, "user1");
    }

    #[test]
    fn test_remove() {
        let mgr = SessionMgr::new(Duration::from_secs(3600));
        let session = mgr.create_session("web", "user1", "chat1");
        assert!(mgr.remove(&session.id).is_some());
        assert!(mgr.get(&session.id).is_none());
    }

    #[test]
    fn test_count() {
        let mgr = SessionMgr::new(Duration::from_secs(3600));
        assert_eq!(mgr.count(), 0);
        mgr.create_session("web", "u1", "c1");
        mgr.create_session("web", "u2", "c2");
        assert_eq!(mgr.count(), 2);
    }

    #[test]
    fn test_chat_session_get_or_create() {
        let mgr = SessionMgr::new(Duration::from_secs(3600));
        let session = mgr.get_or_create_chat("web:user1");
        assert_eq!(session.key, "web:user1");
        assert!(session.messages.is_empty());

        let session2 = mgr.get_or_create_chat("web:user1");
        assert_eq!(session2.key, session.key);
    }

    #[test]
    fn test_chat_session_add_message() {
        let mgr = SessionMgr::new(Duration::from_secs(3600));
        mgr.add_message("web:user1", "user", "hello");
        mgr.add_message("web:user1", "assistant", "hi there");

        let history = mgr.get_history("web:user1");
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].role, "user");
        assert_eq!(history[1].role, "assistant");
    }

    #[test]
    fn test_chat_session_set_summary() {
        let mgr = SessionMgr::new(Duration::from_secs(3600));
        mgr.add_message("web:user1", "user", "hello");
        mgr.set_summary("web:user1", "A greeting");
        assert_eq!(mgr.get_summary("web:user1"), Some("A greeting".to_string()));
    }

    #[test]
    fn test_chat_session_truncate() {
        let mgr = SessionMgr::new(Duration::from_secs(3600));
        for i in 0..10 {
            mgr.add_message("web:user1", "user", &format!("msg {}", i));
        }
        mgr.truncate_history("web:user1", 3);
        let history = mgr.get_history("web:user1");
        assert_eq!(history.len(), 3);
    }

    #[test]
    fn test_chat_session_set_history() {
        let mgr = SessionMgr::new(Duration::from_secs(3600));
        mgr.add_message("web:user1", "user", "old");
        let new_messages = vec![
            Message { role: "system".to_string(), content: "new".to_string(), tool_calls: Vec::new(), tool_call_id: None, timestamp: None },
        ];
        mgr.set_history("web:user1", new_messages);
        let history = mgr.get_history("web:user1");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].content, "new");
    }

    #[test]
    fn test_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions").to_string_lossy().to_string();

        {
            let mgr = SessionMgr::with_storage(Duration::from_secs(3600), &path);
            mgr.add_message("web:user1", "user", "persistent message");
        }

        // Reload
        let mgr2 = SessionMgr::with_storage(Duration::from_secs(3600), &path);
        let history = mgr2.get_history("web:user1");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].content, "persistent message");
    }

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("web:user1"), "web_user1");
        assert_eq!(sanitize_filename("telegram:123456"), "telegram_123456");
    }

    #[test]
    fn test_stats() {
        let mgr = SessionMgr::new(Duration::from_secs(3600));
        mgr.create_session("web", "u1", "c1");
        mgr.add_message("web:user1", "user", "hello");
        let stats = mgr.stats();
        assert_eq!(stats["active_sessions"], 1);
        assert_eq!(stats["chat_sessions"], 1);
    }

    #[test]
    fn test_add_full_message_with_tool_calls() {
        let mgr = SessionMgr::new(Duration::from_secs(3600));

        // Add a message with tool calls
        let tool_msg = Message {
            role: "assistant".to_string(),
            content: String::new(),
            tool_calls: vec![ToolCall {
                id: "call_123".to_string(),
                call_type: Some("function".to_string()),
                function: Some(FunctionCall {
                    name: "read_file".to_string(),
                    arguments: r#"{"path":"/tmp/test"}"#.to_string(),
                }),
            }],
            tool_call_id: None,
            timestamp: None, // should be auto-filled
        };
        mgr.add_full_message("web:user1", tool_msg);

        // Add a tool result message
        let result_msg = Message {
            role: "tool".to_string(),
            content: "file contents here".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: Some("call_123".to_string()),
            timestamp: Some(Utc::now().to_rfc3339()),
        };
        mgr.add_full_message("web:user1", result_msg);

        let history = mgr.get_history("web:user1");
        assert_eq!(history.len(), 2);

        // Check tool call message
        assert_eq!(history[0].role, "assistant");
        assert_eq!(history[0].tool_calls.len(), 1);
        assert_eq!(history[0].tool_calls[0].id, "call_123");
        assert!(history[0].timestamp.is_some()); // auto-filled

        // Check tool result message
        assert_eq!(history[1].role, "tool");
        assert_eq!(history[1].tool_call_id, Some("call_123".to_string()));
    }

    #[test]
    fn test_add_full_message_creates_session() {
        let mgr = SessionMgr::new(Duration::from_secs(3600));
        let msg = Message {
            role: "user".to_string(),
            content: "hello".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: None,
        };
        mgr.add_full_message("web:auto_created", msg);

        let history = mgr.get_history("web:auto_created");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].content, "hello");
    }

    // ==================== Additional coverage tests ====================

    #[test]
    fn test_touch_updates_last_active() {
        let mgr = SessionMgr::new(Duration::from_secs(3600));
        let session = mgr.create_session("web", "user1", "chat1");
        let original_last_active = session.last_active;

        // Sleep briefly so the timestamp changes
        std::thread::sleep(std::time::Duration::from_millis(10));
        mgr.touch(&session.id);

        let updated = mgr.get(&session.id).unwrap();
        assert!(updated.last_active > original_last_active);
    }

    #[test]
    fn test_touch_nonexistent_session_no_panic() {
        let mgr = SessionMgr::new(Duration::from_secs(3600));
        // Should silently do nothing
        mgr.touch("nonexistent_id");
    }

    #[test]
    fn test_cleanup_expired_removes_expired_sessions() {
        // Use a very short timeout (1ms) so sessions expire immediately
        let mgr = SessionMgr::new(Duration::from_millis(1));
        mgr.create_session("web", "u1", "c1");
        mgr.create_session("web", "u2", "c2");
        assert_eq!(mgr.count(), 2);

        // Sleep to let sessions expire
        std::thread::sleep(std::time::Duration::from_millis(50));

        let removed = mgr.cleanup_expired();
        assert_eq!(removed, 2);
        assert_eq!(mgr.count(), 0);
    }

    #[test]
    fn test_cleanup_expired_keeps_active_sessions() {
        // Use a long timeout so sessions won't expire
        let mgr = SessionMgr::new(Duration::from_secs(3600));
        mgr.create_session("web", "u1", "c1");
        mgr.create_session("web", "u2", "c2");

        let removed = mgr.cleanup_expired();
        assert_eq!(removed, 0);
        assert_eq!(mgr.count(), 2);
    }

    #[test]
    fn test_shutdown_clears_all_sessions() {
        let mgr = SessionMgr::new(Duration::from_secs(3600));
        mgr.create_session("web", "u1", "c1");
        mgr.create_session("web", "u2", "c2");
        mgr.create_session("web", "u3", "c3");
        assert_eq!(mgr.count(), 3);

        mgr.shutdown();
        assert_eq!(mgr.count(), 0);
    }

    #[test]
    fn test_with_default_timeout_creates_3600s_timeout() {
        let mgr = SessionMgr::with_default_timeout();
        // Create a session and verify it doesn't expire immediately
        let _session = mgr.create_session("web", "u1", "c1");
        assert_eq!(mgr.count(), 1);

        // Cleanup should not remove it since timeout is 3600s
        let removed = mgr.cleanup_expired();
        assert_eq!(removed, 0);
        assert_eq!(mgr.count(), 1);
    }

    #[test]
    fn test_with_storage_empty_path_no_storage() {
        let mgr = SessionMgr::with_storage(Duration::from_secs(3600), "");
        // Should work fine, just no storage
        mgr.add_message("web:user1", "user", "hello");
        let history = mgr.get_history("web:user1");
        assert_eq!(history.len(), 1);
        assert!(mgr.storage_path.is_none());
    }

    #[test]
    fn test_set_history_nonexistent_session_silent_return() {
        let mgr = SessionMgr::new(Duration::from_secs(3600));
        // Setting history on a non-existent session should not panic
        let msgs = vec![Message {
            role: "user".to_string(),
            content: "test".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: None,
        }];
        mgr.set_history("nonexistent:key", msgs);
        // Verify the session was not created
        assert!(mgr.get_history("nonexistent:key").is_empty());
    }

    #[test]
    fn test_set_summary_nonexistent_session_silent_return() {
        let mgr = SessionMgr::new(Duration::from_secs(3600));
        // Setting summary on a non-existent session should not panic
        mgr.set_summary("nonexistent:key", "some summary");
        // Verify the session was not created
        assert!(mgr.get_summary("nonexistent:key").is_none());
    }

    #[test]
    fn test_truncate_history_keep_zero_clears_all() {
        let mgr = SessionMgr::new(Duration::from_secs(3600));
        mgr.add_message("web:user1", "user", "msg1");
        mgr.add_message("web:user1", "user", "msg2");
        mgr.add_message("web:user1", "user", "msg3");
        assert_eq!(mgr.get_history("web:user1").len(), 3);

        mgr.truncate_history("web:user1", 0);
        assert!(mgr.get_history("web:user1").is_empty());
    }

    #[test]
    fn test_truncate_history_nonexistent_session_silent_return() {
        let mgr = SessionMgr::new(Duration::from_secs(3600));
        // Should not panic
        mgr.truncate_history("nonexistent:key", 5);
    }

    #[test]
    fn test_save_chat_session_invalid_key_dot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions").to_string_lossy().to_string();
        let mgr = SessionMgr::with_storage(Duration::from_secs(3600), &path);

        // Add a message to create a session, then manually try to save with bad key
        mgr.add_message(".", "user", "test");
        // The session is in memory; save_chat_session should return Err for "." key
        let result = mgr.save_chat_session(".");
        assert!(result.is_err());
    }

    #[test]
    fn test_save_chat_session_invalid_key_slash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions").to_string_lossy().to_string();
        let mgr = SessionMgr::with_storage(Duration::from_secs(3600), &path);

        mgr.add_message("bad/key", "user", "test");
        let result = mgr.save_chat_session("bad/key");
        assert!(result.is_err());
    }

    #[test]
    fn test_save_chat_session_invalid_key_backslash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions").to_string_lossy().to_string();
        let mgr = SessionMgr::with_storage(Duration::from_secs(3600), &path);

        mgr.add_message("bad\\key", "user", "test");
        let result = mgr.save_chat_session("bad\\key");
        assert!(result.is_err());
    }

    #[test]
    fn test_message_serialization_deserialization() {
        let msg = Message {
            role: "assistant".to_string(),
            content: "hello world".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: Some("2026-01-01T00:00:00Z".to_string()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.role, "assistant");
        assert_eq!(deserialized.content, "hello world");
        assert_eq!(deserialized.timestamp, Some("2026-01-01T00:00:00Z".to_string()));
    }

    #[test]
    fn test_message_with_tool_calls_serialization() {
        let msg = Message {
            role: "assistant".to_string(),
            content: String::new(),
            tool_calls: vec![ToolCall {
                id: "call_abc".to_string(),
                call_type: Some("function".to_string()),
                function: Some(FunctionCall {
                    name: "read_file".to_string(),
                    arguments: r#"{"path":"/tmp/test.txt"}"#.to_string(),
                }),
            }],
            tool_call_id: None,
            timestamp: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.tool_calls.len(), 1);
        assert_eq!(deserialized.tool_calls[0].id, "call_abc");
        let func = deserialized.tool_calls[0].function.as_ref().unwrap();
        assert_eq!(func.name, "read_file");
    }

    #[test]
    fn test_tool_call_serialization() {
        let tc = ToolCall {
            id: "tc_001".to_string(),
            call_type: Some("function".to_string()),
            function: Some(FunctionCall {
                name: "execute".to_string(),
                arguments: "{}".to_string(),
            }),
        };
        let json = serde_json::to_string(&tc).unwrap();
        let deserialized: ToolCall = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "tc_001");
        assert_eq!(deserialized.call_type, Some("function".to_string()));
    }

    #[test]
    fn test_tool_call_skip_none_fields() {
        let tc = ToolCall {
            id: "tc_002".to_string(),
            call_type: None,
            function: None,
        };
        let json = serde_json::to_string(&tc).unwrap();
        // call_type and function should be absent from JSON since they are None
        assert!(!json.contains("call_type"));
        assert!(!json.contains("function"));
        let deserialized: ToolCall = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "tc_002");
        assert!(deserialized.call_type.is_none());
        assert!(deserialized.function.is_none());
    }

    #[test]
    fn test_function_call_serialization() {
        let fc = FunctionCall {
            name: "search".to_string(),
            arguments: r#"{"query":"test"}"#.to_string(),
        };
        let json = serde_json::to_string(&fc).unwrap();
        let deserialized: FunctionCall = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "search");
        assert_eq!(deserialized.arguments, r#"{"query":"test"}"#);
    }

    #[test]
    fn test_chat_session_serialization_roundtrip() {
        let session = ChatSession {
            key: "web:user1".to_string(),
            messages: vec![
                Message {
                    role: "user".to_string(),
                    content: "hello".to_string(),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                    timestamp: Some("2026-01-01T12:00:00Z".to_string()),
                },
                Message {
                    role: "assistant".to_string(),
                    content: "hi".to_string(),
                    tool_calls: vec![ToolCall {
                        id: "call_1".to_string(),
                        call_type: Some("function".to_string()),
                        function: Some(FunctionCall {
                            name: "tool1".to_string(),
                            arguments: "{}".to_string(),
                        }),
                    }],
                    tool_call_id: None,
                    timestamp: Some("2026-01-01T12:00:01Z".to_string()),
                },
            ],
            summary: Some("A greeting exchange".to_string()),
            created: "2026-01-01T12:00:00Z".to_string(),
            updated: "2026-01-01T12:00:01Z".to_string(),
        };

        let json = serde_json::to_string_pretty(&session).unwrap();
        let deserialized: ChatSession = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.key, "web:user1");
        assert_eq!(deserialized.messages.len(), 2);
        assert_eq!(deserialized.summary, Some("A greeting exchange".to_string()));
        assert_eq!(deserialized.created, "2026-01-01T12:00:00Z");
        assert_eq!(deserialized.messages[0].role, "user");
        assert_eq!(deserialized.messages[1].tool_calls.len(), 1);
    }

    #[test]
    fn test_chat_session_skip_none_summary() {
        let session = ChatSession {
            key: "web:user1".to_string(),
            messages: Vec::new(),
            summary: None,
            created: "2026-01-01T00:00:00Z".to_string(),
            updated: "2026-01-01T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&session).unwrap();
        // summary should be absent from JSON when None
        assert!(!json.contains("summary"));
        let deserialized: ChatSession = serde_json::from_str(&json).unwrap();
        assert!(deserialized.summary.is_none());
    }

    #[test]
    fn test_message_skip_empty_tool_calls() {
        let msg = Message {
            role: "user".to_string(),
            content: "hi".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        // tool_calls and tool_call_id should be absent
        assert!(!json.contains("tool_calls"));
        assert!(!json.contains("tool_call_id"));
    }

    #[test]
    fn test_save_chat_session_no_storage_path() {
        let mgr = SessionMgr::new(Duration::from_secs(3600));
        mgr.add_message("web:user1", "user", "hello");
        // No storage path configured, should return Ok
        let result = mgr.save_chat_session("web:user1");
        assert!(result.is_ok());
    }

    #[test]
    fn test_save_chat_session_nonexistent_key_no_storage() {
        let mgr = SessionMgr::new(Duration::from_secs(3600));
        // Non-existent key with no storage path, should return Ok (early return for None)
        let result = mgr.save_chat_session("nonexistent");
        assert!(result.is_ok());
    }

    #[test]
    fn test_save_chat_session_nonexistent_key_with_storage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions").to_string_lossy().to_string();
        let mgr = SessionMgr::with_storage(Duration::from_secs(3600), &path);
        // Non-existent key, should return Ok (no session found to save)
        let result = mgr.save_chat_session("nonexistent");
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_nonexistent_session() {
        let mgr = SessionMgr::new(Duration::from_secs(3600));
        assert!(mgr.get("nonexistent").is_none());
    }

    #[test]
    fn test_remove_nonexistent_session() {
        let mgr = SessionMgr::new(Duration::from_secs(3600));
        assert!(mgr.remove("nonexistent").is_none());
    }

    #[test]
    fn test_session_counter_increments() {
        let mgr = SessionMgr::new(Duration::from_secs(3600));
        let s1 = mgr.create_session("web", "u1", "c1");
        let s2 = mgr.create_session("web", "u2", "c2");
        let s3 = mgr.create_session("web", "u3", "c3");
        assert!(s1.id != s2.id);
        assert!(s2.id != s3.id);
        // All should start with "sess_"
        assert!(s1.id.starts_with("sess_"));
        assert!(s2.id.starts_with("sess_"));
    }

    #[test]
    fn test_session_metadata_default_empty() {
        let mgr = SessionMgr::new(Duration::from_secs(3600));
        let session = mgr.create_session("web", "u1", "c1");
        assert!(session.metadata.is_empty());
    }

    #[test]
    fn test_get_history_nonexistent_session() {
        let mgr = SessionMgr::new(Duration::from_secs(3600));
        let history = mgr.get_history("nonexistent:key");
        assert!(history.is_empty());
    }
}

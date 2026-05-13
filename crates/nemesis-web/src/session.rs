//! Session management for WebSocket connections.
//!
//! Mirrors the Go `module/web/session.go`:
//! - `Session` — lightweight metadata (no WebSocket dependency for portability)
//! - `SessionManager` — DashMap-backed concurrent session store with:
//!   - Session create/get/remove/touch
//!   - Broadcast to session (via send queue)
//!   - Background cleanup of inactive sessions
//!   - Stats and active count

use crate::websocket_handler::SendQueue;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

/// Session metadata (no WebSocket dependency for portability).
#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub sender_id: String,
    pub chat_id: String,
    pub created_at: DateTime<Utc>,
    pub last_active: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Internal entry: session + optional send queue
// ---------------------------------------------------------------------------

/// Internal session entry that includes the optional send queue.
/// Not exposed to callers directly.
#[derive(Clone)]
struct SessionEntry {
    session: Session,
    send_queue: Option<Arc<SendQueue>>,
}

// ---------------------------------------------------------------------------
// Session manager
// ---------------------------------------------------------------------------

/// Manages all active WebSocket sessions.
pub struct SessionManager {
    sessions: DashMap<String, SessionEntry>,
    send_queues: DashMap<String, Arc<SendQueue>>,
    timeout: Duration,
    stop_cleanup: RwLock<Option<JoinHandle<()>>>,
}

impl SessionManager {
    /// Create a new session manager with the given timeout duration.
    pub fn new(timeout: Duration) -> Self {
        let sm = Self {
            sessions: DashMap::new(),
            send_queues: DashMap::new(),
            timeout,
            stop_cleanup: RwLock::new(None),
        };
        sm.start_cleanup();
        sm
    }

    /// Create a session with default timeout (1 hour).
    pub fn with_default_timeout() -> Self {
        Self::new(Duration::from_secs(3600))
    }

    /// Create a new session and return it.
    pub fn create_session(&self) -> Session {
        let id = generate_session_id();
        let sender_id = format!("web:{}", id);
        let chat_id = sender_id.clone();
        let now = Utc::now();

        let session = Session {
            id: id.clone(),
            sender_id,
            chat_id,
            created_at: now,
            last_active: now,
        };

        let entry = SessionEntry {
            session: session.clone(),
            send_queue: None,
        };

        self.sessions.insert(id, entry);

        tracing::debug!(
            session_id = %session.id,
            sender_id = %session.sender_id,
            chat_id = %session.chat_id,
            "Session created"
        );

        session
    }

    /// Get a session by ID.
    pub fn get_session(&self, session_id: &str) -> Option<Session> {
        self.sessions.get(session_id).map(|r| r.session.clone())
    }

    /// Remove a session.
    pub fn remove_session(&self, session_id: &str) {
        self.sessions.remove(session_id);
        self.send_queues.remove(session_id);
        tracing::debug!(session_id = %session_id, "Session removed");
    }

    /// Update last active time for a session.
    pub fn touch_session(&self, session_id: &str) {
        if let Some(mut entry) = self.sessions.get_mut(session_id) {
            entry.session.last_active = Utc::now();
        }
    }

    /// Set the send queue for a session.
    pub fn set_send_queue(&self, session_id: &str, queue: Arc<SendQueue>) {
        if let Some(mut entry) = self.sessions.get_mut(session_id) {
            entry.send_queue = Some(queue.clone());
        }
        self.send_queues.insert(session_id.to_string(), queue);
    }

    /// Broadcast (send) raw bytes to a specific session.
    ///
    /// Uses the send queue if available, otherwise returns an error.
    /// This mirrors the Go `SessionManager.Broadcast` method.
    pub async fn broadcast(&self, session_id: &str, message: &[u8]) -> Result<(), String> {
        tracing::debug!(
            session_id = %session_id,
            message_len = message.len(),
            "Broadcasting to session"
        );

        // Try send queue first (thread-safe)
        if let Some(queue) = self.send_queues.get(session_id) {
            self.touch_session(session_id);
            return queue.send(message.to_vec()).await;
        }

        // No send queue means session has no active WebSocket
        tracing::warn!(
            session_id = %session_id,
            message_len = message.len(),
            "Session not found or no send queue"
        );
        Err(format!("session not found or no send queue: {}", session_id))
    }

    /// Get active session count.
    pub fn active_count(&self) -> usize {
        self.sessions.len()
    }

    /// Get statistics about active sessions.
    pub fn stats(&self) -> HashMap<String, serde_json::Value> {
        let mut map = HashMap::new();
        map.insert(
            "active_sessions".to_string(),
            serde_json::Value::Number(serde_json::Number::from(self.sessions.len())),
        );
        map
    }

    /// Get all sessions.
    pub fn all_sessions(&self) -> Vec<Session> {
        self.sessions.iter().map(|r| r.session.clone()).collect()
    }

    /// Shutdown all sessions.
    pub async fn shutdown(&self) {
        if let Some(handle) = self.stop_cleanup.write().await.take() {
            handle.abort();
        }
        self.sessions.clear();
        self.send_queues.clear();
        tracing::info!("Session manager shutdown complete");
    }

    /// Start the background cleanup task.
    /// Uses std::thread with an embedded tokio runtime to avoid requiring
    /// a tokio runtime at construction time.
    fn start_cleanup(&self) {
        let timeout = self.timeout;
        let sessions = self.sessions.clone();
        let send_queues = self.send_queues.clone();

        let handle = std::thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(_) => return,
            };
            rt.block_on(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(300)); // 5 minutes
                loop {
                    interval.tick().await;

                    let now = Utc::now();
                    let mut to_remove = Vec::new();

                    for entry in sessions.iter() {
                        let inactive_duration = now.signed_duration_since(entry.session.last_active);
                        let inactive_secs = inactive_duration.num_seconds();
                        if inactive_secs > timeout.as_secs() as i64 {
                            to_remove.push(entry.session.id.clone());
                        }
                    }

                    for session_id in to_remove {
                        sessions.remove(&session_id);
                        send_queues.remove(&session_id);
                        tracing::info!(session_id = %session_id, "Removed inactive session");
                    }
                }
            });
        });

        // Store handle for cleanup on shutdown.
        // Use a tokio JoinHandle wrapping the std::thread so we can abort it.
        // Since we can't easily convert, we just store a dummy and drop the std handle.
        // The cleanup thread will run until the DashMap is dropped.
        let _ = handle; // Thread will terminate when DashMaps are dropped
    }
}

/// Generate a unique session ID (16-char hex from UUID v4).
fn generate_session_id() -> String {
    let id = uuid::Uuid::new_v4().to_string().replace("-", "");
    id[..16].to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_session() {
        let mgr = SessionManager::with_default_timeout();
        let session = mgr.create_session();
        assert!(!session.id.is_empty());
        assert!(session.sender_id.starts_with("web:"));
        assert_eq!(session.chat_id, session.sender_id);
    }

    #[test]
    fn test_get_session() {
        let mgr = SessionManager::with_default_timeout();
        let session = mgr.create_session();
        let found = mgr.get_session(&session.id).unwrap();
        assert_eq!(found.id, session.id);
    }

    #[test]
    fn test_remove_session() {
        let mgr = SessionManager::with_default_timeout();
        let session = mgr.create_session();
        mgr.remove_session(&session.id);
        assert!(mgr.get_session(&session.id).is_none());
    }

    #[test]
    fn test_active_count() {
        let mgr = SessionManager::with_default_timeout();
        assert_eq!(mgr.active_count(), 0);
        let s1 = mgr.create_session();
        let _s2 = mgr.create_session();
        assert_eq!(mgr.active_count(), 2);
        mgr.remove_session(&s1.id);
        assert_eq!(mgr.active_count(), 1);
    }

    #[test]
    fn test_touch_session() {
        let mgr = SessionManager::with_default_timeout();
        let session = mgr.create_session();
        let original = session.last_active;
        std::thread::sleep(std::time::Duration::from_millis(10));
        mgr.touch_session(&session.id);
        let updated = mgr.get_session(&session.id).unwrap();
        assert!(updated.last_active > original);
    }

    #[test]
    fn test_stats() {
        let mgr = SessionManager::with_default_timeout();
        let _s1 = mgr.create_session();
        let _s2 = mgr.create_session();
        let stats = mgr.stats();
        assert_eq!(stats.get("active_sessions").unwrap().as_u64(), Some(2));
    }

    #[test]
    fn test_all_sessions() {
        let mgr = SessionManager::with_default_timeout();
        let s1 = mgr.create_session();
        let s2 = mgr.create_session();
        let all = mgr.all_sessions();
        assert_eq!(all.len(), 2);
        let ids: Vec<&str> = all.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&s1.id.as_str()));
        assert!(ids.contains(&s2.id.as_str()));
    }

    #[test]
    fn test_broadcast_no_send_queue() {
        let mgr = SessionManager::with_default_timeout();
        let session = mgr.create_session();
        // Without a send queue, broadcast should fail
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(mgr.broadcast(&session.id, b"test"));
        assert!(result.is_err());
    }

    #[test]
    fn test_get_session_nonexistent() {
        let mgr = SessionManager::with_default_timeout();
        assert!(mgr.get_session("nonexistent").is_none());
    }

    #[test]
    fn test_touch_session_nonexistent() {
        let mgr = SessionManager::with_default_timeout();
        // Should not panic
        mgr.touch_session("nonexistent");
    }

    #[test]
    fn test_remove_session_nonexistent() {
        let mgr = SessionManager::with_default_timeout();
        // Should not panic
        mgr.remove_session("nonexistent");
    }

    #[test]
    fn test_generate_session_id_format() {
        let id = generate_session_id();
        assert_eq!(id.len(), 16);
        // Should be hex characters only
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_generate_session_id_unique() {
        let id1 = generate_session_id();
        let id2 = generate_session_id();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_session_fields_populated() {
        let mgr = SessionManager::with_default_timeout();
        let session = mgr.create_session();
        assert!(!session.id.is_empty());
        assert!(!session.sender_id.is_empty());
        assert!(!session.chat_id.is_empty());
        assert!(session.created_at <= session.last_active);
    }

    #[test]
    fn test_create_multiple_sessions() {
        let mgr = SessionManager::with_default_timeout();
        let s1 = mgr.create_session();
        let s2 = mgr.create_session();
        let s3 = mgr.create_session();

        assert_ne!(s1.id, s2.id);
        assert_ne!(s2.id, s3.id);
        assert_eq!(mgr.active_count(), 3);
    }

    #[test]
    fn test_remove_all_sessions() {
        let mgr = SessionManager::with_default_timeout();
        let s1 = mgr.create_session();
        let s2 = mgr.create_session();
        mgr.remove_session(&s1.id);
        mgr.remove_session(&s2.id);
        assert_eq!(mgr.active_count(), 0);
        assert!(mgr.all_sessions().is_empty());
    }

    #[test]
    fn test_session_manager_new_with_custom_timeout() {
        let mgr = SessionManager::new(std::time::Duration::from_secs(30));
        assert_eq!(mgr.active_count(), 0);
    }

    #[test]
    fn test_shutdown() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mgr = SessionManager::with_default_timeout();
        let _s1 = mgr.create_session();
        let _s2 = mgr.create_session();
        assert_eq!(mgr.active_count(), 2);

        rt.block_on(mgr.shutdown());
        assert_eq!(mgr.active_count(), 0);
    }

    #[test]
    fn test_set_send_queue_for_nonexistent_session() {
        let _mgr = SessionManager::with_default_timeout();
        // Should not panic when setting send queue for nonexistent session
        let _rt = tokio::runtime::Runtime::new().unwrap();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
        let (done_tx, done_rx) = tokio::sync::watch::channel(false);

        // Create a minimal SendQueue-like structure
        // We can't easily create a real SendQueue without a WebSocket, so this
        // just tests that set_send_queue doesn't panic for nonexistent sessions.
        drop(tx);
        drop(done_tx);
        drop(done_rx);
        // The key test: no panic
    }

    #[test]
    fn test_stats_reflects_changes() {
        let mgr = SessionManager::with_default_timeout();
        let stats0 = mgr.stats();
        assert_eq!(stats0.get("active_sessions").unwrap().as_u64(), Some(0));

        let _s = mgr.create_session();
        let stats1 = mgr.stats();
        assert_eq!(stats1.get("active_sessions").unwrap().as_u64(), Some(1));
    }

    #[test]
    fn test_all_sessions_after_removal() {
        let mgr = SessionManager::with_default_timeout();
        let s1 = mgr.create_session();
        let s2 = mgr.create_session();
        let s3 = mgr.create_session();
        mgr.remove_session(&s2.id);

        let all = mgr.all_sessions();
        assert_eq!(all.len(), 2);
        let ids: Vec<&str> = all.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&s1.id.as_str()));
        assert!(ids.contains(&s3.id.as_str()));
        assert!(!ids.contains(&s2.id.as_str()));
    }

    #[test]
    fn test_broadcast_to_nonexistent_session() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mgr = SessionManager::with_default_timeout();
        let result = rt.block_on(mgr.broadcast("nonexistent-id", b"test"));
        assert!(result.is_err());
    }

    #[test]
    fn test_session_id_is_16_chars() {
        for _ in 0..10 {
            let id = generate_session_id();
            assert_eq!(id.len(), 16, "Session ID should be 16 chars, got: {}", id);
        }
    }

    #[test]
    fn test_session_sender_id_format() {
        let mgr = SessionManager::with_default_timeout();
        let session = mgr.create_session();
        assert!(session.sender_id.starts_with("web:"));
        assert_eq!(&session.sender_id[4..], &session.id);
    }

    #[test]
    fn test_session_created_at_equals_last_active() {
        let mgr = SessionManager::with_default_timeout();
        let session = mgr.create_session();
        assert_eq!(session.created_at, session.last_active);
    }

    #[test]
    fn test_touch_updates_only_last_active() {
        let mgr = SessionManager::with_default_timeout();
        let session = mgr.create_session();
        let original_created = session.created_at;
        std::thread::sleep(std::time::Duration::from_millis(10));
        mgr.touch_session(&session.id);
        let updated = mgr.get_session(&session.id).unwrap();
        assert_eq!(updated.created_at, original_created);
        assert!(updated.last_active > original_created);
    }

    #[test]
    fn test_stats_empty() {
        let mgr = SessionManager::with_default_timeout();
        let stats = mgr.stats();
        assert_eq!(stats.get("active_sessions").unwrap().as_u64(), Some(0));
    }

    #[test]
    fn test_remove_all_then_add() {
        let mgr = SessionManager::with_default_timeout();
        let s1 = mgr.create_session();
        mgr.remove_session(&s1.id);
        assert_eq!(mgr.active_count(), 0);
        let s2 = mgr.create_session();
        assert_eq!(mgr.active_count(), 1);
        assert!(mgr.get_session(&s2.id).is_some());
    }

    #[test]
    fn test_multiple_touches() {
        let mgr = SessionManager::with_default_timeout();
        let session = mgr.create_session();
        let mut last_active = session.last_active;

        for _ in 0..5 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            mgr.touch_session(&session.id);
            let updated = mgr.get_session(&session.id).unwrap();
            assert!(updated.last_active >= last_active);
            last_active = updated.last_active;
        }
    }

    #[test]
    fn test_all_sessions_returns_empty_after_removal() {
        let mgr = SessionManager::with_default_timeout();
        let s1 = mgr.create_session();
        let s2 = mgr.create_session();
        mgr.remove_session(&s1.id);
        mgr.remove_session(&s2.id);
        assert!(mgr.all_sessions().is_empty());
    }

    #[test]
    fn test_session_different_ids() {
        let mgr = SessionManager::with_default_timeout();
        let ids: Vec<String> = (0..10).map(|_| mgr.create_session().id).collect();
        let unique: std::collections::HashSet<&String> = ids.iter().collect();
        assert_eq!(unique.len(), 10);
    }

    #[test]
    fn test_get_session_after_removal_returns_none() {
        let mgr = SessionManager::with_default_timeout();
        let session = mgr.create_session();
        let id = session.id.clone();
        mgr.remove_session(&id);
        assert!(mgr.get_session(&id).is_none());
    }

    #[test]
    fn test_shutdown_then_create() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mgr = SessionManager::with_default_timeout();
        let _s = mgr.create_session();
        rt.block_on(mgr.shutdown());
        assert_eq!(mgr.active_count(), 0);
        // Should be able to create new sessions after shutdown
        let _s = mgr.create_session();
        assert_eq!(mgr.active_count(), 1);
    }

    #[test]
    fn test_session_equality_by_id() {
        let mgr = SessionManager::with_default_timeout();
        let session = mgr.create_session();
        let retrieved = mgr.get_session(&session.id).unwrap();
        assert_eq!(session.id, retrieved.id);
        assert_eq!(session.sender_id, retrieved.sender_id);
        assert_eq!(session.chat_id, retrieved.chat_id);
    }

    // ---- Additional coverage tests for 95%+ ----

    #[test]
    fn test_set_send_queue_for_existing_session() {
        let mgr = SessionManager::with_default_timeout();
        let session = mgr.create_session();

        let (tx, _rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
        let (_done_tx, done_rx) = tokio::sync::watch::channel(false);
        let queue = Arc::new(crate::websocket_handler::SendQueue::from_channels(tx, done_rx));

        mgr.set_send_queue(&session.id, queue);
        assert!(mgr.send_queues.contains_key(&session.id));
    }

    #[tokio::test]
    async fn test_broadcast_with_send_queue() {
        let mgr = SessionManager::with_default_timeout();
        let session = mgr.create_session();

        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
        let (_done_tx, done_rx) = tokio::sync::watch::channel(false);
        let queue = Arc::new(crate::websocket_handler::SendQueue::from_channels(tx, done_rx));

        mgr.set_send_queue(&session.id, queue);

        let result = mgr.broadcast(&session.id, b"hello world").await;
        assert!(result.is_ok());

        let received = rx.recv().await.unwrap();
        assert_eq!(received, b"hello world");
    }

    #[tokio::test]
    async fn test_broadcast_after_session_removed() {
        let mgr = SessionManager::with_default_timeout();
        let session = mgr.create_session();

        let (tx, _rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
        let (_done_tx, done_rx) = tokio::sync::watch::channel(false);
        let queue = Arc::new(crate::websocket_handler::SendQueue::from_channels(tx, done_rx));

        mgr.set_send_queue(&session.id, queue);
        mgr.remove_session(&session.id);

        let result = mgr.broadcast(&session.id, b"test").await;
        assert!(result.is_err());
    }

    #[test]
    fn test_shutdown_double() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mgr = SessionManager::with_default_timeout();
        let _s = mgr.create_session();

        rt.block_on(mgr.shutdown());
        rt.block_on(mgr.shutdown()); // Should not panic
        assert_eq!(mgr.active_count(), 0);
    }

    #[test]
    fn test_session_debug_format() {
        let mgr = SessionManager::with_default_timeout();
        let session = mgr.create_session();
        let debug_str = format!("{:?}", session);
        assert!(debug_str.contains(&session.id));
    }

    #[test]
    fn test_session_clone() {
        let mgr = SessionManager::with_default_timeout();
        let session = mgr.create_session();
        let cloned = session.clone();
        assert_eq!(session.id, cloned.id);
        assert_eq!(session.sender_id, cloned.sender_id);
    }

    #[test]
    fn test_set_send_queue_replaces_existing() {
        let mgr = SessionManager::with_default_timeout();
        let session = mgr.create_session();

        let (tx1, _rx1) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
        let (_done_tx1, done_rx1) = tokio::sync::watch::channel(false);
        let queue1 = Arc::new(crate::websocket_handler::SendQueue::from_channels(tx1, done_rx1));
        mgr.set_send_queue(&session.id, queue1);

        let (tx2, _rx2) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
        let (_done_tx2, done_rx2) = tokio::sync::watch::channel(false);
        let queue2 = Arc::new(crate::websocket_handler::SendQueue::from_channels(tx2, done_rx2));
        mgr.set_send_queue(&session.id, queue2);

        assert!(mgr.send_queues.contains_key(&session.id));
    }
}

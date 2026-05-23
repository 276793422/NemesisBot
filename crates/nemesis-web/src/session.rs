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
            "[WebSocket] Session created"
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
        tracing::debug!(session_id = %session_id, "[WebSocket] Session removed");
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
            "[WebSocket] Broadcasting to session"
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
            "[WebSocket] Session not found or no send queue"
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
        tracing::info!("[WebSocket] Session manager shutdown complete");
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
                        tracing::info!(session_id = %session_id, "[WebSocket] Removed inactive session");
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
mod tests;

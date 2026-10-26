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
use chrono::{DateTime, Local};
use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

// ---------------------------------------------------------------------------
// Auth method (how a WebSocket session authenticated)
// ---------------------------------------------------------------------------

/// How a WebSocket session authenticated.
///
/// Used to gate certain WSAPI commands (e.g. `workflow.set_chat_password`)
/// to dashboard-only access — a session that connected via the standalone
/// workflow-chat page must not be able to change passwords.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AuthMethod {
    /// Authenticated via the dashboard `token` query param (or no auth configured).
    #[default]
    Dashboard,
    /// Authenticated via `workflow_chat=<index>&pwd=<password>` query params
    /// on the standalone `/workflow/chat/<index>` page. Carries the workflow
    /// name resolved from the index at upgrade time — the session is bound to
    /// that workflow for its whole lifetime and may not target any other
    /// workflow nor use dashboard-only WSAPI modules.
    WorkflowChat {
        /// Workflow name resolved from the chat index at upgrade time.
        workflow: String,
    },
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

/// Session metadata (no WebSocket dependency for portability).
#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub sender_id: String,
    pub chat_id: String,
    pub created_at: DateTime<Local>,
    pub last_active: DateTime<Local>,
    /// How this session authenticated at WS upgrade time.
    pub auth_method: AuthMethod,
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
        self.create_session_with_method(AuthMethod::default())
    }

    /// Create a new session with the given auth method.
    pub fn create_session_with_method(&self, auth_method: AuthMethod) -> Session {
        let id = generate_session_id();
        let sender_id = format!("web:{}", id);
        let chat_id = sender_id.clone();
        let now = Local::now();

        let session = Session {
            id: id.clone(),
            sender_id,
            chat_id,
            created_at: now,
            last_active: now,
            auth_method,
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
            entry.session.last_active = Local::now();
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
        // BUG-20261025-84：先 clone Arc 归还 DashMap 分片读守卫，再进入
        // 背压 await。旧写法 Ref 守卫跨越 `queue.send().await`——hi 通道
        // 满（慢客户端）时该 await 可等待任意久（「满则背压等待，绝不
        // 丢」），同分片的全部写操作（remove_session / set_send_queue /
        // evict_inactive 的 remove）停摆到慢客户端消费为止；而
        // `evict_inactive` 跑在单线程 cleanup runtime 上，撞同分片 =
        // GC 线程整体冻结（全部会话逐出停摆）。同库纪律先例：
        // chat_event_log「广播前放锁：publish 内部可能再取全局资源，
        // 不持锁跨调用」。broadcast_droppable 走同步 try_send 非阻塞，
        // 守卫只持有微秒级，不需同改。
        let queue = self
            .send_queues
            .get(session_id)
            .map(|q| Arc::clone(q.value()));
        if let Some(queue) = queue {
            self.touch_session(session_id);
            return queue.send(message.to_vec()).await;
        }

        // No send queue means session has no active WebSocket
        tracing::warn!(
            session_id = %session_id,
            message_len = message.len(),
            "[WebSocket] Session not found or no send queue"
        );
        Err(format!(
            "session not found or no send queue: {}",
            session_id
        ))
    }

    /// 可丢帧广播（非阻塞）：与 [`Self::broadcast`] 同查找逻辑，但走
    /// SendQueue 的 lo 通道——**满即丢，绝不等待**。调用前提：帧已落
    /// `chat_event_log` 环（带 seq），丢失可被前端 `chat.sync` 补拉自愈；
    /// 未落环的帧必须走 `broadcast`。慢客户端隔离墙（BUG 2026-09-22）。
    pub fn broadcast_droppable(&self, session_id: &str, message: &[u8]) -> Result<(), String> {
        if let Some(queue) = self.send_queues.get(session_id) {
            self.touch_session(session_id);
            return queue.send_droppable(message.to_vec());
        }

        // 无队列 = 无活跃 WS：droppable 语义下降为 debug（丢帧可补拉，
        // 与 broadcast 的 warn 不同——单客户端断连不该刷日志）。
        tracing::debug!(
            session_id = %session_id,
            "[WebSocket] droppable: session not found or no send queue"
        );
        Err(format!(
            "session not found or no send queue: {}",
            session_id
        ))
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
                    Self::evict_inactive(&sessions, &send_queues, timeout, Local::now());
                }
            });
        });

        // Store handle for cleanup on shutdown.
        // Use a tokio JoinHandle wrapping the std::thread so we can abort it.
        // Since we can't easily convert, we just store a dummy and drop the std handle.
        // The cleanup thread will run until the DashMap is dropped.
        let _ = handle; // Thread will terminate when DashMaps are dropped
    }

    /// 逐出非活跃会话并**关闭其仍存活的发送队列**（返回逐出数）。
    ///
    /// 为什么逐出必须断连（2026-10-24）：cleanup 的本意是 GC 死连接的残骸，
    /// 但 WS 连接 halves 在 handler 手里——只删映射不断连，会产生僵尸态：
    /// socket 仍活、入站照收（WSAPI 响应走 dispatch 捕获的队列照常送达），
    /// 而一切会话路由的帧（agent 回复 send_to_session / user 回声 / 历史
    /// 响应 / push 推送）在 manager 查表失败后静默丢失。无心跳的客户端
    /// （自动化脚本、第三方集成；dashboard 有 30s 心跳不受影响）静默 1h
    /// 即落入此态且永无自愈路径。关闭 = 发 Close 帧走正常握手 → 前端
    /// onclose 非 1008/4001 自动重连拿新会话，状态重归一致。
    ///
    /// 单独成关联函数：cleanup 循环 300s 一拍，测试等不起，直驱验证。
    fn evict_inactive(
        sessions: &DashMap<String, SessionEntry>,
        send_queues: &DashMap<String, Arc<SendQueue>>,
        timeout: Duration,
        now: DateTime<Local>,
    ) -> usize {
        let mut to_remove = Vec::new();
        for entry in sessions.iter() {
            let inactive_duration = now.signed_duration_since(entry.session.last_active);
            let inactive_secs = inactive_duration.num_seconds();
            if inactive_secs > timeout.as_secs() as i64 {
                to_remove.push(entry.session.id.clone());
            }
        }

        for session_id in &to_remove {
            // 先关队列再删映射：close 触发 Close 帧握手，随 writer 退出
            // hi/lo receiver 被 drop，残余 send()/send_droppable() 诚实报错
            // （不会把帧静默排进无人消费的通道）。
            if let Some(queue) = send_queues.get(session_id) {
                queue.close();
            }
            sessions.remove(session_id);
            send_queues.remove(session_id);
            tracing::info!(session_id = %session_id, "[WebSocket] Removed inactive session (queue closed)");
        }
        to_remove.len()
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

// S10b (2026-08-26, quality-hardening goal 冲刺 web 批次 2): stats /
// all_sessions / shutdown / no-queue broadcast arms.
#[cfg(test)]
mod s10b_tests;

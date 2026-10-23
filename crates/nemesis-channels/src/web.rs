//! Web channel - WebSocket-based chat channel.
//!
//! Mirrors Go's `module/channels/web.go`. Uses dependency injection via
//! `WebServerOps` trait to avoid circular dependencies with nemesis-web.

use async_trait::async_trait;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{debug, error, info, warn};

use nemesis_types::channel::OutboundMessage;
use nemesis_types::error::{NemesisError, Result};

use crate::base::{BaseChannel, Channel};

// ---------------------------------------------------------------------------
// Web server operations trait (dependency injection)
// ---------------------------------------------------------------------------

/// Trait for web server operations needed by WebChannel.
///
/// Implemented by nemesis-web's WebServer or a test mock. This avoids
/// a direct dependency from nemesis-channels on nemesis-web.
pub trait WebServerOps: Send + Sync {
    /// Send a message to a specific WebSocket session.
    ///
    /// `model` carries the producing model (`provider/name`) for the web
    /// channel's per-message "供应商·模型名" badge; `None` for non-assistant /
    /// badge-less deliveries. Implementations that don't care (test mocks)
    /// accept it via `_: Option<&str>`.
    ///
    /// `session_key`（L2，optional）为 agent 会话键——chat_event_log 断线补拉
    /// 环形缓冲的记录键；`None` 回退连接级 session_id。
    ///
    /// `source_node`（2026-09-23，optional）为集群续行归属——实际干活的
    /// worker 节点名，web 前端据此渲染「节点 X」徽章；`None`（非集群回复）
    /// 不写帧键，与历史行为逐字节一致。
    fn send_to_session(
        &self,
        session_id: &str,
        role: &str,
        content: &str,
        model: Option<&str>,
        session_key: Option<&str>,
        source_node: Option<&str>,
    ) -> std::result::Result<(), String>;

    /// Send history content to a specific session.
    fn send_history_to_session(
        &self,
        session_id: &str,
        content: &str,
    ) -> std::result::Result<(), String>;

    /// Broadcast a message to all active sessions.
    fn broadcast(&self, content: &str) -> std::result::Result<(), String>;

    /// Get all active session IDs.
    fn active_session_ids(&self) -> Vec<String>;

    /// Start the web server.
    fn start_server(&self) -> std::result::Result<(), String>;

    /// Stop the web server.
    fn stop_server(&self);

    /// Set the workspace path for API handlers.
    /// Mirrors Go's `web.Server.SetWorkspace()`.
    fn set_workspace(&self, workspace: &str) {
        let _ = workspace; // default no-op
    }

    /// Set the current LLM model name for display/status purposes.
    /// Mirrors Go's `web.Server.SetModelName()`.
    fn set_model_name(&self, name: &str) {
        let _ = name; // default no-op
    }
}

// ---------------------------------------------------------------------------
// Web channel configuration
// ---------------------------------------------------------------------------

/// Web channel configuration.
#[derive(Debug, Clone)]
pub struct WebChannelConfig {
    pub host: String,
    pub port: u16,
    pub ws_path: String,
    pub auth_token: String,
    pub session_timeout_secs: u64,
    pub allow_from: Vec<String>,
}

impl Default for WebChannelConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 8080,
            ws_path: "/ws".to_string(),
            auth_token: String::new(),
            session_timeout_secs: 3600,
            allow_from: vec![],
        }
    }
}

// ---------------------------------------------------------------------------
// WebChannel
// ---------------------------------------------------------------------------

/// A WebSocket-based chat channel that integrates with a web server.
///
/// Handles:
/// - Starting/stopping the web server via injected `WebServerOps`
/// - Routing outbound messages to WebSocket sessions
/// - Broadcasting to all active sessions
/// - Session-based message routing (web:<session-id>)
pub struct WebChannel {
    base: BaseChannel,
    config: WebChannelConfig,
    server: parking_lot::RwLock<Option<Arc<dyn WebServerOps>>>,
    running: AtomicBool,
    /// C4（2026-10-24 死会话软失败）：已做过「目标会话不存在」一次性
    /// WARN 的 session_id 集合。标签页关闭/重连后旧 chat_id 失效是常态
    /// （ConvRouter latest-wins 设计的预期后果，正文已落 chat_log 可经
    /// 历史加载恢复），逐轮 ERROR 刷屏无信息量——每目标只 WARN 一次，
    /// 后续降 debug。
    dead_targets: parking_lot::Mutex<std::collections::HashSet<String>>,
}

impl WebChannel {
    /// Creates a new `WebChannel` with the given configuration.
    pub fn new(config: WebChannelConfig) -> Self {
        Self {
            base: BaseChannel::new("web"),
            config,
            server: parking_lot::RwLock::new(None),
            running: AtomicBool::new(false),
            dead_targets: parking_lot::Mutex::new(std::collections::HashSet::new()),
        }
    }

    /// Creates a new `WebChannel` with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(WebChannelConfig::default())
    }

    /// Inject the web server implementation.
    pub fn set_server(&self, ops: Arc<dyn WebServerOps>) {
        *self.server.write() = Some(ops);
    }

    /// C4（2026-10-24）：死目标会话一次性留痕——同 session_id 首次 WARN
    /// （带恢复路径说明），之后 debug。跨消息去重靠 `dead_targets` 集合。
    fn note_dead_target(&self, session_id: &str, chat_id: &str) {
        let first = self.dead_targets.lock().insert(session_id.to_string());
        if first {
            warn!(
                session_id = %session_id,
                chat_id = %chat_id,
                "[WebChannel] outbound target session is gone (tab closed / reconnected?); message kept in chat history only, further reports for this target downgraded to debug"
            );
        } else {
            debug!(
                session_id = %session_id,
                chat_id = %chat_id,
                "[WebChannel] outbound target session still gone; message kept in chat history only"
            );
        }
    }

    /// Get a reference to the web server implementation, if set.
    ///
    /// Mirrors Go's `WebChannel.GetServer()`. Returns an `Option<Arc<..>>`
    /// so callers can interact with the server for external integration.
    pub fn get_server(&self) -> Option<Arc<dyn WebServerOps>> {
        self.server.read().clone()
    }

    /// Set the workspace path on the web server for API handlers.
    ///
    /// Mirrors Go's `WebChannel.SetWorkspace()`. Delegates to the server's
    /// `set_workspace()` method if a server is configured.
    pub fn set_workspace(&self, workspace: &str) {
        let server = self.server.read();
        if let Some(srv) = server.as_ref() {
            srv.set_workspace(workspace);
        }
    }

    /// Set the current LLM model name on the web server.
    ///
    /// Mirrors Go's `WebChannel.SetModelName()`. Delegates to the server's
    /// `set_model_name()` method if a server is configured.
    pub fn set_model_name(&self, name: &str) {
        let server = self.server.read();
        if let Some(srv) = server.as_ref() {
            srv.set_model_name(name);
        }
    }

    /// Returns whether the channel is running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Broadcasts a message to all active web sessions.
    pub fn broadcast_to_all(&self, content: &str) -> Result<()> {
        let server = self.server.read();
        if let Some(srv) = server.as_ref() {
            srv.broadcast(content).map_err(NemesisError::Channel)
        } else {
            warn!("[WebChannel] no web server configured for broadcast");
            Ok(())
        }
    }

    /// Returns the listen address.
    pub fn listen_addr(&self) -> String {
        format!("{}:{}", self.config.host, self.config.port)
    }
}

impl Default for WebChannel {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[async_trait]
impl Channel for WebChannel {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn is_running(&self) -> bool {
        self.base.is_running()
    }

    async fn start(&self) -> Result<()> {
        info!(
            host = %self.config.host,
            port = self.config.port,
            path = %self.config.ws_path,
            "[WebChannel] starting"
        );

        // Start the web server if configured
        {
            let server = self.server.read();
            if let Some(srv) = server.as_ref() {
                srv.start_server().map_err(|e| {
                    error!(error = %e, "[WebChannel] server failed to start");
                    NemesisError::Channel(e)
                })?;
            }
        }

        // Brief wait to ensure server starts
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        self.running.store(true, Ordering::SeqCst);
        self.base.set_enabled(true);

        info!(
            url = format!("http://{}:{}", self.config.host, self.config.port),
            "[WebChannel] started"
        );

        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("[WebChannel] stopping");

        self.running.store(false, Ordering::SeqCst);
        self.base.set_enabled(false);

        let server = self.server.read();
        if let Some(srv) = server.as_ref() {
            srv.stop_server();
        }

        info!("[WebChannel] stopped");
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !self.is_running() {
            warn!(
                chat_id = %msg.chat_id,
                content_len = msg.content.len(),
                "[WebChannel] not running, cannot send message"
            );
            return Err(NemesisError::Channel("web channel not running".to_string()));
        }

        self.base.record_sent();

        let server = self.server.read();
        let srv = match server.as_ref() {
            Some(s) => s,
            None => {
                warn!("[WebChannel] no web server configured, dropping message");
                return Ok(());
            }
        };

        // Handle broadcast to all sessions
        if msg.chat_id == "web:broadcast" {
            debug!(
                content_len = msg.content.len(),
                "[WebChannel] broadcasting to all sessions"
            );
            return srv.broadcast(&msg.content).map_err(NemesisError::Channel);
        }

        // Extract session ID from chat ID (format: web:<session-id>)
        let session_id = if msg.chat_id.starts_with("web:") {
            &msg.chat_id[4..]
        } else {
            error!(
                chat_id = %msg.chat_id,
                expected_format = "web:<session-id>",
                "[WebChannel] invalid chat ID format"
            );
            return Err(NemesisError::Channel(format!(
                "invalid chat ID format: {}",
                msg.chat_id
            )));
        };

        // Handle history responses via dedicated method
        if msg.message_type == "history" {
            debug!(session_id = %session_id, "[WebChannel] sending history to session");
            return srv
                .send_history_to_session(session_id, &msg.content)
                .map_err(NemesisError::Channel);
        }

        // Send message to session
        debug!(
            session_id = %session_id,
            chat_id = %msg.chat_id,
            content_len = msg.content.len(),
            "[WebChannel] sending message to session"
        );

        if let Err(e) = srv.send_to_session(
            session_id,
            "assistant",
            &msg.content,
            msg.meta.model.as_deref(),
            msg.meta.session_key.as_deref(),
            msg.meta.source_node.as_deref(),
        ) {
            // C4（2026-10-24 死会话软失败）：目标会话已不在（标签页关闭/
            // 重连换新 session id，ConvRouter latest-wins 残留旧绑定）——
            // 不进 manager 错误路径（dispatch loop WARN + 重试箱），一次性
            // WARN + 后续 debug，返回 Ok。正文已由 agent 落 chat_log，历史
            // 加载即恢复。字符串是 nemesis-web session.rs 的稳定产物。
            if e.contains("session not found or no send queue") {
                self.note_dead_target(session_id, &msg.chat_id);
                return Ok(());
            }
            error!(
                error = %e,
                session_id = %session_id,
                chat_id = %msg.chat_id,
                "[WebChannel] failed to send message to session"
            );
            return Err(NemesisError::Channel(e));
        }

        info!(
            session_id = %session_id,
            chat_id = %msg.chat_id,
            "[WebChannel] message sent to session successfully"
        );

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

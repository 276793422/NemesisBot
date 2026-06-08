//! Web channel - WebSocket-based chat channel.
//!
//! Mirrors Go's `module/channels/web.go`. Uses dependency injection via
//! `WebServerOps` trait to avoid circular dependencies with nemesis-web.

use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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
    fn send_to_session(&self, session_id: &str, role: &str, content: &str) -> std::result::Result<(), String>;

    /// Send history content to a specific session.
    fn send_history_to_session(&self, session_id: &str, content: &str) -> std::result::Result<(), String>;

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
}

impl WebChannel {
    /// Creates a new `WebChannel` with the given configuration.
    pub fn new(config: WebChannelConfig) -> Self {
        Self {
            base: BaseChannel::new("web"),
            config,
            server: parking_lot::RwLock::new(None),
            running: AtomicBool::new(false),
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
            srv.broadcast(content).map_err(|e| NemesisError::Channel(e))
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
            debug!(content_len = msg.content.len(), "[WebChannel] broadcasting to all sessions");
            return srv.broadcast(&msg.content).map_err(|e| NemesisError::Channel(e));
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
            return Err(NemesisError::Channel(format!("invalid chat ID format: {}", msg.chat_id)));
        };

        // Handle history responses via dedicated method
        if msg.message_type == "history" {
            debug!(session_id = %session_id, "[WebChannel] sending history to session");
            return srv
                .send_history_to_session(session_id, &msg.content)
                .map_err(|e| NemesisError::Channel(e));
        }

        // Send message to session
        debug!(
            session_id = %session_id,
            chat_id = %msg.chat_id,
            content_len = msg.content.len(),
            "[WebChannel] sending message to session"
        );

        if let Err(e) = srv.send_to_session(session_id, "assistant", &msg.content) {
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

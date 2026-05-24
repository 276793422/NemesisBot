//! WebSocket API router for request/response dispatch.
//!
//! Provides a modular handler registry where each module (models, channels, etc.)
//! registers a `ModuleHandler`. Incoming `type="request"` messages are dispatched
//! to the matching handler, and responses (with `reqId` correlation) are sent back.

use crate::api_handlers::AppState;
use crate::protocol::ProtocolMessage;
use crate::websocket_handler::SendQueue;
use std::collections::HashMap;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// ModuleHandler trait
// ---------------------------------------------------------------------------

/// A handler for a specific module's commands.
///
/// Implementations contain business logic and are transport-agnostic.
#[async_trait::async_trait]
pub trait ModuleHandler: Send + Sync {
    /// The module name this handler responds to (e.g., "models", "channels").
    fn module_name(&self) -> &str;

    /// Handle a command within this module.
    ///
    /// Returns `Ok(Some(data))` for success with payload, `Ok(None)` for success
    /// with no payload, or `Err(msg)` for failures.
    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String>;
}

// ---------------------------------------------------------------------------
// Request context
// ---------------------------------------------------------------------------

/// Context provided to each handler invocation.
#[derive(Clone)]
pub struct RequestContext {
    /// The WebSocket session ID.
    pub session_id: String,
    /// Optional workspace path.
    pub workspace: Option<String>,
    /// Home directory where config.json resides.
    pub home: Option<String>,
    /// Shared application state.
    pub state: Arc<AppState>,
}

// ---------------------------------------------------------------------------
// WsRouter
// ---------------------------------------------------------------------------

/// Router that dispatches `type="request"` messages to the appropriate module handler.
pub struct WsRouter {
    handlers: HashMap<String, Arc<dyn ModuleHandler>>,
}

impl WsRouter {
    /// Create a new empty router.
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    /// Register a module handler.
    pub fn register(&mut self, handler: Arc<dyn ModuleHandler>) {
        self.handlers.insert(handler.module_name().to_string(), handler);
    }

    /// Dispatch a request message to the appropriate handler and send the response.
    ///
    /// If no handler is found for the module, sends an error response.
    pub async fn dispatch(
        &self,
        msg: &ProtocolMessage,
        ctx: &RequestContext,
        send_queue: &SendQueue,
    ) {
        let req_id = msg.req_id.as_deref().unwrap_or("");

        let result = match self.handlers.get(&msg.module) {
            Some(handler) => handler.handle_cmd(&msg.cmd, msg.data.clone(), ctx).await,
            None => Err(format!("unknown module: {}", msg.module)),
        };

        let response = match result {
            Ok(data) => ProtocolMessage::response_ok(&msg.module, &msg.cmd, req_id, data),
            Err(e) => ProtocolMessage::response_err(&msg.module, &msg.cmd, req_id, &e),
        };

        if let Ok(bytes) = response.to_json() {
            if let Err(e) = send_queue.send(bytes).await {
                tracing::warn!(
                    req_id = %req_id,
                    error = %e,
                    "[WebSocket] Failed to send WS API response"
                );
            }
        }
    }
}

impl Default for WsRouter {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

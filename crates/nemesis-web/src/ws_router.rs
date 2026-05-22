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
mod tests {
    use super::*;
    use crate::api_handlers::AppState;
    use crate::events::EventHub;
    use crate::session::SessionManager;
    use crate::websocket_handler::IncomingMessage;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::time::Instant;
    use tokio::sync::mpsc;

    /// A simple test handler for the "test" module.
    struct TestHandler {
        module: String,
    }

    impl TestHandler {
        fn new(module: &str) -> Self {
            Self { module: module.to_string() }
        }
    }

    #[async_trait::async_trait]
    impl ModuleHandler for TestHandler {
        fn module_name(&self) -> &str {
            &self.module
        }

        async fn handle_cmd(
            &self,
            cmd: &str,
            _data: Option<serde_json::Value>,
            _ctx: &RequestContext,
        ) -> Result<Option<serde_json::Value>, String> {
            match cmd {
                "ping" => Ok(Some(serde_json::json!({"pong": true}))),
                "fail" => Err("intentional failure".to_string()),
                "noop" => Ok(None),
                _ => Err(format!("unknown cmd: {}", cmd)),
            }
        }
    }

    fn make_test_state() -> Arc<AppState> {
        Arc::new(AppState {
            auth_token: String::new(),
            session_count: Arc::new(AtomicUsize::new(0)),
            workspace: None,
            version: "test".to_string(),
            start_time: Instant::now(),
            model_name: Arc::new(parking_lot::Mutex::new("test-model".to_string())),
            event_hub: Arc::new(EventHub::new()),
            running: Arc::new(AtomicBool::new(true)),
            session_manager: Arc::new(SessionManager::with_default_timeout()),
            inbound_tx: None,
            streaming_provider: None,
            ws_router: None,
        })
    }

    #[test]
    fn test_router_new() {
        let router = WsRouter::new();
        assert!(router.handlers.is_empty());
    }

    #[test]
    fn test_router_register() {
        let mut router = WsRouter::new();
        router.register(Arc::new(TestHandler::new("test")));
        assert!(router.handlers.contains_key("test"));
    }

    #[test]
    fn test_router_default() {
        let router = WsRouter::default();
        assert!(router.handlers.is_empty());
    }

    #[tokio::test]
    async fn test_dispatch_unknown_module() {
        let router = WsRouter::new();
        let state = make_test_state();
        let ctx = RequestContext {
            session_id: "s1".to_string(),
            workspace: None,
            state,
        };

        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(16);
        let (_, done_rx) = tokio::sync::watch::channel(false);
        let send_queue = SendQueue::from_channels(tx, done_rx);

        let msg = ProtocolMessage::request("nonexistent", "cmd", "req-1", None);
        router.dispatch(&msg, &ctx, &send_queue).await;

        let response_bytes = rx.recv().await.unwrap();
        let resp: serde_json::Value = serde_json::from_slice(&response_bytes).unwrap();
        assert_eq!(resp["type"], "response");
        assert_eq!(resp["reqId"], "req-1");
        assert!(resp["error"].as_str().unwrap().contains("unknown module"));
    }

    #[tokio::test]
    async fn test_dispatch_success() {
        let mut router = WsRouter::new();
        router.register(Arc::new(TestHandler::new("test")));
        let state = make_test_state();
        let ctx = RequestContext {
            session_id: "s1".to_string(),
            workspace: None,
            state,
        };

        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(16);
        let (_, done_rx) = tokio::sync::watch::channel(false);
        let send_queue = SendQueue::from_channels(tx, done_rx);

        let msg = ProtocolMessage::request("test", "ping", "req-2", None);
        router.dispatch(&msg, &ctx, &send_queue).await;

        let response_bytes = rx.recv().await.unwrap();
        let resp: serde_json::Value = serde_json::from_slice(&response_bytes).unwrap();
        assert_eq!(resp["type"], "response");
        assert_eq!(resp["reqId"], "req-2");
        assert!(resp["error"].is_null());
        assert_eq!(resp["data"]["pong"], true);
    }

    #[tokio::test]
    async fn test_dispatch_handler_error() {
        let mut router = WsRouter::new();
        router.register(Arc::new(TestHandler::new("test")));
        let state = make_test_state();
        let ctx = RequestContext {
            session_id: "s1".to_string(),
            workspace: None,
            state,
        };

        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(16);
        let (_, done_rx) = tokio::sync::watch::channel(false);
        let send_queue = SendQueue::from_channels(tx, done_rx);

        let msg = ProtocolMessage::request("test", "fail", "req-3", None);
        router.dispatch(&msg, &ctx, &send_queue).await;

        let response_bytes = rx.recv().await.unwrap();
        let resp: serde_json::Value = serde_json::from_slice(&response_bytes).unwrap();
        assert_eq!(resp["type"], "response");
        assert_eq!(resp["reqId"], "req-3");
        assert_eq!(resp["error"], "intentional failure");
    }

    #[tokio::test]
    async fn test_dispatch_no_data_response() {
        let mut router = WsRouter::new();
        router.register(Arc::new(TestHandler::new("test")));
        let state = make_test_state();
        let ctx = RequestContext {
            session_id: "s1".to_string(),
            workspace: None,
            state,
        };

        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(16);
        let (_, done_rx) = tokio::sync::watch::channel(false);
        let send_queue = SendQueue::from_channels(tx, done_rx);

        let msg = ProtocolMessage::request("test", "noop", "req-4", None);
        router.dispatch(&msg, &ctx, &send_queue).await;

        let response_bytes = rx.recv().await.unwrap();
        let resp: serde_json::Value = serde_json::from_slice(&response_bytes).unwrap();
        assert_eq!(resp["type"], "response");
        assert!(resp["data"].is_null());
    }

    #[tokio::test]
    async fn test_dispatch_req_id_roundtrip() {
        let mut router = WsRouter::new();
        router.register(Arc::new(TestHandler::new("mymod")));
        let state = make_test_state();
        let ctx = RequestContext {
            session_id: "s1".to_string(),
            workspace: None,
            state,
        };

        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(16);
        let (_, done_rx) = tokio::sync::watch::channel(false);
        let send_queue = SendQueue::from_channels(tx, done_rx);

        let custom_id = "uuid-abc-123-def";
        let msg = ProtocolMessage::request("mymod", "ping", custom_id, None);
        router.dispatch(&msg, &ctx, &send_queue).await;

        let response_bytes = rx.recv().await.unwrap();
        let resp: serde_json::Value = serde_json::from_slice(&response_bytes).unwrap();
        assert_eq!(resp["reqId"], custom_id);
    }
}

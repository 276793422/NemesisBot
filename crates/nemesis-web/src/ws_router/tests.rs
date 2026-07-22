use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;
use tokio::sync::mpsc;

/// A simple test handler for the "test" module.
struct TestHandler {
    module: String,
}

impl TestHandler {
    fn new(module: &str) -> Self {
        Self {
            module: module.to_string(),
        }
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
        home: None,
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("test-model".to_string())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
        session_manager: Arc::new(SessionManager::with_default_timeout()),
        inbound_tx: None,
        streaming_provider: None,
        ws_router: None,
        agent_service: None,
        data_store: None,
        memory_manager: None,
        forge: None,
        agent_loop: Arc::new(parking_lot::RwLock::new(None)),
        cluster: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
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
        chat_id: "test-chat".to_string(),
        workspace: None,
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
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
        chat_id: "test-chat".to_string(),
        workspace: None,
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
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
        chat_id: "test-chat".to_string(),
        workspace: None,
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
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
        chat_id: "test-chat".to_string(),
        workspace: None,
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
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
        chat_id: "test-chat".to_string(),
        workspace: None,
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
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

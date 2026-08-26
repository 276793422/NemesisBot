//! S10b (quality-hardening goal 冲刺, web 批次 2): WsRouter::dispatch's
//! send-failure arm — a SendQueue whose receiver is dropped makes the response
//! send fail; dispatch must warn and return without panicking.

use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::protocol::ProtocolMessage;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext, WsRouter};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

struct EchoHandler;

#[async_trait::async_trait]
impl ModuleHandler for EchoHandler {
    fn module_name(&self) -> &str {
        "echo"
    }

    async fn handle_cmd(
        &self,
        _cmd: &str,
        data: Option<serde_json::Value>,
        _ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        Ok(data)
    }
}

fn make_ctx() -> RequestContext {
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: None,
        home: None,
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new(String::new())),
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
        #[cfg(feature = "workflow")]
        chat_secret_store: Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
    });
    RequestContext {
        session_id: "s10b".to_string(),
        chat_id: "chat".to_string(),
        workspace: None,
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

#[tokio::test]
async fn dispatch_with_dead_send_queue_warns_and_survives() {
    let mut router = WsRouter::new();
    router.register(Arc::new(EchoHandler));

    // Build a SendQueue whose receiving side is already dropped → the
    // response send fails into the warn arm.
    let (tx, rx) = tokio::sync::mpsc::channel::<Vec<u8>>(8);
    drop(rx);
    let (_, done_rx) = tokio::sync::watch::channel(false);
    let queue = crate::websocket_handler::SendQueue::from_channels(tx, done_rx);

    let msg = ProtocolMessage::request("echo", "ping", "req-9", Some(serde_json::json!({"x":1})));

    // Must complete (not hang, not panic) despite the dead queue.
    let ctx = make_ctx();
    tokio::time::timeout(std::time::Duration::from_secs(5), router.dispatch(&msg, &ctx, &queue))
        .await
        .expect("dispatch returns promptly on send failure");
}

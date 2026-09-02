//! S10b (quality-hardening goal 冲刺, web 批次 2): handle_chat_stream arms the
//! existing extra tests skip — explicit `model` passthrough to the provider
//! (vs. the empty-model default arm), and a backend stream that ends WITHOUT
//! finish_reason / [DONE] (handler still emits its own done event).

use crate::sse_chat::handle_chat_stream;
use nemesis_providers::http_provider::{HttpProvider, HttpProviderConfig};
use std::collections::HashMap;
use std::sync::Arc;

fn make_state(provider: Option<Arc<HttpProvider>>) -> Arc<crate::api_handlers::AppState> {
    use crate::api_handlers::AppState;
    use crate::events::EventHub;
    use crate::session::SessionManager;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::time::Instant;

    Arc::new(AppState {
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
        streaming_provider: provider,
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
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: std::sync::Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
        board: None,
    })
}

fn provider_for(base_url: String) -> Arc<HttpProvider> {
    Arc::new(HttpProvider::new(HttpProviderConfig {
        name: "s10b".to_string(),
        base_url,
        api_key: "k".to_string(),
        default_model: "default-m".to_string(),
        timeout_secs: 10,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    }))
}

async fn post_stream(state: Arc<crate::api_handlers::AppState>, body: serde_json::Value) -> String {
    let app = axum::Router::new()
        .route("/api/chat/stream", axum::routing::post(handle_chat_stream))
        .with_state(state);
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/api/chat/stream")
        .header("content-type", "application/json")
        .body(serde_json::to_string(&body).unwrap())
        .unwrap();
    let resp = tower::ServiceExt::oneshot(app, req).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).to_string()
}

#[tokio::test]
async fn explicit_model_passes_through_to_provider() {
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"A\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_partial_json(serde_json::json!({
            "stream": true,
            "model": "explicit-model-x"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_string(sse))
        .expect(1)
        .mount(&server)
        .await;

    let body = post_stream(
        make_state(Some(provider_for(server.uri()))),
        serde_json::json!({
            "messages": [{"role": "user", "content": "hi"}],
            "model": "explicit-model-x"
        }),
    )
    .await;
    assert!(body.contains("\"delta\":\"A\""), "{body}");
    assert!(body.contains("event: done"), "{body}");
}

#[tokio::test]
async fn unterminated_backend_stream_still_gets_done_event() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    // One delta chunk, NO finish_reason and NO [DONE] terminator — the HTTP
    // body just ends, so the provider channel closes without a stop chunk.
    let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"tail\"}}]}\n\n";
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_string(sse))
        .expect(1)
        .mount(&server)
        .await;

    let body = post_stream(
        make_state(Some(provider_for(server.uri()))),
        serde_json::json!({ "messages": [{"role": "user", "content": "hi"}] }),
    )
    .await;
    assert!(body.contains("\"delta\":\"tail\""), "{body}");
    assert!(
        body.contains("event: done") && body.contains("data: [DONE]"),
        "handler appends its own done marker: {body}"
    );
    assert!(!body.contains("event: error"), "{body}");
}

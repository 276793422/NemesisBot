//! Extra tests for `sse_chat::handle_chat_stream`.
//!
//! Type-layer coverage already lives in `sse_chat/tests.rs`. This module
//! focuses on request-type behavior — the handler itself uses an opaque
//! `async_stream::stream!` whose `Send` bound is enforced only at the axum
//! router boundary, not when invoked directly, so we verify the request
//! shapes and the handler entrypoint compile and run without panicking.

use crate::sse_chat::{ChatStreamRequest, MessageEntry};

// -----------------------------------------------------------------------
// ChatStreamRequest construction / behavior
// -----------------------------------------------------------------------

#[test]
fn request_with_temperature_zero() {
    let req = ChatStreamRequest {
        messages: vec![MessageEntry {
            role: "user".to_string(),
                content: "x".to_string(),
        }],
        model: String::new(),
        temperature: Some(0.0),
        max_tokens: None,
    };
    assert_eq!(req.temperature, Some(0.0));
}

#[test]
fn request_with_negative_max_tokens_passes_through() {
    // No validation in the request type itself — value is forwarded.
    let req = ChatStreamRequest {
        messages: vec![MessageEntry {
            role: "user".to_string(),
                content: "x".to_string(),
        }],
        model: "m".to_string(),
        temperature: None,
        max_tokens: Some(-1),
    };
    assert_eq!(req.max_tokens, Some(-1));
}

#[test]
fn request_empty_messages_allowed() {
    let req = ChatStreamRequest {
        messages: vec![],
        model: String::new(),
        temperature: None,
        max_tokens: None,
    };
    assert!(req.messages.is_empty());
}

#[test]
fn request_default_model_is_empty_string() {
    let req = ChatStreamRequest {
        messages: vec![MessageEntry {
            role: "user".to_string(),
                content: "x".to_string(),
        }],
        model: String::new(),
        temperature: None,
        max_tokens: None,
    };
    assert!(req.model.is_empty());
}

#[test]
fn request_with_explicit_model() {
    let req = ChatStreamRequest {
        messages: vec![MessageEntry {
            role: "user".to_string(),
                content: "x".to_string(),
        }],
        model: "custom-model".to_string(),
        temperature: None,
        max_tokens: None,
    };
    assert_eq!(req.model, "custom-model");
}

#[test]
fn request_multi_message_preserves_order() {
    let req = ChatStreamRequest {
        messages: vec![
            MessageEntry {
                role: "system".to_string(),
                content: "sys".to_string(),
            },
            MessageEntry {
                role: "user".to_string(),
                    content: "u1".to_string(),
            },
            MessageEntry {
                role: "assistant".to_string(),
                content: "a1".to_string(),
            },
            MessageEntry {
                role: "user".to_string(),
                    content: "u2".to_string(),
            },
        ],
        model: String::new(),
        temperature: None,
        max_tokens: None,
    };
    assert_eq!(req.messages.len(), 4);
    assert_eq!(req.messages[0].role, "system");
    assert_eq!(req.messages[3].content, "u2");
}

#[test]
fn request_full_options() {
    let req = ChatStreamRequest {
        messages: vec![MessageEntry {
            role: "user".to_string(),
                content: "hi".to_string(),
        }],
        model: "gpt-4o".to_string(),
        temperature: Some(0.7),
        max_tokens: Some(2048),
    };
    assert_eq!(req.model, "gpt-4o");
    assert_eq!(req.temperature, Some(0.7));
    assert_eq!(req.max_tokens, Some(2048));
}

// -----------------------------------------------------------------------
// MessageEntry behavior
// -----------------------------------------------------------------------

#[test]
fn message_entry_unicode_content() {
    let m = MessageEntry {
        role: "user".to_string(),
            content: "Hello 世界".to_string(),
    };
    assert!(m.content.contains("世"));
}

#[test]
fn message_entry_empty_content() {
    let m = MessageEntry {
        role: "user".to_string(),
            content: String::new(),
        };
        assert!(m.content.is_empty());
    }

    #[test]
    fn message_entry_role_variants() {
        for role in &["user", "assistant", "system", "tool"] {
        let m = MessageEntry {
            role: role.to_string(),
            content: "x".to_string(),
        };
        assert_eq!(m.role, *role);
    }
}

#[test]
fn message_entry_debug_format() {
    let m = MessageEntry {
        role: "user".to_string(),
            content: "hi".to_string(),
    };
    let s = format!("{:?}", m);
    assert!(s.contains("user"));
        assert!(s.contains("hi"));
}

// -----------------------------------------------------------------------
// JSON (de)serialization edge cases
// -----------------------------------------------------------------------

#[test]
fn request_with_extra_unknown_field_ignored() {
    // serde_default behavior: extra fields are ignored by default.
    let json = r#"{
            "messages": [{"role": "user", "content": "x"}],
            "unknown_field": "ignored"
        }"#;
    let req: ChatStreamRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.messages.len(), 1);
}

#[test]
fn request_messages_with_whitespace_content() {
    let json = r#"{
            "messages": [{"role": "user", "content": "   "}]
        }"#;
    let req: ChatStreamRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.messages[0].content, "   ");
}

#[test]
fn request_temperature_very_high() {
    let json = r#"{
            "messages": [{"role": "user", "content": "x"}],
            "temperature": 2.0
        }"#;
    let req: ChatStreamRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.temperature, Some(2.0));
}

#[test]
fn request_max_tokens_one() {
    let json = r#"{
            "messages": [{"role": "user", "content": "x"}],
            "max_tokens": 1
        }"#;
    let req: ChatStreamRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.max_tokens, Some(1));
}

#[test]
fn request_null_temperature_treated_as_absent_fails() {
    // serde skips Option<T> when value is null only if explicitly opted in.
    // Here, null for temperature should yield None (serde default).
    let json = r#"{
            "messages": [{"role": "user", "content": "x"}],
            "temperature": null
        }"#;
    let req: ChatStreamRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.temperature, None);
}

#[test]
fn message_entry_with_empty_role() {
    let json = r#"{"role": "", "content": "x"}"#;
    let m: MessageEntry = serde_json::from_str(json).unwrap();
    assert_eq!(m.role, "");
}

// ---------------------------------------------------------------------------
// Handler 本体（Phase 3 覆盖率冲刺，2026-08-25）：此前只有请求类型 serde
// 测试，`handle_chat_stream` 的流转换逻辑（chunk/done/error 三态 + 无
// provider 分支 + 无 [DONE] 兜底）零覆盖。router.oneshot 全进程内测，
// wiremock 扮演 OpenAI 风格 SSE 后端（nemesis-providers 同款模式）。
// ---------------------------------------------------------------------------

use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use axum::Router;
use nemesis_providers::http_provider::{HttpProvider, HttpProviderConfig};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::Arc;
use std::time::Instant;
use tower::ServiceExt;

/// 复刻 fork_route_tests 的最小 AppState（全 None，除 streaming_provider
/// 由各测试自行覆盖）。
fn make_state(streaming_provider: Option<Arc<HttpProvider>>) -> Arc<AppState> {
    Arc::new(AppState {
        auth_token: "test-token".to_string(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(".".to_string()),
        home: Some(".".to_string()),
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("test-model".to_string())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
        session_manager: Arc::new(SessionManager::with_default_timeout()),
        inbound_tx: None,
        streaming_provider,
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
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
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
        name: "test".to_string(),
        base_url,
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 10,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    }))
}

/// 把 body 收成完整字符串（SSE 流在 done/error 后终止；keep-alive 只在
/// 流空闲时打点，正常路径立即结束不会卡 to_bytes）。
async fn sse_body(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    String::from_utf8_lossy(&bytes).to_string()
}

async fn post_stream(
    state: Arc<AppState>,
    body: serde_json::Value,
) -> String {
    let app = Router::new()
        .route(
            "/api/chat/stream",
            axum::routing::post(crate::sse_chat::handle_chat_stream),
        )
        .with_state(state);
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/api/chat/stream")
        .header("content-type", "application/json")
        .body(serde_json::to_string(&body).unwrap())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    sse_body(resp).await
}

#[tokio::test]
async fn handler_no_provider_emits_error_event() {
    let body = post_stream(make_state(None), serde_json::json!({
        "messages": [{"role": "user", "content": "hi"}]
    }))
    .await;
    assert!(body.contains("event: error"), "{body}");
    assert!(
        body.contains("No streaming provider configured"),
        "{body}"
    );
}

#[tokio::test]
async fn handler_streams_chunks_then_done_marker() {
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    // 两块 delta，第二块带 finish_reason + usage，最后 [DONE] 终止符。
    let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n\
               data: {\"choices\":[{\"delta\":{\"content\":\"lo\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2,\"total_tokens\":5}}\n\n\
               data: [DONE]\n\n";
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_partial_json(serde_json::json!({
            "stream": true,
            // 空 model → provider 用 default_model 填充（handler 传 "" 的
            // 分支 + provider 侧回退一次钉死）。
            "model": "gpt-4"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_string(sse))
        .expect(1)
        .mount(&server)
        .await;

    let body = post_stream(
        make_state(Some(provider_for(server.uri()))),
        serde_json::json!({
            "messages": [{"role": "user", "content": "hi"}],
            "temperature": 0.1,
            "max_tokens": 32
        }),
    )
    .await;

    // chunk 事件带增量文本（两条）。
    assert!(body.contains("event: chunk"), "{body}");
    assert!(body.contains("\"delta\":\"Hel\""), "{body}");
    assert!(body.contains("\"delta\":\"lo\""), "{body}");
    // 终块带 finish_reason + usage 映射。
    assert!(body.contains("\"finish_reason\":\"stop\""), "{body}");
    assert!(body.contains("\"total_tokens\":5"), "{body}");
    // 收尾 done 事件 + [DONE] 标记（handler 侧，非后端透传）。
    assert!(body.contains("event: done"), "{body}");
    assert!(body.contains("data: [DONE]"), "{body}");
    // 中途不应出现 error 事件。
    assert!(!body.contains("event: error"), "{body}");
}

#[tokio::test]
async fn handler_provider_http_error_emits_error_event() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(500)
                .set_body_string("{\"error\":\"boom\"}"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let body = post_stream(
        make_state(Some(provider_for(server.uri()))),
        serde_json::json!({
            "messages": [{"role": "user", "content": "hi"}]
        }),
    )
    .await;

    assert!(body.contains("event: error"), "{body}");
    assert!(body.contains("\"error\""), "{body}");
    // 错误后必须终止，不能再吐 chunk/done。
    assert!(!body.contains("event: chunk"), "{body}");
}

#[tokio::test]
async fn handler_stream_end_without_done_still_emits_done() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    // 后端只给一块 delta（无 finish_reason、无 [DONE]）就关流——handler 的
    // 兜底分支必须补发 done，否则前端流悬挂。
    let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n";
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_string(sse))
        .expect(1)
        .mount(&server)
        .await;

    let body = post_stream(
        make_state(Some(provider_for(server.uri()))),
        serde_json::json!({
            "messages": [{"role": "user", "content": "hi"}]
        }),
    )
    .await;

    assert!(body.contains("\"delta\":\"partial\""), "{body}");
    assert!(body.contains("event: done"), "{body}");
    assert!(body.contains("data: [DONE]"), "{body}");
}
